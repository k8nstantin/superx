//! Per-session activity — the merged, chronologically ordered record
//! of everything the OS captured for ONE session: its `message` rows
//! AND its `telemetry_stream` events (issue #172).
//!
//! Sessions are the dashboard's top-level grouping of activity
//! (operator directive 2026-08-19): even when a session spans several
//! actors, its events are still bound to that session. The global
//! Activity stream complements this by attributing every row to its
//! session.
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
//! Known gap (tracked): `transcript_raw` events carry no session key,
//! so unparseable transcript lines don't surface here yet.
//!
//! Everything here is pure SELECT — readers must not mutate the
//! stream they observe.

use superx_kernel::types::{RecordId, Value};
use superx_kernel::{Kernel, MessageRecord, Result, TelemetryRecord};

use crate::api::SessionEvent;

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

/// Everything captured for one session, oldest first: the NEWEST
/// `limit` rows of the merged stream (`message` rows + the session's
/// `telemetry_stream` events, merged by capture time). Newest-N so the
/// pinned-to-bottom view always ends at the session's present, not at
/// week-old history. Global events (module lifecycle, boot, discovery
/// of other sessions) are not a session's activity and never appear.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn session_activity(
    kernel: &Kernel,
    session: RecordId,
    limit: u32,
) -> Result<Vec<SessionEvent>> {
    let (src, agent_name) = descriptor_parts(kernel, session.clone()).await?;
    // Resolve the session's agent so the payload.session arm is scoped
    // to it. When either the source key or the agent cannot be
    // resolved (corrupt/foreign descriptor), that arm is dropped
    // entirely — it must match nothing, never everything.
    let agent = match agent_name {
        Some(ref name) => {
            kernel
                .find_entity_by_name("node_agent", "attr_agent_descriptor", name)
                .await?
        }
        None => None,
    };

    // Both streams fetch their NEWEST `limit` rows (DESC), then flip
    // back to reading order.
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
    let mut actions: Vec<TelemetryRecord> = match (agent, src) {
        (Some(agent), Some(src_key)) => {
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
        _ => {
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

    let mut events: Vec<(chrono::DateTime<chrono::Utc>, SessionEvent)> =
        Vec::with_capacity(messages.len() + actions.len());
    for m in &messages {
        events.push((
            m.valid_from,
            SessionEvent {
                kind: "message".to_string(),
                role: Some(m.role.clone()),
                rendered: superx_ops::render_message(m).trim_end().to_string(),
                valid_from: m.valid_from.to_rfc3339(),
            },
        ));
    }
    for a in &actions {
        events.push((
            a.valid_from,
            SessionEvent {
                kind: "action".to_string(),
                role: None,
                rendered: superx_ops::render_event(a).trim_end().to_string(),
                valid_from: a.valid_from.to_rfc3339(),
            },
        ));
    }
    events.sort_by_key(|&(t, _)| t);
    // The merged page keeps the NEWEST `limit` rows — per-stream
    // limits alone would return up to 2×limit.
    let excess = events.len().saturating_sub(limit as usize);
    Ok(events.split_off(excess).into_iter().map(|(_, e)| e).collect())
}
