import { useState } from 'react'
import { SegmentedControl } from '@mantine/core'
import { Board, type Card } from '../Board'
import { EChart, INK, type Clicked } from '../EChart'
import { category, grid, legend, percent, PART_COLOUR, stacked, tip, value, zoom } from './charts'
import { CommitRows, type Behind } from './Rows'
import type { Scoped } from './scope'
import { alivePct, cap, enough, group, inPart, n, neverLanded, outcome, parts, partName, pct, survivedPct, weekLabel, weekOf, type Outcome, type Palette, type Part } from './shared'

// Which model's work reached the main line, and is still there: each
// model's committed lines split into what lives, what was replaced, what
// may still land and what never did; then the same week by week.

export function landedAnswer(s: Scoped): string {
  const by = group(s.commits, (c) => c.model)
  const ranked = enough(by)
  const rows = [...by.entries()].filter(([m]) => ranked.has(m)).map(([m, cs]) => [m, outcome(cs)] as [string, Outcome]).filter(([, o]) => o.committed > 0)
  if (!rows.length) return 'No credited commits in this view.'
  rows.sort((a, b) => pct(b[1].landed, b[1].decided) - pct(a[1].landed, a[1].decided))
  const top = rows.slice(0, 5)
  const alive = [...top].sort((a, b) => alivePct(b[1]) - alivePct(a[1]))
  const tested = top.filter(([, o]) => o.alive + o.replaced > 0).sort((a, b) => survivedPct(b[1]) - survivedPct(a[1]))
  const open = top.filter(([, o]) => o.flight > 0)
  return `Of the lines each committed whose fate is decided, landed: ${top.map(([m, o]) => `${cap(m)} ${pct(o.landed, o.decided)}%`).join(', ')}. Still alive: ${alive
    .map(([m, o]) => `${cap(m)} ${alivePct(o)}%`)
    .join(', ')}.${tested.length ? ` Where the main line changed their files again, still there: ${tested.map(([m, o]) => `${cap(m)} ${survivedPct(o)}%`).join(', ')}.` : ''}${
    open.length ? ` In flight, not yet counted: ${open.map(([m, o]) => `${cap(m)} ${n(o.flight)} lines`).join(', ')}.` : ''
  }`
}

export function Landed({ s, pal, open }: { s: Scoped; pal: Palette; open: (b: Behind) => void }) {
  const by = group(s.commits, (c) => c.model)
  const models = pal.models.filter((m) => by.has(m))
  const outcomes = new Map(models.map((m) => [m, outcome(by.get(m) ?? [])]))
  const weeks = [...new Set(s.commits.map((c) => weekOf(c.at)))].sort()
  const [chosen, setChosen] = useState<string | null>(null)
  const model = chosen && models.includes(chosen) ? chosen : (models[0] ?? '')
  const behindPart = (m: string, p: Part, week?: string) =>
    open({
      title: `${m}: ${partName[p]}${week ? `, week of ${weekLabel(week)}` : ''}`,
      commits: (by.get(m) ?? []).filter((c) => inPart(c, p) && (!week || weekOf(c.at) === week)),
    })
  const shares = (height: number, absolute: boolean) => (
    <EChart
      height={height}
      onClick={(p: Clicked) => p.name && behindPart(p.name, parts[p.seriesIndex ?? 0])}
      option={{
        legend: legend(),
        grid: grid(),
        tooltip: tip({
          formatter: (ps: { name: string; marker: string; seriesName: string; value: number; seriesIndex: number }[]) => {
            const o = outcomes.get(ps[0].name)
            if (!o) return ''
            return `<b>${ps[0].name}</b> · ${n(o.commits)} commits, ${n(o.committed)} lines<br/>${ps
              .map((p) => `${p.marker}${p.seriesName}: ${n(o[parts[p.seriesIndex] as keyof Outcome])} lines (${pct(o[parts[p.seriesIndex] as keyof Outcome], o.committed)}%)`)
              .join('<br/>')}`
          },
        }),
        xAxis: absolute ? value() : percent(),
        yAxis: category([...models].reverse(), { axisLabel: { color: INK } }),
        series: parts.map((p) =>
          stacked(
            partName[p],
            [...models].reverse().map((m) => {
              const o = outcomes.get(m)!
              return absolute ? o[p] : pct(o[p], o.committed)
            }),
            PART_COLOUR[p],
            'o',
            { barMaxWidth: 34, label: { show: !absolute, color: '#fff', fontSize: 11, formatter: (q: { value: number }) => (q.value >= 7 ? `${q.value}%` : '') } },
          ),
        ),
      }}
    />
  )
  const weekly = (height: number) => {
    const mine = (by.get(model) ?? []).filter((c) => c.added > 0)
    const per = group(mine, (c) => weekOf(c.at))
    const shown = weeks.filter((w) => per.has(w))
    return (
      <div>
        <SegmentedControl size="xs" mb={6} value={model} onChange={setChosen} data={models.slice(0, 8)} />
        <EChart
          height={height - 40}
          onClick={(p: Clicked) => p.dataIndex != null && behindPart(model, parts[p.seriesIndex ?? 0], shown[p.dataIndex])}
          option={{
            legend: legend(),
            grid: grid({ bottom: 28 }),
            tooltip: tip(),
            dataZoom: zoom(),
            xAxis: category(shown.map(weekLabel)),
            yAxis: value(),
            series: parts.map((p) => stacked(partName[p], shown.map((w) => outcome(per.get(w) ?? [])[p]), PART_COLOUR[p])),
          }}
        />
      </div>
    )
  }
  const trend = (height: number, part: 'alive' | 'never') => (
    <EChart
      height={height}
      onClick={(p: Clicked) => {
        if (!p.seriesName || p.dataIndex == null) return
        const [m, w] = [p.seriesName, weeks[p.dataIndex]]
        if (part === 'alive') behindPart(m, 'alive', w)
        else open({ title: `${m}: never landed, week of ${weekLabel(w)}`, commits: (by.get(m) ?? []).filter((c) => (inPart(c, 'never') || inPart(c, 'reset')) && weekOf(c.at) === w) })
      }}
      option={{
        legend: legend(),
        grid: grid({ bottom: 28 }),
        tooltip: { ...tip(), axisPointer: { type: 'line' }, valueFormatter: (v: number | null) => (v == null ? '—' : `${v}%`) },
        dataZoom: zoom(),
        xAxis: category(weeks.map(weekLabel)),
        yAxis: percent(),
        series: models.slice(0, 8).map((m) => {
          const per = group(by.get(m) ?? [], (c) => weekOf(c.at))
          return {
            name: m,
            type: 'line',
            connectNulls: true,
            symbolSize: (_: unknown, p: { dataIndex: number }) => {
              const o = outcome(per.get(weeks[p.dataIndex]) ?? [])
              return o.committed ? Math.min(22, 6 + Math.sqrt(o.committed) / 12) : 0
            },
            lineStyle: { width: 2, color: pal.colour(m) },
            itemStyle: { color: pal.colour(m) },
            data: weeks.map((w) => {
              const o = outcome(per.get(w) ?? [])
              return o.decided ? (part === 'never' ? pct(neverLanded(o), o.decided) : alivePct(o)) : null
            }),
          }
        }),
      }}
    />
  )
  const cards: Card[] = [
    { i: 'share', title: 'Of every line each model committed', note: 'click a segment for its commits', w: 6, h: 10, render: (h) => shares(h, false) },
    { i: 'lines', title: 'The same, in lines', note: 'volume as well as share', w: 6, h: 10, render: (h) => shares(h, true) },
    { i: 'weekly', title: `Week by week, the lines ${model} committed`, note: 'pick a model; zoom with the slider; click a bar for its commits', w: 12, h: 11, render: weekly },
    { i: 'alive', title: 'Still alive, by the week the lines were committed', note: 'share of decided lines (in flight left out) blame credits today, of those whose blame was read · dot size = lines committed', w: 6, h: 10, render: (h) => trend(h, 'alive') },
    { i: 'never', title: 'Never landed, by the week the lines were committed', note: 'share of decided lines left on a branch or reset away', w: 6, h: 10, render: (h) => trend(h, 'never') },
    { i: 'commits', title: `Every credited commit in view (${n(s.commits.length)})`, note: 'click a row for the whole commit', w: 12, h: 12, scroll: true, render: () => <CommitRows commits={s.commits} pal={pal} /> },
  ]
  return <Board name="results.landed" cards={cards} />
}
