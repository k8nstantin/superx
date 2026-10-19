// Typed fetchers over the specs service. The types are GENERATED from the
// Rust structs (ts-rs).
import type { TaskLedger } from './generated/TaskLedger'

async function get<T>(path: string): Promise<T> {
  const r = await fetch(path)
  if (!r.ok) throw new Error(`${path}: ${r.status} — ${await r.text()}`)
  return r.json() as Promise<T>
}

export type Ping = { module: string; version: string; core_url: string | null }

export const fetchPing = () => get<Ping>('/api/ping')
export const fetchLedger = () => get<TaskLedger[]>('/api/ledger')
