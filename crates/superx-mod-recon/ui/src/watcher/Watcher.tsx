import { useEffect, useRef, useState } from 'react'
import { Alert, Badge, Grid, Group, Paper, Stack, Switch, Text, Title } from '@mantine/core'
import { useQuery } from '@tanstack/react-query'
import { fetchWatch, type Level, type WatchReport, type WatchSession } from '../api'
import { clock, day } from '../time'
import { Clamped, inWords, LevelBadge } from './parts'
import { Session } from './Session'

// The Watcher (#566): every session as it works, what it is doing this
// second, and every report, newest first, asked of /api/watch every two
// seconds as the Crawler's Now panel asks /api/now; a browser notification
// for each new warn or alert while Notify me is on.

const RANK: Record<Level, number> = { alert: 2, warn: 1, info: 0 }
const NOTIFY_KEY = 'recon.watcher.notify'

/// The switch's state, kept in the browser; off when it cannot be read.
function readNotify(): boolean {
  try {
    return window.localStorage.getItem(NOTIFY_KEY) === 'on'
  } catch {
    return false
  }
}

function keepNotify(on: boolean) {
  try {
    window.localStorage.setItem(NOTIFY_KEY, on ? 'on' : 'off')
  } catch {
    // The browser keeps nothing: the switch holds for this page alone.
  }
}

/// The loudest level among a session's reports, or none.
function loudest(reports: WatchReport[]): number {
  return reports.reduce((m, r) => Math.max(m, RANK[r.level] + 1), 0)
}

export function Watcher() {
  const watch = useQuery({ queryKey: ['watch'], queryFn: fetchWatch, refetchInterval: 2_000 })
  const data = watch.data
  const [notify, setNotify] = useState(readNotify)
  const [refused, setRefused] = useState<string | null>(null)
  // The reports the page already held: none of them raises a notification.
  const held = useRef<Set<string> | null>(null)

  useEffect(() => {
    if (!data) return
    const ids = data.reports.map((r) => r.id)
    if (held.current == null) {
      held.current = new Set(ids)
      return
    }
    const fresh = data.reports.filter((r) => !held.current?.has(r.id))
    for (const id of ids) held.current.add(id)
    if (!notify || typeof Notification === 'undefined' || Notification.permission !== 'granted') return
    for (const r of fresh.filter((r) => r.level !== 'info')) {
      const session = data.sessions.find((s) => s.id === r.session)?.name ?? r.session
      new Notification(`${inWords(r.kind)} · ${session}`, { body: r.what, tag: r.id })
    }
  }, [data, notify])

  const turn = async (on: boolean) => {
    if (!on) {
      setNotify(false)
      keepNotify(false)
      return
    }
    if (typeof Notification === 'undefined') {
      setRefused('this browser has no notifications')
      return
    }
    const permission = await Notification.requestPermission()
    if (permission !== 'granted') {
      setRefused('the browser refused notifications')
      return
    }
    setRefused(null)
    // A report the page already holds raises none.
    held.current = new Set(data?.reports.map((r) => r.id) ?? [])
    setNotify(true)
    keepNotify(true)
  }

  const bySession = new Map<string, WatchReport[]>()
  for (const r of data?.reports ?? []) bySession.set(r.session, [...(bySession.get(r.session) ?? []), r])
  const sessions: WatchSession[] = [...(data?.sessions ?? [])].sort((a, b) => {
    const loud = loudest(bySession.get(b.id) ?? []) - loudest(bySession.get(a.id) ?? [])
    return loud !== 0 ? loud : (b.newest ?? '').localeCompare(a.newest ?? '')
  })
  const unread = (data?.unread ?? []).filter((u) => u.rows > 0)

  return (
    <Stack gap="lg">
      <Group justify="space-between" align="flex-start">
        <Stack gap={4}>
          <Title order={3}>Watcher</Title>
          <Text size="xs" c="dimmed">every session as it works, judged against your orders and the task file it executes · every 2 s</Text>
          {data && (
            <Group gap="xs">
              <Badge color={data.paused ? 'gray' : 'teal'} variant="light">
                {data.paused ? 'paused' : `looks every ${data.every_secs} s`}
              </Badge>
              <Text fz="sm">
                last look {clock(data.looked_at)} · window from {day(data.window_from)} · {data.lines_read.toLocaleString('en-US')} lines read
              </Text>
            </Group>
          )}
          {unread.length > 0 && (
            <Text fz="xs" c="dimmed">
              not read: {unread.map((u) => `${u.rows} ${inWords(u.why)}${u.why === 'agent' ? ` (${u.agent})` : ''}`).join(', ')}
            </Text>
          )}
        </Stack>
        <Stack gap={2} align="flex-end">
          <Switch label="Notify me" checked={notify} onChange={(e) => void turn(e.currentTarget.checked)} />
          {refused && <Text fz="xs" c="red.4">{refused}</Text>}
        </Stack>
      </Group>

      {watch.isError && <Alert color="red" title="The watcher could not be asked">{String(watch.error)}</Alert>}
      {data && data.looked_at == null && !data.paused && <Text c="dimmed">The watcher's first look comes within a few seconds.</Text>}

      <Grid gap="lg">
        <Grid.Col span={{ base: 12, lg: 7 }}>
          <Stack gap="md">
            {sessions.length === 0 && data?.looked_at != null && <Text c="dimmed">No session wrote a line in the window.</Text>}
            {sessions.map((s) => (
              <Session key={s.id} s={s} reports={bySession.get(s.id) ?? []} />
            ))}
          </Stack>
        </Grid.Col>
        <Grid.Col span={{ base: 12, lg: 5 }}>
          <Stack gap="sm">
            <Title order={4}>Reports · {data?.reports.length ?? 0}</Title>
            {(data?.reports ?? []).length === 0 && <Text c="dimmed" fz="sm">Nothing to report in the window.</Text>}
            {(data?.reports ?? []).map((r) => (
              <Paper key={r.id} withBorder p="sm" radius="md">
                <Stack gap={4}>
                  <Group gap={6} justify="space-between" wrap="nowrap">
                    <Group gap={6} wrap="nowrap">
                      <LevelBadge level={r.level} />
                      <Text fw={600} fz="sm">{inWords(r.kind)}</Text>
                    </Group>
                    <Text fz="xs" c="dimmed">{day(r.at)}</Text>
                  </Group>
                  <Text fz="xs" c="dimmed" ff="monospace" style={{ overflowWrap: 'anywhere' }}>
                    {data?.sessions.find((s) => s.id === r.session)?.name ?? r.session}
                  </Text>
                  <Text fz="sm">{r.what}</Text>
                  {r.words.map((w, i) => (
                    <Stack key={i} gap={0}>
                      <Text fz="xs" c="dimmed">{w.whose === 'operator' ? 'your words' : w.whose === 'agent' ? 'its words' : 'the call'}</Text>
                      <Clamped text={w.text} lines={2} mono={w.whose === 'call'} />
                    </Stack>
                  ))}
                </Stack>
              </Paper>
            ))}
          </Stack>
        </Grid.Col>
      </Grid>
    </Stack>
  )
}
