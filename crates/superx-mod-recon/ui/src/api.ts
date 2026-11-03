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


// ---- Results (src/results/): the trail's findings, a window on each
// repository, one commit whole. Times are RFC 3339 in UTC; a model is its
// family ("opus 5.5"), empty where none is credited; a repository is its
// name, empty for work in no repository.

export type DayFile = { path: string; change: string; added: number; removed: number }

/// A commit of ours and what became of it.
export type FoundCommit = {
  repo: string
  hash: string
  subject: string
  at: string
  model: string
  by: 'printed' | 'claim' | null
  session: string | null
  agent: string | null
  added: number
  removed: number
  place: string | null
  in_flight: boolean
  landed: number
  alive: number | null
  never_landed: number
  files: number
  bulk: boolean
  reverted: boolean
  ci: boolean | null
  ci_names: { name: string; verdict: boolean | null }[]
  pulls: number[]
  tested: boolean | null
  tests_green: boolean | null
  built: boolean | null
  effort: string | null
  context: number | null
  subagent: boolean | null
}

/// A pull request and how it went.
export type FoundPull = {
  repo: string
  host: string
  path: string
  number: number
  title: string
  url: string
  created_at: string
  state: string
  merged: boolean
  merged_at: string | null
  closed_at: string | null
  minutes_to_merge: number | null
  additions: number
  deletions: number
  commits: number
  model: string | null
  by: 'opened' | 'commits' | null
  session: string | null
  ci_failed: number
  ci_passed: number
  ci_first: boolean | null
}

/// One model's work in one repository in one week (its Monday, UTC).
export type Week = {
  week: string
  model: string
  repo: string
  replies: number
  subagent_replies: number
  out_tokens: number
  context_tokens: number
  thinking_ms: number
  tool_calls: number
  active_secs: number
  edits: number
  edit_added: number
  edit_removed: number
  operator_changed: number
  tests: number
  tests_failed: number
  builds: number
  builds_failed: number
  prompts: number
  corrections: number
  escalations: number
  interrupts: number
}

export type RepoWeek = { week: string; repo: string; moves: Record<string, number>; rewrites: number; ci_passed: number; ci_failed: number; ci_turned: number }
export type Workflow = { repo: string; name: string; kind: string; runs: number; passed: number; failed: number; turned: number }

export type Findings = {
  commits: FoundCommit[]
  pulls: FoundPull[]
  weeks: Week[]
  repo_weeks: RepoWeek[]
  workflows: Workflow[]
  ci_kinds: Record<string, number>
  computed_at: string
}

export const fetchFindings = () => fetch('/api/results/findings').then((r) => answer<Findings>(r))

/// A repository's numbers in a window.
export type WinTotals = {
  sessions: number
  agents: number
  models: number
  replies: number
  subagent_replies: number
  out_tokens: number
  context_tokens: number
  thinking_ms: number
  tool_calls: number
  active_secs: number
  prompts: number
  corrections: number
  escalations: number
  interrupts: number
  edits: number
  edit_files: number
  edit_added: number
  edit_removed: number
  commits: number
  commit_added: number
  commit_removed: number
  commits_mainline: number
  commits_in_flight: number
  commits_branch: number
  commits_recovered: number
  landed: number
  alive: number
  reverted: number
  prs_opened: number
  prs_merged: number
  prs_closed: number
  ci_passed: number
  ci_failed: number
  moves: number
  rewrites: number
  pushes: number
  tests: number
  tests_failed: number
  builds: number
  builds_failed: number
}

export type WinModel = {
  model: string
  sessions: number
  replies: number
  out_tokens: number
  context_tokens: number
  thinking_ms: number
  tool_calls: number
  active_secs: number
  edit_added: number
  edit_removed: number
  commits: number
  commit_added: number
  commit_removed: number
  landed: number
  alive: number
  prompts: number
  corrections: number
  escalations: number
  tests: number
  tests_failed: number
}

export type Step = {
  start: string
  model: string
  replies: number
  out_tokens: number
  tool_calls: number
  edit_added: number
  edit_removed: number
  commits: number
  commit_added: number
  commit_removed: number
  prompts: number
}

export type WinSession = {
  session: string
  agent: string
  models: string[]
  first: string | null
  last: string | null
  replies: number
  tool_calls: number
  out_tokens: number
  active_secs: number
  edit_added: number
  edit_removed: number
  prompts: number
  commits: number
  tests: number
  builds: number
}

export type WinFile = { path: string; models: string[]; edits: number; edit_added: number; edit_removed: number; commits: number; commit_added: number; commit_removed: number }

export type WinCommit = {
  hash: string
  subject: string
  at: string
  author_name: string
  ours: boolean
  squash: boolean
  model: string
  by: 'printed' | 'claim' | null
  session: string | null
  added: number
  removed: number
  place: string | null
  in_flight: boolean
  landed: number
  alive: number | null
  never_landed: number
  reverted: boolean
  ci: boolean | null
  pulls: number[]
  tested: boolean | null
  files: DayFile[]
}

export type WinPrompt = { at: string; session: string | null; model: string; kind: string; text: string; correction: boolean; escalation: boolean }
export type WinPull = {
  number: number
  title: string
  url: string
  state: string
  merged: boolean
  created_at: string
  merged_at: string | null
  closed_at: string | null
  additions: number
  deletions: number
  model: string | null
  ci_failed: number
  ci_passed: number
  ci_first: boolean | null
  happened: string[]
}
export type WinRun = {
  name: string
  kind: string
  verdict: boolean | null
  failed_before: boolean
  status: string | null
  conclusion: string | null
  created_at: string | null
  completed_at: string | null
  sha: string | null
  git_ref: string | null
  pull: number | null
  url: string | null
}
export type WinMove = { at: string; git_ref: string; kind: string; action: string; identity: string | null; old: string | null; new: string | null; mirror: boolean }
export type WinCheck = { at: string; test: boolean; what: string; command: string; passed: number; failed: number; ok: boolean; session: string | null; model: string }
export type WinGit = { at: string; action: string; hash: string | null; branch: string | null; subject: string | null; pr_number: number | null; url: string | null; session: string | null; model: string }

export type WinRepo = {
  repo: string
  totals: WinTotals
  agents: Record<string, number>
  models: WinModel[]
  timeline: Step[]
  sessions: WinSession[]
  files: WinFile[]
  commits: WinCommit[]
  prompts: WinPrompt[]
  pulls: WinPull[]
  ci: WinRun[]
  moves: WinMove[]
  checks: WinCheck[]
  git: WinGit[]
}

export type Window = { from: string; to: string; step_secs: number; repos: WinRepo[]; computed_at: string }

export const fetchWindow = (from: Date, to: Date, step: number) =>
  fetch(`/api/results/window?${new URLSearchParams({ from: from.toISOString(), to: to.toISOString(), step: String(step) })}`).then((r) =>
    answer<Window>(r),
  )

/// One commit, whole.
export type CommitDetail = {
  repo: string
  hash: string
  subject: string
  at: string
  author_name: string
  model: string
  by: 'printed' | 'claim' | null
  session: string | null
  agent: string | null
  effort: string | null
  context: number | null
  added: number
  removed: number
  place: string | null
  in_flight: boolean
  squash: boolean
  landed: number
  alive: number | null
  never_landed: number
  reverted: boolean
  files: DayFile[]
  ci: { name: string; kind: string; verdict: boolean | null; failed_before: boolean; status: string | null; conclusion: string | null; created_at: string | null; completed_at: string | null; url: string | null }[]
  pulls: { number: number; title: string; url: string; state: string; merged: boolean; created_at: string; merged_at: string | null }[]
  checks: { at: string; test: boolean; what: string; command: string; passed: number; failed: number; ok: boolean }[]
}

export const fetchCommit = (repo: string, hash: string) =>
  fetch(`/api/results/commit?${new URLSearchParams({ repo, hash })}`).then((r) => answer<CommitDetail>(r))
