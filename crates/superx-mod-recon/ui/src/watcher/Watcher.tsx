import { Alert, Badge, Group, Paper, SimpleGrid, Spoiler, Stack, Table, Text, Title } from '@mantine/core'
import { useQuery } from '@tanstack/react-query'
import { fetchWatch, type AlertKind, type ModelTime, type Split, type WatchAlert, type WatchDay, type WatchRun, type WatchSession } from '../api'
import { AXIS, CANCEL, EChart, FAIL, INK, INK_MUTED, MONO, OK, TOOLTIP, TRACK, UNKNOWN, insideFits } from '../EChart'
import { clock, day, since, span } from '../time'

// The Watcher (#573): the critical alerts alone, every session as it works
// and the jobs it runs, and where the agents' time goes, by model and by
// day, from /api/watch every five seconds.

/// Where an agent's time goes, in the one order of the colours: validated on
/// the card surface #2A1235, every adjacent pair clear under colour-blind
/// vision (dataviz validator, 2026-11-05).
const TIME: [keyof Split, string, string][] = [
  ['thinking', 'thinking', '#B833E8'],
  ['writing', 'writing', '#199e70'],
  ['tools', 'running tools', '#3987e5'],
  ['jobs', 'waiting on its jobs', '#c98500'],
]

const KIND: Record<AlertKind, [string, string]> = {
  run_again: ['Finished work run again', FAIL],
  long_run: ['A run going long', CANCEL],
  stop_not_followed: ['Your stop not followed', FAIL],
  plan_repeats: ['A plan that repeats full runs', CANCEL],
}

const STATE: Record<WatchSession['state'], string> = {
  working: OK,
  'job running': CANCEL,
  'waiting on you': INK_MUTED,
  'waiting on your approval': FAIL,
  idle: UNKNOWN,
}

const agentSecs = (s: Split) => s.thinking + s.writing + s.tools + s.jobs
const hours = (secs: number) => (secs / 3600).toFixed(1)
const pct = (part: number, whole: number) => (whole > 0 ? Math.round((100 * part) / whole) : 0)
const took = (secs: number) => span(secs * 1000)

function Section({ title, note, children }: { title: string; note?: string; children: React.ReactNode }) {
  return (
    <Stack gap="xs">
      <Group gap="xs" align="baseline">
        <Title order={4}>{title}</Title>
        {note && <Text fz="xs" c="dimmed">{note}</Text>}
      </Group>
      {children}
    </Stack>
  )
}

function AlertCard({ a }: { a: WatchAlert }) {
  const [label, colour] = KIND[a.kind]
  return (
    <Paper withBorder p="sm" radius="md" style={{ borderLeft: `4px solid ${colour}` }}>
      <Stack gap={4}>
        <Group justify="space-between" wrap="nowrap" align="flex-start">
          <Group gap={8} wrap="nowrap">
            <Badge color={colour} variant="filled" size="sm">{label}</Badge>
            <Text fz="xs" c="dimmed">{day(a.at)}</Text>
          </Group>
          {a.cost_secs != null && a.kind === 'run_again' && (
            <Text fz="sm" fw={700} c={FAIL}>{took(a.cost_secs)} run again</Text>
          )}
          {a.cost_secs != null && a.kind === 'long_run' && <Text fz="sm" fw={700}>ran {took(a.cost_secs)}</Text>}
        </Group>
        <Text fz="sm">{a.what}</Text>
        <Text fz="xs" c="dimmed" ff={MONO} style={{ overflowWrap: 'anywhere' }}>
          {a.name}
          {a.since && a.kind === 'run_again' ? ` · the first run started ${day(a.since)}` : ''}
          {a.since && a.kind === 'stop_not_followed' ? ` · your stop at ${clock(a.since)}` : ''}
        </Text>
        {a.evidence.length > 0 && (
          <Spoiler maxHeight={40} showLabel="show all" hideLabel="show less">
            {a.evidence.map((e, i) => (
              <Text key={i} fz="xs" ff={MONO} c="dimmed" style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>{e}</Text>
            ))}
          </Spoiler>
        )}
      </Stack>
    </Paper>
  )
}

function SessionCard({ s }: { s: WatchSession }) {
  return (
    <Paper withBorder p="sm" radius="md">
      <Stack gap={6}>
        <Group justify="space-between" wrap="nowrap" align="flex-start">
          <Stack gap={0} style={{ minWidth: 0 }}>
            <Text fw={700} ff={MONO} fz="sm" style={{ overflowWrap: 'anywhere' }}>{s.name}</Text>
            <Text fz="xs" c="dimmed" style={{ overflowWrap: 'anywhere' }}>{[s.model, s.folder].filter(Boolean).join(' · ')}</Text>
          </Stack>
          <Badge color={STATE[s.state]} variant="light">{s.state}</Badge>
        </Group>
        {s.doing && (
          <Text fz="sm" style={{ overflowWrap: 'anywhere' }}>
            {s.doing.tool}: {s.doing.what} <Text span c="dimmed" fz="xs">· {since(s.doing.since)}</Text>
          </Text>
        )}
        {s.jobs.map((j) => (
          <Text key={j.since + j.what} fz="sm" style={{ overflowWrap: 'anywhere' }}>
            job: {j.what} <Text span c="dimmed" fz="xs">· running {since(j.since)}{j.expected_end ? ` · the same launch ended by ${clock(j.expected_end)} last time` : ''}</Text>
          </Text>
        ))}
        <Text fz="xs" c="dimmed">newest line {clock(s.newest)}</Text>
      </Stack>
    </Paper>
  )
}

/// A model with under an hour at work draws as a bar of its few minutes'
/// shares: the table lists it, the chart leaves it out.
const charted = (models: ModelTime[]) => models.filter((m) => agentSecs(m.split) >= 3600)

function byModelChart(models: ModelTime[]) {
  const shown = charted(models)
  return {
    grid: { left: 150, right: 24, top: 36, bottom: 24 },
    legend: { top: 0, textStyle: { color: INK } },
    tooltip: { ...TOOLTIP, trigger: 'axis', axisPointer: { type: 'shadow' } },
    xAxis: { type: 'value', max: 100, ...AXIS, axisLabel: { ...AXIS.axisLabel, formatter: '{value}%' } },
    yAxis: { type: 'category', data: shown.map((m) => m.model), inverse: true, ...AXIS },
    series: TIME.map(([key, name, colour]) => ({
      name,
      type: 'bar',
      stack: 'time',
      barWidth: 18,
      itemStyle: { color: colour, borderColor: TRACK, borderWidth: 1 },
      label: { show: true, color: '#fff', fontSize: 11, formatter: (p: { value: number }) => `${p.value}%` },
      labelLayout: insideFits,
      data: shown.map((m) => pct(m.split[key], agentSecs(m.split))),
    })),
  }
}

function byDayChart(days: WatchDay[]) {
  return {
    grid: { left: 48, right: 16, top: 36, bottom: 28 },
    legend: { top: 0, textStyle: { color: INK } },
    tooltip: { ...TOOLTIP, trigger: 'axis', axisPointer: { type: 'shadow' }, valueFormatter: (v: number) => `${v} h` },
    xAxis: { type: 'category', data: days.map((d) => d.day.slice(5)), ...AXIS },
    yAxis: { type: 'value', name: 'agent hours', nameTextStyle: { color: INK_MUTED }, ...AXIS },
    series: TIME.map(([key, name, colour]) => ({
      name,
      type: 'bar',
      stack: 'time',
      itemStyle: { color: colour, borderColor: TRACK, borderWidth: 1 },
      data: days.map((d) => Number(hours(d.split[key]))),
    })),
  }
}

function RunsTable({ runs, long }: { runs: WatchRun[]; long: number }) {
  if (runs.length === 0) return <Text c="dimmed" fz="sm">No run that long in the window.</Text>
  return (
    <Paper withBorder radius="md">
      <Table.ScrollContainer minWidth={760}>
        <Table fz="sm" verticalSpacing={4}>
          <Table.Thead>
            <Table.Tr>
              <Table.Th>Started</Table.Th>
              <Table.Th>Took</Table.Th>
              <Table.Th>What</Table.Th>
              <Table.Th>Session</Table.Th>
              <Table.Th>Ended</Table.Th>
            </Table.Tr>
          </Table.Thead>
          <Table.Tbody>
            {runs.map((r) => (
              <Table.Tr key={r.session + r.start + r.label}>
                <Table.Td style={{ whiteSpace: 'nowrap' }}>{day(r.start)}</Table.Td>
                <Table.Td style={{ whiteSpace: 'nowrap' }}>
                  <Text span fz="sm" fw={r.secs >= long ? 700 : 400} c={r.secs >= long ? CANCEL : undefined}>{took(r.secs)}</Text>
                </Table.Td>
                <Table.Td style={{ overflowWrap: 'anywhere' }}>{r.label}</Table.Td>
                <Table.Td><Text fz="xs" ff={MONO} style={{ overflowWrap: 'anywhere' }}>{r.name}</Text></Table.Td>
                <Table.Td>{r.end == null ? 'running' : r.failed ? 'failed' : (r.status ?? 'done')}</Table.Td>
              </Table.Tr>
            ))}
          </Table.Tbody>
        </Table>
      </Table.ScrollContainer>
    </Paper>
  )
}

export function Watcher() {
  const watch = useQuery({ queryKey: ['watch'], queryFn: fetchWatch, refetchInterval: 5_000 })
  const w = watch.data
  const total = (w?.models ?? []).reduce(
    (t, m) => ({ agent: t.agent + agentSecs(m.split), operator: t.operator + m.split.operator, approval: t.approval + m.split.approval }),
    { agent: 0, operator: 0, approval: 0 },
  )
  return (
    <Stack gap="xl">
      <Stack gap={4}>
        <Text fz="sm" c="dimmed">
          The critical alerts alone, every session as it works and the jobs it launches, and where the agents' time goes. Long running means problems.
        </Text>
        {w && (
          <Group gap="xs">
            <Badge color={w.paused ? 'gray' : OK} variant="light">{w.paused ? 'paused' : `looks every ${w.every_secs} s`}</Badge>
            <Text fz="xs" c="dimmed">
              last look {clock(w.looked_at)} · the window from {day(w.window_from)} · {w.lines_read.toLocaleString('en-US')} lines read · a run is long past {took(w.long_secs)}
            </Text>
          </Group>
        )}
      </Stack>
      {watch.isError && <Alert color="red" title="The watcher could not be asked">{String(watch.error)}</Alert>}
      {w && w.looked_at == null && !w.paused && <Text c="dimmed">The watcher is reading the window back; its first look comes within a minute.</Text>}

      <Section title={`Alerts · ${w?.alerts.length ?? 0}`} note="critical only, each once, newest first">
        {(w?.alerts ?? []).length === 0 ? (
          <Text c="dimmed" fz="sm">No critical alert in the window.</Text>
        ) : (
          <SimpleGrid cols={{ base: 1, lg: 2 }} spacing="sm">
            {(w?.alerts ?? []).map((a) => <AlertCard key={a.id} a={a} />)}
          </SimpleGrid>
        )}
      </Section>

      <Section title="Now" note="the sessions with a line in the last six hours, and every job still running">
        {(w?.sessions ?? []).length === 0 ? (
          <Text c="dimmed" fz="sm">No session at work.</Text>
        ) : (
          <SimpleGrid cols={{ base: 1, md: 2, xl: 3 }} spacing="sm">
            {(w?.sessions ?? []).map((s) => <SessionCard key={s.id} s={s} />)}
          </SimpleGrid>
        )}
      </Section>

      {w && w.models.length > 0 && (
        <Section
          title="Where the agents' time goes"
          note={`${hours(total.agent)} h at work · ${hours(total.operator)} h waiting on you · ${hours(total.approval)} h waiting on your approval`}
        >
          <Paper withBorder p="sm" radius="md">
            <EChart option={byModelChart(w.models)} height={60 + 34 * charted(w.models).length} />
          </Paper>
          <Paper withBorder radius="md">
            <Table.ScrollContainer minWidth={760}>
              <Table fz="sm" verticalSpacing={4}>
                <Table.Thead>
                  <Table.Tr>
                    <Table.Th>Model</Table.Th>
                    <Table.Th>At work</Table.Th>
                    {TIME.map(([k, name]) => <Table.Th key={k}>{name}</Table.Th>)}
                    <Table.Th>Waiting on you</Table.Th>
                    <Table.Th>Sessions</Table.Th>
                    <Table.Th>Prompts</Table.Th>
                    <Table.Th>Calls</Table.Th>
                    <Table.Th>Jobs launched</Table.Th>
                  </Table.Tr>
                </Table.Thead>
                <Table.Tbody>
                  {w.models.map((m) => (
                    <Table.Tr key={m.model}>
                      <Table.Td ff={MONO}>{m.model}</Table.Td>
                      <Table.Td>{hours(agentSecs(m.split))} h</Table.Td>
                      {TIME.map(([k]) => <Table.Td key={k}>{pct(m.split[k], agentSecs(m.split))}%</Table.Td>)}
                      <Table.Td>{hours(m.split.operator + m.split.approval)} h</Table.Td>
                      <Table.Td>{m.sessions}</Table.Td>
                      <Table.Td>{m.counts.prompts.toLocaleString('en-US')}</Table.Td>
                      <Table.Td>{m.counts.calls.toLocaleString('en-US')}</Table.Td>
                      <Table.Td>{m.counts.launches}</Table.Td>
                    </Table.Tr>
                  ))}
                </Table.Tbody>
              </Table>
            </Table.ScrollContainer>
          </Paper>
        </Section>
      )}

      {w && w.days.length > 0 && (
        <Section title="Day by day" note="in your time">
          <Paper withBorder p="sm" radius="md">
            <EChart option={byDayChart(w.days)} height={260} />
          </Paper>
          <Paper withBorder radius="md">
            <Table.ScrollContainer minWidth={760}>
              <Table fz="sm" verticalSpacing={4}>
                <Table.Thead>
                  <Table.Tr>
                    <Table.Th>Day</Table.Th>
                    <Table.Th>Sessions</Table.Th>
                    <Table.Th>Prompts</Table.Th>
                    <Table.Th>Calls</Table.Th>
                    <Table.Th>Jobs launched</Table.Th>
                    <Table.Th>At work</Table.Th>
                    <Table.Th>Long runs</Table.Th>
                    <Table.Th>Alerts</Table.Th>
                  </Table.Tr>
                </Table.Thead>
                <Table.Tbody>
                  {[...w.days].reverse().map((d) => {
                    const alerts = Object.entries(d.alerts).map(([k, n]) => `${n} ${KIND[k as AlertKind][0].toLowerCase()}`)
                    return (
                      <Table.Tr key={d.day}>
                        <Table.Td style={{ whiteSpace: 'nowrap' }}>{d.day}</Table.Td>
                        <Table.Td>{d.sessions}</Table.Td>
                        <Table.Td>{d.counts.prompts}</Table.Td>
                        <Table.Td>{d.counts.calls.toLocaleString('en-US')}</Table.Td>
                        <Table.Td>{d.counts.launches}</Table.Td>
                        <Table.Td>{hours(agentSecs(d.split))} h</Table.Td>
                        <Table.Td c={d.long_runs > 0 ? CANCEL : undefined}>{d.long_runs}</Table.Td>
                        <Table.Td c={alerts.length > 0 ? FAIL : undefined}>{alerts.join(', ') || '—'}</Table.Td>
                      </Table.Tr>
                    )
                  })}
                </Table.Tbody>
              </Table>
            </Table.ScrollContainer>
          </Paper>
        </Section>
      )}

      {w && (
        <Section title={`Long runs · ${w.runs.length}`} note={`every run of ${took(w.repeat_secs)} or more, and every job still running; past ${took(w.long_secs)} in ochre`}>
          <RunsTable runs={w.runs} long={w.long_secs} />
        </Section>
      )}
    </Stack>
  )
}
