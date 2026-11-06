import { useEffect, useState } from 'react'
import { AppShell, Container, Stack, Tabs, Text, Title } from '@mantine/core'
import { useQuery } from '@tanstack/react-query'
import { fetchPing } from './api'
import { Header, type Crumb } from './Header'
import { Now } from './panels/Now'
import { Repos } from './panels/Repos'
import { Stats } from './panels/Stats'
import { Params } from './panels/Params'
import { RESULT_VIEWS, Results } from './results/Results'

// The tab, in two: the Crawler (what it does, has and runs on) and the
// Results (what it gathers them for, a view per question under results/).
// The open one is kept in the address: #crawler, or #results/<view>. Above
// it, the header's trail (#562): SuperX, the module, the tab and the view.
const VIEWS = ['crawler', 'results']

function opened(): [string, string] {
  const [view, sub] = window.location.hash.slice(1).split('/')
  return [VIEWS.includes(view) ? view : 'crawler', sub ?? '']
}

/// The trail to where the page is: the core dashboard (a link when the
/// registry knows where it is), the module, the tab and, in the Results,
/// the view.
function trail(core: string | null | undefined, view: string, sub: string): Crumb[] {
  const crumbs: Crumb[] = [
    { label: 'SuperX', href: core ?? undefined },
    { label: 'Recon', href: '#crawler' },
  ]
  if (view === 'results') {
    const [, label] = RESULT_VIEWS.find(([k]) => k === sub) ?? RESULT_VIEWS[0]
    crumbs.push({ label: 'Results', href: '#results' }, { label })
  } else {
    crumbs.push({ label: 'Crawler' })
  }
  return crumbs
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
  const ping = useQuery({ queryKey: ['ping'], queryFn: fetchPing, staleTime: Infinity })
  const crumbs = trail(ping.data?.core_url, view, sub)
  const here = crumbs[crumbs.length - 1].label
  // The browser's tab says where too, the view first.
  useEffect(() => {
    document.title = `${here} · Recon · SuperX`
  }, [here])
  return (
    <AppShell header={{ height: 52 }}>
      <AppShell.Header>
        <Header crumbs={crumbs} />
      </AppShell.Header>
      <AppShell.Main>
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
      </AppShell.Main>
    </AppShell>
  )
}
