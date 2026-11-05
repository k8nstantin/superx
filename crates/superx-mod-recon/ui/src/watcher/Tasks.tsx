import { Fragment, useState } from 'react'
import { Badge, Group, Paper, Stack, Table, Text } from '@mantine/core'
import type { WatchTask } from '../api'
import { FAIL, MONO } from '../EChart'
import { day } from '../time'
import { dayOf, took, type Focus } from './data'
import { around, Lines } from './Lines'

// Every task of the window: the branch the agent made or pushed for the
// work you ordered, from your message before it to its newest act. How often
// it was merged and reopened after, what it was to take and how that moved,
// how often you corrected it and asked how it stood, and the work spent on
// it after you first merged it. A task that never ends heads the list.

/// A merge as its history says it: its pull request and commit when known.
export const merged = (m: WatchTask['merges'][number]) =>
  `merged${m.pull != null ? ` #${m.pull}` : ''}${m.commit ? ` as ${m.commit.slice(0, 8)}` : ''}`

const MARK: Record<string, string> = { reopened: 'red', merged: 'teal', 'not done': 'orange', estimate: 'yellow' }

/// A task's history, oldest first: your message before it, its merges and
/// reopenings, what the agent said was not done after a merge, and its
/// estimates as they moved.
export function History({ t }: { t: WatchTask }) {
  const rows: [string, string, string][] = [
    [t.asked, 'asked', 'your message before it began'] as [string, string, string],
    ...t.merges.map((m): [string, string, string] => [m.at, 'merged', merged(m)]),
    ...t.reopened.map(([at, how]): [string, string, string] => [at, 'reopened', how]),
    ...t.not_done.map(([at, words]): [string, string, string] => [at, 'not done', `once it read the merge it said: "${words}"`]),
    ...t.estimates.map((e): [string, string, string] => [e.at, 'estimate', `${took(e.secs)}: ${e.words}`]),
  ].sort((a, b) => a[0].localeCompare(b[0]))
  return (
    <Stack gap={2}>
      <Text fz="xs" fw={600}>
        {t.tickets.length ? `${t.tickets.join(', ')} · ` : ''}
        {[t.branch, ...t.versions].join(' → ')}: merged {t.merges.length} times, reopened {t.reopened.length} times, {t.corrections} corrections and {t.asks} status asks of yours
      </Text>
      {rows.map(([at, what, words], i) => (
        <Group key={i} gap="xs" wrap="nowrap" align="flex-start">
          <Text fz="xs" ff={MONO} c="dimmed" w={90} style={{ flex: 'none' }}>{day(at)}</Text>
          <Badge size="xs" variant="light" color={MARK[what] ?? 'gray'} w={74} style={{ flex: 'none' }}>
            {what}
          </Badge>
          <Text fz="xs" style={{ overflowWrap: 'anywhere' }}>{words}</Text>
        </Group>
      ))}
    </Stack>
  )
}

const moved = (t: WatchTask) => {
  if (t.estimates.length < 2) return t.estimates.length ? took(t.estimates[0].secs) : '—'
  const secs = t.estimates.map((e) => e.secs)
  return `${took(secs[0])} → ${took(secs[secs.length - 1])} (${t.estimates.length} given)`
}

export function Tasks({ tasks, focus, setFocus, core }: { tasks: WatchTask[]; focus: Focus; setFocus: (f: Focus) => void; core: string | null }) {
  const [opened, setOpened] = useState<string | null>(null)
  const shown = tasks
    .filter((t) => (!focus.session || t.session === focus.session) && (!focus.day || dayOf(t.asked) <= focus.day && focus.day <= dayOf(t.last)))
    .sort((a, b) => b.reopened.length - a.reopened.length || b.last.localeCompare(a.last))
  const endless = shown.filter((t) => t.reopened.length > 0)
  return (
    <Stack gap="md">
      <Text fz="sm" c="dimmed">
        {shown.length} tasks: {endless.length} never ended, reopened after a merge, with {took(endless.reduce((s, t) => s + t.after_merge_secs, 0))} of work after their first merge. Click a task for its history and its lines.
      </Text>
      <Paper withBorder radius="md">
        <Table.ScrollContainer minWidth={1000}>
          <Table fz="sm" verticalSpacing={4} highlightOnHover>
            <Table.Thead>
              <Table.Tr>
                <Table.Th>Task</Table.Th>
                <Table.Th>Session</Table.Th>
                <Table.Th>Asked</Table.Th>
                <Table.Th>Took</Table.Th>
                <Table.Th>Merged</Table.Th>
                <Table.Th>Reopened</Table.Th>
                <Table.Th>After the merge</Table.Th>
                <Table.Th>Its estimates</Table.Th>
                <Table.Th>Your corrections</Table.Th>
                <Table.Th>Your status asks</Table.Th>
              </Table.Tr>
            </Table.Thead>
            <Table.Tbody>
              {shown.map((t) => {
                const id = `${t.session}/${t.branch}`
                const [from, to] = around(t.asked, t.last, 60, 60)
                return (
                  <Fragment key={id}>
                    <Table.Tr style={{ cursor: 'pointer' }} onClick={() => setOpened(opened === id ? null : id)}>
                      <Table.Td style={{ overflowWrap: 'anywhere' }}>
                        <Text fz="sm" fw={t.reopened.length ? 700 : 400} c={t.reopened.length ? FAIL : undefined}>
                          {opened === id ? '▾' : '▸'} {t.tickets.length ? `${t.tickets.join(', ')} · ` : ''}
                          <Text span ff={MONO} fz="xs">{[t.branch, ...t.versions].join(' → ')}</Text>
                        </Text>
                      </Table.Td>
                      <Table.Td>
                        <Text fz="xs" ff={MONO} td="underline" style={{ overflowWrap: 'anywhere' }} onClick={(e) => { e.stopPropagation(); setFocus({ ...focus, session: t.session }) }}>
                          {t.name}
                        </Text>
                      </Table.Td>
                      <Table.Td ff={MONO} fz="xs" style={{ whiteSpace: 'nowrap' }}>{day(t.asked)}</Table.Td>
                      <Table.Td style={{ whiteSpace: 'nowrap' }}>{took((new Date(t.last).getTime() - new Date(t.asked).getTime()) / 1000)}</Table.Td>
                      <Table.Td>{t.merges.length}</Table.Td>
                      <Table.Td c={t.reopened.length ? FAIL : undefined} fw={t.reopened.length ? 700 : undefined}>{t.reopened.length}</Table.Td>
                      <Table.Td style={{ whiteSpace: 'nowrap' }}>{t.merges.length ? took(t.after_merge_secs) : '—'}</Table.Td>
                      <Table.Td fz="xs">{moved(t)}</Table.Td>
                      <Table.Td>{t.corrections}</Table.Td>
                      <Table.Td>{t.asks}</Table.Td>
                    </Table.Tr>
                    {opened === id && (
                      <Table.Tr>
                        <Table.Td colSpan={10} bg="dark.7">
                          <Stack gap="xs" py={4}>
                            <History t={t} />
                            <Lines session={t.session} from={from} to={to} core={core} />
                          </Stack>
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
