import { useEffect, useRef } from 'react'
import type { SseEvent } from './generated/SseEvent'

/** Subscribe to the OS's live event stream. Events are delivered in
 *  BATCHES, at most once per animation frame — the poller emits one
 *  SSE frame per event (hundreds per tick during bursts), and a
 *  render per frame freezes the tab (issue #187 review). The handler
 *  is read through a ref so the latest closure always runs (no stale
 *  captures across re-renders). EventSource reconnects automatically;
 *  `paused` drops events client-side. */
export function useSse(onEvents: (batch: SseEvent[]) => void, paused = false) {
  const pausedRef = useRef(paused)
  pausedRef.current = paused
  const handlerRef = useRef(onEvents)
  handlerRef.current = onEvents
  useEffect(() => {
    const es = new EventSource('/api/events')
    let buffer: SseEvent[] = []
    let scheduled = false
    es.onmessage = (m) => {
      if (pausedRef.current) return
      try {
        buffer.push(JSON.parse(m.data) as SseEvent)
      } catch {
        return /* tolerate malformed frames */
      }
      if (!scheduled) {
        scheduled = true
        requestAnimationFrame(() => {
          scheduled = false
          const batch = buffer
          buffer = []
          if (batch.length) handlerRef.current(batch)
        })
      }
    }
    return () => es.close()
  }, [])
}
