import { Alert, Group, Paper, SimpleGrid, Stack, Table, Text, Title } from '@mantine/core'
import { useQuery } from '@tanstack/react-query'
import { fetchStats, type Day } from '../api'
import { AXIS, CANCEL, CHART_COLORS, EChart, FAIL, OK, TOOLTIP, UNKNOWN } from '../EChart'

// The crawler's stats, day by day in the operator's own time, from every pass recon recorded:
// /api/stats reads its own pass and pass_source rows.

const OUTCOMES: [string, string][] = [
  ['complete', OK],
  ['partial', CANCEL],
  ['stopped', UNKNOWN],
  ['failed', FAIL],
]

const fmt = (n: number) => n.toLocaleString('en-US')

function axes(days: Day[]) {
  return {
    grid: { left: 8, right: 12, top: 32, bottom: 4, containLabel: true },
    tooltip: { ...TOOLTIP, trigger: 'axis', valueFormatter: (v: number) => fmt(v) },
    xAxis: { ...AXIS, type: 'category', data: days.map((d) => d.day.slice(5)), splitLine: { show: false } },
    yAxis: { ...AXIS, type: 'value', axisLabel: { ...AXIS.axisLabel, formatter: (v: number) => (v >= 1000 ? `${v / 1000}k` : `${v}`) } },
  }
}

function legend(names: string[]) {
  return { top: 0, right: 0, icon: 'roundRect', itemWidth: 12, itemHeight: 8, data: names }
}

function passes(days: Day[]) {
  const seen = new Set(days.flatMap((d) => Object.keys(d.passes)))
  const order = [...OUTCOMES.map(([o]) => o).filter((o) => seen.has(o)), ...[...seen].filter((o) => !OUTCOMES.some(([k]) => k === o))]
  return {
    ...axes(days),
    legend: legend(order),
    series: order.map((o, i) => ({
      name: o,
      type: 'bar',
      stack: 'outcome',
      barMaxWidth: 34,
      data: days.map((d) => d.passes[o] ?? 0),
      itemStyle: { color: OUTCOMES.find(([k]) => k === o)?.[1] ?? CHART_COLORS[i % CHART_COLORS.length] },
    })),
  }
}

function single(days: Day[], name: string, value: (d: Day) => number) {
  return {
    ...axes(days),
    series: [{ name, type: 'bar', barMaxWidth: 34, data: days.map(value), itemStyle: { color: CHART_COLORS[0], borderRadius: [4, 4, 0, 0] } }],
  }
}

function calls(days: Day[], total: (d: Day) => number, failed: (d: Day) => number) {
  return {
    ...axes(days),
    legend: legend(['answered', 'failed']),
    series: [
      { name: 'answered', type: 'bar', stack: 'calls', barMaxWidth: 34, data: days.map((d) => total(d) - failed(d)), itemStyle: { color: CHART_COLORS[0] } },
      { name: 'failed', type: 'bar', stack: 'calls', barMaxWidth: 34, data: days.map(failed), itemStyle: { color: FAIL } },
    ],
  }
}

function Chart({ title, note, option }: { title: string; note: string; option: Record<string, unknown> }) {
  return (
    <Paper withBorder p="md" radius="md">
      <Stack gap={4}>
        <Text fw={600}>{title}</Text>
        <Text size="xs" c="dimmed">{note}</Text>
        <EChart option={option} height={230} />
      </Stack>
    </Paper>
  )
}

export function Stats() {
  const stats = useQuery({ queryKey: ['stats'], queryFn: fetchStats, refetchInterval: 60_000 })
  const days = stats.data?.days ?? []
  return (
    <Stack gap="sm">
      <Group justify="space-between" align="baseline">
        <Title order={3}>Crawler stats</Title>
        <Text size="xs" c="dimmed">per day, your time, from every pass recon recorded · refreshed every minute</Text>
      </Group>
      {stats.isError && <Alert color="red" title="The stats did not load">{String(stats.error)}</Alert>}
      {stats.isSuccess && days.length === 0 && <Text c="dimmed">No pass recorded yet.</Text>}
      {days.length > 0 && (
        <>
          <SimpleGrid cols={{ base: 1, md: 2 }}>
            <Chart title="Passes per day, by outcome" note="Partial: a call failed or a count read short." option={passes(days)} />
            <Chart title="Rows written per day" note="Every row the passes wrote." option={single(days, 'rows written', (d) => d.rows)} />
            <Chart title="Git calls per day" note="The repositories' crawls, each call read-only." option={calls(days, (d) => d.git_calls, (d) => d.git_failed)} />
            <Chart title="Host calls per day" note="GitHub and Bitbucket together, GET only." option={calls(days, (d) => d.host_calls, (d) => d.host_failed)} />
          </SimpleGrid>
          <Paper withBorder radius="md">
            <Table.ScrollContainer minWidth={720}>
              <Table striped highlightOnHover fz="sm">
                <Table.Thead>
                  <Table.Tr>
                    <Table.Th>Day</Table.Th>
                    <Table.Th ta="right">Passes</Table.Th>
                    <Table.Th ta="right">Rows written</Table.Th>
                    <Table.Th ta="right">Lines read</Table.Th>
                    <Table.Th ta="right">Git calls</Table.Th>
                    <Table.Th ta="right">failed</Table.Th>
                    <Table.Th ta="right">Host calls</Table.Th>
                    <Table.Th ta="right">failed</Table.Th>
                  </Table.Tr>
                </Table.Thead>
                <Table.Tbody>
                  {[...days].reverse().map((d) => (
                    <Table.Tr key={d.day}>
                      <Table.Td ff="monospace">{d.day}</Table.Td>
                      <Table.Td ta="right">{fmt(Object.values(d.passes).reduce((a, b) => a + b, 0))}</Table.Td>
                      <Table.Td ta="right">{fmt(d.rows)}</Table.Td>
                      <Table.Td ta="right">{fmt(d.lines)}</Table.Td>
                      <Table.Td ta="right">{fmt(d.git_calls)}</Table.Td>
                      <Table.Td ta="right">{fmt(d.git_failed)}</Table.Td>
                      <Table.Td ta="right">{fmt(d.host_calls)}</Table.Td>
                      <Table.Td ta="right">{fmt(d.host_failed)}</Table.Td>
                    </Table.Tr>
                  ))}
                </Table.Tbody>
              </Table>
            </Table.ScrollContainer>
          </Paper>
        </>
      )}
    </Stack>
  )
}
