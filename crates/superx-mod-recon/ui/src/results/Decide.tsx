import { useState } from 'react'
import { Anchor, Group, List, Paper, Select, Stack, Table, Text, Title } from '@mantine/core'
import { Board, type Card } from '../Board'
import { EChart, type Clicked } from '../EChart'
import type { FoundCommit } from '../api'
import { category, grid, grouped, legend, percent, tip } from './charts'
import { ciAnswer } from './Ci'
import { costAnswer } from './Cost'
import { directionAnswer } from './Direction'
import { landedAnswer } from './Landed'
import { practicesAnswer } from './Practices'
import { rewritesAnswer } from './Rewrites'
import { ModelBadge, type Behind } from './Rows'
import type { Scoped } from './scope'
import { cap, enough as enoughOf, group, kTok, n, outcome, pct, quantile, sum, weekLabel, weekOf, type Palette } from './shared'

// What to decide from it all: each model's scorecard on what its work
// became and what it cost, two models set against each other only where
// both worked (the same repository in the same week, so neither is judged
// on older code), and every view's answer in one list.

type Row = {
  model: string
  commits: number
  committed: number
  landed: number
  alive: number
  never: number
  flight: number
  testedShare: number | null
  tokensPerAlive: number | null
  alivePerHour: number | null
  corrections: number | null
  escalations: number | null
  pulls: number
  firstRed: number | null
  first: string
  last: string
}

function scorecard(s: Scoped, pal: Palette): Row[] {
  const commits = group(s.commits, (c) => c.model)
  const weeks = group(s.weeks.filter((w) => w.model !== ''), (w) => w.model)
  const pulls = group(s.pulls.filter((p) => p.model), (p) => p.model as string)
  return pal.models
    .filter((m) => commits.has(m))
    .map((m) => {
      const cs = commits.get(m) ?? []
      const o = outcome(cs)
      const ws = weeks.get(m) ?? []
      const printed = cs.filter((c) => c.by === 'printed' && c.tested != null)
      const prompts = sum(ws, (w) => w.prompts)
      const tokens = sum(ws, (w) => w.out_tokens)
      const hours = sum(ws, (w) => w.active_secs) / 3600
      const ps = pulls.get(m) ?? []
      const judgedPs = ps.filter((p) => p.ci_first != null)
      const at = cs.map((c) => weekOf(c.at)).sort()
      return {
        model: m,
        commits: o.commits,
        committed: o.committed,
        landed: pct(o.landed, o.decided),
        alive: pct(o.alive, o.decided),
        never: pct(o.never, o.decided),
        flight: o.flight,
        testedShare: printed.length ? pct(printed.filter((c) => c.tested).length, printed.length) : null,
        tokensPerAlive: o.alive ? tokens / o.alive : null,
        alivePerHour: hours ? o.alive / hours : null,
        corrections: prompts ? (100 * sum(ws, (w) => w.corrections)) / prompts : null,
        escalations: prompts ? (100 * sum(ws, (w) => w.escalations)) / prompts : null,
        pulls: ps.length,
        firstRed: judgedPs.length ? pct(judgedPs.filter((p) => p.ci_first === false).length, judgedPs.length) : null,
        first: at[0] ?? '',
        last: at[at.length - 1] ?? '',
      }
    })
}

type Col = { h: string; v: (r: Row) => number | null; show: (r: Row) => string; better: 'up' | 'down' | null }
const COLS: Col[] = [
  { h: 'Commits', v: (r) => r.commits, show: (r) => n(r.commits), better: null },
  { h: 'Lines committed', v: (r) => r.committed, show: (r) => n(r.committed), better: null },
  { h: 'Landed', v: (r) => r.landed, show: (r) => `${r.landed}%`, better: 'up' },
  { h: 'Still alive', v: (r) => r.alive, show: (r) => `${r.alive}%`, better: 'up' },
  { h: 'Never landed', v: (r) => r.never, show: (r) => `${r.never}%`, better: 'down' },
  { h: 'In flight', v: (r) => r.flight, show: (r) => n(r.flight), better: null },
  { h: 'Tested first', v: (r) => r.testedShare, show: (r) => (r.testedShare == null ? '—' : `${r.testedShare}%`), better: 'up' },
  { h: 'Tokens / alive line', v: (r) => r.tokensPerAlive, show: (r) => (r.tokensPerAlive == null ? '—' : n(r.tokensPerAlive)), better: 'down' },
  { h: 'Alive lines / hour', v: (r) => r.alivePerHour, show: (r) => (r.alivePerHour == null ? '—' : n(r.alivePerHour)), better: 'up' },
  { h: 'Corrections / 100', v: (r) => r.corrections, show: (r) => (r.corrections == null ? '—' : r.corrections.toFixed(1)), better: 'down' },
  { h: 'Escalations / 100', v: (r) => r.escalations, show: (r) => (r.escalations == null ? '—' : r.escalations.toFixed(1)), better: 'down' },
  { h: 'Pull requests', v: (r) => r.pulls, show: (r) => n(r.pulls), better: null },
  { h: 'First CI red', v: (r) => r.firstRed, show: (r) => (r.firstRed == null ? '—' : `${r.firstRed}%`), better: 'down' },
]

/// A model with as many commits as the middle model in view: enough to be
/// named best or worst in a column.
function enough(rows: Row[]): (r: Row) => boolean {
  const counts = rows.map((r) => r.commits).sort((a, b) => a - b)
  const middle = quantile(counts, 0.5)
  return (r) => r.commits >= middle
}

/// The cells two models share: the same repository in the same week.
function shared(s: Scoped, a: string, b: string): { a: FoundCommit[]; b: FoundCommit[]; cells: string[] } {
  const cell = (c: FoundCommit) => `${weekOf(c.at)}|${c.repo}`
  const ca = group(s.commits.filter((c) => c.model === a), cell)
  const cb = group(s.commits.filter((c) => c.model === b), cell)
  const cells = [...ca.keys()].filter((k) => cb.has(k)).sort()
  return { a: cells.flatMap((k) => ca.get(k) ?? []), b: cells.flatMap((k) => cb.get(k) ?? []), cells }
}

export function headToHead(s: Scoped, a: string, b: string): string {
  const sh = shared(s, a, b)
  if (!sh.cells.length) return `${cap(a)} and ${b} never committed in the same repository in the same week here, so there is no fair comparison yet.`
  const oa = outcome(sh.a)
  const ob = outcome(sh.b)
  const cells = sh.cells.length === 1 ? 'one repository-week' : `${sh.cells.length} repository-weeks`
  return `Where both worked (${cells}): ${a} landed ${pct(oa.landed, oa.decided)}% and kept ${pct(oa.alive, oa.decided)}% of ${n(oa.decided)} decided lines; ${b} landed ${pct(ob.landed, ob.decided)}% and kept ${pct(ob.alive, ob.decided)}% of ${n(ob.decided)}.`
}

function effortAnswer(s: Scoped): string | null {
  const printed = s.commits.filter((c) => c.by === 'printed' && c.effort)
  const byModel = group(printed, (c) => c.model)
  const ranked = enoughOf(byModel)
  const best = [...byModel.entries()]
    .filter(([m]) => ranked.has(m))
    .map(([m, cs]) => {
      const levels = [...group(cs, (c) => c.effort as string).entries()].map(([e, xs]) => ({ e, o: outcome(xs) })).filter((x) => x.o.decided > 0)
      if (levels.length < 2) return null
      levels.sort((x, y) => pct(y.o.alive, y.o.decided) - pct(x.o.alive, x.o.decided))
      const hi = levels[0]
      const lo = levels[levels.length - 1]
      return { m, hi, lo, gap: pct(hi.o.alive, hi.o.decided) - pct(lo.o.alive, lo.o.decided) }
    })
    .filter((x): x is NonNullable<typeof x> => x != null)
    .sort((x, y) => y.gap - x.gap)[0]
  if (!best) return null
  return `${cap(best.m)} at ${best.hi.e} effort kept ${pct(best.hi.o.alive, best.hi.o.decided)}% alive (${best.hi.o.commits} commits); at ${best.lo.e}, ${pct(best.lo.o.alive, best.lo.o.decided)}% (${best.lo.o.commits}). The two efforts ran in different weeks, so this is what happened, not yet a cause.`
}

function contextAnswer(s: Scoped): string | null {
  const printed = s.commits.filter((c) => c.by === 'printed' && c.context != null)
  if (printed.length < 10) return null
  const ctx = printed.map((c) => c.context as number).sort((a, b) => a - b)
  const edges = [0, 0.2, 0.4, 0.6, 0.8, 1].map((q) => quantile(ctx, q))
  const bins = edges.slice(0, 5).map((lo, i) => {
    const hi = edges[i + 1]
    const o = outcome(printed.filter((c) => (c.context as number) >= lo && (i === 4 ? (c.context as number) <= hi : (c.context as number) < hi)))
    return { lo, hi, o, never: pct(o.never, o.decided) }
  })
  const worst = [...bins].sort((a, b) => b.never - a.never)[0]
  const best = [...bins].sort((a, b) => a.never - b.never)[0]
  return `Commits made with ${kTok(worst.lo)}–${kTok(worst.hi)} tokens of context never landed ${worst.never}% of their lines; with ${kTok(best.lo)}–${kTok(best.hi)}, ${best.never}%.`
}

export function Decide({ s, pal, open, go }: { s: Scoped; pal: Palette; open: (b: Behind) => void; go: (view: string) => void }) {
  const rows = scorecard(s, pal)
  const fair = enough(rows)
  const mark = (c: Col, r: Row): string | undefined => {
    if (!c.better || !fair(r) || c.v(r) == null) return undefined
    const vals = rows.filter(fair).map((x) => c.v(x)).filter((v): v is number => v != null)
    if (vals.length < 2) return undefined
    const best = c.better === 'up' ? Math.max(...vals) : Math.min(...vals)
    const worst = c.better === 'up' ? Math.min(...vals) : Math.max(...vals)
    return c.v(r) === best ? 'teal.4' : c.v(r) === worst ? 'red.4' : undefined
  }
  const [a, setA] = useState<string | null>(null)
  const [b, setB] = useState<string | null>(null)
  const ma = a ?? rows[0]?.model ?? ''
  const mb = b ?? rows.find((r) => r.model !== ma)?.model ?? ''
  const sh = shared(s, ma, mb)
  const evidence: { text: string | null; view: string }[] = [
    { text: landedAnswer(s), view: 'landed' },
    { text: headToHead(s, ma, mb), view: 'decide' },
    { text: ciAnswer(s), view: 'ci' },
    { text: practicesAnswer(s, pal), view: 'practices' },
    { text: effortAnswer(s), view: 'practices' },
    { text: contextAnswer(s), view: 'practices' },
    { text: directionAnswer(s), view: 'direction' },
    { text: costAnswer(s, pal), view: 'cost' },
    { text: rewritesAnswer(s), view: 'rewrites' },
  ]
  const cards: Card[] = [
    {
      i: 'scorecard',
      title: 'The scorecard',
      note: 'green: best, red: worst, among the models with at least the middle model\'s commits · click a row for its commits',
      w: 12,
      h: 9,
      scroll: true,
      render: () => (
        <Table.ScrollContainer minWidth={1200}>
          <Table fz="xs" verticalSpacing={5} highlightOnHover stickyHeader>
            <Table.Thead>
              <Table.Tr>
                <Table.Th fz="xs">Model</Table.Th>
                {COLS.map((c) => <Table.Th key={c.h} fz="xs" ta="right">{c.h}</Table.Th>)}
                <Table.Th fz="xs">Weeks</Table.Th>
              </Table.Tr>
            </Table.Thead>
            <Table.Tbody>
              {rows.map((r) => (
                <Table.Tr key={r.model} style={{ cursor: 'pointer' }} onClick={() => open({ title: `${r.model}: every credited commit`, commits: s.commits.filter((c) => c.model === r.model) })}>
                  <Table.Td><ModelBadge model={r.model} pal={pal} /></Table.Td>
                  {COLS.map((c) => (
                    <Table.Td key={c.h} ta="right" fz="sm" fw={mark(c, r) ? 700 : 400} c={mark(c, r)} style={{ fontVariantNumeric: 'tabular-nums', opacity: fair(r) ? 1 : 0.65 }}>
                      {c.show(r)}
                    </Table.Td>
                  ))}
                  <Table.Td fz="xs" c="dimmed" style={{ whiteSpace: 'nowrap' }}>{r.first ? `${weekLabel(r.first)} – ${weekLabel(r.last)}` : '—'}</Table.Td>
                </Table.Tr>
              ))}
            </Table.Tbody>
          </Table>
        </Table.ScrollContainer>
      ),
    },
    {
      i: 'h2h',
      title: 'Head to head, only where both worked',
      note: 'the same repository in the same week, so neither is judged on older code · click a bar for its commits',
      w: 7,
      h: 12,
      render: (h) => {
        const cellsA = group(sh.a, (c) => `${weekOf(c.at)}|${c.repo}`)
        const cellsB = group(sh.b, (c) => `${weekOf(c.at)}|${c.repo}`)
        const label = (k: string) => {
          const [w, r] = k.split('|')
          return `${weekLabel(w)}\n${r}`
        }
        const rate = (cs: FoundCommit[] | undefined, part: 'landed' | 'alive') => {
          const o = outcome(cs ?? [])
          return pct(o[part], o.decided)
        }
        return (
          <Stack gap={6}>
            <Group gap="xs">
              <Select size="xs" w={170} value={ma} onChange={setA} data={rows.map((r) => r.model)} allowDeselect={false} />
              <Text fz="xs" c="dimmed">against</Text>
              <Select size="xs" w={170} value={mb} onChange={setB} data={rows.map((r) => r.model)} allowDeselect={false} />
            </Group>
            <Text fz="sm">{headToHead(s, ma, mb)}</Text>
            {sh.cells.length > 0 && (
              <EChart
                height={h - 90}
                onClick={(p: Clicked) => {
                  if (p.dataIndex == null) return
                  const k = sh.cells[p.dataIndex]
                  const m = (p.seriesName ?? '').startsWith(ma) ? ma : mb
                  open({ title: `${m}, ${label(k).replace('\n', ' in ')}`, commits: (m === ma ? cellsA : cellsB).get(k) ?? [] })
                }}
                option={{
                  legend: legend(),
                  grid: grid(),
                  tooltip: { ...tip(), valueFormatter: (v: number) => `${v}%` },
                  xAxis: category(sh.cells.map(label), { axisLabel: { color: '#CFC2DB', fontSize: 10, interval: 0 } }),
                  yAxis: percent(),
                  series: [
                    grouped(`${ma} landed`, sh.cells.map((k) => rate(cellsA.get(k), 'landed')), pal.colour(ma), { itemStyle: { color: pal.colour(ma), opacity: 0.55, borderRadius: [3, 3, 0, 0] } }),
                    grouped(`${ma} alive`, sh.cells.map((k) => rate(cellsA.get(k), 'alive')), pal.colour(ma)),
                    grouped(`${mb} landed`, sh.cells.map((k) => rate(cellsB.get(k), 'landed')), pal.colour(mb), { itemStyle: { color: pal.colour(mb), opacity: 0.55, borderRadius: [3, 3, 0, 0] } }),
                    grouped(`${mb} alive`, sh.cells.map((k) => rate(cellsB.get(k), 'alive')), pal.colour(mb)),
                  ],
                }}
              />
            )}
          </Stack>
        )
      },
    },
    {
      i: 'evidence',
      title: 'What the evidence says',
      note: 'each computed from this view\'s facts · open the view for the rows',
      w: 5,
      h: 12,
      scroll: true,
      render: () => (
        <List spacing="sm" size="sm">
          {evidence
            .filter((e) => e.text)
            .map((e, i) => (
              <List.Item key={i}>
                <Text fz="sm">{e.text}</Text>
                {e.view !== 'decide' && <Anchor fz="xs" onClick={() => go(e.view)}>open {e.view}</Anchor>}
              </List.Item>
            ))}
        </List>
      ),
    },
  ]
  return (
    <Stack gap="sm">
      <Board name="results.decide" cards={cards} />
      {rows.length === 0 && (
        <Paper withBorder p="md"><Title order={5}>No credited commits in this view</Title></Paper>
      )}
    </Stack>
  )
}
