import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { AppShell, NavLink, Title, Text, Group } from '@mantine/core'
import { fetchStatus } from './api'
import StatusPage from './pages/Status'
import ActivityPage from './pages/Activity'
import SessionsPage from './pages/Sessions'
import ConsolePage from './pages/Console'

const PAGES = ['Status', 'Activity', 'Sessions', 'Console'] as const
type Page = (typeof PAGES)[number]

export default function App() {
  const [page, setPage] = useState<Page>('Status')
  // Module UIs are discovered from the substrate (epic #216, D-UI2):
  // any module publishing attr_module_ui_url gets a nav button here,
  // with zero per-module code.
  const status = useQuery({ queryKey: ['status'], queryFn: fetchStatus, refetchInterval: 30000 })
  const moduleUis = (status.data?.modules ?? []).filter((m) => m.ui_url != null)
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
        {moduleUis.length > 0 && (
          <>
            <Text size="xs" fw={700} tt="uppercase" c="dimmed" mt="md" px="sm" style={{ letterSpacing: 0.5 }}>
              Module UIs
            </Text>
            {moduleUis.map((m) => (
              <NavLink
                key={m.module_id}
                component="a"
                href={m.ui_url!}
                target="_blank"
                rel="noopener"
                label={m.name}
                rightSection={
                  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                    <path d="M7 17L17 7"></path>
                    <path d="M8 7h9v9"></path>
                  </svg>
                }
              />
            ))}
          </>
        )}
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
