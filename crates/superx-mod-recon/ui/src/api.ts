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

/// A host repository crawled beside a repository in the pass.
export type HostNow = { name: string; calls: number; failed: number; waits: number; asking: string | null; done: boolean }

/// A repository in the pass: waiting, running or done, and where it is.
export type RepoNow = {
  name: string
  common_dir: string
  state: 'waiting' | 'running' | 'done'
  step: string
  step_index: number
  steps: number
  step_since: string | null
  started_at: string | null
  finished_at: string | null
  git_calls: number
  git_failed: number
  blamed: number
  to_blame: number
  outcome: string | null
  hosts: HostNow[]
}

export type PassNow = {
  started_at: string
  finished_at: string | null
  phase: string
  outcome: string | null
  rows: number | null
  lines_read: number | null
  lines_held: number | null
  repos: RepoNow[]
}

/// What the crawler is doing this second, and the last pass.
/// `last_crawl`: the last pass that crawled repositories, which most passes do not.
export type Now = { pass: PassNow | null; last: PassNow | null; last_crawl: PassNow | null }

export const fetchNow = () => fetch('/api/now').then((r) => answer<Now>(r))

export type LastCrawl = {
  started_at: string
  finished_at: string
  outcome: string
  note: string | null
  mainline: string
  tip: string | null
  commits_read: number
  commits_new: number
  refs_read: number
  reflog_read: number
  files_blamed: number
  files_left: number
  git_calls: number
  git_failed: number
  git_timeouts: number
  rows_written: number
}

export type HostLast = {
  name: string
  started_at: string
  finished_at: string
  outcome: string
  note: string | null
  calls: number
  failed: number
  not_modified: number
  rate_waits: number
  objects_new: number
  objects_changed: number
}

/// A repository and its last crawl.
export type Repo = {
  name: string
  common_dir: string
  crawls: number
  commits_held: number
  last: LastCrawl | null
  last_complete: string | null
  hosts: HostLast[]
}

export const fetchRepos = () => fetch('/api/repos').then((r) => answer<{ repos: Repo[] }>(r))

export const fetchParams = () => fetch('/api/params').then((r) => answer<Listing>(r))

/// A change carries the header recon asks of every write (src/api.rs), which
/// no page of another site can send.
export const setParam = (name: string, value: unknown) =>
  fetch(`/api/params/${encodeURIComponent(name)}`, {
    method: 'PUT',
    headers: { 'content-type': 'application/json', 'x-superx-recon': 'write' },
    body: JSON.stringify(value),
  }).then((r) => answer<Listing>(r))

/// What a model did, summed (src/results.rs).
export type Measures = {
  replies: number
  subagent_replies: number
  input_tokens: number
  output_tokens: number
  cache_read_tokens: number
  cache_write_tokens: number
  thinking_tokens: number
  thinking_spans: number
  thinking_ms: number
  tool_calls: number
  lines_added: number
  lines_removed: number
  commits: number
  commit_lines_added: number
  commit_lines_removed: number
}

export type ModelDay = Measures & { day: string; model: string }
export type ModelTotal = Measures & { model: string; sessions: number; first_day: string; last_day: string }
export type Models = { days: ModelDay[]; totals: ModelTotal[] }

export const fetchModels = () => fetch('/api/results/models').then((r) => answer<Models>(r))
