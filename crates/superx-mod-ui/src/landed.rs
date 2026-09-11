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

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use tokio::process::Command;

use crate::api::{ChurnPoint, Landed, LandedRepo};

/// Repositories read per request, at most.
const LANDED_REPOS: usize = 8; // skill-allow: §9-const — read-path bound, not a policy tunable
/// A git call slower than this is abandoned and counted unreadable.
const GIT_TIMEOUT_MS: u64 = 4_000; // skill-allow: §9-const — read-path bound, not a policy tunable

/// `git -C dir args…`, stdout on success, `None` on any failure or on
/// the timeout. No shell is involved: arguments are passed as given.
async fn git(dir: &Path, args: &[&str]) -> Option<String> {
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
async fn main_ref(dir: &Path) -> String {
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

/// The repository's name: the directory holding `.git` — for a
/// worktree, the main checkout's, so worktrees of one repo read as one.
fn repo_name(common_git_dir: &str, toplevel: &str) -> String {
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
