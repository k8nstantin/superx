//! The reflogs (#538, step 8): every entry git keeps, read from its reflog
//! file as git writes it, one line each: the old and the new commit, the
//! identity, the time and its offset, then a tab and the message. A
//! repository whose refs git keeps in a reftable is read with `git reflog
//! show` instead, its old commits unknown.

use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeZone as _, Utc};

use crate::crawler::repos::commits::is_hash;

/// One reflog entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The ref's full name; `HEAD` for a checkout's.
    pub name: String,
    pub at: DateTime<Utc>,
    /// `None` for git's zero id.
    pub old: Option<String>,
    pub new: Option<String>,
    /// The email git recorded.
    pub identity: Option<String>,
    /// The message, whole.
    pub action: String,
}

/// A commit an entry names: `None` for git's zero id.
fn named(id: &str) -> Option<String> {
    (is_hash(id) && id.bytes().any(|b| b != b'0')).then(|| id.to_string())
}

/// The reflog files under a git directory's `logs` folder: `HEAD` and
/// every ref under `refs/`, by its full name.
#[must_use]
pub fn files(git_dir: &Path) -> Vec<(String, PathBuf)> {
    let logs = git_dir.join("logs");
    let mut out = Vec::new();
    let head = logs.join("HEAD");
    if head.is_file() {
        out.push(("HEAD".to_string(), head));
    }
    let mut folders = vec![logs.join("refs")];
    while let Some(folder) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                folders.push(path);
            } else if let Ok(name) = path.strip_prefix(&logs) {
                out.push((name.to_string_lossy().into_owned(), path.clone()));
            }
        }
    }
    out.sort();
    out
}

/// The entries of one reflog file, and the lines in no shape git writes.
#[must_use]
pub fn parse(name: &str, text: &str) -> (Vec<Entry>, i64) {
    let mut entries = Vec::new();
    let mut unread = 0;
    for line in text.lines().filter(|l| !l.is_empty()) {
        match entry(name, line) {
            Some(e) => entries.push(e),
            None => unread += 1,
        }
    }
    (entries, unread)
}

/// `<old> <new> <name> <<email>> <seconds> <offset>\t<message>`.
fn entry(name: &str, line: &str) -> Option<Entry> {
    let (head, action) = line.split_once('\t').unwrap_or((line, ""));
    let (old, rest) = head.split_once(' ')?;
    let (new, who) = rest.split_once(' ')?;
    let (identity, when) = who.rsplit_once('>')?;
    let email = identity.rsplit_once('<').map(|(_, e)| e.to_string())?;
    let seconds: i64 = when.split_whitespace().next()?.parse().ok()?;
    if !is_hash(old) || !is_hash(new) {
        return None;
    }
    Some(Entry {
        name: name.to_string(),
        at: Utc.timestamp_opt(seconds, 0).single()?,
        old: named(old),
        new: named(new),
        identity: Some(email).filter(|e| !e.is_empty()),
        action: action.to_string(),
    })
}

/// The format `git reflog show` prints a reftable's entries in, with
/// `--date=unix`: the new commit, the email, the selector with its time,
/// the message, NUL between them.
pub const SHOW_FORMAT: &str = "--format=%H%x00%ge%x00%gd%x00%gs";

/// The entries `git reflog show` printed for `name` in [`SHOW_FORMAT`],
/// old unknown, and the lines in no shape it prints.
#[must_use]
pub fn shown(name: &str, out: &str) -> (Vec<Entry>, i64) {
    let mut entries = Vec::new();
    let mut unread = 0;
    for line in out.lines().filter(|l| !l.is_empty()) {
        let fields: Vec<&str> = line.split('\0').collect();
        let at = fields
            .get(2)
            .and_then(|s| s.rsplit_once("@{"))
            .and_then(|(_, t)| t.strip_suffix('}'))
            .and_then(|t| t.parse::<i64>().ok())
            .and_then(|t| Utc.timestamp_opt(t, 0).single());
        match (fields.first(), at) {
            (Some(new), Some(at)) if fields.len() == 4 => entries.push(Entry {
                name: name.to_string(),
                at,
                old: None,
                new: named(new),
                identity: Some(fields[1].to_string()).filter(|e| !e.is_empty()),
                action: fields[3].to_string(),
            }),
            _ => unread += 1,
        }
    }
    (entries, unread)
}

/// The checkouts' own git directories under a repository's common one:
/// the main checkout's, the common directory itself, when it is a
/// checkout's `.git`; and each linked worktree's, with the root its
/// `gitdir` file names.
#[must_use]
pub fn worktrees(common: &Path) -> Vec<(PathBuf, Option<PathBuf>)> {
    let mut out = Vec::new();
    if common.file_name().is_some_and(|n| n == ".git") {
        out.push((common.to_path_buf(), common.parent().map(Path::to_path_buf)));
    } else {
        out.push((common.to_path_buf(), None));
    }
    let Ok(entries) = std::fs::read_dir(common.join("worktrees")) else { return out };
    let mut linked: Vec<(PathBuf, Option<PathBuf>)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .map(|dir| {
            let root = std::fs::read_to_string(dir.join("gitdir"))
                .ok()
                .map(|g| PathBuf::from(g.trim()))
                .and_then(|g| g.parent().map(Path::to_path_buf));
            (dir, root)
        })
        .collect();
    linked.sort();
    out.extend(linked);
    out
}
