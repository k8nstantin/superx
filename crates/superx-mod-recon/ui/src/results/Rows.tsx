import { useState, type ReactNode } from 'react'
import { Alert, Anchor, Badge, Drawer, Group, Loader, ScrollArea, Stack, Table, Text } from '@mantine/core'
import { useQuery } from '@tanstack/react-query'
import { fetchCommit, fetchWindow, type FoundCommit, type FoundPull, type WinPrompt } from '../api'
import { local, n, weekLabel, type Palette } from './shared'

// The rows behind a chart: commits (each opening to the whole commit),
// pull requests, and the prompts of a week. A drawer shows them when a bar,
// a point or a row is clicked.

export function ModelBadge({ model, pal }: { model: string; pal: Palette }) {
  if (!model) return <Text fz="xs" c="dimmed">not credited</Text>
  return (
    <Group gap={6} wrap="nowrap">
      <span style={{ width: 9, height: 9, borderRadius: 9, background: pal.colour(model), flex: 'none' }} />
      <Text fz="xs" style={{ whiteSpace: 'nowrap' }}>{model}</Text>
    </Group>
  )
}

export function Place({ c }: { c: { place: string | null; in_flight: boolean; landed: number; added: number; landed_as?: string | null } }) {
  if (c.place === 'mainline') return <Badge size="xs" color="teal" variant="light">main line</Badge>
  if (c.landed_as) return <Badge size="xs" color="teal" variant="light">landed as {c.landed_as.slice(0, 8)}</Badge>
  if (c.landed > 0 && c.landed >= c.added) return <Badge size="xs" color="teal" variant="light">landed by a squash</Badge>
  if (c.landed > 0) return <Badge size="xs" color="teal" variant="outline">part landed</Badge>
  if (c.in_flight) return <Badge size="xs" color="yellow" variant="light">in flight</Badge>
  if (c.place === 'recovered') return <Badge size="xs" color="red" variant="light">reflog only</Badge>
  if (c.place === 'branch') return <Badge size="xs" color="red" variant="outline">never landed</Badge>
  return <Badge size="xs" color="gray" variant="light">not placed yet</Badge>
}

const verdict = (v: boolean | null | undefined) =>
  v == null ? <Text fz="xs" c="dimmed">—</Text> : <Badge size="xs" variant="light" color={v ? 'teal' : 'red'}>{v ? 'passed' : 'failed'}</Badge>

/// One commit, whole: who made it, where it went, its files, its CI, the
/// pull requests that carried it and the tests its session ran first.
export function Whole({ repo, hash }: { repo: string; hash: string }) {
  const q = useQuery({ queryKey: ['commit', repo, hash], queryFn: () => fetchCommit(repo, hash) })
  if (q.isPending) return <Loader size="xs" />
  if (q.isError) return <Alert color="red" title="The commit did not load">{String(q.error)}</Alert>
  const c = q.data
  return (
    <Stack gap={6}>
      <Text fz="xs" c="dimmed">
        {c.author_name} · {c.by ? `${c.by === 'printed' ? 'printed by' : 'claimed for'} ${c.model}` : 'no model credited'}
        {c.agent ? ` · ${c.agent}` : ''}{c.session ? ` · session ${c.session}` : ''}
        {c.effort ? ` · effort ${c.effort}` : ''}{c.context ? ` · ${n(c.context)} tokens of context` : ''}
      </Text>
      <Text fz="xs">
        {c.landed_as ? `rewritten onto the main line as ${c.landed_as.slice(0, 10)} · ` : ''}+{n(c.added)} −{n(c.removed)} · landed {n(c.landed)} · alive {n(c.alive)} · {c.in_flight ? `in flight ${n(c.added - c.landed)}` : `never landed ${n(c.never_landed)}`}
        {c.reverted ? ' · reverted later' : ''}
      </Text>
      {c.checks.length > 0 && (
        <Text fz="xs">
          Before it, its session ran: {c.checks.map((k) => (k.test ? `${k.what} tests ${k.passed} passed ${k.failed} failed` : `${k.what} build ${k.ok ? 'ok' : 'failed'}`)).join(' · ')}
        </Text>
      )}
      {c.pulls.map((p) => (
        <Text key={p.number} fz="xs">
          <Anchor href={p.url} target="_blank" fz="xs">#{p.number}</Anchor> {p.title} · {p.merged ? `merged ${local(p.merged_at)}` : p.state}
        </Text>
      ))}
      {c.ci.length > 0 && (
        <Group gap={6}>
          {c.ci.map((r, i) => (
            <Badge key={i} size="xs" variant="outline" color={r.verdict == null ? 'gray' : r.verdict ? 'teal' : 'red'}>
              {r.kind.replace('_', ' ')} · {r.name || '—'}{r.failed_before ? ' · red first' : ''}
            </Badge>
          ))}
        </Group>
      )}
      <Table fz="xs" verticalSpacing={2}>
        <Table.Tbody>
          {c.files.map((f) => (
            <Table.Tr key={f.path}>
              <Table.Td ff="monospace" style={{ overflowWrap: 'anywhere' }}>{f.path}</Table.Td>
              <Table.Td c="dimmed">{f.change}</Table.Td>
              <Table.Td ta="right" c="teal.4">+{n(f.added)}</Table.Td>
              <Table.Td ta="right" c="red.4">−{n(f.removed)}</Table.Td>
            </Table.Tr>
          ))}
        </Table.Tbody>
      </Table>
    </Stack>
  )
}

function CommitRow({ c, pal }: { c: FoundCommit; pal: Palette }) {
  const [open, setOpen] = useState(false)
  return (
    <>
      <Table.Tr style={{ cursor: 'pointer' }} onClick={() => setOpen(!open)}>
        <Table.Td ff="monospace" fz="xs" style={{ whiteSpace: 'nowrap' }}>{open ? '▾' : '▸'} {local(c.at)}</Table.Td>
        <Table.Td fz="xs">{c.repo}</Table.Td>
        <Table.Td><Text fz="xs" lineClamp={2}>{c.subject}</Text></Table.Td>
        <Table.Td><ModelBadge model={c.model} pal={pal} /></Table.Td>
        <Table.Td ta="right" fz="xs" style={{ whiteSpace: 'nowrap' }}><Text span c="teal.4" fz="xs">+{n(c.added)}</Text> <Text span c="red.4" fz="xs">−{n(c.removed)}</Text></Table.Td>
        <Table.Td><Place c={c} /></Table.Td>
        <Table.Td ta="right" fz="xs">{n(c.landed)}</Table.Td>
        <Table.Td ta="right" fz="xs">{n(c.alive)}</Table.Td>
        <Table.Td fz="xs">{c.pulls.length ? `#${c.pulls.join(' #')}` : '—'}</Table.Td>
        <Table.Td>{verdict(c.ci)}</Table.Td>
        <Table.Td fz="xs">{c.tested == null ? '—' : c.tested ? (c.tests_green ? 'green' : 'red') : 'no'}</Table.Td>
        <Table.Td fz="xs">{c.effort ?? '—'}</Table.Td>
      </Table.Tr>
      {open && (
        <Table.Tr>
          <Table.Td colSpan={12} bg="dark.7"><Whole repo={c.repo} hash={c.hash} /></Table.Td>
        </Table.Tr>
      )}
    </>
  )
}

export function CommitRows({ commits, pal }: { commits: FoundCommit[]; pal: Palette }) {
  const rows = [...commits].sort((a, b) => b.at.localeCompare(a.at))
  return (
    <Table.ScrollContainer minWidth={1100}>
      <Table fz="xs" verticalSpacing={4} highlightOnHover stickyHeader>
        <Table.Thead>
          <Table.Tr>
            {['When', 'Repository', 'Subject', 'Model', '+ / −', 'Where', 'Landed', 'Alive', 'Pull request', 'CI', 'Tested first', 'Effort'].map((h) => (
              <Table.Th key={h} fz="xs">{h}</Table.Th>
            ))}
          </Table.Tr>
        </Table.Thead>
        <Table.Tbody>{rows.map((c) => <CommitRow key={`${c.repo}/${c.hash}`} c={c} pal={pal} />)}</Table.Tbody>
      </Table>
    </Table.ScrollContainer>
  )
}

export function PullRows({ pulls, pal }: { pulls: FoundPull[]; pal: Palette }) {
  const rows = [...pulls].sort((a, b) => b.created_at.localeCompare(a.created_at))
  return (
    <Table.ScrollContainer minWidth={980}>
      <Table fz="xs" verticalSpacing={4} highlightOnHover stickyHeader>
        <Table.Thead>
          <Table.Tr>
            {['Opened', 'Repository', '#', 'Title', 'Opened by', 'State', 'To merge', '+ / −', 'First CI', 'CI runs'].map((h) => <Table.Th key={h} fz="xs">{h}</Table.Th>)}
          </Table.Tr>
        </Table.Thead>
        <Table.Tbody>
          {rows.map((p) => (
            <Table.Tr key={`${p.repo}/${p.number}`}>
              <Table.Td ff="monospace" fz="xs" style={{ whiteSpace: 'nowrap' }}>{local(p.created_at)}</Table.Td>
              <Table.Td fz="xs">{p.repo}</Table.Td>
              <Table.Td fz="xs"><Anchor href={p.url} target="_blank" fz="xs">#{p.number}</Anchor></Table.Td>
              <Table.Td><Text fz="xs" lineClamp={2}>{p.title}</Text></Table.Td>
              <Table.Td>{p.model ? <ModelBadge model={p.model} pal={pal} /> : <Text fz="xs" c="dimmed">no session's</Text>}</Table.Td>
              <Table.Td fz="xs">{p.merged ? 'merged' : p.state.toLowerCase()}</Table.Td>
              <Table.Td ta="right" fz="xs">{p.minutes_to_merge == null ? '—' : `${n(p.minutes_to_merge)} min`}</Table.Td>
              <Table.Td ta="right" fz="xs" style={{ whiteSpace: 'nowrap' }}>+{n(p.additions)} −{n(p.deletions)}</Table.Td>
              <Table.Td>{verdict(p.ci_first)}</Table.Td>
              <Table.Td fz="xs">{p.ci_passed} passed · {p.ci_failed} failed</Table.Td>
            </Table.Tr>
          ))}
        </Table.Tbody>
      </Table>
    </Table.ScrollContainer>
  )
}

export function PromptRows({ prompts, pal }: { prompts: (WinPrompt & { repo?: string })[]; pal: Palette }) {
  return (
    <Table.ScrollContainer minWidth={760}>
      <Table fz="xs" verticalSpacing={4} stickyHeader>
        <Table.Thead>
          <Table.Tr>{['When', 'Repository', 'Model answering', 'Kind', 'Prompt'].map((h) => <Table.Th key={h} fz="xs">{h}</Table.Th>)}</Table.Tr>
        </Table.Thead>
        <Table.Tbody>
          {prompts.map((p, i) => (
            <Table.Tr key={i}>
              <Table.Td ff="monospace" fz="xs" style={{ whiteSpace: 'nowrap' }}>{local(p.at)}</Table.Td>
              <Table.Td fz="xs">{p.repo ?? ''}</Table.Td>
              <Table.Td><ModelBadge model={p.model} pal={pal} /></Table.Td>
              <Table.Td>
                <Group gap={4} wrap="nowrap">
                  <Text fz="xs">{p.kind}</Text>
                  {p.correction && <Badge size="xs" color="yellow" variant="light">correction</Badge>}
                  {p.escalation && <Badge size="xs" color="red" variant="light">escalation</Badge>}
                </Group>
              </Table.Td>
              <Table.Td><Text fz="xs" style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }} lineClamp={6}>{p.text}</Text></Table.Td>
            </Table.Tr>
          ))}
        </Table.Tbody>
      </Table>
    </Table.ScrollContainer>
  )
}

/// A week's prompts in the repositories in view, read from the window.
function WeekPrompts({ week, repos, pal }: { week: string; repos: string[]; pal: Palette }) {
  const from = new Date(`${week}T00:00:00Z`)
  const to = new Date(from.getTime() + 7 * 86400 * 1000)
  const q = useQuery({ queryKey: ['window', from.toISOString(), to.toISOString(), 86400], queryFn: () => fetchWindow(from, to, 86400) })
  if (q.isPending) return <Loader size="xs" />
  if (q.isError) return <Alert color="red" title="The week did not load">{String(q.error)}</Alert>
  const keep = new Set(repos)
  const prompts = q.data.repos.filter((r) => keep.has(r.repo)).flatMap((r) => r.prompts.map((p) => ({ ...p, repo: r.repo })))
  prompts.sort((a, b) => a.at.localeCompare(b.at))
  return <PromptRows prompts={prompts} pal={pal} />
}

/// What a drawer shows.
export type Behind = { title: string; note?: string; commits?: FoundCommit[]; pulls?: FoundPull[]; week?: { week: string; repos: string[] } } | null

export function BehindDrawer({ behind, close, pal }: { behind: Behind; close: () => void; pal: Palette }) {
  let body: ReactNode = null
  if (behind?.commits) body = <CommitRows commits={behind.commits} pal={pal} />
  if (behind?.pulls) body = <PullRows pulls={behind.pulls} pal={pal} />
  return (
    <Drawer opened={behind != null} onClose={close} position="right" size="85%" title={<Text fw={600}>{behind?.title}</Text>} scrollAreaComponent={ScrollArea.Autosize}>
      <Stack gap="sm">
        {behind?.note && <Text fz="sm" c="dimmed">{behind.note}</Text>}
        {behind?.week && (
          <>
            <Text fw={600} fz="sm">Your prompts in the week of {weekLabel(behind.week.week)}</Text>
            <WeekPrompts week={behind.week.week} repos={behind.week.repos} pal={pal} />
          </>
        )}
        {behind?.week && behind.commits && <Text fw={600} fz="sm">Its commits</Text>}
        {body}
      </Stack>
    </Drawer>
  )
}
