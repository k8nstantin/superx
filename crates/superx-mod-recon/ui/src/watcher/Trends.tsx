import { Fragment, useMemo, useState } from 'react'
import { Badge, Button, Group, Paper, SegmentedControl, SimpleGrid, Stack, Table, Text } from '@mantine/core'
import type { Found, Watch, WatchSessionRow, WatchTime } from '../api'
import { AXIS, EChart, FAIL, INK_MUTED, MONO, OK, TOOLTIP, type Clicked } from '../EChart'
import { category, grid, legend, stacked, tip, value } from '../results/charts'
import { day } from '../time'
import { Cause } from './Alarms'
import { atWork, casesOf, dayOf, daysFrom, FAMILIES, kindName, mondayOf, n, weekName, WRONG, WRONG_COLOURS, type Case, type Family, type Focus, type Kind } from './data'
import { span } from './Found'
import { repoOf } from './Issues'
import { Lines } from './Lines'

// Trends (#573): what the agents do wrong, over time, so the operator sees
// it get better or worse instead of reading alarms (operator, 2026-11-06:
// "the visual charts are far better to see the trends than the table with
// alarms"). Every alert and every finding, in families the operator's own
// words name: one row of bars each, the last seven days against the seven
// before; the family picked, by kind, per day or per week, as a count or per
// ten hours at work; by model and by session; and every case, each opening
// on the session's lines that caused it.

type Unit = 'day' | 'week'
type Measure = 'count' | 'rate'

const PAGE = 100 // skill-allow: §9-const — rows drawn at once; the rest a click away
const SESSIONS = 10 // skill-allow: §9-const — bars drawn; the cases list every session
const DAYS_AS_DAYS = 35 // skill-allow: §9-const — a span this long or shorter is drawn day by day, longer week by week

/// The day `days` before a day, both as `2026-11-04`.
function before(d: string, days: number): string {
  const t = new Date(`${d}T12:00:00`)
  t.setDate(t.getDate() - days)
  return dayOf(t.toISOString())
}

/// How a count moved, the last seven days against the seven before: more is
/// worse.
function Change({ now, then }: { now: number; then: number }) {
  if (now === then) return <Text fz="xs" c="dimmed">same</Text>
  return (
    <Text fz="xs" fw={700} c={now > then ? FAIL : OK} style={{ whiteSpace: 'nowrap' }}>
      {now > then ? `▲ ${n(now - then)} more` : `▼ ${n(then - now)} fewer`}
    </Text>
  )
}

/// A family's bars, one per day or week, its own height.
function spark(counts: number[], labels: string[], colour: string) {
  return {
    grid: { left: 2, right: 2, top: 4, bottom: 2 },
    tooltip: { ...TOOLTIP, trigger: 'axis', axisPointer: { type: 'shadow' }, formatter: (p: { name: string; value: number }[]) => `${p[0].name}: ${n(p[0].value)}` },
    xAxis: { type: 'category', data: labels, show: false },
    yAxis: { type: 'value', show: false, max: (v: { max: number }) => Math.max(1, v.max) },
    series: [{ type: 'bar', data: counts, barCategoryGap: '25%', itemStyle: { color: colour, borderRadius: [2, 2, 0, 0] } }],
  }
}

/// A session by its repository, its id and its model.
function sessionLabel(s: WatchSessionRow | undefined, id: string): string {
  const short = (s?.name ?? id).split('/').pop()?.slice(0, 8) ?? id
  return `${repoOf(s?.folder ?? null)} · ${short}${s?.model ? ` · ${s.model.replace('claude-', '')}` : ''}`
}

export function Trends({ w, found, time, focus, setFocus, core }: { w: Watch; found: Found[]; time: WatchTime | undefined; focus: Focus; setFocus: (f: Focus) => void; core: string | null }) {
  const [fam, setFam] = useState<Family | null>(null)
  const [kind, setKind] = useState<Kind | null>(null)
  const [period, setPeriod] = useState<string | null>(null)
  const [measure, setMeasure] = useState<Measure>('count')
  const [unitPicked, setUnit] = useState<Unit | null>(null)
  const [opened, setOpened] = useState<string | null>(null)
  const [shown, setShown] = useState(PAGE)
  const sessions = useMemo(() => new Map(w.sessions.map((s) => [s.id, s])), [w])
  const modelOf = (session: string) => sessions.get(session)?.model ?? 'unknown'
  const inFocus = (session: string) => (!focus.session || focus.session === session) && (!focus.model || focus.model === modelOf(session))
  const cases = useMemo(() => casesOf(w.alerts, found), [w, found]).filter((c) => inFocus(c.session))
  const rows = (time?.rows ?? []).filter((r) => inFocus(r.session))
  const today = dayOf(new Date().toISOString())
  const first = [...cases.map((c) => dayOf(c.at)), ...rows.map((r) => r.day)].reduce((a, b) => (b < a ? b : a), today)
  const days = daysFrom(first, today)
  const unit: Unit = unitPicked ?? (days.length <= DAYS_AS_DAYS ? 'day' : 'week')
  const periodOf = (d: string) => (unit === 'day' ? d : mondayOf(d))
  const periods = [...new Set(days.map(periodOf))]
  const index = new Map(periods.map((p, i) => [p, i]))
  const label = (p: string) => (unit === 'day' ? p.slice(5) : weekName(p, today))
  const labels = periods.map(label)
  // Hours at work in each period: what a count is read against.
  const worked = new Array<number>(periods.length).fill(0)
  for (const r of rows) {
    const i = index.get(periodOf(r.day))
    if (i != null) worked[i] += atWork(r.split) / 3600
  }
  const perPeriod = (keep: (c: Case) => boolean) => {
    const out = new Array<number>(periods.length).fill(0)
    for (const c of cases) {
      const i = keep(c) ? index.get(periodOf(dayOf(c.at))) : undefined
      if (i != null) out[i] += 1
    }
    return out
  }
  const asMeasured = (counts: number[]) => (measure === 'count' ? counts : counts.map((c, i) => (worked[i] > 0 ? Number(((10 * c) / worked[i]).toFixed(1)) : 0)))
  const week = before(today, 6)
  const lastWeek = [before(today, 13), before(today, 7)]
  const lastSeven = (f: Family) => cases.filter((c) => f.kinds.includes(c.kind) && dayOf(c.at) >= week).length
  const sevenBefore = (f: Family) => cases.filter((c) => f.kinds.includes(c.kind) && dayOf(c.at) >= lastWeek[0] && dayOf(c.at) <= lastWeek[1]).length
  const wrongKinds = new Set(WRONG.flatMap((f) => f.kinds))
  const inPick = (c: Case) => (fam ? fam.kinds.includes(c.kind) : wrongKinds.has(c.kind))
  const picked = cases
    .filter((c) => inPick(c) && (!kind || c.kind === kind) && (!period || periodOf(dayOf(c.at)) === period) && (!focus.day || dayOf(c.at) === focus.day))
    .sort((a, b) => b.at.localeCompare(a.at))
  const pickFamily = (f: Family | null) => {
    setFam(f)
    setKind(null)
    setPeriod(null)
    setShown(PAGE)
  }
  // The big chart: the six families of what went wrong, or the kinds of the
  // family picked.
  const series = fam
    ? fam.kinds.map((k, i) => stacked(kindName(k), asMeasured(perPeriod((c) => c.kind === k)), WRONG_COLOURS[i % WRONG_COLOURS.length], 'k', { barMaxWidth: 26 }))
    : WRONG.map((f) => stacked(f.name, asMeasured(perPeriod((c) => f.kinds.includes(c.kind))), f.colour ?? INK_MUTED, 'k', { barMaxWidth: 26 }))
  const total = cases.filter(inPick).length
  const busiest = perPeriod(inPick).reduce((best, c, i, all) => (c > all[best] ? i : best), 0)
  const shownFamilies = fam ? [fam] : WRONG
  const recent = shownFamilies.reduce((t, f) => t + lastSeven(f), 0)
  const earlier = shownFamilies.reduce((t, f) => t + sevenBefore(f), 0)
  // By model: per ten hours at work, so a model used much and one used
  // little read alike; by session: how many.
  const byModel = useMemo(() => {
    const hours = new Map<string, number>()
    for (const r of rows) hours.set(r.model ?? 'unknown', (hours.get(r.model ?? 'unknown') ?? 0) + atWork(r.split) / 3600)
    const counts = new Map<string, number>()
    for (const c of cases.filter(inPick)) counts.set(modelOf(c.session), (counts.get(modelOf(c.session)) ?? 0) + 1)
    return [...hours.entries()]
      .filter(([, h]) => h >= 1)
      .map(([m, h]) => ({ model: m, hours: h, count: counts.get(m) ?? 0, rate: (10 * (counts.get(m) ?? 0)) / h }))
      .sort((a, b) => a.rate - b.rate)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rows, cases, fam])
  const bySession = (() => {
    const counts = new Map<string, number>()
    for (const c of cases.filter(inPick)) counts.set(c.session, (counts.get(c.session) ?? 0) + 1)
    return [...counts.entries()].sort((a, b) => b[1] - a[1]).slice(0, SESSIONS).reverse()
  })()
  const what = fam ? fam.name : 'What the agents did wrong'
  const per = unit === 'day' ? 'day by day' : 'week by week'
  const conclusion =
    `${what}, ${per}: ${n(total)} in all; ${n(recent)} in the last 7 days against ${n(earlier)} the 7 before` +
    (total ? `; the most ${unit === 'day' ? 'on' : 'in the week of'} ${labels[busiest]}, ${n(perPeriod(inPick)[busiest])}.` : '.')
  return (
    <Stack gap="md">
      <Text fz="sm" maw={1000}>
        In the last 7 days the agents{' '}
        {WRONG.map((f, i) => `${i === WRONG.length - 1 ? 'and ' : ''}${f.did} ${n(lastSeven(f))} ${lastSeven(f) === 1 ? 'time' : 'times'} (${n(sevenBefore(f))} the 7 days before)`).join(', ')}. A row is every{' '}
        {unit} since {first}; click a row to pick it, a bar to see its cases.
      </Text>
      <Paper withBorder radius="md">
        <Table.ScrollContainer minWidth={760}>
          <Table fz="sm" verticalSpacing={2} highlightOnHover>
            <Table.Thead>
              <Table.Tr>
                <Table.Th>What the agents did</Table.Th>
                <Table.Th>every {unit} since {first.slice(5)}</Table.Th>
                <Table.Th style={{ whiteSpace: 'nowrap' }}>last 7 days</Table.Th>
                <Table.Th style={{ whiteSpace: 'nowrap' }}>the 7 before</Table.Th>
                <Table.Th />
              </Table.Tr>
            </Table.Thead>
            <Table.Tbody>
              {FAMILIES.map((f) => {
                const mine = fam?.key === f.key
                return (
                  <Table.Tr key={f.key} bg={mine ? 'dark.5' : undefined}>
                    <Table.Td w={300} style={{ cursor: 'pointer' }} onClick={() => pickFamily(mine ? null : f)}>
                      <Group gap={6} wrap="nowrap">
                        <span style={{ width: 10, height: 10, borderRadius: 2, flex: 'none', background: f.colour ?? INK_MUTED }} />
                        <Text fz="sm" fw={f.colour ? 700 : 500}>{f.name}</Text>
                      </Group>
                      <Text fz={11} c="dimmed" lineClamp={2}>{f.about}</Text>
                    </Table.Td>
                    <Table.Td style={{ minWidth: 260 }}>
                      <EChart
                        height={40}
                        option={spark(perPeriod((c) => f.kinds.includes(c.kind)), labels, f.colour ?? INK_MUTED)}
                        onClick={(p: Clicked) => {
                          if (p.dataIndex == null) return
                          setFam(f)
                          setKind(null)
                          setPeriod(periods[p.dataIndex])
                          setShown(PAGE)
                        }}
                      />
                    </Table.Td>
                    <Table.Td fw={700}>{n(lastSeven(f))}</Table.Td>
                    <Table.Td c="dimmed">{n(sevenBefore(f))}</Table.Td>
                    <Table.Td>
                      <Change now={lastSeven(f)} then={sevenBefore(f)} />
                    </Table.Td>
                  </Table.Tr>
                )
              })}
            </Table.Tbody>
          </Table>
        </Table.ScrollContainer>
      </Paper>
      <Paper withBorder radius="md" p="sm">
        <Group justify="space-between" align="flex-start" wrap="wrap" gap="xs">
          <Stack gap={0} maw={760}>
            <Text fw={600} fz="sm">{conclusion}</Text>
            <Text fz="xs" c="dimmed">
              {fam ? `${fam.about}; ` : 'the six families of what an agent did wrong; '}
              {measure === 'rate' ? 'per 10 hours at work, so a busy day and a quiet one read alike; ' : ''}click a bar for its cases
            </Text>
          </Stack>
          <Group gap="xs">
            {fam && <Button size="compact-xs" variant="subtle" onClick={() => pickFamily(null)}>every family</Button>}
            <SegmentedControl size="xs" value={unit} onChange={(v) => { setUnit(v as Unit); setPeriod(null) }} data={[{ value: 'day', label: 'by day' }, { value: 'week', label: 'by week' }]} />
            <SegmentedControl size="xs" value={measure} onChange={(v) => setMeasure(v as Measure)} data={[{ value: 'count', label: 'how many' }, { value: 'rate', label: 'per 10 h at work' }]} />
          </Group>
        </Group>
        <EChart
          height={260}
          onClick={(p: Clicked) => {
            if (p.dataIndex == null) return
            const f = fam ?? WRONG.find((x) => x.name === p.seriesName) ?? null
            if (!fam) setFam(f)
            setKind(fam ? (fam.kinds.find((k) => kindName(k) === p.seriesName) ?? null) : null)
            setPeriod(periods[p.dataIndex])
            setShown(PAGE)
          }}
          option={{
            legend: legend(),
            grid: grid(),
            tooltip: tip({
              formatter: (ps: { seriesName: string; value: number; marker: string; dataIndex: number }[]) => {
                const i = ps[0]?.dataIndex ?? 0
                const lines = ps.filter((p) => p.value).map((p) => `${p.marker}${p.seriesName}: ${p.value}`)
                return `<b>${labels[i]}</b> · ${worked[i].toFixed(1)} h at work<br/>${lines.join('<br/>') || 'nothing'}`
              },
            }),
            xAxis: category(labels, { axisLabel: { ...AXIS.axisLabel, showMaxLabel: true, alignMaxLabel: 'right', hideOverlap: true } }),
            yAxis: value(measure === 'rate' ? (v: number) => `${v}` : undefined),
            series,
          }}
        />
      </Paper>
      <SimpleGrid cols={{ base: 1, md: 2 }} spacing="md">
        <Paper withBorder radius="md" p="sm">
          <Text fw={600} fz="sm">
            {byModel.length
              ? `By model, per 10 hours at work: ${byModel[byModel.length - 1].model.replace('claude-', '')} the most, ${(byModel[byModel.length - 1].rate).toFixed(1)}`
              : 'By model, per 10 hours at work'}
          </Text>
          <Text fz="xs" c="dimmed">models with an hour at work or more · click one to see only it, here and in every view</Text>
          <EChart
            height={Math.max(120, 30 + 26 * byModel.length)}
            onClick={(p: Clicked) => p.dataIndex != null && setFocus({ ...focus, model: byModel[p.dataIndex].model })}
            option={{
              grid: grid({ top: 8, right: 120 }),
              tooltip: { ...TOOLTIP, formatter: (p: { dataIndex: number }) => { const m = byModel[p.dataIndex]; return `<b>${m.model}</b><br/>${n(m.count)} cases in ${m.hours.toFixed(1)} h at work: ${m.rate.toFixed(1)} per 10 h` } },
              xAxis: value(),
              yAxis: category(byModel.map((m) => m.model.replace('claude-', '')), { axisLabel: { ...AXIS.axisLabel, fontFamily: MONO } }),
              series: [
                {
                  type: 'bar',
                  data: byModel.map((m) => Number(m.rate.toFixed(2))),
                  barMaxWidth: 16,
                  itemStyle: { color: fam?.colour ?? WRONG_COLOURS[0], borderRadius: [0, 3, 3, 0] },
                  label: { show: true, position: 'right', color: INK_MUTED, fontSize: 10, formatter: (p: { dataIndex: number }) => `${byModel[p.dataIndex].rate.toFixed(1)} · ${n(byModel[p.dataIndex].count)} in ${byModel[p.dataIndex].hours.toFixed(0)} h` },
                },
              ],
            }}
          />
        </Paper>
        <Paper withBorder radius="md" p="sm">
          <Text fw={600} fz="sm">
            {bySession.length ? `By session: ${sessionLabel(sessions.get(bySession[bySession.length - 1][0]), bySession[bySession.length - 1][0])} the most, ${n(bySession[bySession.length - 1][1])}` : 'By session'}
          </Text>
          <Text fz="xs" c="dimmed">the {SESSIONS} sessions with the most · click one to see only it, here and in every view</Text>
          <EChart
            height={Math.max(120, 30 + 26 * bySession.length)}
            onClick={(p: Clicked) => p.dataIndex != null && setFocus({ ...focus, session: bySession[p.dataIndex][0] })}
            option={{
              grid: grid({ top: 8, right: 40 }),
              tooltip: { ...TOOLTIP, formatter: (p: { dataIndex: number }) => { const [id, c] = bySession[p.dataIndex]; return `<b>${sessions.get(id)?.name ?? id}</b><br/>${sessions.get(id)?.folder ?? ''}<br/>${n(c)} cases` } },
              xAxis: value(),
              yAxis: category(bySession.map(([id]) => sessionLabel(sessions.get(id), id)), { axisLabel: { ...AXIS.axisLabel, fontFamily: MONO, fontSize: 10 } }),
              series: [
                {
                  type: 'bar',
                  data: bySession.map(([, c]) => c),
                  barMaxWidth: 16,
                  itemStyle: { color: fam?.colour ?? WRONG_COLOURS[0], borderRadius: [0, 3, 3, 0] },
                  label: { show: true, position: 'right', color: INK_MUTED, fontSize: 10 },
                },
              ],
            }}
          />
        </Paper>
      </SimpleGrid>
      <Paper withBorder radius="md">
        <Group p="xs" gap="xs" wrap="wrap">
          <Text fz="sm" fw={600}>
            {n(picked.length)} {picked.length === 1 ? 'case' : 'cases'}
          </Text>
          {fam && <Badge variant="light" color="grape">{fam.name}</Badge>}
          {kind && <Badge variant="outline" rightSection="×" style={{ cursor: 'pointer' }} onClick={() => setKind(null)}>{kindName(kind)}</Badge>}
          {period && <Badge variant="outline" rightSection="×" style={{ cursor: 'pointer' }} onClick={() => setPeriod(null)}>{unit === 'day' ? period : `week ${label(period)}`}</Badge>}
          <Text fz="xs" c="dimmed">newest first · an alarm is marked · click one for its cause</Text>
        </Group>
        <Table.ScrollContainer minWidth={900}>
          <Table fz="sm" verticalSpacing={4} highlightOnHover>
            <Table.Tbody>
              {picked.length === 0 && (
                <Table.Tr>
                  <Table.Td>
                    <Text c="dimmed" fz="sm">No case here.</Text>
                  </Table.Td>
                </Table.Tr>
              )}
              {picked.slice(0, shown).map((c) => (
                <Fragment key={c.id}>
                  <Table.Tr style={{ cursor: 'pointer' }} onClick={() => setOpened(opened === c.id ? null : c.id)}>
                    <Table.Td ff={MONO} fz="xs" style={{ whiteSpace: 'nowrap' }}>
                      {opened === c.id ? '▾' : '▸'} {day(c.at)}
                    </Table.Td>
                    <Table.Td style={{ whiteSpace: 'nowrap' }}>
                      <Badge size="sm" variant="light" color="grape">{kindName(c.kind)}</Badge>
                      {c.alert && w.alarms.includes(c.alert.kind) && (
                        <Badge size="sm" variant="filled" color={FAIL} ml={4}>alarm</Badge>
                      )}
                    </Table.Td>
                    <Table.Td style={{ overflowWrap: 'anywhere' }}>
                      <Text fz="sm" lineClamp={opened === c.id ? undefined : 2}>{c.what}</Text>
                    </Table.Td>
                    <Table.Td>
                      <Text fz="xs" ff={MONO} td="underline" style={{ overflowWrap: 'anywhere' }} onClick={(e) => { e.stopPropagation(); setFocus({ ...focus, session: c.session }) }}>
                        {sessionLabel(sessions.get(c.session), c.session)}
                      </Text>
                    </Table.Td>
                    <Table.Td style={{ whiteSpace: 'nowrap' }}>{c.cost}</Table.Td>
                  </Table.Tr>
                  {opened === c.id && (
                    <Table.Tr>
                      <Table.Td colSpan={5} bg="dark.7">
                        {c.alert ? <Cause a={c.alert} w={w} open={setOpened} core={core} /> : c.found ? <Lines session={c.session} {...span(c.found)} mark={c.call} core={core} /> : null}
                      </Table.Td>
                    </Table.Tr>
                  )}
                </Fragment>
              ))}
            </Table.Tbody>
          </Table>
        </Table.ScrollContainer>
        {picked.length > shown && (
          <Group justify="center" p="xs">
            <Button size="compact-sm" variant="subtle" onClick={() => setShown(shown + PAGE)}>
              show {Math.min(PAGE, picked.length - shown)} more of {n(picked.length - shown)}
            </Button>
          </Group>
        )}
      </Paper>
    </Stack>
  )
}
