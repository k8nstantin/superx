import { useMemo, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Title } from '@mantine/core'
import { fetchActivity, fetchAgents, fetchSessions } from '../api'
import { useSse } from '../useSse'
import { Feed, MAX_FEED_ROWS, mergeFeed } from '../Feed'
import type { SseEvent } from '../generated/SseEvent'

// The GLOBAL scope of THE feed (issue #187): everyone and everything
// in one place — historical backlog, then live. The session view is
// the same exact feed, filtered.
export default function ActivityPage() {
  const [paused, setPaused] = useState(false)
  const [liveRows, setLiveRows] = useState<SseEvent[]>([])
  const backlog = useQuery({ queryKey: ['activity', 'global'], queryFn: () => fetchActivity() })
  const sessions = useQuery({ queryKey: ['sessions'], queryFn: () => fetchSessions(), refetchInterval: 10000 })
  const agents = useQuery({ queryKey: ['agents'], queryFn: fetchAgents, refetchInterval: 30000 })

  useSse((batch) => {
    setLiveRows((prev) => [...prev, ...batch].slice(-MAX_FEED_ROWS))
  }, paused)

  const rows = useMemo(() => mergeFeed(backlog.data ?? [], liveRows), [backlog.data, liveRows])

  return (
    <Feed
      header={<Title order={5}>Live activity — everything the OS captures, by session, as it happens</Title>}
      rows={rows}
      sessions={sessions.data ?? []}
      agents={agents.data ?? []}
      paused={paused}
      onPausedChange={setPaused}
      loading={backlog.isLoading}
      error={backlog.isError ? String(backlog.error) : null}
    />
  )
}
