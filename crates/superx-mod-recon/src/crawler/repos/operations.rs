//! What a checkout is in the middle of (#538, step 12), read from the state
//! files of the checkout's own git directory, the ones `git status` reads,
//! never through a command that touches the checkout.

use std::path::{Path, PathBuf};

/// What git is in the middle of in one checkout.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Operations {
    /// Each that is there, in the order `merge`, `am`, `rebase`,
    /// `rebase_interactive`, `cherry_pick`, `revert`, `bisect`.
    pub operations: Vec<String>,
    /// The commits `MERGE_HEAD`, `CHERRY_PICK_HEAD` and `REVERT_HEAD` name,
    /// in that order; none when no file names one.
    pub operation_commits: Option<Vec<String>>,
    pub rebase_branch: Option<String>,
    pub rebase_onto: Option<String>,
    pub rebase_done: Option<i64>,
    pub rebase_left: Option<i64>,
    pub bisect_start: Option<String>,
}

/// A checkout's own git directory: `<toplevel>/.git` when that is a
/// directory; the one its `gitdir:` line names when it is a file, as a
/// worktree's is.
#[must_use]
pub fn git_dir(toplevel: &Path) -> Option<PathBuf> {
    let dot_git = toplevel.join(".git");
    if dot_git.is_dir() {
        return Some(dot_git);
    }
    let text = std::fs::read_to_string(&dot_git).ok()?;
    let named = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
    let named = Path::new(named);
    Some(if named.is_absolute() { named.to_path_buf() } else { toplevel.join(named) })
}

/// What the state files of `git_dir` say.
#[must_use]
pub fn read(git_dir: &Path) -> Operations {
    let at = |name: &str| git_dir.join(name);
    let rebase_apply = at("rebase-apply");
    let rebase_merge = at("rebase-merge");
    let mut operations = Vec::new();
    let mut add = |name: &str, there: bool| {
        if there {
            operations.push(name.to_string());
        }
    };
    add("merge", at("MERGE_HEAD").is_file());
    add("am", rebase_apply.join("applying").exists());
    add(
        "rebase",
        (rebase_apply.is_dir() && !rebase_apply.join("applying").exists())
            || (rebase_merge.is_dir() && !rebase_merge.join("interactive").exists()),
    );
    add("rebase_interactive", rebase_merge.join("interactive").exists());
    add("cherry_pick", at("CHERRY_PICK_HEAD").is_file());
    add("revert", at("REVERT_HEAD").is_file());
    add("bisect", at("BISECT_LOG").is_file());

    let commits: Vec<String> = ["MERGE_HEAD", "CHERRY_PICK_HEAD", "REVERT_HEAD"]
        .iter()
        .filter_map(|f| std::fs::read_to_string(at(f)).ok())
        .flat_map(|text| text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect::<Vec<_>>())
        .collect();
    let in_rebase = |file: &str| {
        std::fs::read_to_string(rebase_merge.join(file)).or_else(|_| std::fs::read_to_string(rebase_apply.join(file))).ok()
    };
    Operations {
        operations,
        operation_commits: (!commits.is_empty()).then_some(commits),
        rebase_branch: in_rebase("head-name").map(|n| {
            let n = n.trim();
            n.strip_prefix("refs/heads/").unwrap_or(n).to_string()
        }),
        rebase_onto: in_rebase("onto").map(|o| o.trim().to_string()).filter(|o| !o.is_empty()),
        rebase_done: commands(&rebase_merge.join("done")),
        rebase_left: commands(&rebase_merge.join("git-rebase-todo")),
        bisect_start: std::fs::read_to_string(at("BISECT_START"))
            .ok()
            .map(|s| s.strip_suffix('\n').unwrap_or(&s).to_string()),
    }
}

/// The commands of an interactive rebase's list: a line that is empty or
/// starts with `#` is none. None when the file is not there.
fn commands(path: &Path) -> Option<i64> {
    let text = std::fs::read_to_string(path).ok()?;
    let n = text.lines().filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#')).count();
    Some(i64::try_from(n).unwrap_or(i64::MAX))
}
