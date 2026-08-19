import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Badge, Button, Card, Group, ScrollArea, Switch, Table, Text, Title } from '@mantine/core'
import { fetchMessages, fetchSessions } from '../api'
import { useSse } from '../useSse'
import type { MessageView } from '../generated/MessageView'

export default function SessionsPage() {
  const [selected, setSelected] = useState<string | null>(null)
  return selected ? (
    <ConversationView id={selected} onBack={() => setSelected(null)} />
  ) : (
    <SessionList onOpen={setSelected} />
  )
}

function SessionList({ onOpen }: { onOpen: (id: string) => void }) {
  const sessions = useQuery({ queryKey: ['sessions'], queryFn: () => fetchSessions(), refetchInterval: 10000 })
  const rows = (sessions.data ?? []).sort((a, b) => Number(b.messages) - Number(a.messages))
  return (
    <Card withBorder>
      <Title order={5} mb="xs">
        Captured conversations
      </Title>
      <ScrollArea h={600}>
        <Table striped highlightOnHover>
          <Table.Thead>
            <Table.Tr>
              <Table.Th>Agent</Table.Th>
              <Table.Th>Session</Table.Th>
              <Table.Th>Source id</Table.Th>
              <Table.Th>Messages</Table.Th>
              <Table.Th />
            </Table.Tr>
          </Table.Thead>
          <Table.Tbody>
            {rows.map((s) => (
              <Table.Tr key={s.session_id}>
                <Table.Td>
                  <Badge variant="light">{s.agent}</Badge>
                </Table.Td>
                <Table.Td>
                  <Text size="xs" ff="monospace">
                    {s.session_id}
                  </Text>
                </Table.Td>
                <Table.Td>
                  <Text size="xs" ff="monospace" c="dimmed">
                    {s.src}
                  </Text>
                </Table.Td>
                <Table.Td>{String(s.messages)}</Table.Td>
                <Table.Td>
                  <Button size="compact-xs" variant="light" onClick={() => onOpen(s.session_id)}>
                    read
                  </Button>
                </Table.Td>
              </Table.Tr>
            ))}
          </Table.Tbody>
        </Table>
      </ScrollArea>
    </Card>
  )
}

function ConversationView({ id, onBack }: { id: string; onBack: () => void }) {
  const [live, setLive] = useState(true)
  const [liveRows, setLiveRows] = useState<string[]>([])
  const backlog = useQuery({ queryKey: ['messages', id], queryFn: () => fetchMessages(id) })

  useSse((e) => {
    if (e.kind === 'message' && e.session_id === id) {
      setLiveRows((prev) => [...prev, e.rendered])
    }
  }, !live)

  const roleColor = (m: MessageView) =>
    m.role === 'user' ? 'teal' : m.role === 'assistant' ? 'indigo' : 'gray'

  return (
    <Card withBorder>
      <Group justify="space-between" mb="xs">
        <Group>
          <Button size="compact-xs" variant="default" onClick={onBack}>
            ← sessions
          </Button>
          <Title order={5} ff="monospace">
            {id}
          </Title>
        </Group>
        <Switch label="live" checked={live} onChange={(e) => setLive(e.currentTarget.checked)} />
      </Group>
      <ScrollArea h={580}>
        {(backlog.data ?? []).map((m, i) => (
          <Text key={i} size="sm" ff="monospace" mb={2} c={roleColor(m) === 'gray' ? 'dimmed' : undefined}>
            {m.rendered}
          </Text>
        ))}
        {liveRows.map((r, i) => (
          <Text key={`live-${i}`} size="sm" ff="monospace" mb={2} c="yellow.3">
            {r}
          </Text>
        ))}
      </ScrollArea>
    </Card>
  )
}
