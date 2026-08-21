import { useEffect, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Anchor, AppShell, Box, Group, NavLink, Text } from '@mantine/core'
import { fetchPing } from './api'
import EntitiesPage from './pages/Entities'
import TypesPage from './pages/Types'
import GraphFull, { graphRouteId } from './pages/GraphFull'
import { BreadcrumbProvider, BreadcrumbTrail } from './Breadcrumbs'

// The entities module's OWN dashboard (epic #216, approved design):
// logo + wordmark, back link to the core dashboard (discovered from
// the substrate), Entities and Types — the graph is per entity, so
// there is no Graph menu.

const PAGES = ['Entities', 'Types'] as const
type Page = (typeof PAGES)[number]

/// One route, so one `pathname` check rather than a router dependency
/// (issue #250): `/graph/<id>` is the graph in its own window, and the
/// module's static handler already serves index.html for it.
function useRoute(): string {
  const [path, setPath] = useState(() => window.location.pathname)
  useEffect(() => {
    const onNav = () => setPath(window.location.pathname)
    window.addEventListener('popstate', onNav)
    return () => window.removeEventListener('popstate', onNav)
  }, [])
  return path
}

export default function App() {
  return (
    <BreadcrumbProvider>
      <Shell />
    </BreadcrumbProvider>
  )
}

function Shell() {
  const [page, setPage] = useState<Page>('Entities')
  const ping = useQuery({ queryKey: ['ping'], queryFn: fetchPing })
  const graphId = graphRouteId(useRoute())
  if (graphId) return <GraphFull frag={graphId} />
  return (
    <AppShell header={{ height: 52 }} navbar={{ width: 180, breakpoint: 'xs' }} padding="md">
      <AppShell.Header>
        <Group h="100%" px="md" gap="lg" wrap="nowrap">
          <Group gap={10} wrap="nowrap">
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
            <Text c="dimmed" fz={15} style={{ whiteSpace: 'nowrap' }}>
              · entities{ping.data ? ` · v${ping.data.version}` : ''}
            </Text>
          </Group>
          {/* Where you are in the graph, not just which page (#253). */}
          <Box style={{ flex: 1, minWidth: 0 }}>
            <BreadcrumbTrail onHome={() => setPage('Entities')} />
          </Box>
          {ping.data?.core_url && (
            <Anchor href={ping.data.core_url} size="sm" style={{ whiteSpace: 'nowrap' }}>
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
