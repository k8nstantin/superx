import { useEffect, useRef } from 'react'
import type { SseEvent } from './generated/SseEvent'

/** Subscribe to the OS's live event stream. EventSource reconnects
 *  automatically; `paused` drops events client-side. */
export function useSse(onEvent: (e: SseEvent) => void, paused = false) {
  const pausedRef = useRef(paused)
  pausedRef.current = paused
  useEffect(() => {
    const es = new EventSource('/api/events')
    es.onmessage = (m) => {
      if (pausedRef.current) return
      try {
        onEvent(JSON.parse(m.data) as SseEvent)
      } catch {
        /* tolerate malformed frames */
      }
    }
    return () => es.close()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])
}
