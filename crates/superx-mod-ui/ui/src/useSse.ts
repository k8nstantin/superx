import { useEffect, useRef } from 'react'
import type { SseEvent } from './generated/SseEvent'

/// A stream the browser closed for good is opened again after this long.
const REOPEN_MS = 3000
/// A batch waits at most this long when no animation frame comes.
const HIDDEN_FLUSH_MS = 1000

/** Subscribe to the OS's live event stream. Events are delivered in
 *  BATCHES, at most once per animation frame — the poller emits one
 *  SSE frame per event (hundreds per tick during bursts), and a
 *  render per frame freezes the tab (issue #187 review). The handler
 *  is read through a ref so the latest closure always runs (no stale
 *  captures across re-renders). `paused` drops events client-side.
 *
 *  A hidden tab gets no animation frames, so a batch also flushes on a
 *  timer: a background feed used to hold every event since it was
 *  hidden, and paint none of them until it was shown (#415 QA).
 *  EventSource reconnects by itself only after a network error. A
 *  response that is not a 200 event stream — a 503 from anything in
 *  between — closes it for good, and the feed went quiet until a
 *  reload; a closed stream is opened again. */
export function useSse(onEvents: (batch: SseEvent[]) => void, paused = false) {
  const pausedRef = useRef(paused)
  pausedRef.current = paused
  const handlerRef = useRef(onEvents)
  handlerRef.current = onEvents
  useEffect(() => {
    let es: EventSource | null = null
    let reopen: ReturnType<typeof setTimeout> | undefined
    let live = true
    let buffer: SseEvent[] = []
    let scheduled = false
    const flush = () => {
      if (!scheduled || !live) return
      scheduled = false
      const batch = buffer
      buffer = []
      if (batch.length) handlerRef.current(batch)
    }
    const open = () => {
      const stream = new EventSource('/api/events')
      es = stream
      stream.onmessage = (m) => {
        if (pausedRef.current) return
        try {
          buffer.push(JSON.parse(m.data) as SseEvent)
        } catch {
          return /* tolerate malformed frames */
        }
        if (!scheduled) {
          scheduled = true
          requestAnimationFrame(flush)
          setTimeout(flush, HIDDEN_FLUSH_MS)
        }
      }
      stream.onerror = () => {
        // A network error: the browser retries by itself. A failed
        // response: CLOSED, and nothing retries but this.
        if (live && stream.readyState === EventSource.CLOSED) {
          reopen = setTimeout(open, REOPEN_MS)
        }
      }
    }
    open()
    return () => {
      live = false
      clearTimeout(reopen)
      es?.close()
    }
  }, [])
}
