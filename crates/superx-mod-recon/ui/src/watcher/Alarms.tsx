import { Fragment, useState } from 'react'
import { Badge, Button, Group, Paper, Stack, Table, Text } from '@mantine/core'
import type { Alongside, Unanswered, Watch, WatchAlert, WatchTask } from '../api'
import { EChart, FAIL, MONO, type Clicked } from '../EChart'
import { category, grid, legend, stacked, tip, value } from '../results/charts'
import { clock, day, since } from '../time'
import { dayOf, KIND, short, took, WRONG, type Focus } from './data'
import { around, Lines } from './Lines'
import { History } from './Tasks'

// The alarms: the critical alerts alone, the kinds attr_recon_watch_alarms
// names (operator, 2026-11-06: "sooner or later the alarms will flood and
// the operator will ignore them … very defined and critical"); every other
// alert is charted in Trends. One table, newest first: each opens on its
// cause. What it is about and what it cost; the alert that caused it and the
// ones it led to; for a plan, the gates that launch the same job; for a task
// that never ends, its merges, reopenings and estimates; and the session's
// own lines from where its cause starts to the alert.

/// The task an alert of a task that never ends is about.
function taskOf(a: WatchAlert, tasks: WatchTask[]): WatchTask | undefined {
  const branch = a.id.split('/endless_task/')[1]?.split('/').slice(0, -1).join('/')
  return tasks.find((t) => t.session === a.session && t.branch === branch)
}

/// The stretches the API did not answer, step by step in your time, and the
/// other sessions it left without an answer in the same minutes.
function NoAnswer({ stretches, alongside, pick }: { stretches: Unanswered[]; alongside: Alongside[]; pick: (session: string) => void }) {
  const steps: [string, string | null][] = stretches.flatMap((u, i): [string, string | null][] => [
    [`${stretches.length > 1 ? `${i + 1}. ` : ''}first request with no answer${u.model ? `, ${u.model}${u.effort ? ` at effort ${u.effort}` : ''}` : ''}`, u.since],
    [`sent again ${u.resent} ${u.resent === 1 ? 'time' : 'times'}, the first`, u.resent ? u.first_resent : null],
    ['you interrupted it', u.interrupted],
    [`Claude Code gave up${u.failures > 1 ? ` (${u.failures} times)` : ''}${u.error ? `: ${u.error}` : ''}`, u.failed_at],
    ['its turn ended there', u.turn_ended],
    [`the newest of ${u.sent} requests sent`, u.last],
    ['the answer came', u.answered],
  ])
  return (
    <Paper withBorder p={6} radius="sm">
      <Stack gap={2}>
        {steps
          .filter(([, at]) => at)
          .sort((x, y) => (x[1] ?? '').localeCompare(y[1] ?? ''))
          .map(([what, at], i) => (
            <Group key={`${i}-${what}`} gap="xs" wrap="nowrap" align="flex-start">
              <Text fz="xs" ff={MONO} c="dimmed" w={64} style={{ flex: 'none' }}>{clock(at)}</Text>
              <Text fz="xs" style={{ overflowWrap: 'anywhere' }}>{what}</Text>
            </Group>
          ))}
        {alongside.length > 0 && (
          <Text fz="xs" fw={600} mt={4}>
            The API left {alongside.length} other {alongside.length === 1 ? 'session' : 'sessions'} without an answer in the same minutes:
          </Text>
        )}
        {alongside.map((o) => (
          <Group key={o.session + o.since} gap="xs" wrap="nowrap">
            <Text fz="xs" ff={MONO} c="dimmed" w={130} style={{ flex: 'none' }}>
              {clock(o.since)}–{clock(o.until)}
            </Text>
            <Text fz="xs" ff={MONO} td="underline" style={{ cursor: 'pointer', overflowWrap: 'anywhere' }} onClick={() => pick(o.session)}>
              {o.name}
            </Text>
            <Text fz="xs" c="dimmed">{since(o.since, o.until)}</Text>
          </Group>
        ))}
      </Stack>
    </Paper>
  )
}

export function Cause({ a, w, open, core }: { a: WatchAlert; w: Watch; open: (id: string) => void; core: string | null }) {
  const caused = a.cause ? w.alerts.find((x) => x.id === a.cause) : undefined
  const led = w.alerts.filter((x) => x.cause === a.id)
  const task = a.kind === 'endless_task' ? taskOf(a, w.tasks) : undefined
  const [from, to] = around(a.from, a.at)
  return (
    <Stack gap="xs" py={4}>
      <Group gap="lg" wrap="wrap">
        {a.since && <Text fz="xs">since {day(a.since)}</Text>}
        {a.cost_secs != null && <Text fz="xs">at stake: {took(a.cost_secs)}</Text>}
        {a.launch && (
          <Text fz="xs" ff={MONO} style={{ overflowWrap: 'anywhere' }}>
            launch: {a.launch}
          </Text>
        )}
      </Group>
      {caused && (
        <Group gap="xs">
          <Text fz="xs" fw={600}>Caused by:</Text>
          <Badge size="xs" color={KIND[caused.kind][1]}>{KIND[caused.kind][0]}</Badge>
          <Text fz="xs">{caused.what}</Text>
          <Button size="compact-xs" variant="subtle" onClick={() => open(caused.id)}>open it</Button>
        </Group>
      )}
      {led.map((x) => (
        <Group key={x.id} gap="xs">
          <Text fz="xs" fw={600}>Led to:</Text>
          <Badge size="xs" color={KIND[x.kind][1]}>{KIND[x.kind][0]}</Badge>
          <Text fz="xs">{x.what}</Text>
          <Button size="compact-xs" variant="subtle" onClick={() => open(x.id)}>open it</Button>
        </Group>
      ))}
      {a.repeats.map((r) => (
        <Paper key={r.job} withBorder p={6} radius="sm">
          <Text fz="xs" fw={600}>
            {r.times} launches of <Text span ff={MONO} fz="xs">{r.job}</Text>, by:
          </Text>
          {r.by.map((g, i) => (
            <Text key={i} fz="xs" pl="sm">
              {g}
            </Text>
          ))}
        </Paper>
      ))}
      {task && <History t={task} />}
      {a.stretches.length > 0 && <NoAnswer stretches={a.stretches} alongside={a.alongside} pick={(s) => open(w.alerts.find((x) => x.session === s && x.kind === 'api_stalled')?.id ?? a.id)} />}
      {a.kind !== 'endless_task' && a.evidence.length > 0 && (
        <Stack gap={2}>
          {a.evidence.map((e, i) => (
            <Text key={i} fz="xs" ff={MONO} c="dimmed" style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>
              {e}
            </Text>
          ))}
        </Stack>
      )}
      <Lines session={a.session} from={from} to={to} mark={a.call} core={core} />
    </Stack>
  )
}

/// The alarms per day, by the family of what went wrong, in the colours
/// Trends gives the families.
function perDay(alerts: WatchAlert[]) {
  const days = [...new Set(alerts.map((a) => dayOf(a.at)))].sort()
  const families = WRONG.filter((f) => alerts.some((a) => f.kinds.includes(a.kind)))
  return {
    days,
    option: {
      legend: legend(),
      grid: grid(),
      tooltip: tip(),
      xAxis: category(days.map(short)),
      yAxis: value(),
      series: families.map((f) => stacked(f.name, days.map((d) => alerts.filter((a) => f.kinds.includes(a.kind) && dayOf(a.at) === d).length), f.colour ?? FAIL, 'a', { barMaxWidth: 26 })),
    },
  }
}

export function Alarms({ w, focus, setFocus, core }: { w: Watch; focus: Focus; setFocus: (f: Focus) => void; core: string | null }) {
  const [opened, setOpened] = useState<string | null>(null)
  const raised = w.alerts.filter((a) => w.alarms.includes(a.kind))
  const alerts = raised.filter((a) => (!focus.session || a.session === focus.session) && (!focus.day || dayOf(a.at) === focus.day))
  const chart = perDay(raised.filter((a) => !focus.session || a.session === focus.session))
  const open = (id: string) => {
    setOpened(id)
    requestAnimationFrame(() => document.getElementById(`alert-${id}`)?.scrollIntoView({ behavior: 'smooth', block: 'center' }))
  }
  const charted = w.alerts.length - raised.length
  return (
    <Stack gap="md">
      <Text fz="sm" c="dimmed" maw={1000}>
        {alerts.length} {alerts.length === 1 ? 'alarm' : 'alarms'}
        {focus.day ? ` on ${focus.day}` : ''}:{' '}
        {w.alarms.map((k) => `${KIND[k][0].toLowerCase()} ${alerts.filter((a) => a.kind === k).length}`).join(' · ')}. Only these kinds raise an alarm; the{' '}
        {charted} other alerts are charted in Trends with everything found. Click a row for its cause.
      </Text>
      {w.not_kinds.length > 0 && (
        <Text fz="xs" c={FAIL}>
          attr_recon_watch_alarms names no kind of alert: {w.not_kinds.join(', ')}
        </Text>
      )}
      {chart.days.length > 0 && (
        <Paper withBorder radius="md" p="sm">
          <Text fw={600} fz="sm">The alarms, day by day</Text>
          <Text fz="xs" c="dimmed">click a day to see only its alarms, here and in every view</Text>
          <EChart height={220} option={chart.option} onClick={(p: Clicked) => p.dataIndex != null && setFocus({ ...focus, day: chart.days[p.dataIndex] })} />
        </Paper>
      )}
      <Paper withBorder radius="md">
        <Table.ScrollContainer minWidth={900}>
          <Table fz="sm" verticalSpacing={4} highlightOnHover>
            <Table.Thead>
              <Table.Tr>
                <Table.Th>When</Table.Th>
                <Table.Th>Alarm</Table.Th>
                <Table.Th>What</Table.Th>
                <Table.Th>Session</Table.Th>
                <Table.Th>At stake</Table.Th>
              </Table.Tr>
            </Table.Thead>
            <Table.Tbody>
              {alerts.length === 0 && (
                <Table.Tr>
                  <Table.Td colSpan={5}>
                    <Text c="dimmed" fz="sm">No alarm here.</Text>
                  </Table.Td>
                </Table.Tr>
              )}
              {alerts.map((a) => (
                <Fragment key={a.id}>
                  <Table.Tr id={`alert-${a.id}`} style={{ cursor: 'pointer' }} onClick={() => setOpened(opened === a.id ? null : a.id)}>
                    <Table.Td ff={MONO} fz="xs" style={{ whiteSpace: 'nowrap' }}>
                      {opened === a.id ? '▾' : '▸'} {day(a.at)}
                    </Table.Td>
                    <Table.Td>
                      <Badge size="sm" variant="filled" color={KIND[a.kind][1]}>{KIND[a.kind][0]}</Badge>
                    </Table.Td>
                    <Table.Td style={{ overflowWrap: 'anywhere' }}>
                      <Text fz="sm">{a.what}</Text>
                      {a.cause && <Text fz="xs" c={FAIL}>caused by another alert</Text>}
                    </Table.Td>
                    <Table.Td>
                      <Text fz="xs" ff={MONO} style={{ overflowWrap: 'anywhere', cursor: 'pointer' }} td="underline" onClick={(e) => { e.stopPropagation(); setFocus({ ...focus, session: a.session }) }}>
                        {a.name}
                      </Text>
                    </Table.Td>
                    <Table.Td style={{ whiteSpace: 'nowrap' }}>{a.cost_secs != null ? took(a.cost_secs) : '—'}</Table.Td>
                  </Table.Tr>
                  {opened === a.id && (
                    <Table.Tr>
                      <Table.Td colSpan={5} bg="dark.7">
                        <Cause a={a} w={w} open={open} core={core} />
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
