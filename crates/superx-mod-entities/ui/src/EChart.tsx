import ReactECharts from 'echarts-for-react'

// Apache ECharts is the SuperX charting standard (operator directive,
// issue #228). Each module UI carries its own copy of the wrapper —
// modules are self-contained, so the shared thing is the design
// system, not a package.

export const CHART_COLORS = [
  '#B833E8', // pelican
  '#e66767', // red
  '#3987e5', // blue
  '#d95926', // orange
  '#199e70', // aqua
  '#c98500', // ochre
  '#d55181', // magenta
  '#008300', // green
] as const

export const INK = '#CFC2DB'
export const INK_MUTED = '#8F7BA5'
export const GRID_LINE = '#3B2449'

export const TOOLTIP = {
  backgroundColor: '#150420',
  borderColor: GRID_LINE,
  textStyle: { color: INK, fontSize: 12 },
}

export function EChart({
  option,
  height,
  onEvents,
}: {
  option: Record<string, unknown>
  height: number
  onEvents?: Record<string, (params: never) => void>
}) {
  const merged = {
    color: [...CHART_COLORS],
    backgroundColor: 'transparent',
    textStyle: { color: INK, fontFamily: 'system-ui, -apple-system, Segoe UI, Arial, sans-serif' },
    ...option,
  }
  return (
    <ReactECharts
      option={merged}
      style={{ height }}
      notMerge
      lazyUpdate
      onEvents={onEvents}
    />
  )
}
