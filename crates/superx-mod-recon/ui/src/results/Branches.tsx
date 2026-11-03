import { Table, Text } from '@mantine/core'
import { Board, type Card } from '../Board'
import { CHART_COLORS, EChart, FAIL, INK, OK } from '../EChart'
import type { BranchWeek, Findings } from '../api'
import { category, grid, legend, stacked, tip, value, zoom } from './charts'
import type { Scoped } from './scope'
import { group, n, pct, sum, weekLabel } from './shared'

// Which branches are busy, and what landed from each: every branch's reflog
// week by week (the commits made on it, its merges, its rewrites, its moves
// fetched from or pushed to a host), and of the commits made on it, those
// whose work landed and those that never did. Measured, not weighted.

type Branch = { repo: string; branch: string; commits: number; lines: number; landed: number; never: number; merges: number; rewrites: number; updates: number; first: string; last: string }

const OTHERS = 'every other branch'
const SHOWN = 10 // skill-allow: §9-const — series drawn per chart; the table lists every branch

function totals(rows: BranchWeek[]): Branch[] {
  return [...group(rows, (r) => `${r.repo}|${r.branch}`).values()]
    .map((ws) => {
      const weeks = ws.map((w) => w.week).sort()
      return {
        repo: ws[0].repo,
        branch: ws[0].branch,
        commits: sum(ws, (w) => w.commits),
        lines: sum(ws, (w) => w.lines),
        landed: sum(ws, (w) => w.landed),
        never: sum(ws, (w) => w.never),
        merges: sum(ws, (w) => w.merges),
        rewrites: sum(ws, (w) => w.rewrites),
        updates: sum(ws, (w) => w.updates),
        first: weeks[0],
        last: weeks[weeks.length - 1],
      }
    })
    .sort((a, b) => b.commits + b.updates - (a.commits + a.updates))
}

const label = (b: { repo: string; branch: string }, many: boolean) => (many ? `${b.repo}: ${b.branch}` : b.branch)

export function branchesAnswer(s: Scoped, f: Findings): string {
  const all = totals(s.branches)
  if (!all.length) return 'The reflogs hold no branch activity in this view.'
  const busiest = all[0]
  const main = new Set(f.mainlines.map((m) => `${m.repo}|${m.mainline.replace(/^refs\/remotes\/[^/]+\//, '').replace(/^refs\/heads\//, '')}`))
  const elsewhere = all.filter((b) => !main.has(`${b.repo}|${b.branch.replace(/ \(host\)$/, '').replace(/^[^/]+\//, '')}`))
  const landed = sum(elsewhere, (b) => b.landed)
  const judged = landed + sum(elsewhere, (b) => b.never)
  return `The busiest branch is ${busiest.repo}'s ${busiest.branch}: ${n(busiest.commits)} commits made on it and ${n(busiest.updates)} moves on a host. Of the commits made on branches other than the main line, ${pct(landed, judged)}% landed (${n(landed)} of ${n(judged)} whose fate is decided).`
}

export function Branches({ s, f }: { s: Scoped; f: Findings }) {
  const all = totals(s.branches)
  const many = new Set(all.map((b) => b.repo)).size > 1
  const weeks = [...new Set(s.branches.map((w) => w.week))].sort()
  const top = (pick: (b: Branch) => number) => all.filter((b) => pick(b) > 0).sort((a, b) => pick(b) - pick(a)).slice(0, SHOWN)
  const weekly = (pick: (b: Branch) => number, field: keyof Pick<BranchWeek, 'commits' | 'updates' | 'rewrites' | 'lines'>) => (height: number) => {
    const lead = top(pick)
    const keys = new Set(lead.map((b) => `${b.repo}|${b.branch}`))
    const at = group(s.branches, (w) => `${w.repo}|${w.branch}|${w.week}`)
    const of = (b: { repo: string; branch: string }, w: string) => sum(at.get(`${b.repo}|${b.branch}|${w}`) ?? [], (r) => r[field])
    const rest = weeks.map((w) => sum(s.branches.filter((r) => r.week === w && !keys.has(`${r.repo}|${r.branch}`)), (r) => r[field]))
    return (
      <EChart
        height={height}
        option={{
          legend: legend(),
          grid: grid({ bottom: 28 }),
          tooltip: tip(),
          dataZoom: zoom(),
          xAxis: category(weeks.map(weekLabel)),
          yAxis: value(),
          series: [
            ...lead.map((b, i) => stacked(label(b, many), weeks.map((w) => of(b, w)), CHART_COLORS[i % CHART_COLORS.length])),
            ...(rest.some((v) => v > 0) ? [stacked(OTHERS, rest, '#5C4470')] : []),
          ],
        }}
      />
    )
  }
  const fates = (height: number) => {
    const lead = top((b) => b.landed + b.never).reverse()
    return (
      <EChart
        height={height}
        option={{
          legend: legend(),
          grid: grid(),
          tooltip: tip(),
          xAxis: value(),
          yAxis: category(lead.map((b) => label(b, many)), { axisLabel: { color: INK, fontSize: 10 } }),
          series: [
            stacked('landed', lead.map((b) => b.landed), OK, 'f', { barMaxWidth: 18 }),
            stacked('never landed', lead.map((b) => b.never), FAIL, 'f', { barMaxWidth: 18 }),
          ],
        }}
      />
    )
  }
  const cards: Card[] = [
    { i: 'commits', title: 'Commits made per week, by branch', note: `the ${SHOWN} branches with most, the rest together`, w: 6, h: 10, render: weekly((b) => b.commits, 'commits') },
    { i: 'updates', title: 'Moves on a host per week, by branch', note: 'pushed or fetched: the branch moving where others see it', w: 6, h: 10, render: weekly((b) => b.updates, 'updates') },
    { i: 'fates', title: 'What landed of the commits made on each branch', note: 'commits whose fate is decided; in flight, side folders and snapshots left out', w: 6, h: 11, render: fates },
    { i: 'rewrites', title: 'Rewrites per week, by branch', note: 'resets, rebases, amends and forced updates', w: 6, h: 11, render: weekly((b) => b.rewrites, 'rewrites') },
    {
      i: 'table',
      title: `Every branch (${n(all.length)})`,
      w: 12,
      h: 12,
      scroll: true,
      render: () => (
        <Table.ScrollContainer minWidth={1000}>
          <Table fz="xs" verticalSpacing={4} stickyHeader highlightOnHover>
            <Table.Thead>
              <Table.Tr>{['Repository', 'Branch', 'Commits', 'Lines', 'Landed', 'Never landed', 'Landed share', 'Merges', 'Rewrites', 'Host moves', 'First week', 'Last week'].map((x) => <Table.Th key={x} fz="xs">{x}</Table.Th>)}</Table.Tr>
            </Table.Thead>
            <Table.Tbody>
              {all.map((b) => (
                <Table.Tr key={`${b.repo}|${b.branch}`}>
                  <Table.Td fz="xs">{b.repo}</Table.Td>
                  <Table.Td ff="monospace" fz="xs" style={{ overflowWrap: 'anywhere' }}>{b.branch}</Table.Td>
                  <Table.Td ta="right" fz="xs">{n(b.commits)}</Table.Td>
                  <Table.Td ta="right" fz="xs">{n(b.lines)}</Table.Td>
                  <Table.Td ta="right" fz="xs" c="teal.4">{n(b.landed)}</Table.Td>
                  <Table.Td ta="right" fz="xs" c={b.never ? 'red.4' : undefined}>{n(b.never)}</Table.Td>
                  <Table.Td ta="right" fz="xs">{b.landed + b.never ? `${pct(b.landed, b.landed + b.never)}%` : '—'}</Table.Td>
                  <Table.Td ta="right" fz="xs">{n(b.merges)}</Table.Td>
                  <Table.Td ta="right" fz="xs" c={b.rewrites ? 'yellow.5' : undefined}>{n(b.rewrites)}</Table.Td>
                  <Table.Td ta="right" fz="xs">{n(b.updates)}</Table.Td>
                  <Table.Td fz="xs">{b.first ? weekLabel(b.first) : '—'}</Table.Td>
                  <Table.Td fz="xs">{b.last ? weekLabel(b.last) : '—'}</Table.Td>
                </Table.Tr>
              ))}
            </Table.Tbody>
          </Table>
        </Table.ScrollContainer>
      ),
    },
  ]
  return (
    <>
      {f.branches.length === 0 && <Text c="dimmed">No reflog activity is held.</Text>}
      <Board name="results.branches" cards={cards} />
    </>
  )
}
