import { Badge, Group, Paper, SimpleGrid, Stack, Text } from '@mantine/core'
import type { WatchNow } from '../api'
import { CANCEL, FAIL, INK_MUTED, MONO, OK, UNKNOWN } from '../EChart'
import { clock, since } from '../time'

// Every session as it works: what it is doing, the jobs it has running and
// when the same launch ended last time, so how it stands is answered before
// you ask.

const STATE: Record<WatchNow['state'], string> = {
  working: OK,
  'waiting on the API': CANCEL,
  'job running': CANCEL,
  'waiting on you': INK_MUTED,
  'waiting on your approval': FAIL,
  idle: UNKNOWN,
}

export function Now({ now, pick }: { now: WatchNow[]; pick: (session: string) => void }) {
  if (now.length === 0) return <Text c="dimmed" fz="sm">No session at work in the last six hours.</Text>
  return (
    <SimpleGrid cols={{ base: 1, md: 2, xl: 3 }} spacing="sm">
      {now.map((s) => (
        <Paper key={s.id} withBorder p="sm" radius="md" style={{ cursor: 'pointer' }} onClick={() => pick(s.id)}>
          <Stack gap={6}>
            <Group justify="space-between" wrap="nowrap" align="flex-start">
              <Stack gap={0} style={{ minWidth: 0 }}>
                <Text fw={700} ff={MONO} fz="sm" style={{ overflowWrap: 'anywhere' }}>{s.name}</Text>
                <Text fz="xs" c="dimmed" style={{ overflowWrap: 'anywhere' }}>{[s.model, s.folder].filter(Boolean).join(' · ')}</Text>
              </Stack>
              <Badge color={s.api?.resent ? FAIL : STATE[s.state]} variant="light">{s.state}</Badge>
            </Group>
            {s.api && (
              <Text fz="sm" style={{ overflowWrap: 'anywhere' }}>
                no answer since {clock(s.api.since)}{' '}
                <Text span c="dimmed" fz="xs">
                  · {since(s.api.since)}
                  {s.api.resent
                    ? ` · its request sent again ${s.api.resent} ${s.api.resent === 1 ? 'time' : 'times'}, first at ${clock(s.api.first_resent)}`
                    : ` · nothing sent again: ${s.api.model ?? 'the model'}${s.api.effort ? ` at effort ${s.api.effort}` : ''} is at work on one answer, and its thinking shows when the answer lands`}
                </Text>
              </Text>
            )}
            {s.doing && (
              <Text fz="sm" style={{ overflowWrap: 'anywhere' }}>
                {s.doing.tool}: {s.doing.what} <Text span c="dimmed" fz="xs">· {since(s.doing.since)}</Text>
              </Text>
            )}
            {s.jobs.map((j) => (
              <Text key={j.since + j.what} fz="sm" style={{ overflowWrap: 'anywhere' }}>
                job: {j.what}{' '}
                <Text span c="dimmed" fz="xs">
                  · running {since(j.since)}
                  {j.expected_end ? ` · the same launch ended by ${clock(j.expected_end)} last time` : ''}
                </Text>
              </Text>
            ))}
            <Text fz="xs" c="dimmed">newest line {clock(s.newest)} · click to see only this session</Text>
          </Stack>
        </Paper>
      ))}
    </SimpleGrid>
  )
}
