import { Table, Text } from '@mantine/core'
import { Board, type Card } from '../Board'
import { EChart, FAIL, INK, OK, type Clicked } from '../EChart'
import { category, grid, grouped, legend, percent, stacked, tip, value, zoom } from './charts'
import { ModelBadge, PullRows, type Behind } from './Rows'
import type { Scoped } from './scope'
import { group, n, pct, quantile, sum, weekLabel, weekOf, type Palette } from './shared'

// Does CI tell you anything, and how do pull requests go: the hosts' own CI
// runs per week, each workflow over all time, who opened each pull request,
// whether its first CI run was red, and how long it waited to merge.

export function ciAnswer(s: Scoped): string {
  const weeks = [...s.repoWeeks].filter((w) => w.ci_passed + w.ci_failed > 0).sort((a, b) => a.week.localeCompare(b.week))
  const byWeek = group(weeks, (w) => w.week)
  const ordered = [...byWeek.keys()].sort()
  if (!ordered.length) return 'No CI runs are held for these repositories: nothing on their hosts judges the code.'
  const passed = (w: string) => sum(byWeek.get(w) ?? [], (r) => r.ci_passed)
  const failed = (w: string) => sum(byWeek.get(w) ?? [], (r) => r.ci_failed)
  const last = [...ordered].reverse().find((w) => passed(w) > 0)
  const after = ordered.filter((w) => !last || w > last)
  const red = sum(after, failed)
  if (after.length && red > 0) {
    const firstRed = s.pulls.filter((p) => p.ci_first === false && after.includes(weekOf(p.created_at)))
    const merged = firstRed.filter((p) => p.merged).length
    return `CI has not passed since the week of ${weekLabel(after[0])}: 0 of ${n(red)} runs in ${after.length} weeks. ${n(firstRed.length)} pull requests opened red since; ${n(merged)} were merged anyway.`
  }
  const total = sum(ordered, passed) + sum(ordered, failed)
  return `CI passed ${pct(sum(ordered, passed), total)}% of ${n(total)} runs.`
}

export function Ci({ s, pal, open }: { s: Scoped; pal: Palette; open: (b: Behind) => void }) {
  const weeks = [...new Set(s.repoWeeks.filter((w) => w.ci_passed + w.ci_failed > 0).map((w) => w.week))].sort()
  const rw = group(s.repoWeeks, (w) => w.week)
  const pullWeeks = [...new Set(s.pulls.map((p) => weekOf(p.created_at)))].sort()
  const byModel = group(s.pulls, (p) => p.model ?? '')
  const models = [...pal.models.filter((m) => byModel.has(m)), ...(byModel.has('') ? [''] : [])]
  const workflows = [...s.workflows].sort((a, b) => b.runs - a.runs)
  const boxes = models
    .map((m) => {
      const mins = (byModel.get(m) ?? []).filter((p) => p.minutes_to_merge != null).map((p) => p.minutes_to_merge as number).sort((a, b) => a - b)
      return { m, mins }
    })
    .filter((b) => b.mins.length)
  const cards: Card[] = [
    {
      i: 'runs',
      title: 'Host CI runs per week',
      note: 'GitHub workflow runs and Bitbucket pipelines; a re-run that turned green counts its red first',
      w: 6,
      h: 10,
      render: (h) => (
        <EChart
          height={h}
          option={{
            legend: legend(),
            grid: grid({ bottom: 28 }),
            tooltip: tip(),
            dataZoom: zoom(),
            xAxis: category(weeks.map(weekLabel)),
            yAxis: value(),
            series: [
              stacked('passed', weeks.map((w) => sum(rw.get(w) ?? [], (r) => r.ci_passed)), OK),
              stacked('failed', weeks.map((w) => sum(rw.get(w) ?? [], (r) => r.ci_failed)), FAIL),
            ],
          }}
        />
      ),
    },
    {
      i: 'workflows',
      title: `The workflows (${workflows.length})`,
      note: 'every host run, over all time',
      w: 6,
      h: 10,
      scroll: true,
      render: () => (
        <Table fz="xs" verticalSpacing={4} stickyHeader>
          <Table.Thead>
            <Table.Tr>{['Repository', 'Workflow', 'Runs', 'Passed', 'Failed', 'Turned', 'Pass'].map((x) => <Table.Th key={x} fz="xs">{x}</Table.Th>)}</Table.Tr>
          </Table.Thead>
          <Table.Tbody>
            {workflows.map((w) => (
              <Table.Tr key={`${w.repo}/${w.name}/${w.kind}`}>
                <Table.Td fz="xs">{w.repo}</Table.Td>
                <Table.Td fz="xs">{w.name}</Table.Td>
                <Table.Td ta="right" fz="xs">{n(w.runs)}</Table.Td>
                <Table.Td ta="right" fz="xs">{n(w.passed)}</Table.Td>
                <Table.Td ta="right" fz="xs" c={w.failed ? 'red.4' : undefined}>{n(w.failed)}</Table.Td>
                <Table.Td ta="right" fz="xs">{n(w.turned)}</Table.Td>
                <Table.Td ta="right" fz="xs">{pct(w.passed, w.passed + w.failed)}%</Table.Td>
              </Table.Tr>
            ))}
          </Table.Tbody>
        </Table>
      ),
    },
    {
      i: 'opened',
      title: 'Pull requests opened per week, by the model that opened them',
      note: 'opened by: the session whose gh pr create printed it, else the model with most lines in it · click a bar',
      w: 6,
      h: 10,
      render: (h) => (
        <EChart
          height={h}
          onClick={(p: Clicked) => {
            const m = p.seriesName === 'no session' ? '' : (p.seriesName ?? '')
            const w = p.dataIndex != null ? pullWeeks[p.dataIndex] : undefined
            if (w != null) open({ title: `Pull requests opened by ${m || 'no session'}, week of ${weekLabel(w)}`, pulls: (byModel.get(m) ?? []).filter((x) => weekOf(x.created_at) === w) })
          }}
          option={{
            legend: legend(),
            grid: grid({ bottom: 28 }),
            tooltip: tip(),
            dataZoom: zoom(),
            xAxis: category(pullWeeks.map(weekLabel)),
            yAxis: value(),
            series: models.map((m) => stacked(m || 'no session', pullWeeks.map((w) => (byModel.get(m) ?? []).filter((p) => weekOf(p.created_at) === w).length), m ? pal.colour(m) : '#5C4470')),
          }}
        />
      ),
    },
    {
      i: 'first',
      title: 'First CI run red, and merged anyway, by the model that opened them',
      note: 'share of its pull requests whose first CI verdict failed · and of those, merged',
      w: 6,
      h: 10,
      render: (h) => (
        <EChart
          height={h}
          onClick={(p: Clicked) => {
            const m = p.name === 'no session' ? '' : (p.name ?? '')
            open({ title: `Pull requests opened by ${m || 'no session'} whose first CI run failed`, pulls: (byModel.get(m) ?? []).filter((x) => x.ci_first === false) })
          }}
          option={{
            legend: legend(),
            grid: grid(),
            tooltip: { ...tip(), valueFormatter: (v: number) => `${v}%` },
            xAxis: category(models.map((m) => m || 'no session')),
            yAxis: percent(),
            series: [
              grouped('first run red', models.map((m) => { const ps = (byModel.get(m) ?? []).filter((p) => p.ci_first != null); return pct(ps.filter((p) => p.ci_first === false).length, ps.length) }), FAIL),
              grouped('red and merged', models.map((m) => { const ps = (byModel.get(m) ?? []).filter((p) => p.ci_first === false); return pct(ps.filter((p) => p.merged).length, ps.length) }), '#d95926'),
            ],
          }}
        />
      ),
    },
    {
      i: 'merge',
      title: 'Minutes from opening to merge',
      note: 'box: middle half · line: median · whiskers: fastest and slowest',
      w: 6,
      h: 10,
      render: (h) => (
        <EChart
          height={h}
          option={{
            grid: grid({ top: 16 }),
            tooltip: { trigger: 'item', backgroundColor: '#150420', borderColor: '#3B2449', textStyle: { color: INK } },
            xAxis: category(boxes.map((b) => b.m || 'no session')),
            yAxis: value((v) => `${v}`, { type: 'log', min: 1, name: 'minutes', nameTextStyle: { color: INK } }),
            series: [
              {
                type: 'boxplot',
                itemStyle: { color: '#341A41', borderColor: '#CC66FF' },
                data: boxes.map((b) => [b.mins[0], quantile(b.mins, 0.25), quantile(b.mins, 0.5), quantile(b.mins, 0.75), b.mins[b.mins.length - 1]].map((v) => Math.max(1, Math.round(v)))),
              },
            ],
          }}
        />
      ),
    },
    {
      i: 'models',
      title: 'Each model\'s pull requests',
      w: 6,
      h: 10,
      scroll: true,
      render: () => (
        <Table fz="xs" verticalSpacing={4}>
          <Table.Thead>
            <Table.Tr>{['Opened by', 'Opened', 'Merged', 'Closed', 'Open', 'Median to merge', 'First CI red'].map((x) => <Table.Th key={x} fz="xs">{x}</Table.Th>)}</Table.Tr>
          </Table.Thead>
          <Table.Tbody>
            {models.map((m) => {
              const ps = byModel.get(m) ?? []
              const mins = ps.filter((p) => p.minutes_to_merge != null).map((p) => p.minutes_to_merge as number).sort((a, b) => a - b)
              const first = ps.filter((p) => p.ci_first != null)
              return (
                <Table.Tr key={m} style={{ cursor: 'pointer' }} onClick={() => open({ title: `Pull requests opened by ${m || 'no session'}`, pulls: ps })}>
                  <Table.Td>{m ? <ModelBadge model={m} pal={pal} /> : <Text fz="xs" c="dimmed">no session's</Text>}</Table.Td>
                  <Table.Td ta="right" fz="xs">{n(ps.length)}</Table.Td>
                  <Table.Td ta="right" fz="xs">{n(ps.filter((p) => p.merged).length)}</Table.Td>
                  <Table.Td ta="right" fz="xs">{n(ps.filter((p) => !p.merged && p.state.toLowerCase() !== 'open').length)}</Table.Td>
                  <Table.Td ta="right" fz="xs">{n(ps.filter((p) => p.state.toLowerCase() === 'open').length)}</Table.Td>
                  <Table.Td ta="right" fz="xs">{mins.length ? `${n(quantile(mins, 0.5))} min` : '—'}</Table.Td>
                  <Table.Td ta="right" fz="xs">{first.length ? `${first.filter((p) => p.ci_first === false).length} of ${first.length}` : '—'}</Table.Td>
                </Table.Tr>
              )
            })}
          </Table.Tbody>
        </Table>
      ),
    },
    { i: 'pulls', title: `Every pull request in view (${n(s.pulls.length)})`, w: 12, h: 12, scroll: true, render: () => <PullRows pulls={s.pulls} pal={pal} /> },
  ]
  return <Board name="results.ci" cards={cards} />
}
