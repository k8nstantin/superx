//! The objects a host's answers hold (#538, step 16), kept whole in
//! `host_object`: one row per object, and one more each time its content
//! changes, its content hashed as serde_json writes it, compact, every
//! object's keys in their sorted order. Every account an object holds, at
//! any depth, is an object of its own and an account of the host's.
//!
//! What a host repository holds is read from the database at its first
//! crawl since superx-recon started, and kept as rows are added ([`Known`]).

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use sha2::{Digest as _, Sha256};
use superx_kernel::message::line_to_object;
use superx_kernel::types::{RecordId, Uuid, Value};
use superx_kernel::{Db, Result};

use crate::crawler::hosts::{Host, Key};
use crate::crawler::lines::kernel::record_uuid_of;
use crate::crawler::store::{Row, Rows};
use crate::crawler::value::{at, text};

/// The SHA-256 of an object as serde_json writes a Value, compact, every
/// object's keys in their sorted order, in lowercase hexadecimal.
#[must_use]
pub fn hash(v: &serde_json::Value) -> String {
    // serde_json's maps keep their keys sorted (no preserve_order), and
    // to_vec writes compact.
    let bytes = serde_json::to_vec(v).unwrap_or_default();
    Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// An account, as a host writes one: on GitHub an object with `login`, `id`
/// and `type`; on Bitbucket an object with `uuid` and the `type` `user`,
/// `team` or `app_user`.
#[must_use]
pub fn is_account(host: Host, v: &serde_json::Value) -> bool {
    let Some(o) = v.as_object() else { return false };
    match host {
        Host::GitHub => o.contains_key("login") && o.contains_key("id") && o.contains_key("type"),
        Host::Bitbucket => {
            o.contains_key("uuid")
                && matches!(o.get("type").and_then(serde_json::Value::as_str), Some("user" | "team" | "app_user"))
        }
    }
}

/// An account's own id: GitHub's id, Bitbucket's uuid.
#[must_use]
pub fn account_key(host: Host, v: &serde_json::Value) -> Option<Key> {
    match host {
        Host::GitHub => v.get("id").and_then(Key::of),
        Host::Bitbucket => v.get("uuid").and_then(Key::of),
    }
}

/// Every account `v` holds, at any depth, itself included.
#[must_use]
pub fn accounts(host: Host, v: &serde_json::Value) -> Vec<&serde_json::Value> {
    let mut out = Vec::new();
    walk(host, v, &mut out);
    out
}

fn walk<'a>(host: Host, v: &'a serde_json::Value, out: &mut Vec<&'a serde_json::Value>) {
    if is_account(host, v) {
        out.push(v);
    }
    match v {
        serde_json::Value::Object(o) => o.values().for_each(|c| walk(host, c, out)),
        serde_json::Value::Array(items) => items.iter().for_each(|c| walk(host, c, out)),
        _ => {}
    }
}

/// An account's row, the first time a host names it.
#[must_use]
pub fn user_row(host: Host, key: &Key, account: &serde_json::Value, found_at: DateTime<Utc>) -> Row {
    let field = |name: &str| account.get(name).and_then(serde_json::Value::as_str).map(str::to_string);
    let (login, name) = match host {
        Host::GitHub => (field("login"), field("name")),
        Host::Bitbucket => (field("nickname"), field("display_name")),
    };
    Row::new()
        .text("host", host.name())
        .value("host_id", key.value())
        .some_text("login", login)
        .some_text("name", name)
        .some_text("kind", field("type"))
        .time("found_at", found_at)
}

/// What the newest row of one object says that a crawl reads again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Newest {
    pub hash: String,
    pub id: RecordId,
    pub at: DateTime<Utc>,
    /// Its `updated_at`, or Bitbucket's `updated_on`.
    pub updated: Option<String>,
    /// Its `state`, where it has one as text: a status's.
    pub state: Option<String>,
    /// Its `url`, where it has one as text: a GitHub status's names its
    /// commit.
    pub commit: Option<String>,
}

/// How an object was kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kept {
    /// A key the host repository held none of: a row.
    New(RecordId),
    /// A key it held, with other content: a row more.
    Changed(RecordId),
    /// Content it holds already: no row.
    Same(RecordId),
}

impl Kept {
    #[must_use]
    pub fn id(&self) -> &RecordId {
        match self {
            Self::New(id) | Self::Changed(id) | Self::Same(id) => id,
        }
    }

    /// New or changed: something the host changed since it was read.
    #[must_use]
    pub fn moved(&self) -> bool {
        !matches!(self, Self::Same(_))
    }
}

/// A pull request's state, as its newest row says it.
#[derive(Debug, Clone, PartialEq)]
pub struct PullState {
    pub fields: Vec<(&'static str, Value)>,
}

/// A CI run's state, as its newest row says it.
#[derive(Debug, Clone, PartialEq)]
pub struct RunState {
    pub fields: Vec<(&'static str, Value)>,
}

/// What a host repository holds, read at its first crawl since superx-recon
/// started and kept as rows are added.
#[derive(Debug, Clone)]
pub struct Known {
    /// The host repository.
    pub host_repository: RecordId,
    /// Every (kind, key, hash) held, with its row.
    pub objects: HashMap<(String, Key, String), RecordId>,
    /// The newest row of each (kind, key).
    pub newest: HashMap<(String, Key), Newest>,
    /// The newest repository object, whole.
    pub repository: Option<(DateTime<Utc>, serde_json::Value)>,
    /// Pull requests by number: each one's row and url.
    pub pulls: HashMap<i64, (RecordId, String)>,
    /// Each pull request's newest state, by its row's UUID.
    pub pull_states: HashMap<Uuid, PullState>,
    /// Each commit a pull request has listed, by the pull request's UUID
    /// and the hash: whether it is present now, and where.
    pub pull_commits: HashMap<Uuid, HashMap<String, (bool, Option<i64>)>>,
    /// CI runs by kind and host id.
    pub runs: HashMap<(String, Key), RecordId>,
    /// Each run's newest state, by its row's UUID.
    pub run_states: HashMap<Uuid, RunState>,
    /// Commits whose statuses an answered call has asked, and those it
    /// has not: looked for in the database once each.
    pub statuses_asked: HashMap<String, bool>,
}

impl Known {
    /// A host repository that holds nothing yet.
    #[must_use]
    pub fn empty(host_repository: &RecordId) -> Self {
        Self {
            host_repository: host_repository.clone(),
            objects: HashMap::new(),
            newest: HashMap::new(),
            repository: None,
            pulls: HashMap::new(),
            pull_states: HashMap::new(),
            pull_commits: HashMap::new(),
            runs: HashMap::new(),
            run_states: HashMap::new(),
            statuses_asked: HashMap::new(),
        }
    }

    /// What the database holds of `host_repository`.
    ///
    /// # Errors
    ///
    /// [`superx_kernel::KernelError`] when the database cannot answer.
    pub async fn load(db: &Db, host_repository: &RecordId) -> Result<Self> {
        let mut res = db
            .query(
                "SELECT id, kind, key, hash, at, valid_from, object.updated_at AS updated_at, object.updated_on AS updated_on, \
                 object.state AS state, object.url AS url FROM host_object WHERE host_repository = $hr;\
                 SELECT object, at, valid_from FROM host_object WHERE host_repository = $hr AND kind = 'repository';\
                 SELECT id, number, url FROM pull_request WHERE host_repository = $hr;\
                 SELECT * FROM pull_request_state WHERE pull_request.host_repository = $hr;\
                 SELECT pull_request, hash, position, present, valid_from FROM pull_request_commit WHERE pull_request.host_repository = $hr;\
                 SELECT id, kind, host_id FROM ci_run WHERE host_repository = $hr;\
                 SELECT * FROM ci_run_state WHERE ci_run.host_repository = $hr;",
            )
            .bind(("hr", host_repository.clone()))
            .await?;
        let objects: Vec<Value> = res.take(0)?;
        let repositories: Vec<Value> = res.take(1)?;
        let pulls: Vec<Value> = res.take(2)?;
        let pull_states: Vec<Value> = res.take(3)?;
        let pull_commits: Vec<Value> = res.take(4)?;
        let runs: Vec<Value> = res.take(5)?;
        let run_states: Vec<Value> = res.take(6)?;
        let mut known = Self::empty(host_repository);
        for o in &objects {
            let (Some(id), Some(kind), Some(key), Some(hash), Some(at_time)) =
                (record(o, "id"), text(o, &["kind"]), at(o, &["key"]).and_then(Key::of_value), text(o, &["hash"]), time(o, "at"))
            else {
                continue;
            };
            known.objects.insert((kind.to_string(), key.clone(), hash.to_string()), id.clone());
            let valid_from = time(o, "valid_from").unwrap_or(at_time);
            let candidate = Newest {
                hash: hash.to_string(),
                id,
                at: valid_from,
                updated: text(o, &["updated_at"]).or_else(|| text(o, &["updated_on"])).map(str::to_string),
                state: text(o, &["state"]).map(str::to_string),
                commit: text(o, &["url"]).map(str::to_string),
            };
            let slot = known.newest.entry((kind.to_string(), key));
            match slot {
                std::collections::hash_map::Entry::Occupied(mut e) if e.get().at < candidate.at => {
                    e.insert(candidate);
                }
                std::collections::hash_map::Entry::Occupied(_) => {}
                std::collections::hash_map::Entry::Vacant(e) => {
                    e.insert(candidate);
                }
            }
        }
        known.repository = repositories
            .iter()
            .filter_map(|r| Some((time(r, "valid_from")?, json_of(at(r, &["object"])?))))
            .max_by_key(|(t, _)| *t);
        for p in &pulls {
            if let (Some(id), Some(Value::Number(n)), Some(url)) = (record(p, "id"), at(p, &["number"]), text(p, &["url"])) {
                if let Some(n) = n.to_int() {
                    known.pulls.insert(n, (id, url.to_string()));
                }
            }
        }
        for (pull, state) in newest_by(&pull_states, "pull_request") {
            known.pull_states.insert(pull, PullState { fields: fields_of(state, crate::crawler::hosts::crawl::PULL_STATE_FIELDS) });
        }
        let mut commits: HashMap<(Uuid, String), Listed> = HashMap::new();
        for c in &pull_commits {
            let (Some(pull), Some(hash), Some(t)) = (record(c, "pull_request").as_ref().and_then(record_uuid_of), text(c, &["hash"]), time(c, "valid_from")) else {
                continue;
            };
            let present = matches!(at(c, &["present"]), Some(Value::Bool(true)));
            let position = match at(c, &["position"]) {
                Some(Value::Number(n)) => n.to_int(),
                _ => None,
            };
            let entry = commits.entry((pull, hash.to_string())).or_insert((t, present, position));
            if entry.0 < t {
                *entry = (t, present, position);
            }
        }
        for ((pull, hash), (_, present, position)) in commits {
            known.pull_commits.entry(pull).or_default().insert(hash, (present, position));
        }
        for r in &runs {
            if let (Some(id), Some(kind), Some(key)) = (record(r, "id"), text(r, &["kind"]), at(r, &["host_id"]).and_then(Key::of_value)) {
                known.runs.insert((kind.to_string(), key), id);
            }
        }
        for (run, state) in newest_by(&run_states, "ci_run") {
            known.run_states.insert(run, RunState { fields: fields_of(state, crate::crawler::hosts::crawl::RUN_STATE_FIELDS) });
        }
        Ok(known)
    }

    /// Keep one object of `kind` under `key`, held by `call`'s answer: a
    /// row when the host repository holds none of that kind, key and
    /// content.
    pub fn keep(&mut self, rows: &mut Rows, kind: &str, key: &Key, object: &serde_json::Value, call: &RecordId) -> Kept {
        let hash = hash(object);
        if let Some(id) = self.objects.get(&(kind.to_string(), key.clone(), hash.clone())) {
            return Kept::Same(id.clone());
        }
        let held_before = self.newest.contains_key(&(kind.to_string(), key.clone()));
        let id = rows.push(
            "host_object",
            Row::new()
                .link("host_repository", &self.host_repository)
                .text("kind", kind.to_string())
                .value("key", key.value())
                .object("object", line_to_object(object))
                .text("hash", hash.clone())
                .link("call", call)
                .time("at", Utc::now()),
        );
        self.objects.insert((kind.to_string(), key.clone(), hash.clone()), id.clone());
        let field = |name: &str| object.get(name).and_then(serde_json::Value::as_str).map(str::to_string);
        self.newest.insert(
            (kind.to_string(), key.clone()),
            Newest {
                hash,
                id: id.clone(),
                at: Utc::now(),
                updated: field("updated_at").or_else(|| field("updated_on")),
                state: field("state"),
                commit: field("url"),
            },
        );
        if kind == "repository" {
            self.repository = Some((Utc::now(), object.clone()));
        }
        if held_before {
            Kept::Changed(id)
        } else {
            Kept::New(id)
        }
    }
}

/// A commit a pull request listed, as its newest row says: when, whether it
/// is present, and where.
type Listed = (DateTime<Utc>, bool, Option<i64>);

/// Of rows linking `link`, the newest of each by `valid_from`.
fn newest_by<'a>(rows: &'a [Value], link: &str) -> HashMap<Uuid, &'a Value> {
    let mut best: HashMap<Uuid, (DateTime<Utc>, &Value)> = HashMap::new();
    for r in rows {
        let (Some(key), Some(t)) = (record(r, link).as_ref().and_then(record_uuid_of), time(r, "valid_from")) else { continue };
        match best.get(&key) {
            Some((seen, _)) if *seen >= t => {}
            _ => {
                best.insert(key, (t, r));
            }
        }
    }
    best.into_iter().map(|(k, (_, v))| (k, v)).collect()
}

/// The named fields of a row, as the database holds them; NONE where it
/// has none.
fn fields_of(row: &Value, names: &[&'static str]) -> Vec<(&'static str, Value)> {
    names.iter().map(|n| (*n, at(row, &[n]).cloned().unwrap_or(Value::None))).collect()
}

fn record(v: &Value, key: &str) -> Option<RecordId> {
    match at(v, &[key])? {
        Value::RecordId(id) => Some(id.clone()),
        _ => None,
    }
}

fn time(v: &Value, key: &str) -> Option<DateTime<Utc>> {
    match at(v, &[key])? {
        Value::Datetime(d) => Some(DateTime::<Utc>::from(*d)),
        _ => None,
    }
}

/// A stored object, back as JSON.
#[must_use]
pub fn json_of(v: &Value) -> serde_json::Value {
    match v {
        Value::None | Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Number(n) => match n.to_int() {
            Some(i) => serde_json::Value::from(i),
            None => n.to_f64().map_or(serde_json::Value::Null, serde_json::Value::from),
        },
        Value::String(s) => serde_json::Value::String(s.clone()),
        Value::Array(items) => serde_json::Value::Array(items.iter().map(json_of).collect()),
        Value::Object(o) => serde_json::Value::Object(o.iter().map(|(k, v)| (k.clone(), json_of(v))).collect()),
        other => serde_json::Value::String(format!("{other:?}")),
    }
}
