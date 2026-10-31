//! Every read of the kernel, in one place, all of it SELECT (§10): its
//! captured lines a page at a time by their ids, how many lines it holds
//! over a page's ids, one session's lines again from a point, and the names
//! of its agents and sessions.
//!
//! A line's id is a UUIDv7 the kernel set when it captured the line, so
//! ids run in the order lines were captured. A page is a seek on a table's
//! own keys. No datetime index is read: SurrealDB 3.1's misorders values
//! within a second and drops rows at range bounds (#486), and paging by
//! one lost 93k of 108k lines where the id walk lost none (#501).
//!
//! Lines come from two tables. Conversation lines are `message` rows: the
//! line as its agent wrote it (`raw_text`, #529) beside the kernel's parsed
//! copy (`raw`). Every other line is a row of `telemetry_stream`, beside a
//! firehose of other events: a `transcript_event` (a Claude Code line that
//! is not conversation, a file beside its transcripts, a Gemini header,
//! header change, notice or earlier write of a record), a `transcript_raw`
//! (a line of a shape its adapter does not know, or that is not JSON) or a
//! `desktop_event` (a Claude Desktop log record). A telemetry page reads a
//! bounded number of rows, says which are lines, and fetches only those
//! whole.
//!
//! The kernel kept a line in one of several shapes over time: the parsed
//! copy alone before #529, both from #529, `raw_text` alone while #542 was
//! its build, both again from #546, and before #471 a telemetry line by its
//! kind alone. [`Body`] is the line as the crawler reads it, whichever.

use std::collections::HashMap;

use chrono::{DateTime, TimeZone as _, Utc};
use superx_kernel::message::line_value_from_json;
use superx_kernel::types::{RecordId, RecordIdKey, Uuid, Value};
use superx_kernel::{Kernel, Result};

use crate::crawler::value::{at, text, time};

/// The telemetry rows that are lines, by their `lifecycle_event`.
pub const LINE_EVENTS: [&str; 3] = ["transcript_event", "transcript_raw", "desktop_event"];

/// Where a page of lines starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cursor {
    /// At this id, or the first after it.
    At(Uuid),
    /// After this id.
    After(Uuid),
}

/// A line, as the crawler reads it.
#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    /// A line the agent wrote as JSON: its `raw_text` parsed, wherever the
    /// kernel kept one; else the parsed copy it kept, a message row's `raw`
    /// or a payload's `line`. A file beside Claude Code's transcripts is the
    /// `line` the kernel made of it.
    Json(Value),
    /// A record written as text and read as text: a Claude Desktop log
    /// record.
    Text(String),
    /// A line that is not JSON.
    NotJson,
    /// Nothing of the line but its kind: a telemetry line kept before #471.
    Missing,
}

/// One captured line, as the crawler reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// The kernel row's UUIDv7: when the line was captured.
    pub id: Uuid,
    /// The agent's name, as the kernel registered it.
    pub agent: Option<String>,
    /// The kernel's session entity.
    pub session: Option<RecordId>,
    /// When the line was written: the agent's own time when it has one.
    pub at: DateTime<Utc>,
    pub body: Body,
    /// The kind the kernel gave a line it kept as telemetry, when it gave
    /// one (the payload's `kind`).
    pub kernel_kind: Option<String>,
}

impl Line {
    /// The line, when it is JSON.
    #[must_use]
    pub fn json(&self) -> Option<&Value> {
        match &self.body {
            Body::Json(v) => Some(v),
            _ => None,
        }
    }
}

const MESSAGES_AT: &str = "SELECT id, session, agent, raw, raw_text, emitted_at, valid_from \
     FROM type::record('message', $from..) LIMIT $limit";
const MESSAGES_AFTER: &str = "SELECT id, session, agent, raw, raw_text, emitted_at, valid_from \
     FROM type::record('message', $from>..) LIMIT $limit";
const SESSION_MESSAGES_AT: &str = "SELECT id, session, agent, raw, raw_text, emitted_at, valid_from \
     FROM type::record('message', $from..) WHERE session = $sess LIMIT $limit";
const SESSION_MESSAGES_AFTER: &str = "SELECT id, session, agent, raw, raw_text, emitted_at, valid_from \
     FROM type::record('message', $from>..) WHERE session = $sess LIMIT $limit";
const EVENTS_AT: &str = "SELECT id, lifecycle_event INSIDE $lines AS wanted \
     FROM type::record('telemetry_stream', $from..) LIMIT $limit";
const EVENTS_AFTER: &str = "SELECT id, lifecycle_event INSIDE $lines AS wanted \
     FROM type::record('telemetry_stream', $from>..) LIMIT $limit";
const EVENT_LINES: &str = "SELECT id, agent, lifecycle_event, valid_from, payload FROM $ids";
const MESSAGES_HELD: &str = "SELECT count() AS n FROM type::record('message', $first..=$last) GROUP ALL";
const EVENTS_HELD: &str = "SELECT count() AS n FROM type::record('telemetry_stream', $first..=$last) \
     WHERE lifecycle_event INSIDE $lines GROUP ALL";

/// A page of conversation lines, from `cursor` on.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the kernel's database cannot answer.
pub async fn messages(kernel: &Kernel, names: &mut Names, cursor: Cursor, limit: u32) -> Result<Page> {
    let (sql, from) = match cursor {
        Cursor::At(u) => (MESSAGES_AT, u),
        Cursor::After(u) => (MESSAGES_AFTER, u),
    };
    let rows: Vec<Value> = kernel.db().query(sql).bind(("from", from)).bind(("limit", limit)).await?.take(0)?;
    message_page(kernel, names, &rows).await
}

/// A page of one session's conversation lines, from `cursor` on: the lines
/// of a reply read again.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the kernel's database cannot answer.
pub async fn session_messages(
    kernel: &Kernel,
    names: &mut Names,
    session: &RecordId,
    cursor: Cursor,
    limit: u32,
) -> Result<Page> {
    let (sql, from) = match cursor {
        Cursor::At(u) => (SESSION_MESSAGES_AT, u),
        Cursor::After(u) => (SESSION_MESSAGES_AFTER, u),
    };
    let rows: Vec<Value> = kernel
        .db()
        .query(sql)
        .bind(("from", from))
        .bind(("sess", session.clone()))
        .bind(("limit", limit))
        .await?
        .take(0)?;
    message_page(kernel, names, &rows).await
}

/// One page: the lines read, how many rows were scanned for them, and the
/// first and last ids scanned: where the page began, and where the next one
/// starts.
#[derive(Debug, Default)]
pub struct Page {
    pub lines: Vec<Line>,
    pub scanned: usize,
    pub first: Option<Uuid>,
    pub last: Option<Uuid>,
}

impl Page {
    fn scanned_id(&mut self, id: Uuid) {
        self.first.get_or_insert(id);
        self.last = Some(id);
    }
}

async fn message_page(kernel: &Kernel, names: &mut Names, rows: &[Value]) -> Result<Page> {
    let mut page = Page { scanned: rows.len(), ..Page::default() };
    for r in rows {
        let Some(id) = at(r, &["id"]).and_then(record_uuid) else { continue };
        page.scanned_id(id);
        let agent = match at(r, &["agent"]) {
            Some(Value::RecordId(a)) => names.agent(kernel, a).await?,
            _ => None,
        };
        let session = match at(r, &["session"]) {
            Some(Value::RecordId(s)) => Some(s.clone()),
            _ => None,
        };
        let body = match at(r, &["raw_text"]) {
            Some(Value::String(t)) => parse(t),
            _ => match at(r, &["raw"]) {
                Some(raw @ Value::Object(_)) => Body::Json(raw.clone()),
                _ => Body::Missing,
            },
        };
        let line_time = written_at(&body)
            .or_else(|| datetime(at(r, &["emitted_at"])))
            .or_else(|| datetime(at(r, &["valid_from"])))
            .unwrap_or_else(|| uuid_time(id).unwrap_or_default());
        page.lines.push(Line { id, agent, session, at: line_time, body, kernel_kind: None });
    }
    Ok(page)
}

/// A page of the telemetry rows that are lines, from `cursor` on: `limit`
/// rows of the stream scanned, the lines among them read whole, payload and
/// all; every other row is no line and is skipped.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the kernel's database cannot answer.
pub async fn events(kernel: &Kernel, names: &mut Names, cursor: Cursor, limit: u32) -> Result<Page> {
    let (sql, from) = match cursor {
        Cursor::At(u) => (EVENTS_AT, u),
        Cursor::After(u) => (EVENTS_AFTER, u),
    };
    let rows: Vec<Value> = kernel
        .db()
        .query(sql)
        .bind(("from", from))
        .bind(("limit", limit))
        .bind(("lines", LINE_EVENTS.to_vec()))
        .await?
        .take(0)?;
    let mut page = Page { scanned: rows.len(), ..Page::default() };
    let mut wanted = Vec::new();
    for r in &rows {
        let Some(Value::RecordId(id)) = at(r, &["id"]) else { continue };
        let Some(u) = record_uuid_of(id) else { continue };
        page.scanned_id(u);
        if matches!(at(r, &["wanted"]), Some(Value::Bool(true))) {
            wanted.push(Value::RecordId(id.clone()));
        }
    }
    if wanted.is_empty() {
        return Ok(page);
    }
    let found: Vec<Value> = kernel.db().query(EVENT_LINES).bind(("ids", wanted)).await?.take(0)?;
    for r in &found {
        let Some(id) = at(r, &["id"]).and_then(record_uuid) else { continue };
        let agent = match at(r, &["agent"]) {
            Some(Value::RecordId(a)) => names.agent(kernel, a).await?,
            _ => None,
        };
        let payload = at(r, &["payload"]);
        let session = match (&agent, payload.and_then(|p| text(p, &["session"]))) {
            (Some(agent), Some(key)) => names.session(kernel, agent, key).await?,
            _ => None,
        };
        let body = event_body(text(r, &["lifecycle_event"]), payload);
        let line_time = written_at(&body)
            .or_else(|| datetime(at(r, &["valid_from"])))
            .unwrap_or_else(|| uuid_time(id).unwrap_or_default());
        let kernel_kind = payload.and_then(|p| text(p, &["kind"])).map(str::to_string);
        page.lines.push(Line { id, agent, session, at: line_time, body, kernel_kind });
    }
    page.lines.sort_by_key(|l| l.id);
    Ok(page)
}

/// How many message rows the kernel holds over the ids `first` to `last`.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the kernel's database cannot answer.
pub async fn messages_held(kernel: &Kernel, first: Uuid, last: Uuid) -> Result<i64> {
    let rows: Vec<Value> =
        kernel.db().query(MESSAGES_HELD).bind(("first", first)).bind(("last", last)).await?.take(0)?;
    Ok(held(&rows))
}

/// How many telemetry rows that are lines the kernel holds over the ids
/// `first` to `last`.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the kernel's database cannot answer.
pub async fn events_held(kernel: &Kernel, first: Uuid, last: Uuid) -> Result<i64> {
    let rows: Vec<Value> = kernel
        .db()
        .query(EVENTS_HELD)
        .bind(("first", first))
        .bind(("last", last))
        .bind(("lines", LINE_EVENTS.to_vec()))
        .await?
        .take(0)?;
    Ok(held(&rows))
}

/// A `count()` grouped over everything: one row, or none over no rows.
fn held(rows: &[Value]) -> i64 {
    rows.first().and_then(|r| crate::crawler::value::int(r, &["n"])).unwrap_or(0)
}

/// A line kept as text, parsed as JSON exactly as the kernel parses it.
fn parse(raw_text: &str) -> Body {
    match serde_json::from_str::<serde_json::Value>(raw_text) {
        Ok(json) => Body::Json(line_value_from_json(&json)),
        Err(_) => Body::NotJson,
    }
}

/// A telemetry line: its `raw_text` wherever the kernel kept one, else the
/// `line` of its payload; a line of a shape its adapter did not know and
/// kept without either is not JSON, and any other is kept by its kind alone.
fn event_body(lifecycle: Option<&str>, payload: Option<&Value>) -> Body {
    let Some(p) = payload else { return Body::Missing };
    if let Some(Value::String(t)) = at(p, &["raw_text"]) {
        return parse(t);
    }
    match at(p, &["line"]) {
        Some(line @ Value::Object(_)) => Body::Json(line.clone()),
        Some(Value::String(record)) => Body::Text(record.clone()),
        _ if lifecycle == Some("transcript_raw") => Body::NotJson,
        _ => Body::Missing,
    }
}

/// When a line says it was written.
fn written_at(body: &Body) -> Option<DateTime<Utc>> {
    match body {
        Body::Json(v) => time(v, &["timestamp"]),
        _ => None,
    }
}

fn datetime(v: Option<&Value>) -> Option<DateTime<Utc>> {
    match v {
        Some(Value::Datetime(d)) => Some(DateTime::<Utc>::from(*d)),
        _ => None,
    }
}

/// The names of the kernel's agents and sessions, read when one is not yet
/// known: an agent by its entity, a session by its entity or by the key
/// the agent's own files give it.
/// Agents and sessions are kept by their entity's UUID: a map keyed by a
/// record id is one clippy refuses (`mutable_key_type`).
#[derive(Debug, Default)]
pub struct Names {
    agents: HashMap<Uuid, String>,
    sessions: HashMap<Uuid, Session>,
    keys: HashMap<(String, String), RecordId>,
    loaded: bool,
}

/// A session as the kernel names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// The agent whose session it is.
    pub agent: String,
    /// The session's id in the agent's own files.
    pub key: String,
}

impl Names {
    /// The agent named by its entity.
    ///
    /// # Errors
    ///
    /// [`superx_kernel::KernelError`] when the kernel cannot answer.
    pub async fn agent(&mut self, kernel: &Kernel, id: &RecordId) -> Result<Option<String>> {
        let Some(key) = record_uuid_of(id) else { return Ok(None) };
        if !self.agents.contains_key(&key) {
            self.load(kernel).await?;
        }
        Ok(self.agents.get(&key).cloned())
    }

    /// The session entity an agent's session key names.
    ///
    /// # Errors
    ///
    /// [`superx_kernel::KernelError`] when the kernel cannot answer.
    pub async fn session(&mut self, kernel: &Kernel, agent: &str, key: &str) -> Result<Option<RecordId>> {
        let k = (agent.to_string(), key.to_string());
        if !self.keys.contains_key(&k) {
            self.load(kernel).await?;
        }
        Ok(self.keys.get(&k).cloned())
    }

    /// What the kernel names a session entity.
    ///
    /// # Errors
    ///
    /// [`superx_kernel::KernelError`] when the kernel cannot answer.
    pub async fn session_of(&mut self, kernel: &Kernel, id: &RecordId) -> Result<Option<Session>> {
        let Some(key) = record_uuid_of(id) else { return Ok(None) };
        if !self.sessions.contains_key(&key) {
            self.load(kernel).await?;
        }
        Ok(self.sessions.get(&key).cloned())
    }

    /// Every agent and session the kernel has named. Read again only when
    /// a name is missing, at most once a page, so a line whose name the
    /// kernel has not written yet does not read them all on every line.
    async fn load(&mut self, kernel: &Kernel) -> Result<()> {
        if self.loaded {
            return Ok(());
        }
        for a in kernel.list_named_entities("node_agent", "attr_agent_descriptor").await? {
            if let (Some(name), Some(key)) = (text(&a.payload, &["name"]), record_uuid_of(&a.entity_id)) {
                self.agents.insert(key, name.to_string());
            }
        }
        for s in kernel.list_named_entities("node_session", "attr_session_descriptor").await? {
            // `<agent>/<session key>` (capture's `ensure_session`).
            let Some((agent, key)) = text(&s.payload, &["name"]).and_then(|n| n.split_once('/')) else { continue };
            self.keys.insert((agent.to_string(), key.to_string()), s.entity_id.clone());
            if let Some(entity) = record_uuid_of(&s.entity_id) {
                self.sessions.insert(entity, Session { agent: agent.to_string(), key: key.to_string() });
            }
        }
        self.loaded = true;
        Ok(())
    }

    /// Forget whether the names were read, so a name missing since is
    /// looked for again: once per page.
    pub fn may_reload(&mut self) {
        self.loaded = false;
    }
}

/// A row id's UUID, when its key is one.
#[must_use]
pub fn record_uuid_of(id: &RecordId) -> Option<Uuid> {
    match &id.key {
        RecordIdKey::Uuid(u) => Some(*u),
        _ => None,
    }
}

fn record_uuid(v: &Value) -> Option<Uuid> {
    match v {
        Value::RecordId(id) => record_uuid_of(id),
        _ => None,
    }
}

/// When a UUIDv7 says its line was captured.
#[must_use]
pub fn uuid_time(id: Uuid) -> Option<DateTime<Utc>> {
    let (secs, nanos) = id.get_timestamp()?.to_unix();
    Utc.timestamp_opt(i64::try_from(secs).ok()?, nanos).single()
}

/// The smallest id a line captured at `t` can have: a UUIDv7 holding that
/// millisecond and nothing else.
#[must_use]
pub fn first_id_at(t: DateTime<Utc>) -> Uuid {
    let ms = u64::try_from(t.timestamp_millis()).unwrap_or(0);
    let mut bytes = [0u8; 16];
    bytes[..6].copy_from_slice(&ms.to_be_bytes()[2..]);
    bytes[6] = 0x70;
    bytes[8] = 0x80;
    Uuid::from(uuid::Uuid::from_bytes(bytes))
}

/// The first id there is: where the first pass starts.
#[must_use]
pub fn first_id() -> Uuid {
    Uuid::from(uuid::Uuid::nil())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_id_says_when_it_was_captured_and_the_first_id_of_a_moment_is_before_every_other() {
        let t = Utc.with_ymd_and_hms(2026, 10, 25, 7, 13, 40).single().expect("time");
        let first = first_id_at(t);
        assert_eq!(uuid_time(first), Some(t));
        let captured = Uuid::from(uuid::Uuid::new_v7(uuid::Timestamp::from_unix(
            uuid::NoContext,
            u64::try_from(t.timestamp()).expect("after 1970"),
            1_000_000,
        )));
        assert!(first < captured, "the first id of the moment comes first");
        assert!(first_id() < first);
    }
}
