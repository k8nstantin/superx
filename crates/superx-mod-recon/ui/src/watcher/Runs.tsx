import { Fragment, useState } from 'react'
import { Paper, Stack, Table, Text } from '@mantine/core'
import type { Watch, WatchRun } from '../api'
import { CANCEL, EChart, FAIL, INK_MUTED, MONO, OK, UNKNOWN, type Clicked } from '../EChart'
import { grid, legend, tip, value } from '../results/charts'
import { day } from '../time'
import { dayOf, KIND, took, type Focus } from './data'
import { around, Lines } from './Lines'

// Every job the agents launched and every run of the repeat bound or more:
// when it started and how long it ran, how it ended, and whether an alert
// was raised on it. Long running means problems (operator, 2026-11-05).

const STATE: [string, string, (r: WatchRun) => boolean][] = [
  ['ended well', OK, (r) => r.end != null && !r.failed && (r.status == null || r.status === 'completed')],
  ['failed or killed', FAIL, (r) => r.end != null && (r.failed || (r.status != null && r.status !== 'completed'))],
  ['still running', CANCEL, (r) => r.end == null],
]

export function Runs({ w, focus, core }: { w: Watch; focus: Focus; core: string | null }) {
  const [opened, setOpened] = useState<string | null>(null)
  const runs = w.runs.filter((r) => (!focus.session || r.session === focus.session) && (!focus.day || dayOf(r.start) === focus.day) && (!focus.model || (r.model ?? 'unknown') === focus.model))
  const alerted = new Map(w.alerts.filter((a) => a.call).map((a) => [`${a.session}/${a.call}`, a]))
  const long = runs.filter((r) => r.secs >= w.long_secs)
  const open = (r: WatchRun) => {
    const id = `${r.session}/${r.call}`
    setOpened(id)
    requestAnimationFrame(() => document.getElementById(`run-${id}`)?.scrollIntoView({ behavior: 'smooth', block: 'center' }))
  }
  return (
    <Stack gap="md">
      <Text fz="sm" c="dimmed">
        {runs.length} runs: {long.length} ran past {took(w.long_secs)}, {took(long.reduce((s, r) => s + r.secs, 0))} between them. Click a dot or a row for the session around it.
      </Text>
      <Paper withBorder radius="md" p="sm">
        <Text fw={600} fz="sm">How long each run took, by when it started</Text>
        <Text fz="xs" c="dimmed">each dot a run; the line is the long bound</Text>
        <EChart
          height={300}
          onClick={(p: Clicked & { data?: unknown[] }) => {
            const i = Array.isArray(p.data) ? (p.data[2] as number) : null
            if (i != null && runs[i]) open(runs[i])
          }}
          option={{
            legend: legend(),
            grid: grid({ bottom: 24 }),
            tooltip: {
              ...tip({ trigger: 'item' }),
              formatter: (p: { data: [number, number, number] }) => {
                const r = runs[p.data[2]]
                return `<b>${r.label}</b><br/>${day(r.start)} · ${took(r.secs)} · ${r.name}`
              },
            },
            xAxis: { type: 'time', axisLine: { lineStyle: { color: UNKNOWN } }, axisLabel: { color: INK_MUTED, fontSize: 11 }, splitLine: { show: false } },
            yAxis: value((v) => `${v} h`),
            series: STATE.map(([name, colour, is]) => ({
              name,
              type: 'scatter',
              symbolSize: 9,
              itemStyle: { color: colour, borderColor: '#2A1235', borderWidth: 1 },
              data: runs.map((r, i) => [new Date(r.start).getTime(), Number((r.secs / 3600).toFixed(2)), i, is(r)]).filter((d) => d[3]).map((d) => d.slice(0, 3)),
              markLine:
                name === 'ended well'
                  ? { silent: true, symbol: 'none', lineStyle: { color: CANCEL, type: 'dashed' }, label: { color: CANCEL, formatter: `long: ${took(w.long_secs)}` }, data: [{ yAxis: w.long_secs / 3600 }] }
                  : undefined,
            })),
          }}
        />
      </Paper>
      <Paper withBorder radius="md">
        <Table.ScrollContainer minWidth={900}>
          <Table fz="sm" verticalSpacing={4} highlightOnHover>
            <Table.Thead>
              <Table.Tr>
                <Table.Th>Started</Table.Th>
                <Table.Th>Took</Table.Th>
                <Table.Th>Run</Table.Th>
                <Table.Th>Ended</Table.Th>
                <Table.Th>Alert</Table.Th>
                <Table.Th>Session</Table.Th>
              </Table.Tr>
            </Table.Thead>
            <Table.Tbody>
              {runs.map((r) => {
                const id = `${r.session}/${r.call}`
                const a = alerted.get(id)
                const [from, to] = around(r.start, r.end ?? r.start, 600, 120)
                return (
                  <Fragment key={id}>
                    <Table.Tr id={`run-${id}`} style={{ cursor: 'pointer' }} onClick={() => setOpened(opened === id ? null : id)}>
                      <Table.Td ff={MONO} fz="xs" style={{ whiteSpace: 'nowrap' }}>{opened === id ? '▾' : '▸'} {day(r.start)}</Table.Td>
                      <Table.Td style={{ whiteSpace: 'nowrap' }}>
                        <Text span fz="sm" fw={r.secs >= w.long_secs ? 700 : 400} c={r.secs >= w.long_secs ? CANCEL : undefined}>{took(r.secs)}</Text>
                      </Table.Td>
                      <Table.Td style={{ overflowWrap: 'anywhere' }}>{r.label}</Table.Td>
                      <Table.Td>{r.end == null ? 'running' : r.failed ? 'failed' : (r.status ?? 'done')}</Table.Td>
                      <Table.Td>{a ? <Text fz="xs" c={KIND[a.kind][1]}>{KIND[a.kind][0]}</Text> : '—'}</Table.Td>
                      <Table.Td><Text fz="xs" ff={MONO} style={{ overflowWrap: 'anywhere' }}>{r.name}</Text></Table.Td>
                    </Table.Tr>
                    {opened === id && (
                      <Table.Tr>
                        <Table.Td colSpan={6} bg="dark.7">
                          <Lines session={r.session} from={from} to={to} mark={r.call} core={core} />
                        </Table.Td>
                      </Table.Tr>
                    )}
                  </Fragment>
                )
              })}
            </Table.Tbody>
          </Table>
        </Table.ScrollContainer>
      </Paper>
    </Stack>
  )
}
