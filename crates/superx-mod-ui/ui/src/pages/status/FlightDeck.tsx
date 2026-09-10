import { Badge, Card, Group, Progress, SimpleGrid, Table, Text, Tooltip } from '@mantine/core'
import type { InsightsSummary } from '../../generated/InsightsSummary'
import type { StatsSummary } from '../../generated/StatsSummary'
import { MONO } from '../../EChart'
import { LivenessDot } from '../../LivenessDot'
import { BANDS, Churn, CoverageStrip, FAIL, Panel, Stat, fmtAge, fmtCompact, n, type Tone } from './parts'

// The flight deck (#367): one row per agent in the air, and the fleet
// readings beside them. Everything here is a reading of right now and
// ignores the range — a pilot's primary display does not scroll back.

const DOING_COLOR: Record<string, string> = {
  writing: 'teal',
  verifying: 'blue',
  reading: 'cyan',
  thinking: 'grape',
  quiet: 'gray',
}

function ContextCell({ pct, tokens }: { pct: number | bigint | null; tokens: number | bigint | null }) {
  if (pct == null)
    return (
      <Text size="xs" c="dimmed">
        —
      </Text>
    )
  const p = n(pct)
  const color = p >= BANDS.contextBad ? 'red' : p >= BANDS.contextWarn ? 'yellow' : 'green'
  return (
    <Tooltip label={`${fmtCompact(tokens)} tokens in context — ${p}% of the window`} withArrow>
      <Group gap={6} wrap="nowrap">
        <Progress value={p} color={color} size="sm" w={64} />
        <Text size="xs" ff={MONO} w={34} ta="right">
          {p}%
        </Text>
      </Group>
    </Tooltip>
  )
}

export function FlightDeck({
  s,
  i,
  loading,
}: {
  s: StatsSummary | undefined
  i: InsightsSummary | undefined
  loading: boolean
}) {
  const live = s?.live ?? []
  const lagTone: Tone =
    i?.last_event_secs == null ? 'none' : n(i.last_event_secs) >= BANDS.lagBad ? 'bad' : n(i.last_event_secs) >= BANDS.lagWarn ? 'warn' : 'ok'
  return (
    <>
      <SimpleGrid cols={{ base: 2, md: 3, lg: 6 }} spacing="xs" mb="md">
        <Stat
          label="In the air"
          value={s ? `${s.sessions_active} live` : '…'}
          sub={s ? `${s.sessions_total} sessions total` : ''}
          tip="sessions with a message in the last five minutes"
        />
        <Stat
          label="Agents at once"
          value={s ? String(s.max_concurrent_sessions) : '…'}
          sub="peak in one five-minute bucket"
          tip="the most sessions seen active in the same five-minute bucket of this range"
        />
        <Stat
          label="Burn rate"
          value={s ? `${fmtCompact(s.tokens_last_hour)}/h` : '…'}
          sub="output tokens, last hour"
          tip="how fast the agents are producing right now"
        />
        <Stat
          label="Throughput"
          value={s ? `${fmtCompact(s.messages_last_hour)}/h` : '…'}
          sub="messages, last hour"
        />
        <Stat
          label="Capture lag"
          value={i ? fmtAge(i.last_event_secs) : '…'}
          sub={i ? `${fmtCompact(i.events_last_hour)} events this hour` : ''}
          tip="age of the newest captured event — how current this whole page is"
          tone={lagTone}
        />
        <Card withBorder p="sm">
          <Text size="xs" c="dimmed" tt="uppercase" style={{ letterSpacing: 0.4 }}>
            Clock coverage
          </Text>
          <Group gap={6} align="baseline">
            <Text fz={22} fw={700} ff={MONO}>
              {s ? `${s.active_hours_24h}/24` : '—'}
            </Text>
            <Text size="xs" c="dimmed">
              hours worked
            </Text>
          </Group>
          <CoverageStrip hours={s?.active_hours_24h} />
        </Card>
      </SimpleGrid>

      <Panel
        title="Running now"
        scope="live"
        range={null}
        note="sessions with a message in the last five minutes · busiest first"
        mb="md"
      >
        {live.length === 0 ? (
          <Text size="sm" c="dimmed">
            {loading ? 'Reading the substrate…' : 'No agent has spoken in the last five minutes.'}
          </Text>
        ) : (
          <Table.ScrollContainer minWidth={1180}>
            <Table striped highlightOnHover>
              <Table.Thead>
                <Table.Tr>
                  <Table.Th>Session</Table.Th>
                  <Table.Th>Repo</Table.Th>
                  <Table.Th>Model</Table.Th>
                  <Table.Th>Doing</Table.Th>
                  <Table.Th>
                    <Tooltip
                      label="context window in use at the newest usage-bearing message — the altitude before compaction"
                      withArrow
                      multiline
                      w={260}
                    >
                      <span>Context</span>
                    </Tooltip>
                  </Table.Th>
                  <Table.Th>Files now</Table.Th>
                  <Table.Th ta="right">
                    <Tooltip
                      label={`unasked share of its own rewrites, and files it has come back to ${s?.revisit_at ?? 3} or more times — the compounding signal`}
                      withArrow
                      multiline
                      w={280}
                    >
                      <span>Circling</span>
                    </Tooltip>
                  </Table.Th>
                  <Table.Th ta="right">Msgs</Table.Th>
                  <Table.Th ta="right">
                    <Tooltip label="lines written, and lines an edit replaced — a whole-file Write counts entirely as added" withArrow>
                      <span>Lines +/−</span>
                    </Tooltip>
                  </Table.Th>
                  <Table.Th ta="right">Tokens</Table.Th>
                  <Table.Th ta="right">Fails</Table.Th>
                  <Table.Th ta="right">Idle</Table.Th>
                </Table.Tr>
              </Table.Thead>
              <Table.Tbody>
                {live.map((l) => {
                  const circling = n(l.self_churn_pct) >= BANDS.selfChurnBad || n(l.files_revisited) >= BANDS.revisitedBad
                  return (
                    <Table.Tr key={l.identity}>
                      <Table.Td>
                        <Group gap={6} wrap="nowrap">
                          {/* Stated, not re-derived: the server already cut
                              this panel to the activity window (#344). */}
                          <LivenessDot state="active" size={8} />
                          <Text size="xs" ff={MONO}>
                            {l.identity.slice(0, 13)}
                          </Text>
                        </Group>
                      </Table.Td>
                      <Table.Td>
                        <Text size="xs" ff={MONO} style={{ whiteSpace: 'nowrap' }}>
                          {l.repo ?? '—'}
                          {l.branch && (
                            <Text span size="xs" c="dimmed">
                              {' · '}
                              {l.branch}
                            </Text>
                          )}
                        </Text>
                      </Table.Td>
                      <Table.Td>
                        {l.model ? (
                          <Group gap={4} wrap="nowrap">
                            <Text size="xs" style={{ whiteSpace: 'nowrap' }}>
                              {l.model}
                            </Text>
                            {l.effort && (
                              <Tooltip label="reasoning effort this session is running at" withArrow>
                                <Badge variant="outline" color="gray" size="xs">
                                  {l.effort}
                                </Badge>
                              </Tooltip>
                            )}
                          </Group>
                        ) : (
                          <Text size="xs" c="dimmed">
                            —
                          </Text>
                        )}
                      </Table.Td>
                      <Table.Td>
                        <Tooltip label={l.last_tool ?? 'no tool call in this range'} withArrow>
                          <Badge size="sm" variant="light" color={DOING_COLOR[l.doing] ?? 'gray'}>
                            {l.doing}
                            {n(l.last_op_secs) > 0 ? ` · ${fmtAge(l.last_op_secs)}` : ''}
                          </Badge>
                        </Tooltip>
                      </Table.Td>
                      <Table.Td>
                        <ContextCell pct={l.context_pct} tokens={l.context_tokens} />
                      </Table.Td>
                      <Table.Td>
                        {(l.files_now ?? []).length === 0 ? (
                          <Text size="xs" c="dimmed">
                            —
                          </Text>
                        ) : (
                          <Tooltip
                            label={<span style={{ whiteSpace: 'pre-line' }}>{(l.files_now ?? []).join('\n')}</span>}
                            withArrow
                            multiline
                            w={420}
                          >
                            <Text size="xs" ff={MONO} c="dimmed" lineClamp={1} style={{ maxWidth: 220 }}>
                              {(l.files_now ?? []).map((f) => f.split('/').slice(-1)[0]).join(' · ')}
                            </Text>
                          </Tooltip>
                        )}
                      </Table.Td>
                      <Table.Td ta="right">
                        {n(l.files_revisited) === 0 && n(l.self_churn_pct) === 0 ? (
                          <Text size="sm" c="dimmed">
                            —
                          </Text>
                        ) : (
                          <Tooltip
                            label={`${l.self_churn_pct}% of replaced lines unasked · ${l.files_revisited} file(s) touched ${s?.revisit_at ?? ''}+ times`}
                            withArrow
                          >
                            <Text size="sm" ff={MONO} c={circling ? FAIL : undefined}>
                              {l.self_churn_pct}%{n(l.files_revisited) > 0 ? ` ·${l.files_revisited}` : ''}
                            </Text>
                          </Tooltip>
                        )}
                      </Table.Td>
                      <Table.Td ta="right">{String(l.messages)}</Table.Td>
                      <Table.Td ta="right">
                        <Churn added={l.lines_added} removed={l.lines_removed} />
                      </Table.Td>
                      <Table.Td ta="right">{fmtCompact(l.out_tokens)}</Table.Td>
                      <Table.Td ta="right" c={n(l.tool_failures) > 0 ? 'red.4' : undefined}>
                        {String(l.tool_failures)}
                      </Table.Td>
                      <Table.Td ta="right">{fmtAge(l.idle_secs)}</Table.Td>
                    </Table.Tr>
                  )
                })}
              </Table.Tbody>
            </Table>
          </Table.ScrollContainer>
        )}
      </Panel>
    </>
  )
}
