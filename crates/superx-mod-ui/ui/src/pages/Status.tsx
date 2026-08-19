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
import { fetchStats, fetchStatus } from '../api'
import { AXIS, CHART_COLORS, EChart, GRID_LINE, INK_MUTED, TOOLTIP } from '../EChart'
import { sessionColor } from '../Feed'

// The Status page (issue #228): the OS's captured numbers, front and
// center — what the agents did (tools, lines of code), who did it
// (per-session leaderboard in each session's feed color), and the
// pulse (events/min). Modules collapse out of the way.

function fmtCompact(n: number | bigint | null | undefined): string {
  if (n == null) return '—'
  const v = Number(n)
  if (v >= 1e6) return `${(v / 1e6).toFixed(1)}M`
  if (v >= 1e3) return `${(v / 1e3).toFixed(1)}k`
  return String(v)
}

export default function StatusPage() {
  const status = useQuery({ queryKey: ['status'], queryFn: fetchStatus, refetchInterval: 10000 })
  const stats = useQuery({ queryKey: ['stats'], queryFn: fetchStats, refetchInterval: 15000 })
  const [modulesOpen, setModulesOpen] = useState(false)

  const s = stats.data
  const windowNote = s ? `newest ${s.window_messages} captured messages` : ''

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
