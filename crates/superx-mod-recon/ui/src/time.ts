// Times as the tab writes them: UTC clock times, and spans in h, m and s.

/// `2026-11-03T16:42:56Z` as `16:42:56`.
export const clock = (iso: string | null | undefined) => (iso ? new Date(iso).toISOString().slice(11, 19) : '—')

/// `2026-11-03T16:42:56Z` as `11-03 16:42`.
export const day = (iso: string | null | undefined) => (iso ? new Date(iso).toISOString().slice(5, 16).replace('T', ' ') : '—')

/// A span in milliseconds as `1h 2m`, `3m 12s` or `41s`.
export function span(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000))
  const h = Math.floor(s / 3600)
  const m = Math.floor((s % 3600) / 60)
  if (h > 0) return `${h}h ${m}m`
  if (m > 0) return `${m}m ${s % 60}s`
  return `${s}s`
}

/// The span from `iso` to `until`, or to now.
export const since = (iso: string | null | undefined, until?: string | null) =>
  iso ? span((until ? new Date(until).getTime() : Date.now()) - new Date(iso).getTime()) : '—'
