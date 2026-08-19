import { useEffect, useRef, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Badge, Button, Card, Group, ScrollArea, Switch, Table, Text, Title, Tooltip } from '@mantine/core'
import { fetchSessionActivity, fetchSessions } from '../api'
import { useSse } from '../useSse'
import type { SessionEvent } from '../generated/SessionEvent'
import type { SessionView } from '../generated/SessionView'

export default function SessionsPage() {
  const [selected, setSelected] = useState<SessionView | null>(null)
  return selected ? (
    <SessionActivityView session={selected} onBack={() => setSelected(null)} />
  ) : (
    <SessionList onOpen={setSelected} />
  )
}

// Liveness thresholds (render-layer presentation, issue #162):
// a session is ACTIVE while its newest message is fresher than this…
const ACTIVE_MS = 5 * 60 * 1000
// …PAUSED until this, ENDED after.
const PAUSED_MS = 24 * 60 * 60 * 1000

type Liveness = 'active' | 'paused' | 'ended' | 'unknown'

function liveness(lastActive: string | null): Liveness {
  if (!lastActive) return 'unknown'
  const age = Date.now() - new Date(lastActive).getTime()
  if (age < ACTIVE_MS) return 'active'
  if (age < PAUSED_MS) return 'paused'
  return 'ended'
}

const LIVENESS_RANK: Record<Liveness, number> = { active: 0, paused: 1, ended: 2, unknown: 3 }

function LivenessDot({ state }: { state: Liveness }) {
  const styles: Record<Liveness, React.CSSProperties> = {
    // Alive: green, pulsing glow.
    active: { background: '#30d158', boxShadow: '0 0 6px 2px rgba(48,209,88,0.7)', animation: 'sx-glow 1.4s ease-in-out infinite' },
    paused: { background: '#fdd835' },
    // Stopped: flat red, no glow.
    ended: { background: '#e03131' },
    unknown: { background: '#555' },
  }
  return (
    <Tooltip label={state} withArrow>
      <span
        style={{
          display: 'inline-block',
          width: 10,
          height: 10,
          borderRadius: '50%',
          verticalAlign: 'middle',
          ...styles[state],
        }}
      />
    </Tooltip>
  )
}

function relativeTime(iso: string | null): string {
  if (!iso) return '—'
  const then = new Date(iso)
  const secs = Math.max(0, Math.floor((Date.now() - then.getTime()) / 1000))
  if (secs < 60) return `${secs}s ago`
  if (secs < 3600) return `${Math.floor(secs / 60)}m ago`
  if (secs < 86400) return `${Math.floor(secs / 3600)}h ago`
  if (secs < 7 * 86400) return `${Math.floor(secs / 86400)}d ago`
  return then.toLocaleDateString()
}

function SessionList({ onOpen }: { onOpen: (s: SessionView) => void }) {
  const sessions = useQuery({ queryKey: ['sessions'], queryFn: () => fetchSessions(), refetchInterval: 10000 })
  const rows = (sessions.data ?? []).slice().sort((a, b) => {
    const rank = LIVENESS_RANK[liveness(a.last_active)] - LIVENESS_RANK[liveness(b.last_active)]
    if (rank !== 0) return rank
    const recency = (b.last_active ?? '').localeCompare(a.last_active ?? '')
    if (recency !== 0) return recency
    return Number(b.messages) - Number(a.messages)
  })
  return (
    <Card withBorder>
      <style>{'@keyframes sx-glow { 0%, 100% { box-shadow: 0 0 4px 1px rgba(48,209,88,0.5); } 50% { box-shadow: 0 0 10px 4px rgba(48,209,88,0.9); } }'}</style>
      <Title order={5} mb="xs">
        Sessions — every conversation, grouping all its activity
      </Title>
      <ScrollArea h={600}>
        <Table striped highlightOnHover>
          <Table.Thead>
            <Table.Tr>
              <Table.Th w={24} />
              <Table.Th>Agent</Table.Th>
              <Table.Th>Session</Table.Th>
              <Table.Th>Source id</Table.Th>
              <Table.Th>Last active</Table.Th>
              <Table.Th>Messages</Table.Th>
              <Table.Th />
            </Table.Tr>
          </Table.Thead>
          <Table.Tbody>
            {rows.map((s) => (
              <Table.Tr key={s.session_id}>
                <Table.Td>
                  <LivenessDot state={liveness(s.last_active)} />
                </Table.Td>
                <Table.Td>
                  <Badge variant="light">{s.agent}</Badge>
                </Table.Td>
                <Table.Td>
                  <Text size="xs" ff="monospace">
                    {s.session_id}
                  </Text>
                </Table.Td>
                <Table.Td>
                  <Text size="xs" ff="monospace" c="dimmed">
                    {s.src}
                  </Text>
                </Table.Td>
                <Table.Td>
                  <Tooltip label={s.last_active ?? 'no messages yet'} withArrow>
                    <Text size="xs">{relativeTime(s.last_active)}</Text>
                  </Tooltip>
                </Table.Td>
                <Table.Td>{String(s.messages)}</Table.Td>
                <Table.Td>
                  <Button size="compact-xs" variant="light" onClick={() => onOpen(s)}>
                    open
                  </Button>
                </Table.Td>
              </Table.Tr>
            ))}
          </Table.Tbody>
        </Table>
      </ScrollArea>
    </Card>
  )
}

// One session's FULL activity (issue #172): messages + actions merged
// chronologically — historical backlog, then live, auto-scrolling.
function SessionActivityView({ session, onBack }: { session: SessionView; onBack: () => void }) {
  const [live, setLive] = useState(true)
  const [kind, setKind] = useState<'all' | 'message' | 'action'>('all')
  const [liveRows, setLiveRows] = useState<SessionEvent[]>([])
  const backlog = useQuery({
    queryKey: ['activity', session.session_id],
    queryFn: () => fetchSessionActivity(session.session_id),
  })

  useSse((e) => {
    const mine =
      e.kind === 'message' ? e.session_id === session.session_id : e.session_src === session.src
    if (!mine) return
    setLiveRows((prev) => [
      ...prev,
      { kind: e.kind, role: null, rendered: e.rendered, valid_from: e.valid_from },
    ])
  }, !live)

  const rows = [...(backlog.data ?? []), ...liveRows].filter(
    (r) => kind === 'all' || r.kind === kind,
  )

  // Pinned-to-bottom auto-scroll: follow new rows unless the user has
  // scrolled up; resume following when they return near the bottom.
  const viewport = useRef<HTMLDivElement>(null)
  const following = useRef(true)
  useEffect(() => {
    if (following.current && viewport.current) {
      viewport.current.scrollTo({ top: viewport.current.scrollHeight })
    }
  }, [rows.length])

  const roleColor = (role: string | null) =>
    role === 'user' ? 'teal' : role === 'assistant' ? 'indigo' : 'gray'

  return (
    <Card withBorder>
      <Group justify="space-between" mb="xs">
        <Group>
          <Button size="compact-xs" variant="default" onClick={onBack}>
            ← sessions
          </Button>
          <Title order={5} ff="monospace">
            {session.agent}/{session.session_id}
          </Title>
        </Group>
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
          <Switch label="live" checked={live} onChange={(e) => setLive(e.currentTarget.checked)} />
        </Group>
      </Group>
      <ScrollArea
        h={580}
        viewportRef={viewport}
        onScrollPositionChange={({ y }) => {
          const v = viewport.current
          if (v) following.current = y + v.clientHeight >= v.scrollHeight - 40
        }}
      >
        {rows.length === 0 && (
          <Text c="dimmed" size="sm">
            nothing captured for this session yet
          </Text>
        )}
        {rows.map((r, i) => (
          <Text
            key={`${r.valid_from}-${i}`}
            size="sm"
            ff="monospace"
            mb={2}
            c={r.kind === 'action' ? 'dimmed' : undefined}
          >
            <Badge
              size="xs"
              mr={6}
              variant={r.kind === 'action' ? 'outline' : 'light'}
              color={r.kind === 'message' ? roleColor(r.role) : 'indigo'}
            >
              {r.kind === 'message' ? (r.role ?? 'message') : 'action'}
            </Badge>
            {r.rendered}
          </Text>
        ))}
      </ScrollArea>
    </Card>
  )
}
