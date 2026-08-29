import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Anchor, AppShell, Box, Breadcrumbs, Group, NavLink, Tabs, Text, Title } from '@mantine/core'
import { fetchPing } from './api'
import MenuTree from './pages/Menu'
import EntityTab from './pages/Entity'
import GraphTab from './pages/Graph'

// The entities module's own dashboard, on its own port.
//
// THE SUPERX LAYOUT, AND THE MENU IS THE TREE (operator, 2026-08-28).
// An AppShell with a navbar down the left, exactly as the core
// dashboard has — and what lives in that navbar is the entity tree
// itself, because in this module the tree IS the menu. There is no
// separate "Menu" page to navigate to: you are always looking at the
// graph, and clicking a branch opens it beside you.
//
// The tree keeps its place while you work. It is mounted once and never
// unmounted, so expanding six levels and opening something does not
// collapse what you spent the time opening.

export default function App() {
  const [open, setOpen] = useState<string | null>(null)
  const [trail, setTrail] = useState<{ uuid: string; name: string }[]>([])
  const [view, setView] = useState<string | null>('Entity')
  const ping = useQuery({ queryKey: ['ping'], queryFn: fetchPing })
  const core = ping.data?.core_url ?? null

  return (
    <AppShell header={{ height: 52 }} navbar={{ width: 300, breakpoint: 'sm' }} padding="md">
      <AppShell.Header>
        <Group h="100%" px="md" gap="lg" wrap="nowrap">
          <Group gap={10} wrap="nowrap">
            <img src="/logo.svg" alt="" width={26} height={26} style={{ borderRadius: 6 }} />
            <Title order={3}>Entities</Title>
          </Group>
          {/* WHERE YOU ARE, in the header's dead middle — the same trail
            the core dashboard puts there. In a graph you can descend six
            levels into, the branch you are on is the one thing the pane
            itself never tells you. Each step walks back up. */}
          <Box style={{ flex: 1, minWidth: 0 }}>
            <Breadcrumbs
              separator="›"
              separatorMargin={8}
              styles={{
                root: { flexWrap: 'nowrap', overflow: 'hidden' },
                separator: { color: 'var(--mantine-color-dimmed)' },
              }}
            >
              {trail.map((c, i) =>
                i === trail.length - 1 ? (
                  <Text key={c.uuid} size="sm" fw={600} truncate style={{ maxWidth: 320 }}>
                    {c.name}
                  </Text>
                ) : (
                  <Anchor
                    key={c.uuid}
                    size="sm"
                    c="dimmed"
                    onClick={() => {
                      setOpen(c.uuid)
                      setTrail(trail.slice(0, i + 1))
                    }}
                  >
                    {c.name}
                  </Anchor>
                ),
              )}
            </Breadcrumbs>
          </Box>
          <Text size="sm" c="dimmed" visibleFrom="sm" style={{ whiteSpace: 'nowrap' }}>
            uuid7 · attributes · edges
          </Text>
        </Group>
      </AppShell.Header>

      <AppShell.Navbar p="xs" style={{ overflowY: 'auto' }}>
        <MenuTree
          onOpen={(uuid, path) => {
            setOpen(uuid)
            setTrail(path)
          }}
          opened={open}
        />
        {/* BACK TO SUPERX, where the core keeps its links out to module
            UIs. A module on its own port is a dead end without it: the
            operator arrives from the core dashboard, and the browser's
            back button is not a control they can see. The URL is
            resolved from the substrate (D-UI2), never hardcoded — and
            when there is no core UI there is no link, rather than a link
            to nowhere. */}
        {core && (
          <NavLink
            component="a"
            href={core}
            label="SuperX"
            mt="auto"
            rightSection={
              <svg
                width="14"
                height="14"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="2"
              >
                <path d="M7 17L17 7"></path>
                <path d="M8 7h9v9"></path>
              </svg>
            }
          />
        )}
      </AppShell.Navbar>

      <AppShell.Main>
        {open ? (
          <Tabs value={view} onChange={setView} keepMounted={false}>
            <Tabs.List mb="md">
              <Tabs.Tab value="Entity">Entity</Tabs.Tab>
              <Tabs.Tab value="Graph">Graph</Tabs.Tab>
            </Tabs.List>
            <Tabs.Panel value="Entity">
              <EntityTab
                frag={open}
                onOpen={(uuid) => {
                  // Followed from a link, not the tree, so there is no
                  // path to inherit — the trail restarts rather than
                  // claiming an ancestry this entity does not have.
                  setOpen(uuid)
                  setTrail([])
                }}
              />
            </Tabs.Panel>
            <Tabs.Panel value="Graph">
              <GraphTab
                frag={open}
                onOpen={(uuid) => {
                  setOpen(uuid)
                  setTrail([])
                }}
              />
            </Tabs.Panel>
          </Tabs>
        ) : (
          <Text c="dimmed" size="sm">
            Pick something from the tree, or add one.
          </Text>
        )}
      </AppShell.Main>
    </AppShell>
  )
}
