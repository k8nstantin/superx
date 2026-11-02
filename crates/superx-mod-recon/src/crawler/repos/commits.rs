//! A commit read whole (#538, step 7), from what git prints of it: the
//! commit object `git cat-file --batch` prints byte for byte, the changes
//! `git log --raw --numstat -z` prints of its change against its first
//! parent, and the patch id `git patch-id --stable` gives it.

use std::collections::HashMap;

use chrono::{DateTime, TimeZone as _, Utc};

/// A commit, as its object says it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub tree: String,
    /// First parent first.
    pub parents: Vec<String>,
    pub author: Ident,
    pub committer: Ident,
    /// The message's first line, as written.
    pub subject: String,
    /// What follows it, the empty lines that open it removed; none when
    /// nothing does.
    pub body: Option<String>,
    /// The `gpgsig` header whole, its continuation lines joined by
    /// newlines without the space that opens each; none when unsigned.
    pub signature: Option<String>,
}

/// Who, and when, as a commit's header writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ident {
    pub name: String,
    pub email: String,
    pub at: DateTime<Utc>,
    /// Minutes east of UTC (`-0400` is -240).
    pub offset: i64,
}

/// What `git cat-file --batch` printed: each object found, by its hash, as
/// its bytes; a hash it could not find is left out.
#[must_use]
pub fn objects(out: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut found = Vec::new();
    let mut rest = out;
    while !rest.is_empty() {
        let Some(end) = rest.iter().position(|b| *b == b'\n') else { break };
        let header = String::from_utf8_lossy(&rest[..end]).into_owned();
        rest = &rest[end + 1..];
        let mut words = header.split(' ');
        let (Some(hash), Some(kind)) = (words.next(), words.next()) else { continue };
        if kind == "missing" || kind == "ambiguous" {
            continue;
        }
        let Some(size) = words.next().and_then(|s| s.parse::<usize>().ok()) else { break };
        if rest.len() < size {
            break;
        }
        if kind == "commit" {
            found.push((hash.to_string(), rest[..size].to_vec()));
        }
        // The object, then the newline git ends it with.
        rest = rest.get(size + 1..).unwrap_or_default();
    }
    found
}

/// One commit object, read: none when it is not one git writes.
#[must_use]
pub fn parse(hash: &str, object: &[u8]) -> Option<Commit> {
    let text = String::from_utf8_lossy(object);
    let (header, message) = text.split_once("\n\n").unwrap_or((&text, ""));
    let mut tree = None;
    let mut parents = Vec::new();
    let mut author = None;
    let mut committer = None;
    let mut signature: Option<Vec<String>> = None;
    let mut in_signature = false;
    for line in header.lines() {
        if let Some(more) = line.strip_prefix(' ') {
            if in_signature {
                if let Some(lines) = signature.as_mut() {
                    lines.push(more.to_string());
                }
            }
            continue;
        }
        in_signature = false;
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        match key {
            "tree" => tree = Some(value.to_string()),
            "parent" => parents.push(value.to_string()),
            "author" => author = ident(value),
            "committer" => committer = ident(value),
            "gpgsig" => {
                signature = Some(vec![value.to_string()]);
                in_signature = true;
            }
            _ => {}
        }
    }
    let (subject, after) = message.split_once('\n').unwrap_or((message, ""));
    let body = after.trim_start_matches('\n');
    Some(Commit {
        hash: hash.to_string(),
        tree: tree?,
        parents,
        author: author?,
        committer: committer?,
        subject: subject.to_string(),
        body: (!body.is_empty()).then(|| body.to_string()),
        signature: signature.map(|lines| lines.join("\n")),
    })
}

/// `Name <email> seconds offset`.
fn ident(value: &str) -> Option<Ident> {
    let close = value.rfind('>')?;
    let open = value[..close].rfind('<')?;
    let name = value[..open].trim_end().to_string();
    let email = value[open + 1..close].to_string();
    let mut when = value[close + 1..].split_whitespace();
    let secs: i64 = when.next()?.parse().ok()?;
    let offset = offset_minutes(when.next()?)?;
    Some(Ident { name, email, at: Utc.timestamp_opt(secs, 0).single()?, offset })
}

/// `+0530` as 330, `-0400` as -240.
fn offset_minutes(tz: &str) -> Option<i64> {
    let (sign, digits) = match tz.as_bytes().first()? {
        b'+' => (1, &tz[1..]),
        b'-' => (-1, &tz[1..]),
        _ => return None,
    };
    if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let hours: i64 = digits[..2].parse().ok()?;
    let minutes: i64 = digits[2..].parse().ok()?;
    Some(sign * (hours * 60 + minutes))
}

/// How a commit changed a file, as git's raw output's status letter says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Added,
    Modified,
    Deleted,
    TypeChanged,
}

impl Change {
    fn of(letter: &str) -> Option<Self> {
        match letter {
            "A" => Some(Self::Added),
            "M" => Some(Self::Modified),
            "D" => Some(Self::Deleted),
            "T" => Some(Self::TypeChanged),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
            Self::TypeChanged => "type_changed",
        }
    }
}

/// One file of a commit's change against its first parent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    pub path: String,
    pub change: Change,
    /// The blob the commit gives the path; none when it deletes it.
    pub blob: Option<String>,
    /// Lines added and removed, as numstat counts them; none for a file
    /// git counts no lines in.
    pub lines: Option<(i64, i64)>,
}

/// What `git log --raw --numstat -z --format='commit %H'` printed: each
/// commit's files, by its hash. A commit that changes nothing has none.
#[must_use]
pub fn files(out: &[u8]) -> HashMap<String, Vec<File>> {
    let mut by_commit: HashMap<String, Vec<File>> = HashMap::new();
    let mut commit: Option<String> = None;
    let mut tokens = out.split(|b| *b == 0).map(|t| String::from_utf8_lossy(t).into_owned());
    let mut numstat: HashMap<String, Option<(i64, i64)>> = HashMap::new();
    let flush = |commit: &Option<String>, numstat: &mut HashMap<String, Option<(i64, i64)>>, by: &mut HashMap<String, Vec<File>>| {
        if let Some(c) = commit {
            if let Some(files) = by.get_mut(c) {
                for f in files.iter_mut() {
                    if let Some(lines) = numstat.get(&f.path) {
                        f.lines = *lines;
                    }
                }
            }
        }
        numstat.clear();
    };
    while let Some(token) = tokens.next() {
        let token = token.trim_start_matches('\n');
        if let Some(hash) = token.strip_prefix("commit ") {
            flush(&commit, &mut numstat, &mut by_commit);
            commit = Some(hash.trim().to_string());
            by_commit.entry(hash.trim().to_string()).or_default();
        } else if let Some(raw) = token.strip_prefix(':') {
            // `:<old mode> <new mode> <old blob> <new blob> <status>`, then
            // the path as the next token.
            let Some(path) = tokens.next() else { break };
            let fields: Vec<&str> = raw.split(' ').collect();
            let (Some(new_blob), Some(status)) = (fields.get(3), fields.get(4)) else { continue };
            let Some(change) = Change::of(status) else { continue };
            let blob = (change != Change::Deleted && !new_blob.bytes().all(|b| b == b'0')).then(|| (*new_blob).to_string());
            if let Some(c) = &commit {
                by_commit.entry(c.clone()).or_default().push(File { path, change, blob, lines: None });
            }
        } else if let Some((added, rest)) = token.split_once('\t') {
            // `<added>\t<removed>\t<path>`: `-` for a file git counts no lines in.
            let Some((removed, path)) = rest.split_once('\t') else { continue };
            let lines = match (added.parse::<i64>(), removed.parse::<i64>()) {
                (Ok(a), Ok(r)) => Some((a, r)),
                _ => None,
            };
            numstat.insert(path.to_string(), lines);
        }
    }
    flush(&commit, &mut numstat, &mut by_commit);
    by_commit
}

/// What `git log --numstat -z --format='commit %H'` printed: each commit's
/// counts by path, none for a file git counts no lines in.
#[must_use]
pub fn numstat(out: &[u8]) -> HashMap<String, HashMap<String, Option<(i64, i64)>>> {
    let mut by_commit: HashMap<String, HashMap<String, Option<(i64, i64)>>> = HashMap::new();
    let mut commit: Option<String> = None;
    for token in out.split(|b| *b == 0).map(|t| String::from_utf8_lossy(t).into_owned()) {
        let token = token.trim_start_matches('\n');
        if let Some(hash) = token.strip_prefix("commit ") {
            commit = Some(hash.trim().to_string());
            by_commit.entry(hash.trim().to_string()).or_default();
        } else if let Some((added, rest)) = token.split_once('\t') {
            let Some((removed, path)) = rest.split_once('\t') else { continue };
            let lines = match (added.parse::<i64>(), removed.parse::<i64>()) {
                (Ok(a), Ok(r)) => Some((a, r)),
                _ => None,
            };
            if let Some(c) = &commit {
                by_commit.entry(c.clone()).or_default().insert(path.to_string(), lines);
            }
        }
    }
    by_commit
}

/// What `git patch-id --stable` printed: each commit's patch id.
#[must_use]
pub fn patch_ids(out: &str) -> HashMap<String, String> {
    out.lines()
        .filter_map(|l| {
            let (patch, commit) = l.trim().split_once(' ')?;
            Some((commit.to_string(), patch.to_string()))
        })
        .collect()
}
