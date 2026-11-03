import { useEffect, useState } from 'react'
import { Container, Stack, Tabs, Text, Title } from '@mantine/core'
import { Now } from './panels/Now'
import { Repos } from './panels/Repos'
import { Stats } from './panels/Stats'
import { Params } from './panels/Params'
import { Models } from './panels/Models'
import { Comparison } from './panels/Comparison'

// The tab, in two: the Crawler (what it does, has and runs on) and the
// Results (what it gathers them for). One panel per file under panels/, each
// reading its own route of recon's URL, so a panel is added without touching
// the others. The open one is kept in the address, #crawler or #results.
const VIEWS = ['crawler', 'results']

function opened(): string {
  const view = window.location.hash.slice(1)
  return VIEWS.includes(view) ? view : 'crawler'
}

export default function App() {
  const [view, setView] = useState(opened)
  useEffect(() => {
    const follow = () => setView(opened())
    window.addEventListener('hashchange', follow)
    return () => window.removeEventListener('hashchange', follow)
  }, [])
  return (
    <Container size="xl" py="lg">
      <Stack gap="lg">
        <Stack gap={2}>
          <Title order={2}>Recon</Title>
          <Text c="dimmed" size="sm">
            The crawler, and what it gathers: the models, the agents and the repositories they worked in, measured over time.
          </Text>
        </Stack>
        <Tabs
          value={view}
          onChange={(v) => {
            if (v) {
              window.location.hash = v
              setView(v)
            }
          }}
          keepMounted={false}
        >
          <Tabs.List>
            <Tabs.Tab value="crawler">Crawler</Tabs.Tab>
            <Tabs.Tab value="results">Results</Tabs.Tab>
          </Tabs.List>
          <Tabs.Panel value="crawler" pt="lg">
            <Stack gap="xl">
              <Now />
              <Repos />
              <Stats />
              <Params />
            </Stack>
          </Tabs.Panel>
          <Tabs.Panel value="results" pt="lg">
            <Stack gap="xl">
              <Comparison />
              <Models />
            </Stack>
          </Tabs.Panel>
        </Tabs>
      </Stack>
    </Container>
  )
}
