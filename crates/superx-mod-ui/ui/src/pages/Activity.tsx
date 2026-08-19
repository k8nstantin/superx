import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Badge, Card, Group, ScrollArea, Switch, Text, Title, Tooltip } from '@mantine/core'
import { fetchSessions } from '../api'
import { useSse } from '../useSse'
import type { SseEvent } from './../generated/SseEvent'
import type { SessionView } from './../generated/SessionView'

const MAX_ROWS = 500

// Who is doing what (issue #172): every row is attributed to its
// session — messages carry the session entity id, actions carry the
// source-session key; both resolve against the sessions list.
function sessionIdentity(e: SseEvent, sessions: SessionView[]): string | null {
  const match =
    e.kind === 'message'
      ? sessions.find((s) => s.session_id === e.session_id)
      : e.session_src
        ? sessions.find((s) => s.src === e.session_src)
        : undefined
  if (match) return `${match.agent}/${match.session_id.slice(0, 8)}`
  if (e.kind === 'message' && e.session_id) return e.session_id.slice(0, 8)
  if (e.session_src) return e.session_src.slice(0, 8)
  return null // a global OS event — no session
}

export default function ActivityPage() {
  const [rows, setRows] = useState<SseEvent[]>([])
  const [paused, setPaused] = useState(false)
  const [kind, setKind] = useState<'all' | 'message' | 'action'>('all')
  const sessions = useQuery({ queryKey: ['sessions'], queryFn: () => fetchSessions(), refetchInterval: 10000 })

  useSse((e) => {
    setRows((prev) => [e, ...prev].slice(0, MAX_ROWS))
  }, paused)

  const visible = rows.filter((r) => kind === 'all' || r.kind === kind)

  return (
    <Card withBorder>
      <Group justify="space-between" mb="xs">
        <Title order={5}>Live activity — everything the OS captures, by session, as it happens</Title>
        <Group>
          {(['all', 'message', 'action'] as const).map((k) => (
            <Badge
              key={k}
              variant={kind === k ? 'filled' : 'outline'}
              style={{ cursor: 'pointer' }}
              onClick={() => setKind(k)}
            >
              {k}
            </Badge>
          ))}
          <Switch label="pause" checked={paused} onChange={(e) => setPaused(e.currentTarget.checked)} />
        </Group>
      </Group>
      <ScrollArea h={560}>
        {visible.length === 0 && (
          <Text c="dimmed" size="sm">
            waiting for events… (work in any coding agent to see them land here)
          </Text>
        )}
        {visible.map((r, i) => {
          const identity = sessionIdentity(r, sessions.data ?? [])
          return (
            <Text key={`${r.valid_from}-${i}`} size="sm" ff="monospace" mb={2}>
              <Badge size="xs" mr={4} color={r.kind === 'message' ? 'teal' : 'indigo'}>
                {r.kind}
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
      </ScrollArea>
    </Card>
  )
}
