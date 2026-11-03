import { Board, type Card } from '../Board'
import { EChart, FAIL, OK, type Clicked } from '../EChart'
import type { FoundCommit } from '../api'
import { category, grid, grouped, legend, percent, PART_COLOUR, stacked, tip, value, zoom } from './charts'
import type { Behind } from './Rows'
import type { Scoped } from './scope'
import { cap, enough, group, kTok, n, outcome, pct, quantile, sum, weekLabel, weekOf, type Palette } from './shared'

// What the agents did before committing, against what became of the
// commit: tests run since the session's previous commit, the thinking
// effort and the context of the reply that committed, and whether a
// subagent did it. Only a commit its session printed carries these.

type Bucket = { label: string; commits: FoundCommit[] }

/// The models with most printed commits, most first.
function leading(printed: FoundCommit[], pal: Palette): string[] {
  const by = group(printed, (c) => c.model)
  return pal.models.filter((m) => (by.get(m)?.length ?? 0) > 0).sort((a, b) => (by.get(b)?.length ?? 0) - (by.get(a)?.length ?? 0))
}

export function practicesAnswer(s: Scoped, pal: Palette): string {
  const printed = s.commits.filter((c) => c.by === 'printed')
  if (!printed.length) return 'No commit in this view was printed by its session, so what came before it is unknown.'
  const tested = printed.filter((c) => c.tested).length
  const ranked = enough(group(printed, (c) => c.model))
  const effects = leading(printed, pal)
    .filter((m) => ranked.has(m))
    .map((m) => {
      const yes = outcome(printed.filter((c) => c.model === m && c.tested === true))
      const no = outcome(printed.filter((c) => c.model === m && c.tested === false))
      return { m, yes, no }
    })
    .filter((e) => e.yes.commits > 0 && e.no.commits > 0)
  const said = effects
    .map((e) => `${cap(e.m)} ${pct(e.yes.alive, e.yes.decided)}% (${e.yes.commits} commits) against ${pct(e.no.alive, e.no.decided)}% (${e.no.commits})`)
    .join('; ')
  return `${n(tested)} of ${n(printed.length)} printed commits (${pct(tested, printed.length)}%) came after their session ran tests.${
    said ? ` Lines still alive, tested first against not: ${said}.` : ''
  }`
}

export function Practices({ s, pal, open }: { s: Scoped; pal: Palette; open: (b: Behind) => void }) {
  const printed = s.commits.filter((c) => c.by === 'printed')
  const models = leading(printed, pal).slice(0, 5)
  const rates = (buckets: Bucket[], title: string) => (height: number) => (
    <EChart
      height={height}
      onClick={(p: Clicked) => p.dataIndex != null && open({ title: `${title}: ${buckets[p.dataIndex].label.replace('\n', ' ')}`, commits: buckets[p.dataIndex].commits })}
      option={{
        legend: legend(),
        grid: grid({ bottom: 8 }),
        tooltip: {
          ...tip(),
          formatter: (ps: { dataIndex: number; marker: string; seriesName: string; value: number }[]) => {
            const b = buckets[ps[0].dataIndex]
            const o = outcome(b.commits)
            return `<b>${b.label.replace('\n', ' ')}</b> · ${n(o.commits)} commits, ${n(o.decided)} decided lines (${n(o.flight)} in flight)<br/>${ps.map((p) => `${p.marker}${p.seriesName}: ${p.value}%`).join('<br/>')}`
          },
        },
        xAxis: category(buckets.map((b) => b.label), { axisLabel: { color: '#CFC2DB', fontSize: 10, interval: 0 } }),
        yAxis: percent(),
        series: [
          grouped('landed', buckets.map((b) => { const o = outcome(b.commits); return pct(o.landed, o.decided) }), '#3987e5'),
          grouped('still alive', buckets.map((b) => { const o = outcome(b.commits); return pct(o.alive, o.decided) }), PART_COLOUR.alive),
          grouped('never landed', buckets.map((b) => { const o = outcome(b.commits); return pct(o.never, o.decided) }), PART_COLOUR.never),
        ],
      }}
    />
  )
  const split = (key: (c: FoundCommit) => string | null, values: string[], label: (v: string) => string): Bucket[] =>
    models.flatMap((m) =>
      values
        .map((v) => ({ label: `${m}\n${label(v)}`, commits: printed.filter((c) => c.model === m && key(c) === v) }))
        .filter((b) => b.commits.length > 0)
        .map((b) => ({ ...b, label: `${b.label} (${b.commits.length})` })),
    )
  const tested = split((c) => (c.tested == null ? null : c.tested ? 'yes' : 'no'), ['yes', 'no'], (v) => (v === 'yes' ? 'tested' : 'untested'))
  const efforts = [...new Set(printed.map((c) => c.effort ?? 'none'))].sort()
  const effort = split((c) => c.effort ?? 'none', efforts, (v) => v)
  const subagent = split((c) => (c.subagent == null ? null : c.subagent ? 'sub' : 'main'), ['main', 'sub'], (v) => (v === 'sub' ? 'subagent' : 'main'))
  // Context in fifths of the printed commits, by the tokens the committing
  // reply was sent: the edges are the data's own.
  const ctx = printed.filter((c) => c.context != null).map((c) => c.context as number).sort((a, b) => a - b)
  const edges = [0, 0.2, 0.4, 0.6, 0.8, 1].map((q) => quantile(ctx, q))
  const context: Bucket[] = edges.slice(0, 5).map((lo, i) => {
    const hi = edges[i + 1]
    const commits = printed.filter((c) => c.context != null && c.context >= lo && (i === 4 ? c.context <= hi : c.context < hi))
    return { label: `${kTok(lo)}–${kTok(hi)}\n(${commits.length})`, commits }
  })
  const weeks = [...new Set(s.weeks.map((w) => w.week))].sort()
  const wk = group(s.weeks, (w) => w.week)
  const testedWeeks = [...new Set(printed.map((c) => weekOf(c.at)))].sort()
  const cards: Card[] = [
    { i: 'tested', title: 'Commits made after the session ran tests, and without', note: 'tests run since the session\'s previous commit · shares of decided lines (in flight left out) · click a group for its commits', w: 12, h: 11, render: rates(tested, 'Tested first') },
    { i: 'effort', title: 'By the thinking effort of the reply that committed', w: 6, h: 10, render: rates(effort, 'Effort') },
    { i: 'context', title: 'By the context the committing reply carried', note: 'fifths of the commits by tokens sent', w: 6, h: 10, render: rates(context, 'Context') },
    { i: 'subagent', title: 'Main session or subagent', w: 6, h: 10, render: rates(subagent, 'Who committed') },
    {
      i: 'tested-share',
      title: 'How often each model tested before committing, by week',
      note: 'share of its printed commits made after a test run',
      w: 6,
      h: 10,
      render: (h) => (
        <EChart
          height={h}
          option={{
            legend: legend(),
            grid: grid({ bottom: 28 }),
            tooltip: { ...tip(), axisPointer: { type: 'line' }, valueFormatter: (v: number | null) => (v == null ? '—' : `${v}%`) },
            dataZoom: zoom(),
            xAxis: category(testedWeeks.map(weekLabel)),
            yAxis: percent(),
            series: models.map((m) => ({
              name: m,
              type: 'line',
              connectNulls: true,
              lineStyle: { width: 2, color: pal.colour(m) },
              itemStyle: { color: pal.colour(m) },
              data: testedWeeks.map((w) => {
                const cs = printed.filter((c) => c.model === m && weekOf(c.at) === w && c.tested != null)
                return cs.length ? pct(cs.filter((c) => c.tested).length, cs.length) : null
              }),
            })),
          }}
        />
      ),
    },
    {
      i: 'tests',
      title: 'Test runs the agents made per week',
      note: 'a run is one test command; failing = it reported a failed test',
      w: 6,
      h: 10,
      render: (h) => (
        <EChart
          height={h}
          option={{
            legend: legend(),
            grid: grid({ bottom: 28 }),
            tooltip: tip(),
            dataZoom: zoom(),
            xAxis: category(weeks.map(weekLabel)),
            yAxis: value(),
            series: [
              stacked('all passing', weeks.map((w) => sum(wk.get(w) ?? [], (r) => r.tests - r.tests_failed)), OK),
              stacked('some failing', weeks.map((w) => sum(wk.get(w) ?? [], (r) => r.tests_failed)), FAIL),
            ],
          }}
        />
      ),
    },
    {
      i: 'builds',
      title: 'Builds the agents ran per week',
      w: 6,
      h: 10,
      render: (h) => (
        <EChart
          height={h}
          option={{
            legend: legend(),
            grid: grid({ bottom: 28 }),
            tooltip: tip(),
            dataZoom: zoom(),
            xAxis: category(weeks.map(weekLabel)),
            yAxis: value(),
            series: [
              stacked('built', weeks.map((w) => sum(wk.get(w) ?? [], (r) => r.builds - r.builds_failed)), OK),
              stacked('failed', weeks.map((w) => sum(wk.get(w) ?? [], (r) => r.builds_failed)), FAIL),
            ],
          }}
        />
      ),
    },
  ]
  return <Board name="results.practices" cards={cards} />
}
