import { Fragment, useMemo, useState } from 'react'
import { Badge, Group, Paper, SegmentedControl, Stack, Table, Text } from '@mantine/core'
import type { BuildRow, Cost, HourRow, Waste, Watch, WatchTask } from '../api'
import { CANCEL, EChart, FAIL, GRID_LINE, INK, INK_MUTED, MONO, OK, TOOLTIP, type Clicked } from '../EChart'
import { day } from '../time'
import { addCost, atWork, big, costWords, n, took, zeroCost } from './data'
import { around, Lines } from './Lines'
import { merged } from './Tasks'

// Tickets (#573): one ticket's every session on one timeline. A ticket is
// the key its branches carry (`ENG-8080`, or `#573` for a branch named for
// an issue); its lanes are each session's branch of it and each build of its
// task file. Every merge, every reopening (an undo names the merge it put
// back), every build you ordered, its door gates, where it stopped short or
// broke its file, and what the agent said was not done after a merge, by
// time; a point opens on that session's lines. The tickets that went round
// the most come first: a task merged, undone, built, stopped and merged
// again is one row here, however many sessions it took.

type Kind = 'asked' | 'merged' | 'merged unproven' | 'reopened' | 'not done' | 'ordered' | 'gate' | 'gate failed' | 'stopped' | 'broke its file'

/// Each kind's colour and shape: the shape tells them apart without colour.
const MARK: Record<Kind, [string, string]> = {
  asked: [INK_MUTED, 'circle'],
  merged: [OK, 'diamond'],
  'merged unproven': [FAIL, 'diamond'],
  reopened: [FAIL, 'triangle'],
  'not done': [CANCEL, 'circle'],
  ordered: ['#3987e5', 'rect'],
  gate: [OK, 'roundRect'],
  'gate failed': [FAIL, 'roundRect'],
  stopped: [FAIL, 'pin'],
  'broke its file': [FAIL, 'arrow'],
}

type Event = { at: string; lane: number; kind: Kind; words: string; session: string }
type Lane = { label: string; session: string; from: string; to: string }
type Ticket = { key: string; tasks: WatchTask[]; builds: BuildRow[] }

/// A session's name as a lane shows it: its id's first part.
const shortName = (name: string) => (name.split('/').pop() ?? name).slice(0, 8)

const esc = (s: string) => s.replace(/[&<>]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' })[c] ?? c)

/// Every ticket of the window: its tasks and its builds.
function group(w: Watch): Ticket[] {
  const by = new Map<string, Ticket>()
  const get = (key: string) => {
    if (!by.has(key)) by.set(key, { key, tasks: [], builds: [] })
    return by.get(key)!
  }
  for (const t of w.tasks) for (const k of t.tickets) get(k).tasks.push(t)
  for (const b of w.builds) if (b.ticket) get(b.ticket).builds.push(b)
  return [...by.values()]
}

/// What a ticket went through, counted; and what it cost: the hours of its
/// sessions at work and waiting on you, over each session's span on it
/// (about: a session's hour spent on two tickets counts on both).
function measure(t: Ticket, hours: HourRow[]) {
  const merges = t.tasks.reduce((s, x) => s + x.merges.length, 0)
  const reopened = t.tasks.reduce((s, x) => s + x.reopened.length, 0)
  const undone = t.tasks.reduce((s, x) => s + x.reopened.filter(([, how]) => how.startsWith('undid') || how.startsWith('rewrote the shared branch')).length, 0)
  const stopped = t.builds.filter((b) => b.stopped).length
  // A task file built again is the task taken up again.
  const rebuilt = Math.max(0, t.builds.length - 1)
  const sessions = new Set([...t.tasks.map((x) => x.session), ...t.builds.map((b) => b.session)]).size
  const times = [
    ...t.tasks.flatMap((x) => [x.asked, x.last]),
    ...t.builds.flatMap((b) => [b.ordered, b.stopped, ...b.gates.map((g) => g.ran)].filter((v): v is string => !!v)),
  ].sort()
  const spans = new Map<string, [number, number]>()
  const span = (session: string, from: string, to: string) => {
    const [f, e] = [Date.parse(from), Date.parse(to)]
    const s = spans.get(session)
    spans.set(session, s ? [Math.min(s[0], f), Math.max(s[1], e)] : [f, e])
  }
  for (const x of t.tasks) span(x.session, x.asked, x.last)
  for (const b of t.builds) span(b.session, b.ordered, [b.stopped, ...b.gates.map((g) => g.ran)].filter((v): v is string => !!v).sort().pop() ?? b.ordered)
  let work = 0
  let you = 0
  for (const h of hours) {
    const s = spans.get(h.session)
    const start = Date.parse(h.hour)
    if (s && start + 3_600_000 > s[0] && start <= s[1]) {
      work += atWork(h.split)
      you += h.split.operator
    }
  }
  return { merges, reopened, undone, stopped, rebuilt, sessions, work, you, first: times[0], last: times[times.length - 1], round: reopened + stopped + rebuilt }
}

/// A ticket's lanes and events, its tasks first, then its builds, each by
/// when it began.
function timeline(t: Ticket, w: Watch, names: Map<string, string>) {
  const lanes: Lane[] = []
  const events: Event[] = []
  const tasks = [...t.tasks].sort((a, b) => a.asked.localeCompare(b.asked))
  for (const x of tasks) {
    const lane = lanes.length
    lanes.push({ label: `${[x.branch, ...x.versions].join(' → ')} · ${shortName(x.name)}`, session: x.session, from: x.asked, to: x.last })
    events.push({ at: x.asked, lane, kind: 'asked', words: 'your message before it began', session: x.session })
    for (const m of x.merges) events.push({ at: m.at, lane, kind: m.proven === false ? 'merged unproven' : 'merged', words: merged(m), session: x.session })
    for (const [at, how] of x.reopened) events.push({ at, lane, kind: 'reopened', words: how, session: x.session })
    for (const [at, words] of x.not_done) events.push({ at, lane, kind: 'not done', words: `once it read the merge it said: "${words}"`, session: x.session })
  }
  const builds = [...t.builds].sort((a, b) => a.ordered.localeCompare(b.ordered))
  for (const b of builds) {
    const lane = lanes.length
    const ends = [b.stopped, ...b.gates.map((g) => g.ran)].filter((v): v is string => !!v).sort()
    lanes.push({ label: `build of ${b.file} · ${shortName(names.get(b.session) ?? b.session)}`, session: b.session, from: b.ordered, to: ends[ends.length - 1] ?? b.ordered })
    events.push({ at: b.ordered, lane, kind: 'ordered', words: `you ordered: Build ${b.file}`, session: b.session })
    for (const g of b.gates) {
      if (!g.ran) continue
      const how = g.ended_well == null ? 'running' : g.ended_well ? 'ended well' : g.ends_well ? 'failed' : 'failed, as it expects'
      events.push({ at: g.ran, lane, kind: g.ended_well === false && g.ends_well ? 'gate failed' : 'gate', words: `door gate «${g.gate}»: ${how}`, session: b.session })
    }
    if (b.stopped) {
      const missing = b.builds.filter((f) => !b.written.includes(f))
      events.push({ at: b.stopped, lane, kind: 'stopped', words: `handed you its turn before it wrote ${missing.join(' and ')}`, session: b.session })
    }
    for (const a of w.alerts.filter((a) => a.kind === 'task_file_broken' && a.session === b.session)) {
      events.push({ at: a.at, lane, kind: 'broke its file', words: a.what, session: b.session })
    }
  }
  events.sort((a, b) => a.at.localeCompare(b.at))
  return { lanes, events }
}

function Story({ ticket, w, names, core }: { ticket: Ticket; w: Watch; names: Map<string, string>; core: string | null }) {
  const { lanes, events } = useMemo(() => timeline(ticket, w, names), [ticket, w, names])
  const [picked, setPicked] = useState<Event | null>(null)
  const kinds = (Object.keys(MARK) as Kind[]).filter((k) => events.some((e) => e.kind === k))
  const option = {
    legend: { data: kinds, top: 0, textStyle: { color: INK_MUTED, fontSize: 11 }, itemWidth: 12, itemHeight: 12 },
    grid: { left: 300, right: 24, top: 36, bottom: 32 },
    tooltip: {
      ...TOOLTIP,
      trigger: 'item',
      formatter: (p: { data?: { kind?: Kind; at?: string; words?: string } }) =>
        p.data?.kind ? `<b>${p.data.kind}</b> · ${day(p.data.at ?? '')}<br/>${esc((p.data.words ?? '').slice(0, 320))}` : '',
    },
    xAxis: { type: 'time', axisLine: { lineStyle: { color: GRID_LINE } }, splitLine: { lineStyle: { color: GRID_LINE } }, axisLabel: { color: INK_MUTED, fontSize: 10 } },
    yAxis: {
      type: 'category',
      inverse: true,
      data: lanes.map((l) => l.label),
      axisLine: { lineStyle: { color: GRID_LINE } },
      axisTick: { show: false },
      axisLabel: { color: INK, fontSize: 11, fontFamily: MONO, width: 284, overflow: 'truncate' },
    },
    series: [
      // Each lane's span, from its first act to its newest.
      ...lanes.map((l, i) => ({
        type: 'line',
        silent: true,
        symbol: 'none',
        lineStyle: { color: GRID_LINE, width: 3, cap: 'round' },
        data: [
          [l.from, i],
          [l.to, i],
        ],
      })),
      ...kinds.map((k) => ({
        type: 'scatter',
        name: k,
        symbol: MARK[k][1],
        symbolSize: k === 'asked' || k === 'not done' ? 10 : k === 'stopped' ? 24 : 18,
        itemStyle: { color: MARK[k][0], borderColor: '#2A1235', borderWidth: 1.5 },
        data: events.filter((e) => e.kind === k).map((e) => ({ value: [e.at, e.lane], ...e })),
      })),
    ],
  }
  const [from, to] = picked ? around(picked.at, picked.at, 600, 300) : ['', '']
  return (
    <Stack gap="xs" py={4}>
      <EChart
        height={90 + 34 * lanes.length}
        option={option}
        onClick={(p: Clicked & { data?: Event }) => {
          if (p.data && 'kind' in p.data) setPicked(p.data)
        }}
      />
      <Table fz="xs" verticalSpacing={2} highlightOnHover>
        <Table.Tbody>
          {events.map((e, i) => (
            <Table.Tr key={i} style={{ cursor: 'pointer' }} bg={picked === e ? 'dark.5' : undefined} onClick={() => setPicked(picked === e ? null : e)}>
              <Table.Td ff={MONO} style={{ whiteSpace: 'nowrap' }}>{day(e.at)}</Table.Td>
              <Table.Td>
                <Badge size="xs" variant="light" color={MARK[e.kind][0]} style={{ whiteSpace: 'nowrap' }}>{e.kind}</Badge>
              </Table.Td>
              <Table.Td ff={MONO} c="dimmed" style={{ whiteSpace: 'nowrap' }}>{lanes[e.lane].label.split(' · ').pop()}</Table.Td>
              <Table.Td style={{ overflowWrap: 'anywhere' }}>
                <Text fz="xs" lineClamp={picked === e ? undefined : 2}>{e.words}</Text>
              </Table.Td>
            </Table.Tr>
          ))}
        </Table.Tbody>
      </Table>
      {picked && <Lines session={picked.session} from={from} to={to} core={core} />}
    </Stack>
  )
}

type Shown = 'round' | 'merged' | 'all'

/// What a ticket threw away: the lines and tokens of its tasks that never
/// end (from your order to the newest reopening), its builds stopped short
/// and its builds that broke their file.
function wasted(t: Ticket, w: Watch, waste: Waste | undefined): Cost {
  if (!waste) return zeroCost()
  const builds = new Set(t.builds.map((b) => b.session))
  return w.alerts
    .filter(
      (a) =>
        (a.kind === 'endless_task' && t.tasks.some((x) => a.id.startsWith(`${x.session}/endless_task/${x.branch}/`))) ||
        ((a.kind === 'build_stopped' || a.kind === 'task_file_broken') && builds.has(a.session)),
    )
    .reduce((c, a) => addCost(c, waste.cases[a.id]?.cost), zeroCost())
}

export function Tickets({ w, hours, waste, core }: { w: Watch; hours: HourRow[]; waste: Waste | undefined; core: string | null }) {
  const [shown, setShown] = useState<Shown>('round')
  const [opened, setOpened] = useState<string | null>(null)
  const names = useMemo(() => new Map(w.sessions.map((s) => [s.id, s.name])), [w])
  const all = useMemo(() => group(w).map((t) => ({ t, m: measure(t, hours), x: wasted(t, w, waste) })), [w, hours, waste])
  const rows = all
    .filter(({ m }) => (shown === 'round' ? m.round > 0 : shown === 'merged' ? m.merges > 0 : true))
    .sort((a, b) => b.x.tokens.generated - a.x.tokens.generated || b.m.round - a.m.round || (b.m.last ?? '').localeCompare(a.m.last ?? ''))
  const round = all.filter(({ m }) => m.round > 0)
  const thrown = round.reduce((c, { x }) => addCost(c, x), zeroCost())
  return (
    <Stack gap="md">
      {waste && (
        <Paper withBorder radius="md" p="sm" style={{ borderColor: FAIL }}>
          <Text fz="lg" fw={700} style={{ textWrap: 'balance' }}>
            {round.length} tickets went round and threw away{' '}
            <Text span inherit c={FAIL}>
              {n(thrown.added)} lines and {big(thrown.tokens.generated)} tokens
            </Text>
            {round.length ? `; the most, ${rows[0]?.t.key}: ${n(rows[0]?.x.added ?? 0)} lines and ${big(rows[0]?.x.tokens.generated ?? 0)} tokens.` : '.'}
          </Text>
          <Text fz="xs" c="dimmed" mt={4}>
            Thrown away: the work of a task from your order to its newest reopening, a build stopped short, a build that broke its file. {costWords(thrown)}.
          </Text>
        </Paper>
      )}
      <Group justify="space-between" align="flex-end">
        <Text fz="sm" c="dimmed" maw={900}>
          {all.length} tickets; {round.length} went round: reopened after a merge, built again or stopped short; {round.reduce((s, { m }) => s + m.undone, 0)} of their
          merges undone and {round.reduce((s, { m }) => s + m.rebuilt, 0)} builds made again. Each row is one ticket across every session that worked on it. Click one for its
          timeline.
        </Text>
        <SegmentedControl
          size="xs"
          value={shown}
          onChange={(v) => setShown(v as Shown)}
          data={[
            { value: 'round', label: 'went round' },
            { value: 'merged', label: 'merged' },
            { value: 'all', label: 'every ticket' },
          ]}
        />
      </Group>
      <Paper withBorder radius="md">
        <Table.ScrollContainer minWidth={1240}>
          <Table fz="sm" verticalSpacing={4} highlightOnHover>
            <Table.Thead>
              <Table.Tr>
                <Table.Th>Ticket</Table.Th>
                <Table.Th>Sessions</Table.Th>
                <Table.Th>Merged</Table.Th>
                <Table.Th>Reopened</Table.Th>
                <Table.Th>Undone</Table.Th>
                <Table.Th>Builds</Table.Th>
                <Table.Th>From</Table.Th>
                <Table.Th>Newest</Table.Th>
                <Table.Th>Span</Table.Th>
                <Table.Th>At work</Table.Th>
                <Table.Th>Waiting on you</Table.Th>
                <Table.Th>Thrown away</Table.Th>
              </Table.Tr>
            </Table.Thead>
            <Table.Tbody>
              {rows.length === 0 && (
                <Table.Tr>
                  <Table.Td colSpan={12}>
                    <Text c="dimmed" fz="sm">No ticket here.</Text>
                  </Table.Td>
                </Table.Tr>
              )}
              {rows.map(({ t, m, x }) => (
                <Fragment key={t.key}>
                  <Table.Tr style={{ cursor: 'pointer' }} onClick={() => setOpened(opened === t.key ? null : t.key)}>
                    <Table.Td>
                      <Text fz="sm" fw={m.round ? 700 : 400} c={m.round ? FAIL : undefined} ff={MONO}>
                        {opened === t.key ? '▾' : '▸'} {t.key}
                      </Text>
                    </Table.Td>
                    <Table.Td>{m.sessions}</Table.Td>
                    <Table.Td>{m.merges}</Table.Td>
                    <Table.Td c={m.reopened ? FAIL : undefined} fw={m.reopened ? 700 : undefined}>{m.reopened}</Table.Td>
                    <Table.Td c={m.undone ? FAIL : undefined}>{m.undone}</Table.Td>
                    <Table.Td>
                      {t.builds.length}
                      {m.stopped ? <Text span c={FAIL} fz="sm">{` · ${m.stopped} stopped short`}</Text> : null}
                    </Table.Td>
                    <Table.Td ff={MONO} fz="xs" style={{ whiteSpace: 'nowrap' }}>{m.first ? day(m.first) : '—'}</Table.Td>
                    <Table.Td ff={MONO} fz="xs" style={{ whiteSpace: 'nowrap' }}>{m.last ? day(m.last) : '—'}</Table.Td>
                    <Table.Td style={{ whiteSpace: 'nowrap' }}>{m.first && m.last ? took((new Date(m.last).getTime() - new Date(m.first).getTime()) / 1000) : '—'}</Table.Td>
                    <Table.Td style={{ whiteSpace: 'nowrap' }}>{m.work ? `about ${took(m.work)}` : '—'}</Table.Td>
                    <Table.Td style={{ whiteSpace: 'nowrap' }}>{m.you ? `about ${took(m.you)}` : '—'}</Table.Td>
                    <Table.Td style={{ whiteSpace: 'nowrap' }}>
                      {x.added || x.tokens.generated ? (
                        <Text fz="xs" fw={700} c={FAIL}>
                          {n(x.added)} lines · {big(x.tokens.generated)} tokens
                        </Text>
                      ) : (
                        '—'
                      )}
                    </Table.Td>
                  </Table.Tr>
                  {opened === t.key && (
                    <Table.Tr>
                      <Table.Td colSpan={12} bg="dark.7">
                        {x.added || x.tokens.generated ? <Text fz="xs" c={FAIL} mb={4}>thrown away: {costWords(x)}</Text> : null}
                        <Story ticket={t} w={w} names={names} core={core} />
                      </Table.Td>
                    </Table.Tr>
                  )}
                </Fragment>
              ))}
            </Table.Tbody>
          </Table>
        </Table.ScrollContainer>
      </Paper>
    </Stack>
  )
}
