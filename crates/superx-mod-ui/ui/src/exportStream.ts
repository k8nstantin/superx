// The export's byte path (#460): the file, piped from the server into a
// writable as it comes, and kept only once its last line has arrived. It
// runs in a worker (exportWorker.ts), so nothing here touches the page.

/// A piece of the file as fetch delivers it: bytes over a plain
/// ArrayBuffer, which is what a Blob takes.
export type Chunk = Uint8Array<ArrayBuffer>

/// How the export ended: the bytes written, and what its last line holds.
export type ExportSummary = { bytes: number; holds: string }

const END_MARK = 'end of export · '

/// What the file's last line says it holds (`27,529 rows · 8,212 messages
/// (…) · 19,317 actions`), or null when the text does not end with it.
export function summaryOf(tail: string): string | null {
  const last = tail.trimEnd().split('\n').pop() ?? ''
  const line = last.replace(/^\*\*end of export\*\* · /, END_MARK)
  return line.startsWith(END_MARK) ? line.slice(END_MARK.length) : null
}

// The last line is short; keep enough of the stream's end to hold it.
const TAIL_CHARS = 2000

/// The file, piped from the server into `sink` as it comes. The sink is
/// closed only once the file's last line has arrived: a stream that fails,
/// or ends without it, aborts the sink instead, and an aborted file write
/// never replaces the file the reader picked (it writes to a swap file
/// until it is closed).
export async function streamExport(
  url: string,
  sink: WritableStream<Chunk>,
  onBytes?: (bytes: number) => void,
): Promise<ExportSummary> {
  const r = await fetch(url)
  if (!r.ok || !r.body) {
    // The server's error body carries its own words (`{ is_error, output }`).
    let detail = `${r.status}`
    try {
      const body = (await r.json()) as { output?: unknown }
      if (typeof body.output === 'string') detail = `${r.status} — ${body.output}`
    } catch {
      /* not JSON — the status is all there is */
    }
    const error = new Error(detail)
    await sink.abort(error)
    throw error
  }
  let bytes = 0
  let tail = ''
  let holds: string | null = null
  const decoder = new TextDecoder()
  const checked = new TransformStream<Chunk, Chunk>({
    transform(chunk, controller) {
      bytes += chunk.byteLength
      tail = (tail + decoder.decode(chunk, { stream: true })).slice(-TAIL_CHARS)
      onBytes?.(bytes)
      controller.enqueue(chunk)
    },
    flush(controller) {
      holds = summaryOf(tail + decoder.decode())
      if (holds == null) controller.error(new Error('the file ended before its last line'))
    },
  })
  await r.body.pipeThrough(checked).pipeTo(sink)
  return { bytes, holds: holds ?? '' }
}
