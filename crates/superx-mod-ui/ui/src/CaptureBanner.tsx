// Says when the page cannot be believed (#452). Capture stops on its
// first error (#368), and the daemon's connection can stop answering for
// hours: either way every session used to look idle, which reads the
// same as nobody working. Nothing shows while capture keeps up.
import { useQuery } from '@tanstack/react-query'
import { Alert, Code, Text } from '@mantine/core'
import { fetchCapture } from './api'
import { fmtAge } from './pages/status/parts'

const RESTART = 'superx restart --home ~/.superx'

function since(iso: string | null): string {
  if (!iso) return 'never'
  const secs = Math.max(0, Math.floor((Date.now() - new Date(iso).getTime()) / 1000))
  return `${fmtAge(secs)} ago (${new Date(iso).toLocaleString()})`
}

export function CaptureBanner() {
  const capture = useQuery({ queryKey: ['capture'], queryFn: fetchCapture, refetchInterval: 30000, retry: 1 })
  if (capture.isError) {
    return (
      <Alert color="orange" variant="light" mb="md" title="SuperX is not answering its database">
        <Text size="sm">
          The OS did not answer the page ({String((capture.error as Error)?.message ?? capture.error)}), so nothing here
          is current. Its connection may be stuck (#452). Restart the OS: <Code>{RESTART}</Code>
        </Text>
      </Alert>
    )
  }
  const c = capture.data
  if (!c?.stalled) return null
  return (
    <Alert color="red" variant="light" mb="md" title="Capture has stalled">
      <Text size="sm">
        A transcript change made {since(c.waiting_since)} has not been taken in yet — the newest is{' '}
        {c.transcript ?? 'a session'}'s, {since(c.transcripts_changed)} — and SuperX last recorded anything{' '}
        {since(c.last_written)}. Sessions look idle because nothing new is being taken in. Restart the OS:{' '}
        <Code>{RESTART}</Code>. If it stalls again at once, the database itself may be stuck — a full disk did that on
        2026-10-13.
      </Text>
    </Alert>
  )
}
