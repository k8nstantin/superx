import { Alert, Group, Stack, Text, Title } from '@mantine/core'
import { useQuery } from '@tanstack/react-query'
import { fetchCompare } from '../api'
import { ModelComparison } from './Compare'

// Which model to reach for (/api/results/compare): mod-ui's comparison, its
// measures computed from recon's facts — the sessions' replies, tokens and
// prompts correlated with the commits the crawls placed, landed and blamed.
export function Comparison() {
  const compare = useQuery({ queryKey: ['compare'], queryFn: fetchCompare, refetchInterval: 300_000 })
  return (
    <Stack gap="sm">
      <Group justify="space-between" align="baseline">
        <Title order={3}>Which model to reach for</Title>
        <Text size="xs" c="dimmed">
          effectiveness, cost and your time, per model · from every session correlated with every crawled commit · every 5 minutes
        </Text>
      </Group>
      {compare.isError && <Alert color="red" title="The comparison did not load">{String(compare.error)}</Alert>}
      {compare.isLoading && <Text c="dimmed">Correlating every session with every commit, placement, landing and blame…</Text>}
      <ModelComparison c={compare.data} />
    </Stack>
  )
}
