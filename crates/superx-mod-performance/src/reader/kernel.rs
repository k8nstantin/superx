//! Every read of the kernel, in one place, all of it SELECT (§10): its
//! captured lines a page at a time by their ids, one session's lines again
//! from a point, and the names of its agents and sessions.
//!
//! A line's id is a UUIDv7 the kernel set when it captured the line, so
//! ids run in the order lines were captured. A page is a seek on a table's
//! own keys. No datetime index is read: SurrealDB 3.1's misorders values
//! within a second and drops rows at range bounds (#486), and paging by
//! one lost 93k of 108k lines where the id walk lost none (#501).
//!
//! Lines come from two tables. Conversation lines are `message` rows.
//! Claude Code's other lines are `transcript_event` rows in
//! `telemetry_stream`, the line whole under `payload.line` (#471), beside
//! a firehose of other events. A telemetry page reads a bounded number of
//! rows, says which are wanted, and fetches only those whole.

use std::collections::HashMap;

use chrono::{DateTime, TimeZone as _, Utc};
use superx_kernel::types::{RecordId, RecordIdKey, Uuid, Value};
use superx_kernel::{Kernel, Result};

use crate::reader::formats;
use crate::reader::value::{at, text, time};

/// Where a page of lines starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cursor {
    /// At this id, or the first after it.
    At(Uuid),
    /// After this id.
    After(Uuid),
}

/// One captured line, as the reader reads it.
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
    /// The line, whole.
    pub raw: Value,
}

const MESSAGES_AT: &str =
    "SELECT id, session, agent, raw, emitted_at, valid_from FROM type::record('message', $from..) LIMIT $limit";
const MESSAGES_AFTER: &str =
    "SELECT id, session, agent, raw, emitted_at, valid_from FROM type::record('message', $from>..) LIMIT $limit";
const SESSION_MESSAGES_AT: &str = "SELECT id, session, agent, raw, emitted_at, valid_from \
     FROM type::record('message', $from..) WHERE session = $sess LIMIT $limit";
const SESSION_MESSAGES_AFTER: &str = "SELECT id, session, agent, raw, emitted_at, valid_from \
     FROM type::record('message', $from>..) WHERE session = $sess LIMIT $limit";
const EVENTS_AT: &str = "SELECT id, lifecycle_event = 'transcript_event' AND \
     ((payload.line.type = 'system' AND payload.line.subtype INSIDE $subtypes) OR \
     (payload.line.type = 'attachment' AND payload.line.attachment.type INSIDE $attachments)) AS wanted \
     FROM type::record('telemetry_stream', $from..) LIMIT $limit";
const EVENTS_AFTER: &str = "SELECT id, lifecycle_event = 'transcript_event' AND \
     ((payload.line.type = 'system' AND payload.line.subtype INSIDE $subtypes) OR \
     (payload.line.type = 'attachment' AND payload.line.attachment.type INSIDE $attachments)) AS wanted \
     FROM type::record('telemetry_stream', $from>..) LIMIT $limit";
const EVENT_LINES: &str = "SELECT id, agent, valid_from, payload.session AS session, payload.line AS line FROM $ids";

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
/// last id scanned, where the next page starts.
#[derive(Debug, Default)]
pub struct Page {
    pub lines: Vec<Line>,
    pub scanned: usize,
    pub last: Option<Uuid>,
}

async fn message_page(kernel: &Kernel, names: &mut Names, rows: &[Value]) -> Result<Page> {
    let mut page = Page { scanned: rows.len(), ..Page::default() };
    for r in rows {
        let Some(id) = at(r, &["id"]).and_then(record_uuid) else { continue };
        page.last = Some(id);
        let agent = match at(r, &["agent"]) {
            Some(Value::RecordId(a)) => names.agent(kernel, a).await?,
            _ => None,
        };
        let session = match at(r, &["session"]) {
            Some(Value::RecordId(s)) => Some(s.clone()),
            _ => None,
        };
        let line_time = match at(r, &["emitted_at"]).or_else(|| at(r, &["valid_from"])) {
            Some(Value::Datetime(d)) => DateTime::<Utc>::from(*d),
            _ => uuid_time(id).unwrap_or_default(),
        };
        page.lines.push(Line { id, agent, session, at: line_time, raw: at(r, &["raw"]).cloned().unwrap_or_default() });
    }
    Ok(page)
}

/// A page of the lines kept as telemetry that the reader reads, from
/// `cursor` on: `limit` rows of the stream scanned, the wanted ones read
/// whole.
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
        .bind(("subtypes", formats::system_subtypes()))
        .bind(("attachments", formats::attachment_types()))
        .await?
        .take(0)?;
    let mut page = Page { scanned: rows.len(), ..Page::default() };
    let mut wanted = Vec::new();
    for r in &rows {
        let Some(Value::RecordId(id)) = at(r, &["id"]) else { continue };
        let Some(u) = record_uuid_of(id) else { continue };
        page.last = Some(u);
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
        let session = match (&agent, text(r, &["session"])) {
            (Some(agent), Some(key)) => names.session(kernel, agent, key).await?,
            _ => None,
        };
        let raw = at(r, &["line"]).cloned().unwrap_or_default();
        let line_time = time(&raw, &["timestamp"])
            .or_else(|| match at(r, &["valid_from"]) {
                Some(Value::Datetime(d)) => Some(DateTime::<Utc>::from(*d)),
                _ => None,
            })
            .unwrap_or_else(|| uuid_time(id).unwrap_or_default());
        page.lines.push(Line { id, agent, session, at: line_time, raw });
    }
    page.lines.sort_by_key(|l| l.id);
    Ok(page)
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
