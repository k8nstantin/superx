import { Badge, Grid, Group, SimpleGrid, Text, Tooltip } from '@mantine/core'
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
  const churnPct = pct(replaced, added + replaced)
  const churnTone = churnPct == null ? undefined : churnPct >= BANDS.churnBad ? FAIL : churnPct >= BANDS.churnOk ? CANCEL : OK
  const churnRead =
    churnPct == null ? '' : churnPct >= BANDS.churnBad ? 'mostly rewriting' : churnPct >= BANDS.churnOk ? 'revising as it goes' : 'mostly new code'
  const tokensPerLine = s && added > 0 ? Math.round(n(s.out_tokens_window) / added) : null
  const testsPer100 = s && added > 0 ? Math.round((n(s.tests_run) * 100 * 10) / added) / 10 : null

  const wr = n(s?.writes_window)
  const rd = n(s?.reads_window)
  const makeRatio = pct(wr, wr + rd)

  // A 30-day range is ~720 hourly points: unreadable. Fold to days.
  const churnSeries = (() => {
    const pts = s?.churn ?? []
    if (!long) return pts
    const byDay = new Map<string, { t: string; added: number; removed: number }>()
    for (const p of pts) {
      const day = p.t.slice(0, 10)
      const cur = byDay.get(day) ?? { t: day, added: 0, removed: 0 }
      cur.added += n(p.added)
      cur.removed += n(p.removed)
      byDay.set(day, cur)
    }
    return [...byDay.values()].sort((a, b) => a.t.localeCompare(b.t))
  })()

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
                <Churn added={s?.lines_added} removed={s?.lines_removed} fz={30} />
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

      <Grid mb="md" gap="md">
        <Grid.Col span={{ base: 12, lg: 8 }}>
          <Panel title="Code churn — added against replaced" scope="range" range={range} note={long ? 'per day' : 'per hour'} h="100%">
            <EChart
              height={210}
              option={{
                grid: { left: 52, right: 12, top: 18, bottom: 26 },
                tooltip: { ...TOOLTIP, trigger: 'axis' },
                legend: { data: ['added', 'replaced'], textStyle: { color: INK_MUTED }, right: 0, top: -2 },
                xAxis: {
                  type: 'category',
                  data: churnSeries.map((p) => (long ? p.t.slice(5, 10) : p.t.slice(11) + ':00')),
                  axisLabel: { color: AXIS.axisLabel.color },
                  axisLine: { lineStyle: { color: GRID_LINE } },
                },
                yAxis: { type: 'value', axisLabel: { color: AXIS.axisLabel.color }, splitLine: { lineStyle: { color: GRID_LINE } } },
                series: [
                  { name: 'added', type: 'bar', stack: 'churn', data: churnSeries.map((p) => n(p.added)), itemStyle: { color: OK } },
                  // Below the axis so the two read as opposing forces.
                  { name: 'replaced', type: 'bar', stack: 'churn', data: churnSeries.map((p) => -n(p.removed)), itemStyle: { color: FAIL } },
                ],
              }}
            />
          </Panel>
        </Grid.Col>
        <Grid.Col span={{ base: 12, lg: 4 }}>
          <Panel title="Churn ratio" scope="range" range={range} h="100%">
            <Group gap="sm" align="baseline">
              <Text fz={44} fw={700} ff={MONO} c={churnTone}>
                {churnPct == null ? '—' : `${churnPct}%`}
              </Text>
              <Text size="sm" c="dimmed">
                {churnRead}
              </Text>
            </Group>
            <Meter value={churnPct} color={churnTone} tip="replaced ÷ (added + replaced) — 0% is all new code" />
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
