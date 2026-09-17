// Typed fetchers over the OS's API. The types are GENERATED from the
// Rust structs (ts-rs) — the frontend type-checks against the kernel.
import type { StatusResponse } from './generated/StatusResponse'
import type { AgentView } from './generated/AgentView'
import type { SessionView } from './generated/SessionView'
import type { SseEvent } from './generated/SseEvent'
import type { ActionView } from './generated/ActionView'
import type { ChartsSummary } from './generated/ChartsSummary'
import type { StatsSummary } from './generated/StatsSummary'
import type { InsightsSummary } from './generated/InsightsSummary'
import type { ThrownSummary } from './generated/ThrownSummary'
import type { CompareSummary } from './generated/CompareSummary'

async function get<T>(path: string): Promise<T> {
  const r = await fetch(path)
  if (!r.ok) {
    // The server's error body carries the engine's own words
    // (`{ is_error, output }`); a bare status hides them (#367).
    let detail = ''
    try {
      const body = (await r.json()) as { output?: unknown }
      if (typeof body.output === 'string') detail = body.output
    } catch {
      /* not JSON — the status is all there is */
    }
    throw new Error(detail ? `${path}: ${r.status} — ${detail}` : `${path}: ${r.status}`)
  }
  return r.json() as Promise<T>
}

export const fetchStatus = () => get<StatusResponse>('/api/status')
export const fetchAgents = () => get<AgentView[]>('/api/agents')
export const fetchSessions = (agent?: string) =>
  get<SessionView[]>(`/api/sessions${agent ? `?agent=${agent}` : ''}`)
// `before` (RFC3339) walks BACKWARDS: the newest page strictly older
// than that instant — how the feeds scroll into history. `q` filters
// in the ENGINE, so it searches all history rather than the loaded
// page (issue #241).
const feedArgs = (before?: string, q?: string) =>
  (before ? `&before=${encodeURIComponent(before)}` : '') +
  (q && q.trim() ? `&q=${encodeURIComponent(q.trim())}` : '')
export const fetchSessionActivity = (id: string, limit = 500, before?: string, q?: string) =>
  get<SseEvent[]>(`/api/sessions/${id}/activity?limit=${limit}${feedArgs(before, q)}`)
export const fetchActivity = (limit = 500, before?: string, q?: string) =>
  get<SseEvent[]>(`/api/activity?limit=${limit}${feedArgs(before, q)}`)
export const fetchActions = (limit = 50) => get<ActionView[]>(`/api/actions?limit=${limit}`)
export const fetchCharts = () => get<ChartsSummary>('/api/charts/summary')
export const fetchStats = (range = 'window') =>
  get<StatsSummary>(`/api/stats?range=${encodeURIComponent(range)}`)
export const fetchInsights = () => get<InsightsSummary>('/api/insights')
/// What each model's work threw away (#406). Read from git rather than
/// the transcript, and cached server-side for minutes — it moves when
/// commits land, not when the page polls.
export const fetchThrown = () => get<ThrownSummary>('/api/thrown')
/// Model comparison (#406): the switches, and whether each model stayed
/// on the objective. Same git-backed read, same server-side cache.
export const fetchCompare = () => get<CompareSummary>('/api/compare')

export async function runCommand(argv: string[]): Promise<{ output: string; is_error: boolean }> {
  const r = await fetch('/api/command', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ argv }),
  })
  return r.json()
}
