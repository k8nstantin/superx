import { useEffect, useState } from 'react'
import { Container, Stack, Tabs, Text, Title } from '@mantine/core'
import { Now } from './panels/Now'
import { Repos } from './panels/Repos'
import { Stats } from './panels/Stats'
import { Params } from './panels/Params'
import { Results } from './results/Results'

// The tab, in two: the Crawler (what it does, has and runs on) and the
// Results (what it gathers them for, a view per question under results/).
// The open one is kept in the address: #crawler, or #results/<view>.
const VIEWS = ['crawler', 'results']

function opened(): [string, string] {
  const [view, sub] = window.location.hash.slice(1).split('/')
  return [VIEWS.includes(view) ? view : 'crawler', sub ?? '']
}

export default function App() {
  const [[view, sub], setView] = useState(opened)
  useEffect(() => {
    const follow = () => setView(opened())
    window.addEventListener('hashchange', follow)
    return () => window.removeEventListener('hashchange', follow)
  }, [])
  const go = (v: string, s = '') => {
    window.location.hash = s ? `${v}/${s}` : v
    setView([v, s])
  }
  return (
    <Container fluid px="lg" py="lg">
      <Stack gap="lg">
        <Stack gap={2}>
          <Title order={2}>Recon</Title>
          <Text c="dimmed" size="sm">
            The crawler, and what it gathers: the models, the agents and the repositories they worked in, measured over time.
          </Text>
        </Stack>
        <Tabs
          value={view}
          onChange={(v) => v && go(v, v === 'results' ? sub : '')}
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
            <Results sub={sub} go={(s) => go('results', s)} />
          </Tabs.Panel>
        </Tabs>
      </Stack>
    </Container>
  )
}
