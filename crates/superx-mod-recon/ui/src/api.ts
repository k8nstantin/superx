// What the tab reads and writes on recon's own URL (src/api.rs).

/// One UTC day of the crawler's passes.
export type Day = {
  day: string
  passes: Record<string, number>
  rows: number
  lines: number
  git_calls: number
  git_failed: number
  host_calls: number
  host_failed: number
}

/// One parameter as it stands.
export type Param = {
  name: string
  value: string
  set: boolean
  wanted: string
  group: string
  decides: string
  input: 'number' | 'number or all' | 'list' | 'refs'
}

export type Listing = {
  params: Param[]
  problems: { name: string; found: string; wanted: string }[]
}

async function answer<T>(r: Response): Promise<T> {
  if (!r.ok) {
    const body = await r.json().catch(() => null)
    throw new Error(body?.error ?? `HTTP ${r.status}`)
  }
  return r.json() as Promise<T>
}

export const fetchStats = () => fetch('/api/stats').then((r) => answer<{ days: Day[] }>(r))

export const fetchParams = () => fetch('/api/params').then((r) => answer<Listing>(r))

/// A change carries the header recon asks of every write (src/api.rs), which
/// no page of another site can send.
export const setParam = (name: string, value: unknown) =>
  fetch(`/api/params/${encodeURIComponent(name)}`, {
    method: 'PUT',
    headers: { 'content-type': 'application/json', 'x-superx-recon': 'write' },
    body: JSON.stringify(value),
  }).then((r) => answer<Listing>(r))
