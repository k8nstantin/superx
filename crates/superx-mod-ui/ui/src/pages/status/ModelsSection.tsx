import { SimpleGrid, Stack, Text, Title } from '@mantine/core'
import { AXIS, CHART_COLORS, EChart, FAIL, GRID_LINE, INK, INK_MUTED, MONO, OK, TOOLTIP } from '../../EChart'
import { Panel, fmtCompact, n } from './parts'
import type { ModelRow } from '../../generated/ModelRow'
import type { TasksSummary } from '../../generated/TasksSummary'

// The Models view (#436, epic #433): which model to reach for.
//
// Every number is a cut of the join (/api/tasks): each operator turn a
// task, each commit matched to the task that made it, each line followed
// to its fate. Three charts, the ones the operator picked (2026-10-11): one
// picture of cost against what lasted, thick bars in one fixed order, and
// the price of a line before and after rework. A scorecard table was tried
// and read as "very confusing".
//
// Only TESTED lines are judged: work whose repositories have since seen the
// tested threshold of later work. New code has had no chance to be
// replaced, so judging it made the newest model look best. Untested work is
// counted, never rated.

/// The row with every i64 as a plain number: ts-rs maps i64 to bigint,
/// which cannot be added to a number or fed to a chart.
type Row = { [K in keyof ModelRow]: ModelRow[K] extends bigint ? number : ModelRow[K] }

function toRow(d: ModelRow): Row {
  const out = {} as Record<string, unknown>
  for (const [k, val] of Object.entries(d)) {
    out[k] = typeof val === 'bigint' ? Number(val) : val
  }
  return out as Row
}

/// A rate is only as good as the work behind it (operator, #415 QA): one
/// file can score what two hundred cannot. A model is rated on this many
/// judged lines or more; the rest are named, not drawn.
const MIN_JUDGED = 1000
/// A model's colour: the validated ramp less the red and the green that
/// mean thrown away and best on this very page (#415 QA).
const IDENT = [CHART_COLORS[0], CHART_COLORS[5], CHART_COLORS[2], CHART_COLORS[3], CHART_COLORS[6]]
const BRIGHT = '#F1EAF7'
const NAME = { color: INK, fontSize: 13 }

const judged = (r: Row) => r.standing + r.replaced + r.never_landed
const fmt = (v: number) => Math.round(v).toLocaleString('en-US')
const hours = (minutes: number) => {
  const h = minutes / 60
  return h < 10 ? `${h.toFixed(1).replace(/\.0$/, '')} h` : `${fmt(h)} h`
}

/// One picture for choosing a model: what a line still standing cost,
/// against how much of what it wrote still stands. Top left is the model
/// to reach for. Each label says how much later work its code has seen,
/// so a model judged on young code is read as such.
function valueMap(rows: Row[], colour: string[]) {
  const cost = (r: Row) => Math.max(1, r.tokens_per_standing)
  const logs = rows.map((r) => Math.log10(cost(r)))
  const [lo, hi] = [Math.min(...logs), Math.max(...logs)]
  return {
    tooltip: {
      ...TOOLTIP,
      trigger: 'item',
      formatter: (p: { seriesIndex: number }) => {
        const r = rows[p.seriesIndex]
        return r
          ? `<b>${r.model}</b><br/>${r.standing_pct}% of ${fmt(judged(r))} judged lines still standing<br/>` +
              `${fmt(r.tokens_per_standing)} output tokens per line still standing<br/>` +
              `its code has seen ${hours(r.median_exposure_minutes)} of later work (median)`
          : ''
      },
    },
    grid: { left: 64, right: 48, top: 30, bottom: 58 },
    graphic: [{ type: 'text', left: 76, top: 38, style: { text: 'better ↖', fill: OK, font: `600 13px ${MONO}` } }],
    xAxis: {
      ...AXIS,
      type: 'log',
      name: 'output tokens per line still standing  (cheaper ←)',
      nameLocation: 'middle',
      nameGap: 34,
      nameTextStyle: { color: INK_MUTED, fontSize: 12 },
      axisLabel: { color: INK_MUTED, fontSize: 12, formatter: (x: number) => fmtCompact(x) },
      splitLine: { lineStyle: { color: GRID_LINE } },
    },
    yAxis: {
      ...AXIS,
      type: 'value',
      min: 0,
      max: 100,
      name: '% of judged lines still standing',
      nameTextStyle: { color: INK_MUTED, fontSize: 12, align: 'left' },
      axisLabel: { color: INK_MUTED, fontSize: 12, formatter: (v: number) => `${v}%` },
      splitLine: { lineStyle: { color: GRID_LINE } },
    },
    series: rows.map((r, i) => ({
      name: r.model,
      type: 'scatter',
      symbolSize: Math.max(20, Math.min(56, Math.sqrt(judged(r)) / 3)),
      itemStyle: { color: colour[i], opacity: 0.9, borderColor: '#150420', borderWidth: 2 },
      label: {
        show: true,
        // Near the right edge a label on the right would be cut off.
        position: hi > lo && (logs[i] - lo) / (hi - lo) > 0.6 ? 'left' : 'right',
        distance: 10,
        formatter: () => `{n|${r.model}}\n{a|${hours(r.median_exposure_minutes)} later work}`,
        rich: {
          n: { color: BRIGHT, fontSize: 13, fontWeight: 600, lineHeight: 18 },
          a: { color: INK_MUTED, fontSize: 11, fontFamily: MONO },
        },
      },
      data: [[cost(r), r.standing_pct]],
    })),
  }
}

type Bar = { title: string; good: 'high' | 'low'; pick: (r: Row) => number; unit?: string }

/// The measures whose job is an order. The value map already carries the
/// share standing; these add what it cannot show.
const BARS: Bar[] = [
  { title: 'Lines still standing per 1M tokens', good: 'high', pick: (r) => r.standing_per_mtok },
  { title: 'Lines still standing per working hour', good: 'high', pick: (r) => r.standing_per_hour },
  { title: 'Never landed, % of judged lines', good: 'low', pick: (r) => r.never_landed_pct, unit: '%' },
  { title: 'Times you pulled it back, per 100 tasks', good: 'low', pick: (r) => r.corrections_per_100 },
]

/// One measure, thick bars, every model in the same row as on every other
/// chart: re-sorting each chart by its own measure moved a model from row
/// to row and made the panels unreadable (operator, 2026-10-10).
function bars(rows: Row[], b: Bar) {
  const vals = rows.map(b.pick)
  // A negative value is the payload's "nothing to divide by", never a reading.
  const valid = vals.filter((v) => v >= 0)
  const best = valid.length === 0 ? null : b.good === 'high' ? Math.max(...valid) : Math.min(...valid)
  const unit = b.unit ?? ''
  const shown = (i: number) => (vals[i] < 0 ? '—' : `${fmt(vals[i])}${unit}`)
  return {
    tooltip: {
      ...TOOLTIP,
      trigger: 'item',
      formatter: (p: { dataIndex: number }) => {
        const r = rows[p.dataIndex]
        return r ? `${r.model}: ${shown(p.dataIndex)}<br/>on ${fmt(judged(r))} judged lines · ${fmt(r.tasks)} tasks` : ''
      },
    },
    grid: { left: 8, right: 70, top: 4, bottom: 4, containLabel: true },
    xAxis: { type: 'value', min: 0, show: false },
    yAxis: {
      type: 'category',
      inverse: true,
      data: rows.map((r) => r.model),
      axisTick: { show: false },
      axisLine: { lineStyle: { color: GRID_LINE } },
      axisLabel: NAME,
    },
    series: [
      {
        type: 'bar',
        barWidth: 20,
        data: rows.map((_, i) => ({
          value: Math.max(0, vals[i]),
          itemStyle: { color: vals[i] === best ? OK : CHART_COLORS[0], borderRadius: [0, 4, 4, 0] },
          label: { color: vals[i] === best ? OK : BRIGHT },
        })),
        label: {
          show: true,
          position: 'right',
          fontSize: 14,
          fontWeight: 600,
          fontFamily: MONO,
          formatter: (p: { dataIndex: number }) => shown(p.dataIndex),
        },
      },
    ],
  }
}

/// What a line cost, and the rework tax: tokens per judged line, then the
/// extra the lines later replaced or never landed add, so the whole bar is
/// the price of a line still standing.
function cost(rows: Row[]) {
  const line = rows.map((r) => Math.max(0, r.tokens_per_line))
  const kept = rows.map((r) => r.tokens_per_standing)
  const tax = rows.map((r, i) => (kept[i] >= 0 ? Math.max(0, kept[i] - line[i]) : 0))
  return {
    tooltip: {
      ...TOOLTIP,
      trigger: 'axis',
      axisPointer: { type: 'shadow' },
      formatter: (ps: { dataIndex: number }[]) => {
        const i = ps[0]?.dataIndex ?? 0
        const r = rows[i]
        return r
          ? `<b>${r.model}</b><br/>per line written: ${fmt(line[i])}<br/>rework tax: ${fmt(tax[i])}<br/>per line still standing: ${kept[i] < 0 ? '—' : fmt(kept[i])}`
          : ''
      },
    },
    legend: {
      data: ['per line written', 'rework tax'],
      top: 0,
      right: 0,
      textStyle: { color: INK, fontSize: 12 },
      itemWidth: 14,
      itemHeight: 14,
    },
    grid: { left: 8, right: 140, top: 32, bottom: 40, containLabel: true },
    xAxis: {
      ...AXIS,
      type: 'value',
      name: 'output tokens per line',
      nameLocation: 'middle',
      nameGap: 26,
      nameTextStyle: { color: INK_MUTED, fontSize: 12 },
      axisLabel: { color: INK_MUTED, fontSize: 12, formatter: (x: number) => fmtCompact(x) },
      splitLine: { lineStyle: { color: GRID_LINE } },
    },
    yAxis: {
      type: 'category',
      inverse: true,
      data: rows.map((r) => r.model),
      axisTick: { show: false },
      axisLine: { lineStyle: { color: GRID_LINE } },
      axisLabel: NAME,
    },
    series: [
      { name: 'per line written', type: 'bar', stack: 'cost', barWidth: 22, itemStyle: { color: CHART_COLORS[0] }, data: line },
      {
        name: 'rework tax',
        type: 'bar',
        stack: 'cost',
        barWidth: 22,
        itemStyle: { color: FAIL, borderRadius: [0, 4, 4, 0] },
        data: tax,
        label: {
          show: true,
          position: 'right',
          color: BRIGHT,
          fontSize: 13,
          fontFamily: MONO,
          formatter: (p: { dataIndex: number }) => {
            const i = p.dataIndex
            if (kept[i] < 0) return 'nothing standing'
            return line[i] > 0 ? `${fmt(kept[i])}  (${(kept[i] / line[i]).toFixed(1)}×)` : fmt(kept[i])
          },
        },
      },
    ],
  }
}

/// The Models view. `t` is `/api/tasks`; a failed join comes back as
/// `{ error }`.
export function ModelsSection({ t }: { t: TasksSummary | { error: string } | undefined }) {
  if (!t) {
    return (
      <Panel title="Models" scope="all" range={null}>
        <Text size="xs" c="dimmed">reading every turn and every commit — git says what landed, blame says what is left.</Text>
      </Panel>
    )
  }
  if ('error' in t) {
    return (
      <Panel title="Models" scope="all" range={null}>
        <Text size="sm" c="red">the join could not be read: {t.error}</Text>
      </Panel>
    )
  }
  const all = (t.models ?? []).map(toRow)
  const rated = all.filter((r) => judged(r) >= MIN_JUDGED)
  const thin = all.filter((r) => judged(r) < MIN_JUDGED && (r.tasks > 0 || r.untested_written > 0))
  const untested = all.filter((r) => r.untested_written > 0)
  const colour = rated.map((_, i) => IDENT[i % IDENT.length])
  const tested = n(t.tested_minutes)
  const cv = t.coverage
  const commits = n(cv.commits)
  const share = (v: bigint | number) => (commits > 0 ? Math.round((100 * n(v)) / commits) : 0)

  if (rated.length === 0) {
    return (
      <Panel title="Models" scope="all" range={null}>
        <Text size="xs" c="dimmed">
          no model has {fmt(MIN_JUDGED)} judged lines yet — nothing to rate.
        </Text>
      </Panel>
    )
  }
  const barHeight = rated.length * 34 + 12

  return (
    <>
      <Panel
        title="Which model to reach for"
        scope="all"
        range={null}
        note={`lines judged once their repository has seen ${hours(tested)} of later work · bubble is lines judged`}
        mb="md"
      >
        <EChart option={valueMap(rated, colour)} height={400} />
      </Panel>

      <Panel title="The same models, the same order" scope="all" range={null} note="the best is green" mb="md">
        <SimpleGrid cols={{ base: 1, md: 2 }} spacing="lg" verticalSpacing="md">
          {BARS.map((b) => (
            <Stack key={b.title} gap={2}>
              <Title order={6}>{b.title}</Title>
              <Text size="xs" c="dimmed">
                {b.good === 'high' ? 'higher is better →' : '← lower is better'}
              </Text>
              <EChart option={bars(rated, b)} height={barHeight} />
            </Stack>
          ))}
        </SimpleGrid>
      </Panel>

      <Panel
        title="What a line cost, and the rework tax"
        scope="all"
        range={null}
        note="the red part is what the lines later replaced or never landed added"
        mb="md"
      >
        <EChart option={cost(rated)} height={rated.length * 44 + 80} />
      </Panel>

      <Stack gap={4}>
        <Text size="xs" c="dimmed">
          Read from {fmt(t.tasks.length)} tasks and {fmt(commits)} commits: {share(cv.exact)}% matched to their task by the hash
          printed when they were made, {fmt(n(cv.files))} by the files written, {fmt(n(cv.time))} by time alone,{' '}
          {fmt(n(cv.uncredited))} left uncredited, and {fmt(n(cv.bulk))} too big to be authored work.
          {cv.unreadable_repos.length > 0 && ` Git could not read: ${cv.unreadable_repos.join(', ')}.`}
        </Text>
        {untested.length > 0 && (
          <Text size="xs" c="dimmed">
            Not yet tested, counted but not judged:{' '}
            {untested.map((r) => `${r.model} ${fmt(r.untested_written)} lines in ${fmt(r.untested_tasks)} tasks`).join(' · ')}.
          </Text>
        )}
        {thin.length > 0 && (
          <Text size="xs" c="dimmed">
            Too little work to rate (under {fmt(MIN_JUDGED)} judged lines):{' '}
            {thin.map((r) => `${r.model} (${fmt(judged(r))} lines, ${fmt(r.tasks)} tasks)`).join(' · ')}.
          </Text>
        )}
      </Stack>
    </>
  )
}
