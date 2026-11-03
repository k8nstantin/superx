//! The commits (#538, steps 6 and 7), as git prints them: a commit object
//! whole from `git cat-file --batch`, its file changes against its first
//! parent from `git log -z --raw --numstat`, the same counted with
//! whitespace ignored, its patch id from `git patch-id --stable`, and the
//! commits `git fsck --unreachable` names. Nothing here runs git.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, TimeZone as _, Utc};

/// One commit, read whole from its object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub tree: String,
    /// First parent first.
    pub parent_hashes: Vec<String>,
    pub author: Identity,
    pub committer: Identity,
    /// The message's first line, as written.
    pub subject: String,
    /// What follows that line, the empty lines that open it removed,
    /// whole; `None` when nothing follows.
    pub body: Option<String>,
    /// The `gpgsig` header whole, its lines joined by newlines without the
    /// space that opens each line after the first.
    pub signature: Option<String>,
}

/// An author or a committer, as a commit object writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub name: String,
    pub email: String,
    pub at: DateTime<Utc>,
    /// The offset git recorded beside the time, in minutes east of UTC.
    pub offset: i64,
}

/// The objects `git cat-file --batch` printed: each commit by its hash,
/// `None` for one git did not have or that is not a commit it could read.
#[must_use]
pub fn batch(out: &[u8]) -> Vec<(String, Option<Commit>)> {
    let mut found = Vec::new();
    let mut rest = out;
    while !rest.is_empty() {
        let Some(end) = rest.iter().position(|b| *b == b'\n') else { break };
        let header = String::from_utf8_lossy(&rest[..end]).into_owned();
        rest = &rest[end + 1..];
        let mut fields = header.split(' ');
        let hash = fields.next().unwrap_or_default().to_string();
        let (Some(kind), Some(size)) = (fields.next(), fields.next().and_then(|s| s.parse::<usize>().ok())) else {
            // `<object> missing`, or `ambiguous`: no content follows.
            found.push((hash, None));
            continue;
        };
        let Some(content) = rest.get(..size) else { break };
        found.push((hash.clone(), (kind == "commit").then(|| commit(&hash, content)).flatten()));
        rest = rest.get(size + 1..).unwrap_or_default();
    }
    found
}

/// A commit object's content.
#[must_use]
pub fn commit(hash: &str, raw: &[u8]) -> Option<Commit> {
    let text = String::from_utf8_lossy(raw);
    let (head, message) = match text.find("\n\n") {
        Some(at) => (&text[..at], &text[at + 2..]),
        None => (text.trim_end_matches('\n'), ""),
    };
    // Each header with its continuation lines, those opening with a space.
    let mut headers: Vec<(String, String)> = Vec::new();
    for line in head.split('\n') {
        match (line.strip_prefix(' '), headers.last_mut()) {
            (Some(more), Some((_, value))) => {
                value.push('\n');
                value.push_str(more);
            }
            _ => {
                let (key, value) = line.split_once(' ').unwrap_or((line, ""));
                headers.push((key.to_string(), value.to_string()));
            }
        }
    }
    let first = |key: &str| headers.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
    let (subject, after) = message.split_once('\n').unwrap_or((message, ""));
    let body = after.trim_start_matches('\n');
    Some(Commit {
        hash: hash.to_string(),
        tree: first("tree")?,
        parent_hashes: headers.iter().filter(|(k, _)| k == "parent").map(|(_, v)| v.clone()).collect(),
        author: identity(&first("author")?)?,
        committer: identity(&first("committer")?)?,
        subject: subject.to_string(),
        body: (!body.is_empty()).then(|| body.to_string()),
        signature: first("gpgsig"),
    })
}

/// `Name <email> <seconds> <offset>`.
fn identity(line: &str) -> Option<Identity> {
    let (who, when) = line.rsplit_once('>')?;
    let (name, email) = who.rsplit_once('<')?;
    let mut when = when.split_whitespace();
    let seconds: i64 = when.next()?.parse().ok()?;
    let offset = offset(when.next().unwrap_or("+0000"))?;
    Some(Identity {
        name: name.trim_end().to_string(),
        email: email.to_string(),
        at: Utc.timestamp_opt(seconds, 0).single()?,
        offset,
    })
}

/// `+hhmm` or `-hhmm` as minutes east of UTC.
fn offset(tz: &str) -> Option<i64> {
    let (sign, digits) = match tz.split_at_checked(1)? {
        ("+", d) => (1, d),
        ("-", d) => (-1, d),
        _ => return None,
    };
    if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let hours: i64 = digits[..2].parse().ok()?;
    let minutes: i64 = digits[2..].parse().ok()?;
    Some(sign * (hours * 60 + minutes))
}

/// How a commit changed one path against its first parent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    /// `added`, `modified`, `deleted` or `type_changed`.
    pub change: &'static str,
    pub added: i64,
    pub removed: i64,
    /// Git counted no lines in it.
    pub binary: bool,
    /// The blob it gives the path, when it added or modified it.
    pub blob: Option<String>,
}

/// The tokens of a `git log -z` listing whose format is `%x01%H`: each
/// commit's hash, then what its diff printed.
fn by_commit(out: &[u8]) -> Vec<(String, Vec<String>)> {
    let mut commits: Vec<(String, Vec<String>)> = Vec::new();
    let mut opening = false;
    for token in out.split(|b| *b == 0) {
        let mut token = String::from_utf8_lossy(token).into_owned();
        if opening {
            // The newline between a commit's line and its diff.
            if let Some(rest) = token.strip_prefix('\n') {
                token = rest.to_string();
            }
        }
        opening = false;
        if let Some(hash) = token.strip_prefix('\u{1}') {
            commits.push((hash.to_string(), Vec::new()));
            opening = true;
        } else if let Some((_, tokens)) = commits.last_mut() {
            if !token.is_empty() || !tokens.is_empty() {
                tokens.push(token);
            }
        }
    }
    commits
}

/// A numstat token: `<added>\t<removed>\t<path>`, `-` for a binary's.
fn numstat_token(token: &str) -> Option<(String, i64, i64, bool)> {
    let mut parts = token.splitn(3, '\t');
    let (added, removed, path) = (parts.next()?, parts.next()?, parts.next()?);
    if added == "-" && removed == "-" {
        return Some((path.to_string(), 0, 0, true));
    }
    Some((path.to_string(), added.parse().ok()?, removed.parse().ok()?, false))
}

/// Each commit's file changes, from `git log -z --raw --numstat` with the
/// format `%x01%H`.
#[must_use]
pub fn changes(out: &[u8]) -> BTreeMap<String, Vec<FileChange>> {
    let mut all = BTreeMap::new();
    for (hash, tokens) in by_commit(out) {
        let mut files: Vec<FileChange> = Vec::new();
        let mut counts: HashMap<String, (i64, i64, bool)> = HashMap::new();
        let mut i = 0;
        while i < tokens.len() {
            let token = &tokens[i];
            if let Some(raw) = token.strip_prefix(':') {
                let fields: Vec<&str> = raw.split(' ').collect();
                let path = tokens.get(i + 1).cloned().unwrap_or_default();
                let status = fields.get(4).and_then(|s| s.chars().next());
                let change = match status {
                    Some('A') => "added",
                    Some('D') => "deleted",
                    Some('T') => "type_changed",
                    _ => "modified",
                };
                let blob = fields.get(3).filter(|_| matches!(change, "added" | "modified")).map(|s| (*s).to_string());
                files.push(FileChange { path, change, added: 0, removed: 0, binary: false, blob });
                i += 2;
            } else {
                if let Some((path, added, removed, binary)) = numstat_token(token) {
                    counts.insert(path, (added, removed, binary));
                }
                i += 1;
            }
        }
        for f in &mut files {
            if let Some((added, removed, binary)) = counts.get(&f.path) {
                (f.added, f.removed, f.binary) = (*added, *removed, *binary);
            }
        }
        all.insert(hash, files);
    }
    all
}

/// Each commit's counts by path, from `git log -z --numstat` with the
/// format `%x01%H`.
#[must_use]
pub fn numstat(out: &[u8]) -> BTreeMap<String, HashMap<String, (i64, i64)>> {
    by_commit(out)
        .into_iter()
        .map(|(hash, tokens)| {
            let counts = tokens.iter().filter_map(|t| numstat_token(t)).map(|(path, a, r, _)| (path, (a, r))).collect();
            (hash, counts)
        })
        .collect()
}

/// `<patch id> <commit>` lines, as `git patch-id --stable` prints them.
#[must_use]
pub fn patch_ids(out: &str) -> HashMap<String, String> {
    out.lines().filter_map(|l| l.split_once(' ')).map(|(patch, commit)| (commit.trim().to_string(), patch.to_string())).collect()
}

/// `<hash>\t<seconds>` lines: the commits a listing reached and when each
/// was committed.
#[must_use]
pub fn listed(out: &str) -> Vec<(String, i64)> {
    out.lines().filter_map(|l| l.split_once('\t')).filter_map(|(h, t)| Some((h.to_string(), t.trim().parse().ok()?))).collect()
}

/// The commits `git fsck --unreachable` names, in its order.
#[must_use]
pub fn unreachable(out: &str) -> Vec<String> {
    out.lines().filter_map(|l| l.strip_prefix("unreachable commit ")).map(|h| h.trim().to_string()).collect()
}

/// A commit hash: 40 or 64 hexadecimal digits.
#[must_use]
pub fn is_hash(h: &str) -> bool {
    matches!(h.len(), 40 | 64) && h.bytes().all(|b| b.is_ascii_hexdigit())
}
