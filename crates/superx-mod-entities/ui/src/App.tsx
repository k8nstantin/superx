import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Anchor, AppShell, Group, NavLink, Text } from '@mantine/core'
import { fetchPing } from './api'
import EntitiesPage from './pages/Entities'
import TypesPage from './pages/Types'

// The entities module's OWN dashboard (epic #216, approved design):
// logo + wordmark, back link to the core dashboard (discovered from
// the substrate), Entities and Types — the graph is per entity, so
// there is no Graph menu.

const PAGES = ['Entities', 'Types'] as const
type Page = (typeof PAGES)[number]

export default function App() {
  const [page, setPage] = useState<Page>('Entities')
  const ping = useQuery({ queryKey: ['ping'], queryFn: fetchPing })
  return (
    <AppShell header={{ height: 52 }} navbar={{ width: 180, breakpoint: 'xs' }} padding="md">
      <AppShell.Header>
        <Group h="100%" px="md" justify="space-between">
          <Group gap={10}>
            <img src="/logo.svg" alt="" width={26} height={26} style={{ borderRadius: 6 }} />
            <Text
              component="span"
              fw={700}
              fz={18}
              c="#EDE4F4"
              style={{ fontFamily: "'Space Grotesk', system-ui, sans-serif" }}
            >
              superx
            </Text>
            <Text c="dimmed" fz={15}>
              · entities{ping.data ? ` · v${ping.data.version}` : ''}
            </Text>
          </Group>
          {ping.data?.core_url && (
            <Anchor href={ping.data.core_url} size="sm">
              ← core dashboard
            </Anchor>
          )}
        </Group>
      </AppShell.Header>
      <AppShell.Navbar p="xs">
        {PAGES.map((p) => (
          <NavLink key={p} label={p} active={page === p} onClick={() => setPage(p)} />
        ))}
      </AppShell.Navbar>
      <AppShell.Main>
        {page === 'Entities' && <EntitiesPage />}
        {page === 'Types' && <TypesPage />}
      </AppShell.Main>
    </AppShell>
  )
}
