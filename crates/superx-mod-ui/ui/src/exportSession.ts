import type { SessionView } from './generated/SessionView'
import type { Chunk, ExportSummary } from './exportStream'
import type { ExportReply, ExportRequest } from './exportWorker'

// A session as a file (#442, #460). The server writes it: every row the
// session holds and every byte of each (a tool call's whole input, a
// tool's whole output, thinking, the OS's actions) in the shape the reader
// chose, streamed straight into the file they picked by a worker, off the
// page's main thread. The page asks for it and shows how far it got.

export type ExportOptions = {
  /// The conversation (every message), or everything (with the OS's actions).
  include: 'conversation' | 'everything'
  /// A tool call's input and a tool's output: whole, their first lines, or
  /// a one-line marker.
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

/// How many lines of a tool's input or output "first lines" keeps: the
/// server's `TRIMMED_TOOL_LINES`, named on the panel.
export const TRIMMED_TOOL_LINES = 20

/// The request for the file. Times are written on the reader's own zone,
/// by its name, so they read right across a daylight-saving change; no
/// zone, no times.
export function exportUrl(sessionId: string, o: ExportOptions, q: string | undefined): string {
  const p = new URLSearchParams({ include: o.include, tools: o.tools, format: o.format })
  if (o.timestamps) p.set('tz', Intl.DateTimeFormat().resolvedOptions().timeZone)
  if (q) p.set('q', q)
  return `/api/sessions/${encodeURIComponent(sessionId)}/export?${p}`
}

/// The export, run in a worker (exportWorker.ts): resolves with what the
/// file holds, and with the gathered file when there was none to write
/// into. Progress comes as the bytes written so far.
export function runExport(
  url: string,
  file: SaveTarget | null,
  format: ExportOptions['format'],
  onBytes?: (bytes: number) => void,
): Promise<ExportSummary & { file?: Blob }> {
  return new Promise((resolve, reject) => {
    const worker = new Worker(new URL('./exportWorker.ts', import.meta.url), { type: 'module' })
    worker.addEventListener('message', (e: MessageEvent<ExportReply>) => {
      const reply = e.data
      if (reply.kind === 'bytes') onBytes?.(reply.bytes)
      else if (reply.kind === 'done') resolve(reply)
      else reject(new Error(reply.message))
    })
    // A worker that cannot load, or fails outside its own handler.
    worker.addEventListener('error', (e) => {
      reject(new Error(e.message || 'the export could not start'))
      worker.terminate()
    })
    const request: ExportRequest = { url, file, type: `${mimeOf(format)};charset=utf-8` }
    worker.postMessage(request)
  })
}

/// `12.3 MB`
export function formatBytes(n: number): string {
  if (n < 1000) return `${n} B`
  if (n < 1_000_000) return `${(n / 1000).toFixed(1)} KB`
  return `${(n / 1_000_000).toFixed(1)} MB`
}

const pad = (n: number) => String(n).padStart(2, '0')

/// `claude_code-01a1309f-1095-20261012-1120.txt`
export function exportFileName(session: SessionView, o: ExportOptions, now: Date = new Date()): string {
  const day = `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}-${pad(now.getHours())}${pad(now.getMinutes())}`
  return `${session.agent}-${session.short_id}-${day}.${o.format}`
}

export const mimeOf = (format: ExportOptions['format']) => (format === 'md' ? 'text/markdown' : 'text/plain')

// Where the file goes is the reader's to choose (#444): the browser's own
// save dialog (File System Access), where it has one. Typed here, not
// from lib.dom, which does not carry the picker. The handle it returns is
// posted to the export's worker, which opens it as a WritableStream.
type SaveTarget = {
  name: string
  createWritable: () => Promise<WritableStream<Chunk>>
}
type SavePicker = (options: {
  suggestedName: string
  id?: string
  types?: { description: string; accept: Record<string, string[]> }[]
}) => Promise<SaveTarget>

const savePicker = (): SavePicker | undefined =>
  (window as unknown as { showSaveFilePicker?: SavePicker }).showSaveFilePicker

/// Does this browser let the reader choose where a file is saved?
export const canChooseWhere = () => savePicker() != null

/// Ask where to save: the chosen file, or null when the reader cancelled.
/// The picker's id makes the browser reopen the folder used last.
export async function chooseWhere(name: string, format: ExportOptions['format']): Promise<SaveTarget | null> {
  const pick = savePicker()
  if (!pick) return null
  try {
    return await pick({
      suggestedName: name,
      id: 'superx-export',
      types: [{ description: format === 'md' ? 'Markdown' : 'Plain text', accept: { [mimeOf(format)]: [`.${format}`] } }],
    })
  } catch (e) {
    if (e instanceof DOMException && e.name === 'AbortError') return null
    throw e
  }
}

/// Hand a finished file to the browser as a download (no save dialog here).
export function download(name: string, file: Blob) {
  const url = URL.createObjectURL(file)
  const a = document.createElement('a')
  a.href = url
  a.download = name
  document.body.appendChild(a)
  a.click()
  a.remove()
  setTimeout(() => URL.revokeObjectURL(url), 1000)
}
