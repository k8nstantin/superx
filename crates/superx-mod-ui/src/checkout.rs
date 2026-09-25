//! Which repository, and which branch, a working directory was in (#411).
//!
//! The transcript says neither. Its `cwd` follows every `cd`, so its last
//! segment is a subdirectory (`src`) or a worktree's name as often as a
//! repository's; its `gitBranch` is the branch of the directory the
//! session was LAUNCHED in, so a session working in a worktree tags every
//! line with another checkout's branch. The repository itself knows both:
//! `--git-common-dir` names the repository a worktree belongs to, and the
//! checkout's HEAD reflog records, with a timestamp, every time it moved
//! to another branch.
//!
//! Read-only, through `git`, under the same boundary as
//! [`crate::landed`]: nothing is written, and a directory git cannot
//! answer for belongs to no repository rather than to a guessed one.

use std::collections::HashMap;
use std::path::Path;

use chrono::{DateTime, TimeZone, Utc};

use crate::landed::{git, repo_name};

/// HEAD moves, oldest first: when, and the branch it moved to (`None`
/// for a detached HEAD).
type Moves = Vec<(DateTime<Utc>, Option<String>)>;

/// One checkout: a repository's main working tree, or one of its
/// worktrees.
#[derive(Debug, Clone)]
pub struct Checkout {
    /// The checkout's own root — a worktree's, not the main checkout's.
    pub toplevel: String,
    /// The repository's name: the directory holding the common `.git`,
    /// so every worktree of one repository reads as that repository.
    pub repo: String,
    /// The repository's common git dir — its identity.
    pub common: String,
    /// HEAD moves, oldest first.
    moves: Moves,
    /// The branch before the first recorded move, or — when the reflog
    /// records none — the branch it is on now.
    initial: Option<String>,
}

impl Checkout {
    /// The branch this checkout was on at `t`, or `None` when HEAD was
    /// detached then.
    #[must_use]
    pub fn branch_at(&self, t: DateTime<Utc>) -> Option<String> {
        match self.moves.iter().rposition(|(at, _)| *at <= t) {
            Some(i) => self.moves[i].1.clone(),
            None => self.initial.clone(),
        }
    }
}

/// Every checkout a set of directories resolved to.
#[derive(Debug, Default)]
pub struct Checkouts {
    list: Vec<Checkout>,
    /// directory → (index into `list`, whether the directory itself
    /// still exists). A vanished directory keeps its repository — found
    /// through its nearest surviving ancestor — but not a branch: the
    /// ancestor's checkout may not be the one it belonged to.
    by_dir: HashMap<String, Option<(usize, bool)>>,
}

impl Checkouts {
    /// Resolve every directory once. A directory git cannot answer for
    /// maps to nothing.
    pub async fn resolve<'a>(dirs: impl IntoIterator<Item = &'a String>) -> Self {
        let mut out = Self::default();
        let mut by_top: HashMap<String, usize> = HashMap::new();
        for dir in dirs {
            if out.by_dir.contains_key(dir) {
                continue;
            }
            let (probe, exists) = nearest_existing(dir);
            let resolved = match probe {
                Some(p) => rev_parse(&p).await,
                None => None,
            };
            let entry = match resolved {
                Some((toplevel, common)) => {
                    let idx = match by_top.get(&toplevel) {
                        Some(&i) => i,
                        None => {
                            let checkout = read_checkout(toplevel.clone(), common).await;
                            out.list.push(checkout);
                            by_top.insert(toplevel, out.list.len() - 1);
                            out.list.len() - 1
                        }
                    };
                    Some((idx, exists))
                }
                None => None,
            };
            out.by_dir.insert(dir.clone(), entry);
        }
        out
    }

    /// The checkout a directory belongs to.
    #[must_use]
    pub fn of(&self, dir: &str) -> Option<&Checkout> {
        self.by_dir.get(dir)?.map(|(i, _)| &self.list[i])
    }

    /// The branch a directory's checkout was on at `t` — only when the
    /// directory itself still exists, so the checkout is surely its own.
    #[must_use]
    pub fn branch_at(&self, dir: &str, t: DateTime<Utc>) -> Option<String> {
        let (i, exists) = (*self.by_dir.get(dir)?)?;
        exists.then(|| self.list[i].branch_at(t)).flatten()
    }

    /// The checkout whose tree holds `path` — the deepest one, so a
    /// worktree nested under its main checkout wins over it.
    #[must_use]
    pub fn holding(&self, path: &str) -> Option<&Checkout> {
        self.list
            .iter()
            .filter(|c| path.starts_with(&format!("{}/", c.toplevel)))
            .max_by_key(|c| c.toplevel.len())
    }
}

/// The directory itself when it exists, else its nearest ancestor that
/// does; and whether it was the directory itself.
fn nearest_existing(dir: &str) -> (Option<String>, bool) {
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

/// `(toplevel, common git dir)`, absolute, or `None` outside a repository.
async fn rev_parse(dir: &str) -> Option<(String, String)> {
    let out = git(
        Path::new(dir),
        &["rev-parse", "--path-format=absolute", "--show-toplevel", "--git-common-dir"],
    )
    .await?;
    let mut lines = out.lines().map(str::trim).filter(|l| !l.is_empty());
    let (top, common) = (lines.next()?, lines.next()?);
    Some((top.to_string(), common.to_string()))
}

/// Read a checkout's branch history from its HEAD reflog.
async fn read_checkout(toplevel: String, common: String) -> Checkout {
    let dir = Path::new(&toplevel);
    let remotes: Vec<String> = git(dir, &["remote"])
        .await
        .map(|s| s.lines().map(str::to_string).collect())
        .unwrap_or_default();
    let current = git(dir, &["symbolic-ref", "-q", "--short", "HEAD"])
        .await
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let reflog = git(dir, &["reflog", "show", "--date=unix", "--format=%gd%x09%gs", "HEAD"])
        .await
        .unwrap_or_default();
    let (moves, first_from) = parse_moves(&reflog, &remotes, current.as_deref());
    Checkout {
        repo: repo_name(&common, &toplevel),
        toplevel,
        common,
        initial: if moves.is_empty() { current } else { first_from },
        moves,
    }
}

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
fn parse_moves(reflog: &str, remotes: &[String], current: Option<&str>) -> (Moves, Option<String>) {
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
            // the work after it belongs to the new one (#415 review).
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
    // names a tag and `checkout --detach main` a branch, and both left
    // HEAD detached (#415 review). The newest move landed where HEAD is now.
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

    fn t(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).single().expect("valid unix time")
    }

    #[test]
    fn a_checkout_reads_the_branch_it_was_on_at_the_time() {
        let reflog = "HEAD@{400}\tcommit: third\n\
                      HEAD@{300}\tcheckout: moving from feat/a to 0123abcd\n\
                      HEAD@{200}\tcheckout: moving from main to feat/a\n\
                      HEAD@{100}\tcommit: first\n";
        let (moves, first_from) = parse_moves(reflog, &["origin".to_string()], None);
        let c = Checkout {
            toplevel: "/r".into(),
            repo: "r".into(),
            common: "/r/.git".into(),
            initial: first_from,
            moves,
        };
        assert_eq!(c.branch_at(t(150)).as_deref(), Some("main"), "before the first move");
        assert_eq!(c.branch_at(t(250)).as_deref(), Some("feat/a"));
        assert_eq!(c.branch_at(t(350)), None, "a commit id detaches HEAD");
    }

    #[test]
    fn a_remote_ref_or_a_hash_is_not_a_branch_and_a_slash_is_fine() {
        let remotes = ["origin".to_string()];
        assert_eq!(branch_name("origin/main", &remotes), None);
        assert_eq!(branch_name("5ac90056a26549866f97b573c9f53af2d1b6497c", &remotes), None);
        assert_eq!(branch_name("fix/409-status", &remotes).as_deref(), Some("fix/409-status"));
        assert_eq!(branch_name("main", &remotes).as_deref(), Some("main"));
    }

    #[test]
    fn a_tag_or_a_detach_is_no_branch_and_a_rename_moves_the_work() {
        // As git writes it, newest first: a tag checked out, back to main,
        // `checkout --detach main`, a new branch, renamed, then main again.
        let reflog = "HEAD@{800}\tcheckout: moving from new to main\n\
                      HEAD@{700}\tBranch: renamed refs/heads/old to refs/heads/new\n\
                      HEAD@{600}\tcheckout: moving from ab5f977e55cdf36d73abaf9227dd77d847f42ecc to old\n\
                      HEAD@{500}\tcheckout: moving from main to main\n\
                      HEAD@{400}\tcheckout: moving from 2c452e9412411ab0fb9d58c113554bb78db970f4 to main\n\
                      HEAD@{300}\tcheckout: moving from main to v1.0\n";
        let (moves, first_from) = parse_moves(reflog, &[], Some("main"));
        let c = Checkout {
            toplevel: "/r".into(),
            repo: "r".into(),
            common: "/r/.git".into(),
            initial: first_from,
            moves,
        };
        assert_eq!(c.branch_at(t(250)).as_deref(), Some("main"));
        assert_eq!(c.branch_at(t(350)), None, "a tag checked out detaches HEAD");
        assert_eq!(c.branch_at(t(450)).as_deref(), Some("main"));
        assert_eq!(c.branch_at(t(550)), None, "--detach names a branch and leaves it");
        assert_eq!(c.branch_at(t(650)).as_deref(), Some("old"));
        assert_eq!(c.branch_at(t(750)).as_deref(), Some("new"), "renamed: the work is the new name's");
        assert_eq!(c.branch_at(t(850)).as_deref(), Some("main"));
    }

    #[test]
    fn a_rebase_detaches_and_returns() {
        let reflog = "HEAD@{30}\trebase (finish): returning to refs/heads/feat/b\n\
                      HEAD@{20}\trebase (start): checkout main\n\
                      HEAD@{10}\tcheckout: moving from main to feat/b\n";
        let (moves, first_from) = parse_moves(reflog, &[], Some("feat/b"));
        let c = Checkout {
            toplevel: "/r".into(),
            repo: "r".into(),
            common: "/r/.git".into(),
            initial: first_from,
            moves,
        };
        assert_eq!(c.branch_at(t(15)).as_deref(), Some("feat/b"));
        assert_eq!(c.branch_at(t(25)), None);
        assert_eq!(c.branch_at(t(35)).as_deref(), Some("feat/b"));
    }
}
