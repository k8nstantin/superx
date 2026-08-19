import ReactECharts from 'echarts-for-react'

// Apache ECharts is the SuperX charting standard (issue #228, operator
// directive — proven by the openpraxis DAG pattern). This wrapper
// bakes in the swindex dark defaults so every chart matches the shell.

// Chart series palette: pelican-led rotation of the validated ramp —
// script-validated (dataviz method) against the swindex card surface
// #2A1235: all checks pass (lead #B833E8 = pelican[4]; #CC66FF sits
// above the dark lightness band).
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

export const INK = '#CFC2DB' // swindex dark[1]
export const INK_MUTED = '#8F7BA5' // dark[3]
export const GRID_LINE = '#3B2449' // dark[5]

export const AXIS = {
  axisLine: { lineStyle: { color: GRID_LINE } },
  axisTick: { show: false },
  axisLabel: { color: INK_MUTED, fontSize: 11 },
  splitLine: { lineStyle: { color: GRID_LINE, opacity: 0.5 } },
}

export const TOOLTIP = {
  backgroundColor: '#150420',
  borderColor: GRID_LINE,
  textStyle: { color: INK, fontSize: 12 },
}

export function EChart({ option, height }: { option: Record<string, unknown>; height: number }) {
  const merged = {
    color: [...CHART_COLORS],
    backgroundColor: 'transparent',
    textStyle: {
      color: INK,
      fontFamily: 'system-ui, -apple-system, Segoe UI, Arial, sans-serif',
    },
    ...option,
  }
  return <ReactECharts option={merged} style={{ height }} notMerge lazyUpdate />
}
