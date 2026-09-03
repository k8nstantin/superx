import { Grid } from '@mantine/core'
import type { InsightsSummary } from '../../generated/InsightsSummary'
import type { StatsSummary } from '../../generated/StatsSummary'
import { AXIS, CHART_COLORS, EChart, GRID_LINE, HEAT, INK_MUTED, TOOLTIP, TRACK } from '../../EChart'
import { Panel, fmtCompact, n } from './parts'

// The long view (#367): everything here is all-history and engine-side,
// and none of it follows the range.

const WEEKDAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun']
// A category y-axis fills bottom-up; reversed so Monday sits on top.
const HEAT_ROWS = [...WEEKDAYS].reverse()

export function HistorySection({ s, i, range }: { s: StatsSummary | undefined; i: InsightsSummary | undefined; range: string | null }) {
  const days = i?.events_per_day ?? []
  return (
    <>
      {days.length > 0 && (
        <Panel title="The work calendar" scope="all" range={range} note="messages by the day they were written — the agent's own clock" mb="md">
          <EChart
            height={190}
            option={{
              tooltip: { ...TOOLTIP, formatter: (p: { value: [string, number] }) => `${p.value[0]}<br/>${fmtCompact(p.value[1])} messages` },
              visualMap: {
                min: 0,
                max: Math.max(...days.map((d) => n(d.value)), 1),
                orient: 'horizontal',
                left: 44,
                bottom: 4,
                itemWidth: 10,
                itemHeight: 140,
                text: ['busy', 'quiet'],
                textStyle: { color: INK_MUTED, fontSize: 10 },
                inRange: { color: HEAT },
              },
              calendar: {
                top: 26,
                left: 44,
                cellSize: [17, 17],
                range: [days[0].t, days[days.length - 1].t],
                itemStyle: { color: '#150420', borderColor: GRID_LINE, borderWidth: 1 },
                yearLabel: { show: false },
                monthLabel: { color: INK_MUTED, fontSize: 11 },
                dayLabel: { color: INK_MUTED, fontSize: 10, firstDay: 1, nameMap: ['Su', 'Mo', 'Tu', 'We', 'Th', 'Fr', 'Sa'] },
                splitLine: { lineStyle: { color: GRID_LINE } },
              },
              series: [{ type: 'heatmap', coordinateSystem: 'calendar', data: days.map((d) => [d.t, n(d.value)]) }],
            }}
          />
        </Panel>
      )}

      <Grid mb="md" gap="md">
        <Grid.Col span={{ base: 12, lg: 7 }}>
          <Panel title="When the agents work" scope="all" range={range} note="hour of day × day of week" h="100%">
            <EChart
              height={240}
              option={{
                tooltip: {
                  ...TOOLTIP,
                  formatter: (p: { value: [number, number, number] }) =>
                    `${HEAT_ROWS[p.value[1]]} ${String(p.value[0]).padStart(2, '0')}:00<br/>${fmtCompact(p.value[2])} messages`,
                },
                grid: { left: 46, right: 14, top: 10, bottom: 40 },
                xAxis: { type: 'category', data: Array.from({ length: 24 }, (_, h) => String(h).padStart(2, '0')), ...AXIS, splitLine: { show: false } },
                yAxis: { type: 'category', data: HEAT_ROWS, ...AXIS, splitLine: { show: false } },
                visualMap: { min: 0, max: Math.max(...(i?.hour_weekday ?? []).map((c) => n(c.value)), 1), show: false, inRange: { color: HEAT } },
                series: [
                  {
                    type: 'heatmap',
                    data: (i?.hour_weekday ?? []).map((c) => [n(c.hour), 7 - n(c.weekday), n(c.value)]),
                    itemStyle: { borderColor: '#1F062A', borderWidth: 1 },
                  },
                ],
              }}
            />
          </Panel>
        </Grid.Col>
        <Grid.Col span={{ base: 12, lg: 5 }}>
          <Panel title="Which models did the work" scope="all" range={range} note="messages" h="100%">
            <EChart
              height={240}
              option={{
                tooltip: { ...TOOLTIP, trigger: 'item' },
                legend: { orient: 'vertical', right: 0, top: 'middle', textStyle: { color: INK_MUTED, fontSize: 11 } },
                series: [
                  {
                    type: 'pie',
                    radius: ['52%', '78%'],
                    center: ['32%', '50%'],
                    itemStyle: { borderColor: TRACK, borderWidth: 2 },
                    label: { show: false },
                    data: (i?.models ?? [])
                      .filter((m) => !m.name.startsWith('<'))
                      .slice(0, 8)
                      .map((m) => ({ name: m.name, value: n(m.value) })),
                  },
                ],
              }}
            />
          </Panel>
        </Grid.Col>
      </Grid>

      <Grid mb="md" gap="md">
        <Grid.Col span={{ base: 12, lg: 7 }}>
          <Panel title="Activity — events per minute" scope="all" range={range} note="the newest two thousand events" h="100%">
            <EChart
              height={200}
              option={{
                grid: { left: 44, right: 12, top: 16, bottom: 28 },
                tooltip: { trigger: 'axis', ...TOOLTIP },
                xAxis: { type: 'category', data: (s?.events_per_minute ?? []).map((p) => p.t), ...AXIS, splitLine: { show: false } },
                yAxis: { type: 'value', ...AXIS },
                series: [
                  {
                    type: 'line',
                    data: (s?.events_per_minute ?? []).map((p) => n(p.value)),
                    smooth: 0.3,
                    symbol: 'none',
                    lineStyle: { width: 2, color: '#B833E8' },
                    areaStyle: { color: '#B833E8', opacity: 0.18 },
                  },
                ],
              }}
            />
          </Panel>
        </Grid.Col>
        <Grid.Col span={{ base: 12, lg: 5 }}>
          <Panel title="Who did the work" scope="all" range={range} note="messages by agent" h="100%">
            <EChart
              height={200}
              option={{
                tooltip: { ...TOOLTIP, trigger: 'axis', axisPointer: { type: 'shadow' }, valueFormatter: (v: number) => fmtCompact(v) },
                grid: { left: 110, right: 46, top: 8, bottom: 24 },
                xAxis: { type: 'value', ...AXIS, axisLabel: { ...AXIS.axisLabel, show: false } },
                yAxis: { type: 'category', data: (i?.per_agent ?? []).map((a) => a.name).reverse(), ...AXIS, splitLine: { show: false } },
                series: [
                  {
                    type: 'bar',
                    barWidth: 16,
                    itemStyle: { borderRadius: 3 },
                    label: { show: true, position: 'right', color: INK_MUTED, fontSize: 11, formatter: (p: { value: number }) => fmtCompact(p.value) },
                    data: (i?.per_agent ?? []).map((a, k) => ({ value: n(a.messages), itemStyle: { color: CHART_COLORS[k % CHART_COLORS.length] } })).reverse(),
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
