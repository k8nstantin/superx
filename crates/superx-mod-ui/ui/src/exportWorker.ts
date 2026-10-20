// The export runs here, off the page's main thread (#460). A long
// session's feed keeps that thread busy for seconds at a time, and a file
// streamed through it waited on each of them: the server's first byte came
// in 0.3 s and the file's first write 5.5 s later. The chosen file's handle
// is posted here (file handles are serializable and writable in workers);
// without one, the file is gathered and handed back to be downloaded.
import { streamExport, type Chunk, type ExportSummary } from './exportStream'

/// What the page asks: the file's request, and where to write it.
export type ExportRequest = {
  url: string
  /// The file the reader chose; null gathers the file to be downloaded.
  file: { createWritable: () => Promise<WritableStream<Chunk>> } | null
  /// The gathered file's media type.
  type: string
}

/// What the worker tells the page.
export type ExportReply =
  | { kind: 'bytes'; bytes: number }
  | ({ kind: 'done'; file?: Blob } & ExportSummary)
  | { kind: 'failed'; message: string }

// Progress at most this often: the page redraws on each.
const PROGRESS_MS = 250

// The worker's own scope, typed here: the page's lib (DOM) does not carry it.
const scope = self as unknown as {
  postMessage(message: ExportReply): void
  addEventListener(type: 'message', listener: (e: MessageEvent<ExportRequest>) => void): void
  close(): void
}

scope.addEventListener('message', (e) => void run(e.data))

async function run({ url, file, type }: ExportRequest) {
  try {
    let shown = 0
    const progress = (bytes: number) => {
      const now = Date.now()
      if (now - shown >= PROGRESS_MS) {
        shown = now
        scope.postMessage({ kind: 'bytes', bytes })
      }
    }
    if (file) {
      const done = await streamExport(url, await file.createWritable(), progress)
      scope.postMessage({ kind: 'done', ...done })
    } else {
      const chunks: Chunk[] = []
      const gather = new WritableStream<Chunk>({
        write(chunk) {
          chunks.push(chunk)
        },
      })
      const done = await streamExport(url, gather, progress)
      scope.postMessage({ kind: 'done', ...done, file: new Blob(chunks, { type }) })
    }
  } catch (e) {
    scope.postMessage({ kind: 'failed', message: e instanceof Error ? e.message : String(e) })
  } finally {
    scope.close()
  }
}
