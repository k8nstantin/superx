//! The read-only crawler: what a build branch's repository shows of it.
//!
//! The chats say what sessions did; a merge made on the host — a pull
//! request merged in Bitbucket's or GitHub's web page — is in no chat. The
//! repository has it, as last fetched. This reads it, and nothing else:
//!
//! - READ-ONLY IN THE STRONG SENSE (#447): only commands that read history
//!   and refs; never `fetch`, never a write; and no program the
//!   repository's own configuration names runs — `log.showSignature` and
//!   `core.fsmonitor` are overridden with `-c`, `log`/`diff` get
//!   `--no-textconv --no-ext-diff`, and `GIT_OPTIONAL_LOCKS=0` keeps any
//!   optional lock (an index refresh) from being taken. This module's own
//!   copy of the rule: it imports no other module.
//! - Host-agnostic: the host is read from the repository's own remote,
//!   and the merge subjects of Bitbucket (`Merged in <b> (pull request
//!   #N)`), GitHub (`Merge pull request #N from <owner>/<b>`) and git
//!   (`Merge branch '<b>'`, `Merge remote-tracking branch 'origin/<b>'`)
//!   are all read.
//! - "Merged into" names only MAINLINES: every branch cut after a merge
//!   also contains it, so containing it says nothing about where it
//!   landed. The mainlines are `attr_specs_mainline_refs` (e.g.
//!   `origin/sandbox,origin/main`), else the repository's `origin/HEAD`.

use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::process::Command;

/// A git call slower than this is abandoned and its answer is "unknown".
const GIT_TIMEOUT_MS: u64 = 8_000; // skill-allow: §9-const — read-path bound, not a policy tunable

/// Merge commits read per repository, newest first.
const MERGES_READ: usize = 5_000; // skill-allow: §9-const — read-path bound, not a policy tunable

/// Configuration under which a read would run a program the repository
/// names, overridden on every call (#447).
const NO_REPOSITORY_PROGRAMS: [&str; 4] = ["-c", "log.showSignature=false", "-c", "core.fsmonitor=false"];

/// `git -C dir args…` read-only, stdout on success, `None` on any failure
/// or on the timeout. No shell is involved.
pub async fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let (sub, rest) = args.split_first()?;
    let drivers: &[&str] = match *sub {
        "log" | "diff" => &["--no-textconv", "--no-ext-diff"],
        "blame" => &["--no-textconv"],
        _ => &[],
    };
    let run = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(NO_REPOSITORY_PROGRAMS)
        .arg(sub)
        .args(drivers)
        .args(rest)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output();
    let out = tokio::time::timeout(Duration::from_millis(GIT_TIMEOUT_MS), run).await.ok()?.ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The host a remote URL names: `git@bitbucket.org:o/r.git`,
/// `https://github.com/o/r`, `ssh://git@host:22/o/r`.
#[must_use]
pub fn host_of(url: &str) -> Option<String> {
    let rest = url.trim();
    let rest = rest.split_once("://").map_or(rest, |(_, r)| r);
    let rest = rest.split_once('@').map_or(rest, |(_, r)| r);
    let host: String = rest.chars().take_while(|c| !matches!(c, ':' | '/')).collect();
    (!host.is_empty()).then_some(host)
}

/// The branch a merge commit's subject merged, and the pull request it
/// came by, when the subject names one.
#[must_use]
pub fn merge_subject(subject: &str) -> Option<(String, Option<String>)> {
    let s = subject.trim();
    // Bitbucket: `Merged in <branch> (pull request #N)`.
    if let Some(rest) = s.strip_prefix("Merged in ") {
        let (branch, tail) = rest.split_once(' ').unwrap_or((rest, ""));
        let pr = tail
            .strip_prefix("(pull request ")
            .and_then(|t| t.split(')').next())
            .map(str::to_string);
        return Some((branch.to_string(), pr));
    }
    // GitHub: `Merge pull request #N from <owner>/<branch>`.
    if let Some(rest) = s.strip_prefix("Merge pull request ") {
        let (pr, tail) = rest.split_once(" from ")?;
        let from = tail.split_whitespace().next()?;
        let branch = from.split_once('/').map_or(from, |(_, b)| b);
        return Some((branch.to_string(), Some(pr.to_string())));
    }
    // git: `Merge branch '<b>'`, `Merge remote-tracking branch '<remote>/<b>'`.
    for prefix in ["Merge branch '", "Merge remote-tracking branch '"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            let name = rest.split('\'').next()?;
            let name = if prefix.contains("remote-tracking") { name.split_once('/').map_or(name, |(_, b)| b) } else { name };
            return Some((name.to_string(), None));
        }
    }
    None
}

/// One merge commit: its hash, when, and what it merged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Merge {
    pub commit: String,
    pub at: Option<DateTime<Utc>>,
    pub branch: String,
    pub pull_request: Option<String>,
}

/// One commit that reverts another: its hash, when, and its message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revert {
    pub commit: String,
    pub at: Option<DateTime<Utc>>,
    pub message: String,
}

/// What a repository holds, read once per crawl.
#[derive(Debug, Clone, Default)]
pub struct Repo {
    pub top: String,
    pub host: Option<String>,
    /// Where its remote's HEAD points (`origin/main`): the mainline when
    /// none is set.
    pub origin_head: Option<String>,
    pub merges: Vec<Merge>,
    pub reverts: Vec<Revert>,
}

/// What a branch looks like in its repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchState {
    pub on_local: bool,
    pub on_remote: bool,
    pub merge: Option<Merge>,
    pub merged_into: Vec<String>,
    pub revert: Option<Revert>,
}

/// The repository a directory is in, read: its top level, host, every
/// merge on its remote branches (as last fetched), and every commit there
/// that says it reverts something. `None` outside a repository.
pub async fn read(dir: &Path) -> Option<Repo> {
    let top = git(dir, &["rev-parse", "--show-toplevel"]).await?.trim().to_string();
    let top_path = Path::new(&top);
    let host = git(top_path, &["config", "--get", "remote.origin.url"]).await.and_then(|u| host_of(&u));
    let origin_head = git(top_path, &["symbolic-ref", "-q", "--short", "refs/remotes/origin/HEAD"])
        .await
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let limit = format!("--max-count={MERGES_READ}");
    let merges = git(top_path, &["log", "--remotes", "--merges", &limit, "--format=%H%x09%cI%x09%s"])
        .await
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut f = l.splitn(3, '\t');
            let (commit, at, subject) = (f.next()?, f.next()?, f.next()?);
            let (branch, pull_request) = merge_subject(subject)?;
            Some(Merge { commit: commit.to_string(), at: parse_time(at), branch, pull_request })
        })
        .collect();
    let reverts = git(top_path, &["log", "--remotes", "-i", "--grep=revert", "--format=%x1e%H%x09%cI%x09%B"])
        .await
        .unwrap_or_default()
        .split('\u{1e}')
        .filter_map(|r| {
            let mut f = r.trim_start().splitn(3, '\t');
            let (commit, at, message) = (f.next()?, f.next()?, f.next()?);
            (!commit.is_empty()).then(|| Revert { commit: commit.to_string(), at: parse_time(at), message: message.to_string() })
        })
        .collect();
    Some(Repo { top, host, origin_head, merges, reverts })
}

/// What one branch looks like in a repository already read. `mainlines`
/// are the remote branches a merge may be said to have gone INTO; empty
/// means the repository's `origin/HEAD`.
pub async fn branch(repo: &Repo, name: &str, mainlines: &[String]) -> BranchState {
    let fallback: Vec<String> = repo.origin_head.iter().cloned().collect();
    let mainlines = if mainlines.is_empty() { &fallback[..] } else { mainlines };
    let top = Path::new(&repo.top);
    let on_local = git(top, &["show-ref", "--verify", "--quiet", &format!("refs/heads/{name}")]).await.is_some();
    let on_remote = git(top, &["for-each-ref", "--format=%(refname)", &format!("refs/remotes/*/{name}")])
        .await
        .is_some_and(|o| !o.trim().is_empty());
    // The newest merge of it, on any remote branch.
    let merge = repo.merges.iter().find(|m| m.branch == name).cloned();
    let mut merged_into = Vec::new();
    let mut revert = None;
    if let Some(m) = &merge {
        merged_into = git(top, &["for-each-ref", "--format=%(refname:short)", "--contains", &m.commit, "refs/remotes"])
            .await
            .unwrap_or_default()
            .lines()
            .map(str::trim)
            .filter(|r| mainlines.iter().any(|m| m == r))
            .map(str::to_string)
            .collect();
        // A revert names the commit it reverts: git's own `This reverts
        // commit <hash>`, or the hash in a message of one's own words.
        let short = &m.commit[..m.commit.len().min(8)];
        revert = repo
            .reverts
            .iter()
            .filter(|r| r.commit != m.commit)
            .find(|r| r.message.contains(&m.commit) || r.message.contains(short))
            .cloned();
    }
    BranchState { on_local, on_remote, merge, merged_into, revert }
}

fn parse_time(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s.trim()).ok().map(|t| t.with_timezone(&Utc))
}
