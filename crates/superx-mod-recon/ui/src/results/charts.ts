import { AXIS, CANCEL, FAIL, GRID_LINE, INK, INK_MUTED, OK, TOOLTIP } from '../EChart'
import type { Part } from './shared'
import { compact } from './shared'

// The pieces every Results chart is built of, so a view says only what it
// draws: a category axis, a value axis, a legend, a tooltip, and the fixed
// colours of a commit's outcome.

export const PART_COLOUR: Record<Part, string> = { alive: OK, replaced: '#3987e5', flight: CANCEL, never: FAIL }

export const grid = (extra: Record<string, unknown> = {}) => ({ left: 8, right: 18, top: 40, bottom: 8, containLabel: true, ...extra })

export const legend = () => ({ top: 0, left: 0, type: 'scroll', textStyle: { color: INK, fontSize: 11 }, itemWidth: 12, itemHeight: 9, pageTextStyle: { color: INK_MUTED } })

export const category = (data: string[], extra: Record<string, unknown> = {}) => ({ type: 'category', data, ...AXIS, splitLine: { show: false }, ...extra })

export const value = (format?: (v: number) => string, extra: Record<string, unknown> = {}) => ({
  type: 'value',
  ...AXIS,
  axisLabel: { ...AXIS.axisLabel, formatter: format ?? ((v: number) => compact(v)) },
  splitLine: { lineStyle: { color: GRID_LINE, opacity: 0.5 } },
  ...extra,
})

export const percent = (extra: Record<string, unknown> = {}) => value((v) => `${v}%`, { max: 100, ...extra })

export const tip = (extra: Record<string, unknown> = {}) => ({ trigger: 'axis', axisPointer: { type: 'shadow' }, ...TOOLTIP, ...extra })

/// A bar series of one stack, its segments parted by the surface.
export const stacked = (name: string, data: unknown[], color: string, stack = 's', extra: Record<string, unknown> = {}) => ({
  name,
  type: 'bar',
  stack,
  data,
  barMaxWidth: 40,
  itemStyle: { color, borderColor: '#2A1235', borderWidth: 1 },
  emphasis: { focus: 'series' },
  ...extra,
})

/// A bar series of a group, its data-end rounded.
export const grouped = (name: string, data: unknown[], color: string, extra: Record<string, unknown> = {}) => ({
  name,
  type: 'bar',
  data,
  barMaxWidth: 26,
  itemStyle: { color, borderRadius: [3, 3, 0, 0] },
  emphasis: { focus: 'series' },
  ...extra,
})

export const zoom = () => [{ type: 'inside' }, { type: 'slider', height: 16, bottom: 4, borderColor: GRID_LINE, textStyle: { color: INK_MUTED } }]
