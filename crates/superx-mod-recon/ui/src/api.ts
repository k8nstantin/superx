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

/// What is behind one model's bar on one UTC day (src/results.rs `day`).
export type DayFile = { path: string; change: string; added: number; removed: number }
export type DayCommit = {
  hash: string
  repo: string
  subject: string
  committed_at: string | null
  added: number
  removed: number
  place: string | null
  landed: boolean
  alive: number | null
  session: string | null
  files: DayFile[]
}
export type DayEdit = { at: string | null; path: string; kind: string; added: number; removed: number; tool: string; session: string | null; operator_changed: boolean }
export type DaySession = { session: string; agent: string; replies: number; first: string | null; last: string | null }
export type ModelDayDetail = { model: string; day: string; commits: DayCommit[]; edits: DayEdit[]; sessions: DaySession[] }

export const fetchDay = (model: string, day: string) =>
  fetch(`/api/results/day?model=${encodeURIComponent(model)}&day=${encodeURIComponent(day)}`).then((r) => answer<ModelDayDetail>(r))

/// The model comparison (src/results/compare.rs): mod-ui's measures, from
/// recon's facts.
export type Deviation = {
  model: string
  commits: number
  added: number
  removed: number
  alive: number
  survived_pct: number
  removed_per_100_added: number
  rework_commits: number
  rework_pct: number
  thrash_files: number
  thrash_per_100_commits: number
  multi_dir_commits: number
  multi_dir_pct: number
  dirs_per_commit_x10: number
  operator_turns: number
  corrections: number
  corrections_per_100: number
  escalations: number
  out_tokens: number
  messages: number
  runs: number
  thrown: number
  tokens_thrown: number
  tokens_per_line_landed: number
  tokens_per_line_kept: number
  median_age_days: number
  context_avg: number
  context_peak: number
  minutes: number
  minutes_thrown: number
  alive_per_mtok: number
  alive_per_hour: number
  abandoned_commits: number
  abandoned_lines: number
  abandoned_pct: number
  in_flight_lines: number
}
export type Handoff = { from: string; to: string; switches: number; commits: number; added: number; removed: number; alive: number; survived_pct: number }
export type RepoModel = { repo: string; model: string; commits: number; added: number; alive: number; removed: number; survived_pct: number; rework_commits: number }
export type UnjudgedRepo = { repo: string; mainline: string; off_mainline_commits: number; unresolved: boolean }
export type CompareSummary = { handoffs: Handoff[]; deviations: Deviation[]; repos: RepoModel[]; unjudged: UnjudgedRepo[]; computed_at: string }

export const fetchCompare = () => fetch('/api/results/compare').then((r) => answer<CompareSummary>(r))
