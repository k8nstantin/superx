//! Per-session activity — the merged, chronologically ordered record
//! of everything the OS captured for ONE session: its `message` rows
//! AND its `telemetry_stream` events (issue #172).
//!
//! Sessions are the dashboard's top-level unit (operator directive
//! 2026-08-19): a global cross-session stream mixes unrelated work and
//! carries no meaning, so it does not exist. Even when a session spans
//! several actors, its events are still bound to that session.
//!
//! Action rows are matched to a session two ways, covering every
//! adapter-emitted per-session event:
//! - `subject = <session entity>` — e.g. `message_captured`, whose
//!   emitter IS the session;
//! - `payload.session = <source key>` — e.g. `transcript_event` /
//!   `tool_call`, which are attributed to a source entity but stamp
//!   the source-native session key on their payload.
//!
//! Everything here is pure SELECT — readers must not mutate the
//! stream they observe.

use superx_kernel::types::{RecordId, Value};
use superx_kernel::{Kernel, Result, TelemetryRecord};

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

/// The session's own source key from its current
/// `attr_session_descriptor` payload (`{name, session, locator}` —
/// written by the capture engine's `ensure_session`).
async fn source_key(kernel: &Kernel, session: RecordId) -> Result<Option<String>> {
    let desc = kernel
        .current_state(session, "attr_session_descriptor")
        .await?;
    Ok(match desc {
        Some(Value::Object(ref o)) => match o.get("session") {
            Some(Value::String(s)) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    })
}

/// Everything captured for one session, oldest first: `message` rows
/// and the session's `telemetry_stream` events, merged by capture
/// time. Global events (module lifecycle, boot, discovery) are not a
/// session's activity and never appear here.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn session_activity(
    kernel: &Kernel,
    session: RecordId,
    limit: u32,
) -> Result<Vec<SessionEvent>> {
    let src = source_key(kernel, session.clone()).await?;
    let messages = kernel.session_messages(session.clone(), limit).await?;
    // A missing source key (corrupt/foreign descriptor) must make the
    // payload.session arm match NOTHING, not everything — adapters
    // never stamp an empty session key.
    let src_key = src.unwrap_or_default();
    let actions: Vec<TelemetryRecord> = kernel
        .db()
        .query(
            "SELECT * FROM telemetry_stream \
             WHERE subject = $sess OR payload.session = $src \
             ORDER BY valid_from ASC LIMIT $limit",
        )
        .bind(("sess", session))
        .bind(("src", src_key))
        .bind(("limit", limit))
        .await?
        .take(0)?;

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
    Ok(events.into_iter().map(|(_, e)| e).collect())
}
