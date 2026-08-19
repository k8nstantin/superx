//! Message verbs — conversations, first-class (schema v2.1 §3.8).
//!
//! One `message` row per conversation event captured from an agent
//! session. `content` carries the extracted readable text; `raw`
//! preserves the full original source event (capture is very detailed
//! by design — operator directive 2026-08-07). Reading a conversation
//! is one indexed query per session; per-agent reads use the
//! `(agent, valid_from)` index.

use chrono::{DateTime, Utc};
use surrealdb::types::{Object, RecordId, SurrealValue, Value};

use crate::error::Result;
use crate::substrate::Kernel;

/// One row read back from `message`.
#[derive(Debug, Clone, SurrealValue)]
pub struct MessageRecord {
    pub id: RecordId,
    pub session: RecordId,
    pub agent: RecordId,
    /// Source-native role label (`user`, `assistant`, `tool`, …).
    pub role: String,
    /// Extracted readable text; empty when the event carries none.
    pub content: String,
    /// The full original source event.
    pub raw: Option<Object>,
    /// Source ordering (e.g. JSONL line number within the transcript).
    pub seq: Option<i64>,
    /// The source's own timestamp, when it carries one.
    pub emitted_at: Option<DateTime<Utc>>,
    /// Capture timestamp.
    pub valid_from: DateTime<Utc>,
}

/// Write-side fields for one message (everything the adapter knows).
#[derive(Debug, Clone)]
pub struct NewMessage {
    pub session: RecordId,
    pub agent: RecordId,
    pub role: String,
    pub content: String,
    pub raw: Option<Object>,
    pub seq: Option<i64>,
    pub emitted_at: Option<DateTime<Utc>>,
}

impl Kernel {
    /// Append one row to `message` with an explicit UUIDv7 id.
    ///
    /// # Errors
    ///
    /// Surfaces engine refusals verbatim ([`crate::KernelError::Db`]) —
    /// e.g. a `session` that is not a `node_session` entity or an
    /// `agent` that is not a `node_agent` entity (schema ASSERTs).
    pub async fn log_message(&self, msg: NewMessage) -> Result<RecordId> {
        let id = self.new_record_id("message");
        let row = MessageRow {
            session: msg.session,
            agent: msg.agent,
            role: msg.role,
            content: msg.content,
            raw: msg.raw,
            seq: msg.seq,
            emitted_at: msg.emitted_at,
            valid_from: Utc::now(),
        };
        let _: Option<MessageRow> = self.db().create(id.clone()).content(row).await?;
        Ok(id)
    }

    /// Read a session's messages in conversation order (oldest first),
    /// bounded by `limit`. Pure SELECT.
    ///
    /// # Errors
    ///
    /// [`crate::KernelError::Db`] for engine errors.
    pub async fn session_messages(
        &self,
        session: RecordId,
        limit: u32,
    ) -> Result<Vec<MessageRecord>> {
        let rows: Vec<MessageRecord> = self
            .db()
            .query(
                "SELECT * FROM message WHERE session = $sess \
                 ORDER BY valid_from ASC LIMIT $limit",
            )
            .bind(("sess", session))
            .bind(("limit", limit))
            .await?
            .take(0)?;
        Ok(rows)
    }

    /// Read a session's messages strictly newer than `after`, oldest
    /// first — the live-tail primitive behind `superx read --live`.
    ///
    /// # Errors
    ///
    /// [`crate::KernelError::Db`] for engine errors.
    pub async fn session_messages_since(
        &self,
        session: RecordId,
        after: DateTime<Utc>,
        limit: u32,
    ) -> Result<Vec<MessageRecord>> {
        let rows: Vec<MessageRecord> = self
            .db()
            .query(
                "SELECT * FROM message WHERE session = $sess \
                 AND valid_from > $after \
                 ORDER BY valid_from ASC LIMIT $limit",
            )
            .bind(("sess", session))
            .bind(("after", after))
            .bind(("limit", limit))
            .await?
            .take(0)?;
        Ok(rows)
    }

    /// Read an agent's messages across all sessions, newest first —
    /// "query the data for a specific agent" (operator directive,
    /// decision D9: the agent's entity id is the agent_id).
    ///
    /// # Errors
    ///
    /// [`crate::KernelError::Db`] for engine errors.
    pub async fn agent_messages(
        &self,
        agent: RecordId,
        limit: u32,
    ) -> Result<Vec<MessageRecord>> {
        let rows: Vec<MessageRecord> = self
            .db()
            .query(
                "SELECT * FROM message WHERE agent = $agent \
                 ORDER BY valid_from DESC LIMIT $limit",
            )
            .bind(("agent", agent))
            .bind(("limit", limit))
            .await?
            .take(0)?;
        Ok(rows)
    }

    /// Read messages strictly newer than `after` across ALL sessions,
    /// oldest first — the firehose-side live primitive behind the UI's
    /// SSE bridge (epic #141 P5).
    ///
    /// # Errors
    ///
    /// [`crate::KernelError::Db`] for engine errors.
    pub async fn messages_since(
        &self,
        after: DateTime<Utc>,
        limit: u32,
    ) -> Result<Vec<MessageRecord>> {
        let rows: Vec<MessageRecord> = self
            .db()
            .query(
                "SELECT * FROM message WHERE valid_from > $after \
                 ORDER BY valid_from ASC LIMIT $limit",
            )
            .bind(("after", after))
            .bind(("limit", limit))
            .await?
            .take(0)?;
        Ok(rows)
    }

    /// Count of message rows per session for a list of sessions —
    /// cheap enough for the `superx sessions` listing.
    ///
    /// # Errors
    ///
    /// [`crate::KernelError::Db`] for engine errors.
    pub async fn session_message_count(&self, session: RecordId) -> Result<i64> {
        #[derive(SurrealValue)]
        struct CountRow {
            c: i64,
        }
        let rows: Vec<CountRow> = self
            .db()
            .query("SELECT count() AS c FROM message WHERE session = $sess GROUP ALL")
            .bind(("sess", session))
            .await?
            .take(0)?;
        Ok(rows.first().map(|r| r.c).unwrap_or(0))
    }
}

#[derive(Debug, SurrealValue)]
struct MessageRow {
    session: RecordId,
    agent: RecordId,
    role: String,
    content: String,
    raw: Option<Object>,
    seq: Option<i64>,
    emitted_at: Option<DateTime<Utc>>,
    valid_from: DateTime<Utc>,
}

/// Convenience: wrap a `serde_json::Value` object as a substrate
/// [`Object`] for `raw` payloads. Non-object JSON becomes an object
/// with a single `"value"` key — nothing is ever dropped.
#[must_use]
pub fn json_to_object(v: &serde_json::Value) -> Object {
    match value_from_json(v) {
        Value::Object(o) => o,
        other => {
            let mut o = Object::new();
            o.insert("value".to_string(), other);
            o
        }
    }
}

/// Recursively convert `serde_json::Value` → substrate [`Value`].
#[must_use]
pub fn value_from_json(v: &serde_json::Value) -> Value {
    match v {
        serde_json::Value::Null => Value::None,
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Number(i.into())
            } else {
                Value::Number(n.as_f64().unwrap_or(0.0).into())
            }
        }
        serde_json::Value::String(s) => Value::String(s.clone()),
        serde_json::Value::Array(items) => Value::Array(surrealdb::types::Array::from(
            items.iter().map(value_from_json).collect::<Vec<_>>(),
        )),
        serde_json::Value::Object(map) => {
            let mut o = Object::new();
            for (k, val) in map {
                o.insert(k.clone(), value_from_json(val));
            }
            Value::Object(o)
        }
    }
}
