import { Alert, Group, Paper, SimpleGrid, Stack, Table, Text, Title } from '@mantine/core'
import { useQuery } from '@tanstack/react-query'
import { fetchModels, type ModelDay, type ModelTotal } from '../api'
import { AXIS, CHART_COLORS, EChart, TOOLTIP } from '../EChart'

// The models, measured over time (/api/results/models): what each answered,
// thought, called, edited and committed, day by day, and over all time.

const n = (v: number) => v.toLocaleString('en-US')
const per = (a: number, b: number, digits = 1) => (b > 0 ? (a / b).toLocaleString('en-US', { maximumFractionDigits: digits }) : '—')

/// A model's place among those shown: its colour, held across every chart.
/// The validated palette has eight hues: the seven with most replies keep
/// their own, the rest are folded into "other".
const SHOWN = CHART_COLORS.length - 1
const OTHER = 'other'

function names(totals: ModelTotal[]): string[] {
  const top = totals.slice(0, SHOWN).map((t) => t.model)
  return totals.length > SHOWN ? [...top, OTHER] : top
}

function as(model: string, shown: string[]) {
  return shown.includes(model) ? model : OTHER
}

/// One measure, a bar per day stacked by model.
function stacked(days: ModelDay[], shown: string[], value: (d: ModelDay) => number, unit: (v: number) => string = (v) => n(v)) {
  const allDays = [...new Set(days.map((d) => d.day))].sort()
  const at = new Map<string, number>()
  for (const d of days) {
    const key = `${d.day}|${as(d.model, shown)}`
    at.set(key, (at.get(key) ?? 0) + value(d))
  }
  return {
    grid: { left: 8, right: 12, top: 52, bottom: 4, containLabel: true },
    legend: { top: 0, left: 0, type: 'scroll', icon: 'roundRect', itemWidth: 12, itemHeight: 8, data: shown },
    tooltip: { ...TOOLTIP, trigger: 'axis', valueFormatter: (v: number) => unit(v) },
    xAxis: { ...AXIS, type: 'category', data: allDays.map((d) => d.slice(5)), splitLine: { show: false } },
    yAxis: { ...AXIS, type: 'value', axisLabel: { ...AXIS.axisLabel, formatter: (v: number) => (v >= 1e6 ? `${v / 1e6}M` : v >= 1000 ? `${v / 1000}k` : `${v}`) } },
    series: shown.map((model, i) => ({
      name: model,
      type: 'bar',
      stack: 'models',
      barMaxWidth: 28,
      data: allDays.map((day) => at.get(`${day}|${model}`) ?? 0),
      itemStyle: { color: CHART_COLORS[i % CHART_COLORS.length], borderColor: '#2A1235', borderWidth: 1 },
    })),
  }
}

function Chart({ title, note, option }: { title: string; note: string; option: Record<string, unknown> }) {
  return (
    <Paper withBorder p="md" radius="md">
      <Stack gap={4}>
        <Text fw={600}>{title}</Text>
        <Text size="xs" c="dimmed">{note}</Text>
        <EChart option={option} height={260} />
      </Stack>
    </Paper>
  )
}

function Scoreboard({ totals, shown }: { totals: ModelTotal[]; shown: string[] }) {
  return (
    <Paper withBorder radius="md">
      <Table.ScrollContainer minWidth={1200}>
        <Table fz="sm" striped highlightOnHover verticalSpacing="xs">
          <Table.Thead>
            <Table.Tr>
              <Table.Th>Model</Table.Th>
              <Table.Th ta="right">Sessions</Table.Th>
              <Table.Th ta="right">Replies</Table.Th>
              <Table.Th ta="right">Output tokens</Table.Th>
              <Table.Th ta="right">per reply</Table.Th>
              <Table.Th ta="right">Input · cache read · cache write</Table.Th>
              <Table.Th ta="right">Thinking</Table.Th>
              <Table.Th ta="right">s per reply</Table.Th>
              <Table.Th ta="right">Tool calls</Table.Th>
              <Table.Th ta="right">per reply</Table.Th>
              <Table.Th ta="right">Lines edited</Table.Th>
              <Table.Th ta="right">Commits</Table.Th>
              <Table.Th>Seen</Table.Th>
            </Table.Tr>
          </Table.Thead>
          <Table.Tbody>
            {totals.map((t) => {
              const i = shown.indexOf(t.model)
              const color = CHART_COLORS[(i >= 0 ? i : shown.length - 1) % CHART_COLORS.length]
              return (
                <Table.Tr key={t.model}>
                  <Table.Td>
                    <Group gap={8} wrap="nowrap">
                      <span style={{ width: 10, height: 10, borderRadius: 3, background: color, flex: 'none' }} />
                      <Text fz="sm" ff="monospace">{t.model}</Text>
                    </Group>
                  </Table.Td>
                  <Table.Td ta="right">{n(t.sessions)}</Table.Td>
                  <Table.Td ta="right">
                    {n(t.replies)}
                    {t.subagent_replies > 0 && <Text fz="xs" c="dimmed">{n(t.subagent_replies)} by subagents</Text>}
                  </Table.Td>
                  <Table.Td ta="right">{n(t.output_tokens)}</Table.Td>
                  <Table.Td ta="right">{per(t.output_tokens, t.replies, 0)}</Table.Td>
                  <Table.Td ta="right">
                    <Text fz="xs">{n(t.input_tokens)} · {n(t.cache_read_tokens)} · {n(t.cache_write_tokens)}</Text>
                  </Table.Td>
                  <Table.Td ta="right">
                    {n(t.thinking_spans)} spans
                    <Text fz="xs" c="dimmed">{(t.thinking_ms / 3_600_000).toLocaleString('en-US', { maximumFractionDigits: 1 })} h</Text>
                  </Table.Td>
                  <Table.Td ta="right">{per(t.thinking_ms / 1000, t.replies)}</Table.Td>
                  <Table.Td ta="right">{n(t.tool_calls)}</Table.Td>
                  <Table.Td ta="right">{per(t.tool_calls, t.replies, 2)}</Table.Td>
                  <Table.Td ta="right">
                    <Text fz="sm" c="teal.4">+{n(t.lines_added)}</Text>
                    <Text fz="xs" c="red.4">−{n(t.lines_removed)}</Text>
                  </Table.Td>
                  <Table.Td ta="right">
                    {n(t.commits)}
                    {t.commits > 0 && <Text fz="xs" c="dimmed">+{n(t.commit_lines_added)} −{n(t.commit_lines_removed)}</Text>}
                  </Table.Td>
                  <Table.Td><Text fz="xs" ff="monospace">{t.first_day} → {t.last_day}</Text></Table.Td>
                </Table.Tr>
              )
            })}
          </Table.Tbody>
        </Table>
      </Table.ScrollContainer>
    </Paper>
  )
}

export function Models() {
  const models = useQuery({ queryKey: ['models'], queryFn: fetchModels, refetchInterval: 300_000 })
  const totals = models.data?.totals ?? []
  const days = models.data?.days ?? []
  const shown = names(totals)
  return (
    <Stack gap="sm">
      <Group justify="space-between" align="baseline">
        <Title order={3}>Models</Title>
        <Text size="xs" c="dimmed">every reply recon read, by the model that answered · per UTC day · every 5 minutes</Text>
      </Group>
      {models.isError && <Alert color="red" title="The models did not load">{String(models.error)}</Alert>}
      {models.isLoading && <Text c="dimmed">Reading every reply, thinking span, tool call, edit and commit…</Text>}
      {totals.length > 0 && (
        <>
          <Scoreboard totals={totals} shown={shown} />
          <SimpleGrid cols={{ base: 1, md: 2 }}>
            <Chart title="Replies per day" note="Each reply once, however many lines it was written in." option={stacked(days, shown, (d) => d.replies)} />
            <Chart title="Output tokens per day" note="From each reply's newest usage." option={stacked(days, shown, (d) => d.output_tokens)} />
            <Chart
              title="Thinking per day, in minutes"
              note="The time each thinking span took, where the agent reports it."
              option={stacked(days, shown, (d) => Math.round(d.thinking_ms / 60_000), (v) => `${n(v)} min`)}
            />
            <Chart title="Tool calls per day" note="Every tool the model called." option={stacked(days, shown, (d) => d.tool_calls)} />
            <Chart title="Lines its edits wrote per day" note="Lines added by each edit's patch." option={stacked(days, shown, (d) => d.lines_added)} />
            <Chart title="Commits per day" note="Commits a session printed, tied to the model whose call made them." option={stacked(days, shown, (d) => d.commits)} />
          </SimpleGrid>
        </>
      )}
    </Stack>
  )
}
