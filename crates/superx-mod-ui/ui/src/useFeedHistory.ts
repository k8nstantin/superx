import { useCallback, useEffect, useRef, useState } from 'react'
import type { SseEvent } from './generated/SseEvent'

// Scrolling back through history (issue #241). The feed's first page
// is the newest N rows; each further page is the newest N rows STRICTLY
// older than the oldest row on screen, so reading backwards never ends
// until the substrate does.
//
// `scopeKey` is the feed's identity (global, or a session id): changing
// it throws the accumulated history away rather than mixing two feeds.

export const HISTORY_PAGE = 300

export function useFeedHistory(
  scopeKey: string,
  fetchPage: (before: string, limit: number) => Promise<SseEvent[]>,
) {
  const [older, setOlder] = useState<SseEvent[]>([])
  const [loadingOlder, setLoadingOlder] = useState(false)
  const [exhausted, setExhausted] = useState(false)
  // A ref, not the state flag: two scroll events in one frame would
  // both see a stale `loadingOlder` and fire the same page twice.
  const busy = useRef(false)

  useEffect(() => {
    setOlder([])
    setExhausted(false)
    busy.current = false
  }, [scopeKey])

  const loadOlder = useCallback(
    (before: string | undefined) => {
      if (!before || busy.current || exhausted) return
      busy.current = true
      setLoadingOlder(true)
      void fetchPage(before, HISTORY_PAGE)
        .then((page) => {
          // An empty page means we reached the beginning; anything the
          // feed already holds is dropped by the id dedupe downstream.
          if (page.length === 0) setExhausted(true)
          else setOlder((prev) => [...page, ...prev])
        })
        .catch(() => setExhausted(true))
        .finally(() => {
          busy.current = false
          setLoadingOlder(false)
        })
    },
    [fetchPage, exhausted],
  )

  return { older, loadOlder, loadingOlder, exhausted }
}
