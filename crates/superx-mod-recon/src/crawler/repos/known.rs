//! What the module's database holds of one repository before its crawl
//! writes (#538, steps 3 to 13): its commits, the newest of each remote,
//! placement and survival, its landings, its reflog entries, its ties to
//! sessions and its file changes; and, for a pass, the git events the
//! lines wrote. Of several rows for one thing, the newest is picked by
//! `valid_from` in code, never by an index's order (#486). The engine holds
//! no unique index where a field is NONE, so what is not written twice is
//! looked up here.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{DateTime, Utc};
use superx_kernel::types::{RecordId, Value};
use superx_kernel::{Db, Result};

use crate::crawler::hosts::held::{record, row_key, time};
use crate::crawler::repos::remotes::Remote;
use crate::crawler::value::{flag, int, text};

/// A commit the database holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldCommit {
    pub row: RecordId,
    pub committed_at: DateTime<Utc>,
    pub added: i64,
}

/// What the database holds of one repository.
#[derive(Debug, Default)]
pub struct Known {
    /// Its commits, by hash.
    pub commits: HashMap<String, HeldCommit>,
    /// Each remote's newest row, by name.
    pub remotes: BTreeMap<String, Remote>,
    /// Each commit's newest placement, by its row ([`row_key`]).
    pub placements: HashMap<String, (String, bool)>,
    /// Every landing: the commit's row, the path, the via's row.
    pub landed: HashSet<(String, String, String)>,
    /// Each commit's newest alive, by its row.
    pub survival: HashMap<String, i64>,
    /// Every reflog entry held, as [`move_key`] writes it.
    pub moves: HashSet<String>,
    /// Every tie: the commit's row, the session's row, how.
    pub sessions: HashSet<(String, String, String)>,
    /// Every file change: the commit's row, its path, whether it is binary.
    pub files: Vec<(String, String, bool)>,
}

const KNOWN: &str = "
    SELECT id, hash, committed_at, added FROM commit WHERE repository = $r;
    SELECT name, url, push_url, host, path, valid_from FROM remote_state WHERE repository = $r;
    SELECT commit, place, first_parent, valid_from FROM placement WHERE commit.repository = $r;
    SELECT commit, path, via FROM landed_via WHERE commit.repository = $r;
    SELECT commit, alive, valid_from FROM survival WHERE commit.repository = $r;
    SELECT checkout, mirror, ref, at, old, new, action FROM ref_move WHERE repository = $r;
    SELECT commit, session, via FROM commit_session WHERE commit.repository = $r;
    SELECT commit, path, binary FROM commit_file WHERE commit.repository = $r;
";

/// The key a reflog entry is held by: its checkout or its mirror, its ref,
/// time, commits and message.
#[must_use]
pub fn move_key(place: &str, name: &str, at: DateTime<Utc>, old: Option<&str>, new: Option<&str>, action: &str) -> String {
    format!("{place}\u{0}{name}\u{0}{}\u{0}{}\u{0}{}\u{0}{action}", at.timestamp(), old.unwrap_or(""), new.unwrap_or(""))
}

/// Where a reflog entry is: its mirror's row, else its checkout's, else
/// the repository's own.
#[must_use]
pub fn move_place(checkout: Option<&RecordId>, mirror: Option<&RecordId>) -> String {
    match (mirror, checkout) {
        (Some(m), _) => format!("mirror {}", row_key(m)),
        (None, Some(c)) => format!("checkout {}", row_key(c)),
        (None, None) => "repository".to_string(),
    }
}

/// What the database holds of `repository`.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the database cannot answer.
pub async fn load(db: &Db, repository: &RecordId) -> Result<Known> {
    let mut res = db.query(KNOWN).bind(("r", repository.clone())).await?;
    let commits: Vec<Value> = res.take(0)?;
    let remotes: Vec<Value> = res.take(1)?;
    let placements: Vec<Value> = res.take(2)?;
    let landed: Vec<Value> = res.take(3)?;
    let survival: Vec<Value> = res.take(4)?;
    let moves: Vec<Value> = res.take(5)?;
    let sessions: Vec<Value> = res.take(6)?;
    let files: Vec<Value> = res.take(7)?;
    let mut known = Known::default();
    for c in &commits {
        let (Some(row), Some(hash), Some(committed_at)) = (record(c, "id"), text(c, &["hash"]), time(c, "committed_at")) else {
            continue;
        };
        known.commits.insert(hash.to_string(), HeldCommit { row, committed_at, added: int(c, &["added"]).unwrap_or(0) });
    }
    let mut newest: HashMap<String, (DateTime<Utc>, Remote)> = HashMap::new();
    for r in &remotes {
        let (Some(name), Some(valid_from)) = (text(r, &["name"]), time(r, "valid_from")) else { continue };
        let remote = Remote {
            url: text(r, &["url"]).map(str::to_string),
            push_url: text(r, &["push_url"]).map(str::to_string),
            host: text(r, &["host"]).map(str::to_string),
            path: text(r, &["path"]).map(str::to_string),
        };
        if newest.get(name).is_none_or(|(seen, _)| *seen < valid_from) {
            newest.insert(name.to_string(), (valid_from, remote));
        }
    }
    known.remotes = newest.into_iter().map(|(name, (_, r))| (name, r)).collect();
    let mut placed: HashMap<String, (DateTime<Utc>, (String, bool))> = HashMap::new();
    for p in &placements {
        let (Some(commit), Some(place), Some(valid_from)) = (record(p, "commit"), text(p, &["place"]), time(p, "valid_from")) else {
            continue;
        };
        let key = row_key(&commit);
        if placed.get(&key).is_none_or(|(seen, _)| *seen < valid_from) {
            placed.insert(key, (valid_from, (place.to_string(), flag(p, &["first_parent"]).unwrap_or(false))));
        }
    }
    known.placements = placed.into_iter().map(|(k, (_, v))| (k, v)).collect();
    known.landed = landed
        .iter()
        .filter_map(|l| Some((row_key(&record(l, "commit")?), text(l, &["path"])?.to_string(), row_key(&record(l, "via")?))))
        .collect();
    let mut alive: HashMap<String, (DateTime<Utc>, i64)> = HashMap::new();
    for s in &survival {
        let (Some(commit), Some(n), Some(valid_from)) = (record(s, "commit"), int(s, &["alive"]), time(s, "valid_from")) else {
            continue;
        };
        let key = row_key(&commit);
        if alive.get(&key).is_none_or(|(seen, _)| *seen < valid_from) {
            alive.insert(key, (valid_from, n));
        }
    }
    known.survival = alive.into_iter().map(|(k, (_, v))| (k, v)).collect();
    known.moves = moves
        .iter()
        .filter_map(|m| {
            let place = move_place(record(m, "checkout").as_ref(), record(m, "mirror").as_ref());
            Some(move_key(&place, text(m, &["ref"])?, time(m, "at")?, text(m, &["old"]), text(m, &["new"]), text(m, &["action"])?))
        })
        .collect();
    known.sessions = sessions
        .iter()
        .filter_map(|s| Some((row_key(&record(s, "commit")?), row_key(&record(s, "session")?), text(s, &["via"])?.to_string())))
        .collect();
    known.files = files
        .iter()
        .filter_map(|f| Some((row_key(&record(f, "commit")?), text(f, &["path"])?.to_string(), flag(f, &["binary"]).unwrap_or(false))))
        .collect();
    Ok(known)
}

/// The reflog entries the database holds of `mirrors`, whichever crawl
/// wrote them, as [`move_key`] writes them.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the database cannot answer.
pub async fn mirror_moves(db: &Db, mirrors: &[RecordId]) -> Result<HashSet<String>> {
    if mirrors.is_empty() {
        return Ok(HashSet::new());
    }
    let rows: Vec<Value> =
        db.query("SELECT mirror, ref, at, old, new, action FROM ref_move WHERE mirror INSIDE $m").bind(("m", mirrors.to_vec())).await?.take(0)?;
    Ok(rows
        .iter()
        .filter_map(|m| {
            let place = move_place(None, record(m, "mirror").as_ref());
            Some(move_key(&place, text(m, &["ref"])?, time(m, "at")?, text(m, &["old"]), text(m, &["new"]), text(m, &["action"])?))
        })
        .collect())
}

/// A git event the lines wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitEvent {
    pub action: String,
    pub dir: Option<String>,
    pub hash: Option<String>,
    pub pr_number: Option<i64>,
    pub url: Option<String>,
    pub at: DateTime<Utc>,
    pub tool_call: RecordId,
    /// The session of its tool call's reply.
    pub session: Option<RecordId>,
    pub valid_from: DateTime<Utc>,
}

/// Every git event of the actions a crawl reads (steps 1 and 13).
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the database cannot answer.
pub async fn git_events(db: &Db) -> Result<Vec<GitEvent>> {
    let rows: Vec<Value> = db
        .query(
            "SELECT action, dir, hash, pr_number, url, at, tool_call, tool_call.reply.session AS session, valid_from FROM git_event \
             WHERE action INSIDE ['commit', 'push', 'merge', 'rebase', 'revert', 'cherry_pick', 'reset', 'branch_deleted', 'pr_created', 'pr_merged']",
        )
        .await?
        .take(0)?;
    Ok(rows
        .iter()
        .filter_map(|e| {
            Some(GitEvent {
                action: text(e, &["action"])?.to_string(),
                dir: text(e, &["dir"]).map(str::to_string),
                hash: text(e, &["hash"]).map(str::to_string),
                pr_number: int(e, &["pr_number"]),
                url: text(e, &["url"]).map(str::to_string),
                at: time(e, "at")?,
                tool_call: record(e, "tool_call")?,
                session: record(e, "session"),
                valid_from: time(e, "valid_from")?,
            })
        })
        .collect())
}

/// A pull request of a host repository a crawl did not take, as the
/// database holds it: its row, number and url, every commit it has listed
/// and its newest merge commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldPull {
    pub row: RecordId,
    pub number: i64,
    pub url: Option<String>,
    pub listed: Vec<String>,
    pub merge_sha: Option<String>,
}

const PULLS: &str = "
    SELECT id, number, url FROM pull_request WHERE host_repository INSIDE $hrs;
    SELECT pull_request, hash FROM pull_request_commit WHERE pull_request.host_repository INSIDE $hrs;
    SELECT pull_request, merge_sha, valid_from FROM pull_request_state WHERE pull_request.host_repository INSIDE $hrs;
";

/// The pull requests the database holds of `host_repositories`.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the database cannot answer.
pub async fn pulls(db: &Db, host_repositories: &[RecordId]) -> Result<Vec<HeldPull>> {
    if host_repositories.is_empty() {
        return Ok(Vec::new());
    }
    let mut res = db.query(PULLS).bind(("hrs", host_repositories.to_vec())).await?;
    let rows: Vec<Value> = res.take(0)?;
    let commits: Vec<Value> = res.take(1)?;
    let states: Vec<Value> = res.take(2)?;
    let mut pulls: BTreeMap<String, HeldPull> = BTreeMap::new();
    for p in &rows {
        let (Some(row), Some(number)) = (record(p, "id"), int(p, &["number"])) else { continue };
        pulls.insert(row_key(&row), HeldPull { row, number, url: text(p, &["url"]).map(str::to_string), listed: Vec::new(), merge_sha: None });
    }
    for c in &commits {
        if let (Some(pull), Some(hash)) = (record(c, "pull_request"), text(c, &["hash"])) {
            if let Some(p) = pulls.get_mut(&row_key(&pull)) {
                if !p.listed.iter().any(|h| h == hash) {
                    p.listed.push(hash.to_string());
                }
            }
        }
    }
    let mut newest: HashMap<String, (DateTime<Utc>, Option<String>)> = HashMap::new();
    for s in &states {
        let (Some(pull), Some(valid_from)) = (record(s, "pull_request"), time(s, "valid_from")) else { continue };
        let key = row_key(&pull);
        if newest.get(&key).is_none_or(|(seen, _)| *seen < valid_from) {
            newest.insert(key, (valid_from, text(s, &["merge_sha"]).map(str::to_string)));
        }
    }
    for (key, (_, merge)) in newest {
        if let Some(p) = pulls.get_mut(&key) {
            p.merge_sha = merge;
        }
    }
    Ok(pulls.into_values().collect())
}
