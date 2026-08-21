import { useState } from 'react'
import { Button, Card, Code, Group, ScrollArea, Text, TextInput, Title } from '@mantine/core'
import { runCommand } from '../api'
import { useBreadcrumb } from '../Breadcrumbs'

interface Entry {
  argv: string
  output: string
  is_error: boolean
}

export default function ConsolePage() {
  useBreadcrumb([{ label: 'Console' }])
  const [input, setInput] = useState('')
  const [history, setHistory] = useState<Entry[]>([])
  const [busy, setBusy] = useState(false)

  async function run() {
    const argv = input.trim().split(/\s+/).filter(Boolean)
    if (argv.length === 0 || busy) return
    setBusy(true)
    try {
      const result = await runCommand(argv)
      setHistory((prev) => [{ argv: argv.join(' '), ...result }, ...prev])
      setInput('')
    } finally {
      setBusy(false)
    }
  }

  return (
    <Card withBorder>
      <Title order={5} mb="xs">
        Console — the CLI, from the browser
      </Title>
      <Group mb="md">
        <TextInput
          style={{ flex: 1 }}
          placeholder="agents · sessions · actions · read <fragment> · modules list"
          value={input}
          ff="monospace"
          onChange={(e) => setInput(e.currentTarget.value)}
          onKeyDown={(e) => e.key === 'Enter' && run()}
        />
        <Button onClick={run} loading={busy}>
          run
        </Button>
      </Group>
      <ScrollArea h={540}>
        {history.map((h, i) => (
          <div key={i} style={{ marginBottom: 12 }}>
            <Text size="sm" ff="monospace" c={h.is_error ? 'red.4' : 'teal.4'}>
              superx {h.argv}
            </Text>
            <Code block>{h.output}</Code>
          </div>
        ))}
        {history.length === 0 && (
          <Text c="dimmed" size="sm">
            command history is also persisted in the UI module's own database (superx/ui)
          </Text>
        )}
      </ScrollArea>
    </Card>
  )
}
