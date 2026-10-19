// The fault ledger (#446): per task, every spec version and every build
// of it — who committed each version and why, what changed, how each
// build did against its checks, and what became of its branch. The same
// ledger `superx specs ledger` prints; the page judges nothing more.
import { useQuery } from '@tanstack/react-query'
import { Alert, Anchor, Badge, Box, Card, Container, Group, Loader, Stack, Table, Text, Title, Tooltip } from '@mantine/core'
import { fetchLedger, fetchPing } from './api'
import type { TaskLedger } from './generated/TaskLedger'
import type { VersionLedger } from './generated/VersionLedger'
import type { BuildLine } from './generated/BuildLine'

/// A check's result, as a colour: kept, broken, or nothing to judge by.
const RESULT_COLOR: Record<string, string> = { complied: 'teal', breached: 'red', 'no-evidence': 'gray' }

/// A branch's fate, as a colour: only a merge that stood is green.
function fateColor(b: BuildLine): string {
  if (b.merged) return 'teal'
  if (b.fate_event === 'reverted') return 'orange'
  if (b.fate_event === 'deleted-remote' || b.fate_event === 'deleted') return 'red'
  return 'gray'
}

function Summary({ t }: { t: TaskLedger }) {
  const s = t.summary
  return (
    <Group gap="xs" wrap="wrap">
      <Badge variant="light" color="gray">{s.builds} builds</Badge>
      <Badge variant="light" color={s.merged > 0 ? 'teal' : 'red'}>{s.merged} merged</Badge>
      {s.checks.map((c) => (
        <Badge key={c.check} variant="light" color={c.broken > 0 ? 'orange' : 'teal'}>
          {c.check}: kept {c.kept}, broken {c.broken}
        </Badge>
      ))}
      <Badge variant="light" color={s.after_build > 0 ? 'grape' : 'gray'}>
        {s.after_build} spec versions after a build
        {s.authors.length > 0 && ` — ${s.authors.map((a) => `${a.model} ×${a.versions}`).join(', ')}`}
      </Badge>
    </Group>
  )
}

function Builds({ v }: { v: VersionLedger }) {
  if (v.builds.length === 0) return <Text size="xs" c="dimmed">no build</Text>
  return (
    <Stack gap={6}>
      {v.builds.map((b) => (
        <Box key={`${b.session}-${b.started_at}`}>
          <Group gap={6} wrap="wrap">
            <Text size="xs" ff="monospace">{b.started_at}</Text>
            <Badge size="sm" variant="outline">{b.label}</Badge>
            <Text size="xs" c="dimmed" ff="monospace">{b.session}</Text>
            {b.checks.map((c) => (
              <Tooltip key={c.check} label={c.note} multiline w={420} withArrow>
                <Badge size="sm" variant="light" color={RESULT_COLOR[c.result] ?? 'gray'} style={{ cursor: 'help' }}>
                  {c.check}: {c.result}
                </Badge>
              </Tooltip>
            ))}
          </Group>
          <Text size="xs" c={fateColor(b)} mt={2}>
            {b.branch ? `${b.branch}: ` : ''}
            {b.fate}
          </Text>
        </Box>
      ))}
    </Stack>
  )
}

function Versions({ t }: { t: TaskLedger }) {
  return (
    <Table verticalSpacing="sm" highlightOnHover>
      <Table.Thead>
        <Table.Tr>
          <Table.Th w={56}>Version</Table.Th>
          <Table.Th w="30%">Committed — and why</Table.Th>
          <Table.Th w="16%">Changed</Table.Th>
          <Table.Th>Builds</Table.Th>
        </Table.Tr>
      </Table.Thead>
      <Table.Tbody>
        {t.versions.map((v) => (
          <Table.Tr key={v.version} style={{ verticalAlign: 'top' }}>
            <Table.Td>
              <Text fw={700} ff="monospace">v{v.version}</Text>
            </Table.Td>
            <Table.Td>
              {v.commit ? (
                <Stack gap={4}>
                  <Group gap={6} wrap="wrap">
                    <Text size="xs" ff="monospace">{v.commit.at}</Text>
                    <Badge size="sm" variant="light" color="grape">{v.commit.model ?? 'unknown model'}</Badge>
                    <Text size="xs" c="dimmed" ff="monospace">{v.commit.session}</Text>
                    {v.commit.commits > 1 && <Text size="xs" c="dimmed">({v.commit.commits} commits)</Text>}
                  </Group>
                  <Text size="xs" lineClamp={3} title={v.commit.line}>{v.commit.line}</Text>
                  {v.commit.why && (
                    <Text size="xs" c="dimmed" lineClamp={4} title={v.commit.why}>
                      why: {v.commit.why}
                    </Text>
                  )}
                </Stack>
              ) : (
                <Text size="xs" c="dimmed">no commit of it was seen in a session</Text>
              )}
            </Table.Td>
            <Table.Td>
              {v.changed == null ? (
                <Text size="xs" c="dimmed">clauses unknown — no session read this version whole</Text>
              ) : v.changed.length === 0 ? (
                <Text size="xs" c="dimmed">—</Text>
              ) : (
                <Text size="xs">{v.changed.join('; ')}</Text>
              )}
            </Table.Td>
            <Table.Td>
              <Builds v={v} />
            </Table.Td>
          </Table.Tr>
        ))}
      </Table.Tbody>
    </Table>
  )
}

export default function App() {
  const ping = useQuery({ queryKey: ['ping'], queryFn: fetchPing })
  const ledger = useQuery({ queryKey: ['ledger'], queryFn: fetchLedger, refetchInterval: 60000 })
  return (
    <Container size="xl" py="md">
      <Group justify="space-between" mb="md">
        <Group gap={10}>
          <img src="/logo.svg" alt="" width={26} height={26} style={{ borderRadius: 6 }} />
          <Title order={3}>Specs · the fault ledger</Title>
        </Group>
        {ping.data?.core_url && (
          <Anchor href={ping.data.core_url} size="sm">
            ← SuperX
          </Anchor>
        )}
      </Group>
      <Text size="sm" c="dimmed" mb="md">
        Every spec version and every build of it, read from the captured sessions: who committed each version and
        why, what changed, how each build did against its checks, and what became of its branch. Hover a check for
        its finding. The CLI prints the same: <Text span ff="monospace" size="sm">superx specs ledger</Text>.
      </Text>
      {ledger.isLoading && <Loader size="sm" />}
      {ledger.isError && (
        <Alert color="red" variant="light" title="The ledger could not be read">
          {String((ledger.error as Error)?.message ?? ledger.error)}
        </Alert>
      )}
      {ledger.data && ledger.data.length === 0 && (
        <Alert color="gray" variant="light" title="Nothing recorded yet">
          Run <Text span ff="monospace">superx specs ingest</Text>.
        </Alert>
      )}
      <Stack gap="lg">
        {(ledger.data ?? []).map((t) => (
          <Card key={t.key} withBorder radius="md" padding="md">
            <Group justify="space-between" mb="sm" wrap="wrap">
              <Title order={4} ff="monospace">{t.key}</Title>
              <Summary t={t} />
            </Group>
            <Versions t={t} />
          </Card>
        ))}
      </Stack>
    </Container>
  )
}
