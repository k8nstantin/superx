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
  if (v >= 1e6) return `${(v / 1e6).toFixed(1)}M`
  if (v >= 1e3) return `${(v / 1e3).toFixed(1)}k`
  return String(v)
}

function fmtAge(secs: number | bigint | null | undefined): string {
  if (secs == null) return '—'
  const v = Number(secs)
  if (v < 60) return `${v}s`
  if (v < 3600) return `${Math.floor(v / 60)}m`
  if (v < 86400) return `${Math.floor(v / 3600)}h`
  return `${Math.floor(v / 86400)}d`
}

export default function StatusPage() {
  const status = useQuery({ queryKey: ['status'], queryFn: fetchStatus, refetchInterval: 10000 })
  const stats = useQuery({ queryKey: ['stats'], queryFn: fetchStats, refetchInterval: 15000 })
  // All-history aggregates: heavier, and they move slowly.
  const insights = useQuery({
    queryKey: ['insights'],
    queryFn: fetchInsights,
    refetchInterval: 60000,
  })
  const [modulesOpen, setModulesOpen] = useState(false)

  const s = stats.data
  const i = insights.data
  const windowNote = s ? `newest ${s.window_messages} captured messages` : ''

  const tok = i?.tokens
  const promptTotal = tok ? Number(tok.input) + Number(tok.cache_read) + Number(tok.cache_write) : 0
  const cacheHit = tok && promptTotal > 0 ? Math.round((Number(tok.cache_read) * 100) / promptTotal) : null
  const outcomes = s?.tool_outcomes ?? []
  const scored = outcomes.reduce((n, t) => n + Number(t.ok) + Number(t.failed) + Number(t.cancelled), 0)
  const failed = outcomes.reduce((n, t) => n + Number(t.failed), 0)
  const failRate = scored > 0 ? Math.round((failed * 1000) / scored) / 10 : null

  return (
    <>
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
          <Title order={5} mb="xs">
            The work calendar — every event the OS ever captured, by day
          </Title>
          <EChart
            height={190}
            option={{
              tooltip: {
                ...TOOLTIP,
                formatter: (p: { value: [string, number] }) =>
                  `${p.value[0]}<br/>${fmtCompact(p.value[1])} events`,
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
                    )} events`,
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
