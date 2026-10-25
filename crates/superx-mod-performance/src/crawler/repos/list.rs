//! Where a working directory lives (#501): its nearest folder that still
//! exists, the checkout whose tree holds it, and the repository that
//! checkout belongs to. A repository is its common git directory, so every
//! worktree of one repository is that repository and a separate clone is
//! one of its own. And what a checkout is on: the branch HEAD is on now,
//! and the one it was on before the oldest move its reflog still holds.
//!
//! Read-only, through [`super::git`]. A directory git cannot answer for
//! belongs to no repository, never to a guessed one. The reflog reading is
//! the one superx-mod-ui's `checkout.rs` does; a module never imports
//! another (D24), so it lives here too.

use std::path::Path;

use chrono::{DateTime, TimeZone, Utc};

use crate::crawler::repos::git::Git;

/// A directory, resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    /// The checkout's own root: a worktree's, not its main checkout's.
    pub toplevel: String,
    /// The repository's common git directory.
    pub common: String,
    /// The directory itself exists. When it does not, only an ancestor was
    /// found, so the checkout found may not be the one it belonged to.
    pub exact: bool,
}

/// The checkout and repository holding `dir`, through its nearest folder
/// that still exists; `None` when no repository holds it.
pub async fn locate(git: &mut Git, dir: &str) -> Option<Place> {
    let (probe, exact) = nearest_existing(dir);
    let probe = probe?;
    let call =
        git.run(Path::new(&probe), &["rev-parse", "--path-format=absolute", "--show-toplevel", "--git-common-dir"]).await;
    let out = call.ok()?;
    let mut lines = out.lines().map(str::trim).filter(|l| !l.is_empty());
    let (toplevel, common) = (lines.next()?, lines.next()?);
    Some(Place { toplevel: toplevel.to_string(), common: common.to_string(), exact })
}

/// The directory itself when it exists, else its nearest ancestor that
/// does; and whether it was the directory itself.
#[must_use]
pub fn nearest_existing(dir: &str) -> (Option<String>, bool) {
    let mut p = Path::new(dir);
    if p.is_dir() {
        return (Some(dir.to_string()), true);
    }
    while let Some(parent) = p.parent() {
        // `/` itself is never a repository worth naming.
        if parent.parent().is_none() {
            break;
        }
        if parent.is_dir() {
            return (Some(parent.to_string_lossy().into_owned()), false);
        }
        p = parent;
    }
    (None, false)
}

/// A repository's name: the directory holding `.git`, so every worktree of
/// one repository reads as that repository; else the checkout's own.
#[must_use]
pub fn repository_name(common: &str, toplevel: &str) -> String {
    let common = Path::new(common);
    let holder = if common.file_name().is_some_and(|n| n == ".git") { common.parent() } else { None };
    holder
        .unwrap_or_else(|| Path::new(toplevel))
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| toplevel.to_string())
}

/// What a checkout is on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Branches {
    /// The branch HEAD is on now; `None` when it is detached.
    pub branch: Option<String>,
    /// The branch before the oldest move its reflog still holds: what it
    /// was on before any recorded move. With no move recorded, the branch
    /// it is on now.
    pub first_branch: Option<String>,
}

/// Read a checkout's branches from its HEAD and its HEAD reflog.
pub async fn branches(git: &mut Git, toplevel: &str) -> Branches {
    let dir = Path::new(toplevel);
    let remotes: Vec<String> =
        git.run(dir, &["remote"]).await.ok().map(|s| s.lines().map(str::to_string).collect()).unwrap_or_default();
    let branch = git
        .run(dir, &["symbolic-ref", "-q", "--short", "HEAD"])
        .await
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let reflog = git.run(dir, &["reflog", "show", "--date=unix", "--format=%gd%x09%gs", "HEAD"]).await;
    let (moves, first_from) = parse_moves(reflog.ok().unwrap_or(""), &remotes, branch.as_deref());
    let first_branch = if moves.is_empty() { branch.clone() } else { first_from };
    Branches { branch, first_branch }
}

/// HEAD moves, oldest first: when, and the branch it moved to (`None` for a
/// detached HEAD).
pub type Moves = Vec<(DateTime<Utc>, Option<String>)>;

/// One reflog entry that moved HEAD, as its text reads.
struct RawMove {
    at: DateTime<Utc>,
    from: Option<String>,
    to: Option<String>,
    /// A `checkout: moving from … to …` entry, whose `from` says what HEAD
    /// was really on.
    checkout: bool,
}

/// HEAD moves from a `%gd\t%gs` reflog listing, oldest first, and the
/// branch the oldest one moved FROM. `current` is the branch HEAD is on
/// now, `None` when detached.
#[must_use]
pub fn parse_moves(reflog: &str, remotes: &[String], current: Option<&str>) -> (Moves, Option<String>) {
    let mut moves: Vec<RawMove> = Vec::new();
    for line in reflog.lines() {
        let Some((selector, subject)) = line.split_once('\t') else { continue };
        let Some(at) = selector
            .strip_prefix("HEAD@{")
            .and_then(|s| s.strip_suffix('}'))
            .and_then(|s| s.parse::<i64>().ok())
            .and_then(|t| Utc.timestamp_opt(t, 0).single())
        else {
            continue;
        };
        if let Some(rest) = subject.strip_prefix("checkout: moving from ") {
            if let Some((from, to)) = rest.rsplit_once(" to ") {
                moves.push(RawMove { at, from: branch_name(from, remotes), to: branch_name(to, remotes), checkout: true });
            }
        } else if let Some(rest) = subject.strip_prefix("Branch: renamed refs/heads/") {
            // `git branch -m`: the branch HEAD is on takes a new name, and
            // the work after it belongs to the new one.
            if let Some((old, new)) = rest.split_once(" to refs/heads/") {
                moves.push(RawMove { at, from: Some(old.to_string()), to: Some(new.trim().to_string()), checkout: false });
            }
        } else if let Some(idx) = subject.find("returning to refs/heads/") {
            // `rebase (finish)` / `rebase (abort)`: back on the branch.
            let b = &subject[idx + "returning to refs/heads/".len()..];
            moves.push(RawMove { at, from: None, to: Some(b.trim().to_string()), checkout: false });
        } else if subject.starts_with("rebase") && subject.contains("(start): checkout ") {
            // A rebase runs on a detached HEAD until it returns.
            moves.push(RawMove { at, from: None, to: None, checkout: false });
        }
    }
    // The listing is newest first.
    moves.reverse();
    // Where a checkout LANDED is what the next checkout moved away from:
    // git names a branch there only if HEAD was on one. `checkout v1.0`
    // names a tag and `checkout --detach main` a branch, and both left HEAD
    // detached. The newest move landed where HEAD is now.
    let landed: Vec<Option<String>> = (0..moves.len())
        .map(|i| match moves.get(i + 1) {
            Some(next) if next.checkout => next.from.clone(),
            Some(_) => moves[i].to.clone(),
            None if moves[i].checkout => current.map(str::to_string),
            None => moves[i].to.clone(),
        })
        .collect();
    let first_from = moves.first().and_then(|m| m.from.clone());
    (moves.into_iter().zip(landed).map(|(m, to)| (m.at, to)).collect(), first_from)
}

/// A reflog endpoint as a branch, or `None` when it detached HEAD: a
/// commit id, or a remote-tracking ref (`origin/main`).
fn branch_name(name: &str, remotes: &[String]) -> Option<String> {
    let name = name.trim();
    let is_hash = name.len() >= 7 && name.bytes().all(|b| b.is_ascii_hexdigit());
    let is_remote = remotes.iter().any(|r| name.starts_with(&format!("{r}/")));
    (!name.is_empty() && !is_hash && !is_remote).then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_branch_is_the_one_before_the_oldest_move() {
        let reflog = "HEAD@{400}\tcommit: third\n\
                      HEAD@{300}\tcheckout: moving from feat/a to 0123abcd\n\
                      HEAD@{200}\tcheckout: moving from main to feat/a\n\
                      HEAD@{100}\tcommit: first\n";
        let (moves, first_from) = parse_moves(reflog, &["origin".to_string()], None);
        assert_eq!(first_from.as_deref(), Some("main"));
        let to: Vec<Option<&str>> = moves.iter().map(|(_, b)| b.as_deref()).collect();
        assert_eq!(to, [Some("feat/a"), None], "a commit id detaches HEAD");
    }

    #[test]
    fn a_tag_or_a_detach_is_no_branch_and_a_rename_moves_the_work() {
        let reflog = "HEAD@{800}\tcheckout: moving from new to main\n\
                      HEAD@{700}\tBranch: renamed refs/heads/old to refs/heads/new\n\
                      HEAD@{600}\tcheckout: moving from ab5f977e55cdf36d73abaf9227dd77d847f42ecc to old\n\
                      HEAD@{500}\tcheckout: moving from main to main\n\
                      HEAD@{400}\tcheckout: moving from 2c452e9412411ab0fb9d58c113554bb78db970f4 to main\n\
                      HEAD@{300}\tcheckout: moving from main to v1.0\n";
        let (moves, first_from) = parse_moves(reflog, &[], Some("main"));
        assert_eq!(first_from.as_deref(), Some("main"));
        let to: Vec<Option<&str>> = moves.iter().map(|(_, b)| b.as_deref()).collect();
        assert_eq!(to, [None, Some("main"), None, Some("old"), Some("new"), Some("main")]);
    }

    #[test]
    fn a_remote_ref_or_a_hash_is_not_a_branch() {
        let remotes = ["origin".to_string()];
        assert_eq!(branch_name("origin/main", &remotes), None);
        assert_eq!(branch_name("5ac90056a26549866f97b573c9f53af2d1b6497c", &remotes), None);
        assert_eq!(branch_name("fix/409-status", &remotes).as_deref(), Some("fix/409-status"));
    }

    #[test]
    fn a_repository_is_named_for_the_directory_holding_its_git_directory() {
        assert_eq!(repository_name("/w/SuperX/.git", "/w/SuperX/.claude/worktrees/x"), "SuperX");
        assert_eq!(repository_name("/w/bare.git", "/w/bare"), "bare");
        assert_eq!(repository_name("/w/a/.git", "/w/a"), "a");
    }

    #[test]
    fn a_vanished_directory_is_found_through_its_nearest_ancestor() {
        let here = env!("CARGO_MANIFEST_DIR");
        assert_eq!(nearest_existing(here), (Some(here.to_string()), true));
        let gone = format!("{here}/no/such/dir");
        assert_eq!(nearest_existing(&gone), (Some(here.to_string()), false));
        assert_eq!(nearest_existing("/no-such-root-dir/x"), (None, false));
    }
}
