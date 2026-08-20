import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { AppShell, Card, Group, NavLink, Text, Title } from '@mantine/core'

// The entities module's OWN dashboard (epic #216) — the per-module-UI
// facility. EU1 ships the shell; EU2 lands browse+read, EU3 the write
// path (BlockNote), EU4 links+files, EU5 the ECharts graph.

const PAGES = ['Entities', 'Graph', 'Types'] as const
type Page = (typeof PAGES)[number]

async function fetchPing(): Promise<{ module: string; version: string }> {
  const r = await fetch('/api/ping')
  if (!r.ok) throw new Error(`ping: ${r.status}`)
  return r.json()
}

export default function App() {
  const [page, setPage] = useState<Page>('Entities')
  const ping = useQuery({ queryKey: ['ping'], queryFn: fetchPing })
  return (
    <AppShell header={{ height: 52 }} navbar={{ width: 180, breakpoint: 'xs' }} padding="md">
      <AppShell.Header>
        <Group h="100%" px="md" justify="space-between">
          <Title order={3}>⚙ SuperX · Entities</Title>
          <Text size="sm" c="dimmed">
            the product graph{ping.data ? ` · v${ping.data.version}` : ''}
          </Text>
        </Group>
      </AppShell.Header>
      <AppShell.Navbar p="xs">
        {PAGES.map((p) => (
          <NavLink key={p} label={p} active={page === p} onClick={() => setPage(p)} />
        ))}
      </AppShell.Navbar>
      <AppShell.Main>
        <Card withBorder>
          <Title order={5} mb="xs">
            {page}
          </Title>
          <Text size="sm" c="dimmed">
            {page === 'Entities' &&
              'The entity list and detail views land here next (EU2): browse the graph by type, open an entity for its descriptions, comments, documents, edges, and full version history.'}
            {page === 'Graph' &&
              'The interactive graph lands here (EU5): the ECharts force layout — nodes shaped and colored by type, dashed depends_on edges, click-through to detail.'}
            {page === 'Types' &&
              'The type registry lands here (EU3): the seeded node and relation kinds, runtime-extensible.'}
          </Text>
        </Card>
      </AppShell.Main>
    </AppShell>
  )
}
