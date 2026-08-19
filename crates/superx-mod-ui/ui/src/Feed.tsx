import { useEffect, useRef, useState } from 'react'
import { Badge, Card, Group, ScrollArea, Switch, Text, Tooltip } from '@mantine/core'
import type { SseEvent } from './generated/SseEvent'
import type { SessionView } from './generated/SessionView'
import type { AgentView } from './generated/AgentView'

// THE feed (issue #187): Activity and the session view render the
// same exact continuous feed — same rows, same chips, same pause,
// same pinned-to-bottom auto-scroll. Only the scope differs.

export const MAX_FEED_ROWS = 500

// A stable identity for one captured event: capture timestamp (ns
// precision) + kind + rendered text — dedupes rows that arrive both
// over SSE and in a backlog (re)fetch.
export const eventKey = (r: SseEvent) => `${r.valid_from}|${r.kind}|${r.rendered}`

// Merge backlog + live rows: dedupe (backlog copy wins), re-sort by
// capture time (SSE emits each tick's actions before its messages, so
// arrival order alone is not chronological), cap to the newest rows.
export function mergeFeed(backlog: SseEvent[], live: SseEvent[]): SseEvent[] {
  const seen = new Map<string, SseEvent>()
  for (const r of [...backlog, ...live]) {
    if (!seen.has(eventKey(r))) seen.set(eventKey(r), r)
  }
  return [...seen.values()]
    .sort((a, b) => a.valid_from.localeCompare(b.valid_from))
    .slice(-MAX_FEED_ROWS)
}

// Who is doing what: every row is attributed to its session —
// messages by session entity id, actions by source-session key. When
// two agents' sessions share a source key, the event's agent_id
// (resolved to a name via the agents list) picks the right one.
export function sessionIdentity(
  e: SseEvent,
  sessions: SessionView[],
  agents: AgentView[],
): string | null {
  let match: SessionView | undefined
  if (e.kind === 'message') {
    match = sessions.find((s) => s.session_id === e.session_id)
  } else if (e.session_src) {
    const candidates = sessions.filter((s) => s.src === e.session_src)
    if (candidates.length > 1 && e.agent_id) {
      const agentName = agents.find((a) => a.agent_id === e.agent_id)?.name
      match = candidates.find((s) => s.agent === agentName) ?? candidates[0]
    } else {
      match = candidates[0]
    }
  }
  if (match) return `${match.agent}/${match.session_id.slice(0, 8)}`
  if (e.kind === 'message' && e.session_id) return e.session_id.slice(0, 8)
  if (e.session_src) return e.session_src.slice(0, 8)
  return null // a global OS event — no session
}

const roleColor = (role: string | null) =>
  role === 'user' ? 'teal' : role === 'assistant' ? 'indigo' : 'gray'

// The label a row wears — the same string its badge shows, and the
// vocabulary of the filter chips (operator directive: filter by the
// labels actually visible — user, assistant, tool, action, …).
const rowLabel = (r: SseEvent) => (r.kind === 'message' ? (r.role ?? 'message') : 'action')

export function Feed({
  header,
  rows,
  sessions,
  agents,
  paused,
  onPausedChange,
  loading,
  error,
}: {
  header: React.ReactNode
  rows: SseEvent[]
  sessions: SessionView[]
  agents: AgentView[]
  paused: boolean
  onPausedChange: (v: boolean) => void
  loading?: boolean
  error?: string | null
}) {
  const [label, setLabel] = useState<string>('all')
  // Chips are the DISTINCT labels present in the feed right now.
  const labels = [...new Set(rows.map(rowLabel))].sort()
  const visible = rows.filter((r) => label === 'all' || rowLabel(r) === label)

  // Pinned-to-bottom auto-scroll: a sentinel below the last row is
  // scrolled into view on every change while following; the user
  // scrolling up releases the pin, returning near the bottom restores
  // it.
  const viewport = useRef<HTMLDivElement>(null)
  const bottom = useRef<HTMLDivElement>(null)
  const following = useRef(true)
  useEffect(() => {
    if (following.current) {
      bottom.current?.scrollIntoView({ block: 'end' })
    }
  }, [visible.length, loading])

  return (
    <Card withBorder>
      <Group justify="space-between" mb="xs">
        {header}
        <Group gap="xs">
          {['all', ...labels].map((k) => (
            <Badge
              key={k}
              variant={label === k ? 'filled' : 'outline'}
              style={{ cursor: 'pointer' }}
              onClick={() => setLabel(k)}
            >
              {k}
            </Badge>
          ))}
          <Switch
            label="pause"
            checked={paused}
            onChange={(e) => onPausedChange(e.currentTarget.checked)}
          />
        </Group>
      </Group>
      <ScrollArea
        h={580}
        viewportRef={viewport}
        onScrollPositionChange={({ y }) => {
          const v = viewport.current
          if (v) following.current = y + v.clientHeight >= v.scrollHeight - 80
        }}
      >
        {loading && (
          <Text c="dimmed" size="sm">
            loading captured activity…
          </Text>
        )}
        {error && (
          <Text c="red.4" size="sm">
            could not load activity: {error}
          </Text>
        )}
        {!loading && !error && visible.length === 0 && (
          <Text c="dimmed" size="sm">
            nothing captured yet — work in any coding agent to see it land here
          </Text>
        )}
        {visible.map((r, i) => {
          const identity = sessionIdentity(r, sessions, agents)
          return (
            <Text
              key={`${r.valid_from}-${i}`}
              size="sm"
              ff="monospace"
              mb={2}
              c={r.kind === 'action' ? 'dimmed' : undefined}
            >
              <Badge
                size="xs"
                mr={4}
                variant={r.kind === 'action' ? 'outline' : 'light'}
                color={r.kind === 'message' ? roleColor(r.role) : 'indigo'}
              >
                {r.kind === 'message' ? (r.role ?? 'message') : 'action'}
              </Badge>
              <Tooltip label={identity ? 'session' : 'no session — OS-level event'} withArrow>
                <Badge size="xs" mr={6} variant="outline" color={identity ? 'gray' : 'yellow'}>
                  {identity ?? 'system'}
                </Badge>
              </Tooltip>
              {r.rendered}
            </Text>
          )
        })}
        <div ref={bottom} />
      </ScrollArea>
    </Card>
  )
}
