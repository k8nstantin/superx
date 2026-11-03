import { Alert, Badge, Group, Paper, Progress, Stack, Table, Text, Title } from '@mantine/core'
import { useQuery } from '@tanstack/react-query'
import { fetchNow, type HostNow, type PassNow, type RepoNow } from '../api'
import { CANCEL, FAIL, OK, PELICAN, UNKNOWN } from '../EChart'
import { clock, since } from '../time'

// What the crawler is doing this second (/api/now, every 2 s): the pass,
// its phase, and each repository in it from waiting to done.

function tone(r: RepoNow): string {
  if (r.state === 'waiting') return UNKNOWN
  if (r.state === 'running') return PELICAN
  if (r.outcome === 'complete') return OK
  if (r.outcome === 'partial') return CANCEL
  if (r.outcome === 'stopped') return UNKNOWN
  return FAIL
}

function Dot({ color }: { color: string }) {
  return <span style={{ display: 'inline-block', width: 9, height: 9, borderRadius: 9, background: color, flex: 'none' }} />
}

function Host({ h }: { h: HostNow }) {
  return (
    <Text fz="xs" ff="monospace" style={{ overflowWrap: 'anywhere' }}>
      {h.name}: {h.calls} calls{h.failed > 0 ? `, ${h.failed} failed` : ''}{h.waits > 0 ? `, ${h.waits} waits` : ''}
      {h.done ? ' · done' : h.asking ? ` · asking ${h.asking}` : ''}
    </Text>
  )
}

function Row({ r }: { r: RepoNow }) {
  const label = r.state === 'running' ? `${r.step}` : r.state === 'done' ? (r.outcome ?? 'done') : 'waiting'
  return (
    <Table.Tr>
      <Table.Td>
        <Group gap={8} wrap="nowrap">
          <Dot color={tone(r)} />
          <div>
            <Text fz="sm" fw={600}>{r.name}</Text>
            <Text fz="xs" c="dimmed" ff="monospace" style={{ overflowWrap: 'anywhere' }}>{r.common_dir}</Text>
          </div>
        </Group>
      </Table.Td>
      <Table.Td miw={220}>
        <Group justify="space-between" gap={6}>
          <Text fz="sm">{label}</Text>
          <Text fz="xs" c="dimmed">step {r.step_index} of {r.steps}</Text>
        </Group>
        <Progress value={(100 * r.step_index) / r.steps} size="sm" color={r.state === 'done' ? (r.outcome === 'complete' ? 'teal' : 'yellow') : 'pelican'} />
        {r.step === 'survival' && r.to_blame > 0 && (
          <Text fz="xs" c="dimmed">blamed {r.blamed.toLocaleString('en-US')} of {r.to_blame.toLocaleString('en-US')} files</Text>
        )}
      </Table.Td>
      <Table.Td ta="right">
        <Text fz="sm">{r.state === 'running' ? since(r.step_since) : r.state === 'done' ? since(r.started_at, r.finished_at) : '—'}</Text>
        <Text fz="xs" c="dimmed">{r.state === 'running' ? 'in this step' : r.state === 'done' ? 'its crawl' : ''}</Text>
      </Table.Td>
      <Table.Td ta="right">
        <Text fz="sm">{r.git_calls.toLocaleString('en-US')}</Text>
        {r.git_failed > 0 && <Text fz="xs" c="red.4">{r.git_failed} failed</Text>}
      </Table.Td>
      <Table.Td>{r.hosts.length === 0 ? <Text fz="xs" c="dimmed">—</Text> : r.hosts.map((h) => <Host key={h.name} h={h} />)}</Table.Td>
    </Table.Tr>
  )
}

function Header({ p, running }: { p: PassNow; running: boolean }) {
  const counts = { waiting: 0, running: 0, done: 0 }
  for (const r of p.repos) counts[r.state] += 1
  return (
    <Stack gap={6}>
      <Group gap="xs">
        <Badge color={running ? 'pelican' : p.outcome === 'complete' ? 'teal' : p.outcome === 'partial' ? 'yellow' : 'gray'} variant={running ? 'filled' : 'light'}>
          {running ? 'running' : `last pass: ${p.outcome}`}
        </Badge>
        <Text fz="sm">
          started {clock(p.started_at)} UTC · {running ? `${since(p.started_at)} so far` : `took ${since(p.started_at, p.finished_at)}, ended ${clock(p.finished_at)}`}
          {!running && p.rows != null ? ` · ${p.rows.toLocaleString('en-US')} rows written` : ''}
        </Text>
      </Group>
      <Group gap="xs">
        <Badge variant={p.lines_read == null ? 'outline' : 'light'} color={p.phase === 'lines' ? 'pelican' : 'teal'}>
          lines {p.lines_read == null ? 'reading' : `${p.lines_read.toLocaleString('en-US')} read of ${(p.lines_held ?? 0).toLocaleString('en-US')}`}
        </Badge>
        <Text c="dimmed">→</Text>
        <Badge variant="light" color={p.phase === 'repositories' ? 'pelican' : p.phase === 'lines' ? 'gray' : 'teal'}>
          repositories: {counts.done} done · {counts.running} running · {counts.waiting} waiting
        </Badge>
        <Text c="dimmed">→</Text>
        <Badge variant="light" color={p.phase === 'recording' ? 'pelican' : p.phase === 'done' ? 'teal' : 'gray'}>
          recorded
        </Badge>
      </Group>
    </Stack>
  )
}

export function Now() {
  const now = useQuery({ queryKey: ['now'], queryFn: fetchNow, refetchInterval: 2_000 })
  const p = now.data?.pass ?? now.data?.last ?? null
  const running = now.data?.pass != null
  // Most passes only read the new lines: their repositories are those the
  // last crawl left.
  const own = p != null && p.repos.length > 0
  const crawl = own ? p : now.data?.last_crawl ?? null
  return (
    <Stack gap="sm">
      <Group justify="space-between" align="baseline">
        <Title order={3}>Now</Title>
        <Text size="xs" c="dimmed">what the crawler is doing this second · every 2 s</Text>
      </Group>
      {now.isError && <Alert color="red" title="What runs could not be read">{String(now.error)}</Alert>}
      {now.isSuccess && p == null && <Text c="dimmed">No pass since superx-recon started: the first one starts within a few seconds.</Text>}
      {p != null && (
        <Paper withBorder p="md" radius="md">
          <Stack gap="sm">
            <Header p={p} running={running} />
            {crawl != null && !own && (
              <Text fz="sm" c="dimmed">
                This pass crawls no repository. As the last crawl left them: started {clock(crawl.started_at)}, took {since(crawl.started_at, crawl.finished_at)}, {crawl.outcome}.
              </Text>
            )}
            {crawl != null && crawl.repos.length > 0 && (
              <Table.ScrollContainer minWidth={900}>
                <Table fz="sm" verticalSpacing="xs">
                  <Table.Thead>
                    <Table.Tr>
                      <Table.Th>Repository</Table.Th>
                      <Table.Th>Where it is</Table.Th>
                      <Table.Th ta="right">Time</Table.Th>
                      <Table.Th ta="right">Git calls</Table.Th>
                      <Table.Th>Its host repositories</Table.Th>
                    </Table.Tr>
                  </Table.Thead>
                  <Table.Tbody>
                    {crawl.repos.map((r) => <Row key={r.common_dir} r={r} />)}
                  </Table.Tbody>
                </Table>
              </Table.ScrollContainer>
            )}
            {crawl == null && p.phase !== 'lines' && (
              <Text fz="sm" c="dimmed">No repository crawled since superx-recon started: a full crawl comes every attr_recon_crawl_secs.</Text>
            )}
          </Stack>
        </Paper>
      )}
    </Stack>
  )
}
