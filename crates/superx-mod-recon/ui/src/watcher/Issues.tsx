import { Fragment, useMemo, useState } from 'react'
import { Badge, Group, Paper, SegmentedControl, Stack, Table, Text } from '@mantine/core'
import type { Found, Watch, WatchTime } from '../api'
import { AXIS, EChart, GRID_LINE, HEAT, INK, INK_MUTED, MONO, TOOLTIP, type Clicked } from '../EChart'
import { category, grid, tip, value } from '../results/charts'
import { day } from '../time'
import { atWork, casesOf, dayOf, HIDDEN, HIDDEN_ORDER, KIND, mondayOf, n, weekName, type Case, type Focus } from './data'
import { around, Lines } from './Lines'

// The mold: every issue the watcher knows, cast the same way over every
// repository, every session and every model, its own sessions included. A
// cell is how often the issue happened there, coloured by how often per ten
// hours at work, so a large repository and a small one read alike; a click
// on it lists every case, each opening on the session's lines that caused
// it; and the issue picked, week by week, says whether it is getting better.

type By = 'repo' | 'session' | 'model'

const ISSUES: [string, string][] = [
  ...(Object.keys(KIND) as (keyof typeof KIND)[]).map((k): [string, string] => [k, KIND[k][0]]),
  ...HIDDEN_ORDER.map((k): [string, string] => [k, HIDDEN[k][0]]),
]

const COLUMNS = 24 // skill-allow: §9-const — columns drawn; the cases list every one

/// A session's folder as the repository it worked in: the first two folders
/// under the home folder (`gryphon/SuperX`), or under the root.
export function repoOf(folder: string | null): string {
  const parts = (folder ?? '').split('/').filter(Boolean)
  const under = parts[0] === 'Users' || parts[0] === 'home' ? parts.slice(2) : parts
  return under.slice(0, 2).join('/') || '(no folder)'
}

export function Issues({ w, found, time, focus, core }: { w: Watch; found: Found[]; time: WatchTime | undefined; focus: Focus; core: string | null }) {
  const [by, setBy] = useState<By>('repo')
  const [cell, setCell] = useState<{ issue: string; column: string } | null>(null)
  const [opened, setOpened] = useState<string | null>(null)
  const sessions = useMemo(() => new Map(w.sessions.map((s) => [s.id, s])), [w])
  const columnOf = (session: string) => {
    const s = sessions.get(session)
    if (by === 'repo') return repoOf(s?.folder ?? null)
    if (by === 'model') return s?.model ?? 'unknown'
    return s?.name ?? session
  }
  const cases: Case[] = useMemo(
    () => casesOf(w.alerts, found).filter((c) => (!focus.session || c.session === focus.session) && (!focus.model || (sessions.get(c.session)?.model ?? 'unknown') === focus.model)),
    [w, found, focus, sessions],
  )
  // Hours at work in each column: the yardstick a count is read against.
  const worked = useMemo(() => {
    const m = new Map<string, number>()
    for (const r of time?.rows ?? []) m.set(columnOf(r.session), (m.get(columnOf(r.session)) ?? 0) + atWork(r.split))
    return m
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [time, by, sessions])
  const counts = new Map<string, number>()
  for (const c of cases) counts.set(`${c.kind}\u0000${columnOf(c.session)}`, (counts.get(`${c.kind}\u0000${columnOf(c.session)}`) ?? 0) + 1)
  const totals = new Map<string, number>()
  for (const c of cases) totals.set(columnOf(c.session), (totals.get(columnOf(c.session)) ?? 0) + 1)
  const columns = [...totals.entries()].sort((a, b) => b[1] - a[1]).slice(0, COLUMNS).map(([k]) => k)
  const rows = ISSUES.filter(([k]) => cases.some((c) => c.kind === k))
  const rate = (count: number, column: string) => {
    const h = (worked.get(column) ?? 0) / 3600
    return h > 0 ? (10 * count) / h : 0
  }
  const cells = rows.flatMap(([k], y) => columns.map((col, x) => [x, y, Number(rate(counts.get(`${k}\u0000${col}`) ?? 0, col).toFixed(2)), counts.get(`${k}\u0000${col}`) ?? 0]))
  const top = Math.max(1, ...cells.map((c) => c[2] as number))
  const picked = cell ? cases.filter((c) => c.kind === cell.issue && columnOf(c.session) === cell.column).sort((a, b) => b.at.localeCompare(a.at)) : []
  const trendCases = cell ? cases.filter((c) => c.kind === cell.issue) : cases
  const today = dayOf(new Date().toISOString())
  const weekOf = (iso: string) => mondayOf(dayOf(iso))
  const weeks = [...new Set(trendCases.map((c) => weekOf(c.at)))].sort((a, b) => (a < b ? -1 : 1))
  const label = (k: string) => ISSUES.find(([x]) => x === k)?.[1] ?? k
  const worst = columns.length && rows.length ? columns.map((col) => [col, totals.get(col) ?? 0, (worked.get(col) ?? 0) / 3600] as [string, number, number]) : []
  return (
    <Stack gap="md">
      <Group justify="space-between" align="flex-end">
        <Text fz="sm" c="dimmed" maw={900}>
          The same issues, cast over every {by}: {n(cases.length)} cases of {rows.length} kinds.
          {worst.length > 0 &&
            ` Most in ${worst[0][0]}: ${n(worst[0][1])} cases over ${worst[0][2].toFixed(0)} h at work.`}{' '}
          A cell counts the cases; its colour is how many per 10 hours at work there. Click one for every case.
        </Text>
        <SegmentedControl size="xs" value={by} onChange={(v) => { setBy(v as By); setCell(null) }} data={[{ value: 'repo', label: 'by repository' }, { value: 'session', label: 'by session' }, { value: 'model', label: 'by model' }]} />
      </Group>
      <Paper withBorder radius="md" p="sm">
        <EChart
          height={60 + 26 * rows.length}
          onClick={(p: Clicked & { data?: number[] }) => {
            if (!Array.isArray(p.data)) return
            const [x, y] = p.data
            setCell({ issue: rows[y][0], column: columns[x] })
            setOpened(null)
          }}
          option={{
            grid: { left: 210, right: 16, top: 8, bottom: by === 'session' ? 150 : 80 },
            tooltip: {
              ...TOOLTIP,
              formatter: (p: { data: number[] }) => {
                const [x, y, r, c] = p.data
                return `<b>${label(rows[y][0])}</b> in ${columns[x]}<br/>${n(c)} cases · ${r} per 10 h at work (${((worked.get(columns[x]) ?? 0) / 3600).toFixed(1)} h)`
              },
            },
            xAxis: {
              type: 'category',
              data: columns,
              axisLine: { lineStyle: { color: GRID_LINE } },
              axisTick: { show: false },
              axisLabel: { color: INK_MUTED, fontSize: 10, fontFamily: MONO, rotate: 35, formatter: (v: string) => (v.length > 26 ? `${v.slice(0, 25)}…` : v) },
              splitArea: { show: false },
            },
            yAxis: { type: 'category', data: rows.map(([k]) => label(k)), inverse: true, axisLine: { lineStyle: { color: GRID_LINE } }, axisTick: { show: false }, axisLabel: { color: INK, fontSize: 11 } },
            visualMap: { min: 0, max: top, show: false, inRange: { color: HEAT } },
            series: [
              {
                type: 'heatmap',
                data: cells,
                label: { show: true, color: '#fff', fontSize: 10, formatter: (p: { data: number[] }) => (p.data[3] ? String(p.data[3]) : '') },
                itemStyle: { borderColor: '#150420', borderWidth: 1 },
                emphasis: { itemStyle: { borderColor: '#fff', borderWidth: 1 } },
              },
            ],
          }}
        />
      </Paper>
      <Paper withBorder radius="md" p="sm">
        <Text fw={600} fz="sm">{cell ? `${label(cell.issue)}, week by week` : 'Every issue, week by week'}</Text>
        <Text fz="xs" c="dimmed">{cell ? 'the issue picked, in every column' : 'click a cell above to follow one issue'}</Text>
        <EChart
          height={180}
          option={{
            grid: grid(),
            tooltip: tip(),
            xAxis: category(weeks.map((wk) => weekName(wk, today)), { axisLabel: { ...AXIS.axisLabel, showMaxLabel: true, alignMaxLabel: 'right', hideOverlap: true } }),
            yAxis: value(),
            series: [{ type: 'bar', barMaxWidth: 26, itemStyle: { color: HEAT[3], borderRadius: [3, 3, 0, 0] }, data: weeks.map((wk) => trendCases.filter((c) => weekOf(c.at) === wk).length) }],
          }}
        />
      </Paper>
      {cell && (
        <Paper withBorder radius="md">
          <Group p="xs" gap="xs">
            <Badge variant="light" color="grape">{label(cell.issue)}</Badge>
            <Text fz="sm">in {cell.column}: {picked.length} cases, newest first · click one for its cause</Text>
          </Group>
          <Table.ScrollContainer minWidth={900}>
            <Table fz="sm" verticalSpacing={4} highlightOnHover>
              <Table.Tbody>
                {picked.slice(0, 200).map((c) => {
                  const [from, to] = around(c.from, c.at, 300, 180)
                  return (
                    <Fragment key={c.id}>
                      <Table.Tr style={{ cursor: 'pointer' }} onClick={() => setOpened(opened === c.id ? null : c.id)}>
                        <Table.Td ff={MONO} fz="xs" style={{ whiteSpace: 'nowrap' }}>{opened === c.id ? '▾' : '▸'} {day(c.at)}</Table.Td>
                        <Table.Td style={{ overflowWrap: 'anywhere' }}><Text fz="sm" lineClamp={opened === c.id ? undefined : 2}>{c.what}</Text></Table.Td>
                        <Table.Td><Text fz="xs" ff={MONO} style={{ overflowWrap: 'anywhere' }}>{c.name}</Text></Table.Td>
                        <Table.Td style={{ whiteSpace: 'nowrap' }}>{c.cost}</Table.Td>
                      </Table.Tr>
                      {opened === c.id && (
                        <Table.Tr>
                          <Table.Td colSpan={4} bg="dark.7">
                            <Lines session={c.session} from={from} to={to} mark={c.call} core={core} />
                          </Table.Td>
                        </Table.Tr>
                      )}
                    </Fragment>
                  )
                })}
              </Table.Tbody>
            </Table>
          </Table.ScrollContainer>
          {picked.length > 200 && <Text fz="xs" c="dimmed" p="xs">the newest 200 of {n(picked.length)}</Text>}
        </Paper>
      )}
    </Stack>
  )
}
