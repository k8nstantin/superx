import { SimpleGrid, Text } from '@mantine/core'
import { Stat } from './parts'
import type { StatsSummary } from '../../generated/StatsSummary'
import { AXIS, CHART_COLORS, EChart, GRID_LINE, INK, INK_MUTED, TOOLTIP } from '../../EChart'
import { FAIL, OK, Panel, fmtCompact, n } from './parts'

// The real cost (#407) — the argument in pictures.
//
// A price per token says what a model costs to RUN. It says nothing
// about what it costs to KEEP, and the difference is the whole story:
// work that is rewritten was paid for twice, and nobody's price list
// carries that column.
//
// Forms chosen by the job, not by taste:
//  · before → after per model, one unit          → dumbbell
//  · part-to-whole of what each model landed     → stacked bar
//  · two measures against each other             → scatter, capped at three
//  · change over time                            → line
// No chart here carries two y-scales; where two measures share a plot
// they share a unit, and where they do not they get separate plots.

type Row = {
  model: string
  tokens: number
  landed: number
  alive: number
  perLanded: number | null
  perKept: number | null
  survived: number | null
}

export function RealCost({ s, range }: { s: StatsSummary | undefined; range: string | null }) {
  const tokensOf = (model: string) =>
    (s?.model_quality ?? []).filter((p) => p.model === model).reduce((a, p) => a + n(p.out_tokens), 0)

  const rows: Row[] = (s?.model_survival ?? [])
    .map((m) => {
      const tokens = tokensOf(m.model)
      const landed = n(m.landed)
      const alive = n(m.alive)
      return {
        model: m.model,
        tokens,
        landed,
        alive,
        perLanded: landed > 0 && tokens > 0 ? Math.round(tokens / landed) : null,
        perKept: alive > 0 && tokens > 0 ? Math.round(tokens / alive) : null,
        survived: landed > 0 ? Math.round((alive * 100) / landed) : null,
      }
    })
    .filter((r) => r.landed > 0)
    .sort((a, b) => (a.perKept ?? 0) - (b.perKept ?? 0))

  const named = rows.map((r) => r.model)

  // What the prompt weighed, per model (#407). One unit, one axis.
  const ctx = (s?.model_quality ?? []).reduce<Record<string, { sum: number; n: number; max: number; msgs: number }>>(
    (acc, p) => {
      const e = (acc[p.model] ??= { sum: 0, n: 0, max: 0, msgs: 0 })
      e.sum += n(p.context_sum)
      e.n += n(p.context_n)
      e.max = Math.max(e.max, n(p.context_max))
      e.msgs += n(p.messages)
      return acc
    },
    {},
  )
  const ctxModels = Object.keys(ctx).filter((m) => ctx[m].n > 0).sort((a, b) => ctx[b].sum / ctx[b].n - ctx[a].sum / ctx[a].n)
  const contextChart = {
    grid: { left: 150, right: 60, top: 26, bottom: 26 },
    tooltip: { ...TOOLTIP, trigger: 'axis' },
    legend: { data: ['typical prompt', 'largest prompt'], textStyle: { color: INK_MUTED }, right: 0, top: -2 },
    xAxis: {
      type: 'value',
      name: 'tokens carried in the prompt',
      nameLocation: 'middle',
      nameGap: 28,
      nameTextStyle: { color: INK_MUTED },
      axisLabel: { color: AXIS.axisLabel.color, formatter: (v: number) => fmtCompact(v) },
      splitLine: { lineStyle: { color: GRID_LINE } },
    },
    yAxis: { type: 'category', data: ctxModels, axisLabel: { color: INK }, axisLine: { lineStyle: { color: GRID_LINE } } },
    series: [
      {
        name: 'typical prompt',
        type: 'bar',
        barWidth: 14,
        itemStyle: { color: CHART_COLORS[0], borderRadius: [0, 4, 4, 0] },
        label: { show: true, position: 'right', color: INK, fontSize: 11, formatter: (p: { value: number }) => fmtCompact(p.value) },
        data: ctxModels.map((m) => Math.round(ctx[m].sum / Math.max(1, ctx[m].n))),
      },
      {
        name: 'largest prompt',
        type: 'bar',
        barWidth: 14,
        itemStyle: { color: CHART_COLORS[2], borderRadius: [0, 4, 4, 0] },
        data: ctxModels.map((m) => ctx[m].max),
      },
    ],
  }

  // ── the gap between what a line cost to land and to keep ──────────
  // Before → after per item, one unit on one axis: a dumbbell. The
  // length of the connector IS the rework tax.
  const dumbbell = {
    grid: { left: 150, right: 60, top: 26, bottom: 26 },
    tooltip: {
      ...TOOLTIP,
      trigger: 'item',
      formatter: (p: { dataIndex: number }) => {
        const r = rows[p.dataIndex]
        if (!r) return ''
        return `${r.model}<br/>to land a line: ${fmtCompact(r.perLanded ?? 0)} tokens<br/>to keep one: ${fmtCompact(
          r.perKept ?? 0,
        )} tokens`
      },
    },
    legend: { data: ['cost to land', 'cost to keep'], textStyle: { color: INK_MUTED }, right: 0, top: -2 },
    xAxis: {
      type: 'value',
      name: 'output tokens per line',
      nameLocation: 'middle',
      nameGap: 28,
      nameTextStyle: { color: INK_MUTED },
      axisLabel: { color: AXIS.axisLabel.color, formatter: (v: number) => fmtCompact(v) },
      splitLine: { lineStyle: { color: GRID_LINE } },
    },
    yAxis: {
      type: 'category',
      data: named,
      axisLabel: { color: INK },
      axisLine: { lineStyle: { color: GRID_LINE } },
    },
    series: [
      {
        // The connector: drawn first so the two ends sit on top of it.
        name: 'rework tax',
        type: 'custom',
        silent: true,
        renderItem: (params: { dataIndex: number }, api: {
          value: (i: number) => number
          coord: (p: [number, number]) => [number, number]
          style: (o: Record<string, unknown>) => Record<string, unknown>
        }) => {
          const r = rows[params.dataIndex]
          if (!r || r.perLanded == null || r.perKept == null) return null
          const a = api.coord([r.perLanded, params.dataIndex])
          const b = api.coord([r.perKept, params.dataIndex])
          return {
            type: 'line',
            shape: { x1: a[0], y1: a[1], x2: b[0], y2: b[1] },
            style: api.style({ stroke: GRID_LINE, lineWidth: 2 }),
          }
        },
        data: rows.map((r) => [r.perLanded ?? 0, r.model]),
        encode: { x: 0, y: 1 },
      },
      {
        name: 'cost to land',
        type: 'scatter',
        symbolSize: 12,
        itemStyle: { color: CHART_COLORS[2], borderColor: '#2A1235', borderWidth: 2 },
        data: rows.map((r) => [r.perLanded ?? 0, r.model]),
      },
      {
        name: 'cost to keep',
        type: 'scatter',
        symbolSize: 14,
        itemStyle: { color: CHART_COLORS[0], borderColor: '#2A1235', borderWidth: 2 },
        label: {
          show: true,
          position: 'right',
          distance: 8,
          color: INK,
          fontSize: 11,
          formatter: (p: { value: [number, string] }) => fmtCompact(p.value[0]),
        },
        data: rows.map((r) => [r.perKept ?? 0, r.model]),
      },
    ],
  }

  // ── of what each model landed, how much is still there ────────────
  // Part-to-whole: a stacked bar, two segments, direct-labelled, with a
  // 2px gap so the segments read as separate marks.
  const survival = {
    grid: { left: 150, right: 24, top: 26, bottom: 26 },
    tooltip: { ...TOOLTIP, trigger: 'axis' },
    legend: { data: ['still there', 'rewritten or deleted'], textStyle: { color: INK_MUTED }, right: 0, top: -2 },
    xAxis: {
      type: 'value',
      axisLabel: { color: AXIS.axisLabel.color, formatter: (v: number) => fmtCompact(v) },
      splitLine: { lineStyle: { color: GRID_LINE } },
    },
    yAxis: { type: 'category', data: named, axisLabel: { color: INK }, axisLine: { lineStyle: { color: GRID_LINE } } },
    series: [
      {
        name: 'still there',
        type: 'bar',
        stack: 'landed',
        barWidth: 18,
        itemStyle: { color: OK, borderColor: '#2A1235', borderWidth: 1, borderRadius: [4, 0, 0, 4] },
        label: {
          show: true,
          color: INK,
          fontSize: 11,
          formatter: (p: { dataIndex: number }) => {
            const r = rows[p.dataIndex]
            return r?.survived == null ? '' : `${r.survived}%`
          },
        },
        data: rows.map((r) => r.alive),
      },
      {
        name: 'rewritten or deleted',
        type: 'bar',
        stack: 'landed',
        barWidth: 18,
        itemStyle: { color: FAIL, borderColor: '#2A1235', borderWidth: 1, borderRadius: [0, 4, 4, 0] },
        data: rows.map((r) => Math.max(0, r.landed - r.alive)),
      },
    ],
  }

  // ── cheap against good, on one plot ───────────────────────────────
  // Two different measures, so they take the two axes rather than two
  // scales on one. Three points at most, each directly labelled.
  const quadrant = {
    grid: { left: 58, right: 40, top: 26, bottom: 44 },
    tooltip: {
      ...TOOLTIP,
      trigger: 'item',
      formatter: (p: { dataIndex: number }) => {
        const r = rows[p.dataIndex]
        return r
          ? `${r.model}<br/>${fmtCompact(r.perKept ?? 0)} tokens per line kept<br/>${r.survived}% survived<br/>${fmtCompact(
              r.landed,
            )} lines landed`
          : ''
      },
    },
    xAxis: {
      type: 'value',
      name: 'tokens per line KEPT  →  more expensive',
      nameLocation: 'middle',
      nameGap: 30,
      nameTextStyle: { color: INK_MUTED },
      axisLabel: { color: AXIS.axisLabel.color, formatter: (v: number) => fmtCompact(v) },
      splitLine: { lineStyle: { color: GRID_LINE } },
    },
    yAxis: {
      type: 'value',
      name: 'survived %',
      nameTextStyle: { color: INK_MUTED },
      max: 100,
      axisLabel: { color: AXIS.axisLabel.color, formatter: '{value}%' },
      splitLine: { lineStyle: { color: GRID_LINE } },
    },
    series: [
      {
        type: 'scatter',
        symbolSize: (v: number[]) => Math.max(14, Math.min(46, Math.sqrt(v[2] ?? 1) / 6)),
        itemStyle: { color: CHART_COLORS[0], borderColor: '#2A1235', borderWidth: 2, opacity: 0.9 },
        label: {
          show: true,
          position: 'top',
          distance: 10,
          color: INK,
          fontSize: 11,
          formatter: (p: { dataIndex: number }) => rows[p.dataIndex]?.model ?? '',
        },
        data: rows.map((r) => [r.perKept ?? 0, r.survived ?? 0, r.landed]),
      },
    ],
  }

  if (rows.length === 0) {
    return (
      <Panel title="The real cost" scope="range" range={range}>
        <Text size="xs" c="dimmed">
          no commit in this range could be credited to a model, so nothing can be costed yet.
        </Text>
      </Panel>
    )
  }

  return (
    <>
      <Panel
        title="What a line costs to land, and what it costs to keep"
        scope="range"
        range={range}
        note="the distance between the two dots is the rework tax — work paid for twice"
        mb="md"
      >
        <EChart height={68 + rows.length * 46} option={dumbbell} />
        <Text size="xs" c="dimmed" mt="xs">
          A price per token says what a model costs to run. This says what it costs to keep, and only the second one
          is the bill you actually pay.
        </Text>
      </Panel>

      <SimpleGrid cols={{ base: 1, lg: 2 }} spacing="md" mb="md">
        <Panel
          title="Of what each model landed, how much is still there"
          scope="range"
          range={range}
          note="lines on the main line, kept against rewritten"
          h="100%"
        >
          <EChart height={68 + rows.length * 46} option={survival} />
        </Panel>
        <Panel
          title="Cheap against good"
          scope="range"
          range={range}
          note="each bubble is a model, sized by the lines it landed · bottom right is the worst place to be"
          h="100%"
        >
          <EChart height={68 + rows.length * 46} option={quadrant} />
        </Panel>
      </SimpleGrid>

      <Panel
        title="How much context it carried to do the work"
        scope="range"
        range={range}
        note="a model that fills its window for a small task pays for the window on every turn after"
        mb="md"
      >
        {ctxModels.length === 0 ? (
          <Text size="xs" c="dimmed">
            no message in this range reported what its prompt weighed.
          </Text>
        ) : (
          <>
            <EChart height={68 + ctxModels.length * 46} option={contextChart} />
            <SimpleGrid cols={{ base: 2, md: 4 }} spacing="xs" mt="sm">
              {ctxModels.slice(0, 4).map((m) => (
                <Stat
                  key={m}
                  label={m}
                  value={`${fmtCompact(Math.round(ctx[m].sum / Math.max(1, ctx[m].n)))} typical`}
                  sub={`peak ${fmtCompact(ctx[m].max)} · ${fmtCompact(ctx[m].msgs)} turns`}
                  tip="prompt tokens carried per turn: what was sent fresh, plus what the vendor served from its cache. Every turn pays it."
                />
              ))}
            </SimpleGrid>
          </>
        )}
      </Panel>
    </>
  )
}
