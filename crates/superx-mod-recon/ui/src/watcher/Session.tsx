import { Badge, Group, Paper, Stack, Text } from '@mantine/core'
import type { WatchReport, WatchSession } from '../api'
import { CANCEL, FAIL, OK, PELICAN, UNKNOWN } from '../EChart'
import { clock, since } from '../time'
import { Clamped, inWords, LevelBadge, MemoryFiles } from './parts'

// One session as the watcher holds it (#566, step 19): who it is, what it
// is doing this second, its words and the operator's, the orders in force,
// the run since the operator's newest message, its jobs, its recap, the task
// file it executes, its memory and its reports.

const STATE_COLOUR: Record<WatchSession['state'], string> = {
  stalled: FAIL,
  working: PELICAN,
  'behind a job': CANCEL,
  'waiting on you': OK,
  idle: UNKNOWN,
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <Stack gap={0}>
      <Text fz="xs" c="dimmed" tt="uppercase" style={{ letterSpacing: 0.4 }}>
        {label}
      </Text>
      {children}
    </Stack>
  )
}

export function Session({ s, reports }: { s: WatchSession; reports: WatchReport[] }) {
  const calls = Object.entries(s.run.calls)
  const message = s.message
  return (
    <Paper withBorder p="md" radius="md">
      <Stack gap="sm">
        <Group justify="space-between" align="flex-start" wrap="nowrap">
          <Stack gap={0} style={{ minWidth: 0 }}>
            <Text fw={700} ff="monospace" style={{ overflowWrap: 'anywhere' }}>{s.name}</Text>
            <Text fz="xs" c="dimmed" style={{ overflowWrap: 'anywhere' }}>
              {s.agent}
              {s.model ? ` · ${s.model}` : ''}
              {s.folder ? ` · ${s.folder}` : ''}
            </Text>
          </Stack>
          <Stack gap={2} align="flex-end">
            <Badge color={STATE_COLOUR[s.state]}>{s.state}</Badge>
            <Text fz="xs" c="dimmed">newest line {clock(s.newest)}</Text>
          </Stack>
        </Group>

        <Field label="Doing now">
          {s.doing ? (
            <Text fz="sm" ff="monospace" style={{ overflowWrap: 'anywhere' }}>
              {s.doing.tool}: {s.doing.what} <Text span c="dimmed" fz="xs">· {since(s.doing.since)}</Text>
            </Text>
          ) : (
            <Text fz="sm" c="dimmed">no call running</Text>
          )}
        </Field>

        {s.said && (
          <Field label={`Its newest words · ${clock(s.said.at)}`}>
            <Clamped text={s.said.text} />
          </Field>
        )}

        {message && (
          <Field label={`Your newest message · ${clock(message.at)} · ${message.class ? inWords(message.class) : message.came}`}>
            <Group gap={4}>
              {message.correction && <Badge size="xs" color={CANCEL} variant="light">correction</Badge>}
              {message.escalation && <Badge size="xs" color={FAIL} variant="light">escalation</Badge>}
            </Group>
            <Clamped text={message.text} />
          </Field>
        )}

        <Field label="Orders in force">
          {s.orders != null ? <Clamped text={s.orders} lines={2} /> : <Text fz="sm" c="dimmed">no order known in the window: nothing is judged unordered</Text>}
        </Field>

        {s.run.since && (
          <Field label={`The run since your message · ${since(s.run.since)}`}>
            <Text fz="sm">
              {calls.length === 0 ? 'no tool call' : calls.map(([tool, n]) => `${tool} ${n}`).join(', ')}
              {` · ${s.run.lines.toLocaleString('en-US')} lines written`}
            </Text>
          </Field>
        )}

        {s.jobs.length > 0 && (
          <Field label="Jobs running">
            {s.jobs.map((j) => (
              <Text key={`${j.tool}${j.since}`} fz="sm" ff="monospace" style={{ overflowWrap: 'anywhere' }}>
                {j.tool}: {j.what} <Text span c="dimmed" fz="xs">· {since(j.since)}</Text>
              </Text>
            ))}
          </Field>
        )}

        {s.recap && (
          <Field label={`Its recap · ${clock(s.recap.at)}`}>
            <Clamped text={s.recap.text} />
          </Field>
        )}

        {s.spec && (
          <Field label="The task file it executes">
            <Text fz="sm" ff="monospace" style={{ overflowWrap: 'anywhere' }}>{s.spec.path}</Text>
            {s.spec.why ? (
              <Text fz="sm" c="red.4">{s.spec.why}</Text>
            ) : (
              <>
                <Text fz="sm">
                  version {s.spec.version}
                  {s.spec.first_version && s.spec.first_version !== s.spec.version ? ` (it started on ${s.spec.first_version})` : ''}
                </Text>
                <Text fz="xs" c="dimmed" style={{ overflowWrap: 'anywhere' }}>owns: {s.spec.owns.join('; ')}</Text>
                <Text fz="xs" c="dimmed" style={{ overflowWrap: 'anywhere' }}>forbids: {s.spec.forbid.join('; ')}</Text>
              </>
            )}
          </Field>
        )}

        <Field label="Its memory">
          {s.memory.files.length > 0 ? (
            <MemoryFiles session={s.id} files={s.memory.files} />
          ) : (
            <Text fz="sm" c="dimmed">{s.memory.why ?? 'no file'}</Text>
          )}
        </Field>

        {reports.length > 0 && (
          <Field label={`Its reports · ${reports.length}`}>
            <Stack gap={4}>
              {reports.map((r) => (
                <Group key={r.id} gap={6} wrap="nowrap" align="flex-start">
                  <LevelBadge level={r.level} />
                  <Text fz="sm">
                    <Text span fw={600}>{inWords(r.kind)}</Text>
                    <Text span c="dimmed" fz="xs">{` · ${clock(r.at)}`}</Text> {r.what}
                  </Text>
                </Group>
              ))}
            </Stack>
          </Field>
        )}
      </Stack>
    </Paper>
  )
}
