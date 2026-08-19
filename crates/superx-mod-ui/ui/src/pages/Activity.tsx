import { useState } from 'react'
import { Badge, Card, Group, ScrollArea, Switch, Text, Title } from '@mantine/core'
import { useSse } from '../useSse'
import type { SseEvent } from './../generated/SseEvent'

const MAX_ROWS = 500

export default function ActivityPage() {
  const [rows, setRows] = useState<SseEvent[]>([])
  const [paused, setPaused] = useState(false)
  const [kind, setKind] = useState<'all' | 'message' | 'action'>('all')

  useSse((e) => {
    setRows((prev) => [e, ...prev].slice(0, MAX_ROWS))
  }, paused)

  const visible = rows.filter((r) => kind === 'all' || r.kind === kind)

  return (
    <Card withBorder>
      <Group justify="space-between" mb="xs">
        <Title order={5}>Live activity — everything the OS captures, as it happens</Title>
        <Group>
          {(['all', 'message', 'action'] as const).map((k) => (
            <Badge
              key={k}
              variant={kind === k ? 'filled' : 'outline'}
              style={{ cursor: 'pointer' }}
              onClick={() => setKind(k)}
            >
              {k}
            </Badge>
          ))}
          <Switch label="pause" checked={paused} onChange={(e) => setPaused(e.currentTarget.checked)} />
        </Group>
      </Group>
      <ScrollArea h={560}>
        {visible.length === 0 && (
          <Text c="dimmed" size="sm">
            waiting for events… (work in any coding agent to see them land here)
          </Text>
        )}
        {visible.map((r, i) => (
          <Text key={`${r.valid_from}-${i}`} size="sm" ff="monospace" mb={2}>
            <Badge size="xs" mr={6} color={r.kind === 'message' ? 'teal' : 'indigo'}>
              {r.kind}
            </Badge>
            {r.rendered}
          </Text>
        ))}
      </ScrollArea>
    </Card>
  )
}
