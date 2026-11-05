import type { AlertKind, Found, Hidden, Split, TimeRow, WatchSessionRow } from '../api'
import { CANCEL, FAIL } from '../EChart'
import { span } from '../time'

// What every Watcher view counts the same way: where an agent's time goes,
// in the one order of its colours; the alerts and the findings by name; the
// operator's days; and the focus a click on any chart sets for every view.

/// Where an agent's time goes, in the one order of the colours: validated on
/// the card surface #2A1235, every adjacent pair clear under colour-blind
/// and normal vision (dataviz validator, 2026-11-05).
export const TIME: [keyof Split, string, string][] = [
  ['thinking', 'thinking', '#B833E8'],
  ['writing', 'writing', '#199e70'],
  ['tools', 'running tools', '#3987e5'],
  ['jobs', 'waiting on its jobs', '#c98500'],
  ['operator', 'waiting on you', '#d55181'],
  ['approval', 'waiting on your approval', '#008300'],
]

/// The time the agent spent at work, waiting on its own jobs included.
export const atWork = (s: Split) => s.thinking + s.writing + s.tools + s.jobs
/// The time it was held: waiting on its jobs, on you, on your approval.
export const heldUp = (s: Split) => s.jobs + s.operator + s.approval
export const every = (s: Split) => atWork(s) + s.operator + s.approval

export const zero = (): Split => ({ thinking: 0, writing: 0, tools: 0, jobs: 0, operator: 0, approval: 0 })
export function plus(a: Split, b: Split): Split {
  return { thinking: a.thinking + b.thinking, writing: a.writing + b.writing, tools: a.tools + b.tools, jobs: a.jobs + b.jobs, operator: a.operator + b.operator, approval: a.approval + b.approval }
}

export const hours = (secs: number) => (secs / 3600).toFixed(1)
export const pct = (part: number, whole: number) => (whole > 0 ? Math.round((100 * part) / whole) : 0)
export const took = (secs: number) => span(secs * 1000)
export const n = (v: number) => v.toLocaleString('en-US')

export const KIND: Record<AlertKind, [string, string]> = {
  run_again: ['Finished work run again', FAIL],
  long_run: ['A run going long', CANCEL],
  stop_not_followed: ['Your stop not followed', FAIL],
  plan_repeats: ['A plan that repeats full runs', CANCEL],
  endless_task: ['A task that never ends', FAIL],
  task_file_broken: ['Its task file broken', FAIL],
  build_stopped: ['A build stopped short', FAIL],
}

/// What each finding is, in the words the page uses, and what it shows
/// that nothing else does.
export const HIDDEN: Record<Hidden, [string, string]> = {
  held: ['Held on your approval', 'a call waited past the command limit on a prompt of yours'],
  slept: ['Asleep in the foreground', 'a command that slept, often polling its own job, deaf to you'],
  thrash: ['Edited over and over', 'one file edited again and again within the hour'],
  refused: ['Refused', 'a call refused by you, auto mode, a hook, a rule or the worktree, and tried again'],
  protected: ['A protected place changed', 'schema, kernel, infrastructure, skills: changed when nothing you said named it'],
  compacted: ['Context lost', 'the context compacted, and the files read again after it'],
  reread: ['Read again', 'the same part of a file read again with nothing changed in it'],
  flood: ['Flooded', "a result of tens of thousands of characters poured into the agent's context"],
  interrupted: ['You stopped it', 'your interrupt, and the call it was in'],
  asked: ['You asked how it stands', 'a status ask: the agent had not told you'],
  corrected: ['You corrected it', "your correction; its cause is the agent's turn before it"],
  undone: ['Work undone', 'a revert, a hard reset, files put back, a history rewritten'],
}

export const HIDDEN_ORDER: Hidden[] = ['held', 'slept', 'thrash', 'refused', 'protected', 'undone', 'compacted', 'reread', 'flood', 'corrected', 'asked', 'interrupted']

const two = (v: number) => String(v).padStart(2, '0')
/// The operator's own day of a time, as the routes count days: `2026-11-04`.
export function dayOf(iso: string): string {
  const d = new Date(iso)
  return `${d.getFullYear()}-${two(d.getMonth() + 1)}-${two(d.getDate())}`
}
/// `2026-11-04` as `11-04`.
export const short = (day: string) => day.slice(5)

/// What a click on a chart narrows every view to.
export type Focus = { model?: string; day?: string; session?: string }
export const focusWords = (f: Focus, sessions: Map<string, WatchSessionRow>) =>
  [
    f.model ? `model ${f.model}` : null,
    f.day ? `day ${f.day}` : null,
    f.session ? `session ${sessions.get(f.session)?.name ?? f.session}` : null,
  ].filter(Boolean) as string[]

/// Whether a session, a day and a model are in the focus.
export function within(f: Focus, session: string, day: string | null, model: string | null | undefined): boolean {
  return (!f.session || f.session === session) && (!f.day || f.day === day) && (!f.model || f.model === (model ?? 'unknown'))
}

export const keepRow = (f: Focus) => (r: TimeRow) => within(f, r.session, r.day, r.model)
export const keepFound = (f: Focus, models: Map<string, string | null>) => (x: Found) => within(f, x.session, dayOf(x.at), models.get(x.session))

/// Rows summed by a key.
export function sumBy<T>(rows: T[], key: (r: T) => string, split: (r: T) => Split): Map<string, Split> {
  const out = new Map<string, Split>()
  for (const r of rows) {
    const k = key(r)
    out.set(k, plus(out.get(k) ?? zero(), split(r)))
  }
  return out
}

/// A file's folder, `depth` folders deep from the root.
export function folderOf(path: string, depth: number): string {
  const parts = path.split('/').filter(Boolean)
  return '/' + parts.slice(0, Math.min(depth, Math.max(parts.length - 1, 0))).join('/')
}

/// The tools that read or write a file by its path.
export const FILE_TOOLS = new Set(['Read', 'Edit', 'Write', 'MultiEdit', 'NotebookEdit'])

/// Time no call held, as the tool it is counted under.
export const unheld = (tool: string) => tool.startsWith('(')
