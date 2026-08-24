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
import type { GraphView } from './generated/GraphView'
import type { LabelView } from './generated/LabelView'
import type { SlotView } from './generated/SlotView'
import type { VocabularyView } from './generated/VocabularyView'
import type { LabelReq } from './generated/LabelReq'
import type { SlotReq } from './generated/SlotReq'

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

// EU5 — the subgraph rooted at ONE entity.
export const fetchGraph = (frag: string, depth: number, direction: string) =>
  get<GraphView>(`/api/entities/${frag}/graph?depth=${depth}&direction=${direction}`)

// EU4 — the bytes ARE the body; the name rides in the query string, so
// there is no multipart boundary on either side of the wire.
export async function attachFile(frag: string, file: File): Promise<{ id: string }> {
  const r = await fetch(
    `/api/entities/${frag}/attach?name=${encodeURIComponent(file.name)}`,
    { method: 'POST', body: file },
  )
  if (!r.ok) throw new Error(await errText(r))
  return r.json() as Promise<{ id: string }>
}

export const downloadUrl = (id: string) => `/api/attachments/${id}/download`

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

// ── The dictionary, designable (#292) ────────────────────────────────
// types -> labels -> entities is the order everything else depends on,
// so all three are edited from the same surface.

export const fetchLabels = (archived = false) =>
  get<LabelView[]>(`/api/labels${archived ? '?archived=true' : ''}`)

/// Every closed vocabulary comes from the substrate, never from a list
/// hardcoded in the frontend — the dictionary owns what a semantics or a
/// kind may be, and a second copy here would rot the moment it changes.
export const fetchVocabulary = () => get<VocabularyView>('/api/vocabulary')

export const defineLabel = (req: LabelReq) =>
  post<{ key: string }>('/api/labels', req)

export const fetchSlots = (type: string, retired = false) =>
  get<SlotView[]>(`/api/types/${encodeURIComponent(type)}/slots${retired ? '?retired=true' : ''}`)

export const bindSlot = (type: string, req: SlotReq) =>
  post<{ type: string; label: string }>(`/api/types/${encodeURIComponent(type)}/slots`, req)
