import { Board, type Card } from '../Board'
import { EChart, type Clicked } from '../EChart'
import { category, grid, legend, stacked, tip, value, zoom } from './charts'
import { CommitRows, type Behind } from './Rows'
import type { Scoped } from './scope'
import { group, inPart, n, neverLanded, outcome, pct, sum, weekLabel, weekOf, type Palette } from './shared'

// Where work was rewritten away: the reflog's rewrites (rebases, amends,
// resets, forced updates from a host) week by week, the committed lines
// that never landed by repository, the commits a later commit reverted,
// and those only the reflog still holds.

const KINDS: [string, string][] = [
  ['rebase', '#B833E8'],
  ['amend', '#3987e5'],
  ['reset', '#c98500'],
  ['forced', '#e66767'],
]

export function rewritesAnswer(s: Scoped): string {
  const byWeek = group(s.repoWeeks, (w) => w.week)
  const totals = [...byWeek.entries()].map(([week, ws]) => ({ week, ws, rewrites: sum(ws, (w) => w.rewrites) })).filter((x) => x.rewrites > 0)
  if (!totals.length) return 'The reflogs hold no rewrites in this view.'
  const top = totals.sort((a, b) => b.rewrites - a.rewrites)[0]
  const kinds = KINDS.map(([k]) => [k, sum(top.ws, (w) => w.moves[k] ?? 0)] as [string, number]).filter(([, v]) => v > 0)
  const o = outcome(s.commits.filter((c) => weekOf(c.at) === top.week))
  return `The most rewriting was the week of ${weekLabel(top.week)}: ${kinds.map(([k, v]) => `${n(v)} ${k}${v === 1 ? '' : 's'}`).join(', ')}${
    o.decided ? `; ${pct(neverLanded(o), o.decided)}% of the ${n(o.decided)} decided lines committed that week never landed` : ''
  }.`
}

export function Rewrites({ s, pal, open }: { s: Scoped; pal: Palette; open: (b: Behind) => void }) {
  const weeks = [...new Set(s.repoWeeks.filter((w) => w.rewrites > 0).map((w) => w.week))].sort()
  const rw = group(s.repoWeeks, (w) => w.week)
  const never = s.commits.filter((c) => inPart(c, 'never') || inPart(c, 'reset'))
  const neverWeeks = [...new Set(never.map((c) => weekOf(c.at)))].sort()
  const repos = [...new Set(never.map((c) => c.repo))].sort()
  const reverted = s.everything.filter((c) => c.reverted)
  const recovered = s.everything.filter((c) => c.place === 'recovered')
  const colours = ['#B833E8', '#3987e5', '#199e70', '#c98500', '#d55181', '#d95926', '#e66767']
  const cards: Card[] = [
    {
      i: 'rewrites',
      title: 'Reflog rewrites per week',
      note: 'rebases, amends, resets, and forced updates fetched from a host',
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
            series: KINDS.map(([k, colour]) => stacked(k, weeks.map((w) => sum(rw.get(w) ?? [], (r) => r.moves[k] ?? 0)), colour)),
          }}
        />
      ),
    },
    {
      i: 'never',
      title: 'Committed lines that never landed, per week',
      note: 'by repository · in-flight work left out · click a bar for its commits',
      w: 6,
      h: 10,
      render: (h) => (
        <EChart
          height={h}
          onClick={(p: Clicked) => {
            const w = p.dataIndex != null ? neverWeeks[p.dataIndex] : undefined
            if (w) open({ title: `Never landed, ${p.seriesName}, week of ${weekLabel(w)}`, commits: never.filter((c) => c.repo === p.seriesName && weekOf(c.at) === w) })
          }}
          option={{
            legend: legend(),
            grid: grid({ bottom: 28 }),
            tooltip: tip(),
            dataZoom: zoom(),
            xAxis: category(neverWeeks.map(weekLabel)),
            yAxis: value(),
            series: repos.map((r, i) => stacked(r, neverWeeks.map((w) => sum(never.filter((c) => c.repo === r && weekOf(c.at) === w), (c) => c.never_landed)), colours[i % colours.length])),
          }}
        />
      ),
    },
    { i: 'reverted', title: `Commits a later commit reverted (${n(reverted.length)})`, w: 12, h: 9, scroll: true, render: () => <CommitRows commits={reverted} pal={pal} /> },
    { i: 'recovered', title: `Commits only the reflog still holds (${n(recovered.length)})`, note: 'reachable from no branch: rewritten or thrown away', w: 12, h: 9, scroll: true, render: () => <CommitRows commits={recovered} pal={pal} /> },
  ]
  return <Board name="results.rewrites" cards={cards} />
}
