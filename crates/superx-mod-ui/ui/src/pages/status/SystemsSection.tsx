import { Anchor, Badge, Grid, SimpleGrid, Table, Text, Tooltip } from '@mantine/core'
import type { InsightsSummary } from '../../generated/InsightsSummary'
import type { StatsSummary } from '../../generated/StatsSummary'
import type { StatusResponse } from '../../generated/StatusResponse'
import { AXIS, CHART_COLORS, EChart, INK_MUTED, MONO, TOOLTIP } from '../../EChart'
import { BANDS, Panel, Stat, fmtAge, fmtCompact, n } from './parts'

// The OS itself (#367): module health read off the lifecycle stream,
// substrate totals, and what capture spends itself on. The registry
// says what state a module is IN; the stream says what happened to it.

export function SystemsSection({
  s,
  i,
  status,
  range,
}: {
  s: StatsSummary | undefined
  i: InsightsSummary | undefined
  status: StatusResponse | undefined
  range: string | null
}) {
  const health = new Map((i?.module_health ?? []).map((h) => [h.name, h]))
  const startup = new Map((i?.module_startup ?? []).map((m) => [m.name, n(m.value)]))
  const modules = [...(status?.modules ?? [])].sort((a, b) => {
    const fa = n(health.get(a.name)?.failures_recent)
    const fb = n(health.get(b.name)?.failures_recent)
    if (fa !== fb) return fb - fa
    if ((a.lifecycle === 'active') !== (b.lifecycle === 'active')) return a.lifecycle === 'active' ? 1 : -1
    return a.name.localeCompare(b.name)
  })
  const lag = i?.last_event_secs == null ? null : n(i.last_event_secs)

  return (
    <>
      <SimpleGrid cols={{ base: 2, md: 3, lg: 6 }} spacing="xs" mb="md">
        <Stat label="Agents" value={s ? String(s.agents) : '…'} tip="agents the OS has discovered" />
        <Stat label="Sessions" value={fmtCompact(s?.sessions_total)} sub={s ? `${s.sessions_active} live` : ''} />
        <Stat label="Events captured" value={fmtCompact(s?.events_total)} sub="all time" />
        <Stat label="Messages" value={fmtCompact(s?.messages_total)} sub="all time" />
        <Stat label="Output tokens" value={fmtCompact(s?.output_tokens_total)} sub="all sessions" />
        <Stat
          label="Events this hour"
          value={i ? fmtCompact(i.events_last_hour) : '…'}
          sub={i ? `newest ${fmtAge(i.last_event_secs)} ago` : ''}
          tone={lag == null ? 'none' : lag >= BANDS.lagBad ? 'bad' : lag >= BANDS.lagWarn ? 'warn' : 'ok'}
        />
      </SimpleGrid>

      <Panel
        title="Modules"
        scope="live"
        range={range}
        note={status && s ? `${s.modules_active}/${s.modules_total} active · failures over the last day and all time` : ''}
        mb="md"
      >
        <Table.ScrollContainer minWidth={980}>
          <Table striped highlightOnHover>
            <Table.Thead>
              <Table.Tr>
                <Table.Th>Module</Table.Th>
                <Table.Th>Kind</Table.Th>
                <Table.Th>Lifecycle</Table.Th>
                <Table.Th>Last event</Table.Th>
                <Table.Th ta="right">
                  <Tooltip label="module_failed + module_start_failed + module_start_abandoned — last day / all time" withArrow multiline w={260}>
                    <span>Failures</span>
                  </Tooltip>
                </Table.Th>
                <Table.Th>Last error</Table.Th>
                <Table.Th ta="right">Startup</Table.Th>
                <Table.Th>Version</Table.Th>
                <Table.Th>Provisioned</Table.Th>
                <Table.Th>UI</Table.Th>
              </Table.Tr>
            </Table.Thead>
            <Table.Tbody>
              {modules.map((m) => {
                const h = health.get(m.name)
                const recent = n(h?.failures_recent)
                const total = n(h?.failures_total)
                const down = m.lifecycle !== 'active'
                return (
                  <Table.Tr key={m.module_id}>
                    <Table.Td>
                      <Text size="sm" fw={down || recent > 0 ? 600 : undefined}>
                        {m.name}
                      </Text>
                      <Text size="xs" c="dimmed" ff={MONO}>
                        {m.module_id.slice(0, 13)}
                      </Text>
                    </Table.Td>
                    <Table.Td>
                      <Text size="xs" c="dimmed">
                        {m.kind}
                      </Text>
                    </Table.Td>
                    <Table.Td>
                      <Badge color={m.lifecycle === 'active' ? 'green' : m.lifecycle === 'disabled' ? 'gray' : 'red'} variant={down ? 'filled' : 'light'}>
                        {m.lifecycle}
                      </Badge>
                    </Table.Td>
                    <Table.Td>
                      {h && h.last_event ? (
                        <Text size="xs" ff={MONO}>
                          {h.last_event.replace('module_', '')}
                          <Text span size="xs" c="dimmed">
                            {' · '}
                            {fmtAge(h.last_event_secs)} ago
                          </Text>
                        </Text>
                      ) : (
                        <Text size="xs" c="dimmed">
                          —
                        </Text>
                      )}
                    </Table.Td>
                    <Table.Td ta="right">
                      <Text size="xs" ff={MONO} c={recent > 0 ? 'red.4' : total > 0 ? 'dimmed' : undefined}>
                        {recent} / {total}
                      </Text>
                    </Table.Td>
                    <Table.Td>
                      {h?.last_error ? (
                        <Tooltip label={h.last_error} withArrow multiline w={420}>
                          <Text size="xs" c="orange.4" lineClamp={1} style={{ maxWidth: 260 }}>
                            {h.last_error}
                          </Text>
                        </Tooltip>
                      ) : (
                        <Text size="xs" c="dimmed">
                          —
                        </Text>
                      )}
                    </Table.Td>
                    <Table.Td ta="right">
                      <Text size="xs" ff={MONO}>
                        {startup.has(m.name) ? `${startup.get(m.name)}ms` : '—'}
                      </Text>
                    </Table.Td>
                    <Table.Td>
                      <Text size="xs">v{m.version}</Text>
                    </Table.Td>
                    <Table.Td>
                      <Text size="xs" c="dimmed">
                        {m.provisioned == null ? '—' : m.provisioned ? 'yes' : 'no'}
                      </Text>
                    </Table.Td>
                    <Table.Td>
                      {m.ui_url ? (
                        <Anchor href={m.ui_url} target="_blank" rel="noopener" size="xs">
                          open ↗
                        </Anchor>
                      ) : (
                        <Text size="xs" c="dimmed">
                          —
                        </Text>
                      )}
                    </Table.Td>
                  </Table.Tr>
                )
              })}
            </Table.Tbody>
          </Table>
        </Table.ScrollContainer>
      </Panel>

      <Grid mb="md" gap="md">
        <Grid.Col span={{ base: 12, lg: 5 }}>
          <Panel title="What capture spends itself on" scope="all" range={range} note="telemetry by kind" h="100%">
            <EChart
              height={230}
              option={{
                tooltip: { ...TOOLTIP, trigger: 'axis', axisPointer: { type: 'shadow' }, valueFormatter: (v: number) => fmtCompact(v) },
                grid: { left: 130, right: 46, top: 8, bottom: 24 },
                xAxis: { type: 'value', ...AXIS, axisLabel: { ...AXIS.axisLabel, show: false } },
                yAxis: { type: 'category', data: (i?.event_kinds ?? []).slice(0, 8).map((k) => k.name).reverse(), ...AXIS, splitLine: { show: false } },
                series: [
                  {
                    type: 'bar',
                    barWidth: 14,
                    itemStyle: { borderRadius: 3, color: CHART_COLORS[0] },
                    label: { show: true, position: 'right', color: INK_MUTED, fontSize: 11, formatter: (p: { value: number }) => fmtCompact(p.value) },
                    data: (i?.event_kinds ?? []).slice(0, 8).map((k) => n(k.value)).reverse(),
                  },
                ],
              }}
            />
          </Panel>
        </Grid.Col>
        <Grid.Col span={{ base: 12, lg: 3 }}>
          <Panel title="Message roles" scope="all" range={range} h="100%">
            <EChart
              height={230}
              option={{
                grid: { left: 44, right: 8, top: 8, bottom: 24 },
                tooltip: { trigger: 'axis', ...TOOLTIP },
                xAxis: { type: 'category', data: (s?.message_roles ?? []).map((r) => r.name), ...AXIS, splitLine: { show: false } },
                yAxis: { type: 'value', ...AXIS },
                series: [
                  {
                    type: 'bar',
                    barWidth: 18,
                    data: (s?.message_roles ?? []).map((r, k) => ({ value: n(r.value), itemStyle: { color: CHART_COLORS[k % CHART_COLORS.length], borderRadius: 3 } })),
                  },
                ],
              }}
            />
          </Panel>
        </Grid.Col>
        <Grid.Col span={{ base: 12, lg: 4 }}>
          <Panel title="Startup cost" scope="live" range={range} note="newest reading per module" h="100%">
            {(i?.module_startup?.length ?? 0) === 0 ? (
              <Text size="xs" c="dimmed">
                no startup reading yet
              </Text>
            ) : (
              <EChart
                height={230}
                option={{
                  tooltip: { ...TOOLTIP, trigger: 'axis', axisPointer: { type: 'shadow' }, valueFormatter: (v: number) => `${v}ms` },
                  grid: { left: 46, right: 14, top: 14, bottom: 60 },
                  xAxis: { type: 'category', data: (i?.module_startup ?? []).map((m) => m.name), ...AXIS, axisLabel: { ...AXIS.axisLabel, rotate: 35, fontSize: 10 }, splitLine: { show: false } },
                  yAxis: { type: 'value', ...AXIS, axisLabel: { ...AXIS.axisLabel, formatter: '{value}ms' } },
                  series: [{ type: 'bar', barWidth: 18, itemStyle: { borderRadius: 3, color: CHART_COLORS[4] }, data: (i?.module_startup ?? []).map((m) => n(m.value)) }],
                }}
              />
            )}
          </Panel>
        </Grid.Col>
      </Grid>
      {(s?.boot_durations?.length ?? 0) > 0 && (
        <Panel title="Boot durations" scope="all" range={range} note="the newest boots, in milliseconds" mb="md">
          <EChart
            height={160}
            option={{
              grid: { left: 60, right: 8, top: 8, bottom: 24 },
              tooltip: { trigger: 'axis', ...TOOLTIP },
              xAxis: { type: 'category', data: (s?.boot_durations ?? []).map((b) => b.t), ...AXIS, axisLabel: { show: false }, splitLine: { show: false } },
              yAxis: { type: 'value', ...AXIS, axisLabel: { ...AXIS.axisLabel, formatter: '{value}ms' } },
              series: [{ type: 'line', data: (s?.boot_durations ?? []).map((b) => n(b.value)), symbol: 'circle', symbolSize: 6, lineStyle: { width: 2, color: '#199e70' }, itemStyle: { color: '#199e70' } }],
            }}
          />
        </Panel>
      )}
    </>
  )
}
