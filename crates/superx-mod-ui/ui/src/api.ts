// Typed fetchers over the OS's API. The types are GENERATED from the
// Rust structs (ts-rs) — the frontend type-checks against the kernel.
import type { StatusResponse } from './generated/StatusResponse'
import type { AgentView } from './generated/AgentView'
import type { SessionView } from './generated/SessionView'
import type { SseEvent } from './generated/SseEvent'
import type { ActionView } from './generated/ActionView'
import type { ChartsSummary } from './generated/ChartsSummary'

async function get<T>(path: string): Promise<T> {
  const r = await fetch(path)
  if (!r.ok) throw new Error(`${path}: ${r.status}`)
  return r.json() as Promise<T>
}

export const fetchStatus = () => get<StatusResponse>('/api/status')
export const fetchAgents = () => get<AgentView[]>('/api/agents')
export const fetchSessions = (agent?: string) =>
  get<SessionView[]>(`/api/sessions${agent ? `?agent=${agent}` : ''}`)
export const fetchSessionActivity = (id: string, limit = 500) =>
  get<SseEvent[]>(`/api/sessions/${id}/activity?limit=${limit}`)
export const fetchActivity = (limit = 500) => get<SseEvent[]>(`/api/activity?limit=${limit}`)
export const fetchActions = (limit = 50) => get<ActionView[]>(`/api/actions?limit=${limit}`)
export const fetchCharts = () => get<ChartsSummary>('/api/charts/summary')

export async function runCommand(argv: string[]): Promise<{ output: string; is_error: boolean }> {
  const r = await fetch('/api/command', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ argv }),
  })
  return r.json()
}
