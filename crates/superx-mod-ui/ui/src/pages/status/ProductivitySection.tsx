import { Grid, Group, SimpleGrid, Table, Text, Tooltip } from '@mantine/core'
import type { StatsSummary } from '../../generated/StatsSummary'
import { AXIS, CHART_COLORS, EChart, GRID_LINE, INK_MUTED, MONO, TOOLTIP } from '../../EChart'
import { CANCEL, Counter, FAIL, OK, Panel, Stat, fmtCompact, n, pct } from './parts'

// Productivity (#391). Every other band counts effort — lines, tokens,
// calls, tests. This one divides effort by outcome, and measures the
// human half of the loop: how long an agent flew before it needed you,
// how much of your attention the work took, and how much of the spend
// happened with nobody watching.
//
// The denominators are the honest ones. Lines LANDED on the main line
// come from the repositories themselves (#387), so they survive an
// operating mode that edits through the shell and leaves the
// transcript's own line counts blind (#383). Lines WRITTEN do not,
// which is why written-against-landed is shown as a pair to read, not
// as a ratio to trust.

export function ProductivitySection({ s, range }: { s: StatsSummary | undefined; range: string | null }) {
  const long = range === '7d' || range === '30d' || range === 'all'
  const out = n(s?.out_tokens_window)
  const landed = n(s?.landed?.added)
  const written = n(s?.lines_added)
  const merged = n(s?.prs_merged)
  const hours = n(s?.active_hours_24h)

  const perLandedLine = landed > 0 ? Math.round(out / landed) : null
  const perMergedPr = merged > 0 ? Math.round(out / merged) : null
  const landedPerHour = hours > 0 ? Math.round(landed / hours) : null
  const thinkingShare = pct(n(s?.thinking_tokens), out)
  const unattended = pct(n(s?.unattended_out_tokens), out)
  const turns = n(s?.human_turns)
  const turnsPerHour = hours > 0 ? Math.round((turns * 10) / hours) / 10 : null
  const autonomy = n(s?.autonomy_p50_mins)

  // Burn over time, stacked by repo. `work_cells` has carried output
  // tokens per agent × repo × bucket since #340 and nothing ever drew
  // them — no backend work was needed for this chart.
  const cells = s?.work_cells ?? []
  const buckets = [...new Set(cells.map((c) => c.t))].sort()
  const repos = [...new Set(cells.map((c) => c.repo))]
    .map((r) => ({ repo: r, total: cells.filter((c) => c.repo === r).reduce((a, c) => a + n(c.out_tokens), 0) }))
    .sort((a, b) => b.total - a.total)
    .slice(0, 6)
    .map((r) => r.repo)
  const byRepo = repos.map((r, i) => ({
    name: r,
    type: 'bar' as const,
    stack: 'burn',
    itemStyle: { color: CHART_COLORS[i % CHART_COLORS.length] },
    data: buckets.map((b) =>
      cells.filter((c) => c.t === b && c.repo === r).reduce((a, c) => a + n(c.out_tokens), 0),
    ),
  }))
  const label = (t: string) => (long ? t.slice(5, 10) : t.length > 10 ? t.slice(11) + ':00' : t)

  // The token mix, in the same buckets: what was sent, what came back
  // out of the vendor's cache, what the models produced, and how much
  // of that production was reasoning.
  const burn = s?.burn ?? []
  const mixBuckets = burn.map((p) => p.t)

  const pairs = s?.model_effort ?? []
  const biggest = pairs.reduce((a, p) => Math.max(a, n(p.messages)), 0)

  return (
    <>
      <Grid mb="md" gap="md">
        <Grid.Col span={12}>
          <Panel
            title="What the work cost, and what came back"
            scope="range"
            range={range}
            note="tokens over outcomes — lines that landed on main, and pull requests that merged"
            h="100%"
          >
            <SimpleGrid cols={{ base: 2, md: 3, lg: 6 }} spacing="xs">
              <Stat
                label="Per landed line"
                value={perLandedLine == null ? '—' : `${fmtCompact(perLandedLine)} tok`}
                sub={`${fmtCompact(landed)} landed`}
                tip="output tokens in the range ÷ lines that landed on the repositories' main lines. The denominator comes from git, so it holds however the edits were made."
              />
              <Stat
                label="Per merged PR"
                value={perMergedPr == null ? '—' : `${fmtCompact(perMergedPr)} tok`}
                sub={`${merged} merged`}
                tip="output tokens ÷ pull requests merged — the price of a shipped unit"
              />
              <Stat
                label="Landed per hour"
                value={landedPerHour == null ? '—' : fmtCompact(landedPerHour)}
                sub={`over ${hours} active hour${hours === 1 ? '' : 's'}`}
                tip="lines landed on main ÷ hours in which anything happened"
              />
              <Stat
                label="Written · landed"
                value={`${fmtCompact(written)} · ${fmtCompact(landed)}`}
                sub={n(s?.replaced_unknown) > 0 ? 'written undercounts shell edits' : 'both from the same range'}
                tip="lines the transcript saw written, beside lines the repositories say landed. A shell edit carries only what it wrote, so the left number undercounts — read them as a pair, not as a ratio."
              />
              <Stat
                label="Thinking share"
                value={thinkingShare == null ? '—' : `${thinkingShare}%`}
                sub={`${fmtCompact(n(s?.thinking_tokens))} reasoning`}
                tip="share of output tokens spent reasoning rather than answering"
              />
              <Stat
                label="Unattended burn"
                value={unattended == null ? '—' : `${unattended}%`}
                sub={`${fmtCompact(n(s?.unattended_out_tokens))} unsupervised`}
                tone={unattended != null && unattended >= 80 ? 'warn' : undefined}
                tip="share of output tokens spent on a message with no human turn in the ten minutes before it — neither good nor bad on its own, but it says how much of the spend flew alone"
              />
            </SimpleGrid>
          </Panel>
        </Grid.Col>
      </Grid>

      <Grid mb="md" gap="md">
        <Grid.Col span={{ base: 12, lg: 8 }}>
          <Panel
            title="Burn over time — output tokens by repository"
            scope="range"
            range={range}
            note={long ? 'per day' : 'per hour'}
            h="100%"
          >
            {buckets.length === 0 ? (
              <Text size="xs" c="dimmed">
                nothing spent in this range
              </Text>
            ) : (
              <EChart
                height={220}
                option={{
                  grid: { left: 58, right: 12, top: 18, bottom: 26 },
                  tooltip: { ...TOOLTIP, trigger: 'axis' },
                  legend: { data: repos, textStyle: { color: INK_MUTED }, right: 0, top: -2 },
                  xAxis: {
                    type: 'category',
                    data: buckets.map(label),
                    axisLabel: { color: AXIS.axisLabel.color },
                    axisLine: { lineStyle: { color: GRID_LINE } },
                  },
                  yAxis: {
                    type: 'value',
                    axisLabel: { color: AXIS.axisLabel.color, formatter: (v: number) => fmtCompact(v) },
                    splitLine: { lineStyle: { color: GRID_LINE } },
                  },
                  series: byRepo,
                }}
              />
            )}
          </Panel>
        </Grid.Col>
        <Grid.Col span={{ base: 12, lg: 4 }}>
          <Panel title="The token mix, over the same hours" scope="range" range={range} h="100%">
            {mixBuckets.length === 0 ? (
              <Text size="xs" c="dimmed">
                nothing spent in this range
              </Text>
            ) : (
              <EChart
                height={220}
                option={{
                  grid: { left: 58, right: 12, top: 18, bottom: 26 },
                  tooltip: { ...TOOLTIP, trigger: 'axis' },
                  legend: { data: ['sent', 'from cache', 'produced', 'reasoning'], textStyle: { color: INK_MUTED }, right: 0, top: -2 },
                  xAxis: {
                    type: 'category',
                    data: mixBuckets.map(label),
                    axisLabel: { color: AXIS.axisLabel.color },
                    axisLine: { lineStyle: { color: GRID_LINE } },
                  },
                  yAxis: {
                    type: 'value',
                    axisLabel: { color: AXIS.axisLabel.color, formatter: (v: number) => fmtCompact(v) },
                    splitLine: { lineStyle: { color: GRID_LINE } },
                  },
                  series: [
                    { name: 'sent', type: 'bar', stack: 'mix', itemStyle: { color: CANCEL }, data: burn.map((p) => n(p.input)) },
                    { name: 'from cache', type: 'bar', stack: 'mix', itemStyle: { color: CHART_COLORS[3] }, data: burn.map((p) => n(p.cache_read)) },
                    { name: 'produced', type: 'bar', stack: 'mix', itemStyle: { color: OK }, data: burn.map((p) => n(p.out)) },
                    // Reasoning is part of what was produced, so it is a
                    // line over the stack rather than another slice of it.
                    { name: 'reasoning', type: 'line', smooth: true, symbol: 'none', itemStyle: { color: FAIL }, data: burn.map((p) => n(p.thinking)) },
                  ],
                }}
              />
            )}
          </Panel>
        </Grid.Col>
      </Grid>

      <Panel
        title="Where the burn went — repository against what it produced"
        scope="range"
        range={range}
        note="one working directory per row · biggest spender first"
        mb="md"
      >
        {(s?.repos?.length ?? 0) === 0 ? (
          <Text size="xs" c="dimmed">
            no repository was worked in this range.
          </Text>
        ) : (
          <Table.ScrollContainer minWidth={920}>
            <Table striped highlightOnHover>
              <Table.Thead>
                <Table.Tr>
                  <Table.Th>Repository</Table.Th>
                  <Table.Th ta="right">Out tokens</Table.Th>
                  <Table.Th ta="right">Share of burn</Table.Th>
                  <Table.Th ta="right">Lines +/−</Table.Th>
                  <Table.Th ta="right">Landed +/−</Table.Th>
                  <Table.Th ta="right">Tok / line</Table.Th>
                  <Table.Th ta="right">Files</Table.Th>
                  <Table.Th ta="right">Tests</Table.Th>
                  <Table.Th ta="right">Fails</Table.Th>
                </Table.Tr>
              </Table.Thead>
              <Table.Tbody>
                {[...(s?.repos ?? [])]
                  .sort((a, b) => n(b.out_tokens) - n(a.out_tokens))
                  .map((r) => {
                    const share = pct(n(r.out_tokens), out)
                    // Landed is keyed by the repository, a worktree's rows
                    // fold into it (#386), so it only matches where the
                    // working directory IS the repository.
                    const land = (s?.landed?.repos ?? []).find((l) => l.name === r.name)
                    // Tokens per line uses whatever the repository landed
                    // when it can, and falls back to what was written.
                    const denom = land ? n(land.added) : n(r.lines_added)
                    const perLine = denom > 0 ? Math.round(n(r.out_tokens) / denom) : null
                    return (
                      <Table.Tr key={r.name}>
                        <Table.Td>
                          <Text size="xs" ff={MONO}>
                            {r.name}
                          </Text>
                        </Table.Td>
                        <Table.Td ta="right">{fmtCompact(n(r.out_tokens))}</Table.Td>
                        <Table.Td ta="right">
                          <Group gap={6} justify="flex-end" wrap="nowrap">
                            <div style={{ width: 56, height: 6, background: GRID_LINE, borderRadius: 3 }}>
                              <div
                                style={{
                                  width: `${share ?? 0}%`,
                                  height: 6,
                                  background: CHART_COLORS[0],
                                  borderRadius: 3,
                                }}
                              />
                            </div>
                            <Text size="xs" ff={MONO} w={34} ta="right">
                              {share == null ? '—' : `${share}%`}
                            </Text>
                          </Group>
                        </Table.Td>
                        <Table.Td ta="right">
                          <Text size="xs" ff={MONO}>
                            +{fmtCompact(n(r.lines_added))} −{fmtCompact(n(r.lines_removed))}
                          </Text>
                        </Table.Td>
                        <Table.Td ta="right">
                          {land ? (
                            <Text size="xs" ff={MONO}>
                              +{fmtCompact(n(land.added))} −{fmtCompact(n(land.removed))}
                            </Text>
                          ) : (
                            <Tooltip label="this working directory is a worktree; its landed lines are counted under its repository" withArrow>
                              <Text size="xs" c="dimmed">
                                —
                              </Text>
                            </Tooltip>
                          )}
                        </Table.Td>
                        <Table.Td ta="right">
                          <Text size="xs" ff={MONO} c={land ? undefined : 'dimmed'}>
                            {perLine == null ? '—' : fmtCompact(perLine)}
                          </Text>
                        </Table.Td>
                        <Table.Td ta="right">{n(r.files_touched)}</Table.Td>
                        <Table.Td ta="right">{n(r.tests_run)}</Table.Td>
                        <Table.Td ta="right">
                          <Text size="xs" c={n(r.tool_failures) > 0 ? FAIL : undefined}>
                            {n(r.tool_failures)}
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

      <Panel
        title="Does thinking harder pay? — model against reasoning level"
        scope="range"
        range={range}
        note="the pair you actually switch · sample size beside every figure"
        mb="md"
      >
        {pairs.length === 0 ? (
          <Text size="xs" c="dimmed">
            no message in this range named both a model and a reasoning level.
          </Text>
        ) : (
          <>
            <Table.ScrollContainer minWidth={1060}>
              <Table striped highlightOnHover>
                <Table.Thead>
                  <Table.Tr>
                    <Table.Th>Model · level</Table.Th>
                    <Table.Th ta="right">
                      <Tooltip label="sample size — sessions and messages behind this row" withArrow>
                        <span>Sessions · msgs</span>
                      </Tooltip>
                    </Table.Th>
                    <Table.Th ta="right">Out tokens</Table.Th>
                    <Table.Th ta="right">Reasoning</Table.Th>
                    <Table.Th ta="right">Lines +/−</Table.Th>
                    <Table.Th ta="right">Tok / line</Table.Th>
                    <Table.Th ta="right">Tests</Table.Th>
                    <Table.Th ta="right">Tool fails</Table.Th>
                    <Table.Th ta="right">Unasked</Table.Th>
                    <Table.Th ta="right">Stepped in</Table.Th>
                  </Table.Tr>
                </Table.Thead>
                <Table.Tbody>
                  {pairs.map((p) => {
                    const tests = n(p.tests_passed) + n(p.tests_failed)
                    const passPct = pct(n(p.tests_passed), tests)
                    const failRate = pct(n(p.tool_failures), n(p.tool_calls))
                    const unasked = pct(n(p.edits_self), n(p.edits_directed) + n(p.edits_self))
                    const perLine = n(p.lines_added) > 0 ? Math.round(n(p.out_tokens) / n(p.lines_added)) : null
                    const think = pct(n(p.thinking_tokens), n(p.out_tokens))
                    // A row from a tenth of the biggest sample cannot be
                    // compared with it; say so rather than let it read as
                    // a result.
                    const thin = biggest > 0 && n(p.messages) * 10 < biggest
                    return (
                      <Table.Tr key={`${p.model}/${p.effort}`} style={thin ? { opacity: 0.55 } : undefined}>
                        <Table.Td>
                          <Group gap={6} wrap="nowrap">
                            <Text size="xs" ff={MONO}>
                              {p.model}
                            </Text>
                            <Text size="xs" c="dimmed" ff={MONO}>
                              {p.effort}
                            </Text>
                          </Group>
                        </Table.Td>
                        <Table.Td ta="right">
                          <Text size="xs" ff={MONO} c={thin ? 'orange.4' : undefined}>
                            {p.sessions} · {fmtCompact(n(p.messages))}
                          </Text>
                        </Table.Td>
                        <Table.Td ta="right">{fmtCompact(n(p.out_tokens))}</Table.Td>
                        <Table.Td ta="right">{think == null ? '—' : `${think}%`}</Table.Td>
                        <Table.Td ta="right">
                          <Text size="xs" ff={MONO}>
                            +{fmtCompact(n(p.lines_added))} −{fmtCompact(n(p.lines_removed))}
                          </Text>
                        </Table.Td>
                        <Table.Td ta="right">{perLine == null ? '—' : fmtCompact(perLine)}</Table.Td>
                        <Table.Td ta="right">{tests === 0 ? '—' : `${passPct}%`}</Table.Td>
                        <Table.Td ta="right">{n(p.tool_calls) === 0 ? '—' : `${failRate}%`}</Table.Td>
                        <Table.Td ta="right">{n(p.edits_directed) + n(p.edits_self) === 0 ? '—' : `${unasked}%`}</Table.Td>
                        <Table.Td ta="right">{n(p.interventions) + n(p.denials)}</Table.Td>
                      </Table.Tr>
                    )
                  })}
                </Table.Tbody>
              </Table>
            </Table.ScrollContainer>
            <Text size="xs" c="dimmed" mt="xs">
              Read this as evidence, not as a verdict. Rows can differ because the level differed, or because the work
              did — a session reviewing a data lake and one editing this dashboard are not the same task. Lines written
              undercount an agent that edits through the shell (#383), so tokens per line compares workflows as much as
              models. A faded row has less than a tenth of the biggest sample here.
            </Text>
          </>
        )}
      </Panel>

      <Panel
        title="The human half of the loop"
        scope="range"
        range={range}
        note="how much steering the work took"
      >
        <SimpleGrid cols={{ base: 2, md: 4 }} spacing="xs">
          <Counter
            label="Your turns"
            value={turns}
            tip="messages you sent in this range"
          />
          <Counter
            label="Turns per hour"
            value={turnsPerHour == null ? '—' : turnsPerHour}
            tip="your turns ÷ hours in which anything happened — how much of your attention the work consumed"
          />
          <Counter
            label="Autonomy span"
            value={autonomy === 0 ? '—' : `${autonomy}m`}
            tip="median minutes from one of your turns to the next, within a session — how long an agent flew before it needed you. A dash means no session had two turns in this range."
          />
          <Counter
            label="Stepped in"
            value={n(s?.interventions) + n(s?.denials)}
            tone={n(s?.interventions) + n(s?.denials) > 0 ? CANCEL : undefined}
            tip="interruptions and refusals — the times you had to stop something"
          />
        </SimpleGrid>
      </Panel>
    </>
  )
}
