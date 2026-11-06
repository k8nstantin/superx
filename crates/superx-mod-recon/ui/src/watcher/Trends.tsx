import { Fragment, useMemo, useState } from 'react'
import { Badge, Button, Group, Paper, SegmentedControl, SimpleGrid, Stack, Table, Text } from '@mantine/core'
import type { Found, Waste, WasteDay, Watch, WatchSessionRow, WatchTime } from '../api'
import { AXIS, EChart, FAIL, INK_MUTED, MONO, OK, TOOLTIP, UNKNOWN, type Clicked } from '../EChart'
import { category, grid, legend, stacked, tip, value } from '../results/charts'
import { day } from '../time'
import { Cause } from './Alarms'
import {
  addCost,
  amount,
  atWork,
  big,
  casesOf,
  costWords,
  dayOf,
  daysFrom,
  FAMILIES,
  kindName,
  MEASURES,
  measureName,
  mondayOf,
  n,
  tick,
  weekName,
  WRONG,
  WRONG_COLOURS,
  zeroCost,
  type Case,
  type Family,
  type Focus,
  type Kind,
  type Measure,
} from './data'
import { span } from './Found'
import { repoOf } from './Issues'
import { Lines } from './Lines'

// Trends (#573): what went to waste, over time, so the operator sees it get
// better or worse (operator, 2026-11-06: "I want the waste highlighted in
// all charts: lines, tokens"). The waste is the work in the span of a case
// that shows work gone wrong: a task redone, a build stopped short, an
// order broken, a turn you corrected (src/watcher/waste.rs). Every chart
// measures lines written, tokens generated, tokens read, or cases: the
// waste in red against the rest; each family of what went wrong in a row;
// by model and by session; and every case with what its span cost, each
// opening on its cause and the session's lines with their tokens.

type Unit = 'day' | 'week'
type Order = 'waste' | 'newest'

const PAGE = 100 // skill-allow: §9-const — rows drawn at once; the rest a click away
const SESSIONS = 10 // skill-allow: §9-const — bars drawn; the cases list every session
const DAYS_AS_DAYS = 35 // skill-allow: §9-const — a span this long or shorter is drawn day by day, longer week by week
const WASTE = FAIL
const REST = UNKNOWN

/// The day `days` before a day, both as `2026-11-04`.
function before(d: string, days: number): string {
  const t = new Date(`${d}T12:00:00`)
  t.setDate(t.getDate() - days)
  return dayOf(t.toISOString())
}

const pct = (part: number, whole: number) => (whole > 0 ? `${Math.round((100 * part) / whole)}%` : '—')

/// How a count moved, the last seven days against the seven before: more is
/// worse.
function Change({ now, then }: { now: number; then: number }) {
  if (Math.round(now) === Math.round(then)) return <Text fz="xs" c="dimmed">same</Text>
  return (
    <Text fz="xs" fw={700} c={now > then ? FAIL : OK} style={{ whiteSpace: 'nowrap' }}>
      {now > then ? `▲ ${big(now - then)} more` : `▼ ${big(then - now)} less`}
    </Text>
  )
}

/// A row's bars, one per day or week, its own height.
function spark(values: number[], labels: string[], colour: string, unit: string) {
  return {
    grid: { left: 2, right: 2, top: 4, bottom: 2 },
    tooltip: { ...TOOLTIP, trigger: 'axis', axisPointer: { type: 'shadow' }, formatter: (p: { name: string; value: number }[]) => `${p[0].name}: ${big(p[0].value)} ${unit}` },
    xAxis: { type: 'category', data: labels, show: false },
    yAxis: { type: 'value', show: false, max: (v: { max: number }) => Math.max(1, v.max) },
    series: [{ type: 'bar', data: values, barCategoryGap: '25%', itemStyle: { color: colour, borderRadius: [2, 2, 0, 0] } }],
  }
}

/// A session by its repository, its id and its model.
function sessionLabel(s: WatchSessionRow | undefined, id: string): string {
  const short = (s?.name ?? id).split('/').pop()?.slice(0, 8) ?? id
  return `${repoOf(s?.folder ?? null)} · ${short}${s?.model ? ` · ${s.model.replace('claude-', '')}` : ''}`
}

export function Trends({
  w,
  found,
  time,
  waste,
  focus,
  setFocus,
  core,
}: {
  w: Watch
  found: Found[]
  time: WatchTime | undefined
  waste: Waste | undefined
  focus: Focus
  setFocus: (f: Focus) => void
  core: string | null
}) {
  const [fam, setFam] = useState<Family | null>(null)
  const [kind, setKind] = useState<Kind | null>(null)
  const [period, setPeriod] = useState<string | null>(null)
  const [measure, setMeasure] = useState<Measure>('lines')
  const [unitPicked, setUnit] = useState<Unit | null>(null)
  const [order, setOrder] = useState<Order>('waste')
  const [opened, setOpened] = useState<string | null>(null)
  const [shown, setShown] = useState(PAGE)
  const sessions = useMemo(() => new Map(w.sessions.map((s) => [s.id, s])), [w])
  const modelOf = (session: string) => sessions.get(session)?.model ?? 'unknown'
  const inFocus = (session: string) => (!focus.session || focus.session === session) && (!focus.model || focus.model === modelOf(session))
  const cases = useMemo(() => casesOf(w.alerts, found), [w, found]).filter((c) => inFocus(c.session))
  const days: WasteDay[] = (waste?.days ?? []).filter((d) => inFocus(d.session) && (!focus.model || (d.model ?? 'unknown') === focus.model))
  const rows = (time?.rows ?? []).filter((r) => inFocus(r.session))
  const costOf = (c: Case) => waste?.cases[c.id]
  const counting = measure === 'cases'
  const unitWord = counting ? 'cases' : measureName(measure)
  const today = dayOf(new Date().toISOString())
  const first = [...cases.map((c) => dayOf(c.at)), ...days.map((d) => d.day)].reduce((a, b) => (b < a ? b : a), today)
  const allDays = daysFrom(first, today)
  const unit: Unit = unitPicked ?? (allDays.length <= DAYS_AS_DAYS ? 'day' : 'week')
  const periodOf = (d: string) => (unit === 'day' ? d : mondayOf(d))
  const periods = [...new Set(allDays.map(periodOf))]
  const index = new Map(periods.map((p, i) => [p, i]))
  const label = (p: string) => (unit === 'day' ? p.slice(5) : weekName(p, today))
  const labels = periods.map(label)
  const week = before(today, 6)
  const [b0, b1] = [before(today, 13), before(today, 7)]
  const inWeek = (d: string) => d >= week
  const inBefore = (d: string) => d >= b0 && d <= b1
  // Per period, and over the last 7 days and the 7 before: everything, the
  // waste, and a family's waste, in the measure; cases counted.
  const series = (pick: (d: WasteDay) => number, count: (c: Case) => boolean) => {
    const out = new Array<number>(periods.length).fill(0)
    if (counting) {
      for (const c of cases) if (count(c)) out[index.get(periodOf(dayOf(c.at))) ?? -1] += 1
    } else {
      for (const d of days) {
        const i = index.get(periodOf(d.day))
        if (i != null) out[i] += pick(d)
      }
    }
    return out.map((v) => v || 0)
  }
  const sum = (pick: (d: WasteDay) => number, count: (c: Case) => boolean, keep: (day: string) => boolean) =>
    counting ? cases.filter((c) => count(c) && keep(dayOf(c.at))).length : days.filter((d) => keep(d.day)).reduce((t, d) => t + pick(d), 0)
  const wasteKinds = new Set(WRONG.flatMap((f) => f.kinds))
  const isWaste = (c: Case) => (waste ? !!costOf(c)?.waste : wasteKinds.has(c.kind))
  const all = { pick: (d: WasteDay) => amount(d.all, measure), count: (_c: Case) => true }
  const wasted = { pick: (d: WasteDay) => amount(d.wasted, measure), count: isWaste }
  const ofFamily = (f: Family) => ({ pick: (d: WasteDay) => amount(d.families[f.key], measure), count: (c: Case) => f.kinds.includes(c.kind) })
  const totals = (s: { pick: (d: WasteDay) => number; count: (c: Case) => boolean }) => [sum(s.pick, s.count, inWeek), sum(s.pick, s.count, inBefore)]
  const [allNow, allThen] = totals(all)
  const [wasteNow, wasteThen] = totals(wasted)
  // The headline: lines and tokens generated, the last 7 days.
  const line = (m: Measure) => {
    const a = days.filter((d) => inWeek(d.day)).reduce((t, d) => t + amount(d.all, m), 0)
    const x = days.filter((d) => inWeek(d.day)).reduce((t, d) => t + amount(d.wasted, m), 0)
    const a0 = days.filter((d) => inBefore(d.day)).reduce((t, d) => t + amount(d.all, m), 0)
    const x0 = days.filter((d) => inBefore(d.day)).reduce((t, d) => t + amount(d.wasted, m), 0)
    return { a, x, a0, x0 }
  }
  const lines = line('lines')
  const gen = line('generated')
  // The cases picked, with what their spans cost.
  const inPick = (c: Case) => (fam ? fam.kinds.includes(c.kind) : isWaste(c))
  const picked = cases
    .filter((c) => inPick(c) && (!kind || c.kind === kind) && (!period || periodOf(dayOf(c.at)) === period) && (!focus.day || dayOf(c.at) === focus.day))
    .sort((a, b) => (order === 'newest' || counting ? b.at.localeCompare(a.at) : amount(costOf(b)?.cost, measure) - amount(costOf(a)?.cost, measure) || b.at.localeCompare(a.at)))
  const pickFamily = (f: Family | null) => {
    setFam(f)
    setKind(null)
    setPeriod(null)
    setShown(PAGE)
  }
  // The big chart: the waste in red against the rest; a family's waste in
  // its colour against the rest of the work.
  const shownWaste = fam ? series(ofFamily(fam).pick, ofFamily(fam).count) : series(wasted.pick, wasted.count)
  const everything = series(all.pick, all.count)
  const rest = everything.map((v, i) => Math.max(0, v - shownWaste[i]))
  const wasteName = fam ? `${fam.name}: went to waste` : 'went to waste'
  const bigSeries = counting
    ? fam
      ? fam.kinds.map((k, i) => stacked(kindName(k), series(() => 0, (c) => c.kind === k), WRONG_COLOURS[i % WRONG_COLOURS.length], 'k', { barMaxWidth: 26 }))
      : WRONG.map((f) => stacked(f.name, series(() => 0, (c) => f.kinds.includes(c.kind)), f.colour ?? INK_MUTED, 'k', { barMaxWidth: 26 }))
    : [stacked(wasteName, shownWaste, fam?.colour ?? WASTE, 'k', { barMaxWidth: 26 }), stacked('the rest of the work', rest, REST, 'k', { barMaxWidth: 26 })]
  const wasteTotal = shownWaste.reduce((a, b) => a + b, 0)
  const allTotal = everything.reduce((a, b) => a + b, 0)
  const worst = shownWaste.reduce((best, v, i, xs) => (v > xs[best] ? i : best), 0)
  const what = fam ? fam.name : 'What went to waste'
  const conclusion = counting
    ? `${what}, ${unit === 'day' ? 'day by day' : 'week by week'}: ${n(picked.length)} cases`
    : `${what}, ${unit === 'day' ? 'day by day' : 'week by week'}: ${big(wasteTotal)} of ${big(allTotal)} ${unitWord} (${pct(wasteTotal, allTotal)}); the most ${unit === 'day' ? 'on' : 'in the week of'} ${labels[worst]}, ${big(shownWaste[worst] ?? 0)} of ${big(everything[worst] ?? 0)}.`
  // By model: the share of its work that went to waste; by session: its
  // waste against the rest of its work.
  const byModel = useMemo(() => {
    if (counting) {
      const hours = new Map<string, number>()
      for (const r of rows) hours.set(r.model ?? 'unknown', (hours.get(r.model ?? 'unknown') ?? 0) + atWork(r.split) / 3600)
      const counts = new Map<string, number>()
      for (const c of cases.filter(inPick)) counts.set(modelOf(c.session), (counts.get(modelOf(c.session)) ?? 0) + 1)
      return [...hours.entries()]
        .filter(([, h]) => h >= 1)
        .map(([model, h]) => ({ model, value: (10 * (counts.get(model) ?? 0)) / h, label: `${((10 * (counts.get(model) ?? 0)) / h).toFixed(1)} per 10 h · ${n(counts.get(model) ?? 0)} in ${h.toFixed(0)} h` }))
        .sort((a, b) => a.value - b.value)
    }
    const by = new Map<string, [number, number]>()
    for (const d of days) {
      const m = d.model ?? 'unknown'
      const [x, a] = by.get(m) ?? [0, 0]
      by.set(m, [x + (fam ? amount(d.families[fam.key], measure) : amount(d.wasted, measure)), a + amount(d.all, measure)])
    }
    return [...by.entries()]
      .filter(([, [, a]]) => a > 0)
      .map(([model, [x, a]]) => ({ model, value: (100 * x) / a, label: `${pct(x, a)} · ${big(x)} of ${big(a)}` }))
      .sort((a, b) => a.value - b.value)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rows, cases, days, fam, measure])
  const bySession = (() => {
    const by = new Map<string, [number, number]>()
    if (counting) {
      for (const c of cases.filter(inPick)) by.set(c.session, [(by.get(c.session)?.[0] ?? 0) + 1, 0])
    } else {
      for (const d of days) {
        const [x, a] = by.get(d.session) ?? [0, 0]
        by.set(d.session, [x + (fam ? amount(d.families[fam.key], measure) : amount(d.wasted, measure)), a + amount(d.all, measure)])
      }
    }
    return [...by.entries()].filter(([, [x]]) => x > 0).sort((a, b) => b[1][0] - a[1][0]).slice(0, SESSIONS).reverse()
  })()
  const familyNow = (f: Family) => totals(ofFamily(f))
  const pickBar = (f: Family | null, i: number, k: Kind | null = null) => {
    setFam(f)
    setKind(k)
    setPeriod(periods[i])
    setShown(PAGE)
  }
  const wastedSum = days.reduce((c, d) => addCost(c, d.wasted), zeroCost())
  return (
    <Stack gap="md">
      {waste ? (
        <Paper withBorder radius="md" p="sm" style={{ borderColor: WASTE }}>
          <Text fz="lg" fw={700} style={{ textWrap: 'balance' }}>
            In the last 7 days the agents wrote {n(lines.a)} lines and generated {big(gen.a)} tokens.{' '}
            <Text span inherit c={WASTE}>
              {n(lines.x)} lines ({pct(lines.x, lines.a)}) and {big(gen.x)} tokens ({pct(gen.x, gen.a)}) went to waste
            </Text>
            , against {pct(lines.x0, lines.a0)} of the lines and {pct(gen.x0, gen.a0)} of the tokens the 7 days before.
          </Text>
          <Text fz="xs" c="dimmed" mt={4}>
            Waste is the work in the span of a case that shows work gone wrong: a task redone or undone, a build stopped short, an order broken, a fix short of what it
            found, an act on its own, a turn you corrected or stopped, effort spent again. Since {first}: {costWords(wastedSum)} went to waste.
          </Text>
        </Paper>
      ) : (
        <Text fz="sm" c="dimmed">Measuring the waste…</Text>
      )}
      <Group justify="space-between" wrap="wrap" gap="xs">
        <SegmentedControl size="xs" value={measure} onChange={(v) => setMeasure(v as Measure)} data={MEASURES.map(([value, label]) => ({ value, label }))} />
        <SegmentedControl size="xs" value={unit} onChange={(v) => { setUnit(v as Unit); setPeriod(null) }} data={[{ value: 'day', label: 'by day' }, { value: 'week', label: 'by week' }]} />
      </Group>
      <Paper withBorder radius="md">
        <Table.ScrollContainer minWidth={760}>
          <Table fz="sm" verticalSpacing={2} highlightOnHover>
            <Table.Thead>
              <Table.Tr>
                <Table.Th>{counting ? 'What the agents did' : `${measureName(measure)} that went to waste, by what went wrong`}</Table.Th>
                <Table.Th>every {unit} since {first.slice(5)}</Table.Th>
                <Table.Th style={{ whiteSpace: 'nowrap' }}>last 7 days</Table.Th>
                <Table.Th style={{ whiteSpace: 'nowrap' }}>the 7 before</Table.Th>
                <Table.Th />
              </Table.Tr>
            </Table.Thead>
            <Table.Tbody>
              {!counting && (
                <Table.Tr bg={!fam ? 'dark.5' : undefined}>
                  <Table.Td w={300} style={{ cursor: 'pointer' }} onClick={() => pickFamily(null)}>
                    <Group gap={6} wrap="nowrap">
                      <span style={{ width: 10, height: 10, borderRadius: 2, flex: 'none', background: WASTE }} />
                      <Text fz="sm" fw={800} c={WASTE}>All the waste</Text>
                    </Group>
                    <Text fz={11} c="dimmed">each line and token once, of {big(allNow)} {unitWord} in the last 7 days ({pct(wasteNow, allNow)})</Text>
                  </Table.Td>
                  <Table.Td style={{ minWidth: 260 }}>
                    <EChart height={40} option={spark(series(wasted.pick, wasted.count), labels, WASTE, unitWord)} onClick={(p: Clicked) => p.dataIndex != null && pickBar(null, p.dataIndex)} />
                  </Table.Td>
                  <Table.Td fw={800} c={WASTE}>{big(wasteNow)}</Table.Td>
                  <Table.Td c="dimmed">{big(wasteThen)}</Table.Td>
                  <Table.Td>
                    <Change now={wasteNow} then={wasteThen} />
                  </Table.Td>
                </Table.Tr>
              )}
              {FAMILIES.map((f) => {
                const mine = fam?.key === f.key
                const [now, then] = familyNow(f)
                const count = cases.filter((c) => f.kinds.includes(c.kind) && inWeek(dayOf(c.at))).length
                const mineAll = cases.filter((c) => f.kinds.includes(c.kind))
                const none = !counting && mineAll.length > 0 && !mineAll.some((c) => costOf(c)?.waste)
                return (
                  <Table.Tr key={f.key} bg={mine ? 'dark.5' : undefined}>
                    <Table.Td w={300} style={{ cursor: 'pointer' }} onClick={() => pickFamily(mine ? null : f)}>
                      <Group gap={6} wrap="nowrap">
                        <span style={{ width: 10, height: 10, borderRadius: 2, flex: 'none', background: f.colour ?? INK_MUTED }} />
                        <Text fz="sm" fw={f.colour ? 700 : 500}>{f.name}</Text>
                      </Group>
                      <Text fz={11} c="dimmed" lineClamp={2}>
                        {n(count)} {count === 1 ? 'case' : 'cases'} in 7 days · {none ? `${f.about}: counted, no work of their own wasted` : f.about}
                      </Text>
                    </Table.Td>
                    <Table.Td style={{ minWidth: 260 }}>
                      <EChart
                        height={40}
                        option={spark(series(ofFamily(f).pick, ofFamily(f).count), labels, f.colour ?? INK_MUTED, unitWord)}
                        onClick={(p: Clicked) => p.dataIndex != null && pickBar(f, p.dataIndex)}
                      />
                    </Table.Td>
                    <Table.Td fw={700}>{big(now)}</Table.Td>
                    <Table.Td c="dimmed">{big(then)}</Table.Td>
                    <Table.Td>
                      <Change now={now} then={then} />
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
          <Stack gap={0} maw={820}>
            <Text fw={600} fz="sm">{conclusion}</Text>
            <Text fz="xs" c="dimmed">{counting ? 'the cases of each family; ' : `${measureName(measure)}: the waste in ${fam ? 'its colour' : 'red'} against the rest of the work; `}click a bar for its cases</Text>
          </Stack>
          {fam && <Button size="compact-xs" variant="subtle" onClick={() => pickFamily(null)}>all the waste</Button>}
        </Group>
        <EChart
          height={260}
          onClick={(p: Clicked) => {
            if (p.dataIndex == null) return
            if (counting && !fam) pickBar(WRONG.find((x) => x.name === p.seriesName) ?? null, p.dataIndex)
            else if (counting && fam) pickBar(fam, p.dataIndex, fam.kinds.find((k) => kindName(k) === p.seriesName) ?? null)
            else pickBar(fam, p.dataIndex)
          }}
          option={{
            legend: legend(),
            grid: grid(),
            tooltip: tip({
              formatter: (ps: { seriesName: string; value: number; marker: string; dataIndex: number }[]) => {
                const i = ps[0]?.dataIndex ?? 0
                const parts = ps.filter((p) => p.value).map((p) => `${p.marker}${p.seriesName}: ${big(p.value)}`)
                const share = counting ? '' : ` · ${pct(shownWaste[i], everything[i])} went to waste`
                return `<b>${labels[i]}</b>${share}<br/>${parts.join('<br/>') || 'nothing'}`
              },
            }),
            xAxis: category(labels, { axisLabel: { ...AXIS.axisLabel, showMaxLabel: true, alignMaxLabel: 'right', hideOverlap: true } }),
            yAxis: value((v: number) => tick(v)),
            series: bigSeries,
          }}
        />
      </Paper>
      <SimpleGrid cols={{ base: 1, md: 2 }} spacing="md">
        <Paper withBorder radius="md" p="sm">
          <Text fw={600} fz="sm">
            {byModel.length
              ? counting
                ? `By model, cases per 10 hours at work: ${byModel[byModel.length - 1].model.replace('claude-', '')} the most`
                : `By model, the share of its ${measureName(measure)} that went to waste: ${byModel[byModel.length - 1].model.replace('claude-', '')} the most, ${byModel[byModel.length - 1].label}`
              : 'By model'}
          </Text>
          <Text fz="xs" c="dimmed">click one to see only it, here and in every view</Text>
          <EChart
            height={Math.max(120, 30 + 26 * byModel.length)}
            onClick={(p: Clicked) => p.dataIndex != null && setFocus({ ...focus, model: byModel[p.dataIndex].model })}
            option={{
              grid: grid({ top: 8, right: 170 }),
              tooltip: { ...TOOLTIP, formatter: (p: { dataIndex: number }) => `<b>${byModel[p.dataIndex].model}</b><br/>${byModel[p.dataIndex].label}` },
              xAxis: value(counting ? undefined : (v: number) => `${v}%`, counting ? {} : { max: 100 }),
              yAxis: category(byModel.map((m) => m.model.replace('claude-', '')), { axisLabel: { ...AXIS.axisLabel, fontFamily: MONO } }),
              series: [
                {
                  type: 'bar',
                  data: byModel.map((m) => Number(m.value.toFixed(1))),
                  barMaxWidth: 16,
                  itemStyle: { color: fam?.colour ?? WASTE, borderRadius: [0, 3, 3, 0] },
                  label: { show: true, position: 'right', color: INK_MUTED, fontSize: 10, formatter: (p: { dataIndex: number }) => byModel[p.dataIndex].label },
                },
              ],
            }}
          />
        </Paper>
        <Paper withBorder radius="md" p="sm">
          <Text fw={600} fz="sm">
            {bySession.length
              ? `By session: ${sessionLabel(sessions.get(bySession[bySession.length - 1][0]), bySession[bySession.length - 1][0])} the most, ${big(bySession[bySession.length - 1][1][0])} ${unitWord}`
              : 'By session'}
          </Text>
          <Text fz="xs" c="dimmed">the {SESSIONS} sessions with the most waste{counting ? '' : ', against the rest of their work'} · click one to see only it</Text>
          <EChart
            height={Math.max(120, 30 + 26 * bySession.length)}
            onClick={(p: Clicked) => p.dataIndex != null && setFocus({ ...focus, session: bySession[p.dataIndex][0] })}
            option={{
              legend: counting ? undefined : legend(),
              grid: grid({ top: counting ? 8 : 30, right: 30 }),
              tooltip: {
                ...TOOLTIP,
                formatter: (p: { dataIndex: number }) => {
                  const [id, [x, a]] = bySession[p.dataIndex]
                  return `<b>${sessions.get(id)?.name ?? id}</b><br/>${sessions.get(id)?.folder ?? ''}<br/>${counting ? `${n(x)} cases` : `${big(x)} of ${big(a)} ${unitWord} went to waste (${pct(x, a)})`}`
                },
              },
              xAxis: value((v: number) => tick(v), { splitNumber: 3 }),
              yAxis: category(bySession.map(([id]) => sessionLabel(sessions.get(id), id)), { axisLabel: { ...AXIS.axisLabel, fontFamily: MONO, fontSize: 10 } }),
              series: counting
                ? [{ type: 'bar', data: bySession.map(([, [x]]) => x), barMaxWidth: 16, itemStyle: { color: fam?.colour ?? WASTE, borderRadius: [0, 3, 3, 0] } }]
                : [
                    stacked('went to waste', bySession.map(([, [x]]) => x), fam?.colour ?? WASTE, 's', { barMaxWidth: 16 }),
                    stacked('the rest of its work', bySession.map(([, [x, a]]) => Math.max(0, a - x)), REST, 's', { barMaxWidth: 16 }),
                  ],
            }}
          />
        </Paper>
      </SimpleGrid>
      <Paper withBorder radius="md">
        <Group p="xs" gap="xs" wrap="wrap" justify="space-between">
          <Group gap="xs" wrap="wrap">
            <Text fz="sm" fw={600}>
              {n(picked.length)} {picked.length === 1 ? 'case' : 'cases'}
            </Text>
            {fam && <Badge variant="light" color="grape">{fam.name}</Badge>}
            {kind && <Badge variant="outline" rightSection="×" style={{ cursor: 'pointer' }} onClick={() => setKind(null)}>{kindName(kind)}</Badge>}
            {period && <Badge variant="outline" rightSection="×" style={{ cursor: 'pointer' }} onClick={() => setPeriod(null)}>{unit === 'day' ? period : `week ${label(period)}`}</Badge>}
            <Text fz="xs" c="dimmed">each with what its span cost · an alarm is marked · click one for its cause and its lines</Text>
          </Group>
          {!counting && <SegmentedControl size="xs" value={order} onChange={(v) => setOrder(v as Order)} data={[{ value: 'waste', label: 'most waste first' }, { value: 'newest', label: 'newest first' }]} />}
        </Group>
        <Table.ScrollContainer minWidth={980}>
          <Table fz="sm" verticalSpacing={4} highlightOnHover>
            <Table.Tbody>
              {picked.length === 0 && (
                <Table.Tr>
                  <Table.Td>
                    <Text c="dimmed" fz="sm">No case here.</Text>
                  </Table.Td>
                </Table.Tr>
              )}
              {picked.slice(0, shown).map((c) => {
                const cc = costOf(c)
                return (
                  <Fragment key={c.id}>
                    <Table.Tr style={{ cursor: 'pointer' }} onClick={() => setOpened(opened === c.id ? null : c.id)}>
                      <Table.Td ff={MONO} fz="xs" style={{ whiteSpace: 'nowrap' }}>
                        {opened === c.id ? '▾' : '▸'} {day(c.at)}
                      </Table.Td>
                      <Table.Td w={170}>
                        <Stack gap={2} align="flex-start">
                          <Badge size="sm" variant="light" color="grape" maw={168}>{kindName(c.kind)}</Badge>
                          {c.alert && w.alarms.includes(c.alert.kind) && <Badge size="xs" variant="filled" color={FAIL}>alarm</Badge>}
                        </Stack>
                      </Table.Td>
                      <Table.Td style={{ overflowWrap: 'anywhere' }}>
                        <Text fz="sm" lineClamp={opened === c.id ? undefined : 2}>{c.what}</Text>
                      </Table.Td>
                      <Table.Td>
                        <Text fz="xs" ff={MONO} td="underline" style={{ overflowWrap: 'anywhere' }} onClick={(e) => { e.stopPropagation(); setFocus({ ...focus, session: c.session }) }}>
                          {sessionLabel(sessions.get(c.session), c.session)}
                        </Text>
                      </Table.Td>
                      <Table.Td style={{ whiteSpace: 'nowrap' }}>
                        {cc ? (
                          <Stack gap={0}>
                            <Text fz="xs" fw={700} c={cc.waste ? WASTE : undefined}>
                              {n(cc.cost.added)} lines · {big(cc.cost.tokens.generated)} tokens
                            </Text>
                            <Text fz={10} c="dimmed">{cc.waste ? 'went to waste' : 'spent in its span'} · {big(cc.cost.tokens.fresh + cc.cost.tokens.cache_written + cc.cost.tokens.cache_read)} read</Text>
                          </Stack>
                        ) : (
                          <Text fz="xs" c="dimmed">{c.cost}</Text>
                        )}
                      </Table.Td>
                    </Table.Tr>
                    {opened === c.id && (
                      <Table.Tr>
                        <Table.Td colSpan={5} bg="dark.7">
                          {cc && <Text fz="xs" c={cc.waste ? WASTE : 'dimmed'} mb={4}>{cc.waste ? 'went to waste in its span' : 'spent in its span'}: {costWords(cc.cost)}</Text>}
                          {c.alert ? <Cause a={c.alert} w={w} open={setOpened} core={core} /> : c.found ? <Lines session={c.session} {...span(c.found)} mark={c.call} core={core} /> : null}
                        </Table.Td>
                      </Table.Tr>
                    )}
                  </Fragment>
                )
              })}
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
