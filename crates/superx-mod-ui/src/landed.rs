//! Churn as the repository saw it (#386): the lines that LANDED on each
//! repo's main line in the range, read with `git log` from the
//! repositories the agents worked in. The transcript sees only the
//! edits whose text rides the tool input — a shell edit replaces an
//! unknown number of lines (#383). The repository knows exactly.
//!
//! Boundary (operator-approved 2026-09-11): the UI module reads the
//! repositories on this machine, READ-ONLY, through `git`. Roots come
//! from the `cwd` the agents' transcripts carry, which the substrate
//! already holds. Anything unreadable degrades to `—` and is counted,
//! never invented. Nothing is written anywhere.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use tokio::process::Command;

use crate::api::{ChurnPoint, Landed, LandedRepo};

/// Repositories read per request, at most.
const LANDED_REPOS: usize = 8; // skill-allow: §9-const — read-path bound, not a policy tunable
/// Files blamed per repository when measuring survival (#405): only the
/// files the period's landed commits touched, so the bound is on the
/// work, not on the alphabet (#414).
const SURVIVAL_FILES: usize = 4_000; // skill-allow: §9-const — read-path bound, not a policy tunable

/// A git call slower than this is abandoned and counted unreadable.
const GIT_TIMEOUT_MS: u64 = 4_000; // skill-allow: §9-const — read-path bound, not a policy tunable

/// `git -C dir args…`, stdout on success, `None` on any failure or on
/// the timeout. No shell is involved: arguments are passed as given.
pub(crate) async fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let run = Command::new("git").arg("-C").arg(dir).args(args).output();
    let out = tokio::time::timeout(Duration::from_millis(GIT_TIMEOUT_MS), run)
        .await
        .ok()?
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The main line's ref: what `origin/HEAD` points at, else a local
/// `main` or `master`, else `HEAD`.
pub(crate) async fn main_ref(dir: &Path) -> String {
    if let Some(s) = git(dir, &["symbolic-ref", "-q", "refs/remotes/origin/HEAD"]).await {
        if let Some(short) = s.trim().strip_prefix("refs/remotes/") {
            return short.to_string();
        }
    }
    for b in ["main", "master"] {
        if git(dir, &["rev-parse", "-q", "--verify", &format!("refs/heads/{b}")])
            .await
            .is_some()
        {
            return b.to_string();
        }
    }
    "HEAD".to_string()
}

/// The repository root a working directory belongs to, if it is one.
pub async fn toplevel(dir: &Path) -> Option<String> {
    if !dir.is_dir() {
        return None;
    }
    git(dir, &["rev-parse", "--show-toplevel"])
        .await
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// The repository's name: the directory holding `.git` — for a
/// worktree, the main checkout's, so worktrees of one repo read as one.
pub(crate) fn repo_name(common_git_dir: &str, toplevel: &str) -> String {
    let common = Path::new(common_git_dir);
    let holder = if common.file_name().is_some_and(|n| n == ".git") {
        common.parent()
    } else {
        None
    };
    holder
        .unwrap_or_else(|| Path::new(toplevel))
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| toplevel.to_string())
}

/// What landed on the main line of every repository the given working
/// directories belong to, since `since` (all history when `None`).
pub async fn landed(cwds: &HashSet<String>, since: Option<DateTime<Utc>>) -> Landed {
    let mut landed = Landed::default();
    // One repository per common git dir: worktrees fold into their repo.
    let mut repos: BTreeMap<String, String> = BTreeMap::new();
    let mut cwds: Vec<&String> = cwds.iter().collect();
    cwds.sort();
    for cwd in cwds {
        let p = Path::new(cwd);
        if !p.is_dir() {
            landed.unreadable += 1;
            continue;
        }
        let Some(top) = git(p, &["rev-parse", "--show-toplevel"]).await else {
            landed.unreadable += 1;
            continue;
        };
        let Some(common) = git(p, &["rev-parse", "--path-format=absolute", "--git-common-dir"]).await
        else {
            landed.unreadable += 1;
            continue;
        };
        repos
            .entry(common.trim().to_string())
            .or_insert_with(|| top.trim().to_string());
        if repos.len() >= LANDED_REPOS {
            break;
        }
    }
    let since_s = since.map(|s| s.to_rfc3339());
    let mut series: BTreeMap<String, (i64, i64)> = BTreeMap::new();
    for (common, top) in repos {
        let dir = Path::new(&top);
        let branch = main_ref(dir).await;
        // First-parent on the main line counts what LANDED, once — not a
        // branch's commits and their squash both. `-m` diffs a merge
        // commit against its first parent so merged work is counted too.
        let mut args = vec!["log", "--first-parent", "-m", "--numstat", "--format=%x01%H%x09%ct"];
        if let Some(s) = &since_s {
            args.push("--since");
            args.push(s);
        }
        args.push(&branch);
        let Some(out) = git(dir, &args).await else {
            landed.unreadable += 1;
            continue;
        };
        let mut repo = LandedRepo {
            name: repo_name(&common, &top),
            branch: branch.clone(),
            commits: 0,
            added: 0,
            removed: 0,
        };
        let mut bucket: Option<String> = None;
        for line in out.lines() {
            if let Some(rest) = line.strip_prefix('\u{1}') {
                repo.commits += 1;
                bucket = rest
                    .split('\t')
                    .nth(1)
                    .and_then(|t| t.trim().parse::<i64>().ok())
                    .and_then(|t| Utc.timestamp_opt(t, 0).single())
                    .map(|d| d.format("%Y-%m-%dT%H").to_string());
                continue;
            }
            let mut parts = line.split('\t');
            let (Some(a), Some(r)) = (parts.next(), parts.next()) else { continue };
            // A binary file prints `-` for both; it moved no lines.
            let (Ok(a), Ok(r)) = (a.trim().parse::<i64>(), r.trim().parse::<i64>()) else { continue };
            repo.added += a;
            repo.removed += r;
            if let Some(b) = &bucket {
                let e = series.entry(b.clone()).or_insert((0, 0));
                e.0 += a;
                e.1 += r;
            }
        }
        landed.commits += repo.commits;
        landed.added += repo.added;
        landed.removed += repo.removed;
        landed.repos.push(repo);
    }
    landed.repos.sort_by_key(|r| std::cmp::Reverse(r.added + r.removed));
    landed.series = series
        .into_iter()
        .map(|(t, (added, removed))| ChurnPoint { t, added, removed })
        .collect();
    landed
}


/// One commit, as the model comparison reads it (#414): when its work
/// was done — the AUTHOR time, which a rebase keeps and a squash's branch
/// commits still carry — who wrote it, what it said, and what it changed,
/// file by file.
#[derive(Debug, Clone)]
pub struct WorkCommit {
    pub hash: String,
    pub at: DateTime<Utc>,
    /// The author's email: the identity a commit was made as.
    pub author: String,
    /// The subject, lower-cased. A model that keeps landing "fix",
    /// "revert" and "undo" is redoing its own work.
    pub subject: String,
    /// `(path, added, removed)` per file. A binary reads `0, 0`.
    pub files: Vec<(String, i64, i64)>,
}

impl WorkCommit {
    #[must_use]
    pub fn added(&self) -> i64 {
        self.files.iter().map(|f| f.1).sum()
    }

    #[must_use]
    pub fn removed(&self) -> i64 {
        self.files.iter().map(|f| f.2).sum()
    }
}

/// Everything the comparison needs from one repository (#414), read once
/// per repository — not once per checkout of it.
#[derive(Debug, Default)]
pub struct RepoWork {
    /// The identity this machine commits as here (`user.email`).
    pub identity: Option<String>,
    /// Non-merge commits the main line contains in the period: direct
    /// commits, squash commits, and the commits a merge brought in.
    pub landed: Vec<WorkCommit>,
    /// Lines blame still attributes to each landed commit — AT THE MAIN
    /// LINE, whatever branch any checkout has out. Blaming the working
    /// tree read every commit a checkout's branch did not contain as
    /// dead.
    pub alive: HashMap<String, i64>,
    /// Non-merge commits on branches the main line does not contain.
    pub off_mainline: Vec<WorkCommit>,
    /// `(off-main commit, path)` → the main-line commit that carries the
    /// final version of that file from a branch holding the commit: that
    /// file's work LANDED, by squash or by replay, though the commit
    /// itself never reached the main line.
    pub landed_via: HashMap<(String, String), String>,
    /// Off-main commits on a branch some checkout still has out: work in
    /// flight, not work abandoned (#414).
    pub in_flight: HashSet<String>,
}

/// Commit logs the comparison reads: author time, author email, subject,
/// per-file line counts. Renames are read as a delete and an add, so
/// every path is one that exists in some tree and can be blamed.
const WORK_FORMAT: &str = "--format=%x01%H %at %ae %s";

fn parse_work_log(out: &str) -> Vec<WorkCommit> {
    let mut commits: Vec<WorkCommit> = Vec::new();
    for line in out.lines() {
        if let Some(rest) = line.strip_prefix('\u{1}') {
            let mut parts = rest.splitn(4, ' ');
            let (Some(hash), Some(ts), Some(author)) = (parts.next(), parts.next(), parts.next()) else {
                continue;
            };
            let Some(at) = ts.parse::<i64>().ok().and_then(|t| Utc.timestamp_opt(t, 0).single()) else {
                continue;
            };
            commits.push(WorkCommit {
                hash: hash.to_string(),
                at,
                author: author.to_string(),
                subject: parts.next().unwrap_or("").to_ascii_lowercase(),
                files: Vec::new(),
            });
            continue;
        }
        let Some(c) = commits.last_mut() else { continue };
        let mut p = line.split('\t');
        let (Some(a), Some(d), Some(path)) = (p.next(), p.next(), p.next()) else { continue };
        if path.is_empty() {
            continue;
        }
        // A binary file prints `-` for both.
        c.files.push((
            path.to_string(),
            a.trim().parse::<i64>().unwrap_or(0),
            d.trim().parse::<i64>().unwrap_or(0),
        ));
    }
    commits
}

/// `(path, blob)` → the main-line commit that wrote that version of the
/// file, oldest writer kept. A `--raw` log names every file's new blob.
fn parse_versions(out: &str) -> HashMap<(String, String), String> {
    let mut versions: HashMap<(String, String), String> = HashMap::new();
    let mut commit = String::new();
    for line in out.lines() {
        if let Some(hash) = line.strip_prefix('\u{1}') {
            commit = hash.trim().to_string();
            continue;
        }
        let Some(meta) = line.strip_prefix(':') else { continue };
        let Some((head, path)) = meta.split_once('\t') else { continue };
        let Some(blob) = head.split_whitespace().nth(3) else { continue };
        // Newest first, so a later insert is an older writer.
        versions.insert((path.to_string(), blob.to_string()), commit.clone());
    }
    versions
}

/// The main line a repository's work lands on: the operator's
/// `attr_ui_mainline_refs` entry for it when there is one (#414), else
/// what `origin/HEAD` points at, else a local `main` or `master`.
pub(crate) async fn mainline_of(dir: &Path, repo: &str, overrides: &HashMap<String, String>) -> String {
    match overrides.get(repo) {
        Some(r) => r.clone(),
        None => main_ref(dir).await,
    }
}

/// `args`, bounded to commits since `since` when there is one.
fn bounded<'a>(mut args: Vec<&'a str>, since: Option<&'a str>) -> Vec<&'a str> {
    if let Some(s) = since {
        args.push("--since");
        args.push(s);
    }
    args
}

/// Read one repository for the comparison (#414). `None` when git cannot
/// answer at all.
pub async fn repo_work(dir: &Path, mainline: &str, since: Option<DateTime<Utc>>) -> Option<RepoWork> {
    let since_s = since.map(|s| s.to_rfc3339());
    let landed_log = git(
        dir,
        &bounded(vec!["log", mainline, "--no-merges", "--no-renames", "--numstat", WORK_FORMAT], since_s.as_deref()),
    )
    .await?;
    let mut work = RepoWork {
        identity: git(dir, &["config", "user.email"])
            .await
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        landed: parse_work_log(&landed_log),
        ..RepoWork::default()
    };
    let versions = git(
        dir,
        &bounded(
            vec!["log", mainline, "--no-merges", "--no-renames", "--raw", "--no-abbrev", "--format=%x01%H"],
            since_s.as_deref(),
        ),
    )
    .await
    .map(|o| parse_versions(&o))
    .unwrap_or_default();

    // Every branch the main line does not contain. `refs/stash` and the
    // other odd refs `--all` swept in are not branches (#414).
    let tips = git(
        dir,
        &["for-each-ref", &format!("--no-merged={mainline}"), "--format=%(refname)", "refs/heads", "refs/remotes"],
    )
    .await
    .unwrap_or_default();
    // Branches a checkout has out right now: what is on them and not on
    // the main line is still being worked (#414).
    let checked_out: HashSet<String> = git(dir, &["worktree", "list", "--porcelain"])
        .await
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.strip_prefix("branch "))
        .map(|b| b.trim().to_string())
        .collect();
    let mut seen: HashMap<String, usize> = HashMap::new();
    for tip in tips.lines().map(str::trim).filter(|t| !t.is_empty() && !t.ends_with("/HEAD")) {
        let live_tip = checked_out.contains(tip);
        // Which of this branch's files reached the main line in exactly
        // the version the branch ends with: those landed (#414). A
        // squash commit writes precisely those versions; so does a
        // replay onto a later main.
        let mut landed_file: HashMap<String, String> = HashMap::new();
        if let Some(base) = git(dir, &["merge-base", mainline, tip]).await {
            let base = base.trim().to_string();
            if let Some(diff) = git(dir, &["diff", "--raw", "--no-abbrev", "--no-renames", &base, tip]).await {
                for line in diff.lines() {
                    let Some(meta) = line.strip_prefix(':') else { continue };
                    let Some((head, path)) = meta.split_once('\t') else { continue };
                    let Some(blob) = head.split_whitespace().nth(3) else { continue };
                    if let Some(via) = versions.get(&(path.to_string(), blob.to_string())) {
                        landed_file.insert(path.to_string(), via.clone());
                    }
                }
            }
        }
        let Some(log) = git(
            dir,
            &bounded(
                vec!["log", tip, "--not", mainline, "--no-merges", "--no-renames", "--numstat", WORK_FORMAT],
                since_s.as_deref(),
            ),
        )
        .await
        else {
            continue;
        };
        for c in parse_work_log(&log) {
            if live_tip {
                work.in_flight.insert(c.hash.clone());
            }
            for (path, _, _) in &c.files {
                if let Some(via) = landed_file.get(path) {
                    work.landed_via.insert((c.hash.clone(), path.clone()), via.clone());
                }
            }
            if !seen.contains_key(&c.hash) {
                seen.insert(c.hash.clone(), work.off_mainline.len());
                work.off_mainline.push(c);
            }
        }
    }

    // What is still there, per landed commit, blamed at the main line.
    let mut files: Vec<&str> = work
        .landed
        .iter()
        .flat_map(|c| c.files.iter().map(|f| f.0.as_str()))
        .filter(|f| !skip_for_survival(f))
        .collect();
    files.sort_unstable();
    files.dedup();
    for file in files.into_iter().take(SURVIVAL_FILES) {
        let Some(blame) = git(dir, &["blame", "--line-porcelain", mainline, "--", file]).await else {
            continue; // not in the main line's tree any more: nothing left
        };
        for line in blame.lines() {
            // A porcelain header opens with the commit and three
            // numbers; everything else is content.
            let Some((hash, rest)) = line.split_once(' ') else { continue };
            if hash.len() == 40
                && hash.bytes().all(|b| b.is_ascii_hexdigit())
                && rest.starts_with(|c: char| c.is_ascii_digit())
            {
                *work.alive.entry(hash.to_string()).or_insert(0) += 1;
            }
        }
    }
    Some(work)
}

/// Files whose lines say nothing about whether work survived: built
/// bundles, lockfiles and binaries.
fn skip_for_survival(path: &str) -> bool {
    path.contains("/dist/")
        || path.ends_with(".lock")
        || path.ends_with(".tsbuildinfo")
        || matches!(
            path.rsplit('.').next(),
            Some("png" | "jpg" | "jpeg" | "ico" | "woff" | "woff2" | "svg" | "pdf")
        )
}
