import { Fragment, useState } from 'react'
import { Badge, Group, Paper, SimpleGrid, Stack, Table, Text } from '@mantine/core'
import type { BuildRow, Waste, Watch, WatchAlert } from '../api'
import { CANCEL, FAIL, MONO, OK, UNKNOWN } from '../EChart'
import { day } from '../time'
import { addCost, big, n, zeroCost } from './data'
import { around, Lines } from './Lines'

// Builds (#573): every build of a task file, held to what its file decides,
// as it goes. What the file has it build and what it wrote so far; each of
// its gates, the local ones by their own commands and expected last lines,
// the door ones by their runs; the branch the file names and the one cut;
// every commit against the file's rules; the breaks the watcher raised; and
// the session's lines from the order to build on. The newest build first.

/// A yes, a no, or not yet, as a mark and its colour.
const mark = (v: boolean | null | undefined): [string, string] => (v == null ? ['not yet', UNKNOWN] : v ? ['✓', OK] : ['✗', FAIL])

/// Where a build stands.
function standing(b: BuildRow, breaks: WatchAlert[]): [string, string] {
  if (b.stopped) return ['stopped short', FAIL]
  if (breaks.length) return [`${breaks.length} ${breaks.length === 1 ? 'break' : 'breaks'}`, FAIL]
  if (b.gates.length && b.gates.every((g) => g.ran && g.ended_well === g.ends_well)) return ['door gates done', OK]
  if (b.gates.some((g) => g.ran && g.ended_well == null)) return ['a door gate running', CANCEL]
  if (b.builds.length && b.builds.every((f) => b.written.includes(f))) return ['written, gates ahead', CANCEL]
  return ['building', CANCEL]
}

function Mark({ v }: { v: boolean | null | undefined }) {
  const [text, colour] = mark(v)
  return (
    <Text span fz="xs" fw={700} c={colour} w={52} style={{ display: 'inline-block', flex: 'none' }}>
      {text}
    </Text>
  )
}

function Plan({ b, breaks, core }: { b: BuildRow; breaks: WatchAlert[]; core: string | null }) {
  const last = [b.stopped, ...b.gates.map((g) => g.ran), ...b.locals.map((l) => l.ran), ...b.commits.map((c) => c.at)].filter((v): v is string => !!v).sort().pop() ?? b.ordered
  const [from, to] = around(b.ordered, last, 60, 600)
  return (
    <Stack gap="sm" py={4}>
      <SimpleGrid cols={{ base: 1, md: 3 }} spacing="md">
        <Stack gap={4}>
          <Text fz="xs" fw={700}>What it builds</Text>
          {b.builds.map((f) => (
            <Group key={f} gap={6} wrap="nowrap">
              <Mark v={b.written.includes(f) ? true : null} />
              <Text fz="xs" ff={MONO} style={{ overflowWrap: 'anywhere' }}>{f}</Text>
            </Group>
          ))}
          <Text fz="xs" fw={700} mt={6}>Forbidden files it changed</Text>
          {b.forbidden.length === 0 ? (
            <Text fz="xs" c="dimmed">none: it may change {b.owns.length} files and none of {b.forbid.length} forbidden places</Text>
          ) : (
            b.forbidden.map((f) => (
              <Text key={f} fz="xs" ff={MONO} c={FAIL} style={{ overflowWrap: 'anywhere' }}>{f}</Text>
            ))
          )}
        </Stack>
        <Stack gap={4}>
          <Text fz="xs" fw={700}>Its gates</Text>
          {b.locals.map((g) => (
            <Group key={`l-${g.gate}`} gap={6} wrap="nowrap">
              <Mark v={g.passed} />
              <Text fz="xs" style={{ flex: 1 }}>{g.gate}</Text>
              <Text fz="xs" ff={MONO} c="dimmed">{g.ran ? day(g.ran) : 'not run'}</Text>
            </Group>
          ))}
          {b.gates.map((g, i) => (
            <Group key={`d-${g.gate}-${i}`} gap={6} wrap="nowrap">
              <Mark v={g.ended_well == null ? null : g.ended_well === g.ends_well} />
              <Text fz="xs" style={{ flex: 1 }}>
                {g.gate} <Text span fz="xs" c="dimmed">· door{g.ends_well ? '' : ', expects to fail'}</Text>
              </Text>
              <Text fz="xs" ff={MONO} c="dimmed">{g.ran ? day(g.ran) : 'not run'}</Text>
            </Group>
          ))}
        </Stack>
        <Stack gap={4}>
          <Text fz="xs" fw={700}>Its git</Text>
          <Text fz="xs" c="dimmed">the file names the branch</Text>
          <Text fz="xs" ff={MONO} style={{ overflowWrap: 'anywhere' }}>{b.branch_rule ?? '—'}</Text>
          <Text fz="xs" c="dimmed">it cut</Text>
          <Text fz="xs" ff={MONO} c={breaks.some((a) => a.id.endsWith('/branch')) ? FAIL : undefined} style={{ overflowWrap: 'anywhere' }}>
            {b.branch ?? 'none yet'}
          </Text>
          <Text fz="xs" c="dimmed" mt={4}>{b.commits.length} commits</Text>
          {b.commits.map((c, i) => (
            <Group key={i} gap={6} wrap="nowrap" align="flex-start">
              <Mark v={c.ok} />
              <Text fz="xs" style={{ overflowWrap: 'anywhere' }}>{c.line}</Text>
            </Group>
          ))}
        </Stack>
      </SimpleGrid>
      {breaks.length > 0 && (
        <Paper withBorder radius="sm" p="xs">
          <Text fz="xs" fw={700} c={FAIL}>What the watcher raised</Text>
          {breaks.map((a) => (
            <Group key={a.id} gap="xs" wrap="nowrap" align="flex-start">
              <Text fz="xs" ff={MONO} c="dimmed" w={90} style={{ flex: 'none' }}>{day(a.at)}</Text>
              <Text fz="xs" style={{ overflowWrap: 'anywhere' }}>{a.what}</Text>
            </Group>
          ))}
        </Paper>
      )}
      <Lines session={b.session} from={from} to={to} core={core} />
    </Stack>
  )
}

export function Builds({ w, waste, core }: { w: Watch; waste: Waste | undefined; core: string | null }) {
  // A build's whole work, and what of it went to waste.
  const spent = (session: string) => (waste?.days ?? []).filter((d) => d.session === session).reduce((c, d) => [addCost(c[0], d.all), addCost(c[1], d.wasted)], [zeroCost(), zeroCost()])
  const [opened, setOpened] = useState<string | null>(w.builds[0]?.session ?? null)
  const names = new Map(w.sessions.map((s) => [s.id, s.name]))
  const breaksOf = (b: BuildRow) => w.alerts.filter((a) => a.session === b.session && (a.kind === 'task_file_broken' || a.kind === 'build_stopped'))
  return (
    <Stack gap="md">
      <Text fz="sm" c="dimmed" maw={900}>
        {w.builds.length} builds of a task file in the window; {w.builds.filter((b) => b.stopped).length} stopped short and{' '}
        {w.builds.filter((b) => breaksOf(b).some((a) => a.kind === 'task_file_broken')).length} broke their file. Each opens on its file's plan as it went: what it builds,
        its gates, its git, and its lines.
      </Text>
      <Paper withBorder radius="md">
        <Table.ScrollContainer minWidth={1240}>
          <Table fz="sm" verticalSpacing={4} highlightOnHover>
            <Table.Thead>
              <Table.Tr>
                <Table.Th>Ordered</Table.Th>
                <Table.Th>Task file</Table.Th>
                <Table.Th>Session</Table.Th>
                <Table.Th>Where it stands</Table.Th>
                <Table.Th>Written</Table.Th>
                <Table.Th>Local gates</Table.Th>
                <Table.Th>Door gates</Table.Th>
                <Table.Th>Commits</Table.Th>
                <Table.Th>Its work</Table.Th>
                <Table.Th>Went to waste</Table.Th>
              </Table.Tr>
            </Table.Thead>
            <Table.Tbody>
              {w.builds.length === 0 && (
                <Table.Tr>
                  <Table.Td colSpan={10}>
                    <Text c="dimmed" fz="sm">No build of a task file in the window.</Text>
                  </Table.Td>
                </Table.Tr>
              )}
              {w.builds.map((b) => {
                const breaks = breaksOf(b)
                const [state, colour] = standing(b, breaks)
                const passed = b.locals.filter((l) => l.passed).length
                const doors = b.gates.filter((g) => g.ran && g.ended_well === g.ends_well).length
                const bad = b.commits.filter((c) => !c.ok).length
                const [work, lost] = spent(b.session)
                return (
                  <Fragment key={b.session}>
                    <Table.Tr style={{ cursor: 'pointer' }} onClick={() => setOpened(opened === b.session ? null : b.session)}>
                      <Table.Td ff={MONO} fz="xs" style={{ whiteSpace: 'nowrap' }}>
                        {opened === b.session ? '▾' : '▸'} {day(b.ordered)}
                      </Table.Td>
                      <Table.Td ff={MONO} fz="xs">
                        {b.file}
                        {b.ticket ? <Text span fz="xs" c="dimmed">{` · ${b.ticket}`}</Text> : null}
                      </Table.Td>
                      <Table.Td ff={MONO} fz="xs">{(names.get(b.session) ?? b.session).split('/').pop()?.slice(0, 8)}</Table.Td>
                      <Table.Td>
                        <Badge size="sm" variant="light" color={colour}>{state}</Badge>
                      </Table.Td>
                      <Table.Td>{`${b.written.length} of ${b.builds.length}`}</Table.Td>
                      <Table.Td>{`${passed} of ${b.locals.length} passed`}</Table.Td>
                      <Table.Td>{`${doors} of ${b.gates.length}`}</Table.Td>
                      <Table.Td c={bad ? FAIL : undefined}>{bad ? `${b.commits.length}, ${bad} against its rules` : b.commits.length}</Table.Td>
                      <Table.Td style={{ whiteSpace: 'nowrap' }} fz="xs">{waste ? `${n(work.added)} lines · ${big(work.tokens.generated)} tokens` : '…'}</Table.Td>
                      <Table.Td style={{ whiteSpace: 'nowrap' }}>
                        {lost.added || lost.tokens.generated ? (
                          <Text fz="xs" fw={700} c={FAIL}>
                            {n(lost.added)} lines · {big(lost.tokens.generated)} tokens
                          </Text>
                        ) : (
                          <Text fz="xs" c="dimmed">none</Text>
                        )}
                      </Table.Td>
                    </Table.Tr>
                    {opened === b.session && (
                      <Table.Tr>
                        <Table.Td colSpan={10} bg="dark.7">
                          <Plan b={b} breaks={breaks} core={core} />
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
