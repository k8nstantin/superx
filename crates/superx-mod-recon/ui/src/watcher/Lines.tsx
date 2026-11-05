import { useState } from 'react'
import { Alert, Anchor, Badge, Button, Group, Loader, Paper, Spoiler, Stack, Text } from '@mantine/core'
import { useQuery } from '@tanstack/react-query'
import { fetchLines, type LineRow } from '../api'
import { CANCEL, FAIL, INK_MUTED, MONO, OK } from '../EChart'
import { clock, day } from '../time'
import { took } from './data'

// A session's own lines over a span: how anything the watcher raised or
// found is read back to its cause. The operator's words, the agent's words
// and thoughts, every call with how long it ran and how it ended, every
// failure, every job's notice, every compaction; the newest of the span
// first shown, the earlier ones a click away.

const WHO: Record<LineRow['kind'], [string, string]> = {
  you: ['you', '#d55181'],
  thought: ['thought', INK_MUTED],
  said: ['agent', '#199e70'],
  call: ['call', '#3987e5'],
  failed: ['failed', FAIL],
  job_ended: ['job', CANCEL],
  compacted: ['context', CANCEL],
}

const key = (l: LineRow) => `${l.at}|${l.kind}|${l.call ?? ''}|${l.text.slice(0, 40)}`

function Line({ l, mark }: { l: LineRow; mark: boolean }) {
  const [label, colour] = WHO[l.kind]
  const how =
    l.kind === 'call'
      ? [l.secs != null ? took(l.secs) : null, l.end == null ? 'running' : l.failed ? 'failed' : (l.status ?? null), l.background ? 'background' : null]
          .filter(Boolean)
          .join(' · ')
      : ''
  return (
    <Group
      gap="xs"
      wrap="nowrap"
      align="flex-start"
      px={6}
      py={3}
      style={{ borderLeft: `3px solid ${mark ? FAIL : colour}`, background: mark ? 'rgba(230,103,103,0.10)' : undefined, borderRadius: 3 }}
    >
      <Text fz="xs" ff={MONO} c="dimmed" w={64} style={{ flex: 'none' }}>
        {clock(l.at)}
      </Text>
      <Badge size="xs" variant="light" color={colour} w={64} style={{ flex: 'none' }}>
        {label}
      </Badge>
      <Stack gap={2} style={{ minWidth: 0, flex: 1 }}>
        {l.kind === 'call' && (
          <Text fz="xs" fw={600}>
            {l.tool}
            {l.target ? ` · ${l.target}` : ''}
            {how ? <Text span fz="xs" c={l.failed ? FAIL : l.end == null ? CANCEL : 'dimmed'}> · {how}</Text> : null}
          </Text>
        )}
        {l.kind === 'compacted' ? (
          <Text fz="xs" c="dimmed">the context compacted: what the agent held is a summary from here on</Text>
        ) : l.kind === 'thought' && !l.text.trim() ? (
          <Text fz="xs" c="dimmed" fs="italic">(its thinking, not kept in words)</Text>
        ) : (
          <Spoiler maxHeight={l.kind === 'you' || l.kind === 'said' ? 120 : 54} showLabel="more" hideLabel="less">
            <Text
              fz="xs"
              ff={l.kind === 'call' || l.kind === 'failed' ? MONO : undefined}
              fs={l.kind === 'thought' ? 'italic' : undefined}
              c={l.kind === 'thought' ? 'dimmed' : l.kind === 'failed' ? FAIL : undefined}
              fw={l.kind === 'you' ? 600 : undefined}
              style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}
            >
              {l.text}
            </Text>
          </Spoiler>
        )}
      </Stack>
    </Group>
  )
}

/// The lines of `session` from `from` to `to`; `mark` is the call the
/// drill is about, drawn apart.
export function Lines({ session, from, to, mark, core }: { session: string; from: string; to: string; mark?: string | null; core?: string | null }) {
  const first = useQuery({ queryKey: ['lines', session, from, to], queryFn: () => fetchLines(session, from, to), staleTime: 60_000 })
  const [before, setBefore] = useState<LineRow[]>([])
  const [more, setMore] = useState<boolean | null>(null)
  const [loading, setLoading] = useState(false)
  if (first.isPending) return <Loader size="xs" />
  if (first.isError) return <Alert color="red" title="The lines did not load">{String(first.error)}</Alert>
  const seen = new Set<string>()
  // A thought with no words kept says nothing you can use.
  const lines = [...before, ...first.data.lines].filter((l) => {
    if (l.kind === 'thought' && !l.text.trim()) return false
    const k = key(l)
    if (seen.has(k)) return false
    seen.add(k)
    return true
  })
  const earlier = more ?? first.data.earlier
  const older = async () => {
    if (!lines.length) return
    setLoading(true)
    try {
      const page = await fetchLines(session, from, lines[0].at)
      setBefore([...page.lines, ...before])
      setMore(page.earlier)
    } finally {
      setLoading(false)
    }
  }
  return (
    <Paper withBorder radius="sm" p="xs">
      <Stack gap={4}>
        <Group justify="space-between">
          <Text fz="xs" c="dimmed">
            the session's lines, {day(from)} to {day(to)} · {lines.length} shown
          </Text>
          {core && (
            <Anchor fz="xs" href={`${core}#page=sessions&session=${encodeURIComponent(session)}`} target="_blank">
              open the session in SuperX
            </Anchor>
          )}
        </Group>
        {earlier && (
          <Button size="compact-xs" variant="subtle" loading={loading} onClick={older} style={{ alignSelf: 'flex-start' }}>
            show the earlier lines
          </Button>
        )}
        {lines.length === 0 && <Text fz="xs" c="dimmed">No line of this session in the span: it is older than the watcher's window.</Text>}
        {lines.map((l) => (
          <Line key={key(l)} l={l} mark={!!mark && l.call === mark} />
        ))}
        {lines.some((l) => l.kind === 'call' && l.end == null) && (
          <Text fz="xs" c={OK}>a call still running is marked running</Text>
        )}
      </Stack>
    </Paper>
  )
}

/// A span around a moment: `before` seconds ahead of `from`, `after`
/// seconds past `to`.
export function around(from: string, to: string, before = 300, after = 120): [string, string] {
  const a = new Date(new Date(from).getTime() - before * 1000)
  const b = new Date(new Date(to).getTime() + after * 1000)
  return [a.toISOString(), b.toISOString()]
}
