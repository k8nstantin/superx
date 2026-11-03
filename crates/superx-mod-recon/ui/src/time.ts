// Times as the tab writes them: in the operator's own time zone (operator,
// 2026-11-03: "speak in time for my time zone"), and spans in h, m and s.

const two = (v: number) => String(v).padStart(2, '0')

/// `2026-11-03T20:42:56Z` as `16:42:56`, in the operator's own time.
export const clock = (iso: string | null | undefined) => {
  if (!iso) return '—'
  const d = new Date(iso)
  return `${two(d.getHours())}:${two(d.getMinutes())}:${two(d.getSeconds())}`
}

/// `2026-11-03T20:42:56Z` as `11-03 16:42`, in the operator's own time.
export const day = (iso: string | null | undefined) => {
  if (!iso) return '—'
  const d = new Date(iso)
  return `${two(d.getMonth() + 1)}-${two(d.getDate())} ${two(d.getHours())}:${two(d.getMinutes())}`
}

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
