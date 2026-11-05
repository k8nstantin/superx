import { useMemo, useState } from 'react'
import { Alert, Badge, Button, Group, Loader, SegmentedControl, Stack, Text, Title } from '@mantine/core'
import { useQuery } from '@tanstack/react-query'
import { fetchFound, fetchPing, fetchWatch, fetchWatchTime, type WatchSessionRow } from '../api'
import { OK } from '../EChart'
import { clock, day } from '../time'
import { Alerts } from './Alerts'
import { Builds } from './Builds'
import { atWork, dayOf, focusWords, keepFound, plus, took, zero, type Focus } from './data'
import { Found } from './Found'
import { Issues } from './Issues'
import { Now } from './Now'
import { Runs } from './Runs'
import { Tasks } from './Tasks'
import { Tickets } from './Tickets'
import { Time } from './Time'

// The Watcher (#573): it follows every session as it works and digs out what
// shows nowhere while it happens. The critical alerts alone, each read back
// to its cause; what it found; where every second went, traced to the tools,
// the commands and the code; every task from your order to its merge and
// past it; every ticket across the sessions that worked on it; every build
// held to its task file as it goes; every run,
// long ones first in mind; and every session as it works. A click on a
// model, a day or a session narrows every view to it.

export const WATCH_VIEWS: [string, string][] = [
  ['alerts', 'Alerts'],
  ['issues', 'Issues'],
  ['found', 'Found'],
  ['time', 'Time'],
  ['tasks', 'Tasks'],
  ['tickets', 'Tickets'],
  ['builds', 'Builds'],
  ['runs', 'Runs'],
  ['now', 'Now'],
]

const QUESTION: Record<string, string> = {
  alerts: 'What went critically wrong, and what caused it?',
  issues: 'Which issues does every repository, session and model have, and how often?',
  found: 'What cost time and showed nowhere while it happened?',
  time: "Where does the agents' time go, and what holds them up?",
  tasks: 'Which tasks never end?',
  tickets: 'Which tickets go round and round, across which sessions?',
  builds: 'What is each build doing against its task file?',
  runs: 'Which runs ran long?',
  now: 'What is every session doing now?',
}

export function Watcher({ sub, go }: { sub: string; go: (sub: string) => void }) {
  const view = WATCH_VIEWS.some(([k]) => k === sub) ? sub : 'alerts'
  const watch = useQuery({ queryKey: ['watch'], queryFn: fetchWatch, refetchInterval: 10_000 })
  const time = useQuery({ queryKey: ['watch-time'], queryFn: fetchWatchTime, refetchInterval: 60_000 })
  const found = useQuery({ queryKey: ['watch-found'], queryFn: fetchFound, refetchInterval: 30_000 })
  const ping = useQuery({ queryKey: ['ping'], queryFn: fetchPing, staleTime: Infinity })
  const [focus, setFocus] = useState<Focus>({})
  const w = watch.data
  const core = ping.data?.core_url ?? null
  const sessions = useMemo(() => new Map<string, WatchSessionRow>((w?.sessions ?? []).map((s) => [s.id, s])), [w])
  const models = useMemo(() => new Map<string, string | null>((w?.sessions ?? []).map((s) => [s.id, s.model])), [w])
  const foundInView = useMemo(() => (found.data ?? []).filter(keepFound(focus, models)), [found.data, focus, models])
  const total = useMemo(() => (time.data?.rows ?? []).reduce((s, r) => plus(s, r.split), zero()), [time.data])
  const words = focusWords(focus, sessions)
  const endless = w?.alerts.filter((a) => a.kind === 'endless_task').length ?? 0
  const pick = (session: string) => {
    setFocus({ ...focus, session })
    go('time')
  }
  const stopped = w?.alerts.filter((a) => a.kind === 'build_stopped').length ?? 0
  const answer = w
    ? `${w.alerts.length} critical alerts, ${endless} of them tasks that never end and ${stopped} builds stopped short; ${found.data?.length ?? '…'} things found; ${took(atWork(total))} at work, ${took(total.operator)} waiting on you, ${took(total.approval)} on your approval, ${took(total.api)} on the API.`
    : null
  return (
    <Stack gap="md">
      <SegmentedControl
        value={view}
        onChange={go}
        data={WATCH_VIEWS.map(([value, label]) => ({
          value,
          label:
            value === 'alerts' && w
              ? `Alerts · ${w.alerts.length}`
              : value === 'found' && found.data
                ? `Found · ${found.data.length}`
                : value === 'tasks' && w
                  ? `Tasks · ${w.tasks.filter((t) => t.reopened.length).length} endless`
                  : value === 'now' && w
                    ? `Now · ${w.now.length}`
                    : label,
        }))}
      />
      <Stack gap={4}>
        <Text ff="monospace" fz="sm" c="pelican.3">{QUESTION[view]}</Text>
        {answer && <Title order={3} style={{ textWrap: 'balance' }}>{answer}</Title>}
        {w && (
          <Group gap="xs">
            <Badge color={w.paused ? 'gray' : OK} variant="light">{w.paused ? 'paused' : `looks every ${w.every_secs} s`}</Badge>
            <Text fz="xs" c="dimmed">
              last look {clock(w.looked_at)} · the window from {day(w.window_from)} · {w.lines_read.toLocaleString('en-US')} lines read · a run is long past {took(w.long_secs)}
            </Text>
          </Group>
        )}
        {w && w.unread.some((u) => u.why === 'agent') && (
          <Text fz="xs" c="yellow">
            not watched yet, another agent's format:{' '}
            {w.unread
              .filter((u) => u.why === 'agent')
              .map((u) => `${u.agent} ${u.rows.toLocaleString('en-US')} lines`)
              .join(' · ')}
          </Text>
        )}
      </Stack>
      {words.length > 0 && (
        <Group gap="xs">
          <Text fz="xs" c="dimmed">only</Text>
          {focus.model && <Badge variant="outline" rightSection="×" style={{ cursor: 'pointer' }} onClick={() => setFocus({ ...focus, model: undefined })}>model {focus.model}</Badge>}
          {focus.day && <Badge variant="outline" rightSection="×" style={{ cursor: 'pointer' }} onClick={() => setFocus({ ...focus, day: undefined })}>day {focus.day}</Badge>}
          {focus.session && (
            <Badge variant="outline" rightSection="×" style={{ cursor: 'pointer' }} onClick={() => setFocus({ ...focus, session: undefined })}>
              session {sessions.get(focus.session)?.name ?? focus.session}
            </Badge>
          )}
          <Button size="compact-xs" variant="subtle" onClick={() => setFocus({})}>show everything</Button>
        </Group>
      )}
      {watch.isError && <Alert color="red" title="The watcher could not be asked">{String(watch.error)}</Alert>}
      {!w && watch.isPending && <Loader size="sm" />}
      {w && w.looked_at == null && !w.paused && <Text c="dimmed">The watcher is reading the window back; its first look comes within a minute.</Text>}
      {w && view === 'alerts' && <Alerts w={w} focus={focus} setFocus={setFocus} core={core} />}
      {w && view === 'issues' && (found.data ? <Issues w={w} found={found.data} time={time.data} focus={focus} core={core} /> : <Loader size="sm" />)}
      {view === 'found' && (found.data ? <Found found={foundInView} focus={focus} setFocus={setFocus} core={core} /> : <Loader size="sm" />)}
      {view === 'time' && (time.data ? <Time time={time.data} focus={focus} setFocus={setFocus} core={core} /> : <Loader size="sm" />)}
      {w && view === 'tasks' && <Tasks tasks={w.tasks} focus={focus} setFocus={setFocus} core={core} />}
      {w && view === 'tickets' && <Tickets w={w} hours={time.data?.hours ?? []} core={core} />}
      {w && view === 'builds' && <Builds w={w} core={core} />}
      {w && view === 'runs' && <Runs w={w} focus={focus} core={core} />}
      {w && view === 'now' && <Now now={w.now} pick={pick} />}
      {found.isError && <Alert color="red" title="What was found did not load">{String(found.error)}</Alert>}
      {time.isError && <Alert color="red" title="The time did not load">{String(time.error)}</Alert>}
      {focus.day && w && <Text fz="xs" c="dimmed">{w.alerts.filter((a) => dayOf(a.at) === focus.day).length} alerts on {focus.day}</Text>}
    </Stack>
  )
}
