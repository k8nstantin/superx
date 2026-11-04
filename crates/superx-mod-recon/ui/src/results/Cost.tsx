import { Board, type Card } from '../Board'
import { EChart, INK } from '../EChart'
import { category, grid, legend, stacked, tip, value, zoom } from './charts'
import type { Scoped } from './scope'
import { cap, enough, group, n, outcome, stillAlive, sum, weekLabel, type Palette } from './shared'

// What a surviving line costs: each model's output tokens and active hours,
// in the repositories in view, per line that landed and per line still
// alive; and the spend week by week. Active hours are the gaps between a
// session's replies of one model, each capped at attr_recon_active_secs.

type Spend = { model: string; tokens: number; hours: number; thinking: number; landed: number; alive: number }

function spend(s: Scoped, pal: Palette): Spend[] {
  const w = group(s.weeks.filter((x) => x.model !== ''), (x) => x.model)
  const c = group(s.commits, (x) => x.model)
  return pal.models
    .filter((m) => w.has(m))
    .map((m) => {
      const o = outcome(c.get(m) ?? [])
      const ws = w.get(m) ?? []
      return { model: m, tokens: sum(ws, (x) => x.out_tokens), hours: sum(ws, (x) => x.active_secs) / 3600, thinking: sum(ws, (x) => x.thinking_ms) / 3.6e6, landed: o.landed, alive: stillAlive(o) }
    })
}

export function costAnswer(s: Scoped, pal: Palette): string {
  const ranked = enough(group(s.commits, (c) => c.model))
  const rows = spend(s, pal).filter((r) => r.alive > 0 && ranked.has(r.model))
  if (!rows.length) return 'No credited line is alive in this view yet.'
  rows.sort((a, b) => a.tokens / a.alive - b.tokens / b.alive)
  return `A line still alive cost ${rows
    .slice(0, 5)
    .map((r) => `${cap(r.model)} ${n(r.tokens / r.alive)}`)
    .join(', ')} output tokens.`
}

export function Cost({ s, pal }: { s: Scoped; pal: Palette }) {
  const rows = spend(s, pal)
  const per = (f: (r: Spend) => number | null, label: string) => (height: number) => {
    const shown = rows.filter((r) => f(r) != null)
    return (
      <EChart
        height={height}
        option={{
          grid: grid({ top: 20 }),
          tooltip: tip({ valueFormatter: (v: number) => `${n(v)} ${label}` }),
          xAxis: category(shown.map((r) => r.model)),
          yAxis: value(),
          series: [
            {
              type: 'bar',
              barMaxWidth: 44,
              data: shown.map((r) => ({ value: Math.round(f(r) as number), itemStyle: { color: pal.colour(r.model), borderRadius: [4, 4, 0, 0] } })),
              label: { show: true, position: 'top', color: INK, fontSize: 11, formatter: (p: { value: number }) => n(p.value) },
            },
          ],
        }}
      />
    )
  }
  const weeks = [...new Set(s.weeks.map((w) => w.week))].sort()
  const byWeekModel = group(s.weeks.filter((w) => w.model !== ''), (w) => `${w.week}|${w.model}`)
  const weekly = (f: (ws: typeof s.weeks) => number) => (height: number) => (
    <EChart
      height={height}
      option={{
        legend: legend(),
        grid: grid({ bottom: 28 }),
        tooltip: tip({ valueFormatter: (v: number) => n(v) }),
        dataZoom: zoom(),
        xAxis: category(weeks.map(weekLabel)),
        yAxis: value(),
        series: rows.slice(0, 8).map((r) => stacked(r.model, weeks.map((w) => Math.round(f(byWeekModel.get(`${w}|${r.model}`) ?? []))), pal.colour(r.model))),
      }}
    />
  )
  const cards: Card[] = [
    { i: 'per-alive', title: 'Output tokens per line still alive', w: 4, h: 9, render: per((r) => (r.alive ? r.tokens / r.alive : null), 'tokens') },
    { i: 'per-landed', title: 'Output tokens per line landed', w: 4, h: 9, render: per((r) => (r.landed ? r.tokens / r.landed : null), 'tokens') },
    { i: 'per-hour', title: 'Lines still alive per active hour', w: 4, h: 9, render: per((r) => (r.hours ? r.alive / r.hours : null), 'lines') },
    { i: 'tokens', title: 'Output tokens per week', w: 6, h: 10, render: weekly((ws) => sum(ws, (w) => w.out_tokens)) },
    { i: 'hours', title: 'Active hours per week', w: 6, h: 10, render: weekly((ws) => sum(ws, (w) => w.active_secs) / 3600) },
    { i: 'thinking', title: 'Thinking hours per week', note: 'only reported by the agents since early October', w: 6, h: 10, render: weekly((ws) => sum(ws, (w) => w.thinking_ms) / 3.6e6) },
    { i: 'context', title: 'Context sent per week (millions of tokens)', note: 'input, cache read and cache write of every reply', w: 6, h: 10, render: weekly((ws) => sum(ws, (w) => w.context_tokens) / 1e6) },
  ]
  return <Board name="results.cost" cards={cards} />
}
