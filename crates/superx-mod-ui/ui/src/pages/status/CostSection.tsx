import { Card, Grid, SimpleGrid, Text } from '@mantine/core'
import type { InsightsSummary } from '../../generated/InsightsSummary'
import type { StatsSummary } from '../../generated/StatsSummary'
import { AXIS, CHART_COLORS, EChart, INK_MUTED, TOOLTIP } from '../../EChart'
import { Panel, Stat, fmtBytes, fmtCompact, n, pct } from './parts'

// What it cost, and what left the machine (#367). Token totals are
// all-history and engine-side; exposure follows the range.

export function CostSection({ s, i, range }: { s: StatsSummary | undefined; i: InsightsSummary | undefined; range: string | null }) {
  const tok = i?.tokens
  const promptTotal = tok ? n(tok.input) + n(tok.cache_read) + n(tok.cache_write) : 0
  const cacheHit = tok ? pct(n(tok.cache_read), promptTotal) : null
  const e = s?.exposure
  return (
    <>
      <Grid mb="md" gap="md">
        <Grid.Col span={{ base: 12, lg: 7 }}>
          <Panel title="Tokens" scope="all" range={range} note="every captured message · cost is not recorded, so it is not guessed" h="100%">
            <SimpleGrid cols={{ base: 2, sm: 4 }} spacing="xs" mb="sm">
              <Stat label="Prompt tokens" value={fmtCompact(tok ? n(tok.input) : undefined)} sub="sent fresh" tip="input tokens billed as new context" />
              <Stat label="Cache reads" value={fmtCompact(tok ? n(tok.cache_read) : undefined)} sub="served from cache" />
              <Stat label="Cache writes" value={fmtCompact(tok ? n(tok.cache_write) : undefined)} sub="stored their side" />
              <Stat label="Output" value={fmtCompact(tok ? n(tok.output) : undefined)} sub={cacheHit == null ? '' : `${cacheHit}% cache hit`} />
            </SimpleGrid>
            <EChart
              height={160}
              option={{
                tooltip: { ...TOOLTIP, trigger: 'axis', axisPointer: { type: 'shadow' }, valueFormatter: (v: number) => fmtCompact(v) },
                grid: { left: 92, right: 56, top: 8, bottom: 8 },
                xAxis: { type: 'value', ...AXIS, axisLabel: { ...AXIS.axisLabel, show: false }, splitLine: { show: false } },
                yAxis: { type: 'category', data: ['cache write', 'output', 'fresh input', 'cache read'], ...AXIS, splitLine: { show: false } },
                series: [
                  {
                    type: 'bar',
                    barWidth: 16,
                    itemStyle: { borderRadius: 3 },
                    label: { show: true, position: 'right', color: INK_MUTED, fontSize: 11, formatter: (p: { value: number }) => fmtCompact(p.value) },
                    data: tok
                      ? [
                          { value: n(tok.cache_write), itemStyle: { color: CHART_COLORS[5] } },
                          { value: n(tok.output), itemStyle: { color: CHART_COLORS[2] } },
                          { value: n(tok.input), itemStyle: { color: CHART_COLORS[1] } },
                          { value: n(tok.cache_read), itemStyle: { color: CHART_COLORS[0] } },
                        ]
                      : [],
                  },
                ],
              }}
            />
          </Panel>
        </Grid.Col>
        <Grid.Col span={{ base: 12, lg: 5 }}>
          <Panel title="What left this machine" scope="range" range={range} note="measured from your own transcripts" h="100%">
            <SimpleGrid cols={{ base: 2, sm: 3 }} spacing="xs" mb="sm">
              <Stat label="Sent fresh" value={fmtCompact(e?.input_tokens)} sub="prompt tokens" />
              <Stat label="Cached by vendor" value={fmtCompact(e?.cache_write_tokens)} sub="stored their side" />
              <Stat label="Served back" value={fmtCompact(e?.cache_read_tokens)} sub="cache reads" />
              <Stat label="File text" value={e ? fmtBytes(e.content_bytes) : '—'} sub="into prompts" />
              <Stat
                label="Files read"
                value={fmtCompact(e?.files_read)}
                sub={`${e?.repos_exposed ?? '—'} repos`}
                tip="distinct paths read into a prompt — by the Read tool, or by a shell call that only looked"
              />
              <Stat label="Attachments" value={fmtCompact(e?.attachments)} sub="images, docs" />
            </SimpleGrid>
            {n(e?.outside_reads) > 0 && (
              <Text size="xs" c="orange.4" mb={4}>
                {fmtCompact(e?.outside_reads)} file reads came from outside the directory the agent was working in.
              </Text>
            )}
            {n(e?.secret_hits) > 0 ? (
              <Card withBorder bg="dark.8" p="xs">
                <Text size="sm" c="red.4" fw={600}>
                  {String(e?.secret_hits)} tool result{n(e?.secret_hits) === 1 ? '' : 's'} carried credential-shaped content into a prompt
                </Text>
                <Text size="xs" c="dimmed">
                  {(e?.secret_paths ?? []).slice(0, 8).join(' · ') || 'path not recorded'}
                </Text>
                <Text size="xs" c="dimmed" mt={4}>
                  Anything sent cannot be recalled. Rotate what was exposed.
                </Text>
              </Card>
            ) : (
              <Text size="xs" c="dimmed">
                {e ? 'No credential-shaped content detected in what was sent.' : ''}
              </Text>
            )}
          </Panel>
        </Grid.Col>
      </Grid>
    </>
  )
}
