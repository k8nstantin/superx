//! What git is in the middle of in a checkout (#538, step 12), read from
//! the state files of the checkout's own git directory, never through a
//! command that touches the checkout: the files `git status` reads.

use std::path::{Path, PathBuf};

use crate::crawler::repos::commits::is_hash;

/// A checkout's operations, as a `checkout_state` row holds them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Operations {
    /// In this order, each that is there: `merge`, `am`, `rebase`,
    /// `rebase_interactive`, `cherry_pick`, `revert`, `bisect`.
    pub operations: Vec<String>,
    /// The commits `MERGE_HEAD`, `CHERRY_PICK_HEAD` and `REVERT_HEAD` name,
    /// in that order; `None` when none.
    pub commits: Option<Vec<String>>,
    pub rebase_branch: Option<String>,
    pub rebase_onto: Option<String>,
    pub rebase_done: Option<i64>,
    pub rebase_left: Option<i64>,
    pub bisect_start: Option<String>,
}

/// A checkout's own git directory: its `.git` folder, or the folder its
/// `.git` file names.
#[must_use]
pub fn git_dir(toplevel: &Path) -> Option<PathBuf> {
    let dot = toplevel.join(".git");
    if dot.is_dir() {
        return Some(dot);
    }
    let named = std::fs::read_to_string(&dot).ok()?;
    let path = PathBuf::from(named.trim().strip_prefix("gitdir:")?.trim());
    Some(if path.is_absolute() { path } else { toplevel.join(path) })
}

/// The commands a rebase's todo list holds: a line that is empty or
/// starts with `#` is none.
fn commands(path: &Path) -> Option<i64> {
    let text = std::fs::read_to_string(path).ok()?;
    let n = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).count();
    Some(i64::try_from(n).unwrap_or(i64::MAX))
}

/// What git is in the middle of, from the state files under `git_dir`.
#[must_use]
pub fn read(git_dir: &Path) -> Operations {
    let at = |name: &str| git_dir.join(name);
    let text = |name: &str| std::fs::read_to_string(at(name)).ok();
    let apply = at("rebase-apply");
    let merge = at("rebase-merge");
    let interactive = merge.join("interactive").exists();
    let mut ops = Operations::default();
    let mut there = |word: &str, yes: bool| {
        if yes {
            ops.operations.push(word.to_string());
        }
    };
    there("merge", at("MERGE_HEAD").exists());
    there("am", apply.join("applying").exists());
    there("rebase", (apply.is_dir() && !apply.join("applying").exists()) || (merge.is_dir() && !interactive));
    there("rebase_interactive", interactive);
    there("cherry_pick", at("CHERRY_PICK_HEAD").exists());
    there("revert", at("REVERT_HEAD").exists());
    there("bisect", at("BISECT_LOG").exists());
    let commits: Vec<String> = ["MERGE_HEAD", "CHERRY_PICK_HEAD", "REVERT_HEAD"]
        .iter()
        .filter_map(|name| text(name))
        .flat_map(|t| t.lines().map(str::trim).filter(|h| is_hash(h)).map(str::to_string).collect::<Vec<_>>())
        .collect();
    ops.commits = (!commits.is_empty()).then_some(commits);
    let state = if merge.is_dir() { Some(&merge) } else if apply.is_dir() { Some(&apply) } else { None };
    if let Some(state) = state {
        let read = |name: &str| std::fs::read_to_string(state.join(name)).ok().map(|t| t.trim().to_string());
        ops.rebase_branch = read("head-name").and_then(|h| h.strip_prefix("refs/heads/").map(str::to_string));
        ops.rebase_onto = read("onto").filter(|h| is_hash(h));
    }
    ops.rebase_done = commands(&merge.join("done"));
    ops.rebase_left = commands(&merge.join("git-rebase-todo"));
    ops.bisect_start = text("BISECT_START").map(|t| t.trim_end_matches(['\r', '\n']).to_string());
    ops
}
