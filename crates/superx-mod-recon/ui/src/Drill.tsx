import { useState } from 'react'
import { Alert, Badge, Drawer, Group, Loader, Stack, Table, Tabs, Text } from '@mantine/core'
import { useQuery } from '@tanstack/react-query'
import { fetchDay, type DayCommit } from './api'
import { clock } from './time'

// What is behind a bar: one model's commits, edits and sessions on one UTC
// day (/api/results/day), counted as the charts count them. A commit opens
// to its files.

const n = (v: number) => v.toLocaleString('en-US')

function Place({ c }: { c: DayCommit }) {
  if (c.landed) return <Badge size="xs" color="teal" variant="light">{c.place === 'mainline' ? 'on the main line' : 'landed via another commit'}</Badge>
  if (c.place === 'in_flight') return <Badge size="xs" color="pelican" variant="light">in flight</Badge>
  if (c.place === 'branch') return <Badge size="xs" color="yellow" variant="light">branch, not landed</Badge>
  if (c.place === 'recovered') return <Badge size="xs" color="red" variant="light">recovered, unreachable</Badge>
  return <Badge size="xs" color="gray" variant="light">not placed yet</Badge>
}

function CommitRow({ c }: { c: DayCommit }) {
  const [open, setOpen] = useState(false)
  return (
    <>
      <Table.Tr style={{ cursor: 'pointer' }} onClick={() => setOpen(!open)}>
        <Table.Td><Text fz="xs" ff="monospace">{open ? '▾' : '▸'} {c.hash.slice(0, 10)}</Text></Table.Td>
        <Table.Td><Text fz="xs">{c.repo}</Text></Table.Td>
        <Table.Td><Text fz="sm" lineClamp={2}>{c.subject}</Text></Table.Td>
        <Table.Td><Text fz="xs" ff="monospace">{clock(c.committed_at)}</Text></Table.Td>
        <Table.Td ta="right"><Text fz="sm" c="teal.4">+{n(c.added)}</Text><Text fz="xs" c="red.4">−{n(c.removed)}</Text></Table.Td>
        <Table.Td><Place c={c} /></Table.Td>
        <Table.Td ta="right"><Text fz="sm">{c.alive == null ? '—' : n(c.alive)}</Text></Table.Td>
      </Table.Tr>
      {open && (
        <Table.Tr>
          <Table.Td colSpan={7} bg="dark.7">
            <Stack gap={2}>
              {c.session && <Text fz="xs" c="dimmed">session {c.session}</Text>}
              {c.files.map((f) => (
                <Group key={f.path} gap="xs" wrap="nowrap">
                  <Text fz="xs" ff="monospace" c="teal.4" w={60} ta="right">+{n(f.added)}</Text>
                  <Text fz="xs" ff="monospace" c="red.4" w={60} ta="right">−{n(f.removed)}</Text>
                  <Text fz="xs" c="dimmed" w={70}>{f.change}</Text>
                  <Text fz="xs" ff="monospace" style={{ overflowWrap: 'anywhere' }}>{f.path}</Text>
                </Group>
              ))}
            </Stack>
          </Table.Td>
        </Table.Tr>
      )}
    </>
  )
}

export function Drill({ model, day, onClose }: { model: string | null; day: string | null; onClose: () => void }) {
  const open = model != null && day != null
  const detail = useQuery({ queryKey: ['day', model, day], queryFn: () => fetchDay(model!, day!), enabled: open })
  const d = detail.data
  return (
    <Drawer opened={open} onClose={onClose} position="right" size="80%" title={<Text fw={700}>{model} · {day} (UTC)</Text>}>
      {detail.isLoading && <Group><Loader size="sm" /><Text c="dimmed">Reading what is behind it…</Text></Group>}
      {detail.isError && <Alert color="red" title="It could not be read">{String(detail.error)}</Alert>}
      {d && (
        <Tabs defaultValue={d.commits.length > 0 ? 'commits' : d.edits.length > 0 ? 'edits' : 'sessions'}>
          <Tabs.List>
            <Tabs.Tab value="commits">Commits ({d.commits.length}) · +{n(d.commits.reduce((a, c) => a + c.added, 0))} −{n(d.commits.reduce((a, c) => a + c.removed, 0))}</Tabs.Tab>
            <Tabs.Tab value="edits">Edits ({d.edits.length}) · +{n(d.edits.reduce((a, e) => a + e.added, 0))} −{n(d.edits.reduce((a, e) => a + e.removed, 0))}</Tabs.Tab>
            <Tabs.Tab value="sessions">Sessions ({d.sessions.length})</Tabs.Tab>
          </Tabs.List>
          <Tabs.Panel value="commits" pt="sm">
            {d.commits.length === 0 ? <Text c="dimmed">No commit this day was made by a call of this model.</Text> : (
              <Table.ScrollContainer minWidth={900}>
                <Table fz="sm" striped highlightOnHover>
                  <Table.Thead>
                    <Table.Tr>
                      <Table.Th>Commit</Table.Th>
                      <Table.Th>Repository</Table.Th>
                      <Table.Th>Subject</Table.Th>
                      <Table.Th>At</Table.Th>
                      <Table.Th ta="right">Lines</Table.Th>
                      <Table.Th>Where it is</Table.Th>
                      <Table.Th ta="right">Alive now</Table.Th>
                    </Table.Tr>
                  </Table.Thead>
                  <Table.Tbody>{d.commits.map((c) => <CommitRow key={c.hash} c={c} />)}</Table.Tbody>
                </Table>
              </Table.ScrollContainer>
            )}
          </Tabs.Panel>
          <Tabs.Panel value="edits" pt="sm">
            {d.edits.length === 0 ? <Text c="dimmed">No edit this day.</Text> : (
              <Table.ScrollContainer minWidth={900}>
                <Table fz="sm" striped>
                  <Table.Thead>
                    <Table.Tr>
                      <Table.Th>At</Table.Th>
                      <Table.Th>File</Table.Th>
                      <Table.Th ta="right">Lines</Table.Th>
                      <Table.Th>Tool</Table.Th>
                      <Table.Th>Session</Table.Th>
                    </Table.Tr>
                  </Table.Thead>
                  <Table.Tbody>
                    {d.edits.map((e, i) => (
                      <Table.Tr key={`${e.at}-${e.path}-${i}`}>
                        <Table.Td><Text fz="xs" ff="monospace">{clock(e.at)}</Text></Table.Td>
                        <Table.Td><Text fz="xs" ff="monospace" style={{ overflowWrap: 'anywhere' }}>{e.path}</Text></Table.Td>
                        <Table.Td ta="right"><Text fz="xs" c="teal.4">+{n(e.added)}</Text><Text fz="xs" c="red.4">−{n(e.removed)}</Text></Table.Td>
                        <Table.Td><Text fz="xs">{e.tool}{e.operator_changed ? ' · you changed it' : ''}</Text></Table.Td>
                        <Table.Td><Text fz="xs" ff="monospace" c="dimmed">{e.session?.slice(0, 8) ?? '—'}</Text></Table.Td>
                      </Table.Tr>
                    ))}
                  </Table.Tbody>
                </Table>
              </Table.ScrollContainer>
            )}
          </Tabs.Panel>
          <Tabs.Panel value="sessions" pt="sm">
            <Table fz="sm" striped>
              <Table.Thead>
                <Table.Tr>
                  <Table.Th>Session</Table.Th>
                  <Table.Th>Agent</Table.Th>
                  <Table.Th ta="right">Replies by this model</Table.Th>
                  <Table.Th>From – to</Table.Th>
                </Table.Tr>
              </Table.Thead>
              <Table.Tbody>
                {d.sessions.map((s) => (
                  <Table.Tr key={s.session}>
                    <Table.Td><Text fz="xs" ff="monospace">{s.session}</Text></Table.Td>
                    <Table.Td><Text fz="xs">{s.agent}</Text></Table.Td>
                    <Table.Td ta="right">{n(s.replies)}</Table.Td>
                    <Table.Td><Text fz="xs" ff="monospace">{clock(s.first)} – {clock(s.last)}</Text></Table.Td>
                  </Table.Tr>
                ))}
              </Table.Tbody>
            </Table>
          </Tabs.Panel>
        </Tabs>
      )}
    </Drawer>
  )
}
