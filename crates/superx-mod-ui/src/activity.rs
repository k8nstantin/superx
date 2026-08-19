//! The activity feed — ONE merged, chronologically ordered stream of
//! everything the OS captures: `message` rows AND `telemetry_stream`
//! events, in one row shape ([`SseEvent`]) end-to-end (issues #172,
//! #187).
//!
//! The same feed serves two scopes (operator directive 2026-08-19:
//! the feeds are identical, only scope differs):
//! - [`global_activity`] — everyone and everything, one place;
//! - [`session_activity`] — the feed filtered to one session
//!   (sessions are the top-level grouping of activity).
//!
//! Action rows are matched to a session two ways:
//! - `subject = <session entity>` — e.g. `message_captured`, whose
//!   emitter IS the session;
//! - `agent = <session's agent> AND payload.session = <source key>` —
//!   e.g. `transcript_event` / `tool_call`, which are attributed to a
//!   source entity but stamp the source-native session key on their
//!   payload. The agent scope prevents cross-session bleed when two
//!   agents' sessions share a fallback key such as `unknown-session`.
//!
//! Known gap (#186): `transcript_raw` events carry no session key, so
//! unparseable transcript lines surface only in the global feed.
//!
//! Everything here is pure SELECT — readers must not mutate the
//! stream they observe.

use superx_kernel::types::{RecordId, Value};
use superx_kernel::{Kernel, MessageRecord, Result, TelemetryRecord};

use crate::api::SseEvent;

/// The source-session key an adapter stamped on a per-session event
/// (`payload.session`), when present. `None` for global events and
/// non-object payloads.
#[must_use]
pub fn session_key_of(payload: &Value) -> Option<String> {
    match payload {
        Value::Object(o) => match o.get("session") {
            Some(Value::String(s)) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// One feed row from a captured message — the SAME shape the SSE
/// bridge broadcasts, so backlog and live rows are interchangeable.
#[must_use]
pub fn message_event(m: &MessageRecord) -> SseEvent {
    SseEvent {
        kind: "message".to_string(),
        rendered: superx_ops::render_message(m).trim_end().to_string(),
        role: Some(m.role.clone()),
        agent_id: Some(superx_ops::record_uuid(&m.agent)),
        session_id: Some(superx_ops::record_uuid(&m.session)),
        session_src: None,
        valid_from: m.valid_from.to_rfc3339(),
    }
}

/// One feed row from a telemetry event — the SAME shape the SSE
/// bridge broadcasts.
#[must_use]
pub fn action_event(a: &TelemetryRecord) -> SseEvent {
    SseEvent {
        kind: "action".to_string(),
        rendered: superx_ops::render_event(a).trim_end().to_string(),
        role: None,
        agent_id: a.agent.as_ref().map(superx_ops::record_uuid),
        session_id: None,
        session_src: session_key_of(&a.payload),
        valid_from: a.valid_from.to_rfc3339(),
    }
}

/// The session's source key and agent name from its current
/// `attr_session_descriptor` payload (`{name, session, locator}` —
/// written by the capture engine's `ensure_session`; `name` is
/// `<agent>/<key>`).
async fn descriptor_parts(
    kernel: &Kernel,
    session: RecordId,
) -> Result<(Option<String>, Option<String>)> {
    let desc = kernel
        .current_state(session, "attr_session_descriptor")
        .await?;
    let Some(Value::Object(o)) = desc else {
        return Ok((None, None));
    };
    let src = match o.get("session") {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    };
    let agent_name = match o.get("name") {
        Some(Value::String(n)) => n.split('/').next().map(str::to_string),
        _ => None,
    };
    Ok((src, agent_name))
}

/// Resolve a session's scoping handles for action matching: the agent
/// entity + source key. `None` parts drop the payload.session arm —
/// it must match nothing, never everything.
async fn session_scope(
    kernel: &Kernel,
    session: RecordId,
) -> Result<Option<(RecordId, String)>> {
    let (src, agent_name) = descriptor_parts(kernel, session).await?;
    let agent = match agent_name {
        Some(ref name) => {
            kernel
                .find_entity_by_name("node_agent", "attr_agent_descriptor", name)
                .await?
        }
        None => None,
    };
    Ok(match (agent, src) {
        (Some(agent), Some(src)) => Some((agent, src)),
        _ => None,
    })
}

/// Merge message + action rows by capture time (oldest first) and keep
/// the NEWEST `limit` — per-stream limits alone would return 2×limit.
fn merge_newest(
    messages: &[MessageRecord],
    actions: &[TelemetryRecord],
    limit: u32,
) -> Vec<SseEvent> {
    let mut events: Vec<(chrono::DateTime<chrono::Utc>, SseEvent)> =
        Vec::with_capacity(messages.len() + actions.len());
    for m in messages {
        events.push((m.valid_from, message_event(m)));
    }
    for a in actions {
        events.push((a.valid_from, action_event(a)));
    }
    events.sort_by_key(|&(t, _)| t);
    let excess = events.len().saturating_sub(limit as usize);
    events.split_off(excess).into_iter().map(|(_, e)| e).collect()
}

/// The NEWEST `limit` messages across ALL sessions, oldest first.
async fn recent_messages(kernel: &Kernel, limit: u32) -> Result<Vec<MessageRecord>> {
    let mut rows: Vec<MessageRecord> = kernel
        .db()
        .query("SELECT * FROM message ORDER BY valid_from DESC LIMIT $limit")
        .bind(("limit", limit))
        .await?
        .take(0)?;
    rows.reverse();
    Ok(rows)
}

/// The GLOBAL feed: everything the OS captured, everyone and every
/// session in one place — the NEWEST `limit` rows of the merged
/// stream, oldest first (issue #187: Activity's historical backlog).
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn global_activity(kernel: &Kernel, limit: u32) -> Result<Vec<SseEvent>> {
    let mut actions = kernel.recent_telemetry(limit).await?;
    actions.reverse();
    let messages = recent_messages(kernel, limit).await?;
    Ok(merge_newest(&messages, &actions, limit))
}

/// The feed filtered to ONE session: the NEWEST `limit` rows of its
/// messages + its action events, oldest first. Newest-N so the
/// pinned-to-bottom view always ends at the session's present. Global
/// events (module lifecycle, boot) are not a session's activity and
/// never appear here.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn session_activity(
    kernel: &Kernel,
    session: RecordId,
    limit: u32,
) -> Result<Vec<SseEvent>> {
    let scope = session_scope(kernel, session.clone()).await?;
    let mut messages: Vec<MessageRecord> = kernel
        .db()
        .query(
            "SELECT * FROM message WHERE session = $sess \
             ORDER BY valid_from DESC LIMIT $limit",
        )
        .bind(("sess", session.clone()))
        .bind(("limit", limit))
        .await?
        .take(0)?;
    messages.reverse();
    let mut actions: Vec<TelemetryRecord> = match scope {
        Some((agent, src_key)) => {
            kernel
                .db()
                .query(
                    "SELECT * FROM telemetry_stream \
                     WHERE subject = $sess \
                        OR (agent = $agent AND payload.session = $src) \
                     ORDER BY valid_from DESC LIMIT $limit",
                )
                .bind(("sess", session))
                .bind(("agent", agent))
                .bind(("src", src_key))
                .bind(("limit", limit))
                .await?
                .take(0)?
        }
        None => {
            kernel
                .db()
                .query(
                    "SELECT * FROM telemetry_stream WHERE subject = $sess \
                     ORDER BY valid_from DESC LIMIT $limit",
                )
                .bind(("sess", session))
                .bind(("limit", limit))
                .await?
                .take(0)?
        }
    };
    actions.reverse();
    Ok(merge_newest(&messages, &actions, limit))
}

/// Count of a session's action events (same matching as
/// [`session_activity`]) — the sessions list shows TOTAL activity
/// (messages + actions), not messages alone (issue #187).
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn session_action_count(kernel: &Kernel, session: RecordId) -> Result<i64> {
    let scope = session_scope(kernel, session.clone()).await?;
    let rows: Vec<Value> = match scope {
        Some((agent, src_key)) => {
            kernel
                .db()
                .query(
                    "SELECT count() AS c FROM telemetry_stream \
                     WHERE subject = $sess \
                        OR (agent = $agent AND payload.session = $src) \
                     GROUP ALL",
                )
                .bind(("sess", session))
                .bind(("agent", agent))
                .bind(("src", src_key))
                .await?
                .take(0)?
        }
        None => {
            kernel
                .db()
                .query(
                    "SELECT count() AS c FROM telemetry_stream \
                     WHERE subject = $sess GROUP ALL",
                )
                .bind(("sess", session))
                .await?
                .take(0)?
        }
    };
    // Row shape: {c: <count>} — read through the kernel's re-exported
    // Value, per the module layering rule (no direct surrealdb dep).
    Ok(rows
        .first()
        .and_then(|row| match row {
            Value::Object(o) => match o.get("c") {
                Some(Value::Number(n)) => n.to_int(),
                _ => None,
            },
            _ => None,
        })
        .unwrap_or(0))
}
