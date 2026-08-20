// Typed fetchers over the entities module's API. The types are
// GENERATED from the Rust structs (ts-rs) — the frontend type-checks
// against the module.
import type { TypeView } from './generated/TypeView'
import type { EntityListItem } from './generated/EntityListItem'
import type { EntityDetail } from './generated/EntityDetail'
import type { VersionView } from './generated/VersionView'
import type { CreateReq } from './generated/CreateReq'
import type { UpdateReq } from './generated/UpdateReq'
import type { LinkReq } from './generated/LinkReq'
import type { TypeReq } from './generated/TypeReq'

async function get<T>(path: string): Promise<T> {
  const r = await fetch(path)
  if (!r.ok) throw new Error(await errText(r))
  return r.json() as Promise<T>
}

async function post<T>(path: string, body: unknown): Promise<T> {
  const r = await fetch(path, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  })
  if (!r.ok) throw new Error(await errText(r))
  return r.json() as Promise<T>
}

async function errText(r: Response): Promise<string> {
  try {
    const j = await r.json()
    return typeof j?.error === 'string' ? j.error : `${r.status}`
  } catch {
    return `${r.status}`
  }
}

export const fetchPing = () =>
  get<{ module: string; version: string; core_url: string | null }>('/api/ping')
export const fetchTypes = () => get<TypeView[]>('/api/types')
export const addType = (req: TypeReq) => post<{ name: string }>('/api/types', req)
export const fetchRelTypes = () => get<string[]>('/api/rel-types')
export const fetchEntities = (type?: string) =>
  get<EntityListItem[]>(`/api/entities${type ? `?type=${encodeURIComponent(type)}` : ''}`)
export const createEntity = (req: CreateReq) => post<{ id: string }>('/api/entities', req)
export const fetchDetail = (frag: string) => get<EntityDetail>(`/api/entities/${frag}`)
export const fetchHistory = (frag: string) => get<VersionView[]>(`/api/entities/${frag}/history`)
export const updateEntity = (frag: string, req: UpdateReq) =>
  post<{ id: string }>(`/api/entities/${frag}/update`, req)
export const describeEntity = (frag: string, text: string) =>
  post<{ text_id: string }>(`/api/entities/${frag}/describe`, { text })
export const commentEntity = (frag: string, text: string) =>
  post<{ text_id: string }>(`/api/entities/${frag}/comment`, { text })
export const linkEntity = (frag: string, req: LinkReq) =>
  post<{ edge_uid: string }>(`/api/entities/${frag}/link`, req)
export const unlinkEntity = (frag: string, req: LinkReq) =>
  post<{ edge_uid: string }>(`/api/entities/${frag}/unlink`, req)

// Every entity type wears a stable color from the validated ramp
// (project UI standard) — hashed by name so runtime-added types get
// one automatically.
const TYPE_COLORS = [
  '#3987e5',
  '#d95926',
  '#199e70',
  '#c98500',
  '#d55181',
  '#008300',
  '#9085e9',
  '#e66767',
] as const

export function typeColor(name: string): string {
  let h = 0
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) >>> 0
  return TYPE_COLORS[h % TYPE_COLORS.length]
}
