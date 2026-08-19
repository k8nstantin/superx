import { useState } from 'react'
import { AppShell, NavLink, Title, Text, Group } from '@mantine/core'
import StatusPage from './pages/Status'
import ActivityPage from './pages/Activity'
import SessionsPage from './pages/Sessions'
import ConsolePage from './pages/Console'

const PAGES = ['Status', 'Activity', 'Sessions', 'Console'] as const
type Page = (typeof PAGES)[number]

export default function App() {
  const [page, setPage] = useState<Page>('Status')
  return (
    <AppShell header={{ height: 52 }} navbar={{ width: 180, breakpoint: 'xs' }} padding="md">
      <AppShell.Header>
        <Group h="100%" px="md" justify="space-between">
          <Group gap={10}>
            <img src="/logo.svg" alt="" width={26} height={26} style={{ borderRadius: 6 }} />
            <Title order={3}>SuperX</Title>
          </Group>
          <Text size="sm" c="dimmed">
            the agentic OS
          </Text>
        </Group>
      </AppShell.Header>
      <AppShell.Navbar p="xs">
        {PAGES.map((p) => (
          <NavLink key={p} label={p} active={page === p} onClick={() => setPage(p)} />
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
