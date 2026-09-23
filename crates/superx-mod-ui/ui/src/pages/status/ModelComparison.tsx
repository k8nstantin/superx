import { Box, SimpleGrid, Table, Text, Title } from '@mantine/core'
import { AXIS, EChart, GRID_LINE, INK, INK_MUTED, TOOLTIP } from '../../EChart'
import { Panel, fmtCompact, n } from './parts'
import type { CompareSummary } from '../../generated/CompareSummary'
import type { Deviation } from '../../generated/Deviation'

/// The same row with every i64 as a plain number. ts-rs maps i64 to
/// bigint, which cannot be added to a number or fed to a chart, so the
/// conversion happens once here rather than at forty call sites.
type Row = { [K in keyof Deviation]: Deviation[K] extends bigint ? number : Deviation[K] }

function toRow(d: Deviation): Row {
  const out = {} as Record<string, unknown>
  for (const [k, val] of Object.entries(d)) {
    out[k] = typeof val === 'bigint' ? Number(val) : val
  }
  return out as Row
}

// Model comparison (#406).
//
// One question: which model should the work go to, and what does
// choosing the other one cost. Everything here is read from git and
// from uncapped per-model runs, in ONE walk producing ONE set of
// numbers — two walks computing "survived %" separately is how two
// sections came to disagree with each other on screen.
//
// The charts are small multiples on purpose. Twenty of them, each one
// measure, each labelled with which direction is good, so the pattern
// is read across the grid rather than argued in prose.

const KEPT = '#199e70'
const THROWN = '#e66767'
const IDENT = ['#B833E8', '#c98500', '#3987e5']
const MIN_ADDED = 5000

/// The model FAMILY. Point releases of one model are the same choice
/// from the operator's side — you reach for Fable or you reach for
/// Opus — and splitting them dilutes the comparison and lets a thin
/// recent point release distort a rate. The full table keeps the exact
/// versions; the charts compare families.
function family(model: string): string {
  const m = model.replace(/^claude-/, '')
  const hit = ['fable', 'opus', 'sonnet', 'haiku'].find((f) => m.startsWith(f))
  return hit ?? m
}

/// Fold versions into families. Every rate is recomputed from the
/// SUMMED numerator and denominator — averaging two rates would weigh
/// a 900-line stint the same as a 40,000-line one.
function byFamily(rows: Row[]): Row[] {
  const acc = new Map<string, Row>()
  for (const r of rows) {
    const k = family(r.model)
    const p = acc.get(k)
    if (!p) {
      acc.set(k, { ...r, model: k })
      continue
    }
    p.commits = n(p.commits) + n(r.commits)
    p.added = n(p.added) + n(r.added)
    p.alive = n(p.alive) + n(r.alive)
    p.removed = n(p.removed) + n(r.removed)
    p.thrown = n(p.thrown) + n(r.thrown)
    p.out_tokens = n(p.out_tokens) + n(r.out_tokens)
    p.messages = n(p.messages) + n(r.messages)
    p.runs = n(p.runs) + n(r.runs)
    p.minutes = n(p.minutes) + n(r.minutes)
    p.minutes_thrown = n(p.minutes_thrown) + n(r.minutes_thrown)
    p.tokens_thrown = n(p.tokens_thrown) + n(r.tokens_thrown)
    p.rework_commits = n(p.rework_commits) + n(r.rework_commits)
    p.thrash_files = n(p.thrash_files) + n(r.thrash_files)
    p.operator_turns = n(p.operator_turns) + n(r.operator_turns)
    p.corrections = n(p.corrections) + n(r.corrections)
    p.context_peak = Math.max(n(p.context_peak), n(r.context_peak))
    // Context is per turn, so it weights by messages.
    p.context_avg =
      n(p.messages) > 0
        ? Math.round(
            (n(p.context_avg) * (n(p.messages) - n(r.messages)) + n(r.context_avg) * n(r.messages)) /
              n(p.messages),
          )
        : n(p.context_avg)
    p.median_age_days = Math.max(n(p.median_age_days), n(r.median_age_days))
  }
  // Recompute every rate from the totals now that they are summed.
  return [...acc.values()].map((p) => ({
    ...p,
    survived_pct: n(p.added) > 0 ? Math.round((100 * n(p.alive)) / n(p.added)) : 0,
    removed_per_100_added: n(p.added) > 0 ? Math.round((100 * n(p.removed)) / n(p.added)) : 0,
    rework_pct: n(p.commits) > 0 ? Math.round((100 * n(p.rework_commits)) / n(p.commits)) : 0,
    thrash_per_100_commits:
      n(p.commits) > 0 ? Math.round((100 * n(p.thrash_files)) / n(p.commits)) : 0,
    corrections_per_100:
      n(p.operator_turns) > 0 ? Math.round((100 * n(p.corrections)) / n(p.operator_turns)) : 0,
    tokens_per_line_landed: n(p.added) > 0 ? Math.round(n(p.out_tokens) / n(p.added)) : 0,
    tokens_per_line_kept: n(p.alive) > 0 ? Math.round(n(p.out_tokens) / n(p.alive)) : 0,
    alive_per_mtok:
      n(p.out_tokens) > 0 ? Math.round((n(p.alive) * 1_000_000) / n(p.out_tokens)) : 0,
    alive_per_hour: n(p.minutes) > 0 ? Math.round((n(p.alive) * 60) / n(p.minutes)) : 0,
  }))
}

type Metric = {
  title: string
  note: string
  pick: (d: Row) => number
  good: 'high' | 'low'
  fmt?: (v: number) => string
}

const per = (a: number, b: number, mult = 1) => (b > 0 ? Math.round((a * mult) / b) : 0)

/// The twenty measures, in reading order: what you GET, what you SPEND
/// per unit, what was WASTED, how the work WENT, and the totals that
/// put the rates in context.
function metrics(): Metric[] {
  // Only measures whose job is a plain RANKING stay as bars. Anything
  // that is a proportion, a pair of values, or two measures against
  // each other gets the form that fits it, below.
  return [
    { title: 'Landed lines per 1M tokens', note: 'before survival is counted', good: 'high', pick: (d) => per(n(d.added), n(d.out_tokens), 1_000_000) },
    { title: 'Lines removed per 100 added', note: 'churn while working', good: 'low', pick: (d) => n(d.removed_per_100_added) },
    { title: 'Files returned to 3+ times', note: 'per 100 commits', good: 'low', pick: (d) => n(d.thrash_per_100_commits) },
    { title: 'Times you put it back on course', note: 'per 100 of your turns', good: 'low', pick: (d) => n(d.corrections_per_100) },
    { title: 'Lines still in the tree', note: 'the actual output', good: 'high', fmt: fmtCompact, pick: (d) => n(d.alive) },
    { title: 'Age of the work', note: 'days — the confounder, check it', good: 'high', fmt: (v) => `${v}d`, pick: (d) => n(d.median_age_days) },
    { title: 'Written but never landed', note: 'share of everything it wrote', good: 'low', fmt: (v) => `${v}%`, pick: (d) => n(d.abandoned_pct) },
    { title: 'Commits on abandoned branches', note: 'deleted or never merged', good: 'low', pick: (d) => n(d.abandoned_commits) },
  ]
}

/// One small multiple: a bar per model, sorted so the best is on top
/// once ECharts flips the category axis, and tinted by whether being
/// high here is good or bad.
function mini(rows: Row[], m: Metric) {
  const sorted = [...rows].sort((a, b) => (m.good === 'high' ? m.pick(a) - m.pick(b) : m.pick(b) - m.pick(a)))
  const best = m.good === 'high' ? Math.max(...rows.map(m.pick)) : Math.min(...rows.map(m.pick))
  return {
    tooltip: { ...TOOLTIP, trigger: 'axis', axisPointer: { type: 'shadow' } },
    grid: { left: 108, right: 62, top: 6, bottom: 6, containLabel: false },
    xAxis: { ...AXIS, type: 'value', axisLabel: { show: false }, splitLine: { show: false }, axisLine: { show: false } },
    yAxis: {
      type: 'category',
      data: sorted.map((r) => r.model.replace('claude-', '')),
      axisLabel: { color: INK_MUTED, fontSize: 10 },
      axisLine: { lineStyle: { color: GRID_LINE } },
      axisTick: { show: false },
    },
    series: [
      {
        // A lollipop, not a bar. One value per family needs a stem and a
        // dot: the bar's area encodes nothing here, and the ink it costs
        // is ink not spent on the number itself.
        type: 'bar',
        barWidth: 2,
        itemStyle: { color: GRID_LINE },
        data: sorted.map(m.pick),
        z: 1,
      },
      {
        type: 'scatter',
        symbolSize: 11,
        itemStyle: {
          color: (p: { value: number }) => (p.value === best ? KEPT : THROWN),
          borderColor: '#150420',
          borderWidth: 2,
        },
        label: {
          show: true,
          position: 'right',
          color: INK,
          fontSize: 11,
          formatter: (p: { value: number }) => (m.fmt ? m.fmt(p.value) : `${p.value}`),
        },
        data: sorted.map(m.pick),
        z: 2,
      },
    ],
  }
}


/// A dumbbell: two values of the SAME unit per family, joined by the
/// line whose length is the gap. The right form when the story is the
/// DISTANCE between two numbers — typical against peak, the price
/// before rework against the price after — where two separate bars
/// make the reader do the subtraction.
function dumbbell(
  rows: Row[],
  a: (d: Row) => number,
  b: (d: Row) => number,
  aName: string,
  bName: string,
  axis: string,
  fmt: (v: number) => string,
) {
  const names = rows.map((r) => r.model)
  return {
    tooltip: { ...TOOLTIP, trigger: 'axis', axisPointer: { type: 'shadow' } },
    legend: { data: [aName, bName], textStyle: { color: INK_MUTED }, top: 0, right: 0 },
    grid: { left: 96, right: 88, top: 30, bottom: 40 },
    xAxis: {
      ...AXIS,
      type: 'value',
      name: axis,
      nameLocation: 'middle',
      nameGap: 22,
      nameTextStyle: { color: INK_MUTED },
      axisLabel: { color: INK_MUTED, formatter: (x: number) => fmt(x) },
      splitLine: { lineStyle: { color: GRID_LINE } },
    },
    yAxis: {
      type: 'category',
      data: names,
      axisLabel: { color: INK, fontSize: 11 },
      axisLine: { lineStyle: { color: GRID_LINE } },
    },
    series: [
      {
        name: 'gap',
        type: 'custom',
        silent: true,
        renderItem: (
          _p: unknown,
          api: { value: (i: number) => number; coord: (v: number[]) => number[] },
        ) => {
          const lo = api.coord([api.value(1), api.value(0)])
          const hi = api.coord([api.value(2), api.value(0)])
          return {
            type: 'line',
            shape: { x1: lo[0], y1: lo[1], x2: hi[0], y2: hi[1] },
            style: { stroke: GRID_LINE, lineWidth: 2 },
          }
        },
        data: rows.map((r, i) => [i, a(r), b(r)]),
        encode: { x: [1, 2], y: 0 },
      },
      {
        name: aName,
        type: 'scatter',
        symbolSize: 11,
        itemStyle: { color: IDENT[2], borderColor: '#150420', borderWidth: 2 },
        label: {
          show: true,
          position: 'left',
          color: INK_MUTED,
          fontSize: 10,
          formatter: (p: { value: number }) => fmt(p.value),
        },
        data: rows.map(a),
      },
      {
        // The gap is the whole reason this form was chosen, so it is
        // written on the line rather than left to be measured by eye.
        name: 'gap',
        type: 'scatter',
        silent: true,
        symbolSize: 0,
        label: {
          show: true,
          position: 'top',
          color: INK,
          fontSize: 10,
          fontWeight: 600,
          formatter: (p: { dataIndex: number }) => {
            const lo = a(rows[p.dataIndex])
            const hi = b(rows[p.dataIndex])
            return lo > 0 ? `${(hi / lo).toFixed(1)}×` : ''
          },
        },
        data: rows.map((r) => (a(r) + b(r)) / 2),
      },
      {
        name: bName,
        type: 'scatter',
        symbolSize: 11,
        itemStyle: { color: IDENT[0], borderColor: '#150420', borderWidth: 2 },
        label: {
          show: true,
          position: 'right',
          color: INK,
          fontSize: 10,
          formatter: (p: { value: number }) => fmt(p.value),
        },
        data: rows.map(b),
      },
    ],
  }
}

/// Where everything a model wrote ended up: still standing, landed and
/// later replaced, or never landed at all.
///
/// Three parts of one whole, so a stacked bar. The third segment is the
/// one the rest of the page cannot see — work committed on a branch that
/// was abandoned or deleted, which never appears in any landed figure.
function fate(rows: Row[]) {
  const seg = (
    name: string,
    colour: string,
    pickVal: (d: Row) => number,
    labelRight = false,
  ) => ({
    name,
    type: 'bar',
    stack: 'fate',
    barWidth: 18,
    itemStyle: { color: colour, borderColor: 'transparent', borderWidth: 1 },
    label: {
      show: true,
      position: labelRight ? 'right' : 'inside',
      color: labelRight ? colour : '#fff',
      fontSize: 10,
      formatter: (p: { value: number; dataIndex: number }) => {
        const r = rows[p.dataIndex]
        const tot = n(r.alive) + Math.max(0, n(r.added) - n(r.alive)) + n(r.abandoned_lines)
        return p.value > 0 && tot > 0
          ? `${fmtCompact(p.value)}  ${Math.round((100 * p.value) / tot)}%`
          : ''
      },
    },
    data: rows.map(pickVal),
  })
  return {
    tooltip: { ...TOOLTIP, trigger: 'axis', axisPointer: { type: 'shadow' } },
    legend: {
      data: ['still standing', 'landed, then replaced', 'never landed'],
      textStyle: { color: INK_MUTED },
      top: 0,
      right: 0,
    },
    grid: { left: 150, right: 96, top: 34, bottom: 42 },
    xAxis: {
      ...AXIS,
      type: 'value',
      name: 'lines written',
      nameLocation: 'middle',
      nameGap: 24,
      nameTextStyle: { color: INK_MUTED },
      axisLabel: { color: INK_MUTED, formatter: (x: number) => fmtCompact(x) },
      splitLine: { lineStyle: { color: GRID_LINE } },
    },
    yAxis: {
      type: 'category',
      data: rows.map((r) => r.model),
      axisLabel: { color: INK },
      axisLine: { lineStyle: { color: GRID_LINE } },
    },
    series: [
      seg('still standing', KEPT, (d) => n(d.alive)),
      seg('landed, then replaced', THROWN, (d) => Math.max(0, n(d.added) - n(d.alive))),
      seg('never landed', '#6B4C7A', (d) => n(d.abandoned_lines), true),
    ],
  }
}

/// What a million tokens buys, and how much of it is left.
///
/// A slopegraph, because the argument is a CHANGE between two states of
/// one unit — lines landed, then lines still standing — and a slope
/// shows decay in a way two bars never do. Both ends carry their value,
/// so the chart is read without touching the axis, and the steeper line
/// is the model losing more of what it wrote.
function slope(rows: Row[]) {
  const landed = (d: Row) => per(n(d.added), n(d.out_tokens), 1_000_000)
  return {
    tooltip: {
      ...TOOLTIP,
      trigger: 'item',
      formatter: (p: { seriesName: string; data: number[] }) =>
        `${p.seriesName}<br/>${p.data[0] === 0 ? 'landed' : 'still there'}: ${p.data[1]} lines per 1M tokens`,
    },
    legend: { data: rows.map((r) => r.model), textStyle: { color: INK_MUTED }, top: 0, right: 0 },
    grid: { left: 34, right: 34, top: 34, bottom: 34 },
    xAxis: {
      type: 'category',
      data: ['lines it landed', 'lines still there'],
      boundaryGap: ['22%', '22%'],
      axisLabel: { color: INK, fontSize: 11 },
      axisLine: { lineStyle: { color: GRID_LINE } },
      axisTick: { show: false },
      splitLine: { show: false },
    },
    yAxis: {
      ...AXIS,
      type: 'value',
      name: 'per 1M output tokens',
      nameTextStyle: { color: INK_MUTED },
      axisLabel: { show: false },
      splitLine: { show: false },
    },
    series: rows.map((r, i) => ({
      name: r.model,
      type: 'line',
      symbol: 'circle',
      symbolSize: 11,
      lineStyle: { width: 2, color: IDENT[i % IDENT.length] },
      itemStyle: { color: IDENT[i % IDENT.length], borderColor: '#150420', borderWidth: 2 },
      label: {
        show: true,
        color: INK,
        fontSize: 11,
        formatter: (p: { dataIndex: number; value: number[] }) =>
          p.dataIndex === 0
            ? `${r.model}  ${p.value[1]}`
            : `${p.value[1]}  (${landed(r) > 0 ? Math.round((100 * n(r.alive_per_mtok)) / landed(r)) : 0}% kept)`,
        position: (p: { dataIndex: number }) => (p.dataIndex === 0 ? 'left' : 'right'),
      },
      data: [
        [0, landed(r)],
        [1, n(r.alive_per_mtok)],
      ],
    })),
  }
}

/// Cheap against good, as a place on a map. Two measures of different
/// units belong on two axes of ONE plot, never on two y-scales of one
/// bar chart.
function quadrant(rows: Row[]) {
  return {
    tooltip: {
      ...TOOLTIP,
      formatter: (p: { data: [number, number, string, number] }) =>
        `${p.data[2]}<br/>${fmtCompact(p.data[0])} tokens per surviving line<br/>${p.data[1]}% survived · ${fmtCompact(p.data[3])} lines landed`,
    },
    grid: { left: 58, right: 40, top: 26, bottom: 46 },
    xAxis: {
      ...AXIS,
      type: 'value',
      name: 'tokens per surviving line  →  dearer',
      nameLocation: 'middle',
      nameGap: 26,
      nameTextStyle: { color: INK_MUTED },
      axisLabel: { color: INK_MUTED, formatter: (x: number) => fmtCompact(x) },
      splitLine: { lineStyle: { color: GRID_LINE } },
    },
    yAxis: {
      ...AXIS,
      type: 'value',
      name: 'survived %',
      max: 100,
      splitLine: { lineStyle: { color: GRID_LINE } },
    },
    series: [
      {
        type: 'scatter',
        symbolSize: (d: number[]) => Math.max(14, Math.min(54, Math.sqrt(d[3]) / 3)),
        itemStyle: {
          color: (p: { dataIndex: number }) => IDENT[p.dataIndex % IDENT.length],
          opacity: 0.85,
          borderColor: '#150420',
          borderWidth: 2,
        },
        label: {
          show: true,
          position: 'top',
          color: INK,
          fontSize: 10,
          lineHeight: 13,
          formatter: (p: { data: [number, number, string, number] }) =>
            `${p.data[2]}\n${fmtCompact(p.data[0])} tok/line · ${p.data[1]}% kept`,
        },
        data: rows.map((r) => [
          n(r.tokens_per_line_kept),
          n(r.survived_pct),
          r.model,
          n(r.added),
        ]),
      },
    ],
  }
}

function stacked(rows: Row[], kept: (d: Row) => number, gone: (d: Row) => number, unit: string, keptName: string, goneName: string) {
  return {
    tooltip: { ...TOOLTIP, trigger: 'axis', axisPointer: { type: 'shadow' } },
    legend: { data: [keptName, goneName], textStyle: { color: INK_MUTED }, top: 0, right: 0 },
    grid: { left: 150, right: 84, top: 32, bottom: 42 },
    xAxis: {
      ...AXIS,
      type: 'value',
      name: unit,
      nameLocation: 'middle',
      nameGap: 24,
      nameTextStyle: { color: INK_MUTED },
      axisLabel: { color: INK_MUTED, formatter: (x: number) => fmtCompact(x) },
      splitLine: { lineStyle: { color: GRID_LINE } },
    },
    yAxis: {
      type: 'category',
      data: rows.map((r) => r.model),
      axisLabel: { color: INK },
      axisLine: { lineStyle: { color: GRID_LINE } },
    },
    series: [
      {
        name: keptName,
        type: 'bar',
        stack: 's',
        barWidth: 16,
        itemStyle: { color: KEPT, borderColor: 'transparent', borderWidth: 1 },
        label: {
          show: true,
          color: '#fff',
          fontSize: 10,
          formatter: (p: { value: number; dataIndex: number }) => {
            const r = rows[p.dataIndex]
            const tot = kept(r) + gone(r)
            return p.value > 0 && tot > 0
              ? `${fmtCompact(p.value)}  ${Math.round((100 * p.value) / tot)}%`
              : ''
          },
        },
        data: rows.map(kept),
      },
      {
        name: goneName,
        type: 'bar',
        stack: 's',
        barWidth: 16,
        itemStyle: { color: THROWN, borderColor: 'transparent', borderWidth: 1 },
        label: {
          show: true,
          position: 'right',
          color: THROWN,
          fontSize: 10,
          formatter: (p: { value: number; dataIndex: number }) => {
            const r = rows[p.dataIndex]
            const tot = kept(r) + gone(r)
            return p.value > 0 && tot > 0
              ? `${fmtCompact(p.value)}  ${Math.round((100 * p.value) / tot)}%`
              : ''
          },
        },
        data: rows.map(gone),
      },
    ],
  }
}

/// The four headline comparisons, as figures rather than charts.
///
/// A single number per family, compared once, is read faster as a
/// number than as a picture of a number — so these are not charts, and
/// the guidance is explicit that sometimes the right form is no chart
/// at all. Each carries the ratio, which is the part that actually
/// decides anything.
function heroes(rows: Row[]): { k: string; v: string; sub: string; good: boolean }[] {
  const v = verdict(rows)
  if (!v) return []
  const ratio = (a: number, b: number) => (b > 0 ? `${(a / b).toFixed(1)}×` : '—')
  const hrsPer1k = (d: Row) =>
    n(d.alive) > 0 ? Math.round(n(d.minutes) / 60 / (n(d.alive) / 1000)) : 0
  return [
    {
      k: 'Lasting lines per 1M tokens',
      v: ratio(n(v.best.alive_per_mtok), n(v.worst.alive_per_mtok)),
      sub: `${fmtCompact(n(v.best.alive_per_mtok))} from ${v.best.model} · ${fmtCompact(n(v.worst.alive_per_mtok))} from ${v.worst.model}`,
      good: true,
    },
    {
      k: 'Lasting lines per hour',
      v: ratio(n(v.best.alive_per_hour), n(v.worst.alive_per_hour)),
      sub: `${n(v.best.alive_per_hour)} from ${v.best.model} · ${n(v.worst.alive_per_hour)} from ${v.worst.model}`,
      good: true,
    },
    {
      k: 'Tokens per surviving line',
      v: ratio(n(v.worst.tokens_per_line_kept), n(v.best.tokens_per_line_kept)),
      sub: `${fmtCompact(n(v.worst.tokens_per_line_kept))} from ${v.worst.model} · ${fmtCompact(n(v.best.tokens_per_line_kept))} from ${v.best.model}`,
      good: false,
    },
    {
      k: 'Hours per 1,000 surviving lines',
      v: ratio(hrsPer1k(v.worst), hrsPer1k(v.best)),
      sub: `${hrsPer1k(v.worst)}h from ${v.worst.model} · ${hrsPer1k(v.best)}h from ${v.best.model}`,
      good: false,
    },
  ]
}

function verdict(rows: Row[]) {
  const ok = rows.filter((r) => n(r.added) >= MIN_ADDED)
  if (ok.length < 2) return null
  const best = ok.reduce((a, b) => (n(a.alive_per_mtok) >= n(b.alive_per_mtok) ? a : b))
  // Compare the best producer against where the budget ACTUALLY GOES,
  // not against whichever model happens to score lowest. The point of
  // the comparison is the money being spent, and that is the model
  // carrying the largest token bill — pairing the best against a model
  // nobody is spending on makes a true statement about nothing.
  const worst = ok.reduce((a, b) => (n(a.out_tokens) >= n(b.out_tokens) ? a : b))
  if (best.model === worst.model) return null
  const tokX = n(worst.alive_per_mtok) > 0 ? n(best.alive_per_mtok) / n(worst.alive_per_mtok) : 0
  const hrX = n(worst.alive_per_hour) > 0 ? n(best.alive_per_hour) / n(worst.alive_per_hour) : 0
  return { best, worst, tokX, hrX }
}

export function ModelComparison({ c }: { c: CompareSummary | undefined }) {
  if (!c) {
    return (
      <Panel title="Model comparison" scope="all" range={null}>
        <Text size="xs" c="dimmed">reading the repositories — git says what landed, blame says what is left.</Text>
      </Panel>
    )
  }
  const rows = (c.deviations ?? []).map(toRow)
  if (rows.length === 0) {
    return (
      <Panel title="Model comparison" scope="all" range={null}>
        <Text size="xs" c="dimmed">no commit could be credited to a single model yet.</Text>
      </Panel>
    )
  }
  const fams = byFamily(rows)
  const v = verdict(fams)
  // Per repository, folded to families like everything else. Two point
  // releases working the same checkout are one choice, not two.
  const repoFam = (() => {
    const acc = new Map<string, { repo: string; model: string; added: number; alive: number }>()
    for (const r of c.repos ?? []) {
      const k = `${r.repo}|${family(r.model)}`
      const p = acc.get(k)
      if (p) {
        p.added += n(r.added)
        p.alive += n(r.alive)
      } else {
        acc.set(k, { repo: r.repo, model: family(r.model), added: n(r.added), alive: n(r.alive) })
      }
    }
    return [...acc.values()]
      .map((r) => ({ ...r, survived_pct: r.added > 0 ? Math.round((100 * r.alive) / r.added) : 0 }))
      .sort((a, b) => b.added - a.added)
  })()
  const repos = repoFam.filter((r) => r.added >= 1000).slice(0, 12)

  // Handoffs between FAMILIES. A fable-5 to fable-5-1 change is a point
  // release, not a decision to switch model, so folding removes it from
  // the count rather than reporting it as a switch nobody made.
  const handFam = (() => {
    const acc = new Map<string, { from: string; to: string; switches: number; commits: number; added: number; alive: number }>()
    for (const h of c.handoffs ?? []) {
      const from = family(h.from)
      const to = family(h.to)
      if (from === to) continue
      const k = `${from}|${to}`
      const p = acc.get(k)
      if (p) {
        p.switches += n(h.switches)
        p.commits += n(h.commits)
        p.added += n(h.added)
        p.alive += n(h.alive)
      } else {
        acc.set(k, { from, to, switches: n(h.switches), commits: n(h.commits), added: n(h.added), alive: n(h.alive) })
      }
    }
    return [...acc.values()]
      .map((h) => ({ ...h, survived_pct: h.added > 0 ? Math.round((100 * h.alive) / h.added) : 0 }))
      .sort((a, b) => b.switches - a.switches)
  })()
  // Only switches that produced commits can say anything about what the
  // incoming family then did.
  const hand = handFam.filter((h) => h.commits > 0)

  const ms = metrics()

  const switchChart = {
    tooltip: { ...TOOLTIP, trigger: 'axis', axisPointer: { type: 'shadow' } },
    grid: { left: 190, right: 90, top: 10, bottom: 42 },
    xAxis: {
      ...AXIS, type: 'value', max: 100,
      name: 'of what it wrote just after taking over, % still there',
      nameLocation: 'middle', nameGap: 24, nameTextStyle: { color: INK_MUTED },
      splitLine: { lineStyle: { color: GRID_LINE } },
    },
    yAxis: {
      type: 'category',
      data: hand.map((h) => `${h.from} → ${h.to}`),
      axisLabel: { color: INK, fontSize: 10 },
      axisLine: { lineStyle: { color: GRID_LINE } },
    },
    series: [{
      type: 'bar', barWidth: 16,
      itemStyle: { color: IDENT[0], borderRadius: [0, 4, 4, 0] },
      label: {
        show: true, position: 'right', color: INK, fontSize: 11,
        formatter: (p: { value: number; dataIndex: number }) => `${p.value}%  ·  ${fmtCompact(n(hand[p.dataIndex]?.added))} lines`,
      },
      data: hand.map((h) => n(h.survived_pct)),
    }],
  }

  const repoChart = {
    tooltip: { ...TOOLTIP, trigger: 'axis', axisPointer: { type: 'shadow' } },
    grid: { left: 210, right: 80, top: 10, bottom: 42 },
    xAxis: {
      ...AXIS, type: 'value', max: 100,
      name: '% of what it landed there that is still in the tree',
      nameLocation: 'middle', nameGap: 24, nameTextStyle: { color: INK_MUTED },
      splitLine: { lineStyle: { color: GRID_LINE } },
    },
    yAxis: {
      type: 'category',
      data: repos.map((r) => `${r.repo} · ${r.model.replace('claude-', '')}`),
      axisLabel: { color: INK, fontSize: 10 },
      axisLine: { lineStyle: { color: GRID_LINE } },
    },
    series: [{
      type: 'bar', barWidth: 12,
      itemStyle: { color: (p: { value: number }) => (p.value >= 50 ? KEPT : THROWN), borderRadius: [0, 3, 3, 0] },
      label: {
        show: true, position: 'right', color: INK, fontSize: 10,
        formatter: (p: { value: number; dataIndex: number }) => `${p.value}%  ·  ${fmtCompact(n(repos[p.dataIndex]?.added))} lines`,
      },
      data: repos.map((r) => n(r.survived_pct)),
    }],
  }

  return (
    <>
      <Panel title="Which model to reach for" scope="all" range={null} note="one walk of git, one set of numbers" mb="md">
        {/* The heading is a NAME, not a claim. An earlier version put the
            computed conclusion here, so the section retitled itself every
            time the numbers moved — 3.0x one hour, 1.4x the next — which
            reads as an unstable instrument rather than a finding. The
            comparison belongs in a table; the heading stays put. */}
        {v && (
          <Box mb="md">
            <Table fz="sm" horizontalSpacing="md" verticalSpacing={6} withTableBorder>
              <Table.Thead>
                <Table.Tr>
                  <Table.Th>Per unit spent</Table.Th>
                  <Table.Th ta="right">{v.best.model}</Table.Th>
                  <Table.Th ta="right">{v.worst.model}</Table.Th>
                  <Table.Th ta="right">Ratio</Table.Th>
                </Table.Tr>
              </Table.Thead>
              <Table.Tbody>
                <Table.Tr>
                  <Table.Td>Lasting lines per 1M output tokens</Table.Td>
                  <Table.Td ta="right" c={KEPT}>{fmtCompact(n(v.best.alive_per_mtok))}</Table.Td>
                  <Table.Td ta="right">{fmtCompact(n(v.worst.alive_per_mtok))}</Table.Td>
                  <Table.Td ta="right">{v.tokX.toFixed(1)}×</Table.Td>
                </Table.Tr>
                <Table.Tr>
                  <Table.Td>Lasting lines per hour</Table.Td>
                  <Table.Td ta="right" c={KEPT}>{n(v.best.alive_per_hour)}</Table.Td>
                  <Table.Td ta="right">{n(v.worst.alive_per_hour)}</Table.Td>
                  <Table.Td ta="right">{v.hrX.toFixed(1)}×</Table.Td>
                </Table.Tr>
                <Table.Tr>
                  <Table.Td>Spent, to leave that behind</Table.Td>
                  <Table.Td ta="right">
                    {fmtCompact(n(v.best.out_tokens))} · {Math.round(n(v.best.minutes) / 60)}h
                  </Table.Td>
                  <Table.Td ta="right">
                    {fmtCompact(n(v.worst.out_tokens))} · {Math.round(n(v.worst.minutes) / 60)}h
                  </Table.Td>
                  <Table.Td ta="right">—</Table.Td>
                </Table.Tr>
              </Table.Tbody>
            </Table>
            <Text size="xs" c="dimmed" mt={6}>
              A cheaper token that buys fewer lasting lines is the dearer choice. These are rates,
              so the totals they came from are in the full table at the foot of the section.
            </Text>
          </Box>
        )}
        <SimpleGrid cols={{ base: 2, md: 4 }} spacing="xs" mb="md">
          {heroes(fams).map((h) => (
            <Box key={h.k} style={{ border: `1px solid ${GRID_LINE}`, borderRadius: 8, padding: '12px 14px' }}>
              <Text fz={10} c="dimmed" tt="uppercase" style={{ letterSpacing: '0.08em' }}>
                {h.k}
              </Text>
              <Text fz={26} fw={600} c={h.good ? KEPT : THROWN} style={{ lineHeight: 1.15, fontVariantNumeric: 'tabular-nums' }}>
                {h.v}
              </Text>
              <Text fz={11} c="dimmed">
                {h.sub}
              </Text>
            </Box>
          ))}
        </SimpleGrid>
        <Text size="xs" c="dimmed">
          Four numbers, no chart: a single headline comparison is read faster as a figure than as a
          picture of a figure. Everything below is the working behind them.
        </Text>
      </Panel>

      <Panel
        title="Everything it wrote, and where it ended up"
        scope="all"
        range={null}
        note="the third bar is work that never reached the main line at all"
        mb="md"
      >
        <EChart option={fate(fams)} height={76 + fams.length * 46} />
        <Text size="xs" c="dimmed" mt={4}>
          Every other figure on this page measures work that LANDED and was later replaced. A
          branch written, committed to, and then abandoned or deleted appears in none of them,
          because none of it ever landed. This is the only chart that counts it.
        </Text>
      </Panel>

      <SimpleGrid cols={{ base: 1, lg: 3 }} mb="md">
        <Panel title="Of what it landed" scope="all" range={null} note="still there against gone">
          <EChart
            option={stacked(fams, (d) => n(d.alive), (d) => Math.max(0, n(d.added) - n(d.alive)), 'lines', 'still there', 'gone')}
            height={60 + fams.length * 40}
          />
        </Panel>
        <Panel title="Of what it cost" scope="all" range={null} note="tokens that bought lasting code, against tokens that did not">
          <EChart
            option={stacked(fams, (d) => Math.max(0, n(d.out_tokens) - n(d.tokens_thrown)), (d) => n(d.tokens_thrown), 'output tokens', 'kept', 'thrown away')}
            height={60 + fams.length * 40}
          />
        </Panel>
        <Panel title="Of your time" scope="all" range={null} note="the cost you cannot get back">
          <EChart
            option={stacked(fams, (d) => Math.max(0, Math.round((n(d.minutes) - n(d.minutes_thrown)) / 60)), (d) => Math.round(n(d.minutes_thrown) / 60), 'hours', 'hours that lasted', 'hours thrown away')}
            height={60 + fams.length * 40}
          />
        </Panel>
      </SimpleGrid>

      <SimpleGrid cols={{ base: 1, lg: 2 }} mb="md">
        <Panel title="What a million tokens buys" scope="all" range={null} note="and how much of it is still there">
          <EChart option={slope(fams)} height={230} />
        </Panel>
        <Panel title="Cheap against good" scope="all" range={null} note="bubble is lines landed · bottom right is the worst place to be">
          <EChart option={quadrant(fams)} height={230} />
        </Panel>
        <Panel title="What a line cost, before and after rework" scope="all" range={null} note="the gap is the rework tax">
          <EChart
            option={dumbbell(fams, (d) => n(d.tokens_per_line_landed), (d) => n(d.tokens_per_line_kept), 'per line landed', 'per line still there', 'output tokens per line', fmtCompact)}
            height={230}
          />
        </Panel>
        <Panel title="Context it carried" scope="all" range={null} note="typical turn against its largest">
          <EChart
            option={dumbbell(fams, (d) => n(d.context_avg), (d) => n(d.context_peak), 'typical prompt', 'largest prompt', 'tokens in the prompt', fmtCompact)}
            height={230}
          />
        </Panel>
      </SimpleGrid>

      <Panel title="The rankings" scope="all" range={null} note="measures whose job is simply an order" mb="md">
        <SimpleGrid cols={{ base: 1, md: 2, xl: 3 }} spacing="xs">
          {ms.map((m) => (
            <Box key={m.title}>
              <Text fz={11} fw={600} c={INK}>
                {m.title}
              </Text>
              <Text fz={9} c="dimmed" mb={2}>
                {m.note} · {m.good === 'high' ? 'higher is better' : 'lower is better'}
              </Text>
              <EChart option={mini(fams, m)} height={26 + fams.length * 22} />
            </Box>
          ))}
        </SimpleGrid>
        <Text size="xs" c="dimmed" mt="sm">
          Green is the better family. Point releases are folded together — you reach for Fable or you
          reach for Opus — and every rate is recomputed from the summed totals rather than averaged.
        </Text>
      </Panel>

      {repos.length > 0 && (
        <Panel title="Where it happened, repository by repository" scope="all" range={null} note="so one bad checkout is visible rather than averaged away" mb="md">
          <EChart option={repoChart} height={50 + repos.length * 26} />
        </Panel>
      )}

      {hand.length > 0 && (
        <Panel title="What happens right after the model is switched" scope="all" range={null} note={`${hand.length} switch directions that produced commits within three hours`} mb="md">
          <EChart option={switchChart} height={40 + hand.length * 42} />
          <Text size="xs" c="dimmed" mt={4}>
            The account being tested is that a budget runs out, another model takes over, and what it
            does then gets thrown away. That is not what this shows: code written in the first hours
            after a takeover survives ABOVE each model&apos;s own average. The damage is in long
            unbroken runs, which makes run length the thing to control, not who picks the work up.
          </Text>
        </Panel>
      )}

      <Panel title="The comparison, in full" scope="all" range={null} note="point releases folded — reaching for Fable or Opus is the choice actually made">
        <Table striped withTableBorder fz="xs" horizontalSpacing="xs">
          <Table.Thead>
            <Table.Tr>
              <Table.Th>Model</Table.Th>
              <Table.Th ta="right">Tokens</Table.Th>
              <Table.Th ta="right">Hours</Table.Th>
              <Table.Th ta="right">Landed</Table.Th>
              <Table.Th ta="right">Still there</Table.Th>
              <Table.Th ta="right">Survived</Table.Th>
              <Table.Th ta="right">Alive /Mtok</Table.Th>
              <Table.Th ta="right">Alive /hour</Table.Th>
              <Table.Th ta="right">Tok /kept</Table.Th>
              <Table.Th ta="right">Never landed</Table.Th>
              <Table.Th ta="right">Age</Table.Th>
            </Table.Tr>
          </Table.Thead>
          <Table.Tbody>
            {fams.map((r) => {
              const thin = n(r.added) < MIN_ADDED
              return (
                <Table.Tr key={r.model} opacity={thin ? 0.55 : 1}>
                  <Table.Td>{r.model}</Table.Td>
                  <Table.Td ta="right">{fmtCompact(n(r.out_tokens))}</Table.Td>
                  <Table.Td ta="right">{Math.round(n(r.minutes) / 60)}</Table.Td>
                  <Table.Td ta="right">{fmtCompact(n(r.added))}</Table.Td>
                  <Table.Td ta="right">{fmtCompact(n(r.alive))}</Table.Td>
                  <Table.Td ta="right" c={n(r.survived_pct) >= 50 ? KEPT : THROWN}>{n(r.survived_pct)}%</Table.Td>
                  <Table.Td ta="right">{fmtCompact(n(r.alive_per_mtok))}</Table.Td>
                  <Table.Td ta="right">{n(r.alive_per_hour)}</Table.Td>
                  <Table.Td ta="right">{fmtCompact(n(r.tokens_per_line_kept))}</Table.Td>
                  <Table.Td ta="right" c={n(r.abandoned_pct) >= 50 ? THROWN : undefined}>
                    {fmtCompact(n(r.abandoned_lines))} · {n(r.abandoned_pct)}%
                  </Table.Td>
                  <Table.Td ta="right">{n(r.median_age_days)}d</Table.Td>
                </Table.Tr>
              )
            })}
          </Table.Tbody>
        </Table>
        <Text size="xs" c="dimmed" mt="xs">
          Two measures do NOT separate these models and are in the grid so that can be seen rather
          than assumed: how often you have to correct, and how often a file is returned to. What
          separates them is what the work is worth afterwards, and how much token and clock time it
          took to get there.
        </Text>
      </Panel>
    </>
  )
}
