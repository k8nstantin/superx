//! The objects a host's answers hold (#538, step 16), kept whole in
//! `host_object`: one row per object of a kind and key, and one more each
//! time its content changes, its hash the SHA-256 of the object as
//! serde_json writes a value, compact, every object's keys sorted. Every
//! account an object holds, at any depth, is an object of the kind `user`
//! too, and a `host_user` row the first time its host names it.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};
use serde_json::Value as Json;
use sha2::{Digest as _, Sha256};
use superx_kernel::types::{RecordId, Value};

use crate::crawler::hosts::Host;
use crate::crawler::store::{Batch, Row};

/// The shape a call's answer has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// One object.
    Object,
    /// A GitHub array of objects.
    Array,
    /// A GitHub list wrapped beside its `total_count`, under this key.
    Wrapped(&'static str),
    /// A Bitbucket page: its `values`, its `size` where it gives one, its
    /// `next`.
    Page,
}

/// What one answer held.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Held {
    pub objects: Vec<Json>,
    /// The list's total, where the answer states one.
    pub total: Option<i64>,
    /// The next page Bitbucket's answer names.
    pub next: Option<String>,
}

/// The objects an answer holds; `None` when it is not JSON or not of the
/// shape asked.
#[must_use]
pub fn held(body: &[u8], shape: Shape) -> Option<Held> {
    let json: Json = serde_json::from_slice(body).ok()?;
    let objects = |items: &Json| -> Option<Vec<Json>> {
        let items = items.as_array()?;
        items.iter().all(Json::is_object).then(|| items.clone())
    };
    match shape {
        Shape::Object => json.is_object().then(|| Held { objects: vec![json], ..Held::default() }),
        Shape::Array => Some(Held { objects: objects(&json)?, ..Held::default() }),
        Shape::Wrapped(key) => {
            let total = json.get("total_count")?.as_i64()?;
            Some(Held { objects: objects(json.get(key)?)?, total: Some(total), next: None })
        }
        Shape::Page => {
            let page = json.as_object()?;
            let values = objects(page.get("values")?)?;
            let next = match page.get("next") {
                None | Some(Json::Null) => None,
                Some(Json::String(s)) => Some(s.clone()),
                Some(_) => return None,
            };
            Some(Held { objects: values, total: page.get("size").and_then(Json::as_i64), next })
        }
    }
}

/// An object written out as serde_json writes a value, compact, every
/// object's keys in their sorted order.
#[must_use]
pub fn canonical(v: &Json) -> String {
    match v {
        Json::Object(o) => {
            let sorted: BTreeMap<&String, &Json> = o.iter().collect();
            let fields: Vec<String> = sorted
                .iter()
                .map(|(k, v)| format!("{}:{}", serde_json::to_string(k).unwrap_or_default(), canonical(v)))
                .collect();
            format!("{{{}}}", fields.join(","))
        }
        Json::Array(items) => format!("[{}]", items.iter().map(canonical).collect::<Vec<_>>().join(",")),
        scalar => serde_json::to_string(scalar).unwrap_or_default(),
    }
}

/// The SHA-256 of an object's canonical form, in lowercase hexadecimal.
#[must_use]
pub fn hash(v: &Json) -> String {
    Sha256::digest(canonical(v).as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// An object's key as the structure keeps it: a whole number or a text.
#[must_use]
pub fn key_value(key: &Json) -> Option<Value> {
    match key {
        Json::Number(n) => n.as_i64().map(|i| Value::Number(superx_kernel::types::Number::Int(i))),
        Json::String(s) if !s.is_empty() => Some(Value::String(s.clone())),
        _ => None,
    }
}

/// A key as text, for the maps the crawler keeps.
#[must_use]
pub fn key_text(key: &Value) -> String {
    match key {
        Value::Number(n) => n.to_int().map_or_else(|| format!("{n:?}"), |i| i.to_string()),
        Value::String(s) => s.clone(),
        other => format!("{other:?}"),
    }
}

/// An account an object holds: GitHub's has `login`, `id` and `type`;
/// Bitbucket's a `uuid` and the type `user`, `team` or `app_user`.
#[must_use]
pub fn is_account(host: Host, v: &Json) -> bool {
    let Some(o) = v.as_object() else { return false };
    match host {
        Host::GitHub => o.contains_key("login") && o.get("id").is_some_and(Json::is_i64) && o.contains_key("type"),
        Host::Bitbucket => {
            o.get("uuid").is_some_and(Json::is_string)
                && matches!(o.get("type").and_then(Json::as_str), Some("user" | "team" | "app_user"))
        }
    }
}

/// Every account `v` holds, at any depth, itself included.
#[must_use]
pub fn accounts(host: Host, v: &Json) -> Vec<Json> {
    let mut out = Vec::new();
    walk(host, v, &mut out);
    out
}

fn walk(host: Host, v: &Json, out: &mut Vec<Json>) {
    if is_account(host, v) {
        out.push(v.clone());
    }
    match v {
        Json::Object(o) => o.values().for_each(|child| walk(host, child, out)),
        Json::Array(items) => items.iter().for_each(|child| walk(host, child, out)),
        _ => {}
    }
}

/// An account's id on its host: GitHub's number, Bitbucket's uuid.
#[must_use]
pub fn account_id(host: Host, v: &Json) -> Option<Value> {
    match host {
        Host::GitHub => v.get("id").and_then(key_value),
        Host::Bitbucket => v.get("uuid").and_then(key_value),
    }
}

/// The objects a host repository holds: each kind and key's content
/// hashes, with the row of each.
#[derive(Debug, Default)]
pub struct Kept {
    rows: HashMap<(String, String), HashMap<String, RecordId>>,
}

/// Where an object was read: its host repository, the call whose answer
/// held it, and when.
#[derive(Debug, Clone, Copy)]
pub struct From<'a> {
    pub host_repository: &'a RecordId,
    pub call: &'a RecordId,
    pub at: DateTime<Utc>,
}

/// How an object read was taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Taken {
    /// The row that holds its content: written now, or held before.
    pub row: RecordId,
    /// A row was written for it now.
    pub written: bool,
    /// Its kind and key were held before, with another content.
    pub changed: bool,
}

impl Kept {
    /// What the database holds of a host repository: `(kind, key, hash,
    /// row)`.
    pub fn from_rows(rows: impl IntoIterator<Item = (String, String, String, RecordId)>) -> Self {
        let mut kept = Self::default();
        for (kind, key, hash, row) in rows {
            kept.rows.entry((kind, key)).or_default().insert(hash, row);
        }
        kept
    }

    /// Whether any object of this kind and key is held.
    #[must_use]
    pub fn holds(&self, kind: &str, key: &str) -> bool {
        self.rows.contains_key(&(kind.to_string(), key.to_string()))
    }

    /// The keys of every object of `kind` held.
    #[must_use]
    pub fn keys(&self, kind: &str) -> Vec<String> {
        let mut keys: Vec<String> = self.rows.keys().filter(|(k, _)| k == kind).map(|(_, key)| key.clone()).collect();
        keys.sort();
        keys
    }

    /// Take one object read from `from`: a row when the host repository
    /// holds none of its kind, key and hash.
    pub fn take(&mut self, rows: &mut Batch, from: &From<'_>, kind: &str, key: &Value, object: &Json) -> Taken {
        let From { host_repository, call, at } = *from;
        let hash = hash(object);
        let held = self.rows.entry((kind.to_string(), key_text(key))).or_default();
        if let Some(row) = held.get(&hash) {
            return Taken { row: row.clone(), written: false, changed: false };
        }
        let changed = !held.is_empty();
        let row = rows.create(
            "host_object",
            Row::new()
                .link("host_repository", host_repository)
                .text("kind", kind)
                .value("key", key.clone())
                .object("object", superx_kernel::message::line_to_object(object))
                .text("hash", hash.clone())
                .link("call", call)
                .time("at", at),
        );
        held.insert(hash, row.clone());
        Taken { row, written: true, changed }
    }
}

/// The accounts the hosts have named, ever, shared by every worker of a
/// pass: one `host_user` row each, made by the worker that sees it first
/// and written before any worker's own rows, which may link it.
#[derive(Debug, Default)]
pub struct Accounts {
    known: HashMap<(Host, String), RecordId>,
    /// Rows made in this pass and not yet written.
    pub waiting: Batch,
    loaded: bool,
}

impl Accounts {
    /// Whether the accounts the database holds were read.
    #[must_use]
    pub fn loaded(&self) -> bool {
        self.loaded
    }

    /// The accounts the database holds: `(host, host_id, row)`.
    pub fn load(&mut self, rows: impl IntoIterator<Item = (Host, String, RecordId)>) {
        for (host, id, row) in rows {
            self.known.insert((host, id), row);
        }
        self.loaded = true;
    }

    /// The `host_user` row of an account: made the first time its host
    /// names it.
    pub fn row(&mut self, host: Host, account: &Json) -> Option<RecordId> {
        let id = account_id(host, account)?;
        let key = (host, key_text(&id));
        if let Some(row) = self.known.get(&key) {
            return Some(row.clone());
        }
        let text = |field: &str| account.get(field).and_then(Json::as_str).filter(|s| !s.is_empty()).map(str::to_string);
        let (login, name) = match host {
            Host::GitHub => (text("login"), text("name")),
            Host::Bitbucket => (text("nickname"), text("display_name")),
        };
        let row = self.waiting.create(
            "host_user",
            Row::new()
                .text("host", host.name())
                .value("host_id", id)
                .some_text("login", login)
                .some_text("name", name)
                .some_text("kind", text("type"))
                .time("found_at", Utc::now()),
        );
        self.known.insert(key, row.clone());
        Some(row)
    }

    /// Forget the rows of a pass that never wrote them.
    pub fn forget_waiting(&mut self) {
        self.waiting = Batch::default();
        self.known.clear();
        self.loaded = false;
    }
}
