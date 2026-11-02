//! Reflog entries (#538, step 8), read from the reflog files as git writes
//! them, one line each: the old and the new commit, the identity's name and
//! `<email>`, the time in seconds and its offset, then a tab and the
//! message. A repository whose refs git keeps in a reftable is read with
//! `git reflog show` instead, its old commit unknown.

use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeZone as _, Utc};

/// One reflog entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// None for git's zero id, and where the reftable does not say.
    pub old: Option<String>,
    pub new: Option<String>,
    /// The email git recorded.
    pub identity: Option<String>,
    pub at: DateTime<Utc>,
    /// The message, whole.
    pub action: String,
}

/// One reflog file: the ref it logs, and where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Log {
    /// The ref's full name; `HEAD` for a HEAD.
    pub name: String,
    pub path: PathBuf,
}

/// Every reflog file under `git_dir`: `logs/HEAD` and every file under
/// `logs/refs/`, in the order of their names.
#[must_use]
pub fn logs(git_dir: &Path) -> Vec<Log> {
    let logs = git_dir.join("logs");
    let mut out = Vec::new();
    if logs.join("HEAD").is_file() {
        out.push(Log { name: "HEAD".to_string(), path: logs.join("HEAD") });
    }
    let mut refs = Vec::new();
    walk(&logs.join("refs"), "refs", &mut refs);
    refs.sort_by(|a, b| a.name.cmp(&b.name));
    out.extend(refs);
    out
}

fn walk(dir: &Path, name: &str, out: &mut Vec<Log>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let child = format!("{name}/{}", entry.file_name().to_string_lossy());
        if path.is_dir() {
            walk(&path, &child, out);
        } else {
            out.push(Log { name: child, path });
        }
    }
}

/// The worktrees of a repository: each one's own git directory under the
/// common directory's `worktrees/`, and its root as its `gitdir` file names
/// it (the `.git` file in the worktree's root).
#[must_use]
pub fn worktrees(common: &Path) -> Vec<(PathBuf, Option<String>)> {
    let Ok(entries) = std::fs::read_dir(common.join("worktrees")) else { return Vec::new() };
    let mut out: Vec<(PathBuf, Option<String>)> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| {
            let dir = e.path();
            let root = std::fs::read_to_string(dir.join("gitdir")).ok().and_then(|g| {
                let g = PathBuf::from(g.trim());
                g.parent().map(|p| p.to_string_lossy().into_owned())
            });
            (dir, root)
        })
        .collect();
    out.sort();
    out
}

/// The entries of one reflog file, oldest first, as git appended them.
#[must_use]
pub fn parse(text: &str) -> Vec<Entry> {
    text.lines().filter_map(entry).collect()
}

fn entry(line: &str) -> Option<Entry> {
    let (left, action) = line.split_once('\t').unwrap_or((line, ""));
    let mut words = left.splitn(3, ' ');
    let old = words.next()?;
    let new = words.next()?;
    let ident = words.next()?;
    let close = ident.rfind('>')?;
    let open = ident[..close].rfind('<')?;
    let email = &ident[open + 1..close];
    let secs: i64 = ident[close + 1..].split_whitespace().next()?.parse().ok()?;
    Some(Entry {
        old: commit(old),
        new: commit(new),
        identity: (!email.is_empty()).then(|| email.to_string()),
        at: Utc.timestamp_opt(secs, 0).single()?,
        action: action.to_string(),
    })
}

/// A commit id, or none for git's zero id.
fn commit(id: &str) -> Option<String> {
    (!id.is_empty() && !id.bytes().all(|b| b == b'0')).then(|| id.to_string())
}

/// What `git reflog show --date=unix --format=%H%x09%ge%x09%gd%x09%gs <ref>`
/// printed: the reftable's entries, newest first as git lists them, each
/// with its new commit and no old one.
#[must_use]
pub fn parse_shown(out: &str) -> Vec<Entry> {
    out.lines()
        .filter_map(|line| {
            let mut f = line.splitn(4, '\t');
            let new = f.next()?;
            let email = f.next()?;
            let selector = f.next()?;
            let action = f.next().unwrap_or_default();
            let secs: i64 = selector.rsplit_once("@{")?.1.trim_end_matches('}').parse().ok()?;
            Some(Entry {
                old: None,
                new: commit(new),
                identity: (!email.is_empty()).then(|| email.to_string()),
                at: Utc.timestamp_opt(secs, 0).single()?,
                action: action.to_string(),
            })
        })
        .collect()
}
