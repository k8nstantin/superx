//! Level 2: did a build write only what its task file owns?
//!
//! The root's method `ownership` step (2026-10-13): "Create, modify and
//! touch only the paths the task file lists under owns; a path it lists
//! under forbid is never opened for writing; a path it does not list is
//! not touched. A PR that touches another path is closed."
//!
//! So every file the build wrote inside the repository must be one its
//! task file's `owns` names. A write to a `forbid` path is named as
//! such; a write to any other repository path is "not owned". The
//! repository is the directory the build was launched in (the kickoff
//! line's `cwd`); a file written outside it — a scratch file, a draft of
//! the PR body — is not the repository's and is not judged.
//!
//! What this cannot see: a write through the shell (`sed -i`, `cat >`,
//! a script). Only the writing tools are counted, and the result says
//! so.

use crate::read_order::Judgement;
use crate::session::Write;

/// Judge every write a build made against its task file's lists.
#[must_use]
pub fn judge(writes: &[&Write], repository: Option<&str>, owns: &[String], forbid: &[String]) -> Judgement {
    let Some(root) = repository.map(|r| r.trim_end_matches('/')).filter(|r| !r.is_empty()) else {
        return Judgement {
            result: "no-evidence",
            note: "the build's launch directory is unknown, so its repository is".to_string(),
            evidence: Vec::new(),
        };
    };
    if owns.is_empty() {
        return Judgement {
            result: "no-evidence",
            note: "the task file names no path it owns".to_string(),
            evidence: Vec::new(),
        };
    }
    let mut forbidden: Vec<(String, Option<String>)> = Vec::new();
    let mut unowned: Vec<(String, Option<String>)> = Vec::new();
    let mut inside = 0usize;
    for w in writes {
        let Some(rel) = relative(&w.path, root) else { continue };
        inside += 1;
        if owns.iter().any(|o| matches(&rel, o)) {
            continue;
        }
        let entry = (rel.clone(), w.tool_use_id.clone());
        if forbid.iter().any(|f| matches(&rel, f)) {
            if !forbidden.iter().any(|(p, _)| *p == rel) {
                forbidden.push(entry);
            }
        } else if !unowned.iter().any(|(p, _)| *p == rel) {
            unowned.push(entry);
        }
    }
    if inside == 0 {
        return Judgement {
            result: "no-evidence",
            note: "the build wrote nothing in its repository through the writing tools".to_string(),
            evidence: Vec::new(),
        };
    }
    if forbidden.is_empty() && unowned.is_empty() {
        return Judgement {
            result: "complied",
            note: format!("wrote only what the task file owns ({inside} writes in the repository)"),
            evidence: Vec::new(),
        };
    }
    let mut problems = Vec::new();
    if !forbidden.is_empty() {
        problems.push(format!(
            "wrote what the task file forbids: {}",
            forbidden.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>().join(", ")
        ));
    }
    if !unowned.is_empty() {
        problems.push(format!(
            "wrote what the task file does not own: {}",
            unowned.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>().join(", ")
        ));
    }
    Judgement {
        result: "breached",
        note: problems.join("; "),
        evidence: forbidden.iter().chain(unowned.iter()).filter_map(|(_, id)| id.clone()).collect(),
    }
}

/// A written path relative to the repository, or `None` outside it. A
/// Claude Code worktree inside the repository (`.claude/worktrees/<n>/`)
/// is the repository too.
#[must_use]
pub fn relative(path: &str, root: &str) -> Option<String> {
    let rest = path.strip_prefix(root)?.strip_prefix('/')?;
    let rest = match rest.strip_prefix(".claude/worktrees/") {
        Some(inner) => inner.split_once('/').map_or("", |(_, r)| r),
        None => rest,
    };
    (!rest.is_empty()).then(|| rest.to_string())
}

/// A repository path against one entry of `owns` or `forbid`: the file
/// itself, everything under a directory (`dir/`), or a glob (`dir/**`,
/// `dir/*.py`).
#[must_use]
pub fn matches(rel: &str, pattern: &str) -> bool {
    let pattern = pattern.trim_start_matches("./");
    if let Some(dir) = pattern.strip_suffix("/**") {
        return rel.starts_with(&format!("{dir}/"));
    }
    if pattern.ends_with('/') {
        return rel.starts_with(pattern);
    }
    if let Some((dir, tail)) = pattern.rsplit_once("/*") {
        // `dir/*.py`: one level under `dir`, ending as the tail says.
        return rel
            .strip_prefix(&format!("{dir}/"))
            .is_some_and(|f| !f.contains('/') && f.ends_with(tail));
    }
    rel == pattern
}
