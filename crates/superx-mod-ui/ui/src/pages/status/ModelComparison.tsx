import { Box, SimpleGrid, Table, Text, Title } from '@mantine/core'
import { AXIS, CHART_COLORS, EChart, GRID_LINE, INK, INK_MUTED, TOOLTIP, insideFits } from '../../EChart'
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
// A model's colour: the validated ramp (EChart.tsx) less the red and the
// greens that mean thrown away and kept on this very page. Three colours
// for six versions painted Opus 5 and Opus 5.5 alike (#415 QA).
const IDENT = [CHART_COLORS[0], CHART_COLORS[5], CHART_COLORS[2], CHART_COLORS[3], CHART_COLORS[6]]
const MIN_ADDED = 5000

/// Each row's colour. The models with work enough to place take the
/// identity colours in order; one under MIN_ADDED lines is drawn muted,
/// as its row in the table is faded.
function colours(rows: Row[]): string[] {
  let k = 0
  return rows.map((r) => (n(r.added) < MIN_ADDED ? INK_MUTED : IDENT[k++ % IDENT.length]))
}

// Models are folded on the server (#414), and each version is its own
// model (#421): every row is one model at its version, and every rate and
// median in it was computed from its own totals. Re-folding here dropped
// the never-landed figures.

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
/// Working hours from minutes, to one decimal under ten (#415 QA): rounded
/// to whole hours, eight minutes read "0" beside the 1,027 lines an hour
/// computed from them.
const hours = (minutes: number) => {
  const h = minutes / 60
  return h < 10 ? h.toFixed(1) : String(Math.round(h))
}

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
    { title: 'Age of the work', note: 'days — the confounder, check it', good: 'high', fmt: (v) => (v < 0 ? '—' : `${v}d`), pick: (d) => n(d.median_age_days) },
    { title: 'Written but never landed', note: 'share of everything it wrote', good: 'low', fmt: (v) => `${v}%`, pick: (d) => n(d.abandoned_pct) },
    { title: 'Commits on abandoned branches', note: 'deleted or never merged', good: 'low', pick: (d) => n(d.abandoned_commits) },
  ]
}

/// One small multiple: a lollipop per model, the best on top once ECharts
/// flips the category axis. A rate is only as good as the work behind it
/// (operator, #415 QA): one file can score what two hundred cannot, and
/// Sonnet 5 ranked second on 190 lines. So every figure carries the lines
/// it rests on, and a model under MIN_ADDED lines is drawn faded under the
/// ranked ones — shown, never ranked, never marked best.
function mini(all: Row[], m: Metric) {
  // A negative value is the payload's "no data" (-1), never a reading: no
  // metric here can go below zero. It is left out, not drawn left of zero.
  const rows = all.filter((r) => m.pick(r) >= 0)
  const worstFirst = (a: Row, b: Row) => (m.good === 'high' ? m.pick(a) - m.pick(b) : m.pick(b) - m.pick(a))
  const ranked = rows.filter((r) => n(r.added) >= MIN_ADDED).sort(worstFirst)
  const thin = rows.filter((r) => n(r.added) < MIN_ADDED).sort((a, b) => n(a.added) - n(b.added))
  // The category axis runs bottom-up: the thin rows first, under the ranked.
  const sorted = [...thin, ...ranked]
  const isThin = (i: number) => i < thin.length
  const picks = ranked.map(m.pick)
  const best = picks.length === 0 ? null : m.good === 'high' ? Math.max(...picks) : Math.min(...picks)
  const fmt = (v: number) => (m.fmt ? m.fmt(v) : `${v}`)
  return {
    tooltip: {
      ...TOOLTIP,
      trigger: 'axis',
      axisPointer: { type: 'shadow' },
      formatter: (ps: { dataIndex: number }[]) => {
        const i = ps[0]?.dataIndex ?? 0
        const r = sorted[i]
        if (!r) return ''
        const work = `${fmtCompact(n(r.added))} lines landed · ${n(r.commits)} commits`
        return `${r.model}<br/>${fmt(m.pick(r))}<br/>${work}${isThin(i) ? '<br/>too little work to rank' : ''}`
      },
    },
    grid: { left: 108, right: 116, top: 6, bottom: 6, containLabel: false },
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
        // A lollipop, not a bar. One value per model needs a stem and a
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
          color: (p: { value: number; dataIndex: number }) =>
            isThin(p.dataIndex) ? INK_MUTED : p.value === best ? KEPT : THROWN,
          borderColor: '#150420',
          borderWidth: 2,
        },
        label: {
          show: true,
          position: 'right',
          formatter: (p: { value: number; dataIndex: number }) => {
            const lines = `${fmtCompact(n(sorted[p.dataIndex]?.added))} lines`
            return isThin(p.dataIndex) ? `{tv|${fmt(p.value)}}  {tn|${lines}}` : `{v|${fmt(p.value)}}  {n|${lines}}`
          },
          rich: {
            v: { color: INK, fontSize: 11 },
            n: { color: INK_MUTED, fontSize: 10 },
            tv: { color: INK_MUTED, fontSize: 11 },
            tn: { color: INK_MUTED, fontSize: 10 },
          },
        },
        data: sorted.map((r, i) => (isThin(i) ? { value: m.pick(r), itemStyle: { opacity: 0.55 } } : m.pick(r))),
        z: 2,
      },
    ],
  }
}


/// A dumbbell: two values of the SAME unit per model, joined by the
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
  // A rate is only as good as the work behind it (#415 QA): each name
  // carries its lines, and a model under MIN_ADDED is drawn faded.
  const thin = (r: Row) => n(r.added) < MIN_ADDED
  const faded = (r: Row, value: number, label?: object) =>
    thin(r) ? { value, itemStyle: { opacity: 0.45 }, ...(label ? { label } : {}) } : value
  return {
    tooltip: { ...TOOLTIP, trigger: 'axis', axisPointer: { type: 'shadow' } },
    legend: { data: [aName, bName], textStyle: { color: INK_MUTED }, top: 0, right: 0 },
    // The names stand off the plot far enough for a value at the axis to
    // sit between them: it ran into its model's name (#415 QA).
    grid: { left: 8, right: 88, top: 30, bottom: 40, containLabel: true },
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
      axisLabel: {
        margin: 44,
        formatter: (v: string, i: number) =>
          `{${thin(rows[i]) ? 't' : 'm'}|${v}}\n{n|${fmtCompact(n(rows[i]?.added))} lines}`,
        rich: {
          m: { color: INK, fontSize: 11 },
          t: { color: INK_MUTED, fontSize: 11 },
          n: { color: INK_MUTED, fontSize: 9 },
        },
      },
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
        data: rows.map((r) => faded(r, a(r))),
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
        data: rows.map((r) => faded(r, (a(r) + b(r)) / 2, { color: INK_MUTED })),
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
        data: rows.map((r) => faded(r, b(r), { color: INK_MUTED })),
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
    ...(labelRight ? {} : { labelLayout: insideFits }),
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
/// The slope chart's height, which its label spread is computed for.
const SLOPE_HEIGHT = 230

function slope(rows: Row[]) {
  const landed = (d: Row) => per(n(d.added), n(d.out_tokens), 1_000_000)
  const kept = (d: Row) => n(d.alive_per_mtok)
  const colour = colours(rows)
  // Each end's labels are spread so no two overlap (#415 QA): four sat on
  // top of each other. ECharts' own shift treats every label on a chart
  // as one column, and this chart has two, so the spread is done here —
  // the axis max is fixed, which makes each value's height known.
  const [height, top, bottom, line] = [SLOPE_HEIGHT, 34, 34, 14]
  const max = Math.max(1, ...rows.flatMap((r) => [landed(r), kept(r)])) * 1.08
  const spread = (vals: number[]): number[] => {
    const at = vals.map((v) => top + (height - top - bottom) * (1 - v / max))
    const order = at.map((y, i) => [y, i]).sort((a, b) => a[0] - b[0])
    const pos: number[] = []
    let last = -Infinity
    for (const [y, i] of order) {
      pos[i] = Math.max(y, last + line)
      last = pos[i]
    }
    // Pushed past the floor: walk back up. The floor is the plot's, so no
    // label reaches the axis names under it.
    let floor = height - bottom - line / 2
    for (const [, i] of [...order].reverse()) {
      pos[i] = Math.min(pos[i], floor)
      floor = pos[i] - line
    }
    return pos.map((p, i) => p - at[i])
  }
  const [dyLanded, dyKept] = [spread(rows.map(landed)), spread(rows.map(kept))]
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
      max,
      axisLabel: { show: false },
      splitLine: { show: false },
    },
    series: rows.map((r, i) => ({
      name: r.model,
      type: 'line',
      symbol: 'circle',
      symbolSize: 11,
      lineStyle: { width: 2, color: colour[i] },
      itemStyle: { color: colour[i], borderColor: '#150420', borderWidth: 2 },
      // In its line's colour, so a label moved off its dot still says whose.
      label: {
        show: true,
        color: colour[i],
        fontSize: 11,
        formatter: (p: { dataIndex: number; value: number[] }) =>
          p.dataIndex === 0
            ? `${r.model}  ${p.value[1]}`
            : `${p.value[1]}  (${landed(r) > 0 ? Math.round((100 * kept(r)) / landed(r)) : 0}% kept)`,
      },
      // A position per point: the series-wide `position` takes no function.
      data: [
        { value: [0, landed(r)], label: { position: 'left', offset: [0, dyLanded[i]] } },
        { value: [1, kept(r)], label: { position: 'right', offset: [0, dyKept[i]] } },
      ],
    })),
  }
}

/// Cheap against good, as a place on a map. Two measures of different
/// units belong on two axes of ONE plot, never on two y-scales of one
/// bar chart.
function quadrant(rows: Row[]) {
  const colour = colours(rows)
  const xmax = Math.max(1, ...rows.map((r) => n(r.tokens_per_line_kept)))
  return {
    tooltip: {
      ...TOOLTIP,
      formatter: (p: { data: [number, number, string, number] }) =>
        `${p.data[2]}<br/>${fmtCompact(p.data[0])} tokens per surviving line<br/>${p.data[1]}% survived · ${fmtCompact(p.data[3])} lines landed`,
    },
    // The legend says whose bubble is whose; a label names it where it
    // fits and gives way where it would land on another (#415 QA — six
    // two-line labels sat on each other and on the axis name).
    legend: { data: rows.map((r) => r.model), textStyle: { color: INK_MUTED }, top: 0, right: 0 },
    grid: { left: 58, right: 40, top: 40, bottom: 46 },
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
      nameLocation: 'middle',
      nameGap: 36,
      nameTextStyle: { color: INK_MUTED },
      max: 100,
      splitLine: { lineStyle: { color: GRID_LINE } },
    },
    series: rows.map((r, i) => ({
      name: r.model,
      type: 'scatter',
      symbolSize: (d: number[]) => Math.max(14, Math.min(54, Math.sqrt(d[3]) / 3)),
      itemStyle: { color: colour[i], opacity: 0.85, borderColor: '#150420', borderWidth: 2 },
      labelLayout: { hideOverlap: true },
      label: {
        show: true,
        // Near the right edge a label on the right would be cut off.
        position: n(r.tokens_per_line_kept) > 0.6 * xmax ? 'left' : 'right',
        color: INK,
        fontSize: 10,
        formatter: () => r.model,
      },
      data: [[n(r.tokens_per_line_kept), n(r.survived_pct), r.model, n(r.added)]],
    })),
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
        labelLayout: insideFits,
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
/// A single number per model, compared once, is read faster as a
/// number than as a picture of a number — so these are not charts, and
/// the guidance is explicit that sometimes the right form is no chart
/// at all. Each carries the ratio, which is the part that actually
/// decides anything.
function heroes(rows: Row[]): { k: string; v: string; sub: string; good: boolean }[] {
  const v = verdict(rows)
  if (!v) return []
  const ratio = (a: number, b: number) => (b > 0 ? `${(a / b).toFixed(1)}×` : '—')
  // Unrounded: the ratio is taken on these, and rounding first made it
  // 3.5× where lines-per-hour — its reciprocal — read 2.8× (#415 QA).
  const hrsPer1k = (d: Row) => (n(d.alive) > 0 ? n(d.minutes) / 60 / (n(d.alive) / 1000) : 0)
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
      sub: `${hours(hrsPer1k(v.worst) * 60)}h from ${v.worst.model} · ${hours(hrsPer1k(v.best) * 60)}h from ${v.best.model}`,
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
  const fams = rows
  const v = verdict(fams)
  const repos = (c.repos ?? [])
    .map((r) => ({ repo: r.repo, model: r.model, added: n(r.added), alive: n(r.alive), survived_pct: n(r.survived_pct) }))
    .filter((r) => r.added >= 1000)
    .slice(0, 12)
  // Only switches that produced commits can say anything about what the
  // incoming model then did.
  const hand = (c.handoffs ?? [])
    .map((h) => ({ from: h.from, to: h.to, switches: n(h.switches), commits: n(h.commits), added: n(h.added), alive: n(h.alive), survived_pct: n(h.survived_pct) }))
    .filter((h) => h.commits > 0)
  // Does work written just after a takeover survive better or worse than
  // the incoming model's own average? Read from the rows, not asserted —
  // and a tie is a tie: near 100% survival it is the common case, and
  // counting it as "below" announced a loss that was not there (#415
  // review).
  const versusOwn = (h: (typeof hand)[number]) => {
    const own = fams.find((f) => f.model === h.to)
    return own == null ? null : Math.sign(h.survived_pct - n(own.survived_pct))
  }
  const aboveOwn = hand.filter((h) => versusOwn(h) === 1).length
  const levelOwn = hand.filter((h) => versusOwn(h) === 0).length
  const belowOwn = hand.filter((h) => versusOwn(h) === -1).length
  const unjudged = c.unjudged ?? []

  const ms = metrics()

  const switchChart = {
    tooltip: { ...TOOLTIP, trigger: 'axis', axisPointer: { type: 'shadow' } },
    // Room for a full bar's end label: "100% · 248 lines" was cut off (#415 QA).
    grid: { left: 190, right: 130, top: 10, bottom: 42 },
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
    grid: { left: 210, right: 130, top: 10, bottom: 42 },
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
                    {fmtCompact(n(v.best.out_tokens))} · {hours(n(v.best.minutes))}h
                  </Table.Td>
                  <Table.Td ta="right">
                    {fmtCompact(n(v.worst.out_tokens))} · {hours(n(v.worst.minutes))}h
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
          <EChart option={slope(fams)} height={SLOPE_HEIGHT} />
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
          Green is the best of the models with work enough to rank — {MIN_ADDED.toLocaleString()} landed
          lines or more. A faded row did less: one file can score what two hundred cannot, so its figure is
          shown beside the lines behind it, not ranked. Each version is its own model — Opus 5.5 is not
          Opus 5 — and every rate is recomputed from its own summed totals rather than averaged.
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
            does then gets thrown away. Of {hand.length} switch direction{hand.length === 1 ? '' : 's'}, what the
            incoming model wrote in its first three hours survives above its own average in {aboveOwn}, level
            with it in {levelOwn} and below it in {belowOwn}
            {belowOwn === 0
              ? ' — the handover is not where the work is lost.'
              : belowOwn === hand.length
                ? ' — in every one: what follows a takeover is thrown away more than usual.'
                : '.'}
          </Text>
        </Panel>
      )}

      <Panel title="The comparison, in full" scope="all" range={null} note="each version its own model · a dated snapshot counts as its version">
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
                  <Table.Td ta="right">{hours(n(r.minutes))}</Table.Td>
                  <Table.Td ta="right">{fmtCompact(n(r.added))}</Table.Td>
                  <Table.Td ta="right">{fmtCompact(n(r.alive))}</Table.Td>
                  <Table.Td ta="right" c={n(r.survived_pct) >= 50 ? KEPT : THROWN}>{n(r.survived_pct)}%</Table.Td>
                  <Table.Td ta="right">{fmtCompact(n(r.alive_per_mtok))}</Table.Td>
                  <Table.Td ta="right">{n(r.alive_per_hour)}</Table.Td>
                  <Table.Td ta="right">{fmtCompact(n(r.tokens_per_line_kept))}</Table.Td>
                  <Table.Td ta="right" c={n(r.abandoned_pct) >= 50 ? THROWN : undefined}>
                    {fmtCompact(n(r.abandoned_lines))} · {n(r.abandoned_pct)}%
                  </Table.Td>
                  <Table.Td ta="right">{n(r.median_age_days) < 0 ? '—' : `${n(r.median_age_days)}d`}</Table.Td>
                </Table.Tr>
              )
            })}
          </Table.Tbody>
        </Table>
        <Text size="xs" c="dimmed" mt="xs">
          Hours are working hours — the gaps between a run&apos;s replies, each capped at the live
          threshold — and tokens are counted once per reply, over the work done in the repositories
          judged here. Corrections and files returned to are in the grid beside what the work was
          worth; read them side by side rather than as a verdict.
        </Text>
        {fams.some((r) => n(r.in_flight_lines) > 0) && (
          <Text size="xs" c="dimmed" mt="xs">
            In flight, and not counted as never landed:{' '}
            {fams
              .filter((r) => n(r.in_flight_lines) > 0)
              .map((r) => `${r.model} ${fmtCompact(n(r.in_flight_lines))} lines`)
              .join(' · ')}{' '}
            on branches a checkout still has out.
          </Text>
        )}
        {unjudged.length > 0 && (
          <Text size="xs" c="yellow.5" mt="xs">
            Not judged:{' '}
            {unjudged
              .map((u) =>
                u.unresolved
                  ? `${u.repo} (its main line ${u.mainline} does not resolve in git)`
                  : `${u.repo} (its main line ${u.mainline} took none of this machine's commits in the period while ${n(u.off_mainline_commits)} went elsewhere)`,
              )
              .join('; ')}
            . Counting it would call all of its work never landed; <code>superx ui mainline &lt;repo&gt; &lt;ref&gt;</code>{' '}
            names the branch a repository&apos;s work lands on.
          </Text>
        )}
      </Panel>
    </>
  )
}
