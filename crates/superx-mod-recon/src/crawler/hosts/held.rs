//! What the crawler holds of a host repository from one host crawl to the
//! next: what the database holds of it, read at its first host crawl since
//! superx-recon started, and kept as rows are added. Every query the hosts
//! source issues is here. Of several rows for one thing, the newest is
//! picked by `valid_from` or `started_at` in code, never by an index's
//! order (#486).

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use superx_kernel::types::{RecordId, Value};
use superx_kernel::{Db, Result};

use crate::crawler::hosts::ci::{CiRun, CiState};
use crate::crawler::hosts::objects::{key_text, Kept};
use crate::crawler::hosts::pulls::{Pull, PullCommit, PullState};
use crate::crawler::hosts::Host;
use crate::crawler::value::{at, flag, int, text};

/// The newest answer to one path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asked {
    pub etag: Option<String>,
    /// Its host crawl ended complete: its etag may be sent back.
    pub complete: bool,
    /// It was answered HTTP 200 and read, or HTTP 304.
    pub read: bool,
    /// The path of the page it named next.
    pub next: Option<String>,
}

impl Asked {
    /// Read in a host crawl that ended complete: a path asked on a
    /// condition is not asked again for itself (steps 15, 17, 18).
    #[must_use]
    pub fn settled(&self) -> bool {
        self.complete && self.read
    }
}

/// What the crawler holds of one host repository.
#[derive(Debug, Default)]
pub struct HostMemory {
    pub kept: Kept,
    /// The newest repository object kept.
    pub repository: Option<serde_json::Value>,
    /// Every path ever asked, with its newest answer.
    pub answers: HashMap<String, Asked>,
    /// The newest kept object's updated time of each pull request and
    /// ruleset, by its key.
    pub updated: HashMap<(String, String), String>,
    /// Pull requests by number.
    pub pulls: HashMap<i64, Pull>,
    /// The newest closed event of each pull request's timeline as kept, by
    /// number: when, and its actor.
    pub closed: HashMap<i64, (String, serde_json::Value)>,
    /// CI runs by kind and host id.
    pub ci: HashMap<(String, String), CiRun>,
}

/// The paths of a list's pages, in a crawl's calls: `(path, next)` in the
/// order the calls were made.
fn successors(calls: &[(String, bool)]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for pair in calls.windows(2) {
        if pair[0].1 {
            out.insert(pair[0].0.clone(), pair[1].0.clone());
        }
    }
    out
}

const LOAD: &str = "
    SELECT id, outcome FROM host_crawl WHERE host_repository = $hr;
    LET $crawls = (SELECT VALUE id FROM host_crawl WHERE host_repository = $hr);
    SELECT path, etag, next, started_at, host_crawl, status, error FROM host_call WHERE host_crawl INSIDE $crawls;
    SELECT id, kind, key, hash FROM host_object WHERE host_repository = $hr;
    SELECT kind, key, object.updated_at AS updated_at, object.updated_on AS updated_on, valid_from FROM host_object
        WHERE host_repository = $hr AND kind INSIDE ['pull_request', 'ruleset'];
    SELECT key, object.event AS event, object.created_at AS created_at, object.actor AS actor, call.path AS path
        FROM host_object WHERE host_repository = $hr AND kind = 'timeline_event';
    SELECT id, number, url FROM pull_request WHERE host_repository = $hr;
    LET $pulls = (SELECT VALUE id FROM pull_request WHERE host_repository = $hr);
    SELECT * FROM pull_request_state WHERE pull_request INSIDE $pulls;
    SELECT pull_request, hash, position, present, valid_from FROM pull_request_commit WHERE pull_request INSIDE $pulls;
    SELECT id, kind, host_id, name, commit_sha, ref, created_at FROM ci_run WHERE host_repository = $hr;
    LET $runs = (SELECT VALUE id FROM ci_run WHERE host_repository = $hr);
    SELECT * FROM ci_run_state WHERE ci_run INSIDE $runs;
    SELECT object, valid_from FROM host_object WHERE host_repository = $hr AND kind = 'repository';
";

/// What the database holds of a host repository.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the database cannot answer.
pub async fn load(db: &Db, host_repository: &RecordId) -> Result<HostMemory> {
    let mut res = db.query(LOAD).bind(("hr", host_repository.clone())).await?;
    let crawls: Vec<Value> = res.take(0)?;
    let calls: Vec<Value> = res.take(2)?;
    let objects: Vec<Value> = res.take(3)?;
    let updated: Vec<Value> = res.take(4)?;
    let timeline: Vec<Value> = res.take(5)?;
    let pulls: Vec<Value> = res.take(6)?;
    let states: Vec<Value> = res.take(8)?;
    let commits: Vec<Value> = res.take(9)?;
    let runs: Vec<Value> = res.take(10)?;
    let run_states: Vec<Value> = res.take(12)?;
    let repositories: Vec<Value> = res.take(13)?;

    let mut memory = HostMemory::default();
    let complete: HashMap<String, bool> = crawls
        .iter()
        .filter_map(|c| Some((row_key(&record(c, "id")?), text(c, &["outcome"])? == "complete")))
        .collect();
    // Each path's newest answer, and the page it named next: the call made
    // after it in its own host crawl, since a list is paged before anything
    // else of its crawl is asked.
    // Each crawl's calls: when, the path, whether a page followed, the
    // etag, whether it was read.
    type Calls = Vec<(DateTime<Utc>, String, bool, Option<String>, bool)>;
    let mut by_crawl: HashMap<String, Calls> = HashMap::new();
    for c in &calls {
        let (Some(crawl), Some(path), Some(started)) = (record(c, "host_crawl"), text(c, &["path"]), time(c, "started_at")) else {
            continue;
        };
        let next = flag(c, &["next"]).unwrap_or(false);
        // Read: an answer HTTP 200 with no error, or HTTP 304.
        let read = match int(c, &["status"]) {
            Some(200) => text(c, &["error"]).is_none(),
            Some(304) => true,
            _ => false,
        };
        by_crawl.entry(row_key(&crawl)).or_default().push((
            started,
            path.to_string(),
            next,
            text(c, &["etag"]).map(str::to_string),
            read,
        ));
    }
    let mut newest: HashMap<String, (DateTime<Utc>, Asked)> = HashMap::new();
    for (crawl, mut calls) in by_crawl {
        calls.sort_by_key(|c| c.0);
        let order: Vec<(String, bool)> = calls.iter().map(|c| (c.1.clone(), c.2)).collect();
        let next = successors(&order);
        let ended_complete = complete.get(&crawl).copied().unwrap_or(false);
        for (started, path, _, etag, read) in calls {
            let asked = Asked { etag, complete: ended_complete, read, next: next.get(&path).cloned() };
            match newest.get(&path) {
                Some((seen, _)) if *seen >= started => {}
                _ => {
                    newest.insert(path, (started, asked));
                }
            }
        }
    }
    memory.answers = newest.into_iter().map(|(path, (_, asked))| (path, asked)).collect();

    memory.kept = Kept::from_rows(objects.iter().filter_map(|o| {
        Some((text(o, &["kind"])?.to_string(), key_text(at(o, &["key"])?), text(o, &["hash"])?.to_string(), record(o, "id")?))
    }));

    let mut newest_updated: HashMap<(String, String), (DateTime<Utc>, String)> = HashMap::new();
    for u in &updated {
        let (Some(kind), Some(key), Some(valid_from)) = (text(u, &["kind"]), at(u, &["key"]), time(u, "valid_from")) else {
            continue;
        };
        let Some(stamp) = text(u, &["updated_at"]).or_else(|| text(u, &["updated_on"])) else { continue };
        let k = (kind.to_string(), key_text(key));
        if newest_updated.get(&k).is_none_or(|(seen, _)| *seen < valid_from) {
            newest_updated.insert(k, (valid_from, stamp.to_string()));
        }
    }
    memory.updated = newest_updated.into_iter().map(|(k, (_, v))| (k, v)).collect();

    for t in &timeline {
        if text(t, &["event"]) != Some("closed") {
            continue;
        }
        let (Some(path), Some(created)) = (text(t, &["path"]), text(t, &["created_at"])) else { continue };
        let Some(number) = timeline_number(path) else { continue };
        let actor = at(t, &["actor"]).map(json_of).unwrap_or(serde_json::Value::Null);
        memory.close(number, created.to_string(), actor);
    }

    let mut by_row: HashMap<String, i64> = HashMap::new();
    for p in &pulls {
        let (Some(row), Some(number)) = (record(p, "id"), int(p, &["number"])) else { continue };
        by_row.insert(row_key(&row), number);
        memory.pulls.insert(
            number,
            Pull { row, url: text(p, &["url"]).map(str::to_string), state: None, commits: HashMap::new() },
        );
    }
    let mut newest_state: HashMap<i64, DateTime<Utc>> = HashMap::new();
    for s in &states {
        let (Some(pull), Some(valid_from)) = (record(s, "pull_request"), time(s, "valid_from")) else { continue };
        let Some(number) = by_row.get(&row_key(&pull)).copied() else { continue };
        if newest_state.get(&number).is_some_and(|seen| *seen >= valid_from) {
            continue;
        }
        newest_state.insert(number, valid_from);
        if let Some(p) = memory.pulls.get_mut(&number) {
            p.state = Some(PullState::from_row(s));
        }
    }
    let mut newest_commit: HashMap<(i64, String), DateTime<Utc>> = HashMap::new();
    for c in &commits {
        let (Some(pull), Some(hash), Some(valid_from)) = (record(c, "pull_request"), text(c, &["hash"]), time(c, "valid_from")) else {
            continue;
        };
        let Some(number) = by_row.get(&row_key(&pull)).copied() else { continue };
        let k = (number, hash.to_string());
        if newest_commit.get(&k).is_some_and(|seen| *seen >= valid_from) {
            continue;
        }
        newest_commit.insert(k, valid_from);
        if let Some(p) = memory.pulls.get_mut(&number) {
            p.commits.insert(
                hash.to_string(),
                PullCommit { present: flag(c, &["present"]).unwrap_or(false), position: int(c, &["position"]) },
            );
        }
    }

    let mut run_by_row: HashMap<String, (String, String)> = HashMap::new();
    for r in &runs {
        let (Some(row), Some(kind), Some(host_id)) = (record(r, "id"), text(r, &["kind"]), at(r, &["host_id"])) else { continue };
        let k = (kind.to_string(), key_text(host_id));
        run_by_row.insert(row_key(&row), k.clone());
        memory.ci.insert(
            k,
            CiRun {
                row,
                name: text(r, &["name"]).map(str::to_string),
                commit_sha: text(r, &["commit_sha"]).map(str::to_string),
                git_ref: text(r, &["ref"]).map(str::to_string),
                created_at: time(r, "created_at"),
                state: None,
            },
        );
    }
    let mut newest_run_state: HashMap<String, DateTime<Utc>> = HashMap::new();
    for s in &run_states {
        let (Some(run), Some(valid_from)) = (record(s, "ci_run"), time(s, "valid_from")) else { continue };
        let row = row_key(&run);
        if newest_run_state.get(&row).is_some_and(|seen| *seen >= valid_from) {
            continue;
        }
        newest_run_state.insert(row.clone(), valid_from);
        if let Some(k) = run_by_row.get(&row) {
            if let Some(r) = memory.ci.get_mut(k) {
                r.state = Some(CiState::from_row(s));
            }
        }
    }
    memory.repository = repositories
        .iter()
        .filter_map(|r| Some((time(r, "valid_from")?, at(r, &["object"])?)))
        .max_by_key(|(valid_from, _)| *valid_from)
        .map(|(_, object)| json_of(object));
    Ok(memory)
}

impl HostMemory {
    /// A closed event of a pull request's timeline: kept when it is its
    /// newest.
    pub fn close(&mut self, number: i64, created_at: String, actor: serde_json::Value) {
        match self.closed.get(&number) {
            Some((seen, _)) if *seen >= created_at => {}
            _ => {
                self.closed.insert(number, (created_at, actor));
            }
        }
    }
}

/// The number a timeline's path names: `…/issues/<number>/timeline…`.
#[must_use]
pub fn timeline_number(path: &str) -> Option<i64> {
    let rest = path.split("/issues/").nth(1)?;
    let (number, after) = rest.split_once('/')?;
    after.starts_with("timeline").then(|| number.parse().ok()).flatten()
}

/// Every host repository, every account and every mirror the database
/// holds: what a pass looks up before its workers start.
#[derive(Debug, Default)]
pub struct Known {
    pub repositories: HashMap<(Host, String), RecordId>,
    /// Each mirror's row, by its host repository's row ([`row_key`]).
    pub mirrors: HashMap<String, RecordId>,
    /// The default branch the newest repository object kept of each host
    /// repository names, by the host repository's row ([`row_key`]).
    pub default_branches: HashMap<String, String>,
}

const KNOWN: &str = "
    SELECT id, host, path FROM host_repository;
    SELECT id, host_repository FROM mirror;
    SELECT id, host, host_id FROM host_user;
    SELECT host_repository, object.default_branch AS github, object.mainbranch.name AS bitbucket, valid_from
        FROM host_object WHERE kind = 'repository';
";

/// The host repositories, mirrors and accounts the database holds.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the database cannot answer.
pub async fn known(db: &Db) -> Result<(Known, Vec<(Host, String, RecordId)>)> {
    let mut res = db.query(KNOWN).await?;
    let repositories: Vec<Value> = res.take(0)?;
    let mirrors: Vec<Value> = res.take(1)?;
    let users: Vec<Value> = res.take(2)?;
    let defaults: Vec<Value> = res.take(3)?;
    let mut known = Known::default();
    let mut newest: HashMap<String, (DateTime<Utc>, Option<String>)> = HashMap::new();
    for d in &defaults {
        let (Some(repository), Some(valid_from)) = (record(d, "host_repository"), time(d, "valid_from")) else { continue };
        let branch = text(d, &["github"]).or_else(|| text(d, &["bitbucket"])).filter(|b| !b.is_empty()).map(str::to_string);
        let key = row_key(&repository);
        if newest.get(&key).is_none_or(|(seen, _)| *seen < valid_from) {
            newest.insert(key, (valid_from, branch));
        }
    }
    known.default_branches = newest.into_iter().filter_map(|(k, (_, b))| Some((k, b?))).collect();
    for r in &repositories {
        let (Some(id), Some(host), Some(path)) = (record(r, "id"), text(r, &["host"]).and_then(Host::named), text(r, &["path"])) else {
            continue;
        };
        known.repositories.insert((host, path.to_string()), id);
    }
    known.mirrors = mirrors.iter().filter_map(|m| Some((row_key(&record(m, "host_repository")?), record(m, "id")?))).collect();
    let accounts = users
        .iter()
        .filter_map(|u| Some((text(u, &["host"]).and_then(Host::named)?, key_text(at(u, &["host_id"])?), record(u, "id")?)))
        .collect();
    Ok((known, accounts))
}

/// A row's id as a map's key: a map keyed by a record id is one clippy
/// refuses (`mutable_key_type`).
#[must_use]
pub fn row_key(id: &RecordId) -> String {
    format!("{id:?}")
}

/// A link in a row.
#[must_use]
pub fn record(v: &Value, key: &str) -> Option<RecordId> {
    match at(v, &[key])? {
        Value::RecordId(id) => Some(id.clone()),
        _ => None,
    }
}

/// A time in a row.
#[must_use]
pub fn time(v: &Value, key: &str) -> Option<DateTime<Utc>> {
    match at(v, &[key])? {
        Value::Datetime(d) => Some(DateTime::<Utc>::from(*d)),
        _ => None,
    }
}

/// A value read back from the database, as JSON.
#[must_use]
pub fn json_of(v: &Value) -> serde_json::Value {
    match v {
        Value::None | Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Number(n) => n.to_int().map_or_else(
            || n.to_f64().and_then(serde_json::Number::from_f64).map_or(serde_json::Value::Null, serde_json::Value::Number),
            |i| serde_json::Value::Number(i.into()),
        ),
        Value::String(s) => serde_json::Value::String(s.clone()),
        Value::Array(items) => serde_json::Value::Array(items.iter().map(json_of).collect()),
        Value::Object(o) => serde_json::Value::Object(o.iter().map(|(k, v)| (k.clone(), json_of(v))).collect()),
        other => serde_json::Value::String(format!("{other:?}")),
    }
}
