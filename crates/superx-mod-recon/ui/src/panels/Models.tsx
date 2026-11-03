import { useState } from 'react'
import { Alert, Group, Stack, Table, Text, Title } from '@mantine/core'
import { useQuery } from '@tanstack/react-query'
import { fetchModels, type Measures, type ModelDay, type ModelTotal } from '../api'
import { Board, type Card } from '../Board'
import { Drill } from '../Drill'
import { AXIS, CHART_COLORS, EChart, TOOLTIP, type Clicked } from '../EChart'

// The models, measured over time (/api/results/models), on a board the
// operator arranges. Every chart of a day opens what is behind its bar: the
// commits, edits and sessions of that model that day.

const n = (v: number) => v.toLocaleString('en-US')
const per = (a: number, b: number, digits = 1) => (b > 0 ? (a / b).toLocaleString('en-US', { maximumFractionDigits: digits }) : '—')

/// The validated palette has eight hues: the seven models with most replies
/// keep their own, the rest are folded into "other", in every chart.
const SHOWN = CHART_COLORS.length - 1
const OTHER = 'other'

function names(totals: ModelTotal[]): string[] {
  const top = totals.slice(0, SHOWN).map((t) => t.model)
  return totals.length > SHOWN ? [...top, OTHER] : top
}

function colourOf(model: string, shown: string[]): string {
  const i = shown.indexOf(model)
  return CHART_COLORS[(i >= 0 ? i : shown.length - 1) % CHART_COLORS.length]
}

/// Every UTC day from the first to the last, so a day without work is a
/// gap on the axis rather than missing from it.
function everyDay(days: ModelDay[]): string[] {
  const seen = days.map((d) => d.day).sort()
  if (seen.length === 0) return []
  const out: string[] = []
  const end = new Date(`${seen[seen.length - 1]}T00:00:00Z`).getTime()
  for (let t = new Date(`${seen[0]}T00:00:00Z`).getTime(); t <= end; t += 86_400_000) out.push(new Date(t).toISOString().slice(0, 10))
  return out
}

const compact = (v: number) => (v >= 1e9 ? `${v / 1e9}B` : v >= 1e6 ? `${v / 1e6}M` : v >= 1000 ? `${v / 1000}k` : `${v}`)

/// One measure, a bar per day stacked by model, zoomable over time.
function stacked(days: ModelDay[], axis: string[], shown: string[], value: (d: ModelDay) => number, unit: (v: number) => string = n) {
  const at = new Map<string, number>()
  for (const d of days) {
    const model = shown.includes(d.model) ? d.model : OTHER
    const key = `${d.day}|${model}`
    at.set(key, (at.get(key) ?? 0) + value(d))
  }
  return {
    grid: { left: 8, right: 16, top: 40, bottom: 44, containLabel: true },
    legend: { top: 0, left: 0, type: 'scroll', icon: 'roundRect', itemWidth: 12, itemHeight: 8, data: shown },
    tooltip: { ...TOOLTIP, trigger: 'axis', axisPointer: { type: 'shadow' }, valueFormatter: (v: number) => unit(v) },
    dataZoom: [{ type: 'inside' }, { type: 'slider', height: 16, bottom: 8 }],
    xAxis: { ...AXIS, type: 'category', data: axis, splitLine: { show: false } },
    yAxis: { ...AXIS, type: 'value', axisLabel: { ...AXIS.axisLabel, formatter: compact } },
    series: shown.map((model) => ({
      name: model,
      type: 'bar',
      stack: 'models',
      barMaxWidth: 36,
      data: axis.map((day) => at.get(`${day}|${model}`) ?? 0),
      itemStyle: { color: colourOf(model, shown), borderColor: '#2A1235', borderWidth: 1 },
      emphasis: { focus: 'series' },
    })),
  }
}

/// One rate per model over all time, highest first.
function ranked(totals: ModelTotal[], shown: string[], value: (t: ModelTotal) => number | null, unit: (v: number) => string) {
  const rows = totals.map((t) => ({ t, v: value(t) })).filter((r): r is { t: ModelTotal; v: number } => r.v != null && Number.isFinite(r.v))
  rows.sort((a, b) => b.v - a.v)
  return {
    grid: { left: 8, right: 56, top: 8, bottom: 8, containLabel: true },
    tooltip: { ...TOOLTIP, trigger: 'axis', axisPointer: { type: 'shadow' }, valueFormatter: (v: number) => unit(v) },
    xAxis: { ...AXIS, type: 'value', axisLabel: { ...AXIS.axisLabel, formatter: compact } },
    yAxis: { ...AXIS, type: 'category', inverse: true, data: rows.map((r) => r.t.model), splitLine: { show: false } },
    series: [
      {
        type: 'bar',
        barMaxWidth: 22,
        data: rows.map((r) => ({ value: Math.round(r.v * 100) / 100, itemStyle: { color: colourOf(r.t.model, shown), borderRadius: [0, 4, 4, 0] } })),
        label: { show: true, position: 'right', color: '#CFC2DB', fontSize: 11, formatter: (p: { value: number }) => unit(p.value) },
      },
    ],
  }
}

function share(totals: ModelTotal[], shown: string[], value: (m: Measures) => number) {
  const by = new Map<string, number>()
  for (const t of totals) {
    const model = shown.includes(t.model) ? t.model : OTHER
    by.set(model, (by.get(model) ?? 0) + value(t))
  }
  return {
    tooltip: { ...TOOLTIP, trigger: 'item', valueFormatter: (v: number) => n(v) },
    legend: { type: 'scroll', orient: 'vertical', right: 0, top: 'middle', icon: 'roundRect', itemWidth: 12, itemHeight: 8 },
    series: [
      {
        type: 'pie',
        radius: ['45%', '75%'],
        center: ['38%', '50%'],
        itemStyle: { borderColor: '#2A1235', borderWidth: 2 },
        label: { show: false },
        data: shown.map((m) => ({ name: m, value: by.get(m) ?? 0, itemStyle: { color: colourOf(m, shown) } })),
      },
    ],
  }
}

function Scoreboard({ totals, shown }: { totals: ModelTotal[]; shown: string[] }) {
  return (
    <Table.ScrollContainer minWidth={1250}>
      <Table fz="sm" striped highlightOnHover verticalSpacing={6} stickyHeader>
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
            <Table.Th ta="right">per 1M tokens</Table.Th>
            <Table.Th ta="right">Commits</Table.Th>
            <Table.Th>Seen</Table.Th>
          </Table.Tr>
        </Table.Thead>
        <Table.Tbody>
          {totals.map((t) => (
            <Table.Tr key={t.model}>
              <Table.Td>
                <Group gap={8} wrap="nowrap">
                  <span style={{ width: 10, height: 10, borderRadius: 3, background: colourOf(t.model, shown), flex: 'none' }} />
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
              <Table.Td ta="right"><Text fz="xs">{n(t.input_tokens)} · {n(t.cache_read_tokens)} · {n(t.cache_write_tokens)}</Text></Table.Td>
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
              <Table.Td ta="right">{per(t.lines_added * 1e6, t.output_tokens, 0)}</Table.Td>
              <Table.Td ta="right">
                {n(t.commits)}
                {t.commits > 0 && <Text fz="xs" c="dimmed">+{n(t.commit_lines_added)} −{n(t.commit_lines_removed)}</Text>}
              </Table.Td>
              <Table.Td><Text fz="xs" ff="monospace">{t.first_day} → {t.last_day}</Text></Table.Td>
            </Table.Tr>
          ))}
        </Table.Tbody>
      </Table>
    </Table.ScrollContainer>
  )
}

export function Models() {
  const models = useQuery({ queryKey: ['models'], queryFn: fetchModels, refetchInterval: 300_000 })
  const [drill, setDrill] = useState<{ model: string; day: string } | null>(null)
  const totals = models.data?.totals ?? []
  const days = models.data?.days ?? []
  const shown = names(totals)
  const axis = everyDay(days)
  const open = (p: Clicked) => {
    const model = p.seriesName
    const day = p.dataIndex != null ? axis[p.dataIndex] : undefined
    if (model && model !== OTHER && day) setDrill({ model, day })
  }
  const daily = (i: string, title: string, note: string, value: (d: ModelDay) => number, unit?: (v: number) => string, w = 6): Card => ({
    i,
    title,
    note: `${note} · click a bar for what is behind it`,
    w,
    h: 11,
    render: (h) => <EChart option={stacked(days, axis, shown, value, unit)} height={h} onClick={open} />,
  })
  const rate = (i: string, title: string, note: string, value: (t: ModelTotal) => number | null, unit: (v: number) => string): Card => ({
    i,
    title,
    note,
    w: 6,
    h: 10,
    render: (h) => <EChart option={ranked(totals, shown, value, unit)} height={h} />,
  })
  const mins = (v: number) => `${n(v)} min`
  const cards: Card[] = [
    { i: 'scoreboard', title: 'Every model, over all time', note: 'sessions, replies, tokens, thinking, tool calls, lines its edits wrote, the commits its calls made', w: 12, h: 13, scroll: true, render: () => <Scoreboard totals={totals} shown={shown} /> },
    daily('replies', 'Replies per day', 'each reply once, however many lines it was written in', (d) => d.replies, n, 12),
    daily('edit-added', 'Lines its edits added, per day', 'from each edit’s patch', (d) => d.lines_added),
    daily('edit-removed', 'Lines its edits removed, per day', 'from each edit’s patch', (d) => d.lines_removed),
    daily('commits', 'Commits per day', 'commits a session printed, under the model whose call made them', (d) => d.commits),
    daily('commit-added', 'Lines committed per day', 'the lines those commits added', (d) => d.commit_lines_added),
    daily('tokens', 'Output tokens per day', 'each reply’s newest usage', (d) => d.output_tokens),
    daily('thinking', 'Thinking per day, in minutes', 'where the agent reports how long a thinking span took', (d) => Math.round(d.thinking_ms / 60_000), mins),
    daily('tools', 'Tool calls per day', 'every tool the model called', (d) => d.tool_calls),
    daily('cache', 'Context read from cache per day', 'cache-read input tokens', (d) => d.cache_read_tokens),
    { i: 'share', title: 'Share of replies', note: 'over all time', w: 6, h: 10, render: (h) => <EChart option={share(totals, shown, (m) => m.replies)} height={h} /> },
    { i: 'share-tokens', title: 'Share of output tokens', note: 'over all time', w: 6, h: 10, render: (h) => <EChart option={share(totals, shown, (m) => m.output_tokens)} height={h} /> },
    rate('per-reply', 'Output tokens per reply', 'how much each reply writes', (t) => (t.replies > 0 ? t.output_tokens / t.replies : null), (v) => n(Math.round(v))),
    rate('tools-per-reply', 'Tool calls per reply', 'how much each reply does', (t) => (t.replies > 0 ? t.tool_calls / t.replies : null), (v) => v.toFixed(2)),
    rate('lines-per-mtok', 'Lines its edits added per 1M output tokens', 'what the tokens bought', (t) => (t.output_tokens > 0 ? (t.lines_added * 1e6) / t.output_tokens : null), (v) => n(Math.round(v))),
    rate('commits-per-k', 'Commits per 1,000 replies', 'how often its work was committed', (t) => (t.replies > 0 ? (t.commits * 1000) / t.replies : null), (v) => v.toFixed(1)),
    rate('thinking-per-reply', 'Thinking seconds per reply', 'where reported', (t) => (t.replies > 0 && t.thinking_ms > 0 ? t.thinking_ms / 1000 / t.replies : null), (v) => `${v.toFixed(1)} s`),
    rate('removed-per-added', 'Lines removed per 100 added, by its edits', 'how much it rewrote', (t) => (t.lines_added > 0 ? (100 * t.lines_removed) / t.lines_added : null), (v) => v.toFixed(0)),
  ]
  return (
    <Stack gap="sm">
      <Group justify="space-between" align="baseline">
        <Title order={3}>Models</Title>
        <Text size="xs" c="dimmed">every reply recon read, by the model that answered · per UTC day · every 5 minutes</Text>
      </Group>
      {models.isError && <Alert color="red" title="The models did not load">{String(models.error)}</Alert>}
      {models.isLoading && <Text c="dimmed">Reading every reply, thinking span, tool call, edit and commit…</Text>}
      {totals.length > 0 && <Board name="results-models" cards={cards} />}
      <Drill model={drill?.model ?? null} day={drill?.day ?? null} onClose={() => setDrill(null)} />
    </Stack>
  )
}
