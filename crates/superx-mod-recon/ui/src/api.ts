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

/// The stats by the operator's own days: the browser's offset, in minutes
/// east of UTC.
export const fetchStats = () => fetch(`/api/stats?offset=${-new Date().getTimezoneOffset()}`).then((r) => answer<{ days: Day[] }>(r))

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

/// Which module answers, and where the core dashboard lives, for the way
/// back (#562); `core_url` is null when it is not in the registry.
export type Ping = { module: string; version: string; core_url: string | null }

export const fetchPing = () => fetch('/api/ping').then((r) => answer<Ping>(r))

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
  /// mainline, squash, landed, in_flight, side, branch, reset or snapshot.
  fate: Fate
  side: string | null
  in_flight: boolean
  landed_as: string | null
  landed: number
  /// What blame credits it today; null where its lines were never blamed.
  alive: number | null
  never_landed: number
  /// Of the lines it landed, those in files the main line changed again
  /// since: put to the test. The rest sit in files nothing touched since.
  landed_changed: number
  files: number
  bulk: boolean
  /// A merge: the merged commits carry its lines, so it authored none.
  merge: boolean
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
  target_branch: string
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
  folders: { folder: string; repo: string }[]
  /// The main line each repository's newest crawl read, how it was chosen,
  /// and its newest commit: how long the repository has been parked.
  mainlines: { repo: string; mainline: string; from: string; last_change: string | null }[]
  branches: BranchWeek[]
  files: FoundFile[]
  computed_at: string
}

/// One file's share of the work: one model's credited commits that changed
/// it, by how they were credited, and what became of their lines in it.
export type FoundFile = {
  repo: string
  path: string
  model: string
  printed: boolean
  /// The commits, as their places in Findings.commits.
  commits: number[]
  added: number
  landed: number
  flight: number
  side: number
  never: number
  reset: number
  /// The main-line commits that changed it, anyone's, and the newest.
  main_changes: number
  last_main_change: string | null
}

/// Where a commit's work went.
export type Fate = 'mainline' | 'squash' | 'landed' | 'in_flight' | 'side' | 'branch' | 'reset' | 'snapshot'

/// One branch of a repository in one week, from its reflog.
export type BranchWeek = {
  repo: string
  branch: string
  week: string
  commits: number
  lines: number
  landed: number
  never: number
  merges: number
  rewrites: number
  updates: number
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
  commits_landed: number
  commits_in_flight: number
  commits_side: number
  commits_branch: number
  commits_recovered: number
  commits_snapshot: number
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
  fate: Fate
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
  fate: Fate
  side: string | null
  in_flight: boolean
  squash: boolean
  landed_as: string | null
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

/// Where a session's time went, in seconds (src/watcher/track.rs).
export type Split = { thinking: number; writing: number; tools: number; jobs: number; operator: number; approval: number; api: number }
export type Counts = { prompts: number; calls: number; launches: number }

export type AlertKind =
  | 'run_again'
  | 'long_run'
  | 'stop_not_followed'
  | 'plan_repeats'
  | 'endless_task'
  | 'task_file_broken'
  | 'build_stopped'
  | 'unproven_merge'
  | 'api_stalled'
  | 'count_gap'
  | 'acted_alone'
  | 'said_false'
  | 'got_wrong'

/// A stretch the API did not answer a session (src/watcher/track.rs): from
/// its first request with no answer to the answer, if one came.
export type Unanswered = {
  since: string
  last: string
  sent: number
  resent: number
  first_resent: string | null
  model: string | null
  effort: string | null
  interrupted: string | null
  failed_at: string | null
  error: string | null
  failures: number
  turn_ended: string | null
  answered: string | null
}

/// Another session the API left without an answer in the same minutes.
export type Alongside = { session: string; name: string; since: string; until: string }

/// One of the watcher's alerts (#573), each with where its cause starts and
/// the alert that caused it; the alarms are the kinds `alarms` names.
export type WatchAlert = {
  id: string
  session: string
  name: string
  kind: AlertKind
  at: string
  what: string
  since: string | null
  cost_secs: number | null
  evidence: string[]
  call: string | null
  from: string
  launch: string | null
  repeats: { job: string; times: number; by: string[] }[]
  cause: string | null
  stretches: Unanswered[]
  alongside: Alongside[]
}

export type Doing = { tool: string; what: string; since: string; expected_end: string | null }

export type WatchNow = {
  id: string
  name: string
  model: string | null
  folder: string | null
  newest: string
  state: 'working' | 'waiting on the API' | 'job running' | 'waiting on you' | 'waiting on your approval' | 'idle'
  api: Unanswered | null
  doing: Doing | null
  jobs: Doing[]
}

export type WatchSessionRow = { id: string; name: string; model: string | null; folder: string | null; first: string; newest: string }

export type WatchRun = {
  session: string
  name: string
  model: string | null
  call: string
  tool: string
  target: string
  label: string
  what: string
  launch: string | null
  start: string
  end: string | null
  secs: number
  background: boolean
  failed: boolean
  status: string | null
}

export type WatchTask = {
  session: string
  name: string
  branch: string
  tickets: string[]
  asked: string
  last: string
  estimates: { at: string; secs: number; words: string }[]
  /// Each merge: when it was seen, its pull request and merge commit when
  /// the answer named them, and whether a spec gate passed after its task
  /// file's last change (null when the session changed no task file).
  merges: { at: string; pull: number | null; commit: string | null; proven: boolean | null }[]
  reopened: [string, string][]
  /// The branches made for it after a merge, newest last.
  versions: string[]
  /// What the agent said was not done once it had read a merge of it.
  not_done: [string, string][]
  corrections: number
  asks: number
  after_merge_secs: number
}

export type Watch = {
  looked_at: string | null
  every_secs: number
  paused: boolean
  window_from: string | null
  lines_read: number
  /// The captured rows the watcher does not read yet, by why and by agent.
  unread: { why: string; agent: string; rows: number }[]
  long_secs: number
  repeat_secs: number
  alerts: WatchAlert[]
  /// The kinds of alert that are alarms (attr_recon_watch_alarms); every
  /// other one is charted, never raised.
  alarms: AlertKind[]
  /// The words of attr_recon_watch_alarms that name no kind of alert.
  not_kinds: string[]
  now: WatchNow[]
  sessions: WatchSessionRow[]
  runs: WatchRun[]
  tasks: WatchTask[]
  /// Every build of a task file read in the window, the newest first.
  builds: BuildRow[]
}

/// A build of a task file: when you ordered it and when it stopped short,
/// what it builds and wrote, the forbidden files it changed, and each door
/// gate's run.
export type BuildRow = {
  session: string
  file: string
  ticket: string | null
  ordered: string
  stopped: string | null
  builds: string[]
  written: string[]
  forbidden: string[]
  gates: { gate: string; ends_well: boolean; ran: string | null; ended_well: boolean | null }[]
  /// Each local gate with its newest run.
  locals: { gate: string; ran: string | null; passed: boolean | null }[]
  /// What the file owns and forbids, as it names them.
  owns: string[]
  forbid: string[]
  /// The branch the file names, the one cut, and the commits made.
  branch_rule: string | null
  branch: string | null
  commits: { at: string; line: string; ok: boolean }[]
}

/// What the watcher holds at its newest look.
export const fetchWatch = () => fetch('/api/watch').then((r) => answer<Watch>(r))

/// A day of a session's time on one thing, under one model.
export type TimeRow = { day: string; session: string; model: string | null; tool: string; target: string; split: Split; calls: number; failed: number }
export type HourRow = { hour: string; session: string; split: Split; counts: Counts }
export type WatchTime = { rows: TimeRow[]; hours: HourRow[] }

/// Where the time went, in the operator's own days.
export const fetchWatchTime = () => fetch(`/api/watch/time?offset=${-new Date().getTimezoneOffset()}`).then((r) => answer<WatchTime>(r))

export type Hidden =
  | 'held'
  | 'slept'
  | 'thrash'
  | 'refused'
  | 'protected'
  | 'compacted'
  | 'reread'
  | 'flood'
  | 'interrupted'
  | 'asked'
  | 'corrected'
  | 'undone'

/// One thing the watcher dug out.
export type Found = {
  id: string
  session: string
  name: string
  kind: Hidden
  at: string
  from: string
  what: string
  tool: string | null
  target: string | null
  call: string | null
  secs: number | null
  chars: number | null
  times: number
}

export const fetchFound = () => fetch('/api/watch/found').then((r) => answer<Found[]>(r))

/// One line of a session as a drill shows it; a call with how it ended.
export type LineRow = {
  at: string
  kind: 'you' | 'thought' | 'said' | 'call' | 'failed' | 'job_ended' | 'compacted' | 'api'
  text: string
  tool: string | null
  call: string | null
  target: string | null
  end: string | null
  secs: number | null
  failed: boolean | null
  status: string | null
  background: boolean | null
}

export type Lines = { lines: LineRow[]; earlier: boolean }

/// A session's lines from `from` to `to`, the newest of them.
export const fetchLines = (session: string, from: string, to: string) =>
  fetch(`/api/watch/lines?${new URLSearchParams({ session, from, to })}`).then((r) => answer<Lines>(r))

export type CallsAsked = { session?: string; tool?: string; target?: string; under?: string; from?: string; to?: string }
export type Calls = { calls: WatchRun[]; total: number }

/// The calls asked for, newest first.
export const fetchCalls = (asked: CallsAsked) => {
  const q = new URLSearchParams(Object.entries(asked).filter(([, v]) => v != null && v !== '') as [string, string][])
  return fetch(`/api/watch/calls?${q}`).then((r) => answer<Calls>(r))
}
