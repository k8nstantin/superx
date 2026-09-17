import { Group, Table, Text, Tooltip } from '@mantine/core'
import type { StatsSummary } from '../../generated/StatsSummary'
import { AXIS, CHART_COLORS, EChart, GRID_LINE, INK_MUTED, MONO, TOOLTIP } from '../../EChart'
import { CANCEL, FAIL, OK, Panel, fmtCompact, n } from './parts'

// Is one model actually better than another (#403)?
//
// The operator has a feeling and wants the data to confirm or refute
// it. That means two honesties the page has not needed before.
//
// First, a rate without its denominator cannot be argued with. Every
// figure here carries a 95% Wilson interval computed from the counts,
// and where two models' intervals overlap the panel says the
// difference is not separable rather than ranking them anyway.
//
// Second, models do different work at different times. Pooled across
// everything, a comparison compares tasks as much as models — so the
// same-repository table is offered as the closest thing to like for
// like, and the series over time exists because within one model the
// day-to-day swing turned out to be larger than any gap between two of
// them.

/** A proportion with its 95% Wilson interval, in percent. */
function wilson(k: number, total: number): { p: number; lo: number; hi: number } | null {
  if (total <= 0) return null
  const z = 1.96
  const p = k / total
  const d = 1 + (z * z) / total
  const centre = (p + (z * z) / (2 * total)) / d
  const half = (z * Math.sqrt((p * (1 - p)) / total + (z * z) / (4 * total * total))) / d
  return { p: 100 * p, lo: 100 * Math.max(0, centre - half), hi: 100 * Math.min(1, centre + half) }
}

const overlap = (a: { lo: number; hi: number }, b: { lo: number; hi: number }) => a.lo <= b.hi && b.lo <= a.hi

/** Below this a point says nothing, so it is not drawn or ranked. */
const MIN_CALLS = 150

function band(v: { p: number; lo: number; hi: number } | null): string {
  return v == null ? '—' : `${v.p.toFixed(2)}% [${v.lo.toFixed(2)}–${v.hi.toFixed(2)}]`
}

export function ModelQuality({ s, range }: { s: StatsSummary | undefined; range: string | null }) {
  const pts = s?.model_quality ?? []
  const models = [...new Set(pts.map((p) => p.model))]
    .map((m) => ({
      model: m,
      calls: pts.filter((p) => p.model === m).reduce((a, p) => a + n(p.tool_calls), 0),
      fails: pts.filter((p) => p.model === m).reduce((a, p) => a + n(p.tool_failures), 0),
      passed: pts.filter((p) => p.model === m).reduce((a, p) => a + n(p.tests_passed), 0),
      failed: pts.filter((p) => p.model === m).reduce((a, p) => a + n(p.tests_failed), 0),
      msgs: pts.filter((p) => p.model === m).reduce((a, p) => a + n(p.messages), 0),
      stepped: pts.filter((p) => p.model === m).reduce((a, p) => a + n(p.interventions) + n(p.denials), 0),
      tokens: pts.filter((p) => p.model === m).reduce((a, p) => a + n(p.out_tokens), 0),
      lines: pts.filter((p) => p.model === m).reduce((a, p) => a + n(p.lines_added), 0),
    }))
    .sort((a, b) => b.calls - a.calls)

  // The verdict: for every pair with a sample worth comparing, do the
  // intervals separate?
  const verdicts: string[] = []
  const ranked = models.filter((m) => m.calls >= MIN_CALLS)
  for (let i = 0; i < ranked.length; i += 1) {
    for (let j = i + 1; j < ranked.length; j += 1) {
      const a = ranked[i]
      const b = ranked[j]
      const fa = wilson(a.fails, a.calls)
      const fb = wilson(b.fails, b.calls)
      const ta = wilson(a.passed, a.passed + a.failed)
      const tb = wilson(b.passed, b.passed + b.failed)
      const parts: string[] = []
      if (fa && fb) parts.push(overlap(fa, fb) ? 'tool failures: not separable' : `tool failures: ${fa.p < fb.p ? a.model : b.model} is lower`)
      if (ta && tb) parts.push(overlap(ta, tb) ? 'tests: not separable' : `tests: ${ta.p > tb.p ? a.model : b.model} passes more`)
      if (parts.length) verdicts.push(`${a.model} against ${b.model} — ${parts.join(' · ')}`)
    }
  }

  // Over time: one series per model, only where the sample carries it.
  const buckets = [...new Set(pts.filter((p) => n(p.tool_calls) >= MIN_CALLS).map((p) => p.t))].sort()
  const series = models
    .filter((m) => m.calls >= MIN_CALLS)
    .map((m, i) => ({
      name: m.model,
      type: 'line' as const,
      connectNulls: true,
      symbol: 'circle',
      symbolSize: 6,
      itemStyle: { color: CHART_COLORS[i % CHART_COLORS.length] },
      data: buckets.map((b) => {
        const p = pts.find((x) => x.model === m.model && x.t === b)
        if (!p || n(p.tool_calls) < MIN_CALLS) return null
        return Math.round((n(p.tool_failures) * 10000) / n(p.tool_calls)) / 100
      }),
    }))

  // Same repository, so the work is roughly held constant.
  const repoRows = s?.model_repos ?? []
  const sharedRepos = [...new Set(repoRows.map((r) => r.repo))].filter(
    (r) => repoRows.filter((x) => x.repo === r && n(x.tool_calls) >= MIN_CALLS).length >= 2,
  )

  return (
    <>
      <Panel
        title="Is one model better? — and is the difference real"
        scope="range"
        range={range}
        note="every rate with its 95% interval · a difference only counts when the intervals do not overlap"
        mb="md"
      >
        {models.length === 0 ? (
          <Text size="xs" c="dimmed">
            no message in this range named a model.
          </Text>
        ) : (
          <>
            <Table.ScrollContainer minWidth={900}>
              <Table striped highlightOnHover>
                <Table.Thead>
                  <Table.Tr>
                    <Table.Th>Model</Table.Th>
                    <Table.Th ta="right">Messages</Table.Th>
                    <Table.Th ta="right">Tool calls</Table.Th>
                    <Table.Th ta="right">Tool failure rate</Table.Th>
                    <Table.Th ta="right">Tests</Table.Th>
                    <Table.Th ta="right">Test pass rate</Table.Th>
                    <Table.Th ta="right">Stepped in / 1k msgs</Table.Th>
                    <Table.Th ta="right">Out tokens</Table.Th>
                  </Table.Tr>
                </Table.Thead>
                <Table.Tbody>
                  {models.map((m) => {
                    const f = wilson(m.fails, m.calls)
                    const t = wilson(m.passed, m.passed + m.failed)
                    const thin = m.calls < MIN_CALLS
                    return (
                      <Table.Tr key={m.model} style={thin ? { opacity: 0.55 } : undefined}>
                        <Table.Td>
                          <Group gap={6} wrap="nowrap">
                            <Text size="xs" ff={MONO}>
                              {m.model}
                            </Text>
                            {thin && (
                              <Tooltip label={`fewer than ${MIN_CALLS} tool calls — too thin to rank`} withArrow>
                                <Text size="xs" c="orange.4">
                                  thin
                                </Text>
                              </Tooltip>
                            )}
                          </Group>
                        </Table.Td>
                        <Table.Td ta="right">{fmtCompact(m.msgs)}</Table.Td>
                        <Table.Td ta="right">{fmtCompact(m.calls)}</Table.Td>
                        <Table.Td ta="right">
                          <Text size="xs" ff={MONO}>
                            {band(f)}
                          </Text>
                        </Table.Td>
                        <Table.Td ta="right">{m.passed + m.failed === 0 ? '—' : fmtCompact(m.passed + m.failed)}</Table.Td>
                        <Table.Td ta="right">
                          <Text size="xs" ff={MONO}>
                            {band(t)}
                          </Text>
                        </Table.Td>
                        <Table.Td ta="right">{m.msgs === 0 ? '—' : (Math.round((m.stepped * 10000) / m.msgs) / 10).toFixed(1)}</Table.Td>
                        <Table.Td ta="right">{fmtCompact(m.tokens)}</Table.Td>
                      </Table.Tr>
                    )
                  })}
                </Table.Tbody>
              </Table>
            </Table.ScrollContainer>
            <Text size="sm" fw={600} mt="md" mb={4}>
              The verdict
            </Text>
            {verdicts.length === 0 ? (
              <Text size="xs" c="dimmed">
                only one model has a sample worth comparing in this range.
              </Text>
            ) : (
              verdicts.map((v) => (
                <Text key={v} size="xs" c={v.includes('not separable') ? 'dimmed' : CANCEL} ff={MONO}>
                  {v}
                </Text>
              ))
            )}
            <Text size="xs" c="dimmed" mt="xs">
              These are hygiene measures: whether calls worked and whether tests passed. They do not say whether the
              work was the right work. A session can score perfectly here and still be thrown away — one in this
              instance was — so read this beside what landed, not instead of it.
            </Text>
          </>
        )}
      </Panel>

      <Panel
        title="Model quality over time"
        scope="range"
        range={range}
        note={`tool failure rate per bucket · only where at least ${MIN_CALLS} calls back it`}
        mb="md"
      >
        {buckets.length === 0 ? (
          <Text size="xs" c="dimmed">
            no bucket in this range carries enough calls to say anything.
          </Text>
        ) : (
          <EChart
            height={220}
            option={{
              grid: { left: 52, right: 12, top: 18, bottom: 26 },
              tooltip: { ...TOOLTIP, trigger: 'axis', valueFormatter: (v: number) => (v == null ? '—' : `${v}%`) },
              legend: { data: series.map((x) => x.name), textStyle: { color: INK_MUTED }, right: 0, top: -2 },
              xAxis: {
                type: 'category',
                data: buckets.map((b) => (b.length > 10 ? b.slice(5) : b.slice(5))),
                axisLabel: { color: AXIS.axisLabel.color },
                axisLine: { lineStyle: { color: GRID_LINE } },
              },
              yAxis: {
                type: 'value',
                axisLabel: { color: AXIS.axisLabel.color, formatter: '{value}%' },
                splitLine: { lineStyle: { color: GRID_LINE } },
              },
              series,
            }}
          />
        )}
      </Panel>

      <Panel
        title="Same repository, side by side"
        scope="range"
        range={range}
        note="the closest the transcript gets to like for like — the work is held roughly constant"
      >
        {sharedRepos.length === 0 ? (
          <Text size="xs" c="dimmed">
            no repository in this range was worked by two models with a sample worth comparing.
          </Text>
        ) : (
          <Table.ScrollContainer minWidth={760}>
            <Table striped>
              <Table.Thead>
                <Table.Tr>
                  <Table.Th>Repository</Table.Th>
                  <Table.Th>Model</Table.Th>
                  <Table.Th ta="right">Tool calls</Table.Th>
                  <Table.Th ta="right">Tool failure rate</Table.Th>
                  <Table.Th ta="right">Test pass rate</Table.Th>
                  <Table.Th ta="right">Lines</Table.Th>
                  <Table.Th ta="right">Out tokens</Table.Th>
                </Table.Tr>
              </Table.Thead>
              <Table.Tbody>
                {sharedRepos.flatMap((repo) =>
                  repoRows
                    .filter((r) => r.repo === repo && n(r.tool_calls) >= MIN_CALLS)
                    .sort((a, b) => n(b.tool_calls) - n(a.tool_calls))
                    .map((r, idx) => {
                      const f = wilson(n(r.tool_failures), n(r.tool_calls))
                      const t = wilson(n(r.tests_passed), n(r.tests_passed) + n(r.tests_failed))
                      return (
                        <Table.Tr key={`${repo}/${r.model}`}>
                          <Table.Td>
                            <Text size="xs" ff={MONO} c={idx === 0 ? undefined : 'dimmed'}>
                              {idx === 0 ? repo : ''}
                            </Text>
                          </Table.Td>
                          <Table.Td>
                            <Text size="xs" ff={MONO}>
                              {r.model}
                            </Text>
                          </Table.Td>
                          <Table.Td ta="right">{fmtCompact(r.tool_calls)}</Table.Td>
                          <Table.Td ta="right">
                            <Text size="xs" ff={MONO} c={f && f.p > 5 ? FAIL : f && f.p < 2 ? OK : undefined}>
                              {band(f)}
                            </Text>
                          </Table.Td>
                          <Table.Td ta="right">
                            <Text size="xs" ff={MONO}>
                              {band(t)}
                            </Text>
                          </Table.Td>
                          <Table.Td ta="right">{fmtCompact(r.lines_added)}</Table.Td>
                          <Table.Td ta="right">{fmtCompact(r.out_tokens)}</Table.Td>
                        </Table.Tr>
                      )
                    }),
                )}
              </Table.Tbody>
            </Table>
          </Table.ScrollContainer>
        )}
      </Panel>
    </>
  )
}
