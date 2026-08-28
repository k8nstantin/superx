import { useState } from 'react'
import { AppShell, Group, Tabs, Text, Title } from '@mantine/core'
import MenuTab from './pages/Menu'
import EntityTab from './pages/Entity'
import GraphTab from './pages/Graph'

// The entities module's own dashboard, on its own port.
//
// THREE TABS (operator, 2026-08-28): the menu is where entities are
// added and traversed, the entity is where one is designed, and the
// graph is where its shape is seen. The graph DESIGNER is deliberately
// not here — it is the next PR.
//
// Opening an entity does not navigate away from the menu: the tab
// switches and the menu keeps its place, because traversing a deep graph
// and losing where you were is the thing that makes it unusable.

export default function App() {
  const [open, setOpen] = useState<string | null>(null)
  const [tab, setTab] = useState<string | null>('menu')

  const openEntity = (uuid: string) => {
    setOpen(uuid)
    setTab('entity')
  }

  return (
    <AppShell header={{ height: 52 }} padding="md">
      <AppShell.Header>
        <Group h="100%" px="md" gap="sm" wrap="nowrap">
          <Title order={4}>entities</Title>
          <Text size="xs" c="dimmed" ff="monospace">
            uuid7 · attributes · edges
          </Text>
        </Group>
      </AppShell.Header>
      <AppShell.Main>
        {/* keepMounted, deliberately. The header above promises the menu
            keeps its place; MenuTab holds that place in local state, so
            unmounting it collapsed every expanded branch and refetched
            each level from scratch — exactly inverting the property. */}
        <Tabs value={tab} onChange={setTab}>
          <Tabs.List mb="md">
            <Tabs.Tab value="menu">Menu</Tabs.Tab>
            <Tabs.Tab value="entity" disabled={!open}>
              Entity
            </Tabs.Tab>
            <Tabs.Tab value="graph" disabled={!open}>
              Graph
            </Tabs.Tab>
          </Tabs.List>

          <Tabs.Panel value="menu">
            <MenuTab onOpen={openEntity} />
          </Tabs.Panel>
          <Tabs.Panel value="entity">
            {open && <EntityTab frag={open} onOpen={openEntity} />}
          </Tabs.Panel>
          <Tabs.Panel value="graph">
            {open && <GraphTab frag={open} onOpen={openEntity} />}
          </Tabs.Panel>
        </Tabs>
      </AppShell.Main>
    </AppShell>
  )
}
