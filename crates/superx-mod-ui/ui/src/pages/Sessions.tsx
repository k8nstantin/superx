import { useCallback, useMemo, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { useDebouncedValue } from '@mantine/hooks'
import { Badge, Button, Card, Group, Progress, ScrollArea, Table, Text, Title, Tooltip } from '@mantine/core'
import { fetchAgents, fetchSessionActivity, fetchSessions } from '../api'
import { useSse } from '../useSse'
import { Feed, MAX_FEED_ROWS, matchesSearch, mergeFeed } from '../Feed'
import { useFeedHistory } from '../useFeedHistory'
import type { SseEvent } from '../generated/SseEvent'
import type { SessionView } from '../generated/SessionView'
import { useBreadcrumb } from '../Breadcrumbs'

export default function SessionsPage() {
  const [selected, setSelected] = useState<SessionView | null>(null)
  useBreadcrumb(
    selected
      ? [{ label: 'Sessions', onClick: () => setSelected(null) }, { label: selected.identity }]
      : [{ label: 'Sessions' }],
  )
  return selected ? (
    <SessionFeed session={selected} onBack={() => setSelected(null)} />
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

// Context is a FINITE window (issue #202): render usage as a bar —
// green through mid-range, yellow from 70%, red when full (≥90%).
function ContextBar({ pct, tokens }: { pct: bigint | null; tokens: bigint | null }) {
  if (pct == null) return <Text size="xs">—</Text>
  const p = Number(pct)
  const color = p >= 90 ? 'red' : p >= 70 ? 'yellow' : 'green'
  return (
    <Tooltip label={`context window: ${fmtTokens(tokens)} used (${p}%)`} withArrow>
      <Group gap={6} wrap="nowrap">
        <Progress value={p} color={color} size="sm" w={70} />
        <Text size="xs" w={44}>
          {fmtTokens(tokens)}
        </Text>
      </Group>
    </Tooltip>
  )
}

// Compact token rendering: 1234 → 1.2k, 1234567 → 1.2M, absent → —.
function fmtTokens(n: number | bigint | null): string {
  if (n == null) return '—'
  const v = Number(n)
  if (v >= 1e6) return `${(v / 1e6).toFixed(1)}M`
  if (v >= 1e3) return `${(v / 1e3).toFixed(1)}k`
  return String(v)
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
    return Number(b.actions) - Number(a.actions)
  })
  return (
    <Card withBorder>
      <style>{'@keyframes sx-glow { 0%, 100% { box-shadow: 0 0 4px 1px rgba(48,209,88,0.5); } 50% { box-shadow: 0 0 10px 4px rgba(48,209,88,0.9); } }'}</style>
      <Title order={5} mb="xs">
        Sessions — click one to open its feed
      </Title>
      <ScrollArea h={600}>
        <Table striped highlightOnHover>
          <Table.Thead>
            <Table.Tr>
              <Table.Th w={24} />
              <Table.Th>Agent</Table.Th>
              <Table.Th>Session</Table.Th>
              <Table.Th>Model</Table.Th>
              <Table.Th>Source id</Table.Th>
              <Table.Th>Context</Table.Th>
              <Table.Th>Tokens</Table.Th>
              <Table.Th>Last active</Table.Th>
              <Table.Th>Actions</Table.Th>
            </Table.Tr>
          </Table.Thead>
          <Table.Tbody>
            {rows.map((s) => (
              <Table.Tr key={s.session_id} onClick={() => onOpen(s)} style={{ cursor: 'pointer' }}>
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
                  {s.model ? (
                    <Tooltip label="the model currently doing this session's work" withArrow>
                      <Badge variant="light" color="pelican">
                        {s.model}
                      </Badge>
                    </Tooltip>
                  ) : (
                    <Text size="xs" c="dimmed">
                      —
                    </Text>
                  )}
                </Table.Td>
                <Table.Td>
                  <Text size="xs" ff="monospace" c="dimmed">
                    {s.src}
                  </Text>
                </Table.Td>
                <Table.Td>
                  <ContextBar pct={s.context_pct} tokens={s.context_tokens} />
                </Table.Td>
                <Table.Td>
                  <Tooltip label="cumulative output tokens" withArrow>
                    <Text size="xs">{fmtTokens(s.output_tokens)}</Text>
                  </Tooltip>
                </Table.Td>
                <Table.Td>
                  <Tooltip label={s.last_active ?? 'no messages yet'} withArrow>
                    <Text size="xs">{relativeTime(s.last_active)}</Text>
                  </Tooltip>
                </Table.Td>
                <Table.Td>{String(s.actions)}</Table.Td>
              </Table.Tr>
            ))}
          </Table.Tbody>
        </Table>
      </ScrollArea>
    </Card>
  )
}

// THE feed, scoped to one session (issue #187): same exact rendering,
// controls, and auto-scroll as the global Activity feed.
function SessionFeed({ session, onBack }: { session: SessionView; onBack: () => void }) {
  const [paused, setPaused] = useState(false)
  const [liveRows, setLiveRows] = useState<SseEvent[]>([])
  const [search, setSearch] = useState('')
  const [q] = useDebouncedValue(search.trim(), 350)
  const backlog = useQuery({
    queryKey: ['activity', session.session_id, q],
    queryFn: () => fetchSessionActivity(session.session_id, undefined, undefined, q),
  })
  const sessions = useQuery({ queryKey: ['sessions'], queryFn: () => fetchSessions(), refetchInterval: 10000 })
  const agents = useQuery({ queryKey: ['agents'], queryFn: fetchAgents, refetchInterval: 30000 })

  // agent_id → name, for the live filter's agent scope below.
  const agentName = useMemo(
    () => new Map((agents.data ?? []).map((a) => [a.agent_id, a.name])),
    [agents.data],
  )

  useSse((batch) => {
    // Same scoping as the server-side backlog query: actions match by
    // source key AND agent — source keys alone collide across agents
    // on fallback keys like `unknown-session` (review finding). An
    // agent the directory doesn't know yet is accepted rather than
    // dropped (staleness must not lose legitimate rows).
    const mine = batch.filter((e) =>
      e.kind === 'message'
        ? e.session_id === session.session_id
        : e.session_src === session.src &&
          (e.agent_id == null || (agentName.get(e.agent_id) ?? session.agent) === session.agent),
    )
    const keep = mine.filter((e) => matchesSearch(e, q))
    if (keep.length) setLiveRows((prev) => [...prev, ...keep].slice(-MAX_FEED_ROWS))
  }, paused)

  const page = useCallback(
    (before: string, limit: number) => fetchSessionActivity(session.session_id, limit, before, q),
    [session.session_id, q],
  )
  const { older, loadOlder, loadingOlder, exhausted } = useFeedHistory(
    `${session.session_id}:${q}`,
    page,
  )

  const rows = useMemo(
    () => mergeFeed([...older, ...(backlog.data ?? [])], liveRows, MAX_FEED_ROWS + older.length),
    [older, backlog.data, liveRows],
  )

  return (
    <Feed
      header={
        <Group>
          <Button size="compact-xs" variant="default" onClick={onBack}>
            ← sessions
          </Button>
          <Title order={5} ff="monospace">
            {session.agent}/{session.session_id}
          </Title>
          {session.model && (
            <Badge variant="light" color="pelican">
              {session.model}
            </Badge>
          )}
        </Group>
      }
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
