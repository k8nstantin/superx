import { fetchSessionActivity, type FeedCursor } from './api'
import { mergeFeed } from './Feed'
import type { SessionView } from './generated/SessionView'
import type { SseEvent } from './generated/SseEvent'

// A session as a file (#442). The whole session, not the rows on screen:
// its activity is paged back to the first captured row, then written in
// the shape the reader chose.

export type ExportOptions = {
  /// The conversation (every message), or everything (with the OS's actions).
  include: 'conversation' | 'everything'
  /// A tool's output: whole, its first lines, or a one-line marker.
  tools: 'full' | 'trimmed' | 'none'
  timestamps: boolean
  /// Only rows matching the session view's search (when one is active).
  matching: boolean
  format: 'txt' | 'md'
}

export const DEFAULT_EXPORT: ExportOptions = {
  include: 'conversation',
  tools: 'full',
  timestamps: true,
  matching: false,
  format: 'txt',
}

/// How many lines of a tool's output a trimmed export keeps.
export const TRIMMED_TOOL_LINES = 20
// The server's page bound (ACTIVITY_BACKLOG_MAX).
const EXPORT_PAGE = 2000

/// Every captured row of a session, oldest first: the newest page, then
/// each page before the oldest row held, until a page comes back short.
export async function collectSession(
  sessionId: string,
  q: string | undefined,
  onProgress?: (rows: number) => void,
): Promise<SseEvent[]> {
  let rows: SseEvent[] = []
  let before: FeedCursor | undefined
  for (;;) {
    const page = await fetchSessionActivity(sessionId, EXPORT_PAGE, before, q)
    if (page.length === 0) break
    rows = mergeFeed(page, rows, Number.MAX_SAFE_INTEGER)
    onProgress?.(rows.length)
    const oldest = rows[0]
    // A short page is the beginning; a cursor that did not move would
    // ask for the same page forever.
    if (page.length < EXPORT_PAGE || (before && oldest.id === before.id)) break
    before = { at: oldest.valid_from, id: oldest.id }
  }
  return rows
}

type Entry = { at: Date | null; label: string; lines: string[]; isAction: boolean }

const pad = (n: number) => String(n).padStart(2, '0')

/// The viewer's clock, always with the date: a file outlives the day.
export function localDateTime(d: Date): string {
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
}

/// One row, taken apart: its moment, its label, its text. The renderer
/// the CLI shares writes `[utc] role: text` for a message, continuation
/// lines indented four spaces, and `[hh:mm:ss] event payload` for an
/// action, whose moment is the row's own capture time.
export function entryOf(r: SseEvent): Entry {
  const m = r.rendered.match(/^\[(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}|\d{2}:\d{2}:\d{2})\] ?/)
  const rest = (m ? r.rendered.slice(m[0].length) : r.rendered).replace(/\s+$/, '')
  const stamp = m && m[1].length > 8 ? new Date(`${m[1].replace(' ', 'T')}Z`) : new Date(r.valid_from)
  const at = Number.isNaN(stamp.getTime()) ? null : stamp
  // An action's event name is padded into a column for the terminal; a
  // file reads it as one line.
  if (r.kind !== 'message') return { at, label: 'action', lines: [rest.replace(/ {2,}/g, ' ')], isAction: true }
  const role = r.role ?? 'message'
  const body = rest.replace(new RegExp(`^\\s*${role.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}: ?`), '')
  return { at, label: role, lines: body.split('\n').map((l) => l.replace(/^ {4}/, '')), isAction: false }
}

function toolLines(lines: string[], tools: ExportOptions['tools']): string[] {
  if (tools === 'none') return [`(tool output left out, ${lines.length} line${lines.length === 1 ? '' : 's'})`]
  if (tools === 'trimmed' && lines.length > TRIMMED_TOOL_LINES)
    return [...lines.slice(0, TRIMMED_TOOL_LINES), `… (${lines.length - TRIMMED_TOOL_LINES} more lines)`]
  return lines
}

/// The rows the options keep, each in the shape both formats write.
export function entriesFor(rows: SseEvent[], o: ExportOptions): Entry[] {
  return rows
    .filter((r) => o.include === 'everything' || r.kind === 'message')
    .map(entryOf)
    .map((e) => (e.label === 'tool' ? { ...e, lines: toolLines(e.lines, o.tools) } : e))
}

const longestTicks = (text: string) => Math.max(0, ...(text.match(/`+/g) ?? []).map((s) => s.length))
/// A fence longer than any run of backticks in the text it wraps.
const fenceFor = (text: string) => '`'.repeat(Math.max(3, longestTicks(text) + 1))
/// The same, for code inside a line.
const tickFor = (text: string) => '`'.repeat(longestTicks(text) + 1)

function optionsLine(o: ExportOptions, q: string | undefined): string {
  const parts = [
    o.include === 'conversation' ? 'conversation' : 'everything',
    `tool output ${o.tools === 'full' ? 'full' : o.tools === 'trimmed' ? `first ${TRIMMED_TOOL_LINES} lines` : 'left out'}`,
    `timestamps ${o.timestamps ? 'on' : 'off'}`,
  ]
  if (o.matching && q) parts.push(`only rows matching “${q}”`)
  return parts.join(' · ')
}

function offsetLabel(d: Date): string {
  const m = -d.getTimezoneOffset()
  const sign = m < 0 ? '-' : '+'
  return `UTC${sign}${pad(Math.floor(Math.abs(m) / 60))}:${pad(Math.abs(m) % 60)}`
}

/// The file's text.
export function formatSession(
  session: SessionView,
  rows: SseEvent[],
  o: ExportOptions,
  q: string | undefined,
  now: Date = new Date(),
): string {
  const entries = entriesFor(rows, o)
  const model = [session.model, session.effort].filter(Boolean).join(' · ') || '—'
  const head: [string, string][] = [
    ['session', `${session.agent}/${session.session_id}`],
    ['source', session.src],
    ['model', model],
    ['exported', `${localDateTime(now)} (${offsetLabel(now)})`],
    ['rows', `${entries.length} · ${optionsLine(o, q)}`],
  ]
  const stamp = (e: Entry) => (o.timestamps && e.at ? localDateTime(e.at) : '')

  if (o.format === 'md') {
    const out = [`# Session ${session.agent}/${session.short_id}`, '', ...head.map(([k, v]) => `- **${k}:** ${v}`), '', '---', '']
    for (const e of entries) {
      const when = stamp(e)
      if (e.isAction) {
        const text = e.lines.join(' ')
        const tick = tickFor(text)
        out.push(`- *action*${when ? ` · ${when}` : ''} · ${tick} ${text} ${tick}`, '')
        continue
      }
      out.push(`**${e.label}**${when ? ` · ${when}` : ''}`, '')
      const text = e.lines.join('\n')
      if (e.label === 'tool') {
        const fence = fenceFor(text)
        out.push(`${fence}text`, text, fence, '')
      } else {
        out.push(text, '')
      }
    }
    return out.join('\n')
  }

  const width = Math.max(...head.map(([k]) => k.length))
  const out = ['SuperX session export', ...head.map(([k, v]) => `${k.padEnd(width)}  ${v}`), '', '']
  for (const e of entries) {
    const when = stamp(e)
    const [first, ...more] = e.lines
    out.push(`${when ? `[${when}] ` : ''}${e.label}: ${first ?? ''}`.replace(/\s+$/, ''))
    for (const l of more) out.push(`    ${l}`.replace(/\s+$/, ''))
  }
  return out.join('\n') + '\n'
}

/// `claude_code-01a1309f-1095-20261012-1120.txt`
export function exportFileName(session: SessionView, o: ExportOptions, now: Date = new Date()): string {
  const day = `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}-${pad(now.getHours())}${pad(now.getMinutes())}`
  return `${session.agent}-${session.short_id}-${day}.${o.format}`
}

/// Hand the text to the browser as a download.
export function download(name: string, text: string, format: ExportOptions['format']) {
  const blob = new Blob([text], { type: format === 'md' ? 'text/markdown;charset=utf-8' : 'text/plain;charset=utf-8' })
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = name
  document.body.appendChild(a)
  a.click()
  a.remove()
  setTimeout(() => URL.revokeObjectURL(url), 1000)
}
