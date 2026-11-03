import { Board, type Card } from '../Board'
import { EChart, FAIL, INK, INK_MUTED, type Clicked } from '../EChart'
import { category, grid, grouped, legend, percent, tip, value, zoom } from './charts'
import type { Behind } from './Rows'
import type { Scoped } from './scope'
import { correlation, group, inPart, n, outcome, pct, sum, weekLabel, weekOf, type Palette } from './shared'

// Are you directing better, and does it show in the code: your prompts,
// corrections, escalations and stops week by week and per model, set
// against what that week's commits became. A correction is a prompt with
// one of attr_recon_correction_words, an escalation one with
// attr_recon_escalation_words.

type WeekRow = { week: string; prompts: number; corrections: number; escalations: number; interrupts: number; committed: number; never: number; alive: number }

function weekRows(s: Scoped): WeekRow[] {
  const prompts = group(s.weeks, (w) => w.week)
  const commits = group(s.commits, (c) => weekOf(c.at))
  const weeks = [...new Set([...prompts.keys(), ...commits.keys()])].sort()
  return weeks.map((week) => {
    const ws = prompts.get(week) ?? []
    const o = outcome(commits.get(week) ?? [])
    return {
      week,
      prompts: sum(ws, (w) => w.prompts),
      corrections: sum(ws, (w) => w.corrections),
      escalations: sum(ws, (w) => w.escalations),
      interrupts: sum(ws, (w) => w.interrupts),
      committed: o.committed,
      never: o.never,
      alive: o.alive,
    }
  })
}

/// The weeks with both prompts and committed lines: the points of the
/// scatters and their r.
const paired = (rows: WeekRow[]) => rows.filter((r) => r.prompts > 0 && r.committed > 0)

export function directionAnswer(s: Scoped): string {
  const pts = paired(weekRows(s))
  const rc = correlation(pts.map((r) => (100 * r.corrections) / r.prompts), pts.map((r) => (100 * r.never) / r.committed))
  const re = correlation(pts.map((r) => (100 * r.escalations) / r.prompts), pts.map((r) => (100 * r.alive) / r.committed))
  if (rc == null) return 'Too few weeks with both prompts and commits to set them against each other.'
  const strength = (r: number) => (Math.abs(r) >= 0.7 ? 'strongly' : Math.abs(r) >= 0.4 ? 'moderately' : 'barely')
  return `Week by week, your corrections ${strength(rc)} track the share of committed lines that never landed (r = ${rc.toFixed(2)} over ${pts.length} weeks)${
    re == null ? '' : `; escalations against what stayed alive, r = ${re.toFixed(2)}`
  }. A correlation: thrown-away work may draw the corrections as much as they cause it.`
}

export function Direction({ s, pal, open }: { s: Scoped; pal: Palette; open: (b: Behind) => void }) {
  const rows = weekRows(s)
  const pts = paired(rows)
  const drill = (week: string, what: string) =>
    open({
      title: `Week of ${weekLabel(week)}: ${what}`,
      week: { week, repos: s.repos },
      commits: s.commits.filter((c) => weekOf(c.at) === week && inPart(c, 'never')),
      note: 'Your prompts that week, then its credited commits whose lines never landed.',
    })
  const scatter = (height: number, x: (r: WeekRow) => number, y: (r: WeekRow) => number, xName: string, yName: string, r: number | null) => (
    <EChart
      height={height}
      onClick={(p: Clicked) => p.dataIndex != null && drill(pts[p.dataIndex].week, 'prompts and never-landed commits')}
      option={{
        grid: grid({ bottom: 36, top: 30 }),
        title: { text: r == null ? '' : `r = ${r.toFixed(2)} · ${pts.length} weeks`, right: 8, top: 0, textStyle: { color: INK, fontSize: 12, fontWeight: 500 } },
        tooltip: {
          trigger: 'item',
          backgroundColor: '#150420',
          borderColor: '#3B2449',
          textStyle: { color: INK },
          formatter: (p: { dataIndex: number }) => {
            const w = pts[p.dataIndex]
            return `<b>week of ${weekLabel(w.week)}</b><br/>${n(w.prompts)} prompts · ${n(w.corrections)} corrections · ${n(w.escalations)} escalations · ${n(
              w.interrupts,
            )} stops<br/>${n(w.committed)} lines committed: ${pct(w.never, w.committed)}% never landed, ${pct(w.alive, w.committed)}% still alive`
          },
        },
        xAxis: value((v) => `${v}`, { name: xName, nameLocation: 'middle', nameGap: 26, nameTextStyle: { color: INK_MUTED } }),
        yAxis: percent({ name: yName, nameTextStyle: { color: INK_MUTED } }),
        series: [
          {
            type: 'scatter',
            data: pts.map((w) => [Math.round(x(w) * 10) / 10, y(w)]),
            symbolSize: (_: unknown, p: { dataIndex: number }) => Math.min(40, 8 + Math.sqrt(pts[p.dataIndex].committed) / 10),
            itemStyle: { color: '#CC66FF', borderColor: '#2A1235', borderWidth: 2, opacity: 0.85 },
            label: { show: true, position: 'right', color: INK_MUTED, fontSize: 10, formatter: (p: { dataIndex: number }) => weekLabel(pts[p.dataIndex].week) },
          },
        ],
      }}
    />
  )
  const xs = (k: keyof WeekRow) => pts.map((r) => (100 * (r[k] as number)) / r.prompts)
  const rc = correlation(xs('corrections'), pts.map((r) => (100 * r.never) / r.committed))
  const re = correlation(xs('escalations'), pts.map((r) => (100 * r.alive) / r.committed))
  const byModel = group(s.weeks.filter((w) => w.model !== ''), (w) => w.model)
  const models = pal.models.filter((m) => sum(byModel.get(m) ?? [], (w) => w.prompts) > 0).slice(0, 8)
  const per = (m: string, k: 'corrections' | 'escalations' | 'interrupts') => {
    const ws = byModel.get(m) ?? []
    return Math.round((100 * sum(ws, (w) => w[k])) / Math.max(1, sum(ws, (w) => w.prompts)))
  }
  const cards: Card[] = [
    {
      i: 'corrections',
      title: 'Each week: your corrections against the lines that never landed',
      note: 'x: corrections per 100 prompts · y: share of committed lines never landed · dot size = lines committed · click a week',
      w: 6,
      h: 11,
      render: (h) => scatter(h, (r) => (100 * r.corrections) / r.prompts, (r) => pct(r.never, r.committed), 'corrections per 100 prompts', 'never landed', rc),
    },
    {
      i: 'escalations',
      title: 'Each week: your escalations against the lines still alive',
      note: 'x: escalations per 100 prompts · y: share of committed lines alive today',
      w: 6,
      h: 11,
      render: (h) => scatter(h, (r) => (100 * r.escalations) / r.prompts, (r) => pct(r.alive, r.committed), 'escalations per 100 prompts', 'still alive', re),
    },
    {
      i: 'weekly',
      title: 'Your prompts per week',
      note: 'click a week for its prompts',
      w: 12,
      h: 10,
      render: (h) => {
        const shown = rows.filter((r) => r.prompts > 0)
        return (
          <EChart
            height={h}
            onClick={(p: Clicked) => p.dataIndex != null && drill(shown[p.dataIndex].week, 'prompts')}
            option={{
              legend: legend(),
              grid: grid({ bottom: 28 }),
              tooltip: tip(),
              dataZoom: zoom(),
              xAxis: category(shown.map((r) => weekLabel(r.week))),
              yAxis: value(),
              series: [
                grouped('prompts', shown.map((r) => r.prompts), '#3987e5'),
                grouped('corrections', shown.map((r) => r.corrections), '#c98500'),
                grouped('escalations', shown.map((r) => r.escalations), FAIL),
                grouped('stops', shown.map((r) => r.interrupts), '#d55181'),
              ],
            }}
          />
        )
      },
    },
    {
      i: 'models',
      title: 'Per 100 prompts, by the model answering',
      note: 'the model answering in the session when you typed',
      w: 12,
      h: 10,
      render: (h) => (
        <EChart
          height={h}
          option={{
            legend: legend(),
            grid: grid(),
            tooltip: tip(),
            xAxis: category(models),
            yAxis: value(),
            series: [
              grouped('corrections', models.map((m) => per(m, 'corrections')), '#c98500'),
              grouped('escalations', models.map((m) => per(m, 'escalations')), FAIL),
              grouped('stops', models.map((m) => per(m, 'interrupts')), '#d55181'),
            ],
          }}
        />
      ),
    },
  ]
  return <Board name="results.direction" cards={cards} />
}
