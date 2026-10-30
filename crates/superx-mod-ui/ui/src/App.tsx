import { useEffect, useState } from 'react'
import { AppShell, Box, NavLink, Title, Text, Group } from '@mantine/core'
import { PAGES, goToPage, pageFromHash, type Page } from './route'
import { BreadcrumbProvider, BreadcrumbTrail } from './Breadcrumbs'
import StatusPage from './pages/status'
import ActivityPage from './pages/Activity'
import SessionsPage from './pages/Sessions'
import ConsolePage from './pages/Console'

export default function App() {
  return (
    <BreadcrumbProvider>
      <Shell />
    </BreadcrumbProvider>
  )
}

function Shell() {
  // The page is the URL hash's (#378): a reload lands where you were,
  // Back and Forward walk the pages, and a session on the cockpit can
  // be a link into the Sessions page.
  const [page, setPage] = useState<Page>(pageFromHash)
  useEffect(() => {
    const sync = () => setPage(pageFromHash())
    window.addEventListener('hashchange', sync)
    return () => window.removeEventListener('hashchange', sync)
  }, [])
  return (
    <AppShell header={{ height: 52 }} navbar={{ width: 180, breakpoint: 'xs' }} padding="md">
      <AppShell.Header>
        <Group h="100%" px="md" gap="lg" wrap="nowrap">
          <Group gap={10} wrap="nowrap">
            <img src="/logo.svg" alt="" width={26} height={26} style={{ borderRadius: 6 }} />
            <Title order={3}>SuperX</Title>
          </Group>
          {/* The header's dead middle now says where you are (#253). */}
          <Box style={{ flex: 1, minWidth: 0 }}>
            <BreadcrumbTrail onHome={() => goToPage('Status')} />
          </Box>
          <Text size="sm" c="dimmed" visibleFrom="sm" style={{ whiteSpace: 'nowrap' }}>
            the agentic OS
          </Text>
        </Group>
      </AppShell.Header>
      <AppShell.Navbar p="xs">
        {PAGES.map((p) => (
          <NavLink key={p} label={p} active={page === p} onClick={() => goToPage(p)} />
        ))}
      </AppShell.Navbar>
      <AppShell.Main>
        {page === 'Status' && <StatusPage />}
        {page === 'Activity' && <ActivityPage />}
        {page === 'Sessions' && <SessionsPage />}
        {page === 'Console' && <ConsolePage />}
      </AppShell.Main>
    </AppShell>
  )
}
