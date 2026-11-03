import type { BranchWeek, Findings, FoundCommit, FoundPull, RepoWeek, Week, Workflow } from '../api'

// Which part of the findings a view reads: one repository, the judged ones
// (those whose main line took some credited work; the others cannot say
// what landed), or all; and every credited commit, or only those a
// session's own output named.

export type Scope = { repo: string; evidence: 'all' | 'printed' }
export const JUDGED = '__judged'
export const ALL = '__all'

/// The repositories whose main line took any credited commit's lines.
export function judged(f: Findings): string[] {
  const out = new Set<string>()
  for (const c of f.commits) if (c.model && !c.bulk && c.fate !== 'snapshot' && c.landed > 0) out.add(c.repo)
  return [...out].sort()
}

/// The repositories holding commits of ours, most first.
export function repositories(f: Findings): string[] {
  const count = new Map<string, number>()
  for (const c of f.commits) count.set(c.repo, (count.get(c.repo) ?? 0) + 1)
  return [...count.keys()].filter((r) => r !== '').sort((a, b) => (count.get(b) ?? 0) - (count.get(a) ?? 0))
}

export type Scoped = {
  /// Credited commits, bulk drops left out: what the views measure.
  commits: FoundCommit[]
  /// Every commit of ours in the repositories, credited or not.
  everything: FoundCommit[]
  pulls: FoundPull[]
  weeks: Week[]
  repoWeeks: RepoWeek[]
  workflows: Workflow[]
  branches: BranchWeek[]
  repos: string[]
}

export function scoped(f: Findings, s: Scope): Scoped {
  const repos = s.repo === ALL ? repositories(f) : s.repo === JUDGED ? judged(f) : [s.repo]
  const keep = new Set(repos)
  const inRepo = (r: string) => (s.repo === ALL ? true : keep.has(r))
  const everything = f.commits.filter((c) => inRepo(c.repo))
  return {
    everything,
    commits: everything.filter((c) => c.fate !== 'snapshot' && c.model !== '' && !c.bulk && (s.evidence === 'all' || c.by === 'printed')),
    pulls: f.pulls.filter((p) => inRepo(p.repo)),
    weeks: f.weeks.filter((w) => inRepo(w.repo)),
    repoWeeks: f.repo_weeks.filter((w) => inRepo(w.repo)),
    workflows: f.workflows.filter((w) => inRepo(w.repo)),
    branches: f.branches.filter((b) => inRepo(b.repo)),
    repos,
  }
}
