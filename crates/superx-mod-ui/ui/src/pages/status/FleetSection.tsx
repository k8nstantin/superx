import { Badge, Grid, Group, Progress, Table, Text, Tooltip } from '@mantine/core'
import type { StatsSummary } from '../../generated/StatsSummary'
import { AXIS, CHART_COLORS, EChart, GRID_LINE, INK_MUTED, MONO, TOOLTIP } from '../../EChart'
import { sessionColor } from '../../Feed'
import { BANDS, Churn, FAIL, OK, Panel, fmtAge, fmtCompact, fmtMs, n, pct } from './parts'

// Who flew what (#367): agents, reasoning levels, models, branches and
// repos compared on outcome rather than volume, the work cube, and the
// sortie log — every session's span in the range.

function pctCell(v: number | null, bad: number, low = false) {
  if (v == null) return <Text size="xs" c="dimmed">—</Text>
  const tone = low ? (v >= bad ? FAIL : undefined) : v < bad ? FAIL : OK
  return (
    <Text size="xs" ff={MONO} c={tone}>
      {v}%
    </Text>
  )
}

export function FleetSection({ s, range }: { s: StatsSummary | undefined; range: string | null }) {
  const long = range === '7d' || range === '30d' || range === 'all'
  const cells = s?.work_cells ?? []
  const cube = (() => {
    const totals = new Map<string, number>()
    for (const c of cells) {
      const k = `${c.agent} · ${c.repo}`
      totals.set(k, (totals.get(k) ?? 0) + n(c.added))
    }
    const top = [...totals.entries()].sort((a, b) => b[1] - a[1]).slice(0, 7).map(([k]) => k)
    const buckets = [...new Set(cells.map((c) => c.t))].sort()
    const keyOf = (c: (typeof cells)[number]) => {
      const k = `${c.agent} · ${c.repo}`
      return top.includes(k) ? k : 'other'
    }
    const names = [...top, ...(cells.some((c) => keyOf(c) === 'other') ? ['other'] : [])]
    return { buckets, names, keyOf }
  })()

  const spans = (s?.timeline ?? []).slice(0, 10)

  return (
    <>
      <Panel title="Agent productivity and what it cost" scope="range" range={range} note="tokens spent per line that survived — lower is cheaper work" mb="md">
        {(s?.agent_stats?.length ?? 0) === 0 ? (
          <Text size="xs" c="dimmed">no agent wrote code in this range</Text>
        ) : (
          <Table.ScrollContainer minWidth={1100}>
            <Table striped>
              <Table.Thead>
                <Table.Tr>
                  <Table.Th>Agent</Table.Th>
                  <Table.Th ta="right">Sessions</Table.Th>
                  <Table.Th ta="right">Repos</Table.Th>
                  <Table.Th ta="right">Lines +/−</Table.Th>
                  <Table.Th ta="right">Churn</Table.Th>
                  <Table.Th ta="right">
                    <Tooltip label="share of this agent's replaced lines that nobody asked for" withArrow multiline w={260}>
                      <span>Unasked</span>
                    </Tooltip>
                  </Table.Th>
                  <Table.Th ta="right">Tests</Table.Th>
                  <Table.Th ta="right">Sent</Table.Th>
                  <Table.Th ta="right">Written</Table.Th>
                  <Table.Th ta="right">Tok/line</Table.Th>
                  <Table.Th ta="right">Switches</Table.Th>
                  <Table.Th ta="right">Verify</Table.Th>
                  <Table.Th ta="right">Compact</Table.Th>
                  <Table.Th ta="right">Undone</Table.Th>
                  <Table.Th ta="right">Fails</Table.Th>
                </Table.Tr>
              </Table.Thead>
              <Table.Tbody>
                {(s?.agent_stats ?? []).map((a) => {
                  const add = n(a.lines_added)
                  const del = n(a.lines_removed)
                  const churn = pct(del, add + del)
                  const cost = add > 0 ? Math.round((n(a.in_tokens) + n(a.out_tokens)) / add) : null
                  const unasked = pct(n(a.churn_self), n(a.churn_directed) + n(a.churn_self))
                  const tests = n(a.tests_passed) + n(a.tests_failed)
                  const passPct = pct(n(a.tests_passed), tests)
                  return (
                    <Table.Tr key={a.name}>
                      <Table.Td>
                        <Badge variant="light" color="pelican">{a.name}</Badge>
                      </Table.Td>
                      <Table.Td ta="right">{String(a.sessions)}</Table.Td>
                      <Table.Td ta="right">{String(a.repos)}</Table.Td>
                      <Table.Td ta="right"><Churn added={add} removed={del} size="xs" /></Table.Td>
                      <Table.Td ta="right">{pctCell(churn, BANDS.churnBad, true)}</Table.Td>
                      <Table.Td ta="right">{pctCell(unasked, BANDS.selfChurnBad, true)}</Table.Td>
                      <Table.Td ta="right">
                        {tests === 0 ? (
                          <Text size="xs" c="dimmed">—</Text>
                        ) : (
                          <Tooltip label={`${a.tests_passed} passed · ${a.tests_failed} failed · ${a.compile_errors} compile error(s)`} withArrow>
                            {pctCell(passPct, BANDS.passOk)}
                          </Tooltip>
                        )}
                      </Table.Td>
                      <Table.Td ta="right">{fmtCompact(a.in_tokens)}</Table.Td>
                      <Table.Td ta="right">{fmtCompact(a.out_tokens)}</Table.Td>
                      <Table.Td ta="right">{cost == null ? '—' : fmtCompact(cost)}</Table.Td>
                      <Table.Td ta="right" c={n(a.repo_switches) > BANDS.switchesBad ? 'orange.4' : undefined}>{String(a.repo_switches)}</Table.Td>
                      <Table.Td ta="right">{n(a.edit_to_verify_p50_secs) > 0 ? fmtMs(n(a.edit_to_verify_p50_secs) * 1000) : '—'}</Table.Td>
                      <Table.Td ta="right">{n(a.compactions) > 0 ? `${a.compactions} · ${fmtMs(a.compaction_ms)}` : '—'}</Table.Td>
                      <Table.Td ta="right" c={n(a.reverts) > 0 ? 'orange.4' : undefined}>{String(a.reverts)}</Table.Td>
                      <Table.Td ta="right" c={n(a.tool_failures) > 0 ? 'red.4' : undefined}>{String(a.tool_failures)}</Table.Td>
                    </Table.Tr>
                  )
                })}
              </Table.Tbody>
            </Table>
          </Table.ScrollContainer>
        )}
      </Panel>

      <Grid mb="md" gap="md">
        <Grid.Col span={{ base: 12, lg: 6 }}>
          <Panel title="Reasoning level against outcome" scope="range" range={range} note="does thinking harder produce keepable code" h="100%">
            {(s?.efforts?.length ?? 0) === 0 ? (
              <Text size="xs" c="dimmed">no agent reported a reasoning level in this range</Text>
            ) : (
              <Table.ScrollContainer minWidth={560}>
                <Table striped>
                  <Table.Thead>
                    <Table.Tr>
                      <Table.Th>Effort</Table.Th>
                      <Table.Th ta="right">Msgs</Table.Th>
                      <Table.Th ta="right">Lines +/−</Table.Th>
                      <Table.Th ta="right">Churn</Table.Th>
                      <Table.Th ta="right">Thinking</Table.Th>
                      <Table.Th ta="right">Tok/line</Table.Th>
                      <Table.Th ta="right">Tests</Table.Th>
                      <Table.Th ta="right">Undone</Table.Th>
                      <Table.Th ta="right">Fails</Table.Th>
                    </Table.Tr>
                  </Table.Thead>
                  <Table.Tbody>
                    {(s?.efforts ?? []).map((e) => {
                      const a = n(e.lines_added)
                      const d = n(e.lines_removed)
                      const tests = n(e.tests_passed) + n(e.tests_failed)
                      return (
                        <Table.Tr key={e.name}>
                          <Table.Td><Badge variant="light" color="pelican">{e.name}</Badge></Table.Td>
                          <Table.Td ta="right">{fmtCompact(e.messages)}</Table.Td>
                          <Table.Td ta="right"><Churn added={a} removed={d} size="xs" /></Table.Td>
                          <Table.Td ta="right">{pctCell(pct(d, a + d), BANDS.churnBad, true)}</Table.Td>
                          <Table.Td ta="right">{fmtCompact(e.thinking_tokens)}</Table.Td>
                          <Table.Td ta="right">{a > 0 ? Math.round(n(e.out_tokens) / a) : '—'}</Table.Td>
                          <Table.Td ta="right">{tests === 0 ? <Text size="xs" c="dimmed">—</Text> : pctCell(pct(n(e.tests_passed), tests), BANDS.passOk)}</Table.Td>
                          <Table.Td ta="right" c={n(e.reverts) > 0 ? 'orange.4' : undefined}>{String(e.reverts)}</Table.Td>
                          <Table.Td ta="right" c={n(e.tool_failures) > 0 ? 'red.4' : undefined}>{String(e.tool_failures)}</Table.Td>
                        </Table.Tr>
                      )
                    })}
                  </Table.Tbody>
                </Table>
              </Table.ScrollContainer>
            )}
          </Panel>
        </Grid.Col>
        <Grid.Col span={{ base: 12, lg: 6 }}>
          <Panel title="Model against model" scope="range" range={range} note="which one produces keepable code" h="100%">
            {(s?.models?.length ?? 0) === 0 ? (
              <Text size="xs" c="dimmed">no model named in this range</Text>
            ) : (
              <Table striped>
                <Table.Thead>
                  <Table.Tr>
                    <Table.Th>Model</Table.Th>
                    <Table.Th ta="right">Lines +/−</Table.Th>
                    <Table.Th ta="right">Churn</Table.Th>
                    <Table.Th ta="right">Tok/line</Table.Th>
                    <Table.Th ta="right">Undone</Table.Th>
                    <Table.Th ta="right">Fails</Table.Th>
                  </Table.Tr>
                </Table.Thead>
                <Table.Tbody>
                  {(s?.models ?? []).filter((m) => m.name !== 'unknown' && !m.name.startsWith('<')).map((m) => {
                    const a = n(m.lines_added)
                    const d = n(m.lines_removed)
                    return (
                      <Table.Tr key={m.name}>
                        <Table.Td>
                          <Text size="xs" ff={MONO}>{m.name}</Text>
                          <Text size="xs" c="dimmed">{fmtCompact(m.messages)} msgs</Text>
                        </Table.Td>
                        <Table.Td ta="right"><Churn added={a} removed={d} size="xs" /></Table.Td>
                        <Table.Td ta="right">{pctCell(pct(d, a + d), BANDS.churnBad, true)}</Table.Td>
                        <Table.Td ta="right">{a > 0 ? Math.round(n(m.out_tokens) / a) : '—'}</Table.Td>
                        <Table.Td ta="right" c={n(m.reverts) > 0 ? 'orange.4' : undefined}>{String(m.reverts)}</Table.Td>
                        <Table.Td ta="right" c={n(m.tool_failures) > 0 ? 'red.4' : undefined}>{String(m.tool_failures)}</Table.Td>
                      </Table.Tr>
                    )
                  })}
                </Table.Tbody>
              </Table>
            )}
          </Panel>
        </Grid.Col>
      </Grid>

      <Panel title="Branches — worst first" scope="range" range={range} note="two branches in one repo no longer sum into one row" mb="md">
        {(s?.branches?.length ?? 0) === 0 ? (
          <Text size="xs" c="dimmed">no branch named in this range</Text>
        ) : (
          <Table.ScrollContainer minWidth={900}>
            <Table striped highlightOnHover>
              <Table.Thead>
                <Table.Tr>
                  <Table.Th>Branch</Table.Th>
                  <Table.Th ta="right">
                    <Tooltip label="30% steering · 25% keep rate · 20% test pass · 15% tool success · 10% half-life. Components with no data are dropped, not scored zero." withArrow multiline w={300}>
                      <span>Quality</span>
                    </Tooltip>
                  </Table.Th>
                  <Table.Th ta="right">
                    <Tooltip label="share of replaced lines nobody asked for — the spaghetti signal" withArrow>
                      <span>Unasked</span>
                    </Tooltip>
                  </Table.Th>
                  <Table.Th ta="right">Lines +/−</Table.Th>
                  <Table.Th ta="right">Rework</Table.Th>
                  <Table.Th ta="right">
                    <Tooltip label="minutes means thrash; hours means the design moved" withArrow>
                      <span>Half-life</span>
                    </Tooltip>
                  </Table.Th>
                  <Table.Th ta="right">Verify</Table.Th>
                  <Table.Th ta="right">Tests</Table.Th>
                  <Table.Th ta="right">Reverts</Table.Th>
                  <Table.Th ta="right">Fails</Table.Th>
                  <Table.Th ta="right">Agents</Table.Th>
                  <Table.Th ta="right">Active</Table.Th>
                </Table.Tr>
              </Table.Thead>
              <Table.Tbody>
                {(s?.branches ?? []).map((b) => {
                  const q = n(b.quality_pct)
                  const unasked = n(b.self_churn_pct)
                  const pass = n(b.test_pass_pct)
                  const half = n(b.survival_p50_mins)
                  const idle = b.last_active ? Math.max(0, Math.floor((Date.now() - new Date(b.last_active).getTime()) / 1000)) : null
                  return (
                    <Table.Tr key={`${b.repo}/${b.branch}`}>
                      <Table.Td>
                        <Text size="xs" ff={MONO}>{b.branch}</Text>
                        <Text size="xs" c="dimmed">{b.repo}</Text>
                      </Table.Td>
                      <Table.Td ta="right">
                        {q < 0 ? (
                          <Tooltip label="nothing measurable happened on this branch" withArrow>
                            <Text size="sm" c="dimmed">—</Text>
                          </Tooltip>
                        ) : (
                          <Group gap={6} justify="flex-end" wrap="nowrap">
                            <Progress value={q} w={54} size="sm" color={q >= 70 ? 'teal' : q >= 40 ? 'yellow' : 'red'} />
                            <Text size="sm" ff={MONO}>{q}%</Text>
                          </Group>
                        )}
                      </Table.Td>
                      <Table.Td ta="right">
                        {n(b.churn_directed) + n(b.churn_self) === 0 ? <Text size="xs" c="dimmed">—</Text> : pctCell(unasked, BANDS.selfChurnBad, true)}
                      </Table.Td>
                      <Table.Td ta="right"><Churn added={b.lines_added} removed={b.lines_removed} size="xs" /></Table.Td>
                      <Table.Td ta="right">{n(b.lines_added) === 0 ? '—' : `${n(b.rework_pct)}%`}</Table.Td>
                      <Table.Td ta="right">
                        {half < 0 ? (
                          <Text size="xs" c="dimmed">—</Text>
                        ) : half === 0 ? (
                          <Tooltip label="rewritten within a minute" withArrow>
                            <Text size="xs" c={FAIL}>&lt;1m</Text>
                          </Tooltip>
                        ) : (
                          fmtAge(half * 60)
                        )}
                      </Table.Td>
                      <Table.Td ta="right">{n(b.edit_to_verify_p50_secs) > 0 ? fmtMs(n(b.edit_to_verify_p50_secs) * 1000) : '—'}</Table.Td>
                      <Table.Td ta="right">
                        {pass < 0 ? (
                          <Tooltip label={n(b.tests_run) > 0 ? `${b.tests_run} test run(s), but no tally could be read from the output` : 'this branch ran no tests'} withArrow>
                            <Text size="xs" c={n(b.tests_run) > 0 ? 'yellow.5' : 'dimmed'}>{n(b.tests_run) > 0 ? 'unread' : 'none'}</Text>
                          </Tooltip>
                        ) : (
                          pctCell(pass, BANDS.passOk)
                        )}
                      </Table.Td>
                      <Table.Td ta="right" c={n(b.reverts) > 0 ? FAIL : undefined}>{String(b.reverts)}</Table.Td>
                      <Table.Td ta="right" c={n(b.tool_failures) > 0 ? 'red.4' : undefined}>{String(b.tool_failures)}</Table.Td>
                      <Table.Td ta="right">{String(b.agents)}</Table.Td>
                      <Table.Td ta="right"><Text size="xs" c="dimmed">{idle == null ? '—' : `${fmtAge(idle)} ago`}</Text></Table.Td>
                    </Table.Tr>
                  )
                })}
              </Table.Tbody>
            </Table>
          </Table.ScrollContainer>
        )}
      </Panel>

      <Grid mb="md" gap="md">
        <Grid.Col span={{ base: 12, lg: 6 }}>
          <Panel title="Repos worked" scope="range" range={range} note="one busy repo should not hide a thrashing one" h="100%">
            <Table.ScrollContainer minWidth={560}>
              <Table striped highlightOnHover>
                <Table.Thead>
                  <Table.Tr>
                    <Table.Th>Repo</Table.Th>
                    <Table.Th ta="right">Agents</Table.Th>
                    <Table.Th ta="right">Msgs</Table.Th>
                    <Table.Th ta="right">+/−</Table.Th>
                    <Table.Th ta="right">Churn</Table.Th>
                    <Table.Th ta="right">Files</Table.Th>
                    <Table.Th ta="right">New</Table.Th>
                    <Table.Th ta="right">Tests</Table.Th>
                    <Table.Th ta="right">Fails</Table.Th>
                  </Table.Tr>
                </Table.Thead>
                <Table.Tbody>
                  {(s?.repos ?? []).map((r) => {
                    const a = n(r.lines_added)
                    const d = n(r.lines_removed)
                    return (
                      <Table.Tr key={r.name}>
                        <Table.Td>
                          <Text size="xs" ff={MONO}>{r.name}</Text>
                          {r.branch && <Text size="xs" c="dimmed">{r.branch}</Text>}
                        </Table.Td>
                        <Table.Td ta="right">{String(r.agents)}</Table.Td>
                        <Table.Td ta="right">{fmtCompact(r.messages)}</Table.Td>
                        <Table.Td ta="right"><Churn added={a} removed={d} size="xs" /></Table.Td>
                        <Table.Td ta="right">{pctCell(pct(d, a + d), BANDS.churnBad, true)}</Table.Td>
                        <Table.Td ta="right">{String(r.files_touched)}</Table.Td>
                        <Table.Td ta="right" c={n(r.files_created) > 0 ? OK : undefined}>{String(r.files_created)}</Table.Td>
                        <Table.Td ta="right">{String(r.tests_run)}</Table.Td>
                        <Table.Td ta="right" c={n(r.tool_failures) > 0 ? 'red.4' : undefined}>{String(r.tool_failures)}</Table.Td>
                      </Table.Tr>
                    )
                  })}
                </Table.Tbody>
              </Table>
            </Table.ScrollContainer>
          </Panel>
        </Grid.Col>
        <Grid.Col span={{ base: 12, lg: 6 }}>
          <Panel title="Sortie log" scope="range" range={range} note="every session's span in the range · newest first" h="100%">
            {spans.length === 0 ? (
              <Text size="xs" c="dimmed">no session in this range</Text>
            ) : (
              <Table striped>
                <Table.Thead>
                  <Table.Tr>
                    <Table.Th>Session</Table.Th>
                    <Table.Th>Repo</Table.Th>
                    <Table.Th ta="right">Took off</Table.Th>
                    <Table.Th ta="right">Airtime</Table.Th>
                    <Table.Th ta="right">Msgs</Table.Th>
                  </Table.Tr>
                </Table.Thead>
                <Table.Tbody>
                  {spans.map((sp) => {
                    const start = new Date(sp.start)
                    const end = new Date(sp.end)
                    const mins = Math.max(0, Math.round((end.getTime() - start.getTime()) / 60_000))
                    return (
                      <Table.Tr key={sp.identity}>
                        <Table.Td>
                          <Group gap={6} wrap="nowrap">
                            <span style={{ width: 8, height: 8, borderRadius: 2, background: sessionColor(sp.identity), flexShrink: 0 }} />
                            <Text size="xs" ff={MONO}>{sp.identity.slice(0, 13)}</Text>
                          </Group>
                        </Table.Td>
                        <Table.Td><Text size="xs" c="dimmed">{sp.repo ?? '—'}</Text></Table.Td>
                        <Table.Td ta="right"><Text size="xs">{start.toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' })}</Text></Table.Td>
                        <Table.Td ta="right"><Text size="xs" ff={MONO}>{mins >= 60 ? `${Math.floor(mins / 60)}h ${mins % 60}m` : `${mins}m`}</Text></Table.Td>
                        <Table.Td ta="right">{String(sp.messages)}</Table.Td>
                      </Table.Tr>
                    )
                  })}
                </Table.Tbody>
              </Table>
            )}
          </Panel>
        </Grid.Col>
      </Grid>

      <Grid mb="md" gap="md">
        <Grid.Col span={{ base: 12, lg: 7 }}>
          <Panel title="Where the work went" scope="range" range={range} note={`lines written per agent, per repo, per ${long ? 'day' : 'hour'}`} h="100%">
            {cells.length === 0 ? (
              <Text size="xs" c="dimmed">no code written in this range</Text>
            ) : (
              <EChart
                height={280}
                option={{
                  legend: { data: cube.names, textStyle: { color: INK_MUTED, fontSize: 10 }, top: 0 },
                  grid: { left: 48, right: 12, top: 30, bottom: 24 },
                  xAxis: {
                    type: 'category',
                    data: cube.buckets.map((b) => (b.includes('T') ? b.slice(11) + ':00' : b.slice(5))),
                    axisLabel: { color: AXIS.axisLabel.color, fontSize: 10 },
                    axisLine: { lineStyle: { color: GRID_LINE } },
                  },
                  yAxis: { type: 'value', axisLabel: { color: AXIS.axisLabel.color, fontSize: 10 }, splitLine: { lineStyle: { color: GRID_LINE } } },
                  tooltip: { ...TOOLTIP, trigger: 'axis', axisPointer: { type: 'shadow' } },
                  series: cube.names.map((name, idx) => ({
                    name,
                    type: 'bar',
                    stack: 'work',
                    itemStyle: { color: CHART_COLORS[idx % CHART_COLORS.length] },
                    data: cube.buckets.map((b) => cells.filter((c) => c.t === b && cube.keyOf(c) === name).reduce((acc, c) => acc + n(c.added), 0)),
                  })),
                }}
              />
            )}
          </Panel>
        </Grid.Col>
        <Grid.Col span={{ base: 12, lg: 5 }}>
          <Panel title="Busiest sessions" scope="range" range={range} note="bars wear each session's feed colour" h="100%">
            <EChart
              height={280}
              option={{
                grid: { left: 150, right: 60, top: 8, bottom: 24 },
                tooltip: {
                  trigger: 'item',
                  ...TOOLTIP,
                  formatter: (p: { dataIndex: number }) => {
                    const rows = [...(s?.top_sessions ?? [])].reverse()
                    const t = rows[p.dataIndex]
                    return t ? `${t.identity}<br/>${t.messages} messages · ${fmtCompact(t.lines_written)} lines · ${fmtCompact(t.output_tokens)} out tokens` : ''
                  },
                },
                xAxis: { type: 'value', ...AXIS },
                yAxis: {
                  type: 'category',
                  data: (s?.top_sessions ?? []).map((t) => t.identity).reverse(),
                  ...AXIS,
                  axisLabel: { color: INK_MUTED, fontSize: 11, fontFamily: MONO },
                  splitLine: { show: false },
                },
                series: [
                  {
                    type: 'bar',
                    barWidth: 14,
                    data: (s?.top_sessions ?? []).map((t) => ({ value: n(t.messages), itemStyle: { color: sessionColor(t.session_id), borderRadius: 3 } })).reverse(),
                    label: { show: true, position: 'right', color: INK_MUTED, fontSize: 11 },
                  },
                ],
              }}
            />
          </Panel>
        </Grid.Col>
      </Grid>
    </>
  )
}
