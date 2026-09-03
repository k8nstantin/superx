import { Grid, Group, SimpleGrid, Table, Text } from '@mantine/core'
import type { StatsSummary } from '../../generated/StatsSummary'
import { AXIS, EChart, GRID_LINE, INK_MUTED, MONO, TOOLTIP } from '../../EChart'
import { BANDS, CANCEL, Counter, FAIL, Meter, OK, Panel, Stat, UNKNOWN, fmtAge, fmtCompact, fmtMs, n, pct, rangeLabel } from './parts'

// Did it hold (#367): what the commands reported, when it went wrong,
// why the rewrites happened, and what the agents waited on.

export function QualitySection({ s, range }: { s: StatsSummary | undefined; range: string | null }) {
  const note = rangeLabel(range, s?.window_messages)
  const passed = n(s?.tests_passed)
  const failed = n(s?.tests_failed)
  const passPct = pct(passed, passed + failed)

  const cd = n(s?.churn_directed)
  const cs = n(s?.churn_self)
  const directedPct = pct(cd, cd + cs)
  const churnCause =
    directedPct == null
      ? ''
      : directedPct >= BANDS.directedOk
        ? 'the design is moving'
        : directedPct <= BANDS.directedBad
          ? 'the agents are rewriting themselves'
          : 'mixed — some redirection, some rework'

  const outcomes = s?.tool_outcomes ?? []
  const byHour = s?.fail_by_hour ?? []
  const hours = Array.from({ length: 24 }, (_, h) => h)
  const hourRate = hours.map((h) => {
    const row = byHour.find((r) => n(r.hour) === h)
    return row ? pct(n(row.failures), n(row.calls)) : null
  })
  const hourCalls = hours.map((h) => n(byHour.find((r) => n(r.hour) === h)?.calls))

  return (
    <>
      <SimpleGrid cols={{ base: 2, md: 3, lg: 6 }} spacing="xs" mb="md">
        <Stat
          label="Tests passed"
          value={fmtCompact(s?.tests_passed)}
          sub={passPct == null ? 'no tally read' : `${passPct}% pass rate`}
          tip="read out of what the test runners printed"
          tone={passPct == null ? 'none' : passPct >= BANDS.passOk ? 'ok' : passPct >= BANDS.passBad ? 'warn' : 'bad'}
        />
        <Stat label="Tests failed" value={fmtCompact(s?.tests_failed)} tip="failing tests reported in tool output" tone={failed > 0 ? 'bad' : 'none'} />
        <Stat
          label="Compile errors"
          value={fmtCompact(s?.compile_errors)}
          tip="rustc / tsc diagnostics seen in tool output"
          tone={n(s?.compile_errors) > 0 ? 'bad' : 'none'}
        />
        <Stat
          label="Edit → verify"
          value={n(s?.edit_to_verify_p50_secs) > 0 ? fmtMs(n(s?.edit_to_verify_p50_secs) * 1000) : '—'}
          sub="median wait to check"
          tip="long, with high churn, is the signature of an agent guessing"
        />
        <Stat
          label="Code half-life"
          value={n(s?.survival_p50_mins) > 0 ? fmtMs(n(s?.survival_p50_mins) * 60_000) : '—'}
          sub="before it was rewritten"
          tip="minutes means thrash; hours means the design moved"
        />
        <Stat
          label="Compaction cost"
          value={fmtMs(s?.compaction_total_ms)}
          sub={s ? `${s.compactions} compaction(s)` : ''}
          tip="wall-clock the agents spent re-reading their own history after the context filled"
        />
      </SimpleGrid>

      <Grid mb="md" gap="md">
        <Grid.Col span={{ base: 12, lg: 8 }}>
          <Panel title="Quality over time" scope="range" range={range} note={`tests and tool failures per hour · ${note}`} h="100%">
            <EChart
              height={200}
              option={{
                grid: { left: 52, right: 12, top: 18, bottom: 26 },
                tooltip: { ...TOOLTIP, trigger: 'axis' },
                legend: { data: ['passed', 'failed', 'tool failures'], textStyle: { color: INK_MUTED }, right: 0, top: -2 },
                xAxis: {
                  type: 'category',
                  data: (s?.quality_series ?? []).map((p) => (range === '7d' || range === '30d' || range === 'all' ? p.t.slice(5) : p.t.slice(11) + ':00')),
                  axisLabel: { color: AXIS.axisLabel.color, fontSize: 10 },
                  axisLine: { lineStyle: { color: GRID_LINE } },
                },
                yAxis: { type: 'value', axisLabel: { color: AXIS.axisLabel.color }, splitLine: { lineStyle: { color: GRID_LINE } } },
                series: [
                  { name: 'passed', type: 'bar', stack: 'q', itemStyle: { color: OK }, data: (s?.quality_series ?? []).map((p) => n(p.tests_passed)) },
                  { name: 'failed', type: 'bar', stack: 'q', itemStyle: { color: FAIL }, data: (s?.quality_series ?? []).map((p) => n(p.tests_failed)) },
                  { name: 'tool failures', type: 'line', smooth: true, itemStyle: { color: CANCEL }, data: (s?.quality_series ?? []).map((p) => n(p.tool_failures)) },
                ],
              }}
            />
          </Panel>
        </Grid.Col>
        <Grid.Col span={{ base: 12, lg: 4 }}>
          <Panel title="Why the churn" scope="range" range={range} h="100%">
            <Text fz={30} fw={700} ff={MONO} c={directedPct == null ? undefined : directedPct <= BANDS.directedBad ? FAIL : directedPct >= BANDS.directedOk ? OK : CANCEL}>
              {directedPct == null ? '—' : `${directedPct}% directed`}
            </Text>
            <Text size="sm" c="dimmed" mb="xs">
              {churnCause}
            </Text>
            <Group gap={2} wrap="nowrap" mb="md" title="replaced lines that followed a human instruction, against those with nobody steering">
              <div style={{ width: `${directedPct ?? 0}%`, background: OK, height: 10, borderRadius: '3px 0 0 3px' }} />
              <div style={{ flex: 1, background: directedPct == null ? UNKNOWN : FAIL, height: 10, borderRadius: '0 3px 3px 0' }} />
            </Group>
            <SimpleGrid cols={2} spacing="xs">
              <Counter label="Directed" value={s?.churn_directed} tone={OK} tip="replaced lines that followed a human turn within ten minutes" />
              <Counter label="Self-inflicted" value={s?.churn_self} tone={FAIL} tip="replaced lines with nobody steering" />
            </SimpleGrid>
            <Text size="sm" fw={600} mt="md" mb="xs">
              Waiting on operations
            </Text>
            <SimpleGrid cols={2} spacing="xs">
              <Counter label="Total wait" value={fmtMs(s?.wait_ms_total)} tip="wall-clock the agents spent inside long operations, summed" />
              <Counter label="Median / p95" value={`${fmtMs(s?.wait_ms_median)} / ${fmtMs(s?.wait_ms_p95)}`} />
            </SimpleGrid>
            {(s?.slowest?.length ?? 0) > 0 && (
              <>
                <Text size="xs" c="dimmed" tt="uppercase" mt="sm" mb={4} style={{ letterSpacing: 0.4 }}>
                  Slowest
                </Text>
                {(s?.slowest ?? []).slice(0, 5).map((o, idx) => (
                  <Group key={`${o.at}-${idx}`} gap="xs" wrap="nowrap" justify="space-between">
                    <Text size="xs" ff={MONO} lineClamp={1}>
                      {o.label}
                    </Text>
                    <Text size="xs" c="orange.4" ff={MONO} style={{ whiteSpace: 'nowrap' }}>
                      {fmtMs(o.ms)}
                    </Text>
                  </Group>
                ))}
              </>
            )}
          </Panel>
        </Grid.Col>
      </Grid>

      <Grid mb="md" gap="md">
        <Grid.Col span={{ base: 12, lg: 7 }}>
          <Panel title="Did the calls work?" scope="range" range={range} note="by tool · ok, failed, cancelled, unresolved" h="100%">
            <EChart
              height={250}
              option={{
                tooltip: { ...TOOLTIP, trigger: 'axis', axisPointer: { type: 'shadow' } },
                legend: { data: ['ok', 'failed', 'cancelled', 'no result yet'], textStyle: { color: INK_MUTED, fontSize: 11 }, top: 0, right: 0 },
                grid: { left: 130, right: 20, top: 30, bottom: 24 },
                xAxis: { type: 'value', ...AXIS },
                yAxis: { type: 'category', data: outcomes.slice(0, 8).map((t) => t.name).reverse(), ...AXIS, splitLine: { show: false } },
                series: (
                  [
                    ['ok', OK, 'ok'],
                    ['failed', FAIL, 'failed'],
                    ['cancelled', CANCEL, 'cancelled'],
                    ['no result yet', UNKNOWN, 'unknown'],
                  ] as const
                ).map(([label, color, key]) => ({
                  name: label,
                  type: 'bar',
                  stack: 'calls',
                  barWidth: 14,
                  itemStyle: { color },
                  data: outcomes
                    .slice(0, 8)
                    .map((t) => n(t[key]))
                    .reverse(),
                })),
              }}
            />
          </Panel>
        </Grid.Col>
        <Grid.Col span={{ base: 12, lg: 5 }}>
          <Panel title="When it goes wrong" scope="range" range={range} note="failure rate by hour of day" h="100%">
            {byHour.length === 0 ? (
              <Text size="xs" c="dimmed">
                no tool calls in this range
              </Text>
            ) : (
              <EChart
                height={250}
                option={{
                  tooltip: {
                    ...TOOLTIP,
                    trigger: 'axis',
                    formatter: (params: { dataIndex: number }[]) => {
                      const h = params[0]?.dataIndex ?? 0
                      return `${String(h).padStart(2, '0')}:00<br/>${hourCalls[h]} calls · ${hourRate[h] == null ? '—' : `${hourRate[h]}% failed`}`
                    },
                  },
                  grid: { left: 40, right: 40, top: 18, bottom: 26 },
                  xAxis: { type: 'category', data: hours.map((h) => String(h).padStart(2, '0')), ...AXIS, axisLabel: { ...AXIS.axisLabel, fontSize: 9 }, splitLine: { show: false } },
                  yAxis: [
                    { type: 'value', name: 'calls', nameTextStyle: { color: INK_MUTED, fontSize: 9 }, ...AXIS },
                    { type: 'value', name: '% failed', max: 100, nameTextStyle: { color: INK_MUTED, fontSize: 9 }, ...AXIS, splitLine: { show: false } },
                  ],
                  series: [
                    { type: 'bar', name: 'calls', data: hourCalls, itemStyle: { color: 'var(--mantine-color-pelican-7)', borderRadius: 2 } },
                    { type: 'line', name: '% failed', yAxisIndex: 1, data: hourRate, smooth: true, connectNulls: false, itemStyle: { color: FAIL }, lineStyle: { color: FAIL, width: 2 }, symbolSize: 5 },
                  ],
                }}
              />
            )}
          </Panel>
        </Grid.Col>
      </Grid>

      <Panel title="Compaction" scope="range" range={range} note="dead time, per session" mb="md">
        {(s?.compaction_sessions?.length ?? 0) === 0 ? (
          <Text size="xs" c="dimmed">
            No session hit its context ceiling in this range.
          </Text>
        ) : (
          <Table striped>
            <Table.Thead>
              <Table.Tr>
                <Table.Th>Session</Table.Th>
                <Table.Th>Repo</Table.Th>
                <Table.Th ta="right">Times</Table.Th>
                <Table.Th ta="right">Lost</Table.Th>
                <Table.Th ta="right">Median</Table.Th>
                <Table.Th ta="right">Context at compaction</Table.Th>
              </Table.Tr>
            </Table.Thead>
            <Table.Tbody>
              {(s?.compaction_sessions ?? []).slice(0, 8).map((c) => (
                <Table.Tr key={c.identity}>
                  <Table.Td>
                    <Text size="xs" ff={MONO}>
                      {c.identity.slice(0, 20)}
                    </Text>
                  </Table.Td>
                  <Table.Td>
                    <Text size="xs" c="dimmed">
                      {c.repo ?? '—'}
                      {n(c.manual) > 0 ? ` · ${c.manual} manual` : ''}
                    </Text>
                  </Table.Td>
                  <Table.Td ta="right">{String(c.count)}</Table.Td>
                  <Table.Td ta="right" c="orange.4">
                    {fmtMs(c.total_ms)}
                  </Table.Td>
                  <Table.Td ta="right">{fmtMs(c.median_ms)}</Table.Td>
                  <Table.Td ta="right">
                    <Group gap={6} justify="flex-end" wrap="nowrap">
                      <div style={{ width: 60 }}>
                        <Meter value={Math.min(100, Math.round(n(c.pre_tokens_max) / 10_000))} color={CANCEL} />
                      </div>
                      <Text size="xs" ff={MONO}>
                        {fmtCompact(c.pre_tokens_max)}
                      </Text>
                    </Group>
                  </Table.Td>
                </Table.Tr>
              ))}
            </Table.Tbody>
          </Table>
        )}
        {(s?.compaction_sessions?.length ?? 0) > 0 && (
          <Text size="xs" c="dimmed" mt="xs">
            longest quiet stretch in range: {fmtAge(n(s?.longest_quiet_mins) * 60)}
          </Text>
        )}
      </Panel>
    </>
  )
}
