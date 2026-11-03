import { Alert, Badge, Group, Paper, Stack, Table, Text, Title } from '@mantine/core'
import { useQuery } from '@tanstack/react-query'
import { fetchRepos, type HostLast, type Repo } from '../api'
import { day, since } from '../time'

// Every repository recon holds, each with its last crawl as its crawl row
// holds it, and the host repositories crawled beside it (/api/repos).

const n = (v: number) => v.toLocaleString('en-US')

function Outcome({ outcome }: { outcome: string }) {
  const color = outcome === 'complete' ? 'teal' : outcome === 'partial' ? 'yellow' : outcome === 'unreadable' ? 'red' : 'gray'
  return <Badge size="sm" variant="light" color={color}>{outcome}</Badge>
}

function Note({ note }: { note: string | null }) {
  if (!note) return null
  return (
    <Text fz="xs" c="dimmed" lineClamp={2} title={note} style={{ overflowWrap: 'anywhere' }}>
      {note}
    </Text>
  )
}

function Host({ h }: { h: HostLast }) {
  return (
    <Stack gap={0}>
      <Group gap={6} wrap="nowrap">
        <Outcome outcome={h.outcome} />
        <Text fz="xs" ff="monospace" style={{ overflowWrap: 'anywhere' }}>{h.name}</Text>
      </Group>
      <Text fz="xs" c="dimmed">
        {day(h.finished_at)} · {n(h.calls)} calls{h.failed > 0 ? `, ${n(h.failed)} failed` : ''}, {n(h.not_modified)} unchanged
        {h.rate_waits > 0 ? `, ${n(h.rate_waits)} waits` : ''} · objects {n(h.objects_new)} new, {n(h.objects_changed)} changed
      </Text>
      <Note note={h.note} />
    </Stack>
  )
}

function Row({ r }: { r: Repo }) {
  const c = r.last
  return (
    <Table.Tr>
      <Table.Td miw={240}>
        <Text fz="sm" fw={600}>{r.name}</Text>
        <Text fz="xs" c="dimmed" ff="monospace" style={{ overflowWrap: 'anywhere' }}>{r.common_dir}</Text>
        <Text fz="xs" c="dimmed">{n(r.crawls)} crawls · last complete {day(r.last_complete)}</Text>
      </Table.Td>
      <Table.Td>
        {c == null ? <Text fz="sm" c="dimmed">never crawled</Text> : (
          <Stack gap={2}>
            <Group gap={6}>
              <Outcome outcome={c.outcome} />
              <Text fz="xs">{day(c.finished_at)} · took {since(c.started_at, c.finished_at)}</Text>
            </Group>
            <Note note={c.note} />
          </Stack>
        )}
      </Table.Td>
      <Table.Td ta="right">
        {c && <Text fz="sm">{n(c.commits_read)} read</Text>}
        <Text fz="xs" c="dimmed">{n(r.commits_held)} held{c && c.commits_new > 0 ? ` · ${n(c.commits_new)} new` : ''}</Text>
      </Table.Td>
      <Table.Td ta="right">
        {c && (
          <>
            <Text fz="sm">{n(c.git_calls)}</Text>
            {(c.git_failed > 0 || c.git_timeouts > 0) && (
              <Text fz="xs" c="red.4">{n(c.git_failed)} failed{c.git_timeouts > 0 ? `, ${n(c.git_timeouts)} cut off` : ''}</Text>
            )}
          </>
        )}
      </Table.Td>
      <Table.Td ta="right">
        {c && (
          <>
            <Text fz="sm">{n(c.files_blamed)}</Text>
            <Text fz="xs" c="dimmed">{c.files_left > 0 ? `${n(c.files_left)} left` : 'none left'}</Text>
          </>
        )}
      </Table.Td>
      <Table.Td>
        {r.hosts.length === 0 ? <Text fz="xs" c="dimmed">none</Text> : <Stack gap={6}>{r.hosts.map((h) => <Host key={h.name} h={h} />)}</Stack>}
      </Table.Td>
    </Table.Tr>
  )
}

export function Repos() {
  const repos = useQuery({ queryKey: ['repos'], queryFn: fetchRepos, refetchInterval: 30_000 })
  const list = repos.data?.repos ?? []
  const partial = list.filter((r) => r.last && r.last.outcome !== 'complete').length
  return (
    <Stack gap="sm">
      <Group justify="space-between" align="baseline">
        <Title order={3}>Repositories</Title>
        <Text size="xs" c="dimmed">
          {list.length > 0 ? `${list.length} repositories · ${partial} not complete at their last crawl · ` : ''}each as its last crawl left it · every 30 s
        </Text>
      </Group>
      {repos.isError && <Alert color="red" title="The repositories did not load">{String(repos.error)}</Alert>}
      {repos.isSuccess && list.length === 0 && <Text c="dimmed">No repository found yet.</Text>}
      {list.length > 0 && (
        <Paper withBorder radius="md">
          <Table.ScrollContainer minWidth={1100}>
            <Table fz="sm" verticalSpacing="xs" striped>
              <Table.Thead>
                <Table.Tr>
                  <Table.Th>Repository</Table.Th>
                  <Table.Th>Last crawl</Table.Th>
                  <Table.Th ta="right">Commits</Table.Th>
                  <Table.Th ta="right">Git calls</Table.Th>
                  <Table.Th ta="right">Files blamed</Table.Th>
                  <Table.Th>Its host repositories, at their last crawl</Table.Th>
                </Table.Tr>
              </Table.Thead>
              <Table.Tbody>
                {list.map((r) => <Row key={r.common_dir} r={r} />)}
              </Table.Tbody>
            </Table>
          </Table.ScrollContainer>
        </Paper>
      )}
    </Stack>
  )
}
