import { Fragment, useMemo, useState } from 'react'
import { Alert, Badge, Group, Loader, Table, Text } from '@mantine/core'
import { useQuery } from '@tanstack/react-query'
import { fetchCalls, type HourRow, type Split, type TimeRow, type WatchTime } from '../api'
import { Board, type Card } from '../Board'
import { EChart, FAIL, GRID_LINE, INK, INK_MUTED, MONO, TRACK, type Clicked } from '../EChart'
import { category, grid, legend, percent, stacked, tip, value } from '../results/charts'
import { clock, day } from '../time'
import { atWork, dayOf, every, FILE_TOOLS, heldUp, hours, keepRow, n, pct, plus, short, sumBy, TIME, took, unheld, zero, type Focus } from './data'
import { around, Lines } from './Lines'

// Where the agents' time goes, traced to what it went on: by model, by day,
// thinking and writing over time, what the agents do (each tool, then what
// it was called on, then each call), the code they spent it on, and what
// held them up. A click on a model, a day or a session narrows every chart
// and every view to it; a click on a tool, a target or a folder opens the
// next level here.

const TOP_TOOLS = 15 // skill-allow: §9-const — bars drawn; the drill lists every one
const TOP_TARGETS = 25 // skill-allow: §9-const — bars drawn; the calls list every one

/// An agent's own scratch and memory: not the code it works on.
const SCRATCH = ['/private/tmp/', '/tmp/', '/.claude/projects/', '/.claude/plans/']

const series = (keys: (keyof Split)[], data: (k: keyof Split) => number[], stack = 't', extra: Record<string, unknown> = {}) =>
  TIME.filter(([k]) => keys.includes(k)).map(([k, name, colour]) => stacked(name, data(k), colour, stack, extra))

const ALL: (keyof Split)[] = TIME.map(([k]) => k)
const WORK: (keyof Split)[] = ['thinking', 'writing', 'tools', 'jobs']
const HELD: (keyof Split)[] = ['jobs', 'approval', 'api']

const hourTip = tip({ valueFormatter: (v: number) => `${v} h` })

function CallsTable({ tool, target, under, focus, core }: { tool?: string; target?: string; under?: string; focus: Focus; core: string | null }) {
  const [opened, setOpened] = useState<string | null>(null)
  const from = focus.day ? new Date(`${focus.day}T00:00:00`).toISOString() : undefined
  const to = focus.day ? new Date(new Date(`${focus.day}T00:00:00`).getTime() + 86_400_000).toISOString() : undefined
  const q = useQuery({
    queryKey: ['calls', tool, target, under, focus.session, from],
    queryFn: () => fetchCalls({ tool, target, under, session: focus.session, from, to }),
    staleTime: 30_000,
  })
  if (q.isPending) return <Loader size="xs" />
  if (q.isError) return <Alert color="red" title="The calls did not load">{String(q.error)}</Alert>
  return (
    <>
      <Text fz="xs" c="dimmed" mb={4}>
        {n(q.data.total)} calls{q.data.total > q.data.calls.length ? `, the newest ${n(q.data.calls.length)} listed` : ''} · click one for its lines
      </Text>
      <Table fz="xs" verticalSpacing={2} highlightOnHover stickyHeader>
        <Table.Thead>
          <Table.Tr>
            <Table.Th>Started</Table.Th>
            <Table.Th>Ran</Table.Th>
            <Table.Th>Call</Table.Th>
            <Table.Th>Session</Table.Th>
          </Table.Tr>
        </Table.Thead>
        <Table.Tbody>
          {q.data.calls.map((c) => (
            <Fragment key={c.session + c.call}>
              <Table.Tr style={{ cursor: 'pointer' }} onClick={() => setOpened(opened === c.call ? null : c.call)}>
                <Table.Td ff={MONO} style={{ whiteSpace: 'nowrap' }}>{day(c.start)}</Table.Td>
                <Table.Td style={{ whiteSpace: 'nowrap' }}>
                  <Text span fz="xs" c={c.failed ? FAIL : undefined}>{took(c.secs)}{c.failed ? ' · failed' : ''}{c.end == null ? ' · running' : ''}</Text>
                </Table.Td>
                <Table.Td>
                  <Text fz="xs" ff={MONO} lineClamp={2} style={{ overflowWrap: 'anywhere' }}>{c.tool} · {c.what}</Text>
                </Table.Td>
                <Table.Td ff={MONO} style={{ overflowWrap: 'anywhere' }}>{c.name}</Table.Td>
              </Table.Tr>
              {opened === c.call && (
                <Table.Tr>
                  <Table.Td colSpan={4} bg="dark.7">
                    <Lines session={c.session} {...spanOf(c.start, c.end)} mark={c.call} core={core} />
                  </Table.Td>
                </Table.Tr>
              )}
            </Fragment>
          ))}
        </Table.Tbody>
      </Table>
    </>
  )
}

const spanOf = (start: string, end: string | null) => {
  const [from, to] = around(start, end ?? start, 180, 120)
  return { from, to }
}

/// A tree of folders and files, each sized by the time on it, single-child
/// folders folded into one name.
type Node = { name: string; value: number; children?: Node[]; path: string }
function tree(files: Map<string, number>): Node[] {
  const root: Node = { name: '', value: 0, children: [], path: '' }
  for (const [path, secs] of files) {
    let at = root
    const parts = path.split('/').filter(Boolean)
    parts.forEach((p, i) => {
      at.value += secs
      at.children = at.children ?? []
      let next = at.children.find((c) => c.name === p)
      if (!next) {
        next = { name: p, value: 0, path: `${at.path}/${p}`, children: i < parts.length - 1 ? [] : undefined }
        at.children.push(next)
      }
      at = next
    })
    at.value += secs
  }
  const fold = (x: Node): Node => {
    let node = x
    while (node.children && node.children.length === 1 && node.children[0].children) {
      const only = node.children[0]
      node = { ...only, name: node.name ? `${node.name}/${only.name}` : only.name }
    }
    return { ...node, value: Math.round(node.value / 60), children: node.children?.map(fold) }
  }
  return (root.children ?? []).map(fold)
}

export function Time({ time, focus, setFocus, core }: { time: WatchTime; focus: Focus; setFocus: (f: Focus) => void; core: string | null }) {
  const [tool, setTool] = useState<string | null>(null)
  const [target, setTarget] = useState<string | null>(null)
  const [under, setUnder] = useState<string | null>(null)
  const rows = useMemo(() => time.rows.filter(keepRow(focus)), [time, focus])
  const unfocusedDay = useMemo(() => time.rows.filter(keepRow({ ...focus, day: undefined })), [time, focus])
  const byModel = useMemo(() => {
    const m = sumBy(time.rows.filter(keepRow({ ...focus, model: undefined })), (r) => r.model ?? 'unknown', (r) => r.split)
    return [...m.entries()].filter(([, s]) => atWork(s) >= 1800).sort((a, b) => atWork(b[1]) - atWork(a[1]))
  }, [time, focus])
  const days = useMemo(() => {
    const m = sumBy(unfocusedDay, (r) => r.day, (r) => r.split)
    return [...m.entries()].sort((a, b) => a[0].localeCompare(b[0]))
  }, [unfocusedDay])
  const tools = useMemo(() => {
    const m = sumBy(rows, (r) => r.tool, (r) => r.split)
    const calls = new Map<string, number>()
    for (const r of rows) calls.set(r.tool, (calls.get(r.tool) ?? 0) + r.calls)
    const busy = (s: Split) => atWork(s) + s.approval
    return [...m.entries()]
      .filter(([t, s]) => t !== '(your next message)' && busy(s) > 0)
      .sort((a, b) => busy(b[1]) - busy(a[1]))
      .map(([t, s]) => ({ tool: t, split: s, calls: calls.get(t) ?? 0 }))
  }, [rows])
  const targets = useMemo(() => {
    if (!tool) return []
    const of = rows.filter((r) => r.tool === tool)
    const m = new Map<string, { split: Split; calls: number; failed: number }>()
    for (const r of of) {
      const e = m.get(r.target) ?? { split: zero(), calls: 0, failed: 0 }
      m.set(r.target, { split: plus(e.split, r.split), calls: e.calls + r.calls, failed: e.failed + r.failed })
    }
    return [...m.entries()].sort((a, b) => every(b[1].split) - every(a[1].split))
  }, [rows, tool])
  const files = useMemo(() => {
    const m = new Map<string, number>()
    const code = (r: TimeRow) => FILE_TOOLS.has(r.tool) && r.target.startsWith('/') && !SCRATCH.some((x) => r.target.includes(x))
    for (const r of rows.filter(code)) m.set(r.target, (m.get(r.target) ?? 0) + atWork(r.split))
    return m
  }, [rows])
  const holdups = useMemo(() => {
    const m = new Map<string, Split>()
    for (const r of rows) {
      if (heldUp(r.split) === 0 || r.tool === '(your next message)') continue
      const k = `${r.tool}${r.target ? ` · ${r.target}` : ''}`
      m.set(k, plus(m.get(k) ?? zero(), r.split))
    }
    return [...m.entries()].map(([k, s]) => [k, { ...s, operator: 0, thinking: 0, writing: 0, tools: 0 }] as [string, Split]).sort((a, b) => heldUp(b[1]) - heldUp(a[1])).slice(0, TOP_TARGETS)
  }, [rows])
  const sessionHours: HourRow[] = useMemo(() => (focus.session ? time.hours.filter((h) => h.session === focus.session) : []), [time, focus])

  // Past three weeks, a day's share swings too far to read: weeks instead.
  const trend = useMemo(() => {
    if (days.length <= 21) return days.map(([d, s]) => [short(d), s] as [string, Split])
    const weeks = new Map<string, Split>()
    for (const [d, s] of days) {
      const t = new Date(`${d}T00:00:00`)
      t.setDate(t.getDate() - ((t.getDay() + 6) % 7))
      const k = `wk ${short(dayOf(t.toISOString()))}`
      weeks.set(k, plus(weeks.get(k) ?? zero(), s))
    }
    return [...weeks.entries()]
  }, [days])
  const total = rows.reduce((s, r) => plus(s, r.split), zero())
  const work = atWork(total)
  const pickDay = (i: number) => days[i] && setFocus({ ...focus, day: days[i][0] })
  const answer = work
    ? `${hours(work)} h at work: ${pct(total.thinking, work)}% thinking, ${pct(total.writing, work)}% writing, ${pct(total.tools, work)}% running tools, ${pct(total.jobs, work)}% waiting on its own jobs; ${hours(total.operator)} h waiting on you, ${hours(total.approval)} h on your approval, ${hours(total.api)} h on the API's answers that never came`
    : 'No time in view'

  const cards: Card[] = [
    {
      i: 'models',
      title: byModel.length
        ? `How each model works · ${byModel[0][0]}, most at work (${hours(atWork(byModel[0][1]))} h): ${pct(byModel[0][1].thinking, atWork(byModel[0][1]))}% thinking, ${pct(byModel[0][1].writing, atWork(byModel[0][1]))}% writing`
        : 'How each model works',
      note: 'share of its time at work, its own jobs included; the waiting on you is in the tooltip · click a model to see only it',
      w: 12,
      h: 8,
      render: (height) => (
        <EChart
          height={height}
          onClick={(p: Clicked) => p.dataIndex != null && setFocus({ ...focus, model: [...byModel].reverse()[p.dataIndex][0] })}
          option={{
            legend: legend(),
            grid: grid(),
            tooltip: tip({
              formatter: (ps: { dataIndex: number; marker: string; seriesName: string; value: number }[]) => {
                const [m, s] = [...byModel].reverse()[ps[0].dataIndex]
                return `<b>${m}</b> · ${hours(atWork(s))} h at work<br/>${ps.map((p) => `${p.marker}${p.seriesName}: ${p.value}%`).join('<br/>')}<br/>waiting on you ${hours(s.operator)} h · on your approval ${hours(s.approval)} h`
              },
            }),
            xAxis: percent(),
            yAxis: category([...byModel].reverse().map(([m]) => m), { axisLabel: { color: INK, fontSize: 11 } }),
            series: series(WORK, (k) => [...byModel].reverse().map(([, s]) => pct(s[k], atWork(s))), 'm', { barMaxWidth: 20 }),
          }}
        />
      ),
    },
    {
      i: 'days',
      title: days.length ? `Hours at work by day · ${hours(days.reduce((t, [, s]) => t + atWork(s), 0))} h over ${days.length} days` : 'Hours at work by day',
      note: 'every second the agents worked, their own jobs included; the waiting is in "What held them up" · click a day',
      w: 8,
      h: 9,
      render: (height) => (
        <EChart
          height={height}
          onClick={(p: Clicked) => p.dataIndex != null && pickDay(p.dataIndex)}
          option={{ legend: legend(), grid: grid(), tooltip: hourTip, xAxis: category(days.map(([d]) => short(d))), yAxis: value(), series: series(WORK, (k) => days.map(([, s]) => Number(hours(s[k])))) }}
        />
      ),
    },
    {
      i: 'share',
      title: `Thinking and writing, share of the work by ${days.length > 21 ? 'week' : 'day'}`,
      note: `each ${days.length > 21 ? 'week' : 'day'}'s work, waiting on its own jobs included, as shares${days.length > 21 ? '' : ' · click a day'}`,
      w: 4,
      h: 9,
      render: (height) => (
        <EChart
          height={height}
          onClick={(p: Clicked) => p.dataIndex != null && days.length <= 21 && pickDay(p.dataIndex)}
          option={{
            legend: legend(),
            grid: grid(),
            tooltip: tip({ trigger: 'axis', axisPointer: { type: 'line' }, valueFormatter: (v: number) => `${v}%` }),
            xAxis: category(trend.map(([d]) => d)),
            yAxis: percent(),
            series: TIME.filter(([k]) => WORK.includes(k)).map(([k, name, colour]) => ({
              name,
              type: 'line',
              showSymbol: true,
              symbolSize: 6,
              lineStyle: { width: 2, color: colour },
              itemStyle: { color: colour },
              data: trend.map(([, s]) => pct(s[k], atWork(s))),
            })),
          }}
        />
      ),
    },
    {
      i: 'tools',
      title: tools.length ? `What the agents do · ${tools[0].tool} took the most, ${hours(atWork(tools[0].split) + tools[0].split.approval)} h` : 'What the agents do',
      note: `each tool's hours at work and on your approval, the ${TOP_TOOLS} largest · click a tool to see what it worked on`,
      w: 6,
      h: 12,
      render: (height) => {
        const shown = tools.slice(0, TOP_TOOLS).reverse()
        return (
          <EChart
            height={height}
            onClick={(p: Clicked) => p.dataIndex != null && (setTool(shown[p.dataIndex].tool), setTarget(null), setUnder(null))}
            option={{
              legend: legend(),
              grid: grid(),
              tooltip: tip({
                formatter: (ps: { dataIndex: number; marker: string; seriesName: string; value: number }[]) => {
                  const x = shown[ps[0].dataIndex]
                  return `<b>${x.tool}</b> · ${n(x.calls)} calls<br/>${ps.filter((p) => p.value > 0).map((p) => `${p.marker}${p.seriesName}: ${p.value} h`).join('<br/>')}`
                },
              }),
              xAxis: value((v) => `${v} h`),
              yAxis: category(shown.map((x) => x.tool), { axisLabel: { color: INK, fontSize: 11, fontFamily: MONO } }),
              series: series([...WORK, 'approval'], (k) => shown.map((x) => Number(hours(x.split[k]))), 'w', { barMaxWidth: 16 }),
            }}
          />
        )
      },
    },
    {
      i: 'targets',
      title: tool ? `What ${tool} worked on · ${targets.length} ${unheld(tool) ? 'kinds' : 'targets'}` : 'What a tool worked on',
      note: tool ? `the ${TOP_TARGETS} largest by hours · click one for its calls` : 'click a tool in the chart beside this',
      w: 6,
      h: 12,
      render: (height) => {
        if (!tool) return <Text c="dimmed" fz="sm">Pick a tool: its files, commands or jobs open here, with their hours.</Text>
        const shown = targets.slice(0, TOP_TARGETS).reverse()
        return (
          <EChart
            height={height}
            onClick={(p: Clicked) => p.dataIndex != null && (setTarget(shown[p.dataIndex][0]), setUnder(null))}
            option={{
              legend: legend(),
              grid: grid(),
              tooltip: tip({
                formatter: (ps: { dataIndex: number; marker: string; seriesName: string; value: number }[]) => {
                  const [t, x] = shown[ps[0].dataIndex]
                  return `<b>${t || '(none)'}</b><br/>${n(x.calls)} calls, ${n(x.failed)} failed<br/>${ps.filter((p) => p.value > 0).map((p) => `${p.marker}${p.seriesName}: ${p.value} min`).join('<br/>')}`
                },
              }),
              xAxis: value((v) => `${v} min`),
              yAxis: category(shown.map(([t]) => (t.length > 48 ? `…${t.slice(-47)}` : t || '(none)')), { axisLabel: { color: INK, fontSize: 10, fontFamily: MONO } }),
              series: series(ALL, (k) => shown.map(([, x]) => Math.round(x.split[k] / 60)), 'g', { barMaxWidth: 14 }),
            }}
          />
        )
      },
    },
    {
      i: 'calls',
      title: target != null || under ? `The calls · ${under ? `under ${under}` : `${tool ?? ''} ${target || '(none)'}`}` : 'The calls',
      note: 'every call, newest first, with how long it ran · click one for the session around it',
      w: 12,
      h: 10,
      scroll: true,
      render: () =>
        target != null || under ? (
          <CallsTable tool={tool && !under ? tool : undefined} target={under ? undefined : target ?? undefined} under={under ?? undefined} focus={focus} core={core} />
        ) : (
          <Text c="dimmed" fz="sm">Pick what a tool worked on, or a folder in the code below: its calls open here.</Text>
        ),
    },
    {
      i: 'code',
      title: files.size ? `Traced to the code · ${n(files.size)} files, ${hours([...files.values()].reduce((a, b) => a + b, 0))} h on them` : 'Traced to the code',
      note: "every file of the code the agents read or wrote (their scratch and memory left out), sized by their time on it · click a folder to open it, a file for its calls",
      w: 12,
      h: 12,
      render: (height) => (
        <EChart
          height={height}
          onClick={(p: Clicked & { data?: { path?: string; children?: unknown } }) => {
            const path = p.data?.path
            if (path && !p.data?.children) {
              setTool(null)
              setTarget(path)
              setUnder(null)
            } else if (path) {
              setUnder(path)
            }
          }}
          option={{
            tooltip: {
              backgroundColor: '#150420',
              borderColor: GRID_LINE,
              textStyle: { color: INK, fontSize: 12 },
              formatter: (p: { data: Node; treePathInfo: { name: string }[] }) => `<b>${p.data.path}</b><br/>${took(p.data.value * 60)} of the agents' work`,
            },
            series: [
              {
                type: 'treemap',
                left: 0,
                right: 0,
                top: 0,
                bottom: 30,
                data: tree(files),
                roam: false,
                nodeClick: 'zoomToNode',
                leafDepth: 3,
                breadcrumb: { show: true, bottom: 0, itemStyle: { color: TRACK, borderColor: GRID_LINE, textStyle: { color: INK } } },
                label: { show: true, color: '#fff', fontSize: 11, formatter: '{b}' },
                upperLabel: { show: true, height: 18, color: INK, fontSize: 11 },
                itemStyle: { borderColor: TRACK, borderWidth: 1, gapWidth: 1 },
                levels: [
                  { itemStyle: { borderColor: '#150420', borderWidth: 3, gapWidth: 3 } },
                  { colorSaturation: [0.35, 0.6], itemStyle: { gapWidth: 1, borderColorSaturation: 0.6 } },
                ],
                color: ['#B833E8', '#3987e5', '#199e70', '#c98500', '#d55181', '#008300'],
              },
            ],
          }}
        />
      ),
    },
    {
      i: 'held',
      title: `What held the work up · ${hours(total.jobs)} h on their own jobs, ${hours(total.approval)} h on your approval, ${hours(total.api)} h on the API`,
      note: `the hours the work stood still, by day; besides, ${hours(total.operator)} h the agents waited on your next message · click a day`,
      w: 6,
      h: 10,
      render: (height) => (
        <EChart
          height={height}
          onClick={(p: Clicked) => p.dataIndex != null && pickDay(p.dataIndex)}
          option={{ legend: legend(), grid: grid(), tooltip: hourTip, xAxis: category(days.map(([d]) => short(d))), yAxis: value(), series: series(HELD, (k) => days.map(([, s]) => Number(hours(s[k])))) }}
        />
      ),
    },
    {
      i: 'holdups',
      title: holdups.length ? `The biggest hold-ups · ${holdups[0][0].slice(0, 60)}` : 'The biggest hold-ups',
      note: 'the jobs it waited on, the calls held on your approval and the API not answering, by hours',
      w: 6,
      h: 10,
      render: (height) => {
        const shown = [...holdups].reverse()
        return (
          <EChart
            height={height}
            option={{
              legend: legend(),
              grid: grid(),
              tooltip: tip({ valueFormatter: (v: number) => `${v} h` }),
              xAxis: value((v) => `${v} h`),
              yAxis: category(shown.map(([k]) => (k.length > 52 ? `${k.slice(0, 51)}…` : k)), { axisLabel: { color: INK_MUTED, fontSize: 10, fontFamily: MONO } }),
              series: series(HELD, (k) => shown.map(([, s]) => Number(hours(s[k]))), 'h', { barMaxWidth: 14 }),
            }}
          />
        )
      },
    },
  ]
  if (focus.session) {
    cards.push({
      i: 'session',
      title: `This session, hour by hour · ${sessionHours.length} hours with a line`,
      note: 'every hour of the session in focus, by what it went on',
      w: 12,
      h: 9,
      render: (height) => (
        <EChart
          height={height}
          option={{
            legend: legend(),
            grid: grid(),
            tooltip: tip({ valueFormatter: (v: number) => `${v} min` }),
            xAxis: category(sessionHours.map((h) => `${dayOf(h.hour).slice(5)} ${clock(h.hour).slice(0, 5)}`)),
            yAxis: value((v) => `${v} min`),
            series: series(ALL, (k) => sessionHours.map((h) => Math.round(h.split[k] / 60))),
          }}
        />
      ),
    })
  }
  return (
    <>
      <Group gap="xs" mb={6}>
        <Badge variant="light" color="grape">{answer}</Badge>
        {tool && <Badge variant="outline" rightSection="×" style={{ cursor: 'pointer' }} onClick={() => (setTool(null), setTarget(null))}>tool {tool}</Badge>}
        {target != null && <Badge variant="outline" rightSection="×" style={{ cursor: 'pointer' }} onClick={() => setTarget(null)}>{target || '(none)'}</Badge>}
        {under && <Badge variant="outline" rightSection="×" style={{ cursor: 'pointer' }} onClick={() => setUnder(null)}>under {under}</Badge>}
      </Group>
      <Board name="watcher.time" cards={cards} />
    </>
  )
}
