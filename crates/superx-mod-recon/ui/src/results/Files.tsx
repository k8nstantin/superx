import { Group, Table, Text } from '@mantine/core'
import { Board, type Card } from '../Board'
import { EChart, INK, type Clicked } from '../EChart'
import type { Findings, FoundCommit } from '../api'
import { category, grid, legend, PART_COLOUR, percent, stacked, tip, value } from './charts'
import { ModelBadge, type Behind } from './Rows'
import type { Scoped } from './scope'
import { group, local, n, pct, sum, type Palette } from './shared'

// Where the agents' work dies: every file the credited commits changed, by
// the lines that never landed in it. A few files hold most of it, and they
// are the ones to split, specify or keep an agent away from. The main line's
// own changes to each file are beside it: how often it is changed at all.

type File = {
  repo: string
  path: string
  models: string[]
  commits: number[]
  added: number
  landed: number
  never: number
  reset: number
  flight: number
  mainChanges: number
  lastMainChange: string | null
}

const SHOWN = 15 // skill-allow: §9-const — files drawn in the bars; the table lists every file

/// Each file in view, its models' rows summed, most lines never landed first.
function files(s: Scoped): File[] {
  return [...group(s.files, (f) => `${f.repo}\u0000${f.path}`).values()]
    .map((rows) => ({
      repo: rows[0].repo,
      path: rows[0].path,
      models: [...new Set(rows.map((r) => r.model))],
      commits: [...new Set(rows.flatMap((r) => r.commits))],
      added: sum(rows, (r) => r.added),
      landed: sum(rows, (r) => r.landed),
      never: sum(rows, (r) => r.never),
      reset: sum(rows, (r) => r.reset),
      flight: sum(rows, (r) => r.flight),
      mainChanges: Math.max(...rows.map((r) => r.main_changes)),
      lastMainChange: rows.map((r) => r.last_main_change).filter((t): t is string => t != null).sort().pop() ?? null,
    }))
    .sort((a, b) => b.never + b.reset - (a.never + a.reset))
}

const lost = (f: File) => f.never + f.reset
const label = (f: File, many: boolean) => {
  const parts = f.path.split('/')
  const tail = parts.length > 2 ? `…/${parts.slice(-2).join('/')}` : f.path
  return many ? `${f.repo}: ${tail}` : tail
}

/// The fewest files holding half the lines that never landed, as a share of
/// every file in view: how concentrated the thrown-away work is.
function concentration(all: File[]): { files: number; share: number; total: number } {
  const total = sum(all, lost)
  let held = 0
  let k = 0
  while (k < all.length && held * 2 < total) held += lost(all[k++])
  return { files: k, share: pct(k, all.length), total }
}

export function filesAnswer(s: Scoped): string {
  const all = files(s)
  const c = concentration(all)
  if (!c.total) return 'No line the agents committed in this view failed to land.'
  const top = all[0]
  return `Half of the ${n(c.total)} lines that never landed sit in ${n(c.files)} of the ${n(all.length)} files the agents changed (${c.share}%). The most in ${top.path} (${top.repo}): ${n(lost(top))} of ${n(top.added)} lines, ${top.models.length === 1 ? top.models[0] : `${top.models.length} models`}.`
}

export function Files({ s, f, pal, open }: { s: Scoped; f: Findings; pal: Palette; open: (b: Behind) => void }) {
  const all = files(s)
  const many = new Set(all.map((x) => x.repo)).size > 1
  const shown = all.filter((x) => lost(x) > 0).slice(0, SHOWN)
  const behind = (x: File) => open({ title: `${x.repo}: ${x.path}`, note: `${n(lost(x))} of ${n(x.added)} lines never landed`, commits: x.commits.map((i) => f.commits[i]).filter((c): c is FoundCommit => c != null) })
  const bars = (height: number) => (
    <EChart
      height={height}
      onClick={(p: Clicked) => p.dataIndex != null && behind([...shown].reverse()[p.dataIndex])}
      option={{
        legend: legend(),
        grid: grid(),
        tooltip: tip({
          formatter: (ps: { dataIndex: number; marker: string; seriesName: string; value: number }[]) => {
            const x = [...shown].reverse()[ps[0].dataIndex]
            return `<b>${x.repo}: ${x.path}</b><br/>${n(x.commits.length)} commits by ${x.models.join(', ')} · changed on the main line ${n(x.mainChanges)} times<br/>${ps
              .map((p) => `${p.marker}${p.seriesName}: ${n(p.value)} lines`)
              .join('<br/>')}`
          },
        }),
        xAxis: value(),
        yAxis: category([...shown].reverse().map((x) => label(x, many)), { axisLabel: { color: INK, fontSize: 11, fontFamily: 'JetBrains Mono, ui-monospace, monospace' } }),
        series: [
          stacked('never landed, left on a branch', [...shown].reverse().map((x) => x.never), PART_COLOUR.never, 'f', { barMaxWidth: 18 }),
          stacked('never landed, reset away', [...shown].reverse().map((x) => x.reset), PART_COLOUR.reset, 'f', { barMaxWidth: 18 }),
          stacked('in flight', [...shown].reverse().map((x) => x.flight), PART_COLOUR.flight, 'f', { barMaxWidth: 18 }),
          stacked('landed', [...shown].reverse().map((x) => x.landed), PART_COLOUR.replaced, 'f', { barMaxWidth: 18 }),
        ],
      }}
    />
  )
  // How concentrated it is: the files, most never-landed lines first, against
  // the share of every never-landed line they hold between them.
  const curve = (height: number) => {
    const total = sum(all, lost)
    const steps = all.filter((x) => lost(x) > 0)
    let held = 0
    const points = steps.map((x, i) => {
      held += lost(x)
      return [pct(i + 1, all.length), pct(held, total)]
    })
    return (
      <EChart
        height={height}
        option={{
          grid: grid({ top: 16, bottom: 8 }),
          tooltip: { ...tip({ axisPointer: { type: 'line' } }), formatter: (ps: { value: number[] }[]) => `${ps[0].value[0]}% of the files hold ${ps[0].value[1]}% of the lines that never landed` },
          xAxis: { ...value((v) => `${v}%`), max: 100, name: 'files, most never landed first', nameLocation: 'middle', nameGap: 26, nameTextStyle: { color: INK } },
          yAxis: percent(),
          series: [{ type: 'line', data: [[0, 0], ...points], showSymbol: false, lineStyle: { width: 2, color: PART_COLOUR.never }, areaStyle: { color: PART_COLOUR.never, opacity: 0.12 } }],
        }}
      />
    )
  }
  const table = () => (
    <Table striped highlightOnHover fz="xs" verticalSpacing={4}>
      <Table.Thead>
        <Table.Tr>
          <Table.Th>File</Table.Th>
          <Table.Th>Models</Table.Th>
          <Table.Th ta="right">Commits</Table.Th>
          <Table.Th ta="right">Lines</Table.Th>
          <Table.Th ta="right">Landed</Table.Th>
          <Table.Th ta="right">Never landed</Table.Th>
          <Table.Th ta="right">Main-line changes</Table.Th>
          <Table.Th>Last changed</Table.Th>
        </Table.Tr>
      </Table.Thead>
      <Table.Tbody>
        {all.map((x) => (
          <Table.Tr key={`${x.repo}/${x.path}`} style={{ cursor: 'pointer' }} onClick={() => behind(x)}>
            <Table.Td>
              <Text fz="xs" ff="monospace" style={{ overflowWrap: 'anywhere' }}>{many ? `${x.repo}: ${x.path}` : x.path}</Text>
            </Table.Td>
            <Table.Td>
              <Group gap={8}>{x.models.map((m) => <ModelBadge key={m} model={m} pal={pal} />)}</Group>
            </Table.Td>
            <Table.Td ta="right">{n(x.commits.length)}</Table.Td>
            <Table.Td ta="right">{n(x.added)}</Table.Td>
            <Table.Td ta="right">{pct(x.landed, x.added)}%</Table.Td>
            <Table.Td ta="right">{n(lost(x))} ({pct(lost(x), x.added)}%)</Table.Td>
            <Table.Td ta="right">{n(x.mainChanges)}</Table.Td>
            <Table.Td>{local(x.lastMainChange)}</Table.Td>
          </Table.Tr>
        ))}
      </Table.Tbody>
    </Table>
  )
  const cards: Card[] = [
    { i: 'files', title: `The ${shown.length} files holding the most lines that never landed`, note: 'lines the credited commits added to each · click a bar for its commits', w: 8, h: 12, render: bars },
    { i: 'curve', title: 'How concentrated it is', note: 'files, most never-landed lines first, against the share of those lines they hold', w: 4, h: 12, render: curve },
    { i: 'table', title: `Every file in view (${n(all.length)})`, note: 'most never-landed lines first · click a row for its commits', w: 12, h: 14, scroll: true, render: table },
  ]
  return <Board name="results.files" cards={cards} />
}
