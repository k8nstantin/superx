import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import {
  Badge,
  Button,
  Card,
  Collapse,
  Grid,
  Group,
  SimpleGrid,
  Table,
  Text,
  Title,
  Tooltip,
} from '@mantine/core'
import { fetchInsights, fetchStats, fetchStatus } from '../api'
import { AXIS, CHART_COLORS, EChart, GRID_LINE, INK, INK_MUTED, TOOLTIP } from '../EChart'
import { sessionColor } from '../Feed'
import { LivenessDot } from '../LivenessDot'
import { useBreadcrumb } from '../Breadcrumbs'

// The Status page (issues #228, #237): the OS's captured numbers,
// front and center — what the agents did (tools, lines of code, did
// the calls even work), who did it (per-session, per-agent, per
// model), what it cost in tokens, and whether capture itself is
// healthy. Window-scoped figures say so; the rest is all history.

const OK = '#199e70'
const FAIL = '#e66767'
const CANCEL = '#c98500'
const UNKNOWN = '#5C4470'
// Low→high magnitude ramp on the swindex surface, for the two heatmaps.
const HEAT = ['#241033', '#4C0063', '#8300A8', '#B833E8', '#DDA9FF']
const WEEKDAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun']
// A category y-axis fills bottom-up, so the axis rows are reversed to
// put Monday on top: row index = 7 - time::wday.
const HEAT_ROWS = [...WEEKDAYS].reverse()

function fmtCompact(n: number | bigint | null | undefined): string {
  if (n == null) return '—'
  const v = Number(n)
  // Cache reads run to billions on a long day — 7679.1M is not a
  // number anyone reads (#338).
  if (v >= 1e12) return `${(v / 1e12).toFixed(1)}T`
  if (v >= 1e9) return `${(v / 1e9).toFixed(1)}B`
  if (v >= 1e6) return `${(v / 1e6).toFixed(1)}M`
  if (v >= 1e3) return `${(v / 1e3).toFixed(1)}k`
  return String(v)
}

/// Bytes of file text that rode into a prompt (#337).
function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`
  return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GB`
}

function fmtAge(secs: number | bigint | null | undefined): string {
  if (secs == null) return '—'
  const v = Number(secs)
  if (v < 60) return `${v}s`
  if (v < 3600) return `${Math.floor(v / 60)}m`
  if (v < 86400) return `${Math.floor(v / 3600)}h`
  return `${Math.floor(v / 86400)}d`
}

/// A 24-segment clock: which hours of the last day saw work. The
/// cockpit instrument for a 24×7 operator — coverage, not volume.
function CoverageStrip({ hours }: { hours: number | bigint | null | undefined }) {
  const lit = hours == null ? 0 : Number(hours)
  return (
    <Group gap={2} mt={6} wrap="nowrap">
      {Array.from({ length: 24 }, (_, i) => (
        <div
          key={i}
          style={{
            flex: 1,
            height: 8,
            borderRadius: 2,
            background: i < lit ? 'var(--mantine-color-pelican-4)' : '#2A1235',
          }}
        />
      ))}
    </Group>
  )
}

/// A ranked list rendered as proportional bars — the shape that reads
/// fastest for "what dominated": languages, commands, files, projects.
function BarList({
  rows,
  color = 'var(--mantine-color-pelican-5)',
  mono,
  shorten,
  empty = 'nothing in this window',
}: {
  rows: { name: string; value: number | bigint }[]
  color?: string
  mono?: boolean
  shorten?: (s: string) => string
  empty?: string
}) {
  if (rows.length === 0)
    return (
      <Text size="xs" c="dimmed">
        {empty}
      </Text>
    )
  const max = Math.max(...rows.map((r) => Number(r.value)), 1)
  return (
    <div>
      {rows.map((r) => {
        const label = shorten ? shorten(r.name) : r.name
        return (
          <Tooltip key={r.name} label={`${r.name} · ${r.value}`} withArrow>
            <Group gap="xs" wrap="nowrap" mb={4}>
              <Text
                size="xs"
                ff={mono ? 'monospace' : undefined}
                style={{
                  width: 148,
                  flexShrink: 0,
                  whiteSpace: 'nowrap',
                  overflow: 'hidden',
                  textOverflow: 'ellipsis',
                }}
              >
                {label}
              </Text>
              <div style={{ flex: 1, background: '#2A1235', borderRadius: 3, height: 10 }}>
                <div
                  style={{
                    width: `${(Number(r.value) * 100) / max}%`,
                    background: color,
                    borderRadius: 3,
                    height: 10,
                  }}
                />
              </div>
              <Text size="xs" c="dimmed" ff="monospace" style={{ width: 42, textAlign: 'right' }}>
                {fmtCompact(r.value)}
              </Text>
            </Group>
          </Tooltip>
        )
      })}
    </div>
  )
}

/// One small labelled counter inside a card — the work-mix row.
function Counter({ label, value, tone }: { label: string; value: number | bigint | undefined; tone?: string }) {
  return (
    <div>
      <Text size="xs" c="dimmed" tt="uppercase" style={{ letterSpacing: 0.4 }}>
        {label}
      </Text>
      <Text fz={22} fw={700} ff="monospace" c={tone}>
        {fmtCompact(value)}
      </Text>
    </div>
  )
}

const baseName = (p: string) => p.split('/').slice(-2).join('/')

export default function StatusPage() {
  useBreadcrumb([{ label: 'Status' }])
  const status = useQuery({ queryKey: ['status'], queryFn: fetchStatus, refetchInterval: 10000 })
  // Scroll-back (#326): every window-scoped panel follows this.
  const [range, setRange] = useState<string>('window')
  const stats = useQuery({
    queryKey: ['stats', range],
    queryFn: () => fetchStats(range),
    refetchInterval: 15000,
  })
  // All-history aggregates: heavier, and they move slowly.
  const insights = useQuery({
    queryKey: ['insights'],
    queryFn: fetchInsights,
    refetchInterval: 60000,
  })
  const [modulesOpen, setModulesOpen] = useState(false)

  const s = stats.data
  const i = insights.data
  const RANGES: [string, string][] = [
    ['window', 'live'],
    ['1h', '1h'],
    ['6h', '6h'],
    ['24h', '24h'],
    ['7d', '7d'],
    ['30d', '30d'],
    ['all', 'all'],
  ]
  const windowNote = !s
    ? ''
    : s.range === 'window'
      ? `newest ${s.window_messages} captured messages`
      : s.truncated
        ? `${s.range} · sampled, row cap reached`
        : `over the last ${s.range}`

  const tok = i?.tokens
  const promptTotal = tok ? Number(tok.input) + Number(tok.cache_read) + Number(tok.cache_write) : 0
  const cacheHit = tok && promptTotal > 0 ? Math.round((Number(tok.cache_read) * 100) / promptTotal) : null
  const outcomes = s?.tool_outcomes ?? []
  const scored = outcomes.reduce((n, t) => n + Number(t.ok) + Number(t.failed) + Number(t.cancelled), 0)
  const failed = outcomes.reduce((n, t) => n + Number(t.failed), 0)
  const failRate = scored > 0 ? Math.round((failed * 1000) / scored) / 10 : null

  // Churn: replaced ÷ (added + replaced). 0% is greenfield; a high
  // number means the window spent itself rewriting (#324).
  const added = s ? Number(s.lines_added) : 0
  const replaced = s ? Number(s.lines_removed) : 0
  const churnPct = added + replaced > 0 ? Math.round((replaced * 100) / (added + replaced)) : null
  const churnTone = churnPct == null ? undefined : churnPct >= 50 ? FAIL : churnPct >= 25 ? CANCEL : OK
  const churnRead =
    churnPct == null
      ? ''
      : churnPct >= 50
        ? 'mostly rewriting'
        : churnPct >= 25
          ? 'revising as it goes'
          : 'mostly new code'
  const tokensPerLine = s && added > 0 ? Math.round(Number(s.out_tokens_window) / added) : null
  const testsPer100 = s && added > 0 ? Math.round((Number(s.tests_run) * 100 * 10) / added) / 10 : null

  const burn = s ? Number(s.tokens_last_hour) : 0
  const wr = s ? Number(s.writes_window) : 0
  const rd = s ? Number(s.reads_window) : 0
  const makeRatio = wr + rd > 0 ? Math.round((wr * 100) / (wr + rd)) : null

  // A 30-day range is ~720 hourly points: unreadable, and the labels
  // repeat. Fold to days for long ranges (review of #330).
  const churnSeries = (() => {
    const pts = s?.churn ?? []
    if (!(range === '7d' || range === '30d' || range === 'all')) return pts
    const byDay = new Map<string, { t: string; added: number; removed: number }>()
    for (const p of pts) {
      const day = p.t.slice(0, 10)
      const cur = byDay.get(day) ?? { t: day, added: 0, removed: 0 }
      cur.added += Number(p.added)
      cur.removed += Number(p.removed)
      byDay.set(day, cur)
    }
    return [...byDay.values()].sort((a, b) => a.t.localeCompare(b.t))
  })()

  // Why is it churning? (operator insight) Directed churn is the
  // design moving; self-directed is the agent rewriting itself.
  const cd = s ? Number(s.churn_directed) : 0
  const cs = s ? Number(s.churn_self) : 0
  const directedPct = cd + cs > 0 ? Math.round((cd * 100) / (cd + cs)) : null
  const churnCause =
    directedPct == null
      ? ''
      : directedPct >= 60
        ? 'the design is moving'
        : directedPct <= 30
          ? 'the agents are rewriting themselves'
          : 'mixed — some redirection, some rework'
  const fmtMs = (ms: number | bigint | null | undefined) => {
    if (ms == null) return '—'
    const v = Number(ms)
    if (v >= 3_600_000) return `${(v / 3_600_000).toFixed(1)}h`
    if (v >= 60_000) return `${Math.round(v / 60_000)}m`
    return `${Math.round(v / 1000)}s`
  }

  const q = s
    ? Number(s.tests_passed) + Number(s.tests_failed) > 0
      ? Math.round(
          (Number(s.tests_passed) * 1000) / (Number(s.tests_passed) + Number(s.tests_failed)),
        ) / 10
      : null
    : null

  return (
    <>
      {/* ── scroll-back: one control, every window-scoped panel ── */}
      <Group justify="space-between" mb="md" wrap="nowrap">
        <Group gap={6} wrap="nowrap">
          {RANGES.map(([key, label]) => (
            <Button
              key={key}
              size="compact-xs"
              variant={range === key ? 'filled' : 'default'}
              onClick={() => setRange(key)}
            >
              {label}
            </Button>
          ))}
        </Group>
        <Text size="xs" c="dimmed">
          {windowNote}
          {s?.truncated && ' — figures are a sample of this range'}
        </Text>
      </Group>

      {/* ── what is happening right now, per session ───────────── */}
      {(s?.live?.length ?? 0) > 0 && (
        <Card withBorder mb="md">
          <Group justify="space-between" mb="xs">
            <Title order={5}>Running now</Title>
            <Text size="xs" c="dimmed">
              sessions with a message in the last five minutes
            </Text>
          </Group>
          <Table striped highlightOnHover>
            <Table.Thead>
              <Table.Tr>
                <Table.Th>Session</Table.Th>
                <Table.Th>Repo</Table.Th>
                <Table.Th>Model</Table.Th>
                <Table.Th>Doing</Table.Th>
                <Table.Th ta="right">Msgs</Table.Th>
                <Table.Th ta="right">
                  <Tooltip
                    label="lines written, and lines an edit replaced — a whole-file Write counts entirely as added"
                    withArrow
                  >
                    <span>Lines +/−</span>
                  </Tooltip>
                </Table.Th>
                <Table.Th ta="right">Tokens</Table.Th>
                <Table.Th ta="right">Fails</Table.Th>
                <Table.Th ta="right">Idle</Table.Th>
              </Table.Tr>
            </Table.Thead>
            <Table.Tbody>
              {(s?.live ?? []).map((l) => (
                <Table.Tr key={l.identity}>
                  <Table.Td>
                    <Group gap={6} wrap="nowrap">
                      {/* The same green the Sessions page draws (#343).
                          Stated, not re-derived: the server already cut
                          this panel to the activity window, and asking
                          the question a second time on the client only
                          created a boundary the two answered differently
                          — a row at exactly the cut came back yellow
                          (#344 review). `idle_secs` cannot settle it
                          either; it is computed server-side and never
                          advances between fetches. */}
                      <LivenessDot state="active" size={8} />
                      <Text size="xs" ff="monospace">
                        {l.identity.slice(0, 13)}
                      </Text>
                    </Group>
                  </Table.Td>
                  <Table.Td>
                    <Text size="xs" ff="monospace">
                      {l.repo ?? '—'}
                      {l.branch && (
                        <Text span size="xs" c="dimmed">
                          {' · '}
                          {l.branch}
                        </Text>
                      )}
                    </Text>
                  </Table.Td>
                  <Table.Td>
                    {/* An effort qualifies a model — never shown on its
                        own, the way Sessions gates the same pair. The
                        model carries the weight: a coloured badge beside
                        dimmed grey text put the eye on `xhigh` in a
                        column headed Model (#344 review). */}
                    {l.model ? (
                      <Group gap={4} wrap="nowrap">
                        <Text size="xs">{l.model}</Text>
                        {l.effort && (
                          <Tooltip label="reasoning effort this session is running at" withArrow>
                            <Badge variant="outline" color="gray" size="xs">
                              {l.effort}
                            </Badge>
                          </Tooltip>
                        )}
                      </Group>
                    ) : (
                      <Text size="xs" c="dimmed">
                        —
                      </Text>
                    )}
                  </Table.Td>
                  <Table.Td>
                    {l.last_tool ? (
                      <Badge size="sm" variant="light">
                        {l.last_tool}
                      </Badge>
                    ) : (
                      '—'
                    )}
                  </Table.Td>
                  <Table.Td ta="right">{String(l.messages)}</Table.Td>
                  {/* Added alone reads 0 for a session deep in a
                      rewrite — show both halves (#343). */}
                  <Table.Td ta="right">
                    {/* `size="sm"` is the Table's own size. Mantine's
                        Text defaults to `md`, so an unsized Text is
                        LARGER than the bare cells beside it, not equal
                        to them (#344 review). */}
                    {Number(l.lines_added) === 0 && Number(l.lines_removed) === 0 ? (
                      <Text size="sm" c="dimmed">
                        —
                      </Text>
                    ) : (
                      <Group gap={6} justify="flex-end" wrap="nowrap">
                        <Text size="sm" c={OK}>
                          +{fmtCompact(l.lines_added)}
                        </Text>
                        <Text size="sm" c={FAIL}>
                          −{fmtCompact(l.lines_removed)}
                        </Text>
                      </Group>
                    )}
                  </Table.Td>
                  <Table.Td ta="right">{fmtCompact(l.out_tokens)}</Table.Td>
                  <Table.Td ta="right" c={Number(l.tool_failures) > 0 ? 'red.4' : undefined}>
                    {String(l.tool_failures)}
                  </Table.Td>
                  <Table.Td ta="right">{fmtAge(l.idle_secs)}</Table.Td>
                </Table.Tr>
              ))}
            </Table.Tbody>
          </Table>
        </Card>
      )}

      {/* ── quality: what the commands actually reported ───────── */}
      <SimpleGrid cols={{ base: 2, md: 3, lg: 6 }} mb="md">
        <Stat
          label="Tests passed"
          value={fmtCompact(s?.tests_passed)}
          sub={q == null ? '' : `${q}% pass rate`}
          tip="read out of what the test runners printed"
        />
        <Stat
          label="Tests failed"
          value={fmtCompact(s?.tests_failed)}
          tip="failing tests reported in tool output"
        />
        <Stat
          label="Compile errors"
          value={fmtCompact(s?.compile_errors)}
          tip="rustc / tsc diagnostics seen in tool output"
        />
        <Stat
          label="Interventions"
          value={fmtCompact(s?.interventions)}
          sub="you stepped in"
          tip="messages carrying an interruption or correction"
        />
        <Stat
          label="Compactions"
          value={fmtCompact(s?.compactions)}
          sub="context ran out"
          tip="the agent had to compact its context"
        />
        <Stat
          label="Denials"
          value={fmtCompact(s?.denials)}
          sub="not permitted"
          tip="tool calls the agent was not allowed to make"
        />
      </SimpleGrid>

      {/* ── flight strip: always NOW, never the selected range ── */}
      <Group justify="space-between" mb={6}>
        <Text size="xs" c="dimmed" tt="uppercase" style={{ letterSpacing: 0.5 }}>
          Right now
        </Text>
        <Text size="xs" c="dimmed">
          these five do not follow the range — they are live readings
        </Text>
      </Group>
      <SimpleGrid cols={{ base: 2, md: 3, lg: 5 }} mb="md">
        <Stat
          label="In the air"
          value={s ? `${s.sessions_active} live` : '…'}
          sub={s ? `${s.sessions_total} sessions total` : ''}
          tip="sessions with a message in the last five minutes"
        />
        <Stat
          label="Burn rate"
          value={`${fmtCompact(s?.tokens_last_hour)}/h`}
          sub="output tokens, last hour"
          tip="how fast the agents are producing right now"
        />
        <Stat
          label="Throughput"
          value={`${fmtCompact(s?.messages_last_hour)}/h`}
          sub="messages, last hour"
        />
        <Stat
          label="Capture lag"
          value={fmtAge(i?.last_event_secs)}
          sub={i ? `${fmtCompact(i.events_last_hour)} events this hour` : ''}
          tip="age of the newest captured event — the capture-alive signal"
        />
        <Card withBorder p="sm">
          <Text size="xs" c="dimmed" tt="uppercase" style={{ letterSpacing: 0.4 }}>
            Clock coverage
          </Text>
          <Group gap={6} align="baseline">
            <Text fz={22} fw={700} ff="monospace">
              {s ? `${s.active_hours_24h}/24` : '—'}
            </Text>
            <Text size="xs" c="dimmed">
              hours worked
            </Text>
          </Group>
          <CoverageStrip hours={s?.active_hours_24h} />
        </Card>
      </SimpleGrid>

      {/* ── the code panel: what happened to the codebase ──────── */}
      <Grid mb="md" gap="md">
        <Grid.Col span={{ base: 12, lg: 4 }}>
          <Card withBorder h="100%">
            <Group justify="space-between" mb="sm">
              <Title order={5}>Code written</Title>
              <Text size="xs" c="dimmed">
                {windowNote}
              </Text>
            </Group>
            <Group gap="xl" mb="sm">
              <div>
                <Text size="xs" c="dimmed" tt="uppercase" style={{ letterSpacing: 0.4 }}>
                  Lines added
                </Text>
                <Text fz={30} fw={700} ff="monospace" c={OK}>
                  +{fmtCompact(s?.lines_added)}
                </Text>
              </div>
              <div>
                <Text size="xs" c="dimmed" tt="uppercase" style={{ letterSpacing: 0.4 }}>
                  Replaced
                </Text>
                <Text fz={30} fw={700} ff="monospace" c={FAIL}>
                  −{fmtCompact(s?.lines_removed)}
                </Text>
              </div>
            </Group>
            <SimpleGrid cols={2} spacing="xs" mb="sm">
              <Counter label="Files touched" value={s?.files_touched} />
              <Counter label="Edits" value={s?.writes_window} />
            </SimpleGrid>
            {makeRatio != null && (
              <Tooltip label={`${wr} write calls vs ${rd} read calls`} withArrow>
                <div>
                  <Text size="xs" c="dimmed" mb={4}>
                    make ↔ inspect · {makeRatio}% writing
                  </Text>
                  <div style={{ background: '#2A1235', borderRadius: 3, height: 10 }}>
                    <div
                      style={{
                        width: `${makeRatio}%`,
                        background: OK,
                        borderRadius: 3,
                        height: 10,
                      }}
                    />
                  </div>
                </div>
              </Tooltip>
            )}
          </Card>
        </Grid.Col>

        <Grid.Col span={{ base: 12, lg: 4 }}>
          <Card withBorder h="100%">
            <Title order={5} mb="sm">
              Languages
            </Title>
            <BarList rows={s?.languages ?? []} mono empty="no files edited in this window" />
            <Title order={5} mt="md" mb="sm">
              Projects
            </Title>
            <BarList rows={s?.projects ?? []} color="var(--mantine-color-pelican-3)" mono />
          </Card>
        </Grid.Col>

        <Grid.Col span={{ base: 12, lg: 4 }}>
          <Card withBorder h="100%">
            <Title order={5} mb="sm">
              The work mix
            </Title>
            <SimpleGrid cols={3} spacing="xs" mb="md">
              <Counter label="Tests" value={s?.tests_run} tone={OK} />
              <Counter label="Builds" value={s?.builds_run} />
              <Counter label="Git ops" value={s?.git_ops} />
              <Counter label="Subagents" value={s?.subagent_calls} />
              <Counter label="MCP" value={s?.mcp_calls} />
              <Counter label="Web" value={s?.web_calls} />
            </SimpleGrid>
            <Title order={5} mb="sm">
              Commands
            </Title>
            <BarList
              rows={s?.commands ?? []}
              color="var(--mantine-color-pelican-6)"
              mono
              empty="no shell calls in this window"
            />
          </Card>
        </Grid.Col>
      </Grid>

      {/* ── churn: is the work accumulating, or being redone? ─── */}
      <Grid mb="md" gap="md">
        <Grid.Col span={{ base: 12, lg: 8 }}>
          <Card withBorder h="100%">
            <Group justify="space-between" mb="xs">
              <Title order={5}>Code churn — added against replaced</Title>
              <Text size="xs" c="dimmed">
                per hour · {windowNote}
              </Text>
            </Group>
            <EChart
              height={210}
              option={{
                grid: { left: 52, right: 12, top: 18, bottom: 26 },
                tooltip: { ...TOOLTIP, trigger: 'axis' },
                legend: {
                  data: ['added', 'replaced'],
                  textStyle: { color: INK_MUTED },
                  right: 0,
                  top: -2,
                },
                xAxis: {
                  type: 'category',
                  // Hour labels repeat once a range spans days, so
                  // long ranges show the date instead (review of #330).
                  data: churnSeries.map((p) =>
                    range === '7d' || range === '30d' || range === 'all'
                      ? p.t.slice(5, 10)
                      : p.t.slice(11) + ':00',
                  ),
                  axisLabel: { color: AXIS },
                  axisLine: { lineStyle: { color: GRID_LINE } },
                },
                yAxis: {
                  type: 'value',
                  axisLabel: { color: AXIS },
                  splitLine: { lineStyle: { color: GRID_LINE } },
                },
                series: [
                  {
                    name: 'added',
                    type: 'bar',
                    stack: 'churn',
                    data: churnSeries.map((p) => Number(p.added)),
                    itemStyle: { color: OK },
                  },
                  {
                    // Drawn below the axis so the two read as opposing
                    // forces rather than a sum.
                    name: 'replaced',
                    type: 'bar',
                    stack: 'churn',
                    data: churnSeries.map((p) => -Number(p.removed)),
                    itemStyle: { color: FAIL },
                  },
                ],
              }}
            />
          </Card>
        </Grid.Col>
        <Grid.Col span={{ base: 12, lg: 4 }}>
          <Card withBorder h="100%">
            <Title order={5} mb="xs">
              Churn ratio
            </Title>
            <Group gap="sm" align="baseline">
              <Text fz={44} fw={700} ff="monospace" c={churnTone}>
                {churnPct == null ? '—' : `${churnPct}%`}
              </Text>
              <Text size="sm" c="dimmed">
                {churnRead}
              </Text>
            </Group>
            <Tooltip label="replaced ÷ (added + replaced) — 0% is all new code" withArrow>
              <div style={{ background: '#2A1235', borderRadius: 3, height: 10, marginTop: 6 }}>
                <div
                  style={{
                    width: `${churnPct ?? 0}%`,
                    background: churnTone,
                    borderRadius: 3,
                    height: 10,
                  }}
                />
              </div>
            </Tooltip>
            <SimpleGrid cols={2} spacing="xs" mt="md">
              <Tooltip label="edits whose work a later edit threw away — a flip-flop counts twice" withArrow>
                <div>
                  <Counter label="Work undone" value={s?.reverts} tone={s && Number(s.reverts) > 0 ? FAIL : undefined} />
                </div>
              </Tooltip>
              <Tooltip label="files touched three or more times in this window" withArrow>
                <div>
                  <Counter label="Thrash files" value={s?.thrash_files} />
                </div>
              </Tooltip>
              <Tooltip label="output tokens spent per line of code that survived" withArrow>
                <div>
                  <Counter label="Tokens / line" value={tokensPerLine ?? undefined} />
                </div>
              </Tooltip>
              <div>
                <Text size="xs" c="dimmed" tt="uppercase" style={{ letterSpacing: 0.4 }}>
                  Tests / 100 lines
                </Text>
                <Text fz={22} fw={700} ff="monospace">
                  {testsPer100 == null ? '—' : testsPer100}
                </Text>
              </div>
            </SimpleGrid>
            {s?.top_repeat && (
              <Tooltip label="the same command, over and over — the shape of fighting something" withArrow>
                <Group gap="xs" mt="md" wrap="nowrap">
                  <Badge color="orange" variant="light">
                    ×{String(s.top_repeat.value)}
                  </Badge>
                  <Text size="sm" ff="monospace" style={{ overflow: 'hidden', textOverflow: 'ellipsis' }}>
                    {s.top_repeat.name}
                  </Text>
                </Group>
              </Tooltip>
            )}
            <Group gap="lg" mt="md">
              <Counter label="Agents at once" value={s?.max_concurrent_sessions} />
              <div>
                <Text size="xs" c="dimmed" tt="uppercase" style={{ letterSpacing: 0.4 }}>
                  Longest quiet
                </Text>
                <Text fz={22} fw={700} ff="monospace">
                  {s ? `${s.longest_quiet_mins}m` : '—'}
                </Text>
              </div>
            </Group>
          </Card>
        </Grid.Col>
      </Grid>

      {/* ── quality as a trend, and when it goes wrong ────────── */}
      <Grid mb="md" gap="md">
        <Grid.Col span={{ base: 12, lg: 8 }}>
          <Card withBorder h="100%">
            <Group justify="space-between" mb="xs">
              <Title order={5}>Quality over time</Title>
              <Text size="xs" c="dimmed">
                tests and tool failures per hour · {windowNote}
              </Text>
            </Group>
            <EChart
              height={200}
              option={{
                grid: { left: 52, right: 12, top: 18, bottom: 26 },
                tooltip: { ...TOOLTIP, trigger: 'axis' },
                legend: {
                  data: ['passed', 'failed', 'tool failures'],
                  textStyle: { color: INK_MUTED },
                  right: 0,
                  top: -2,
                },
                xAxis: {
                  type: 'category',
                  data: (s?.quality_series ?? []).map((p) => p.t.slice(11) + ':00'),
                  axisLabel: { color: AXIS },
                  axisLine: { lineStyle: { color: GRID_LINE } },
                },
                yAxis: {
                  type: 'value',
                  axisLabel: { color: AXIS },
                  splitLine: { lineStyle: { color: GRID_LINE } },
                },
                series: [
                  { name: 'passed', type: 'bar', stack: 'q', itemStyle: { color: OK },
                    data: (s?.quality_series ?? []).map((p) => Number(p.tests_passed)) },
                  { name: 'failed', type: 'bar', stack: 'q', itemStyle: { color: FAIL },
                    data: (s?.quality_series ?? []).map((p) => Number(p.tests_failed)) },
                  { name: 'tool failures', type: 'line', smooth: true, itemStyle: { color: CANCEL },
                    data: (s?.quality_series ?? []).map((p) => Number(p.tool_failures)) },
                ],
              }}
            />
          </Card>
        </Grid.Col>
        <Grid.Col span={{ base: 12, lg: 4 }}>
          <Card withBorder h="100%">
            <Title order={5} mb="xs">
              Why the churn
            </Title>
            <Text fz={30} fw={700} ff="monospace" c={directedPct != null && directedPct <= 30 ? FAIL : OK}>
              {directedPct == null ? '—' : `${directedPct}% directed`}
            </Text>
            <Text size="sm" c="dimmed" mb="xs">
              {churnCause}
            </Text>
            <Tooltip
              label="replaced lines that followed a human instruction, against those with nobody steering"
              withArrow
            >
              <Group gap={2} wrap="nowrap" mb="md">
                <div style={{ width: `${directedPct ?? 0}%`, background: OK, height: 10, borderRadius: '3px 0 0 3px' }} />
                <div style={{ flex: 1, background: FAIL, height: 10, borderRadius: '0 3px 3px 0' }} />
              </Group>
            </Tooltip>
            <SimpleGrid cols={2} spacing="xs">
              <Counter label="Directed" value={s?.churn_directed} tone={OK} />
              <Counter label="Self-inflicted" value={s?.churn_self} tone={FAIL} />
            </SimpleGrid>
            <Title order={5} mt="md" mb="xs">
              Waiting on operations
            </Title>
            <SimpleGrid cols={2} spacing="xs">
              <div>
                <Text size="xs" c="dimmed" tt="uppercase" style={{ letterSpacing: 0.4 }}>
                  Total wait
                </Text>
                <Text fz={22} fw={700} ff="monospace">
                  {fmtMs(s?.wait_ms_total)}
                </Text>
              </div>
              <div>
                <Text size="xs" c="dimmed" tt="uppercase" style={{ letterSpacing: 0.4 }}>
                  Median / p95
                </Text>
                <Text fz={22} fw={700} ff="monospace">
                  {fmtMs(s?.wait_ms_median)} / {fmtMs(s?.wait_ms_p95)}
                </Text>
              </div>
              <Counter label="Interrupted" value={s?.interrupted_calls} tone={s && Number(s.interrupted_calls) > 0 ? CANCEL : undefined} />
            </SimpleGrid>
          </Card>
        </Grid.Col>
      </Grid>

      {/* ── reasoning level against churn and productivity ─────── */}
      {(s?.efforts?.length ?? 0) > 0 && (
        <Card withBorder mb="md">
          <Group justify="space-between" mb="xs">
            <Title order={5}>Reasoning level against outcome</Title>
            <Text size="xs" c="dimmed">
              does thinking harder produce keepable code
            </Text>
          </Group>
          <Table striped>
            <Table.Thead>
              <Table.Tr>
                <Table.Th>Effort</Table.Th>
                <Table.Th ta="right">Msgs</Table.Th>
                <Table.Th ta="right">Lines +</Table.Th>
                <Table.Th ta="right">Churn</Table.Th>
                <Table.Th ta="right">Thinking</Table.Th>
                <Table.Th ta="right">Tok/line</Table.Th>
                <Table.Th ta="right">Undone</Table.Th>
                <Table.Th ta="right">Fails</Table.Th>
              </Table.Tr>
            </Table.Thead>
            <Table.Tbody>
              {(s?.efforts ?? []).map((e) => {
                const a = Number(e.lines_added)
                const d = Number(e.lines_removed)
                const pct = a + d > 0 ? Math.round((d * 100) / (a + d)) : null
                const tpl = a > 0 ? Math.round(Number(e.out_tokens) / a) : null
                return (
                  <Table.Tr key={e.name}>
                    <Table.Td>
                      <Badge variant="light" color="pelican">
                        {e.name}
                      </Badge>
                    </Table.Td>
                    <Table.Td ta="right">{fmtCompact(e.messages)}</Table.Td>
                    <Table.Td ta="right" c={OK}>
                      +{fmtCompact(a)}
                    </Table.Td>
                    <Table.Td ta="right" c={pct != null && pct >= 50 ? 'red.4' : undefined}>
                      {pct == null ? '—' : `${pct}%`}
                    </Table.Td>
                    <Table.Td ta="right">{fmtCompact(e.thinking_tokens)}</Table.Td>
                    <Table.Td ta="right">{tpl ?? '—'}</Table.Td>
                    <Table.Td ta="right" c={Number(e.reverts) > 0 ? 'orange.4' : undefined}>
                      {String(e.reverts)}
                    </Table.Td>
                    <Table.Td ta="right" c={Number(e.tool_failures) > 0 ? 'red.4' : undefined}>
                      {String(e.tool_failures)}
                    </Table.Td>
                  </Table.Tr>
                )
              })}
            </Table.Tbody>
          </Table>
        </Card>
      )}

      {/* ── the work cube: agent × repo × bucket (#340) ───────── */}
      {(s?.work_cells?.length ?? 0) > 0 && (
        <Card withBorder mb="md">
          <Group justify="space-between" mb="xs">
            <Title order={5}>Where the work went</Title>
            <Text size="xs" c="dimmed">
              lines written per agent, per repo, per {range === '7d' || range === '30d' || range === 'all' ? 'day' : 'hour'}
            </Text>
          </Group>
          <EChart
            height={300}
            option={(() => {
              const cells = s?.work_cells ?? []
              // One series per agent·repo pair, biggest first, the
              // tail folded into `other` so the legend stays readable.
              const totals = new Map<string, number>()
              for (const c of cells) {
                const k = `${c.agent} · ${c.repo}`
                totals.set(k, (totals.get(k) ?? 0) + Number(c.added))
              }
              const top = [...totals.entries()].sort((a, b) => b[1] - a[1]).slice(0, 7).map(([k]) => k)
              const buckets = [...new Set(cells.map((c) => c.t))].sort()
              const keyOf = (c: (typeof cells)[number]) => {
                const k = `${c.agent} · ${c.repo}`
                return top.includes(k) ? k : 'other'
              }
              const names = [...top, ...(cells.some((c) => keyOf(c) === 'other') ? ['other'] : [])]
              return {
                legend: { data: names, textStyle: { color: INK_MUTED, fontSize: 10 }, top: 0 },
                grid: { left: 48, right: 12, top: 30, bottom: 24 },
                xAxis: {
                  type: 'category',
                  data: buckets.map((b) => (b.includes('T') ? b.slice(11) + ':00' : b.slice(5))),
                  axisLabel: { color: AXIS, fontSize: 10 },
                  axisLine: { lineStyle: { color: GRID_LINE } },
                },
                yAxis: {
                  type: 'value',
                  axisLabel: { color: AXIS, fontSize: 10 },
                  splitLine: { lineStyle: { color: GRID_LINE } },
                },
                tooltip: { ...TOOLTIP, trigger: 'axis', axisPointer: { type: 'shadow' } },
                series: names.map((name, idx) => ({
                  name,
                  type: 'bar',
                  stack: 'work',
                  itemStyle: { color: CHART_COLORS[idx % CHART_COLORS.length] },
                  data: buckets.map((b) =>
                    cells
                      .filter((c) => c.t === b && keyOf(c) === name)
                      .reduce((acc, c) => acc + Number(c.added), 0),
                  ),
                })),
              }
            })()}
          />
        </Card>
      )}

      {/* ── the deeper reads (#340) ───────────────────────────── */}
      <Grid mb="md" gap="md">
        <Grid.Col span={{ base: 12, lg: 7 }}>
          <Card withBorder h="100%">
            <Group justify="space-between" mb="xs">
              <Title order={5}>How the work behaved</Title>
              <Text size="xs" c="dimmed">the same line count can mean opposite things</Text>
            </Group>
            <SimpleGrid cols={{ base: 2, sm: 3 }} spacing="xs">
              <Stat
                label="New files"
                value={fmtCompact(s?.files_created)}
                sub={`${fmtCompact(s?.files_modified)} already existed`}
                tip="a file whose oldest event in the window created it"
              />
              <Stat
                label="Repo switches"
                value={fmtCompact(s?.repo_switches)}
                sub="crossings mid-session"
                tip="an agent that keeps leaving is progressing in neither repo"
              />
              <Stat
                label="Edit → verify"
                value={
                  Number(s?.edit_to_verify_p50_secs ?? 0) > 0
                    ? fmtMs(Number(s?.edit_to_verify_p50_secs) * 1000)
                    : '—'
                }
                sub="median wait to check"
                tip="long, with high churn, is the signature of an agent guessing"
              />
              <Stat
                label="Code half-life"
                value={
                  Number(s?.survival_p50_mins ?? 0) > 0
                    ? fmtMs(Number(s?.survival_p50_mins) * 60_000)
                    : '—'
                }
                sub="before it was rewritten"
                tip="minutes means thrash; hours means the design moved"
              />
              <Stat
                label="Compactions"
                value={fmtCompact(s?.compactions)}
                sub={`${fmtMs(s?.compaction_total_ms)} lost`}
                tip="the agent stopped, re-read its history, resumed with less of it"
              />
              <Stat
                label="Churn"
                value={`${Math.round(
                  (Number(s?.churn_directed ?? 0) * 100) /
                    Math.max(1, Number(s?.churn_directed ?? 0) + Number(s?.churn_self ?? 0)),
                )}%`}
                sub="directed by you"
                tip="the rest the agents did to themselves"
              />
            </SimpleGrid>
          </Card>
        </Grid.Col>
        <Grid.Col span={{ base: 12, lg: 5 }}>
          <Card withBorder h="100%">
            <Group justify="space-between" mb="xs">
              <Title order={5}>Compaction</Title>
              <Text size="xs" c="dimmed">dead time, per session</Text>
            </Group>
            {(s?.compaction_sessions?.length ?? 0) === 0 ? (
              <Text size="xs" c="dimmed">No session hit its context ceiling in this range.</Text>
            ) : (
              <Table striped>
                <Table.Thead>
                  <Table.Tr>
                    <Table.Th>Session</Table.Th>
                    <Table.Th ta="right">Times</Table.Th>
                    <Table.Th ta="right">Lost</Table.Th>
                    <Table.Th ta="right">Context</Table.Th>
                  </Table.Tr>
                </Table.Thead>
                <Table.Tbody>
                  {(s?.compaction_sessions ?? []).slice(0, 6).map((c) => (
                    <Table.Tr key={c.identity}>
                      <Table.Td>
                        <Text size="xs" ff="monospace">{c.identity.slice(0, 20)}</Text>
                        <Text size="xs" c="dimmed">
                          {c.repo ?? '—'}
                          {Number(c.manual) > 0 ? ` · ${c.manual} manual` : ''}
                        </Text>
                      </Table.Td>
                      <Table.Td ta="right">{String(c.count)}</Table.Td>
                      <Table.Td ta="right" c="orange.4">{fmtMs(c.total_ms)}</Table.Td>
                      <Table.Td ta="right">{fmtCompact(c.pre_tokens_max)}</Table.Td>
                    </Table.Tr>
                  ))}
                </Table.Tbody>
              </Table>
            )}
          </Card>
        </Grid.Col>
      </Grid>

      {/* ── what each agent costs per line it keeps (#337) ────── */}
      {(s?.agent_stats?.length ?? 0) > 0 && (
        <Card withBorder mb="md">
          <Group justify="space-between" mb="xs">
            <Title order={5}>Agent productivity and what it cost</Title>
            <Text size="xs" c="dimmed">
              tokens spent per line that survived — lower is cheaper work
            </Text>
          </Group>
          <Table striped>
            <Table.Thead>
              <Table.Tr>
                <Table.Th>Agent</Table.Th>
                <Table.Th ta="right">Sessions</Table.Th>
                <Table.Th ta="right">Repos</Table.Th>
                <Table.Th ta="right">Lines +</Table.Th>
                <Table.Th ta="right">Churn</Table.Th>
                <Table.Th ta="right">Sent</Table.Th>
                <Table.Th ta="right">Written</Table.Th>
                <Table.Th ta="right">Tok/line</Table.Th>
                <Table.Th ta="right">Switches</Table.Th>
                <Table.Th ta="right">Verify</Table.Th>
                <Table.Th ta="right">Compact</Table.Th>
                <Table.Th ta="right">Undone</Table.Th>
                <Table.Th ta="right">Fails</Table.Th>
              </Table.Tr>
            </Table.Thead>
            <Table.Tbody>
              {(s?.agent_stats ?? []).map((a) => {
                const add = Number(a.lines_added)
                const del = Number(a.lines_removed)
                const pct = add + del > 0 ? Math.round((del * 100) / (add + del)) : null
                // Everything the turn cost, against what it left behind.
                const cost = add > 0 ? Math.round((Number(a.in_tokens) + Number(a.out_tokens)) / add) : null
                return (
                  <Table.Tr key={a.name}>
                    <Table.Td>
                      <Badge variant="light" color="pelican">
                        {a.name}
                      </Badge>
                    </Table.Td>
                    <Table.Td ta="right">{String(a.sessions)}</Table.Td>
                    <Table.Td ta="right">{String(a.repos)}</Table.Td>
                    <Table.Td ta="right" c={OK}>
                      +{fmtCompact(add)}
                    </Table.Td>
                    <Table.Td ta="right" c={pct != null && pct >= 50 ? 'red.4' : undefined}>
                      {pct == null ? '—' : `${pct}%`}
                    </Table.Td>
                    <Table.Td ta="right">{fmtCompact(a.in_tokens)}</Table.Td>
                    <Table.Td ta="right">{fmtCompact(a.out_tokens)}</Table.Td>
                    <Table.Td ta="right">{cost == null ? '—' : fmtCompact(cost)}</Table.Td>
                    <Table.Td ta="right" c={Number(a.repo_switches) > 20 ? 'orange.4' : undefined}>
                      {String(a.repo_switches)}
                    </Table.Td>
                    <Table.Td ta="right">
                      {Number(a.edit_to_verify_p50_secs) > 0
                        ? fmtMs(Number(a.edit_to_verify_p50_secs) * 1000)
                        : '—'}
                    </Table.Td>
                    <Table.Td ta="right">
                      {Number(a.compactions) > 0
                        ? `${a.compactions} · ${fmtMs(a.compaction_ms)}`
                        : '—'}
                    </Table.Td>
                    <Table.Td ta="right" c={Number(a.reverts) > 0 ? 'orange.4' : undefined}>
                      {String(a.reverts)}
                    </Table.Td>
                    <Table.Td ta="right" c={Number(a.tool_failures) > 0 ? 'red.4' : undefined}>
                      {String(a.tool_failures)}
                    </Table.Td>
                  </Table.Tr>
                )
              })}
            </Table.Tbody>
          </Table>
        </Card>
      )}

      {/* ── what left this machine, and what was kept (#337) ───── */}
      <Card withBorder mb="md">
        <Group justify="space-between" mb="xs">
          <Title order={5}>What left this machine</Title>
          <Text size="xs" c="dimmed">
            measured from your own transcripts — what the vendor does with it
            afterwards is not observable from here
          </Text>
        </Group>
        <SimpleGrid cols={{ base: 2, sm: 3, lg: 6 }} spacing="xs" mb="sm">
          <Stat label="Sent fresh" value={fmtCompact(s?.exposure?.input_tokens ?? 0)} sub="prompt tokens" />
          <Stat
            label="Cached by vendor"
            value={fmtCompact(s?.exposure?.cache_write_tokens ?? 0)}
            sub="stored their side"
          />
          <Stat label="Served back" value={fmtCompact(s?.exposure?.cache_read_tokens ?? 0)} sub="cache reads" />
          <Stat label="File text" value={fmtBytes(Number(s?.exposure?.content_bytes ?? 0))} sub="into prompts" />
          <Stat label="Files read" value={fmtCompact(s?.exposure?.files_read ?? 0)} sub={`${s?.exposure?.repos_exposed ?? 0} repos`} />
          <Stat label="Attachments" value={fmtCompact(s?.exposure?.attachments ?? 0)} sub="images, docs" />
        </SimpleGrid>
        {Number(s?.exposure?.outside_reads ?? 0) > 0 && (
          <Text size="xs" c="orange.4" mb={4}>
            {fmtCompact(s?.exposure?.outside_reads ?? 0)} file reads came from outside the
            directory the agent was working in.
          </Text>
        )}
        {Number(s?.exposure?.secret_hits ?? 0) > 0 ? (
          <Card withBorder bg="dark.8" p="xs">
            <Text size="sm" c="red.4" fw={600}>
              {String(s?.exposure?.secret_hits)} tool result
              {Number(s?.exposure?.secret_hits) === 1 ? '' : 's'} carried credential-shaped
              content into a prompt
            </Text>
            <Text size="xs" c="dimmed">
              {(s?.exposure?.secret_paths ?? []).slice(0, 8).join(' · ') || 'path not recorded'}
            </Text>
            <Text size="xs" c="dimmed" mt={4}>
              Anything sent cannot be recalled. Rotate what was exposed.
            </Text>
          </Card>
        ) : (
          <Text size="xs" c="dimmed">
            No credential-shaped content detected in what was sent.
          </Text>
        )}
      </Card>

      {/* ── many agents, many repos: one row each ─────────────── */}
      <Grid mb="md" gap="md">
        <Grid.Col span={{ base: 12, lg: 7 }}>
          <Card withBorder h="100%">
            <Group justify="space-between" mb="xs">
              <Title order={5}>Repos worked</Title>
              <Text size="xs" c="dimmed">
                one busy repo should not hide a thrashing one
              </Text>
            </Group>
            <Table striped highlightOnHover>
              <Table.Thead>
                <Table.Tr>
                  <Table.Th>Repo</Table.Th>
                  <Table.Th ta="right">Agents</Table.Th>
                  <Table.Th ta="right">Msgs</Table.Th>
                  <Table.Th ta="right">+/−</Table.Th>
                  <Table.Th ta="right">Churn</Table.Th>
                  <Table.Th ta="right">Files</Table.Th>
                  <Table.Th ta="right">New</Table.Th>
                  <Table.Th ta="right">Half-life</Table.Th>
                  <Table.Th ta="right">Tests</Table.Th>
                  <Table.Th ta="right">Fails</Table.Th>
                </Table.Tr>
              </Table.Thead>
              <Table.Tbody>
                {(s?.repos ?? []).map((r) => {
                  const a = Number(r.lines_added)
                  const d = Number(r.lines_removed)
                  const pct = a + d > 0 ? Math.round((d * 100) / (a + d)) : null
                  return (
                    <Table.Tr key={r.name}>
                      <Table.Td>
                        <Text size="xs" ff="monospace">
                          {r.name}
                        </Text>
                        {r.branch && (
                          <Text size="xs" c="dimmed">
                            {r.branch}
                          </Text>
                        )}
                      </Table.Td>
                      <Table.Td ta="right">{String(r.agents)}</Table.Td>
                      <Table.Td ta="right">{fmtCompact(r.messages)}</Table.Td>
                      <Table.Td ta="right">
                        <Text span size="xs" c={OK}>
                          +{fmtCompact(a)}
                        </Text>{' '}
                        <Text span size="xs" c={FAIL}>
                          −{fmtCompact(d)}
                        </Text>
                      </Table.Td>
                      <Table.Td ta="right" c={pct != null && pct >= 50 ? 'red.4' : undefined}>
                        {pct == null ? '—' : `${pct}%`}
                      </Table.Td>
                      <Table.Td ta="right">{String(r.files_touched)}</Table.Td>
                      <Table.Td ta="right" c={Number(r.files_created) > 0 ? OK : undefined}>
                        {String(r.files_created)}
                      </Table.Td>
                      <Table.Td ta="right">
                        {Number(r.survival_p50_mins) > 0
                          ? fmtMs(Number(r.survival_p50_mins) * 60_000)
                          : '—'}
                      </Table.Td>
                      <Table.Td ta="right">{String(r.tests_run)}</Table.Td>
                      <Table.Td ta="right" c={Number(r.tool_failures) > 0 ? 'red.4' : undefined}>
                        {String(r.tool_failures)}
                      </Table.Td>
                    </Table.Tr>
                  )
                })}
              </Table.Tbody>
            </Table>
          </Card>
        </Grid.Col>
        <Grid.Col span={{ base: 12, lg: 5 }}>
          <Card withBorder h="100%">
            <Group justify="space-between" mb="xs">
              <Title order={5}>Model against model</Title>
              <Text size="xs" c="dimmed">
                which one produces keepable code
              </Text>
            </Group>
            <Table striped>
              <Table.Thead>
                <Table.Tr>
                  <Table.Th>Model</Table.Th>
                  <Table.Th ta="right">Churn</Table.Th>
                  <Table.Th ta="right">Tok/line</Table.Th>
                  <Table.Th ta="right">Undone</Table.Th>
                  <Table.Th ta="right">Fails</Table.Th>
                </Table.Tr>
              </Table.Thead>
              <Table.Tbody>
                {(s?.models ?? []).filter((m) => m.name !== 'unknown').map((m) => {
                  const a = Number(m.lines_added)
                  const d = Number(m.lines_removed)
                  const pct = a + d > 0 ? Math.round((d * 100) / (a + d)) : null
                  const tpl = a > 0 ? Math.round(Number(m.out_tokens) / a) : null
                  return (
                    <Table.Tr key={m.name}>
                      <Table.Td>
                        <Text size="xs" ff="monospace">
                          {m.name}
                        </Text>
                        <Text size="xs" c="dimmed">
                          {fmtCompact(m.messages)} msgs · +{fmtCompact(a)} lines
                        </Text>
                      </Table.Td>
                      <Table.Td ta="right" c={pct != null && pct >= 50 ? 'red.4' : undefined}>
                        {pct == null ? '—' : `${pct}%`}
                      </Table.Td>
                      <Table.Td ta="right">{tpl ?? '—'}</Table.Td>
                      <Table.Td ta="right" c={Number(m.reverts) > 0 ? 'orange.4' : undefined}>
                        {String(m.reverts)}
                      </Table.Td>
                      <Table.Td ta="right" c={Number(m.tool_failures) > 0 ? 'red.4' : undefined}>
                        {String(m.tool_failures)}
                      </Table.Td>
                    </Table.Tr>
                  )
                })}
              </Table.Tbody>
            </Table>
          </Card>
        </Grid.Col>
      </Grid>

      <Grid mb="md" gap="md">
        <Grid.Col span={{ base: 12, lg: 7 }}>
          <Card withBorder h="100%">
            <Group justify="space-between" mb="sm">
              <Title order={5}>Hottest files</Title>
              <Text size="xs" c="dimmed">
                most-touched paths · {windowNote}
              </Text>
            </Group>
            <BarList rows={s?.files ?? []} mono shorten={baseName} />
          </Card>
        </Grid.Col>
        <Grid.Col span={{ base: 12, lg: 5 }}>
          <Card withBorder h="100%">
            <Group justify="space-between" mb="sm">
              <Title order={5}>Where the work happened</Title>
              <Text size="xs" c="dimmed">
                directories
              </Text>
            </Group>
            <BarList rows={s?.dirs ?? []} color="var(--mantine-color-pelican-3)" mono />
            <Group gap="xl" mt="md">
              <Counter label="Thinking tokens" value={s?.thinking_tokens} />
              <Counter label="Tool calls" value={s?.tools_window} />
            </Group>
          </Card>
        </Grid.Col>
      </Grid>

      <SimpleGrid cols={{ base: 2, md: 4, lg: 7 }} mb="md">
        <Stat label="Agents" value={s ? String(s.agents) : '…'} />
        <Stat
          label="Sessions"
          value={s ? `${s.sessions_active} live` : '…'}
          sub={s ? `${s.sessions_total} total` : ''}
        />
        <Stat label="Events captured" value={fmtCompact(s?.events_total)} />
        <Stat label="Messages" value={fmtCompact(s?.messages_total)} />
        <Stat label="Output tokens" value={fmtCompact(s?.output_tokens_total)} sub="all sessions" />
        <Stat
          label="Lines written"
          value={fmtCompact(s?.lines_written)}
          sub={windowNote}
          tip={`code written by Write/Edit tools across the ${windowNote}`}
        />
        <Stat
          label="Tool calls"
          value={fmtCompact(s?.tools_window)}
          sub={windowNote}
          tip={`tool invocations across the ${windowNote}`}
        />
      </SimpleGrid>

      <Grid mb="md" gap="md">
        <Grid.Col span={{ base: 12, lg: 7 }}>
          <Card withBorder>
            <Title order={5} mb="xs">
              Activity — events per minute
            </Title>
            <EChart
              height={220}
              option={{
                grid: { left: 44, right: 12, top: 16, bottom: 28 },
                tooltip: { trigger: 'axis', ...TOOLTIP },
                xAxis: {
                  type: 'category',
                  data: (s?.events_per_minute ?? []).map((p) => p.t),
                  ...AXIS,
                  splitLine: { show: false },
                },
                yAxis: { type: 'value', ...AXIS },
                series: [
                  {
                    type: 'line',
                    data: (s?.events_per_minute ?? []).map((p) => Number(p.value)),
                    smooth: 0.3,
                    symbol: 'none',
                    lineStyle: { width: 2, color: '#B833E8' },
                    areaStyle: { color: '#B833E8', opacity: 0.18 },
                  },
                ],
              }}
            />
          </Card>
        </Grid.Col>
        <Grid.Col span={{ base: 12, lg: 5 }}>
          <Card withBorder>
            <Group justify="space-between" mb="xs">
              <Title order={5}>What the agents run</Title>
              <Text size="xs" c="dimmed">
                {windowNote}
              </Text>
            </Group>
            <EChart
              height={220}
              option={{
                tooltip: { trigger: 'item', ...TOOLTIP },
                legend: {
                  orient: 'vertical',
                  right: 0,
                  top: 'middle',
                  textStyle: { color: INK_MUTED, fontSize: 11 },
                },
                series: [
                  {
                    type: 'pie',
                    radius: ['52%', '78%'],
                    center: ['35%', '50%'],
                    itemStyle: { borderColor: '#2A1235', borderWidth: 2 },
                    label: { show: false },
                    data: (s?.tools ?? []).slice(0, 8).map((t) => ({
                      name: t.name,
                      value: Number(t.value),
                    })),
                  },
                ],
              }}
            />
          </Card>
        </Grid.Col>
      </Grid>

      <Grid mb="md" gap="md">
        <Grid.Col span={{ base: 12, lg: 7 }}>
          <Card withBorder>
            <Group justify="space-between" mb="xs">
              <Title order={5}>Busiest sessions</Title>
              <Text size="xs" c="dimmed">
                bars wear each session's feed color · {windowNote}
              </Text>
            </Group>
            <EChart
              height={230}
              option={{
                grid: { left: 170, right: 60, top: 8, bottom: 24 },
                tooltip: {
                  trigger: 'item',
                  ...TOOLTIP,
                  formatter: (p: { dataIndex: number }) => {
                    const t = (s?.top_sessions ?? [])[p.dataIndex]
                    return t
                      ? `${t.identity}<br/>${t.messages} messages · ${fmtCompact(t.lines_written)} lines · ${fmtCompact(t.output_tokens)} out tokens`
                      : ''
                  },
                },
                xAxis: { type: 'value', ...AXIS },
                yAxis: {
                  type: 'category',
                  data: (s?.top_sessions ?? []).map((t) => t.identity).reverse(),
                  ...AXIS,
                  axisLabel: {
                    color: INK_MUTED,
                    fontSize: 11,
                    fontFamily: "'JetBrains Mono', ui-monospace, Menlo, monospace",
                  },
                  splitLine: { show: false },
                },
                series: [
                  {
                    type: 'bar',
                    barWidth: 14,
                    data: (s?.top_sessions ?? [])
                      .map((t) => ({
                        value: Number(t.messages),
                        itemStyle: { color: sessionColor(t.session_id), borderRadius: 3 },
                      }))
                      .reverse(),
                    label: {
                      show: true,
                      position: 'right',
                      color: INK_MUTED,
                      fontSize: 11,
                    },
                  },
                ],
              }}
            />
          </Card>
        </Grid.Col>
        <Grid.Col span={{ base: 6, lg: 2.5 }}>
          <Card withBorder>
            <Title order={5} mb="xs">
              Message roles
            </Title>
            <EChart
              height={230}
              option={{
                grid: { left: 40, right: 8, top: 8, bottom: 24 },
                tooltip: { trigger: 'axis', ...TOOLTIP },
                xAxis: {
                  type: 'category',
                  data: (s?.message_roles ?? []).map((r) => r.name),
                  ...AXIS,
                  splitLine: { show: false },
                },
                yAxis: { type: 'value', ...AXIS },
                series: [
                  {
                    type: 'bar',
                    barWidth: 18,
                    data: (s?.message_roles ?? []).map((r, i) => ({
                      value: Number(r.value),
                      itemStyle: {
                        color: CHART_COLORS[i % CHART_COLORS.length],
                        borderRadius: 3,
                      },
                    })),
                  },
                ],
              }}
            />
          </Card>
        </Grid.Col>
        <Grid.Col span={{ base: 6, lg: 2.5 }}>
          <Card withBorder>
            <Title order={5} mb="xs">
              Boot durations
            </Title>
            <EChart
              height={230}
              option={{
                grid: { left: 44, right: 8, top: 8, bottom: 24 },
                tooltip: { trigger: 'axis', ...TOOLTIP },
                xAxis: {
                  type: 'category',
                  data: (s?.boot_durations ?? []).map((b) => b.t),
                  ...AXIS,
                  axisLabel: { show: false },
                  splitLine: { show: false },
                },
                yAxis: {
                  type: 'value',
                  ...AXIS,
                  axisLabel: { ...AXIS.axisLabel, formatter: '{value}ms' },
                },
                series: [
                  {
                    type: 'line',
                    data: (s?.boot_durations ?? []).map((b) => Number(b.value)),
                    symbol: 'circle',
                    symbolSize: 6,
                    lineStyle: { width: 2, color: '#199e70' },
                    itemStyle: { color: '#199e70' },
                  },
                ],
              }}
            />
          </Card>
        </Grid.Col>
      </Grid>

      <SimpleGrid cols={{ base: 2, md: 3, lg: 5 }} mb="md">
        <Stat
          label="Prompt tokens"
          value={fmtCompact(tok ? Number(tok.input) : undefined)}
          sub="sent fresh"
          tip="input tokens billed as new context, across every captured message"
        />
        <Stat
          label="Cache reads"
          value={fmtCompact(tok ? Number(tok.cache_read) : undefined)}
          sub="served from cache"
          tip="prompt tokens answered from cache instead of being re-sent"
        />
        <Stat
          label="Cache hit"
          value={cacheHit == null ? '…' : `${cacheHit}%`}
          sub="of all prompt tokens"
          tip="cache reads ÷ (cache reads + fresh input + cache writes)"
        />
        <Stat
          label="Tool failures"
          value={failRate == null ? '…' : `${failRate}%`}
          sub={scored > 0 ? `${failed} of ${scored} scored` : windowNote}
          tip={`calls that came back an error, across the ${windowNote}`}
        />
        <Stat
          label="Capture lag"
          value={i ? fmtAge(i.last_event_secs) : '…'}
          sub={i ? `${fmtCompact(i.events_last_hour)} events this hour` : ''}
          tip="age of the newest captured event — how current the OS's picture is"
        />
      </SimpleGrid>

      {(i?.events_per_day.length ?? 0) > 0 && (
        <Card withBorder mb="md">
          <Group justify="space-between" mb="xs">
            <Title order={5}>The work calendar — messages by the day they were written</Title>
            <Text size="xs" c="dimmed">
              the agent&apos;s own clock, not our capture time
            </Text>
          </Group>
          <EChart
            height={190}
            option={{
              tooltip: {
                ...TOOLTIP,
                formatter: (p: { value: [string, number] }) =>
                  `${p.value[0]}<br/>${fmtCompact(p.value[1])} messages`,
              },
              visualMap: {
                min: 0,
                max: Math.max(...(i?.events_per_day ?? []).map((d) => Number(d.value)), 1),
                orient: 'horizontal',
                left: 44,
                bottom: 4,
                itemWidth: 10,
                itemHeight: 140,
                text: ['busy', 'quiet'],
                textStyle: { color: INK_MUTED, fontSize: 10 },
                inRange: { color: HEAT },
              },
              calendar: {
                top: 26,
                left: 44,
                // Square cells: with `auto` width a short range stretches
                // each day into a stripe and stops reading as a calendar.
                cellSize: [17, 17],
                range: [
                  i?.events_per_day[0].t,
                  i?.events_per_day[(i?.events_per_day.length ?? 1) - 1].t,
                ],
                itemStyle: { color: '#150420', borderColor: GRID_LINE, borderWidth: 1 },
                yearLabel: { show: false },
                monthLabel: { color: INK_MUTED, fontSize: 11 },
                dayLabel: {
                  color: INK_MUTED,
                  fontSize: 10,
                  firstDay: 1,
                  nameMap: ['Su', 'Mo', 'Tu', 'We', 'Th', 'Fr', 'Sa'],
                },
                splitLine: { lineStyle: { color: GRID_LINE } },
              },
              series: [
                {
                  type: 'heatmap',
                  coordinateSystem: 'calendar',
                  data: (i?.events_per_day ?? []).map((d) => [d.t, Number(d.value)]),
                },
              ],
            }}
          />
        </Card>
      )}

      <Grid mb="md" gap="md">
        <Grid.Col span={{ base: 12, lg: 7 }}>
          <Card withBorder>
            <Title order={5} mb="xs">
              When the agents work — hour of day × day of week
            </Title>
            <EChart
              height={240}
              option={{
                tooltip: {
                  ...TOOLTIP,
                  formatter: (p: { value: [number, number, number] }) =>
                    `${HEAT_ROWS[p.value[1]]} ${String(p.value[0]).padStart(2, '0')}:00<br/>${fmtCompact(
                      p.value[2],
                    )} messages`,
                },
                grid: { left: 46, right: 14, top: 10, bottom: 40 },
                xAxis: {
                  type: 'category',
                  data: Array.from({ length: 24 }, (_, h) => String(h).padStart(2, '0')),
                  ...AXIS,
                  splitLine: { show: false },
                },
                yAxis: { type: 'category', data: HEAT_ROWS, ...AXIS, splitLine: { show: false } },
                visualMap: {
                  min: 0,
                  max: Math.max(...(i?.hour_weekday ?? []).map((c) => Number(c.value)), 1),
                  show: false,
                  inRange: { color: HEAT },
                },
                series: [
                  {
                    type: 'heatmap',
                    // weekday is time::wday: 1 = Monday … 7 = Sunday.
                    data: (i?.hour_weekday ?? []).map((c) => [
                      Number(c.hour),
                      7 - Number(c.weekday),
                      Number(c.value),
                    ]),
                    itemStyle: { borderColor: '#1F062A', borderWidth: 1 },
                  },
                ],
              }}
            />
          </Card>
        </Grid.Col>
        <Grid.Col span={{ base: 12, lg: 5 }}>
          <Card withBorder>
            <Group justify="space-between" mb="xs">
              <Title order={5}>Which models did the work</Title>
              <Text size="xs" c="dimmed">
                messages, all history
              </Text>
            </Group>
            <EChart
              height={240}
              option={{
                tooltip: { ...TOOLTIP, trigger: 'item' },
                legend: {
                  orient: 'vertical',
                  right: 0,
                  top: 'middle',
                  textStyle: { color: INK_MUTED, fontSize: 11 },
                },
                series: [
                  {
                    type: 'pie',
                    radius: ['52%', '78%'],
                    center: ['32%', '50%'],
                    itemStyle: { borderColor: '#2A1235', borderWidth: 2 },
                    label: { show: false },
                    data: (i?.models ?? []).slice(0, 8).map((m) => ({
                      name: m.name,
                      value: Number(m.value),
                    })),
                  },
                ],
              }}
            />
          </Card>
        </Grid.Col>
      </Grid>

      <Grid mb="md" gap="md">
        <Grid.Col span={{ base: 12, lg: 4 }}>
          <Card withBorder>
            <Title order={5} mb="xs">
              Where the tokens went
            </Title>
            <EChart
              height={220}
              option={{
                tooltip: {
                  ...TOOLTIP,
                  trigger: 'axis',
                  axisPointer: { type: 'shadow' },
                  valueFormatter: (v: number) => fmtCompact(v),
                },
                grid: { left: 92, right: 46, top: 8, bottom: 24 },
                xAxis: { type: 'value', ...AXIS, axisLabel: { ...AXIS.axisLabel, show: false } },
                yAxis: {
                  type: 'category',
                  data: ['cache write', 'output', 'fresh input', 'cache read'],
                  ...AXIS,
                  splitLine: { show: false },
                },
                series: [
                  {
                    type: 'bar',
                    barWidth: 16,
                    itemStyle: { borderRadius: 3 },
                    label: {
                      show: true,
                      position: 'right',
                      color: INK_MUTED,
                      fontSize: 11,
                      formatter: (p: { value: number }) => fmtCompact(p.value),
                    },
                    data: tok
                      ? [
                          { value: Number(tok.cache_write), itemStyle: { color: CHART_COLORS[5] } },
                          { value: Number(tok.output), itemStyle: { color: CHART_COLORS[2] } },
                          { value: Number(tok.input), itemStyle: { color: CHART_COLORS[1] } },
                          { value: Number(tok.cache_read), itemStyle: { color: CHART_COLORS[0] } },
                        ]
                      : [],
                  },
                ],
              }}
            />
          </Card>
        </Grid.Col>
        <Grid.Col span={{ base: 12, lg: 4 }}>
          <Card withBorder>
            <Group justify="space-between" mb="xs">
              <Title order={5}>Cache efficiency</Title>
              <Text size="xs" c="dimmed">
                prompt served from cache
              </Text>
            </Group>
            <EChart
              height={220}
              option={{
                series: [
                  {
                    type: 'gauge',
                    startAngle: 210,
                    endAngle: -30,
                    min: 0,
                    max: 100,
                    radius: '92%',
                    progress: { show: true, width: 16, itemStyle: { color: CHART_COLORS[0] } },
                    axisLine: { lineStyle: { width: 16, color: [[1, '#2A1235']] } },
                    pointer: { show: false },
                    axisTick: { show: false },
                    splitLine: { show: false },
                    axisLabel: { show: false },
                    anchor: { show: false },
                    title: { show: false },
                    detail: {
                      valueAnimation: true,
                      fontSize: 30,
                      fontWeight: 600,
                      color: INK,
                      offsetCenter: [0, 0],
                      formatter: '{value}%',
                    },
                    data: [{ value: cacheHit ?? 0 }],
                  },
                ],
              }}
            />
          </Card>
        </Grid.Col>
        <Grid.Col span={{ base: 12, lg: 4 }}>
          <Card withBorder>
            <Group justify="space-between" mb="xs">
              <Title order={5}>Who did the work</Title>
              <Text size="xs" c="dimmed">
                messages by agent
              </Text>
            </Group>
            <EChart
              height={220}
              option={{
                tooltip: {
                  ...TOOLTIP,
                  trigger: 'axis',
                  axisPointer: { type: 'shadow' },
                  valueFormatter: (v: number) => fmtCompact(v),
                },
                grid: { left: 110, right: 46, top: 8, bottom: 24 },
                xAxis: { type: 'value', ...AXIS, axisLabel: { ...AXIS.axisLabel, show: false } },
                yAxis: {
                  type: 'category',
                  data: (i?.per_agent ?? []).map((a) => a.name).reverse(),
                  ...AXIS,
                  splitLine: { show: false },
                },
                series: [
                  {
                    type: 'bar',
                    barWidth: 16,
                    itemStyle: { borderRadius: 3 },
                    label: {
                      show: true,
                      position: 'right',
                      color: INK_MUTED,
                      fontSize: 11,
                      formatter: (p: { value: number }) => fmtCompact(p.value),
                    },
                    data: (i?.per_agent ?? [])
                      .map((a, n) => ({
                        value: Number(a.messages),
                        itemStyle: { color: CHART_COLORS[n % CHART_COLORS.length] },
                      }))
                      .reverse(),
                  },
                ],
              }}
            />
          </Card>
        </Grid.Col>
      </Grid>

      <Grid mb="md" gap="md">
        <Grid.Col span={{ base: 12, lg: 7 }}>
          <Card withBorder>
            <Group justify="space-between" mb="xs">
              <Title order={5}>Did the calls work?</Title>
              <Text size="xs" c="dimmed">
                {windowNote}
              </Text>
            </Group>
            <EChart
              height={250}
              option={{
                tooltip: { ...TOOLTIP, trigger: 'axis', axisPointer: { type: 'shadow' } },
                legend: {
                  data: ['ok', 'failed', 'cancelled', 'no result yet'],
                  textStyle: { color: INK_MUTED, fontSize: 11 },
                  top: 0,
                  right: 0,
                },
                grid: { left: 130, right: 20, top: 30, bottom: 24 },
                xAxis: { type: 'value', ...AXIS },
                yAxis: {
                  type: 'category',
                  data: outcomes.slice(0, 8).map((t) => t.name).reverse(),
                  ...AXIS,
                  splitLine: { show: false },
                },
                series: [
                  ['ok', OK, 'ok'],
                  ['failed', FAIL, 'failed'],
                  ['cancelled', CANCEL, 'cancelled'],
                  ['no result yet', UNKNOWN, 'unknown'],
                ].map(([label, color, key]) => ({
                  name: label,
                  type: 'bar',
                  stack: 'calls',
                  barWidth: 14,
                  itemStyle: { color },
                  data: outcomes
                    .slice(0, 8)
                    .map((t) => Number(t[key as 'ok' | 'failed' | 'cancelled' | 'unknown']))
                    .reverse(),
                })),
              }}
            />
          </Card>
        </Grid.Col>
        <Grid.Col span={{ base: 12, lg: 5 }}>
          <Card withBorder>
            <Group justify="space-between" mb="xs">
              <Title order={5}>What capture spends itself on</Title>
              <Text size="xs" c="dimmed">
                all history
              </Text>
            </Group>
            <EChart
              height={250}
              option={{
                tooltip: {
                  ...TOOLTIP,
                  trigger: 'axis',
                  axisPointer: { type: 'shadow' },
                  valueFormatter: (v: number) => fmtCompact(v),
                },
                grid: { left: 130, right: 46, top: 8, bottom: 24 },
                xAxis: { type: 'value', ...AXIS, axisLabel: { ...AXIS.axisLabel, show: false } },
                yAxis: {
                  type: 'category',
                  data: (i?.event_kinds ?? []).slice(0, 8).map((k) => k.name).reverse(),
                  ...AXIS,
                  splitLine: { show: false },
                },
                series: [
                  {
                    type: 'bar',
                    barWidth: 14,
                    itemStyle: { borderRadius: 3, color: CHART_COLORS[0] },
                    label: {
                      show: true,
                      position: 'right',
                      color: INK_MUTED,
                      fontSize: 11,
                      formatter: (p: { value: number }) => fmtCompact(p.value),
                    },
                    data: (i?.event_kinds ?? [])
                      .slice(0, 8)
                      .map((k) => Number(k.value))
                      .reverse(),
                  },
                ],
              }}
            />
          </Card>
        </Grid.Col>
      </Grid>

      <Card withBorder>
        <Group justify="space-between">
          <Group gap="sm">
            <Title order={5}>Modules</Title>
            <Text size="sm" c="dimmed">
              {s ? `${s.modules_active}/${s.modules_total} active` : '…'}
            </Text>
          </Group>
          <Button size="compact-xs" variant="subtle" onClick={() => setModulesOpen((o) => !o)}>
            {modulesOpen ? 'hide ▴' : 'expand ▾'}
          </Button>
        </Group>
        <Collapse expanded={modulesOpen}>
          {(i?.module_startup.length ?? 0) > 0 && (
            <>
              <Text size="xs" c="dimmed" tt="uppercase" mt="sm">
                startup cost — newest reading per module
              </Text>
              <EChart
                height={150}
                option={{
                  tooltip: {
                    ...TOOLTIP,
                    trigger: 'axis',
                    axisPointer: { type: 'shadow' },
                    valueFormatter: (v: number) => `${v}ms`,
                  },
                  grid: { left: 46, right: 14, top: 14, bottom: 44 },
                  xAxis: {
                    type: 'category',
                    data: (i?.module_startup ?? []).map((m) => m.name),
                    ...AXIS,
                    axisLabel: { ...AXIS.axisLabel, rotate: 30 },
                    splitLine: { show: false },
                  },
                  yAxis: {
                    type: 'value',
                    ...AXIS,
                    axisLabel: { ...AXIS.axisLabel, formatter: '{value}ms' },
                  },
                  series: [
                    {
                      type: 'bar',
                      barWidth: 18,
                      itemStyle: { borderRadius: 3, color: CHART_COLORS[4] },
                      data: (i?.module_startup ?? []).map((m) => Number(m.value)),
                    },
                  ],
                }}
              />
            </>
          )}
          <Table striped highlightOnHover mt="sm">
            <Table.Thead>
              <Table.Tr>
                <Table.Th>Module</Table.Th>
                <Table.Th>Kind</Table.Th>
                <Table.Th>Lifecycle</Table.Th>
                <Table.Th>Provisioned</Table.Th>
                <Table.Th>Version</Table.Th>
                <Table.Th>Module ID</Table.Th>
              </Table.Tr>
            </Table.Thead>
            <Table.Tbody>
              {(status.data?.modules ?? []).map((m) => (
                <Table.Tr key={m.module_id}>
                  <Table.Td>{m.name}</Table.Td>
                  <Table.Td>{m.kind}</Table.Td>
                  <Table.Td>
                    <Badge
                      color={
                        m.lifecycle === 'active'
                          ? 'green'
                          : m.lifecycle === 'disabled'
                            ? 'gray'
                            : 'yellow'
                      }
                    >
                      {m.lifecycle}
                    </Badge>
                  </Table.Td>
                  <Table.Td>{m.provisioned == null ? '—' : m.provisioned ? 'yes' : 'no'}</Table.Td>
                  <Table.Td>v{m.version}</Table.Td>
                  <Table.Td>
                    <Text size="xs" c="dimmed" ff="monospace">
                      {m.module_id}
                    </Text>
                  </Table.Td>
                </Table.Tr>
              ))}
            </Table.Tbody>
          </Table>
        </Collapse>
      </Card>
    </>
  )
}

function Stat({
  label,
  value,
  sub,
  tip,
}: {
  label: string
  value: string
  sub?: string
  tip?: string
}) {
  const card = (
    <Card withBorder padding="sm">
      <Text size="xs" c="dimmed" tt="uppercase">
        {label}
      </Text>
      <Title order={3}>{value}</Title>
      {sub ? (
        <Text size="xs" c="dimmed">
          {sub}
        </Text>
      ) : null}
    </Card>
  )
  return tip ? (
    <Tooltip label={tip} withArrow>
      {card}
    </Tooltip>
  ) : (
    card
  )
}
