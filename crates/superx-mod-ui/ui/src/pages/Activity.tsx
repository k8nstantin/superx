import { useCallback, useMemo, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { useDebouncedValue } from '@mantine/hooks'
import { Title } from '@mantine/core'
import { fetchActivity, fetchAgents, fetchSessions } from '../api'
import { useSse } from '../useSse'
import { Feed, MAX_FEED_ROWS, matchesSearch, mergeFeed } from '../Feed'
import { useFeedHistory } from '../useFeedHistory'
import type { SseEvent } from '../generated/SseEvent'

// The GLOBAL scope of THE feed (issue #187): everyone and everything
// in one place — historical backlog, then live. The session view is
// the same exact feed, filtered.
export default function ActivityPage() {
  const [paused, setPaused] = useState(false)
  const [liveRows, setLiveRows] = useState<SseEvent[]>([])
  const [search, setSearch] = useState('')
  const [q] = useDebouncedValue(search.trim(), 350)
  const backlog = useQuery({
    queryKey: ['activity', 'global', q],
    queryFn: () => fetchActivity(undefined, undefined, q),
  })
  const sessions = useQuery({ queryKey: ['sessions'], queryFn: () => fetchSessions(), refetchInterval: 10000 })
  const agents = useQuery({ queryKey: ['agents'], queryFn: fetchAgents, refetchInterval: 30000 })

  useSse((batch) => {
    // While a search is on, only matching rows may join the feed —
    // otherwise live traffic would quietly break the filter.
    const keep = batch.filter((e) => matchesSearch(e, q))
    if (keep.length) setLiveRows((prev) => [...prev, ...keep].slice(-MAX_FEED_ROWS))
  }, paused)

  const page = useCallback(
    (before: string, limit: number) => fetchActivity(limit, before, q),
    [q],
  )
  const { older, loadOlder, loadingOlder, exhausted } = useFeedHistory(`global:${q}`, page)

  // The cap grows with what the reader deliberately fetched, so paging
  // back never drops the rows being read (issue #241).
  const rows = useMemo(
    () => mergeFeed([...older, ...(backlog.data ?? [])], liveRows, MAX_FEED_ROWS + older.length),
    [older, backlog.data, liveRows],
  )

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
      onLoadOlder={loadOlder}
      loadingOlder={loadingOlder}
      exhausted={exhausted}
      search={search}
      onSearchChange={setSearch}
    />
  )
}
