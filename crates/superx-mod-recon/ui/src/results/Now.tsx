import { useState } from 'react'
import { Alert, Anchor, Badge, Group, Loader, Paper, SegmentedControl, SimpleGrid, Stack, Table, Tabs, Text, Title } from '@mantine/core'
import { useQuery } from '@tanstack/react-query'
import { Board, type Card } from '../Board'
import { EChart, FAIL, OK } from '../EChart'
import { fetchWindow, type WinCommit, type WinRepo, type Window } from '../api'
import { category, grid, grouped, legend, stacked, tip, value, zoom } from './charts'
import { ModelBadge, Place, PromptRows, Whole } from './Rows'
import { local, n, palette, type Palette } from './shared'

// What was done in each repository, now: the last hour, today, yesterday,
// the last 7 or 30 days, in the operator's own time. Who worked there, what
// it cost, what changed, the commits and what became of them, the pull
// requests, CI, the reflog, the tests and builds, every prompt; each
// repository a board of its own.

type Preset = { key: string; label: string; range: () => [Date, Date]; step: number; every: number }

const midnight = () => {
  const d = new Date()
  d.setHours(0, 0, 0, 0)
  return d
}
const days = (d: Date, k: number) => new Date(d.getTime() + k * 86400 * 1000)

const PRESETS: Preset[] = [
  { key: 'hour', label: 'Last hour', range: () => [new Date(Date.now() - 3600 * 1000), new Date()], step: 300, every: 60_000 },
  { key: 'today', label: 'Today', range: () => [midnight(), new Date()], step: 3600, every: 60_000 },
  { key: 'yesterday', label: 'Yesterday', range: () => [days(midnight(), -1), midnight()], step: 3600, every: 600_000 },
  { key: 'week', label: '7 days', range: () => [days(midnight(), -6), new Date()], step: 21600, every: 300_000 },
  { key: 'month', label: '30 days', range: () => [days(midnight(), -29), new Date()], step: 86400, every: 600_000 },
]

function steps(w: Window): number[] {
  const out: number[] = []
  const a = new Date(w.from).getTime()
  const b = new Date(w.to).getTime()
  for (let t = a; t < b; t += w.step_secs * 1000) out.push(t)
  return out
}

const stepLabel = (t: number, step: number) =>
  step < 86400
    ? new Date(t).toLocaleString('en-GB', step >= 21600 ? { day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' } : { hour: '2-digit', minute: '2-digit' })
    : new Date(t).toLocaleDateString('en-GB', { day: 'numeric', month: 'short' })

function Tile({ label, value: v, sub, tone }: { label: string; value: string; sub?: string; tone?: 'bad' | 'warn' | 'good' }) {
  return (
    <Paper withBorder radius="sm" p={8} bg="dark.6">
      <Text fz={20} fw={700} c={tone === 'bad' ? 'red.4' : tone === 'warn' ? 'yellow.5' : tone === 'good' ? 'teal.4' : undefined} style={{ fontVariantNumeric: 'tabular-nums', lineHeight: 1.15 }}>
        {v}
      </Text>
      <Text fz="xs">{label}</Text>
      {sub && <Text fz={11} c="dimmed" ff="monospace">{sub}</Text>}
    </Paper>
  )
}

function CommitRow({ repo, c, pal }: { repo: string; c: WinCommit; pal: Palette }) {
  const [open, setOpen] = useState(false)
  return (
    <>
      <Table.Tr style={{ cursor: 'pointer' }} onClick={() => setOpen(!open)}>
        <Table.Td ff="monospace" fz="xs" style={{ whiteSpace: 'nowrap' }}>{open ? '▾' : '▸'} {local(c.at)}</Table.Td>
        <Table.Td ff="monospace" fz="xs">{c.hash.slice(0, 8)}</Table.Td>
        <Table.Td><Text fz="xs" lineClamp={2}>{c.subject}</Text></Table.Td>
        <Table.Td>{c.squash ? <Text fz="xs" c="dimmed">squash of branch work</Text> : <ModelBadge model={c.model} pal={pal} />}</Table.Td>
        <Table.Td ta="right" fz="xs" style={{ whiteSpace: 'nowrap' }}>+{n(c.added)} −{n(c.removed)}</Table.Td>
        <Table.Td ta="right" fz="xs">{n(c.files.length)}</Table.Td>
        <Table.Td><Place c={c} /></Table.Td>
        <Table.Td ta="right" fz="xs">{n(c.landed)}</Table.Td>
        <Table.Td>{c.ci == null ? <Text fz="xs" c="dimmed">—</Text> : <Badge size="xs" variant="light" color={c.ci ? 'teal' : 'red'}>{c.ci ? 'passed' : 'failed'}</Badge>}</Table.Td>
        <Table.Td fz="xs">{c.pulls.length ? `#${c.pulls.join(' #')}` : '—'}</Table.Td>
        <Table.Td fz="xs">{c.tested == null ? '—' : c.tested ? 'yes' : 'no'}</Table.Td>
      </Table.Tr>
      {open && (
        <Table.Tr>
          <Table.Td colSpan={11} bg="dark.7"><Whole repo={repo} hash={c.hash} /></Table.Td>
        </Table.Tr>
      )}
    </>
  )
}

function list(headers: string[], rows: React.ReactNode, min = 900) {
  return (
    <Table.ScrollContainer minWidth={min}>
      <Table fz="xs" verticalSpacing={4} stickyHeader highlightOnHover>
        <Table.Thead><Table.Tr>{headers.map((h) => <Table.Th key={h} fz="xs">{h}</Table.Th>)}</Table.Tr></Table.Thead>
        <Table.Tbody>{rows}</Table.Tbody>
      </Table>
    </Table.ScrollContainer>
  )
}

function Rows({ r, pal }: { r: WinRepo; pal: Palette }) {
  return (
    <Tabs defaultValue="commits" keepMounted={false}>
      <Tabs.List>
        <Tabs.Tab value="commits">Commits ({r.commits.length})</Tabs.Tab>
        <Tabs.Tab value="files">Files ({r.files.length})</Tabs.Tab>
        <Tabs.Tab value="pulls">Pull requests ({r.pulls.length})</Tabs.Tab>
        <Tabs.Tab value="ci">CI ({r.ci.length})</Tabs.Tab>
        <Tabs.Tab value="checks">Tests and builds ({r.checks.length})</Tabs.Tab>
        <Tabs.Tab value="moves">Reflog ({r.moves.length})</Tabs.Tab>
        <Tabs.Tab value="prompts">Prompts ({r.prompts.length})</Tabs.Tab>
        <Tabs.Tab value="git">Agents' git ({r.git.length})</Tabs.Tab>
        <Tabs.Tab value="sessions">Sessions ({r.sessions.length})</Tabs.Tab>
      </Tabs.List>
      <Tabs.Panel value="commits" pt={6}>
        {list(['When', 'Commit', 'Subject', 'Model', '+ / −', 'Files', 'Where', 'Landed', 'CI', 'Pull request', 'Tested first'], [...r.commits].reverse().map((c) => <CommitRow key={c.hash} repo={r.repo} c={c} pal={pal} />), 1100)}
      </Tabs.Panel>
      <Tabs.Panel value="files" pt={6}>
        {list(['Path', 'Models', 'Edits', 'Edited', 'Commits', 'Committed'], r.files.map((f) => (
          <Table.Tr key={f.path}>
            <Table.Td ff="monospace" fz="xs" style={{ overflowWrap: 'anywhere' }}>{f.path}</Table.Td>
            <Table.Td><Group gap={4}>{f.models.map((m) => <ModelBadge key={m} model={m} pal={pal} />)}</Group></Table.Td>
            <Table.Td ta="right" fz="xs">{n(f.edits)}</Table.Td>
            <Table.Td ta="right" fz="xs">+{n(f.edit_added)} −{n(f.edit_removed)}</Table.Td>
            <Table.Td ta="right" fz="xs">{n(f.commits)}</Table.Td>
            <Table.Td ta="right" fz="xs">+{n(f.commit_added)} −{n(f.commit_removed)}</Table.Td>
          </Table.Tr>
        )))}
      </Tabs.Panel>
      <Tabs.Panel value="pulls" pt={6}>
        {list(['#', 'Title', 'Here', 'Opened by', 'State', '+ / −', 'First CI', 'CI runs'], r.pulls.map((p) => (
          <Table.Tr key={p.number}>
            <Table.Td fz="xs"><Anchor href={p.url} target="_blank" fz="xs">#{p.number}</Anchor></Table.Td>
            <Table.Td><Text fz="xs" lineClamp={2}>{p.title}</Text></Table.Td>
            <Table.Td fz="xs">{p.happened.join(', ')}</Table.Td>
            <Table.Td>{p.model ? <ModelBadge model={p.model} pal={pal} /> : <Text fz="xs" c="dimmed">no session's</Text>}</Table.Td>
            <Table.Td fz="xs">{p.merged ? 'merged' : p.state.toLowerCase()}</Table.Td>
            <Table.Td ta="right" fz="xs">+{n(p.additions)} −{n(p.deletions)}</Table.Td>
            <Table.Td>{p.ci_first == null ? '—' : <Badge size="xs" variant="light" color={p.ci_first ? 'teal' : 'red'}>{p.ci_first ? 'passed' : 'failed'}</Badge>}</Table.Td>
            <Table.Td fz="xs">{p.ci_passed} passed · {p.ci_failed} failed</Table.Td>
          </Table.Tr>
        )))}
      </Tabs.Panel>
      <Tabs.Panel value="ci" pt={6}>
        {list(['Started', 'Kind', 'Workflow', 'Ref', 'Pull request', 'Verdict'], [...r.ci].reverse().map((c, i) => (
          <Table.Tr key={i}>
            <Table.Td ff="monospace" fz="xs">{local(c.created_at)}</Table.Td>
            <Table.Td fz="xs">{c.kind.replace('_', ' ')}</Table.Td>
            <Table.Td fz="xs">{c.url ? <Anchor href={c.url} target="_blank" fz="xs">{c.name || '—'}</Anchor> : c.name || '—'}</Table.Td>
            <Table.Td ff="monospace" fz="xs">{c.git_ref ?? '—'}</Table.Td>
            <Table.Td fz="xs">{c.pull ? `#${c.pull}` : '—'}</Table.Td>
            <Table.Td>{c.verdict == null ? <Text fz="xs" c="dimmed">{c.status ?? 'running'}</Text> : <Badge size="xs" variant="light" color={c.verdict ? 'teal' : 'red'}>{c.verdict ? (c.failed_before ? 'passed on re-run' : 'passed') : 'failed'}</Badge>}</Table.Td>
          </Table.Tr>
        )))}
      </Tabs.Panel>
      <Tabs.Panel value="checks" pt={6}>
        {list(['When', 'What', 'Model', 'Result', 'Command'], [...r.checks].reverse().map((k, i) => (
          <Table.Tr key={i}>
            <Table.Td ff="monospace" fz="xs">{local(k.at)}</Table.Td>
            <Table.Td fz="xs">{k.test ? 'test' : 'build'} · {k.what}</Table.Td>
            <Table.Td><ModelBadge model={k.model} pal={pal} /></Table.Td>
            <Table.Td>{k.test ? <Badge size="xs" variant="light" color={k.failed ? 'red' : 'teal'}>{k.passed} passed · {k.failed} failed</Badge> : <Badge size="xs" variant="light" color={k.ok ? 'teal' : 'red'}>{k.ok ? 'built' : 'failed'}</Badge>}</Table.Td>
            <Table.Td ff="monospace" fz="xs" style={{ overflowWrap: 'anywhere' }}><Text fz="xs" ff="monospace" lineClamp={2}>{k.command}</Text></Table.Td>
          </Table.Tr>
        )))}
      </Tabs.Panel>
      <Tabs.Panel value="moves" pt={6}>
        {list(['When', 'Ref', 'Kind', 'Message', 'Who'], [...r.moves].reverse().map((m, i) => (
          <Table.Tr key={i}>
            <Table.Td ff="monospace" fz="xs">{local(m.at)}</Table.Td>
            <Table.Td ff="monospace" fz="xs">{m.git_ref}{m.mirror ? ' (mirror)' : ''}</Table.Td>
            <Table.Td><Badge size="xs" variant="light" color={['amend', 'reset', 'rebase', 'forced'].includes(m.kind) ? 'yellow' : 'gray'}>{m.kind}</Badge></Table.Td>
            <Table.Td fz="xs" style={{ overflowWrap: 'anywhere' }}>{m.action}</Table.Td>
            <Table.Td fz="xs">{m.identity ?? '—'}</Table.Td>
          </Table.Tr>
        )))}
      </Tabs.Panel>
      <Tabs.Panel value="prompts" pt={6}><PromptRows prompts={[...r.prompts].reverse()} pal={pal} /></Tabs.Panel>
      <Tabs.Panel value="git" pt={6}>
        {list(['When', 'Action', 'Model', 'Branch', 'Hash', 'Subject', 'Pull request'], [...r.git].reverse().map((g, i) => (
          <Table.Tr key={i}>
            <Table.Td ff="monospace" fz="xs">{local(g.at)}</Table.Td>
            <Table.Td fz="xs">{g.action.replace('_', ' ')}</Table.Td>
            <Table.Td><ModelBadge model={g.model} pal={pal} /></Table.Td>
            <Table.Td ff="monospace" fz="xs">{g.branch ?? '—'}</Table.Td>
            <Table.Td ff="monospace" fz="xs">{g.hash?.slice(0, 8) ?? '—'}</Table.Td>
            <Table.Td><Text fz="xs" lineClamp={2}>{g.subject ?? ''}</Text></Table.Td>
            <Table.Td fz="xs">{g.url ? <Anchor href={g.url} target="_blank" fz="xs">#{g.pr_number}</Anchor> : g.pr_number ? `#${g.pr_number}` : '—'}</Table.Td>
          </Table.Tr>
        )))}
      </Tabs.Panel>
      <Tabs.Panel value="sessions" pt={6}>
        {list(['Session', 'Agent', 'Models', 'From', 'To', 'Hours', 'Replies', 'Tool calls', 'Output tokens', 'Edited', 'Prompts', 'Commits', 'Tests', 'Builds'], r.sessions.map((x) => (
          <Table.Tr key={x.session}>
            <Table.Td ff="monospace" fz="xs">{x.session.slice(0, 8)}</Table.Td>
            <Table.Td fz="xs">{x.agent}</Table.Td>
            <Table.Td><Group gap={4}>{x.models.map((m) => <ModelBadge key={m} model={m} pal={pal} />)}</Group></Table.Td>
            <Table.Td ff="monospace" fz="xs">{local(x.first)}</Table.Td>
            <Table.Td ff="monospace" fz="xs">{local(x.last)}</Table.Td>
            <Table.Td ta="right" fz="xs">{(x.active_secs / 3600).toFixed(1)}</Table.Td>
            <Table.Td ta="right" fz="xs">{n(x.replies)}</Table.Td>
            <Table.Td ta="right" fz="xs">{n(x.tool_calls)}</Table.Td>
            <Table.Td ta="right" fz="xs">{n(x.out_tokens)}</Table.Td>
            <Table.Td ta="right" fz="xs">+{n(x.edit_added)} −{n(x.edit_removed)}</Table.Td>
            <Table.Td ta="right" fz="xs">{n(x.prompts)}</Table.Td>
            <Table.Td ta="right" fz="xs">{n(x.commits)}</Table.Td>
            <Table.Td ta="right" fz="xs">{n(x.tests)}</Table.Td>
            <Table.Td ta="right" fz="xs">{n(x.builds)}</Table.Td>
          </Table.Tr>
        )), 1200)}
      </Tabs.Panel>
    </Tabs>
  )
}

function RepoBoard({ w, r, pal }: { w: Window; r: WinRepo; pal: Palette }) {
  const t = r.totals
  const ts = steps(w)
  const at = new Map(ts.map((x, i) => [x, i]))
  const models = [...new Set(r.timeline.map((p) => p.model))].sort((a, b) => pal.models.indexOf(a) - pal.models.indexOf(b))
  const series = (field: 'replies' | 'out_tokens' | 'edit_added' | 'commit_added' | 'commits' | 'tool_calls', model?: string) => {
    const d = ts.map(() => 0)
    for (const p of r.timeline) {
      if (model != null && p.model !== model) continue
      const i = at.get(new Date(p.start).getTime())
      if (i != null) d[i] += p[field]
    }
    return d
  }
  const labels = ts.map((x) => stepLabel(x, w.step_secs))
  const byModel = (field: 'replies' | 'out_tokens') => (h: number) => (
    <EChart
      height={h}
      option={{
        legend: legend(),
        grid: grid({ bottom: 28 }),
        tooltip: tip(),
        dataZoom: zoom(),
        xAxis: category(labels),
        yAxis: value(),
        series: models.map((m) => stacked(m || 'not credited', series(field, m), m ? pal.colour(m) : '#5C4470')),
      }}
    />
  )
  const hours = (t.active_secs / 3600).toFixed(1)
  const cards: Card[] = [
    {
      i: 'numbers',
      title: 'In the window',
      w: 12,
      h: 6,
      scroll: true,
      render: () => (
        <SimpleGrid cols={{ base: 2, sm: 4, md: 6, lg: 8 }} spacing={6}>
          <Tile label="active hours" value={hours} sub={`${t.sessions} sessions · ${t.agents} agents`} />
          <Tile label="your prompts" value={n(t.prompts)} sub={`${t.corrections} corr · ${t.escalations} esc · ${t.interrupts} stops`} tone={t.escalations ? 'warn' : undefined} />
          <Tile label="replies · tool calls" value={`${n(t.replies)} · ${n(t.tool_calls)}`} sub={`${n(t.out_tokens)} output tokens`} />
          <Tile label="lines the agents edited" value={`+${n(t.edit_added)}`} sub={`−${n(t.edit_removed)} in ${t.edit_files} files`} />
          <Tile label="commits" value={n(t.commits)} sub={`+${n(t.commit_added)} −${n(t.commit_removed)}`} />
          <Tile
            label="where they are"
            value={`${t.commits_mainline + t.commits_landed} landed`}
            sub={`${t.commits_in_flight} in flight · ${t.commits_side} side folder · ${t.commits_branch} branch · ${t.commits_recovered} reset · ${t.commits_snapshot} snapshots`}
            tone={t.commits_branch + t.commits_recovered > 0 ? 'warn' : undefined}
          />
          <Tile label="lines landed · alive" value={`${n(t.landed)} · ${n(t.alive)}`} sub={`${t.reverted} reverted`} />
          <Tile label="pull requests" value={`${t.prs_opened} opened`} sub={`${t.prs_merged} merged · ${t.prs_closed} closed`} />
          <Tile label="CI runs" value={`${t.ci_passed} passed`} sub={`${t.ci_failed} failed`} tone={t.ci_failed && !t.ci_passed ? 'bad' : t.ci_failed ? 'warn' : undefined} />
          <Tile label="test runs" value={n(t.tests)} sub={`${t.tests_failed} with failures`} tone={t.tests_failed ? 'warn' : undefined} />
          <Tile label="builds" value={n(t.builds)} sub={`${t.builds_failed} failed`} tone={t.builds_failed ? 'warn' : undefined} />
          <Tile label="reflog moves" value={n(t.moves)} sub={`${t.rewrites} rewrites · ${t.pushes} pushes`} tone={t.rewrites ? 'warn' : undefined} />
        </SimpleGrid>
      ),
    },
    {
      i: 'models',
      title: 'Each model here',
      w: 12,
      h: 5,
      scroll: true,
      render: () =>
        list(['Model', 'Sessions', 'Hours', 'Replies', 'Output tokens', 'Edited', 'Commits', 'Committed', 'Landed', 'Alive', 'Prompts', 'Corr · esc', 'Tests (failing)'], r.models.map((m) => (
          <Table.Tr key={m.model}>
            <Table.Td><ModelBadge model={m.model} pal={pal} /></Table.Td>
            <Table.Td ta="right" fz="xs">{n(m.sessions)}</Table.Td>
            <Table.Td ta="right" fz="xs">{(m.active_secs / 3600).toFixed(1)}</Table.Td>
            <Table.Td ta="right" fz="xs">{n(m.replies)}</Table.Td>
            <Table.Td ta="right" fz="xs">{n(m.out_tokens)}</Table.Td>
            <Table.Td ta="right" fz="xs">+{n(m.edit_added)} −{n(m.edit_removed)}</Table.Td>
            <Table.Td ta="right" fz="xs">{n(m.commits)}</Table.Td>
            <Table.Td ta="right" fz="xs">+{n(m.commit_added)} −{n(m.commit_removed)}</Table.Td>
            <Table.Td ta="right" fz="xs">{n(m.landed)}</Table.Td>
            <Table.Td ta="right" fz="xs">{n(m.alive)}</Table.Td>
            <Table.Td ta="right" fz="xs">{n(m.prompts)}</Table.Td>
            <Table.Td ta="right" fz="xs">{m.corrections} · {m.escalations}</Table.Td>
            <Table.Td ta="right" fz="xs">{m.tests} ({m.tests_failed})</Table.Td>
          </Table.Tr>
        )), 1100),
    },
    { i: 'replies', title: 'Replies per step, by model', w: 6, h: 9, render: byModel('replies') },
    { i: 'tokens', title: 'Output tokens per step, by model', w: 6, h: 9, render: byModel('out_tokens') },
    {
      i: 'lines',
      title: 'Lines the agents edited, and lines the commits took',
      w: 6,
      h: 9,
      render: (h) => (
        <EChart
          height={h}
          option={{
            legend: legend(),
            grid: grid({ bottom: 28 }),
            tooltip: tip(),
            dataZoom: zoom(),
            xAxis: category(labels),
            yAxis: value(),
            series: [grouped('edited (+)', series('edit_added'), '#CC66FF'), grouped('committed (+)', series('commit_added'), '#3987e5')],
          }}
        />
      ),
    },
    {
      i: 'commits-step',
      title: 'Commits per step, by model',
      w: 6,
      h: 9,
      render: (h) => (
        <EChart
          height={h}
          option={{
            legend: legend(),
            grid: grid({ bottom: 28 }),
            tooltip: tip(),
            dataZoom: zoom(),
            xAxis: category(labels),
            yAxis: value(),
            series: models.map((m) => stacked(m || 'not credited', series('commits', m), m ? pal.colour(m) : '#5C4470')),
          }}
        />
      ),
    },
    { i: 'rows', title: 'Every row behind the numbers', note: 'click a commit for the whole of it', w: 12, h: 13, scroll: true, render: () => <Rows r={r} pal={pal} /> },
  ]
  return <Board name={`results.now.${r.repo || 'none'}`} cards={cards} />
}

export function Now({ pal }: { pal: Palette }) {
  const [key, setKey] = useState('today')
  const preset = PRESETS.find((p) => p.key === key) ?? PRESETS[1]
  const q = useQuery({
    queryKey: ['window', preset.key],
    queryFn: () => {
      const [from, to] = preset.range()
      return fetchWindow(from, to, preset.step)
    },
    refetchInterval: preset.every,
  })
  const w = q.data
  // Until the findings have loaded, the models take their colours from
  // this window, in the same order.
  const shown = pal.models.length || !w ? pal : palette(w.repos.flatMap((r) => r.models))
  return (
    <Stack gap="md">
      <Group justify="space-between" align="center">
        <SegmentedControl value={key} onChange={setKey} data={PRESETS.map((p) => ({ value: p.key, label: p.label }))} />
        {w && <Text fz="sm" c="dimmed">From {local(w.from)} to {local(w.to)}, your time · {w.repos.length} repositories with work · read {local(w.computed_at)}</Text>}
      </Group>
      {q.isPending && <Loader size="sm" />}
      {q.isError && <Alert color="red" title="The window did not load">{String(q.error)}</Alert>}
      {w && w.repos.length === 0 && <Text c="dimmed">Nothing was done in this window.</Text>}
      {w && w.repos.length > 0 && (
        <Board
          name="results.now.overview"
          cards={[
            {
              i: 'hours',
              title: 'Where the work went: active hours by repository',
              w: 6,
              h: 8,
              render: (h) => {
                const repos = w.repos.map((r) => r.repo || '(no repository)')
                const models = [...new Set(w.repos.flatMap((r) => r.models.map((m) => m.model)))].sort((a, b) => shown.models.indexOf(a) - shown.models.indexOf(b))
                return (
                  <EChart
                    height={h}
                    option={{
                      legend: legend(),
                      grid: grid(),
                      tooltip: tip({ valueFormatter: (v: number) => `${v} h` }),
                      xAxis: value(),
                      yAxis: category([...repos].reverse()),
                      series: models.map((m) => stacked(m || 'not credited', [...w.repos].reverse().map((r) => Math.round((r.models.find((x) => x.model === m)?.active_secs ?? 0) / 360) / 10), m ? shown.colour(m) : '#5C4470', 'h')),
                    }}
                  />
                )
              },
            },
            {
              i: 'got',
              title: 'What each repository got',
              w: 6,
              h: 8,
              render: (h) => {
                const repos = w.repos.map((r) => r.repo || '(no repository)')
                return (
                  <EChart
                    height={h}
                    option={{
                      legend: legend(),
                      grid: grid(),
                      tooltip: tip(),
                      xAxis: category(repos, { axisLabel: { color: '#CFC2DB', fontSize: 10, interval: 0, rotate: repos.length > 4 ? 20 : 0 } }),
                      yAxis: value(),
                      series: [
                        grouped('commits', w.repos.map((r) => r.totals.commits), '#3987e5'),
                        grouped('pull requests opened', w.repos.map((r) => r.totals.prs_opened), '#B833E8'),
                        grouped('merged', w.repos.map((r) => r.totals.prs_merged), OK),
                        grouped('CI failed', w.repos.map((r) => r.totals.ci_failed), FAIL),
                        grouped('test runs failing', w.repos.map((r) => r.totals.tests_failed), '#c98500'),
                      ],
                    }}
                  />
                )
              },
            },
          ]}
        />
      )}
      {w?.repos.map((r) => (
        <Paper key={r.repo} withBorder radius="md" p="sm" bg="dark.8">
          <Stack gap={6}>
            <Group gap="sm" align="baseline">
              <Title order={3}>{r.repo || 'In no repository'}</Title>
              {Object.entries(r.agents).map(([a, k]) => <Badge key={a} variant="outline" color="gray">{a || 'agent'} · {k} session{k === 1 ? '' : 's'}</Badge>)}
              {r.models.map((m) => <ModelBadge key={m.model} model={m.model} pal={shown} />)}
            </Group>
            <RepoBoard w={w} r={r} pal={shown} />
          </Stack>
        </Paper>
      ))}
    </Stack>
  )
}
