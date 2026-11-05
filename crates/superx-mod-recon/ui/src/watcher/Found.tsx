import { Fragment, useState } from 'react'
import { Badge, Button, Group, Paper, SimpleGrid, Stack, Table, Text, UnstyledButton } from '@mantine/core'
import type { Found as FoundRow, Hidden } from '../api'
import { CANCEL, EChart, INK, MONO, PELICAN, type Clicked } from '../EChart'
import { category, grid, tip, value } from '../results/charts'
import { day } from '../time'
import { dayOf, HIDDEN, HIDDEN_ORDER, n, short, took, type Focus } from './data'
import { around, Lines } from './Lines'

// What the watcher dug out: what cost the agents' time, or yours, and showed
// nowhere while it happened. A card for each kind with how often and what it
// cost; the kind picked, day by day; and every one of them, newest first,
// each opening on the session's lines that caused it.

const PAGE = 100

/// What a finding cost, in its own measure.
function cost(f: FoundRow): string {
  if (f.secs != null) return took(f.secs)
  if (f.chars != null) return `${n(f.chars)} chars`
  if (f.times > 1) return `${f.times} times`
  return '—'
}

function Card({ kind, rows, picked, pick }: { kind: Hidden; rows: FoundRow[]; picked: boolean; pick: () => void }) {
  const [name, about] = HIDDEN[kind]
  const secs = rows.reduce((t, f) => t + (f.secs ?? 0), 0)
  const chars = rows.reduce((t, f) => t + (f.chars ?? 0), 0)
  const sessions = new Set(rows.map((f) => f.session)).size
  return (
    <UnstyledButton onClick={pick}>
      <Paper withBorder radius="md" p="sm" h="100%" style={{ borderColor: picked ? PELICAN : undefined, borderWidth: picked ? 2 : undefined }}>
        <Stack gap={2}>
          <Group justify="space-between" wrap="nowrap">
            <Text fw={700} fz="sm">{name}</Text>
            <Text fw={700} fz="lg">{n(rows.length)}</Text>
          </Group>
          <Text fz="xs" c="dimmed">{about}</Text>
          <Text fz="xs">
            {sessions} sessions{secs ? ` · ${took(secs)}` : ''}
            {chars ? ` · ${n(chars)} chars` : ''}
          </Text>
        </Stack>
      </Paper>
    </UnstyledButton>
  )
}

export function Found({ found, focus, setFocus, core }: { found: FoundRow[]; focus: Focus; setFocus: (f: Focus) => void; core: string | null }) {
  const [kind, setKind] = useState<Hidden | null>(null)
  const [opened, setOpened] = useState<string | null>(null)
  const [shown, setShown] = useState(PAGE)
  const ofKind = (k: Hidden) => found.filter((f) => f.kind === k)
  const picked = kind ? ofKind(kind) : found
  const days = [...new Set(picked.map((f) => dayOf(f.at)))].sort()
  const rows = picked.filter((f) => !focus.day || dayOf(f.at) === focus.day)
  const trend = {
    grid: grid(),
    tooltip: tip(),
    xAxis: category(days.map(short)),
    yAxis: value(),
    series: [
      {
        name: kind ? HIDDEN[kind][0] : 'everything found',
        type: 'bar',
        barMaxWidth: 26,
        itemStyle: { color: kind === 'held' || kind === 'slept' ? CANCEL : PELICAN, borderRadius: [3, 3, 0, 0] },
        data: days.map((d) => picked.filter((f) => dayOf(f.at) === d).length),
      },
    ],
  }
  return (
    <Stack gap="md">
      <SimpleGrid cols={{ base: 1, sm: 2, lg: 4, xl: 6 }} spacing="sm">
        {HIDDEN_ORDER.map((k) => (
          <Card key={k} kind={k} rows={ofKind(k)} picked={kind === k} pick={() => { setKind(kind === k ? null : k); setShown(PAGE) }} />
        ))}
      </SimpleGrid>
      <Paper withBorder radius="md" p="sm">
        <Group justify="space-between">
          <Stack gap={0}>
            <Text fw={600} fz="sm">
              {kind ? HIDDEN[kind][0] : 'Everything found'}, day by day: {n(picked.length)} in all
            </Text>
            <Text fz="xs" c="dimmed">click a card above to pick a kind · click a day to see only that day, here and in every view</Text>
          </Stack>
          {kind && <Button size="compact-xs" variant="subtle" onClick={() => setKind(null)}>every kind</Button>}
        </Group>
        <EChart height={200} option={trend} onClick={(p: Clicked) => p.dataIndex != null && setFocus({ ...focus, day: days[p.dataIndex] })} />
      </Paper>
      <Paper withBorder radius="md">
        <Table.ScrollContainer minWidth={900}>
          <Table fz="sm" verticalSpacing={4} highlightOnHover>
            <Table.Thead>
              <Table.Tr>
                <Table.Th>When</Table.Th>
                <Table.Th>Found</Table.Th>
                <Table.Th>What</Table.Th>
                <Table.Th>Session</Table.Th>
                <Table.Th>Cost</Table.Th>
              </Table.Tr>
            </Table.Thead>
            <Table.Tbody>
              {rows.slice(0, shown).map((f) => (
                <Fragment key={f.id}>
                  <Table.Tr style={{ cursor: 'pointer' }} onClick={() => setOpened(opened === f.id ? null : f.id)}>
                    <Table.Td ff={MONO} fz="xs" style={{ whiteSpace: 'nowrap' }}>
                      {opened === f.id ? '▾' : '▸'} {day(f.at)}
                    </Table.Td>
                    <Table.Td>
                      <Badge size="sm" variant="light" color="grape">{HIDDEN[f.kind][0]}</Badge>
                    </Table.Td>
                    <Table.Td style={{ overflowWrap: 'anywhere' }}>
                      <Text fz="sm" lineClamp={opened === f.id ? undefined : 2}>{f.what}</Text>
                    </Table.Td>
                    <Table.Td>
                      <Text fz="xs" ff={MONO} c={INK} td="underline" style={{ overflowWrap: 'anywhere' }} onClick={(e) => { e.stopPropagation(); setFocus({ ...focus, session: f.session }) }}>
                        {f.name}
                      </Text>
                    </Table.Td>
                    <Table.Td style={{ whiteSpace: 'nowrap' }}>{cost(f)}</Table.Td>
                  </Table.Tr>
                  {opened === f.id && (
                    <Table.Tr>
                      <Table.Td colSpan={5} bg="dark.7">
                        <Lines session={f.session} {...span(f)} mark={f.call} core={core} />
                      </Table.Td>
                    </Table.Tr>
                  )}
                </Fragment>
              ))}
            </Table.Tbody>
          </Table>
        </Table.ScrollContainer>
        {rows.length > shown && (
          <Group justify="center" p="xs">
            <Button size="compact-sm" variant="subtle" onClick={() => setShown(shown + PAGE)}>
              show {Math.min(PAGE, rows.length - shown)} more of {n(rows.length - shown)}
            </Button>
          </Group>
        )}
      </Paper>
    </Stack>
  )
}

/// The span a finding's lines are read over: from where its cause starts,
/// a few minutes ahead, to a little past it, and to its end when it ran.
function span(f: FoundRow): { from: string; to: string } {
  const end = f.secs != null && f.kind !== 'corrected' && f.kind !== 'asked' ? new Date(new Date(f.at).getTime() + f.secs * 1000).toISOString() : f.at
  const [from, to] = around(f.from, end, 300, 180)
  return { from, to }
}
