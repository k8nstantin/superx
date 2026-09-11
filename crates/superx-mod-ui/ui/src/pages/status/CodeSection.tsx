import { Badge, Grid, Group, SimpleGrid, Table, Text, Tooltip } from '@mantine/core'
import type { StatsSummary } from '../../generated/StatsSummary'
import { AXIS, EChart, GRID_LINE, INK_MUTED, MONO, TOOLTIP } from '../../EChart'
import {
  BANDS,
  BarList,
  CANCEL,
  Churn,
  Counter,
  FAIL,
  Meter,
  OK,
  Panel,
  baseName,
  fmtCompact,
  n,
  pct,
  rangeLabel,
} from './parts'

// What got built (#367): the code the range produced, what it was made
// of, and whether it accumulated or was redone.

export function CodeSection({ s, range }: { s: StatsSummary | undefined; range: string | null }) {
  const note = rangeLabel(range, s?.window_messages)
  const long = range === '7d' || range === '30d' || range === 'all'

  const added = n(s?.lines_added)
  const replaced = n(s?.lines_removed)
  // Shell edits and notebook cells replace text that is not on the line
  // (#383): with nothing else replaced the ratio is unknown, not 0%.
  const unknown = n(s?.replaced_unknown)
  const churnPct = replaced === 0 && unknown > 0 ? null : pct(replaced, added + replaced)
  // Churn as the repository saw it (#386): what landed on main, read
  // with git. When the transcript cannot say what its edits replaced,
  // this is the ratio that can.
  const landedAdded = n(s?.landed?.added)
  const landedRemoved = n(s?.landed?.removed)
  const landedCommits = n(s?.landed?.commits)
  const landedPct = pct(landedRemoved, landedAdded + landedRemoved)
  const shownPct = churnPct ?? landedPct
  const churnTone = shownPct == null ? undefined : shownPct >= BANDS.churnBad ? FAIL : shownPct >= BANDS.churnOk ? CANCEL : OK
  const band = (p: number) => (p >= BANDS.churnBad ? 'mostly rewriting' : p >= BANDS.churnOk ? 'revising as it goes' : 'mostly new code')
  const churnRead =
    churnPct != null
      ? `${band(churnPct)}${unknown > 0 ? ` · ${unknown} more edit${unknown === 1 ? '' : 's'} of unknown size` : ''}${landedPct != null ? ` · landed on main: ${landedPct}%` : ''}`
      : landedPct != null
        ? `${band(landedPct)} — as the repository saw it: ${fmtCompact(landedAdded)} added, ${fmtCompact(landedRemoved)} removed in ${landedCommits} commit${landedCommits === 1 ? '' : 's'} on main${unknown > 0 ? ` · ${unknown} transcript edit${unknown === 1 ? '' : 's'} of unknown size` : ''}`
        : unknown > 0
          ? `${unknown} edit${unknown === 1 ? '' : 's'} replaced an unknown number of lines`
          : ''
  const tokensPerLine = s && added > 0 ? Math.round(n(s.out_tokens_window) / added) : null
  // The cost of a shipped unit (#381): what the range's output tokens
  // bought in merged PRs.
  const tokensPerPr = s && n(s.prs_merged) > 0 ? Math.round(n(s.out_tokens_window) / n(s.prs_merged)) : null
  const testsPer100 = s && added > 0 ? Math.round((n(s.tests_run) * 100 * 10) / added) / 10 : null

  const wr = n(s?.writes_window)
  const rd = n(s?.reads_window)
  const makeRatio = pct(wr, wr + rd)

  // A 30-day range is ~720 hourly points: unreadable. Fold to days.
  type Pt = { t: string; added: number; removed: number }
  const fold = (pts: { t: string; added: number | bigint; removed: number | bigint }[]): Pt[] => {
    if (!long) return pts.map((p) => ({ t: p.t, added: n(p.added), removed: n(p.removed) }))
    const byDay = new Map<string, Pt>()
    for (const p of pts) {
      const day = p.t.slice(0, 10)
      const cur = byDay.get(day) ?? { t: day, added: 0, removed: 0 }
      cur.added += n(p.added)
      cur.removed += n(p.removed)
      byDay.set(day, cur)
    }
    return [...byDay.values()].sort((a, b) => a.t.localeCompare(b.t))
  }
  const churnSeries = fold(s?.churn ?? [])
  // What landed on main in the same buckets (#386), so the two read on
  // one axis: the transcript's edits beside the repository's commits.
  const landedSeries = fold(s?.landed?.series ?? [])
  const buckets = [...new Set([...churnSeries, ...landedSeries].map((p) => p.t))].sort()
  const at = (series: Pt[], key: 'added' | 'removed') => {
    const m = new Map(series.map((p) => [p.t, p[key]]))
    return buckets.map((b) => m.get(b) ?? 0)
  }

  return (
    <>
      <Grid mb="md" gap="md">
        <Grid.Col span={{ base: 12, lg: 4 }}>
          <Panel title="Code written" scope="range" range={range} note={note} h="100%">
            <Group gap="xl" mb="sm">
              <div>
                <Text size="xs" c="dimmed" tt="uppercase" style={{ letterSpacing: 0.4 }}>
                  Added · replaced
                </Text>
                <Churn added={s?.lines_added} removed={s?.lines_removed} unknown={s?.replaced_unknown} fz={30} />
              </div>
            </Group>
            <SimpleGrid cols={2} spacing="xs" mb="sm">
              <Counter label="Files touched" value={s?.files_touched} />
              <Counter label="Edits" value={s?.writes_window} />
              <Counter label="New files" value={s?.files_created} tip="a file whose oldest event in the range created it" />
              <Counter label="Existing files" value={s?.files_modified} />
            </SimpleGrid>
            {makeRatio != null && (
              <div>
                <Text size="xs" c="dimmed" mb={4}>
                  make ↔ inspect · {makeRatio}% writing
                </Text>
                <Meter
                  value={makeRatio}
                  color={OK}
                  tip={`${wr} write calls against ${rd} reads — Read/Grep/Glob, and shell calls that only look (cat, sed -n, grep, git log…)`}
                />
              </div>
            )}
          </Panel>
        </Grid.Col>
        <Grid.Col span={{ base: 12, lg: 4 }}>
          <Panel title="Languages" scope="range" range={range} h="100%">
            <BarList rows={s?.languages ?? []} mono empty="no files edited in this range" />
            <Text size="sm" fw={600} mt="md" mb="xs">
              Projects
            </Text>
            <BarList rows={s?.projects ?? []} color="var(--mantine-color-pelican-3)" mono />
          </Panel>
        </Grid.Col>
        <Grid.Col span={{ base: 12, lg: 4 }}>
          <Panel title="The work mix" scope="range" range={range} h="100%">
            <SimpleGrid cols={3} spacing="xs" mb="md">
              <Counter label="Tests" value={s?.tests_run} tone={OK} />
              <Counter label="Builds" value={s?.builds_run} />
              <Counter label="Git ops" value={s?.git_ops} />
              <Counter label="Subagents" value={s?.subagent_calls} />
              <Counter label="MCP" value={s?.mcp_calls} />
              <Counter label="Web" value={s?.web_calls} />
            </SimpleGrid>
            <Text size="sm" fw={600} mb="xs">
              Commands
            </Text>
            <BarList rows={s?.commands ?? []} color="var(--mantine-color-pelican-6)" mono empty="no shell calls in this range" />
          </Panel>
        </Grid.Col>
      </Grid>

      {/* Outcomes beside the effort (#381): what the range SHIPPED, and
          what the repository said those commits carried. */}
      <Grid mb="md" gap="md">
        <Grid.Col span={12}>
          <Panel
            title="Shipped"
            scope="range"
            range={range}
            note="commits, pushes and PRs read from the shell · lines as git reported them at commit"
            h="100%"
          >
            <SimpleGrid cols={{ base: 3, md: 6 }} spacing="xs">
              <Counter label="Commits" value={s?.commits} tone={OK} tip="git commit calls in the range" />
              <Counter label="Pushes" value={s?.pushes} tip="git push calls" />
              <Counter label="PRs opened" value={s?.prs_opened} tip="gh pr create calls" />
              <Counter label="PRs merged" value={s?.prs_merged} tone={OK} tip="gh pr merge calls" />
              <div>
                <Text size="xs" c="dimmed" tt="uppercase" style={{ letterSpacing: 0.4 }}>
                  Landed on main
                </Text>
                <Tooltip
                  label={`lines that landed on the main line of the ${s?.landed?.repos?.length ?? 0} repositor${(s?.landed?.repos?.length ?? 0) === 1 ? 'y' : 'ies'} the agents worked in, read with git — churn as the repository saw it, however the edits were made${n(s?.landed?.unreadable) > 0 ? ` · ${n(s?.landed?.unreadable)} working director${n(s?.landed?.unreadable) === 1 ? 'y' : 'ies'} could not be read as a repository` : ''}`}
                  withArrow
                  multiline
                  w={300}
                >
                  <span>
                    <Churn added={s?.landed?.added} removed={s?.landed?.removed} size="md" />
                  </span>
                </Tooltip>
              </div>
              <Counter label="Tokens / merged PR" value={tokensPerPr} tip="output tokens in the range ÷ PRs merged — the cost of a shipped unit" />
            </SimpleGrid>
            {(s?.landed?.repos?.length ?? 0) > 0 && (
              <Table.ScrollContainer minWidth={520} mt="sm">
                <Table striped verticalSpacing={2} fz="xs">
                  <Table.Thead>
                    <Table.Tr>
                      <Table.Th>Repository</Table.Th>
                      <Table.Th>Main line</Table.Th>
                      <Table.Th ta="right">Commits landed</Table.Th>
                      <Table.Th ta="right">Lines landed</Table.Th>
                      <Table.Th ta="right">Churn</Table.Th>
                    </Table.Tr>
                  </Table.Thead>
                  <Table.Tbody>
                    {(s?.landed?.repos ?? []).map((r) => {
                      const p = pct(n(r.removed), n(r.added) + n(r.removed))
                      return (
                        <Table.Tr key={r.name}>
                          <Table.Td>
                            <Text size="xs" ff={MONO}>
                              {r.name}
                            </Text>
                          </Table.Td>
                          <Table.Td>
                            <Text size="xs" c="dimmed" ff={MONO}>
                              {r.branch}
                            </Text>
                          </Table.Td>
                          <Table.Td ta="right">{String(r.commits)}</Table.Td>
                          <Table.Td ta="right">
                            <Churn added={r.added} removed={r.removed} size="xs" />
                          </Table.Td>
                          <Table.Td ta="right">
                            <Text size="xs" ff={MONO}>
                              {p == null ? '—' : `${p}%`}
                            </Text>
                          </Table.Td>
                        </Table.Tr>
                      )
                    })}
                  </Table.Tbody>
                </Table>
              </Table.ScrollContainer>
            )}
          </Panel>
        </Grid.Col>
      </Grid>

      <Grid mb="md" gap="md">
        <Grid.Col span={{ base: 12, lg: 8 }}>
          <Panel title="Code churn — added against replaced" scope="range" range={range} note={`${long ? 'per day' : 'per hour'} · solid: the transcript's edits · faint: what landed on main`} h="100%">
            <EChart
              height={210}
              option={{
                grid: { left: 52, right: 12, top: 18, bottom: 26 },
                tooltip: { ...TOOLTIP, trigger: 'axis' },
                legend: { data: ['added', 'replaced', 'landed +', 'landed −'], textStyle: { color: INK_MUTED }, right: 0, top: -2 },
                xAxis: {
                  type: 'category',
                  data: buckets.map((t) => (long ? t.slice(5, 10) : t.slice(11) + ':00')),
                  axisLabel: { color: AXIS.axisLabel.color },
                  axisLine: { lineStyle: { color: GRID_LINE } },
                },
                yAxis: { type: 'value', axisLabel: { color: AXIS.axisLabel.color }, splitLine: { lineStyle: { color: GRID_LINE } } },
                series: [
                  { name: 'added', type: 'bar', stack: 'churn', data: at(churnSeries, 'added'), itemStyle: { color: OK } },
                  // Below the axis so the two read as opposing forces.
                  { name: 'replaced', type: 'bar', stack: 'churn', data: at(churnSeries, 'removed').map((v) => -v), itemStyle: { color: FAIL } },
                  // The repository's account of the same hours, beside.
                  { name: 'landed +', type: 'bar', stack: 'landed', data: at(landedSeries, 'added'), itemStyle: { color: OK, opacity: 0.45 } },
                  { name: 'landed −', type: 'bar', stack: 'landed', data: at(landedSeries, 'removed').map((v) => -v), itemStyle: { color: FAIL, opacity: 0.45 } },
                ],
              }}
            />
          </Panel>
        </Grid.Col>
        <Grid.Col span={{ base: 12, lg: 4 }}>
          <Panel title="Churn ratio" scope="range" range={range} h="100%">
            <Group gap="sm" align="baseline">
              <Text fz={44} fw={700} ff={MONO} c={churnTone}>
                {shownPct == null ? '—' : `${shownPct}%`}
              </Text>
              <Text size="sm" c="dimmed">
                {churnRead}
              </Text>
            </Group>
            <Meter value={shownPct} color={churnTone} tip="replaced ÷ (added + replaced) — 0% is all new code; from the transcript when it can see what was replaced, else from what landed on main" />
            <SimpleGrid cols={2} spacing="xs" mt="md">
              <Counter
                label="Work undone"
                value={s?.reverts}
                tone={n(s?.reverts) > 0 ? FAIL : undefined}
                tip="edits whose work a later edit threw away — a flip-flop counts twice"
              />
              <Counter label="Thrash files" value={s?.thrash_files} tip={`files touched ${s?.revisit_at ?? 3} or more times in this range`} />
              <Counter label="Tokens / line" value={tokensPerLine} tip="output tokens spent per line of code that survived" />
              <Counter label="Tests / 100 lines" value={testsPer100 == null ? null : String(testsPer100)} />
            </SimpleGrid>
            {s?.top_repeat && (
              <Tooltip label="the same command line, over and over — the shape of fighting something" withArrow>
                <Group gap="xs" mt="md" wrap="nowrap">
                  <Badge color="orange" variant="light">
                    ×{String(s.top_repeat.value)}
                  </Badge>
                  <Text size="sm" ff={MONO} lineClamp={1}>
                    {s.top_repeat.name}
                  </Text>
                </Group>
              </Tooltip>
            )}
            <Group gap="lg" mt="md">
              <Counter label="Longest quiet" value={s ? `${s.longest_quiet_mins}m` : null} tip="the longest gap between captured messages in this range" />
              <Counter label="Repo switches" value={s?.repo_switches} tip="an agent crossing repos mid-session — thrash, when high" />
            </Group>
          </Panel>
        </Grid.Col>
      </Grid>

      <Grid mb="md" gap="md">
        <Grid.Col span={{ base: 12, lg: 7 }}>
          <Panel title="Hottest files" scope="range" range={range} note="most-touched paths" h="100%">
            <BarList rows={s?.files ?? []} mono shorten={baseName} />
          </Panel>
        </Grid.Col>
        <Grid.Col span={{ base: 12, lg: 5 }}>
          <Panel title="Where the work happened" scope="range" range={range} note="directories" h="100%">
            <BarList rows={s?.dirs ?? []} color="var(--mantine-color-pelican-3)" mono />
            <Group gap="xl" mt="md">
              <Counter label="Thinking tokens" value={s?.thinking_tokens} />
              <Counter label="Tool calls" value={s?.tools_window} />
              <Counter label="Output tokens" value={s?.out_tokens_window} />
            </Group>
          </Panel>
        </Grid.Col>
      </Grid>
    </>
  )
}
