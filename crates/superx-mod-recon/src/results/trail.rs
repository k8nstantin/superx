//! The trail (#539): each piece of agent work followed from the session
//! that did it to what became of it, the facts the findings and the
//! repositories' windows are drawn from.
//!
//! - **A reply** worked where its session was at that moment, and cost the
//!   tokens of its newest state, its thinking and its tool calls; its active
//!   time is the gap since its session's previous reply of the same family,
//!   capped at attr_recon_active_secs.
//! - **A commit** is a model's by the session that printed it, else by the
//!   one claim on its repository covering its time, as the comparison
//!   credits ([`super::compare`]). A squash's or a replay's lines, and their
//!   blame, go to the branch commits it carried, in proportion to their
//!   files' lines; the squash itself is no one's.
//! - Each commit then has where it went, the lines of it the main line took,
//!   the lines alive at the newest blame, the lines of its files that never
//!   landed, the pull requests that carried it, what CI said of it, whether
//!   a later commit reverted it, and, when a session printed it, whether
//!   that session ran tests and builds since its previous commit.
//!
//! Every table is read a page at a time ([`super::every`]) and joined here,
//! since sums over links and NONE are not trusted to the engine.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use superx_kernel::types::{RecordId, Value};
use superx_kernel::{Db, Result};

use super::compare::{credit, family, split, Claim};
use super::{every, flag, int, model_of, owned, text, uuid_text, DayFile};
use crate::crawler::hosts::ci::{CHECK_SUITE, COMMIT_STATUS, PIPELINE, WORKFLOW_RUN};
use crate::crawler::hosts::held::{record, row_key, time};
use crate::params::Params;

/// Where a working directory is: the repository of its own `directory`
/// row, else that of the checkout whose top level holds it, the deepest;
/// and, once folded, the repository a folder is a copy of.
pub struct RepoMap {
    /// Each repository's name, by its row.
    pub names: HashMap<String, String>,
    exact: HashMap<String, String>,
    tops: Vec<(String, String)>,
    canonical: HashMap<String, String>,
    /// The folders read as another repository: a folder's name, the
    /// repository's.
    pub folders: Vec<(String, String)>,
}

impl RepoMap {
    pub fn new(repositories: &[Value], directories: &[Value], checkouts: &[Value]) -> Self {
        let names = repositories
            .iter()
            .filter_map(|r| Some((row_key(&record(r, "id")?), text(r, "name").unwrap_or_default())))
            .collect();
        let exact =
            directories.iter().filter_map(|d| Some((text(d, "path")?, row_key(&record(d, "repository")?)))).collect();
        let mut tops: Vec<(String, String)> =
            checkouts.iter().filter_map(|c| Some((text(c, "toplevel")?, row_key(&record(c, "repository")?)))).collect();
        tops.sort_by_key(|(t, _)| std::cmp::Reverse(t.len()));
        Self { names, exact, tops, canonical: HashMap::new(), folders: Vec::new() }
    }

    /// Reads each folder in `canonical` as the repository it names.
    pub fn fold(&mut self, canonical: HashMap<String, String>) {
        let mut folders: Vec<(String, String)> = canonical
            .iter()
            .filter(|(folder, repo)| folder != repo)
            .map(|(folder, repo)| (self.name(Some(folder)), self.name(Some(repo))))
            .collect();
        folders.sort();
        self.folders = folders;
        self.canonical = canonical;
    }

    /// The repository a repository row is read as: itself, or the one its
    /// folder is a copy of.
    pub fn canon(&self, repo: &str) -> String {
        self.canonical.get(repo).cloned().unwrap_or_else(|| repo.to_string())
    }

    pub fn repo_of(&self, cwd: &str) -> Option<String> {
        self.exact
            .get(cwd)
            .cloned()
            .or_else(|| {
                self.tops
                    .iter()
                    .find(|(t, _)| cwd == t || cwd.strip_prefix(t.as_str()).is_some_and(|rest| rest.starts_with('/')))
                    .map(|(_, r)| r.clone())
            })
            .map(|r| self.canon(&r))
    }

    /// A repository's name; empty for none.
    pub fn name(&self, repo: Option<&String>) -> String {
        repo.and_then(|r| self.names.get(r)).cloned().unwrap_or_default()
    }
}

/// Each session's places, oldest first: when, and the repository there.
pub type Places = HashMap<String, Vec<(DateTime<Utc>, Option<String>)>>;

/// Every session's places, each oldest first.
pub fn places(rows: &[Value], map: &RepoMap) -> Places {
    let mut out: Places = HashMap::new();
    for p in rows {
        let (Some(session), Some(at), Some(cwd)) = (record(p, "session"), time(p, "at"), text(p, "cwd")) else { continue };
        out.entry(row_key(&session)).or_default().push((at, map.repo_of(&cwd)));
    }
    for v in out.values_mut() {
        v.sort_by_key(|(at, _)| *at);
    }
    out
}

/// The repository a session was in at `at`: its newest place by then, else
/// its first.
pub fn place_at(here: Option<&Vec<(DateTime<Utc>, Option<String>)>>, at: DateTime<Utc>) -> Option<String> {
    here.and_then(|p| p.iter().rev().find(|(t, _)| *t <= at).or_else(|| p.first()).and_then(|(_, repo)| repo.clone()))
}

/// What a CI run's newest state says of the code: passed, failed, or
/// nothing (queued, running, cancelled, skipped, neutral). The words are
/// the hosts' own: GitHub's conclusions and its statuses' states,
/// Bitbucket's results and its statuses' states.
pub fn verdict(kind: &str, status: Option<&str>, conclusion: Option<&str>) -> Option<bool> {
    let word = if kind == COMMIT_STATUS { status } else { conclusion }?;
    match word.to_ascii_lowercase().as_str() {
        "success" | "successful" => Some(true),
        "failure" | "failed" | "error" | "timed_out" | "startup_failure" => Some(false),
        _ => None,
    }
}

/// What a reflog entry did, by the message git wrote for it.
pub fn move_kind(action: &str) -> &'static str {
    let a = action.trim_start();
    if a.starts_with("commit (amend)") {
        "amend"
    } else if a.starts_with("commit (merge)") || a.starts_with("merge") {
        "merge"
    } else if a.starts_with("commit") {
        "commit"
    } else if a.starts_with("reset") {
        "reset"
    } else if a.starts_with("rebase") || (a.starts_with("pull") && a.contains("rebase")) {
        "rebase"
    } else if a.starts_with("cherry-pick") {
        "cherry_pick"
    } else if a.starts_with("revert") {
        "revert"
    } else if a.starts_with("checkout") || a.starts_with("switch") {
        "checkout"
    } else if a.contains("forced-update") {
        "forced"
    } else if a.starts_with("pull") {
        "pull"
    } else if a.starts_with("fetch") {
        "fetch"
    } else if a.starts_with("update by push") || a.starts_with("push") {
        "push"
    } else if a.starts_with("branch") {
        "branch"
    } else if a.starts_with("clone") {
        "clone"
    } else {
        "other"
    }
}

/// A move that rewrote what a ref held: an amend, a reset, a rebase, or a
/// forced update fetched from a host.
pub fn rewrites(kind: &str) -> bool {
    matches!(kind, "amend" | "reset" | "rebase" | "forced")
}

/// A reply: where it worked and what it cost.
#[derive(Debug, Clone)]
pub struct Reply {
    pub session: String,
    pub at: DateTime<Utc>,
    pub family: String,
    pub effort: Option<String>,
    pub subagent: bool,
    pub repo: Option<String>,
    pub out_tokens: i64,
    /// What it was sent: its newest state's input, cache-read and
    /// cache-write tokens.
    pub context: i64,
    pub thinking_ms: i64,
    pub tool_calls: i64,
    /// Seconds since its session's previous reply of the same family,
    /// capped at attr_recon_active_secs; 0 for a run's first.
    pub active_secs: i64,
}

/// An edit an agent's tool call made.
#[derive(Debug, Clone)]
pub struct Edit {
    pub reply: Option<usize>,
    pub at: Option<DateTime<Utc>>,
    pub path: String,
    pub kind: String,
    pub tool: String,
    pub added: i64,
    pub removed: i64,
    pub operator_changed: bool,
}

/// A test or a build an agent's command ran.
#[derive(Debug, Clone)]
pub struct Check {
    pub reply: Option<usize>,
    pub at: DateTime<Utc>,
    pub test: bool,
    /// Its runner or builder.
    pub what: String,
    pub command: String,
    pub passed: i64,
    pub failed: i64,
    pub ok: bool,
}

/// A git or host command an agent ran.
#[derive(Debug, Clone)]
pub struct Git {
    pub reply: Option<usize>,
    pub at: DateTime<Utc>,
    pub action: String,
    pub hash: Option<String>,
    pub branch: Option<String>,
    pub subject: Option<String>,
    pub pr_number: Option<i64>,
    pub url: Option<String>,
}

/// A message from the operator.
#[derive(Debug, Clone)]
pub struct Prompt {
    pub session: String,
    pub at: DateTime<Utc>,
    pub kind: String,
    pub text: String,
    /// The family answering in its session when it was typed: the newest
    /// reply's by then, else the first after.
    pub family: Option<String>,
    pub repo: Option<String>,
}

/// A commit and what became of it.
#[derive(Debug, Clone)]
pub struct Commit {
    pub id: RecordId,
    pub key: String,
    pub repo: String,
    pub hash: String,
    pub author: String,
    pub author_name: String,
    pub at: DateTime<Utc>,
    pub added: i64,
    pub removed: i64,
    pub subject: String,
    /// Its newest placement: mainline, in_flight, branch or recovered.
    pub place: Option<String>,
    /// More lines than attr_recon_bulk_lines: a drop, not authored work.
    pub bulk: bool,
    /// By the identity its repository's crawl commits as, or printed.
    pub ours: bool,
    /// A main-line commit carrying branch work: its lines are the branch
    /// commits'.
    pub squash: bool,
    /// Lines of it the main line took: all of a main-line commit's; of a
    /// branch commit's, the lines of each file whose version the main line
    /// carries.
    pub landed: i64,
    /// Lines of it alive at the newest blame; None where its repository was
    /// never blamed or none of it is on the main line.
    pub alive: Option<i64>,
    /// Lines of its files that never reached the main line.
    pub never_landed: i64,
    pub reverted: bool,
    pub family: Option<String>,
    /// `printed` (a session's own output named it) or `claim`.
    pub by: Option<&'static str>,
    pub session: Option<String>,
    pub reply: Option<usize>,
    pub tied_at: Option<DateTime<Utc>>,
    /// Every CI workflow's newest verdict on it, failed if any failed.
    pub ci: Option<bool>,
    pub ci_names: Vec<(String, Option<bool>)>,
    pub pulls: Vec<usize>,
    /// One of those pull requests is open: work in flight on the host.
    pub open_pull: bool,
    /// A printed commit's session ran tests since its previous commit.
    pub tested: Option<bool>,
    /// The newest of those tests failed nothing.
    pub tests_green: Option<bool>,
    pub built: Option<bool>,
}

/// A pull request, who opened it and what CI said along the way.
#[derive(Debug, Clone)]
pub struct Pull {
    pub key: String,
    pub repo: Option<String>,
    pub host: String,
    pub path: String,
    pub number: i64,
    pub url: String,
    pub created_at: DateTime<Utc>,
    pub title: String,
    pub state: String,
    pub merged: bool,
    /// The host's merge time; Bitbucket gives none, so there it is the
    /// update time of the first state seen merged.
    pub merged_at: Option<DateTime<Utc>>,
    pub closed_at: Option<DateTime<Utc>>,
    pub additions: i64,
    pub deletions: i64,
    pub source_branch: String,
    pub target_branch: String,
    pub hashes: Vec<String>,
    pub family: Option<String>,
    /// `opened` (a session's command opened it) or `commits` (its commits'
    /// models, the one with most lines).
    pub by: Option<&'static str>,
    pub session: Option<String>,
    pub ci_failed: i64,
    pub ci_passed: i64,
    /// The verdict of the first CI run on it that gave one.
    pub ci_first: Option<bool>,
}

/// A CI run and its newest state.
#[derive(Debug, Clone)]
pub struct Ci {
    pub repo: Option<String>,
    pub kind: String,
    pub name: String,
    pub sha: Option<String>,
    pub git_ref: Option<String>,
    pub pull: Option<usize>,
    pub url: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub status: Option<String>,
    pub conclusion: Option<String>,
    pub verdict: Option<bool>,
    /// An older state of it failed: a re-run turned it.
    pub failed_before: bool,
}

/// A reflog entry: a ref's move.
#[derive(Debug, Clone)]
pub struct Move {
    pub repo: String,
    pub mirror: bool,
    pub git_ref: String,
    pub at: DateTime<Utc>,
    pub action: String,
    pub kind: &'static str,
    pub identity: Option<String>,
    pub old: Option<String>,
    pub new: Option<String>,
}

/// A commit's printed tie: the model and session of the call that printed
/// it, and when.
struct Tie {
    family: String,
    session: Option<String>,
    reply: Option<String>,
    at: Option<DateTime<Utc>>,
}

/// Every fact, joined.
pub struct Trail {
    pub map: RepoMap,
    /// Each session's agent and kernel session, by its row.
    pub agents: HashMap<String, String>,
    pub kernel_sessions: HashMap<String, String>,
    /// By session, then time.
    pub replies: Vec<Reply>,
    pub edits: Vec<Edit>,
    pub checks: Vec<Check>,
    pub git: Vec<Git>,
    pub prompts: Vec<Prompt>,
    pub commits: Vec<Commit>,
    pub pulls: Vec<Pull>,
    pub ci: Vec<Ci>,
    pub moves: Vec<Move>,
    /// Every commit's files, by the commit's row.
    pub files: HashMap<String, Vec<DayFile>>,
}

/// The trail as last loaded, kept until it is older than the crawler's
/// reads of the lines (attr_recon_read_secs), since its facts change no
/// faster. A load holds it, so two pages asking at once load it once.
#[derive(Clone, Default)]
pub struct Held(Arc<tokio::sync::Mutex<Option<Loaded>>>);

/// A trail and when it was loaded.
type Loaded = (Instant, Arc<Trail>);

impl Held {
    /// The trail; loaded again when older than attr_recon_read_secs.
    ///
    /// # Errors
    ///
    /// [`superx_kernel::KernelError`] when the database does not answer.
    pub async fn get(&self, db: &Db, params: &Params) -> Result<Arc<Trail>> {
        let mut held = self.0.lock().await;
        if let Some((at, trail)) = held.as_ref() {
            if at.elapsed() < Duration::from_secs(params.read_secs) {
                return Ok(Arc::clone(trail));
            }
        }
        let trail = Arc::new(load(db, params).await?);
        *held = Some((Instant::now(), Arc::clone(&trail)));
        Ok(trail)
    }
}

/// The CI kinds a commit's verdict is read from, in order: a host's own
/// runs, else statuses, else check suites. Jobs, steps and check runs are
/// parts of a run and are not counted again.
const TIERS: [&[&str]; 3] = [&[WORKFLOW_RUN, PIPELINE], &[COMMIT_STATUS], &[CHECK_SUITE]];

/// Every fact recon holds, joined.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the database does not answer.
#[allow(clippy::too_many_lines)] // skill-allow: allow — one join of every table, each step named
pub async fn load(db: &Db, params: &Params) -> Result<Trail> {
    let repositories = every(db, "repository", "name").await?;
    let directories = every(db, "directory", "path, checkout.repository AS repository").await?;
    let checkouts = every(db, "checkout", "toplevel, repository").await?;
    let mut map = RepoMap::new(&repositories, &directories, &checkouts);
    let host_rows = every(db, "host_repository", "host, path").await?;
    let remote_rows = every(db, "remote_state", "repository, host, path").await?;
    // Which repository each host repository is: the local repositories
    // whose remotes name it, the one of its own name first. Every other
    // folder whose remote names it (a clone, a build folder) is that
    // repository: its work is done there, its commits are its commits.
    let mut naming: HashMap<(String, String), BTreeSet<String>> = HashMap::new();
    for r in &remote_rows {
        if let (Some(repo), Some(host), Some(path)) = (record(r, "repository"), text(r, "host"), text(r, "path")) {
            naming.entry((host, path)).or_default().insert(row_key(&repo));
        }
    }
    let mut hosts: HashMap<String, (String, String)> = HashMap::new();
    let mut local: HashMap<String, String> = HashMap::new();
    for h in &host_rows {
        let Some(id) = record(h, "id") else { continue };
        let (host, path) = (text(h, "host").unwrap_or_default(), text(h, "path").unwrap_or_default());
        let own = path.rsplit('/').next().unwrap_or_default().to_lowercase();
        if let Some(repos) = naming.get(&(host.clone(), path.clone())) {
            let named = repos.iter().find(|r| map.names.get(*r).is_some_and(|n| n.to_lowercase() == own));
            if let Some(r) = named.or_else(|| repos.iter().next()) {
                local.insert(row_key(&id), r.clone());
            }
        }
        hosts.insert(row_key(&id), (host, path));
    }
    let mut canonical: HashMap<String, String> = HashMap::new();
    for (key, repos) in &naming {
        let Some(repo) = host_rows
            .iter()
            .find(|h| text(h, "host").as_ref() == Some(&key.0) && text(h, "path").as_ref() == Some(&key.1))
            .and_then(|h| record(h, "id"))
            .and_then(|id| local.get(&row_key(&id)))
        else {
            continue;
        };
        for folder in repos {
            canonical.insert(folder.clone(), repo.clone());
        }
    }
    map.fold(canonical);
    let session_rows = every(db, "session", "kernel_session, agent").await?;
    let place_rows = every(db, "session_place", "session, cwd, at").await?;
    let reply_rows = every(db, "reply", "session, at, model, effort, subagent").await?;
    let mut states =
        every(db, "reply_state", "reply, valid_from, output_tokens, input_tokens, cache_read_tokens, cache_write_tokens").await?;
    let thinking_rows = every(db, "thinking", "reply, ms").await?;
    let call_rows = every(db, "tool_call", "reply").await?;
    let edit_rows = every(
        db,
        "edit",
        "tool_call.reply AS reply, tool_call.at AS at, tool_call.tool AS tool, path, kind, added, removed, operator_changed",
    )
    .await?;
    let test_rows =
        every(db, "test_run", "tool_call.reply AS reply, tool_call.command AS command, at, runner, passed, failed").await?;
    let build_rows = every(db, "build_run", "tool_call.reply AS reply, tool_call.command AS command, at, builder, ok").await?;
    let git_rows =
        every(db, "git_event", "tool_call.reply AS reply, at, action, hash, branch, subject, pr_number, url").await?;
    let prompt_rows = every(db, "prompt", "session, at, kind, text").await?;

    // Sessions, and each reply where it worked and what it cost.
    let agents: HashMap<String, String> =
        session_rows.iter().filter_map(|s| Some((row_key(&record(s, "id")?), text(s, "agent").unwrap_or_default()))).collect();
    let kernel_sessions: HashMap<String, String> =
        session_rows.iter().filter_map(|s| Some((row_key(&record(s, "id")?), uuid_text(s, "kernel_session")?))).collect();
    let place_of = places(&place_rows, &map);
    states.sort_by_key(|s| std::cmp::Reverse(time(s, "valid_from")));
    let mut usage: HashMap<String, (i64, i64)> = HashMap::new();
    for s in &states {
        if let Some(r) = record(s, "reply") {
            usage.entry(row_key(&r)).or_insert_with(|| {
                (int(s, "output_tokens"), int(s, "input_tokens") + int(s, "cache_read_tokens") + int(s, "cache_write_tokens"))
            });
        }
    }
    let mut thought: HashMap<String, i64> = HashMap::new();
    for t in &thinking_rows {
        if let Some(r) = record(t, "reply") {
            *thought.entry(row_key(&r)).or_default() += int(t, "ms");
        }
    }
    let mut calls: HashMap<String, i64> = HashMap::new();
    for c in &call_rows {
        if let Some(r) = record(c, "reply") {
            *calls.entry(row_key(&r)).or_default() += 1;
        }
    }
    let mut keyed: Vec<(String, Reply)> = reply_rows
        .iter()
        .filter_map(|r| {
            let key = row_key(&record(r, "id")?);
            let session = row_key(&record(r, "session")?);
            let at = time(r, "at")?;
            let (out_tokens, context) = usage.get(&key).copied().unwrap_or((0, 0));
            let reply = Reply {
                repo: place_at(place_of.get(&session), at),
                session,
                at,
                family: family(&model_of(r)),
                effort: text(r, "effort"),
                subagent: flag(r, "subagent"),
                out_tokens,
                context,
                thinking_ms: thought.get(&key).copied().unwrap_or(0),
                tool_calls: calls.get(&key).copied().unwrap_or(0),
                active_secs: 0,
            };
            Some((key, reply))
        })
        .collect();
    keyed.sort_by(|a, b| (&a.1.session, a.1.at).cmp(&(&b.1.session, b.1.at)));
    let reply_ix: HashMap<String, usize> = keyed.iter().enumerate().map(|(i, (k, _))| (k.clone(), i)).collect();
    let mut replies: Vec<Reply> = keyed.into_iter().map(|(_, r)| r).collect();

    // Each session's span of replies; runs of one family, their active time
    // and their claims on the repositories they worked in, as the
    // comparison makes them.
    let active = i64::try_from(params.active_secs).unwrap_or(i64::MAX);
    let mut spans: HashMap<String, (usize, usize)> = HashMap::new();
    let mut claims: BTreeMap<String, Vec<Claim>> = BTreeMap::new();
    let mut i = 0;
    while i < replies.len() {
        let mut j = i;
        while j < replies.len() && replies[j].session == replies[i].session {
            j += 1;
        }
        spans.insert(replies[i].session.clone(), (i, j));
        let mut k = i;
        while k < j {
            let mut m = k;
            let mut worked: BTreeSet<String> = BTreeSet::new();
            while m < j && replies[m].family == replies[k].family {
                if m > k {
                    replies[m].active_secs = (replies[m].at - replies[m - 1].at).num_seconds().clamp(0, active);
                }
                if let Some(repo) = &replies[m].repo {
                    worked.insert(repo.clone());
                }
                m += 1;
            }
            for repo in worked {
                claims.entry(repo).or_default().push(Claim {
                    model: replies[k].family.clone(),
                    from: replies[k].at,
                    to: replies[m - 1].at,
                });
            }
            k = m;
        }
        i = j;
    }
    let ix = |v: &Value| record(v, "reply").and_then(|r| reply_ix.get(&row_key(&r)).copied());

    let edits: Vec<Edit> = edit_rows
        .iter()
        .map(|e| Edit {
            reply: ix(e),
            at: time(e, "at"),
            path: text(e, "path").unwrap_or_default(),
            kind: text(e, "kind").unwrap_or_default(),
            tool: text(e, "tool").unwrap_or_default(),
            added: int(e, "added"),
            removed: int(e, "removed"),
            operator_changed: flag(e, "operator_changed"),
        })
        .collect();
    let mut checks: Vec<Check> = Vec::new();
    for (rows, test) in [(&test_rows, true), (&build_rows, false)] {
        for c in rows {
            let Some(at) = time(c, "at") else { continue };
            checks.push(Check {
                reply: ix(c),
                at,
                test,
                what: text(c, if test { "runner" } else { "builder" }).unwrap_or_default(),
                command: text(c, "command").unwrap_or_default(),
                passed: int(c, "passed"),
                failed: int(c, "failed"),
                ok: if test { int(c, "failed") == 0 } else { flag(c, "ok") },
            });
        }
    }
    checks.sort_by_key(|c| c.at);
    let mut git: Vec<Git> = git_rows
        .iter()
        .filter_map(|g| {
            Some(Git {
                reply: ix(g),
                at: time(g, "at")?,
                action: text(g, "action").unwrap_or_default(),
                hash: text(g, "hash"),
                branch: text(g, "branch"),
                subject: text(g, "subject"),
                pr_number: Some(int(g, "pr_number")).filter(|n| *n > 0),
                url: text(g, "url"),
            })
        })
        .collect();
    git.sort_by_key(|g| g.at);
    let mut prompts: Vec<Prompt> = prompt_rows
        .iter()
        .filter_map(|p| {
            let session = row_key(&record(p, "session")?);
            let at = time(p, "at")?;
            let family = spans.get(&session).and_then(|&(a, b)| {
                let list = &replies[a..b];
                let before = list.partition_point(|r| r.at <= at);
                before.checked_sub(1).and_then(|k| list.get(k)).or_else(|| list.get(before)).map(|r| r.family.clone())
            });
            Some(Prompt {
                repo: place_at(place_of.get(&session), at),
                session,
                at,
                kind: text(p, "kind").unwrap_or_default(),
                text: text(p, "text").unwrap_or_default(),
                family,
            })
        })
        .collect();
    prompts.sort_by_key(|p| p.at);

    // The commits: where each went, who made it, what landed and lived.
    let commit_rows =
        every(db, "commit", "repository, hash, author, author_name, committed_at, added, removed, subject").await?;
    let mut placements = every(db, "placement", "commit, place, valid_from").await?;
    let landed_rows = every(db, "landed_via", "commit, path, via").await?;
    let mut survival = every(db, "survival", "commit, alive, valid_from").await?;
    let tie_rows = every(
        db,
        "commit_session",
        "commit, session, tool_call.reply AS reply, tool_call.reply.model AS model, at AS tied_at",
    )
    .await?;
    let mut crawls = every(db, "crawl", "repository, identity, started_at").await?;
    placements.sort_by_key(|p| std::cmp::Reverse(time(p, "valid_from")));
    let mut place: HashMap<String, String> = HashMap::new();
    for p in &placements {
        if let (Some(c), Some(at)) = (record(p, "commit"), text(p, "place")) {
            place.entry(row_key(&c)).or_insert(at);
        }
    }
    survival.sort_by_key(|s| std::cmp::Reverse(time(s, "valid_from")));
    let mut alive_of: HashMap<String, i64> = HashMap::new();
    for s in &survival {
        if let Some(c) = record(s, "commit") {
            alive_of.entry(row_key(&c)).or_insert_with(|| int(s, "alive"));
        }
    }
    crawls.sort_by_key(|c| std::cmp::Reverse(time(c, "started_at")));
    let mut identity: HashMap<String, String> = HashMap::new();
    for c in &crawls {
        if let (Some(repo), Some(who)) = (record(c, "repository"), text(c, "identity")) {
            identity.entry(map.canon(&row_key(&repo))).or_insert(who);
        }
    }
    let tied: HashMap<String, Tie> = owned(tie_rows)
        .iter()
        .filter_map(|t| {
            Some((
                row_key(&record(t, "commit")?),
                Tie {
                    family: family(&model_of(t)),
                    session: record(t, "session").map(|s| row_key(&s)),
                    reply: record(t, "reply").map(|r| row_key(&r)),
                    at: time(t, "tied_at"),
                },
            ))
        })
        .collect();
    let bulk_lines = i64::try_from(params.bulk_lines).unwrap_or(i64::MAX);
    let mut commits: Vec<Commit> = commit_rows
        .iter()
        .filter_map(|c| {
            let id = record(c, "id")?;
            let key = row_key(&id);
            let repo = map.canon(&row_key(&record(c, "repository")?));
            let author = text(c, "author").unwrap_or_default();
            let (added, removed) = (int(c, "added"), int(c, "removed"));
            let tie = tied.get(&key);
            Some(Commit {
                place: place.get(&key).cloned(),
                bulk: added + removed > bulk_lines,
                ours: tie.is_some() || identity.get(&repo).is_some_and(|w| *w == author),
                squash: false,
                landed: 0,
                alive: None,
                never_landed: 0,
                reverted: false,
                family: tie.map(|t| t.family.clone()),
                by: tie.map(|_| "printed"),
                session: tie.and_then(|t| t.session.clone()),
                reply: tie.and_then(|t| t.reply.as_ref()).and_then(|r| reply_ix.get(r).copied()),
                tied_at: tie.and_then(|t| t.at),
                ci: None,
                ci_names: Vec::new(),
                pulls: Vec::new(),
                open_pull: false,
                tested: None,
                tests_green: None,
                built: None,
                hash: text(c, "hash").unwrap_or_default(),
                author_name: text(c, "author_name").unwrap_or_default(),
                at: time(c, "committed_at")?,
                subject: text(c, "subject").unwrap_or_default(),
                id,
                key,
                repo,
                author,
                added,
                removed,
            })
        })
        .collect();
    for c in &mut commits {
        if c.family.is_none() && c.ours {
            if let Some(model) = claims.get(&c.repo).and_then(|rc| credit(rc, c.at)) {
                c.family = Some(model.to_string());
                c.by = Some("claim");
            }
        }
    }
    let commit_ix: HashMap<String, usize> = commits.iter().enumerate().map(|(i, c)| (c.key.clone(), i)).collect();
    let blamed: HashSet<String> = alive_of.keys().filter_map(|k| commit_ix.get(k)).map(|&i| commits[i].repo.clone()).collect();
    for c in &mut commits {
        if c.place.as_deref() == Some("mainline") {
            c.alive = alive_of.get(&c.key).copied().or_else(|| blamed.contains(&c.repo).then_some(0));
        }
    }

    // Every commit's files; and what of each commit of ours off the main
    // line reached it: the lines of its files whose versions the main line
    // carries, landed by a squash or a replay. The rest never did. A
    // squash's blame goes to the branch commits it carried, in proportion
    // to the lines each landed through it and never more than those; the
    // squash itself is no one's.
    let mut files: HashMap<String, Vec<DayFile>> = HashMap::new();
    for f in &every(db, "commit_file", "commit, path, change, added, removed").await? {
        if let (Some(c), Some(path)) = (record(f, "commit"), text(f, "path")) {
            files.entry(row_key(&c)).or_default().push(DayFile {
                path,
                change: text(f, "change").unwrap_or_default(),
                added: int(f, "added"),
                removed: int(f, "removed"),
            });
        }
    }
    let mut landed_via: HashMap<(String, String), String> = HashMap::new();
    for l in &landed_rows {
        if let (Some(c), Some(path), Some(via)) = (record(l, "commit"), text(l, "path"), record(l, "via")) {
            landed_via.insert((row_key(&c), path), row_key(&via));
        }
    }
    let off = |c: &Commit| matches!(c.place.as_deref(), Some("branch" | "in_flight" | "recovered"));
    let mut pieces: HashMap<String, Vec<(usize, i64)>> = HashMap::new();
    let mut bulk_via: HashSet<String> = HashSet::new();
    for (i, c) in commits.iter_mut().enumerate() {
        if c.place.as_deref() == Some("mainline") {
            c.landed = c.added;
            continue;
        }
        if !off(c) || !c.ours {
            continue;
        }
        let mut through: BTreeMap<String, i64> = BTreeMap::new();
        for f in files.get(&c.key).map(Vec::as_slice).unwrap_or_default() {
            match landed_via.get(&(c.key.clone(), f.path.clone())) {
                Some(via) => {
                    c.landed += f.added;
                    *through.entry(via.clone()).or_default() += f.added;
                }
                None => c.never_landed += f.added,
            }
        }
        for (via, lines) in through {
            if c.bulk {
                bulk_via.insert(via);
            } else {
                pieces.entry(via).or_default().push((i, lines));
            }
        }
    }
    let mut squashes: Vec<usize> = Vec::new();
    let mut gains: Vec<(usize, i64)> = Vec::new();
    for (via, list) in &pieces {
        let Some(&m) = commit_ix.get(via) else { continue };
        let squash = &commits[m];
        if squash.place.as_deref() != Some("mainline") || (squash.bulk && bulk_via.contains(via)) {
            continue;
        }
        if let Some(alive) = squash.alive {
            let weights: Vec<i64> = list.iter().map(|p| p.1.max(1)).collect();
            for (k, share) in split(alive, &weights).into_iter().enumerate() {
                gains.push((list[k].0, share.min(list[k].1)));
            }
        }
        squashes.push(m);
    }
    for m in squashes {
        let c = &mut commits[m];
        c.squash = true;
        c.family = None;
        c.by = None;
    }
    for (i, alive) in gains {
        let c = &mut commits[i];
        c.alive = Some(c.alive.unwrap_or(0) + alive);
    }

    // The same commit held by two folders of one repository is one commit:
    // where it went from the copy that went furthest, who made it from the
    // copy with the strongest evidence.
    let rank = |c: &Commit| match c.place.as_deref() {
        Some("mainline") => 4,
        Some("in_flight") => 3,
        Some("branch") => 2,
        Some("recovered") => 1,
        _ => 0,
    };
    let evidence = |c: &Commit| match c.by {
        Some("printed") => 2,
        Some("claim") => 1,
        _ => 0,
    };
    let mut copies: BTreeMap<(String, String), Vec<Commit>> = BTreeMap::new();
    for c in commits.drain(..) {
        copies.entry((c.repo.clone(), c.hash.clone())).or_default().push(c);
    }
    for (_, mut held) in copies {
        held.sort_by_key(|c| std::cmp::Reverse((rank(c), c.alive.unwrap_or(-1), c.landed)));
        let strongest = held.iter().map(&evidence).max().unwrap_or(0);
        let witness = held
            .iter()
            .find(|c| evidence(c) == strongest)
            .map(|c| (c.family.clone(), c.by, c.session.clone(), c.reply, c.tied_at));
        let mut one = held.swap_remove(0);
        if let Some(w) = witness.filter(|_| evidence(&one) < strongest) {
            (one.family, one.by, one.session, one.reply, one.tied_at) = w;
        }
        commits.push(one);
    }

    // What a later commit reverted: git's own `Revert "<subject>"`.
    let mut by_subject: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (i, c) in commits.iter().enumerate() {
        by_subject.entry((c.repo.clone(), c.subject.clone())).or_default().push(i);
    }
    let mut reverted: Vec<usize> = Vec::new();
    for c in &commits {
        let Some(inner) = c.subject.strip_prefix("Revert \"").and_then(|s| s.strip_suffix('"')) else { continue };
        for &i in by_subject.get(&(c.repo.clone(), inner.to_string())).map(Vec::as_slice).unwrap_or_default() {
            if commits[i].at <= c.at {
                reverted.push(i);
            }
        }
    }
    for i in reverted {
        commits[i].reverted = true;
    }

    // Printed commits: whether their session tested and built since its
    // previous commit.
    let mut printed: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, c) in commits.iter().enumerate() {
        if let (Some(s), Some(_)) = (&c.session, c.tied_at) {
            printed.entry(s.clone()).or_default().push(i);
        }
    }
    let mut session_checks: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, c) in checks.iter().enumerate() {
        if let Some(r) = c.reply.and_then(|r| replies.get(r)) {
            session_checks.entry(r.session.clone()).or_default().push(i);
        }
    }
    for (session, list) in &mut printed {
        list.sort_by_key(|&i| commits[i].tied_at);
        let ran = session_checks.get(session).map(Vec::as_slice).unwrap_or_default();
        let mut since: Option<DateTime<Utc>> = None;
        for &i in list.iter() {
            let to = commits[i].tied_at;
            let between: Vec<&Check> =
                ran.iter().map(|&k| &checks[k]).filter(|k| since.is_none_or(|s| k.at > s) && Some(k.at) <= to).collect();
            let tests: Vec<&&Check> = between.iter().filter(|k| k.test).collect();
            let c = &mut commits[i];
            c.tested = Some(!tests.is_empty());
            c.tests_green = tests.last().map(|k| k.ok);
            c.built = Some(between.iter().any(|k| !k.test));
            since = to;
        }
    }

    // The hosts: which repository each host repository is, its pull
    // requests and their commits, its CI runs.
    let pull_rows = every(db, "pull_request", "host_repository, number, created_at, source_branch, target_branch, url").await?;
    let mut pull_states = every(
        db,
        "pull_request_state",
        "pull_request, state, merged, title, merged_at, closed_at, additions, deletions, updated_at, valid_from",
    )
    .await?;
    let pull_commit_rows = every(db, "pull_request_commit", "pull_request, hash").await?;
    let ci_rows = every(db, "ci_run", "host_repository, kind, name, commit_sha, ref, pull_request, url, created_at").await?;
    let mut ci_states = every(db, "ci_run_state", "ci_run, status, conclusion, completed_at, valid_from").await?;
    let move_rows = every(db, "ref_move", "repository, mirror, ref, at, old, new, identity, action").await?;

    pull_states.sort_by_key(|s| time(s, "valid_from"));
    let mut newest: HashMap<String, &Value> = HashMap::new();
    let mut first_merged: HashMap<String, DateTime<Utc>> = HashMap::new();
    for s in &pull_states {
        let Some(p) = record(s, "pull_request") else { continue };
        let key = row_key(&p);
        if flag(s, "merged") {
            if let Some(at) = time(s, "merged_at").or_else(|| time(s, "updated_at")) {
                first_merged.entry(key.clone()).or_insert(at);
            }
        }
        newest.insert(key, s);
    }
    let mut hashes: HashMap<String, Vec<String>> = HashMap::new();
    for p in &pull_commit_rows {
        if let (Some(pr), Some(h)) = (record(p, "pull_request"), text(p, "hash")) {
            hashes.entry(row_key(&pr)).or_default().push(h);
        }
    }
    let opened: HashMap<String, usize> =
        git.iter().filter(|g| g.action == "pr_created").filter_map(|g| Some((g.url.clone()?, g.reply?))).collect();
    let by_hash: HashMap<(String, String), usize> =
        commits.iter().enumerate().map(|(i, c)| ((c.repo.clone(), c.hash.clone()), i)).collect();
    let mut pulls: Vec<Pull> = pull_rows
        .iter()
        .filter_map(|p| {
            let key = row_key(&record(p, "id")?);
            let hr = row_key(&record(p, "host_repository")?);
            let (host, path) = hosts.get(&hr).cloned().unwrap_or_default();
            let state = newest.get(&key);
            let url = text(p, "url").unwrap_or_default();
            let repo = local.get(&hr).cloned();
            let mut hs = hashes.remove(&key).unwrap_or_default();
            hs.sort();
            hs.dedup();
            let (family, by, session) = match opened.get(&url).and_then(|&r| replies.get(r)) {
                Some(r) => (Some(r.family.clone()), Some("opened"), Some(r.session.clone())),
                None => {
                    let mut lines: BTreeMap<String, i64> = BTreeMap::new();
                    for h in &hs {
                        let Some(c) = repo.as_ref().and_then(|r| by_hash.get(&(r.clone(), h.clone()))).map(|&i| &commits[i]) else {
                            continue;
                        };
                        if let Some(f) = &c.family {
                            *lines.entry(f.clone()).or_default() += c.added + c.removed;
                        }
                    }
                    let top = lines.into_iter().max_by_key(|(_, n)| *n).map(|(f, _)| f);
                    (top.clone(), top.map(|_| "commits"), None)
                }
            };
            Some(Pull {
                repo,
                host,
                path,
                number: int(p, "number"),
                created_at: time(p, "created_at")?,
                title: state.and_then(|s| text(s, "title")).unwrap_or_default(),
                state: state.and_then(|s| text(s, "state")).unwrap_or_default(),
                merged: state.is_some_and(|s| flag(s, "merged")),
                merged_at: first_merged.get(&key).copied(),
                closed_at: state.and_then(|s| time(s, "closed_at")),
                additions: state.map(|s| int(s, "additions")).unwrap_or(0),
                deletions: state.map(|s| int(s, "deletions")).unwrap_or(0),
                source_branch: text(p, "source_branch").unwrap_or_default(),
                target_branch: text(p, "target_branch").unwrap_or_default(),
                hashes: hs,
                family,
                by,
                session,
                ci_failed: 0,
                ci_passed: 0,
                ci_first: None,
                url,
                key,
            })
        })
        .collect();
    pulls.sort_by_key(|p| p.created_at);
    let pull_ix: HashMap<String, usize> = pulls.iter().enumerate().map(|(i, p)| (p.key.clone(), i)).collect();
    for (i, p) in pulls.iter().enumerate() {
        for h in &p.hashes {
            if let Some(&c) = p.repo.as_ref().and_then(|r| by_hash.get(&(r.clone(), h.clone()))) {
                commits[c].pulls.push(i);
                commits[c].open_pull |= p.state.eq_ignore_ascii_case("open");
            }
        }
    }

    ci_states.sort_by_key(|s| time(s, "valid_from"));
    let mut run_states: HashMap<String, Vec<&Value>> = HashMap::new();
    for s in &ci_states {
        if let Some(r) = record(s, "ci_run") {
            run_states.entry(row_key(&r)).or_default().push(s);
        }
    }
    let mut ci: Vec<Ci> = ci_rows
        .iter()
        .filter_map(|r| {
            let key = row_key(&record(r, "id")?);
            let kind = text(r, "kind").unwrap_or_default();
            let states = run_states.get(&key).map(Vec::as_slice).unwrap_or_default();
            let said = |s: &&Value| verdict(&kind, text(s, "status").as_deref(), text(s, "conclusion").as_deref());
            let last = states.last();
            let verdict = last.and_then(said);
            Some(Ci {
                repo: record(r, "host_repository").and_then(|h| local.get(&row_key(&h)).cloned()),
                name: text(r, "name").unwrap_or_default(),
                sha: text(r, "commit_sha"),
                git_ref: text(r, "ref"),
                pull: record(r, "pull_request").and_then(|p| pull_ix.get(&row_key(&p)).copied()),
                url: text(r, "url"),
                created_at: time(r, "created_at"),
                completed_at: last.and_then(|s| time(s, "completed_at")),
                status: last.and_then(|s| text(s, "status")),
                conclusion: last.and_then(|s| text(s, "conclusion")),
                failed_before: verdict == Some(true) && states.iter().any(|s| said(s) == Some(false)),
                verdict,
                kind,
            })
        })
        .collect();
    ci.sort_by_key(|c| c.created_at);

    // Each commit's CI: the newest run of each workflow, from the first tier
    // that has any; failed if any failed.
    let mut on_sha: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (i, run) in ci.iter().enumerate() {
        if let (Some(repo), Some(sha)) = (&run.repo, &run.sha) {
            on_sha.entry((repo.clone(), sha.clone())).or_default().push(i);
        }
    }
    for c in &mut commits {
        let Some(runs) = on_sha.get(&(c.repo.clone(), c.hash.clone())) else { continue };
        let Some(tier) = TIERS.iter().find(|t| runs.iter().any(|&r| t.contains(&ci[r].kind.as_str()))) else { continue };
        let mut by_name: BTreeMap<String, Option<bool>> = BTreeMap::new();
        for &r in runs.iter().filter(|&&r| tier.contains(&ci[r].kind.as_str())) {
            by_name.insert(ci[r].name.clone(), ci[r].verdict);
        }
        let said: Vec<bool> = by_name.values().filter_map(|v| *v).collect();
        c.ci = if said.contains(&false) { Some(false) } else if said.is_empty() { None } else { Some(true) };
        c.ci_names = by_name.into_iter().collect();
    }

    // Each pull request's CI: the runs on its commits or named for it.
    for (i, p) in pulls.iter_mut().enumerate() {
        let shas: HashSet<&String> = p.hashes.iter().collect();
        let mut runs: Vec<&Ci> = ci
            .iter()
            .filter(|r| r.pull == Some(i) || (r.repo == p.repo && r.sha.as_ref().is_some_and(|s| shas.contains(s))))
            .filter(|r| TIERS[0].contains(&r.kind.as_str()) || TIERS[1].contains(&r.kind.as_str()))
            .collect();
        runs.sort_by_key(|r| r.created_at);
        p.ci_failed = runs.iter().filter(|r| r.verdict == Some(false) || r.failed_before).count().try_into().unwrap_or(i64::MAX);
        p.ci_passed = runs.iter().filter(|r| r.verdict == Some(true)).count().try_into().unwrap_or(i64::MAX);
        p.ci_first = runs.iter().find_map(|r| if r.failed_before { Some(false) } else { r.verdict });
    }

    let mut moves: Vec<Move> = move_rows
        .iter()
        .filter_map(|m| {
            let action = text(m, "action").unwrap_or_default();
            Some(Move {
                repo: map.canon(&row_key(&record(m, "repository")?)),
                mirror: record(m, "mirror").is_some(),
                git_ref: text(m, "ref").unwrap_or_default(),
                at: time(m, "at")?,
                kind: move_kind(&action),
                action,
                identity: text(m, "identity"),
                old: text(m, "old"),
                new: text(m, "new"),
            })
        })
        .collect();
    moves.sort_by_key(|m| m.at);
    commits.sort_by_key(|c| c.at);

    Ok(Trail { map, agents, kernel_sessions, replies, edits, checks, git, prompts, commits, pulls, ci, moves, files })
}

#[cfg(test)]
mod tests {
    use super::{move_kind, rewrites, verdict};

    #[test]
    fn a_reflog_entry_is_what_its_message_says_it_did() {
        for (message, kind) in [
            ("commit: add a", "commit"),
            ("commit (initial): first", "commit"),
            ("commit (amend): add a", "amend"),
            ("commit (merge): Merge branch 'x'", "merge"),
            ("merge feat/x: Fast-forward", "merge"),
            ("reset: moving to HEAD~1", "reset"),
            ("rebase (finish): returning to refs/heads/x", "rebase"),
            ("pull --rebase (finish): returning to refs/heads/main", "rebase"),
            ("pull: Fast-forward", "pull"),
            ("cherry-pick: add a", "cherry_pick"),
            ("checkout: moving from main to x", "checkout"),
            ("fetch: forced-update", "forced"),
            ("fetch origin: fast-forward", "fetch"),
            ("update by push", "push"),
            ("branch: Created from HEAD", "branch"),
            ("clone: from https://github.com/a/b", "clone"),
            ("something else", "other"),
        ] {
            assert_eq!(move_kind(message), kind, "{message}");
        }
        assert!(["amend", "reset", "rebase", "forced"].iter().all(|k| rewrites(k)));
        assert!(!["commit", "merge", "push", "fetch", "checkout"].iter().any(|k| rewrites(k)));
    }

    #[test]
    fn a_ci_run_passes_or_fails_by_its_hosts_own_words() {
        assert_eq!(verdict("workflow_run", Some("completed"), Some("success")), Some(true));
        assert_eq!(verdict("workflow_run", Some("completed"), Some("failure")), Some(false));
        assert_eq!(verdict("workflow_run", Some("completed"), Some("timed_out")), Some(false));
        assert_eq!(verdict("workflow_run", Some("in_progress"), None), None, "still running");
        assert_eq!(verdict("check_suite", Some("completed"), Some("cancelled")), None, "cancelled says nothing of the code");
        assert_eq!(verdict("pipeline", Some("COMPLETED"), Some("SUCCESSFUL")), Some(true));
        assert_eq!(verdict("pipeline", Some("COMPLETED"), Some("FAILED")), Some(false));
        assert_eq!(verdict("commit_status", Some("failure"), None), Some(false), "a status's state is its verdict");
        assert_eq!(verdict("commit_status", Some("SUCCESSFUL"), None), Some(true));
        assert_eq!(verdict("commit_status", Some("pending"), None), None);
    }
}
