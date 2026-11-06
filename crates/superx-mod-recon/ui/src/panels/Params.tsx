import { useState } from 'react'
import { Alert, Badge, Button, Group, Paper, Stack, Table, Text, Textarea, TextInput, Title } from '@mantine/core'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { fetchParams, setParam, type Listing, type Param } from '../api'

// The parameters recon runs on, each settable here: the value is checked as
// recon checks what it reads and written as `superx param set recon` writes
// it (/api/params). The crawler takes a change at its next pass, the port at
// the next start.

const GROUPS: [string, string][] = [
  ['when', 'When'],
  ['where', 'Where'],
  ['how much', 'How much'],
  ['listens', 'Listens'],
  ['measures', 'Measures'],
  ['watches', 'Watches'],
]

/// The value typed, as the JSON recon is sent: a number, "all", a list of
/// lines, or an object of repository names to refs, one per line; `none`
/// for an empty list or object, so an empty box never sets one.
function typed(input: Param['input'], text: string): unknown {
  const t = text.trim()
  if (input === 'list') return t === 'none' ? [] : text.split('\n').map((l) => l.trim()).filter((l) => l !== '')
  if (input === 'refs') {
    if (t === 'none') return {}
    const refs: Record<string, string> = {}
    for (const line of text.split('\n')) {
      const [name, ref] = line.trim().split(/\s+/, 2)
      if (name && ref) refs[name] = ref
    }
    return refs
  }
  if (input === 'number or all' && t === 'all') return 'all'
  return /^\d+$/.test(t) ? Number(t) : t
}

const hint: Record<Param['input'], string> = {
  number: 'a whole number',
  'number or all': 'a whole number, or all',
  list: 'one per line, or none',
  refs: 'repository ref, a pair per line, or none',
}

function Row({ p }: { p: Param }) {
  const client = useQueryClient()
  const [text, setText] = useState('')
  const [saved, setSaved] = useState(false)
  const change = useMutation({
    mutationFn: () => setParam(p.name, typed(p.input, text)),
    onSuccess: (listing: Listing) => {
      client.setQueryData(['params'], listing)
      setText('')
      setSaved(true)
    },
  })
  const many = p.input === 'list' || p.input === 'refs'
  const field = many ? (
    <Textarea size="xs" autosize minRows={1} maxRows={6} value={text} placeholder={hint[p.input]}
      onChange={(e) => { setText(e.currentTarget.value); setSaved(false) }} />
  ) : (
    <TextInput size="xs" value={text} placeholder={hint[p.input]}
      onChange={(e) => { setText(e.currentTarget.value); setSaved(false) }}
      onKeyDown={(e) => { if (e.key === 'Enter' && text.trim() !== '') change.mutate() }} />
  )
  return (
    <Table.Tr>
      <Table.Td ff="monospace" fz="xs">{p.name}</Table.Td>
      <Table.Td>
        <Group gap={6}>
          <Text ff="monospace" fz="sm" style={{ overflowWrap: 'anywhere' }}>{p.value}</Text>
          <Badge size="xs" variant={p.set ? 'filled' : 'outline'} color={p.set ? 'pelican' : 'gray'}>{p.set ? 'set' : 'fallback'}</Badge>
        </Group>
      </Table.Td>
      <Table.Td>
        <Text fz="sm">{p.decides}</Text>
        <Text fz="xs" c="dimmed">{p.wanted}</Text>
      </Table.Td>
      <Table.Td miw={260}>
        <Group gap={6} align="flex-start" wrap="nowrap">
          <div style={{ flex: 1 }}>{field}</div>
          <Button size="xs" disabled={text.trim() === ''} loading={change.isPending} onClick={() => change.mutate()}>
            Set
          </Button>
        </Group>
        {change.isError && <Text fz="xs" c="red.4">{String(change.error.message)}</Text>}
        {saved && !change.isError && <Text fz="xs" c="teal.4">Set; the next pass uses it.</Text>}
      </Table.Td>
    </Table.Tr>
  )
}

export function Params() {
  const listing = useQuery({ queryKey: ['params'], queryFn: fetchParams })
  const params = listing.data?.params ?? []
  return (
    <Stack gap="sm">
      <Group justify="space-between" align="baseline">
        <Title order={3}>Parameters</Title>
        <Text size="xs" c="dimmed">each as superx param set recon writes it · the crawler takes a change at its next pass</Text>
      </Group>
      {listing.isError && <Alert color="red" title="The parameters did not load">{String(listing.error)}</Alert>}
      {(listing.data?.problems ?? []).map((p) => (
        <Alert key={p.name} color="yellow" title={`${p.name} is set to ${p.found}`}>
          It wants {p.wanted}; the fallback stands in until it is set again.
        </Alert>
      ))}
      {params.length > 0 && (
        <Paper withBorder radius="md">
          <Table.ScrollContainer minWidth={900}>
            <Table fz="sm" verticalSpacing="xs">
              <Table.Thead>
                <Table.Tr>
                  <Table.Th>Parameter</Table.Th>
                  <Table.Th>Now</Table.Th>
                  <Table.Th>What it decides</Table.Th>
                  <Table.Th>Change it</Table.Th>
                </Table.Tr>
              </Table.Thead>
              <Table.Tbody>
                {GROUPS.map(([group, label]) => {
                  const rows = params.filter((p) => p.group === group)
                  if (rows.length === 0) return null
                  return [
                    <Table.Tr key={group}>
                      <Table.Td colSpan={4} bg="dark.6"><Text fw={600} fz="sm">{label}</Text></Table.Td>
                    </Table.Tr>,
                    ...rows.map((p) => <Row key={p.name} p={p} />),
                  ]
                })}
              </Table.Tbody>
            </Table>
          </Table.ScrollContainer>
        </Paper>
      )}
    </Stack>
  )
}
