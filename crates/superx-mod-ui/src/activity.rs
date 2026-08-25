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
        id: superx_ops::record_uuid(&m.id),
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
        id: superx_ops::record_uuid(&a.id),
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

/// A raw captured row awaiting rendering — rendering happens ONLY for
/// rows that survive truncation.
enum Raw<'a> {
    Msg(&'a MessageRecord),
    Act(&'a TelemetryRecord),
}

/// Merge message + action rows by capture time and keep the NEWEST
/// `limit`, rendered oldest-first — per-stream limits alone would
/// return 2×limit. This function is the SOLE ordering authority: the
/// input slices may arrive in any order.
fn merge_newest(
    messages: &[MessageRecord],
    actions: &[TelemetryRecord],
    limit: u32,
) -> Vec<SseEvent> {
    let mut rows: Vec<(chrono::DateTime<chrono::Utc>, Raw)> =
        Vec::with_capacity(messages.len() + actions.len());
    for m in messages {
        rows.push((m.valid_from, Raw::Msg(m)));
    }
    for a in actions {
        rows.push((a.valid_from, Raw::Act(a)));
    }
    rows.sort_by_key(|&(t, _)| t);
    let excess = rows.len().saturating_sub(limit as usize);
    rows.split_off(excess)
        .into_iter()
        .map(|(_, r)| match r {
            Raw::Msg(m) => message_event(m),
            Raw::Act(a) => action_event(a),
        })
        .collect()
}

/// Where a backwards page starts: the newest rows STRICTLY older than
/// this instant. `None` means the present — the first page.
///
/// Strict `<` cannot skip a row: the cursor is the oldest row the
/// client already holds, so its exact instant is already on screen.
pub type Before = Option<chrono::DateTime<chrono::Utc>>;

/// A keyword the feed is filtered to, lowercased. `None` = no filter.
pub type Query<'a> = Option<&'a str>;

/// The message-side keyword clause. Searching runs over the captured
/// text itself, in the engine, so it reaches ALL history rather than
/// whatever the client happens to be holding.
const MSG_MATCH: &str = "string::contains(string::lowercase(content), $q)";

/// The payload fields a keyword search looks inside. A telemetry
/// payload is `any`, so there is no schema to enumerate — this is the
/// vocabulary the adapters and modules actually emit (the same kind of
/// captured-shape knowledge as `stats::WRITE_TOOLS`). Each is read as
/// the string it already is: no `<string>` cast, which §14 forbids.
const SEARCHED_PAYLOAD_FIELDS: &[&str] = &[
    "tool", "name", "file", "line", "session", "error", "source", "kind", "url", "detail",
    "snippet", "adapter", "reason", "status",
];

/// The action-side keyword clause: the event name, plus every payload
/// field above. A field the payload lacks coalesces to the empty
/// string rather than dropping the row.
fn act_match() -> String {
    let fields: Vec<String> = SEARCHED_PAYLOAD_FIELDS
        .iter()
        .map(|f| format!("string::contains(string::lowercase(payload.{f} ?? ''), $q)"))
        .collect();
    format!(
        "(string::contains(string::lowercase(lifecycle_event), $q) OR {})",
        fields.join(" OR ")
    )
}

/// Assemble one page query. Every fragment spliced in here is a
/// compile-time constant; the operator's keyword and the cursor reach
/// the engine ONLY as bound parameters, never as query text.
fn page_query(table: &str, scope: Option<&str>, before: Before, q: Query, keyword: &str) -> String {
    let mut wheres: Vec<&str> = Vec::new();
    if let Some(s) = scope {
        wheres.push(s);
    }
    if before.is_some() {
        wheres.push("valid_from < $before");
    }
    if q.is_some() {
        wheres.push(keyword);
    }
    let clause = if wheres.is_empty() {
        String::new()
    } else {
        format!("WHERE {} ", wheres.join(" AND "))
    };
    format!("SELECT * FROM {table} {clause}ORDER BY valid_from DESC LIMIT $limit")
}

/// Run a page query with only the bindings its clauses actually use.
macro_rules! page {
    ($kernel:expr, $sql:expr, $limit:expr, $before:expr, $q:expr $(, $extra:expr)*) => {{
        let mut stmt = $kernel.db().query(&$sql).bind(("limit", $limit));
        $( stmt = stmt.bind($extra); )*
        if let Some(cut) = $before {
            stmt = stmt.bind(("before", cut));
        }
        if let Some(k) = $q {
            stmt = stmt.bind(("q", k.to_lowercase()));
        }
        stmt.await?.take(0)?
    }};
}

/// The NEWEST `limit` messages across ALL sessions, optionally walking
/// backwards from a cursor and/or filtered to a keyword (any order —
/// merge_newest sorts).
async fn recent_messages(
    kernel: &Kernel,
    limit: u32,
    before: Before,
    q: Query<'_>,
) -> Result<Vec<MessageRecord>> {
    if before.is_none() && q.is_none() {
        let rows: Vec<MessageRecord> = kernel
            .db()
            .query("SELECT * FROM message ORDER BY valid_from DESC LIMIT $limit")
            .bind(("limit", limit))
            .await?
            .take(0)?;
        return Ok(rows);
    }
    let sql = page_query("message", None, before, q, MSG_MATCH);
    let rows: Vec<MessageRecord> = page!(kernel, sql, limit, before, q);
    Ok(rows)
}

/// The NEWEST `limit` telemetry events, optionally older than a cursor
/// and/or filtered to a keyword. The kernel's own `recent_telemetry`
/// has no cursor or search form, and adding one is a kernel change —
/// so the paged read lives here, in the module, over the same table
/// and index.
async fn recent_actions(
    kernel: &Kernel,
    limit: u32,
    before: Before,
    q: Query<'_>,
) -> Result<Vec<TelemetryRecord>> {
    if before.is_none() && q.is_none() {
        return kernel.recent_telemetry(limit).await;
    }
    let sql = page_query("telemetry_stream", None, before, q, &act_match());
    let rows: Vec<TelemetryRecord> = page!(kernel, sql, limit, before, q);
    Ok(rows)
}

/// The GLOBAL feed: everything the OS captured, everyone and every
/// session in one place — the NEWEST `limit` rows of the merged
/// stream, oldest first (issue #187: Activity's historical backlog).
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn global_activity(
    kernel: &Kernel,
    limit: u32,
    before: Before,
    q: Query<'_>,
) -> Result<Vec<SseEvent>> {
    let actions = recent_actions(kernel, limit, before, q).await?;
    let messages = recent_messages(kernel, limit, before, q).await?;
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
    before: Before,
    q: Query<'_>,
) -> Result<Vec<SseEvent>> {
    let scope = session_scope(kernel, session.clone()).await?;

    let msg_sql = page_query("message", Some("session = $sess"), before, q, MSG_MATCH);
    let messages: Vec<MessageRecord> =
        page!(kernel, msg_sql, limit, before, q, ("sess", session.clone()));

    // Two ways an action belongs to a session (see the module doc);
    // without a resolved scope only the subject arm can match — it must
    // never widen to everything.
    let act_sql = page_query(
        "telemetry_stream",
        Some(match scope {
            Some(_) => "(subject = $sess OR (agent = $agent AND payload.session = $src))",
            None => "subject = $sess",
        }),
        before,
        q,
        &act_match(),
    );
    let actions: Vec<TelemetryRecord> = match scope {
        Some((agent, src_key)) => page!(
            kernel,
            act_sql,
            limit,
            before,
            q,
            ("sess", session),
            ("agent", agent),
            ("src", src_key)
        ),
        None => page!(kernel, act_sql, limit, before, q, ("sess", session)),
    };
    Ok(merge_newest(&messages, &actions, limit))
}

/// Pull an i64 field out of a dynamic row object; 0 when absent.
fn int_of(o: &superx_kernel::types::Object, key: &str) -> i64 {
    match o.get(key) {
        Some(Value::Number(n)) => n.to_int().unwrap_or(0),
        _ => 0,
    }
}

/// Pull a nested object field out of a dynamic row.
fn obj_of(row: &Value, key: &str) -> Option<superx_kernel::types::Object> {
    match row {
        Value::Object(o) => match o.get(key) {
            Some(Value::Object(inner)) => Some(inner.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// A session's token telemetry, mined from the raw events adapters
/// already preserve (issue #200): `(context, output_total)`.
///
/// - `context` — the NEWEST usage-bearing message's prompt footprint:
///   Claude Code `raw.message.usage` (input + cache_read +
///   cache_creation) or Gemini `raw.tokens` (total; else input+cached).
/// - `output_total` — Σ output tokens across the session.
///
/// Both `None` when the session carries no usage data (e.g. telemetry-
/// only agents). Pure SELECT over the `(session, valid_from)` index.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
/// The model CURRENTLY doing this session's work: the newest message
/// that names one. A session outlives the model choice — the operator
/// switches mid-conversation — so this is a moving fact, read fresh
/// rather than stamped on the session once (issue #241).
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn session_model(kernel: &Kernel, session: RecordId) -> Result<Option<String>> {
    Ok(session_model_effort(kernel, session).await?.0)
}

/// The session's CURRENT model and reasoning effort, in one read.
/// Effort is a separate newest-value lookup because a session can name
/// a model on one message and its effort on another.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn session_model_effort(
    kernel: &Kernel,
    session: RecordId,
) -> Result<(Option<String>, Option<String>)> {
    let newest = |field: &str, guard: &str| {
        format!(
            "SELECT {field} AS v, valid_from FROM message \
             WHERE session = $sess AND {guard} \
             ORDER BY valid_from DESC LIMIT 1"
        )
    };
    let pick = |rows: Vec<Value>| {
        rows.first().and_then(|row| match row {
            Value::Object(o) => match o.get("v") {
                Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
                _ => None,
            },
            _ => None,
        })
    };
    let model: Vec<Value> = kernel
        .db()
        .query(newest(
            "raw.message.model ?? raw.model",
            "(raw.message.model != NONE OR raw.model != NONE)",
        ))
        .bind(("sess", session.clone()))
        .await?
        .take(0)?;
    let effort: Vec<Value> = kernel
        .db()
        .query(newest("raw.effort", "raw.effort != NONE"))
        .bind(("sess", session))
        .await?
        .take(0)?;
    Ok((pick(model), pick(effort)))
}

pub async fn session_token_stats(
    kernel: &Kernel,
    session: RecordId,
) -> Result<(Option<i64>, Option<i64>)> {
    let rows: Vec<Value> = kernel
        .db()
        .query(
            "SELECT math::sum(raw.message.usage.output_tokens ?? raw.tokens.output ?? 0) \
                 AS toks \
             FROM message WHERE session = $sess GROUP ALL",
        )
        .bind(("sess", session.clone()))
        .await?
        .take(0)?;
    let output_total = rows
        .first()
        .and_then(|row| match row {
            Value::Object(o) => Some(int_of(o, "toks")),
            _ => None,
        })
        .filter(|&n| n > 0);

    let rows: Vec<Value> = kernel
        .db()
        .query(
            "SELECT raw.message.usage AS cu, raw.tokens AS gu, valid_from \
             FROM message WHERE session = $sess \
               AND (raw.message.usage != NONE OR raw.tokens != NONE) \
             ORDER BY valid_from DESC LIMIT 1",
        )
        .bind(("sess", session))
        .await?
        .take(0)?;
    let context = rows.first().and_then(|row| {
        if let Some(cu) = obj_of(row, "cu") {
            let n = int_of(&cu, "input_tokens")
                + int_of(&cu, "cache_read_input_tokens")
                + int_of(&cu, "cache_creation_input_tokens");
            return (n > 0).then_some(n);
        }
        if let Some(gu) = obj_of(row, "gu") {
            let total = int_of(&gu, "total");
            let n = if total > 0 {
                total
            } else {
                int_of(&gu, "input") + int_of(&gu, "cached")
            };
            return (n > 0).then_some(n);
        }
        None
    });
    Ok((context, output_total))
}

/// Count of a session's action events (same matching as
/// [`session_activity`]) — the sessions list shows TOTAL activity
/// (messages + actions), not messages alone (issue #187).
///
/// `scope` is the pre-resolved `(agent entity, source key)` pair —
/// callers iterating many sessions resolve agents ONCE and pass the
/// scope in, instead of paying a descriptor re-read + agent lookup
/// per session (review finding). `None` drops the payload.session
/// arm (matches nothing, never everything).
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn session_action_count(
    kernel: &Kernel,
    session: RecordId,
    scope: Option<(RecordId, String)>,
) -> Result<i64> {
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
