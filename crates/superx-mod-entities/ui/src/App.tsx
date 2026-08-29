import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { AppShell, Box, Button, Container, Group, Tabs, Title } from '@mantine/core'
import { fetchPing } from './api'
import MenuTab from './pages/Menu'
import EntityTab from './pages/Entity'
import GraphTab from './pages/Graph'

// The entities module's own dashboard, on its own port.
//
// THREE TABS. MENU, ENTITY, GRAPH (operator, 2026-08-28, verbatim: "the
// menu is one tab where I can add entities, then entity creation design
// its own tab, and display graph is another own tab, this is it for
// now"). The menu is a TAB — not a sidebar, not a navbar. Clicking an
// entity in it opens the Entity tab; the Graph tab draws the same
// entity's shape. The graph DESIGNER is deliberately absent; it is the
// next PR.
//
// The menu keeps its place while you work in the other two: it is
// mounted once and never unmounted, so expanding six levels and opening
// something does not collapse what it took six clicks to open.

export default function App() {
  const [open, setOpen] = useState<string | null>(null)
  const [trail, setTrail] = useState<{ uuid: string; name: string }[]>([])
  const [tab, setTab] = useState<string | null>('menu')
  const ping = useQuery({ queryKey: ['ping'], queryFn: fetchPing })
  const core = ping.data?.core_url ?? null

  const openEntity = (uuid: string, path: { uuid: string; name: string }[]) => {
    setOpen(uuid)
    setTrail(path)
    setTab('entity')
  }

  return (
    <AppShell header={{ height: 52 }} padding="md">
      <AppShell.Header>
        <Group h="100%" px="md" gap="lg" wrap="nowrap">
          <Group gap={10} wrap="nowrap">
            <img src="/logo.svg" alt="" width={26} height={26} style={{ borderRadius: 6 }} />
            <Title order={3}>Entities</Title>
          </Group>
          <Box style={{ flex: 1, minWidth: 0 }} />
          {/* BACK TO SUPERX. This module runs on its own port, so
              without it the operator is stranded: the dashboard they
              came from is a different origin and the browser's back
              button is not a control they can see. The URL is resolved
              from the substrate (D-UI2), never hardcoded — and when
              there is no core UI there is no button, rather than a
              button that goes nowhere. */}
          {core && (
            <Button component="a" href={core} variant="subtle" size="compact-sm" leftSection="←">
              SuperX
            </Button>
          )}
        </Group>
      </AppShell.Header>

      <AppShell.Main>
        {/* A COLUMN, NOT THE WHOLE MONITOR. Form rows and short labels
            stretched edge to edge on a wide screen are unreadable, and a
            menu row highlighting across two thousand pixels looks
            broken rather than selected. */}
        <Container size="xl" px={0}>
        <Tabs value={tab} onChange={setTab} keepMounted={false}>
          <Tabs.List mb="md">
            <Tabs.Tab value="menu">Menu</Tabs.Tab>
            <Tabs.Tab value="entity" disabled={!open}>
              Entity
            </Tabs.Tab>
            <Tabs.Tab value="graph" disabled={!open}>
              Graph
            </Tabs.Tab>
          </Tabs.List>

          {/* keepMounted is off for Entity and Graph — a graph nobody
              asked for should not fetch or draw. The MENU is the
              exception: it is rendered outside the panels precisely so
              switching tabs cannot unmount it and throw away every
              branch you expanded. */}
          <Box display={tab === 'menu' ? undefined : 'none'}>
            <MenuTab
              onOpen={openEntity}
              opened={open}
              trail={trail}
              onCrumb={(uuid, upto) => {
                setOpen(uuid)
                setTrail(trail.slice(0, upto + 1))
                setTab('entity')
              }}
            />
          </Box>
          <Tabs.Panel value="entity">
            {open && (
              <EntityTab
                frag={open}
                onOpen={(uuid) => {
                  // Followed from a link, not the menu, so there is no
                  // path to inherit — the trail restarts rather than
                  // claiming an ancestry this entity does not have.
                  setOpen(uuid)
                  setTrail([])
                }}
              />
            )}
          </Tabs.Panel>
          <Tabs.Panel value="graph">
            {open && (
              <GraphTab
                frag={open}
                onOpen={(uuid) => {
                  setOpen(uuid)
                  setTrail([])
                }}
              />
            )}
          </Tabs.Panel>
        </Tabs>
        </Container>
      </AppShell.Main>
    </AppShell>
  )
}
