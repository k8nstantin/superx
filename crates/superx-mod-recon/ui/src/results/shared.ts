import { CHART_COLORS, INK_MUTED } from '../EChart'
import type { FoundCommit, Week } from '../api'

// What every Results view counts the same way: how a commit's lines split
// into what lives, what was replaced, what may still land and what never
// did; the weeks (each a Monday, UTC, as recon groups them); the models'
// colours, fixed per model across every chart; and the arithmetic.

export const n = (v: number | null | undefined) => (v == null ? '—' : Math.round(v).toLocaleString('en-US'))
export const pct = (a: number, b: number) => (b ? Math.round((100 * a) / b) : 0)
export const kTok = (v: number) => (Math.round(v / 1e3) >= 1000 ? `${(v / 1e6).toFixed(1)}M` : v >= 1e3 ? `${Math.round(v / 1e3)}k` : String(Math.round(v)))
export const compact = (v: number) => (Math.abs(v) >= 1e6 ? `${(v / 1e6).toFixed(1)}M` : Math.abs(v) >= 1e4 ? `${Math.round(v / 1e3)}k` : n(v))

/// The Monday of `iso`'s week, UTC, as `2026-11-02`.
export function weekOf(iso: string): string {
  const d = new Date(iso)
  const back = (d.getUTCDay() + 6) % 7
  return new Date(Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate() - back)).toISOString().slice(0, 10)
}

/// `2026-11-02` as `2 Nov`.
export const weekLabel = (w: string) => new Date(`${w}T00:00:00Z`).toLocaleDateString('en-GB', { day: 'numeric', month: 'short', timeZone: 'UTC' })

/// A time in the operator's own zone, as `3 Nov 14:05`.
export const local = (iso: string | null | undefined) =>
  iso ? new Date(iso).toLocaleString('en-GB', { day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' }) : '—'

/// Each model's colour: the models with most replies take the validated
/// palette in order, the rest share one muted colour, so a model keeps its
/// colour whatever a filter leaves.
export type Palette = { models: string[]; colour: (model: string) => string }

export function palette(weeks: Pick<Week, 'model' | 'replies'>[]): Palette {
  const replies = new Map<string, number>()
  for (const w of weeks) replies.set(w.model, (replies.get(w.model) ?? 0) + w.replies)
  const models = [...replies.keys()].filter((m) => m !== '').sort((a, b) => (replies.get(b) ?? 0) - (replies.get(a) ?? 0))
  const lead = models.slice(0, CHART_COLORS.length - 1)
  return {
    models,
    colour: (m) => {
      const i = lead.indexOf(m)
      return i >= 0 ? CHART_COLORS[i] : INK_MUTED
    },
  }
}

/// How a set of commits' lines split. `committed` is every line they added;
/// `landed` the lines of files whose versions the main line carries; of
/// those, `alive` is what blame credits today and `replaced` the rest;
/// `flight` what may still land (a checked-out branch or an open pull
/// request); `never` the lines of files that never reached the main line.
/// `decided` is committed less what is in flight: the rates divide by it,
/// so open work counts neither for a model nor against it.
export type Outcome = { commits: number; committed: number; decided: number; landed: number; alive: number; replaced: number; flight: number; never: number }

export function outcome(cs: FoundCommit[]): Outcome {
  const o: Outcome = { commits: 0, committed: 0, decided: 0, landed: 0, alive: 0, replaced: 0, flight: 0, never: 0 }
  for (const c of cs) {
    o.commits += 1
    o.committed += c.added
    o.landed += c.landed
    const alive = Math.min(c.alive ?? 0, c.landed)
    o.alive += alive
    o.replaced += c.landed - alive
    if (c.in_flight) o.flight += Math.max(0, c.added - c.landed)
    else if (c.place !== 'mainline') o.never += c.never_landed
  }
  o.decided = Math.max(0, o.committed - o.flight)
  return o
}

/// Which part of an outcome a commit counts in, for a drill-down.
export const parts = ['alive', 'replaced', 'flight', 'never'] as const
export type Part = (typeof parts)[number]
export const partName: Record<Part, string> = {
  alive: 'landed, still alive',
  replaced: 'landed, since replaced',
  flight: 'in flight',
  never: 'never landed',
}
export function inPart(c: FoundCommit, p: Part): boolean {
  switch (p) {
    case 'alive':
      return (c.alive ?? 0) > 0
    case 'replaced':
      return c.landed > (c.alive ?? 0)
    case 'flight':
      return c.in_flight && c.added > c.landed
    case 'never':
      return !c.in_flight && c.place !== 'mainline' && c.never_landed > 0
  }
}

/// Pearson's r; null below three points or with no spread.
export function correlation(xs: number[], ys: number[]): number | null {
  if (xs.length < 3) return null
  const mx = xs.reduce((a, b) => a + b, 0) / xs.length
  const my = ys.reduce((a, b) => a + b, 0) / ys.length
  let sxy = 0
  let sx = 0
  let sy = 0
  xs.forEach((x, i) => {
    sxy += (x - mx) * (ys[i] - my)
    sx += (x - mx) ** 2
    sy += (ys[i] - my) ** 2
  })
  return sx && sy ? sxy / Math.sqrt(sx * sy) : null
}

export function quantile(sorted: number[], q: number): number {
  if (!sorted.length) return 0
  const at = (sorted.length - 1) * q
  const lo = Math.floor(at)
  const hi = Math.ceil(at)
  return sorted[lo] + (sorted[hi] - sorted[lo]) * (at - lo)
}

/// Every key of rows, grouped.
export function group<T>(rows: T[], key: (r: T) => string): Map<string, T[]> {
  const out = new Map<string, T[]>()
  for (const r of rows) {
    const k = key(r)
    const list = out.get(k)
    if (list) list.push(r)
    else out.set(k, [r])
  }
  return out
}

export const sum = <T,>(rows: T[], f: (r: T) => number) => rows.reduce((a, r) => a + f(r), 0)

/// A model's name at the start of a sentence.
export const cap = (m: string) => m.charAt(0).toUpperCase() + m.slice(1)

/// The keys whose rows are at least as many as the middle key's: the ones
/// with enough evidence to be ranked, by the data's own middle.
export function enough<T>(groups: Map<string, T[]>): Set<string> {
  const counts = [...groups.values()].map((v) => v.length).sort((a, b) => a - b)
  const middle = quantile(counts, 0.5)
  return new Set([...groups.entries()].filter(([, v]) => v.length >= middle).map(([k]) => k))
}
