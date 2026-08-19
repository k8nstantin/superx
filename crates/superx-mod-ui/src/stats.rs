//! Status-page statistics (issue #228) — the numbers the OS already
//! captures, aggregated for display: totals from the substrate,
//! plus a bounded walk over the newest raw messages that mines what
//! the agents actually DID — tools run, lines of code written,
//! per-session activity. Window-scoped figures are labeled with
//! their window; nothing here pretends to be all-time when it isn't.
//!
//! Pure SELECT throughout — readers must not mutate the stream they
//! observe. All code lives in the ui module; kernel untouched.

use std::collections::HashMap;

use superx_kernel::types::{Object, Value};
use superx_kernel::{Kernel, MessageRecord, NodeKind, Result};

use crate::api::{NameCount, SessionStat, StatsSummary, TimeCount};

/// Telemetry window backing the events/min timeline (same bound the
/// charts endpoint has always used).
const EVENT_WINDOW: u32 = 2000; // skill-allow: §9-const — aggregation page bound

/// A session counts as ACTIVE when it has a message newer than this.
/// Matches the Sessions page's liveness threshold.
pub const ACTIVE_SECS_PARAM: &str = "attr_ui_active_session_secs";
const DEFAULT_ACTIVE_SECS: i64 = 300; // skill-allow: §9-const — bootstrap fallback, param-overridable

/// Tools whose input content counts as CODE WRITTEN.
const WRITE_TOOLS: &[&str] = &["Write", "Edit", "MultiEdit", "NotebookEdit"];

fn obj(v: &Value) -> Option<&Object> {
    match v {
        Value::Object(o) => Some(o),
        _ => None,
    }
}

fn get_str<'a>(o: &'a Object, key: &str) -> Option<&'a str> {
    match o.get(key) {
        Some(Value::String(s)) => Some(s.as_str()),
        _ => None,
    }
}

fn get_int(o: &Object, key: &str) -> i64 {
    match o.get(key) {
        Some(Value::Number(n)) => n.to_int().unwrap_or(0),
        _ => 0,
    }
}

fn line_count(s: &str) -> i64 {
    if s.is_empty() {
        0
    } else {
        s.split('\n').count() as i64
    }
}

/// Lines of code a single tool_use block writes (Write/Edit content;
/// MultiEdit sums its edits).
fn block_lines(name: &str, input: &Object) -> i64 {
    if !WRITE_TOOLS.contains(&name) {
        return 0;
    }
    if let Some(s) = get_str(input, "content").or_else(|| get_str(input, "new_string")) {
        return line_count(s);
    }
    if let Some(Value::Array(edits)) = input.get("edits") {
        return edits
            .iter()
            .filter_map(|e| obj(e))
            .filter_map(|e| get_str(e, "new_string"))
            .map(line_count)
            .sum();
    }
    0
}

#[derive(Default)]
struct SessAgg {
    messages: i64,
    lines: i64,
    out_tokens: i64,
}

/// One in-engine `count() GROUP ALL` over a table.
async fn count_rows(kernel: &Kernel, query: &'static str) -> Result<i64> {
    let rows: Vec<Value> = kernel.db().query(query).await?.take(0)?;
    Ok(rows.first().and_then(|r| obj(r).map(|o| get_int(o, "c"))).unwrap_or(0))
}

/// The full status aggregation. `window` bounds the raw-message walk
/// (newest N) — the window-scoped figures carry it back for honest
/// labeling.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn stats_summary(kernel: &Kernel, window: u32) -> Result<StatsSummary> {
    // ── cheap in-engine totals ──────────────────────────────────────
    let events_total =
        count_rows(kernel, "SELECT count() AS c FROM telemetry_stream GROUP ALL").await?;
    let messages_total = count_rows(kernel, "SELECT count() AS c FROM message GROUP ALL").await?;
    let rows: Vec<Value> = kernel
        .db()
        .query(
            "SELECT math::sum(raw.message.usage.output_tokens ?? raw.tokens.output ?? 0) \
                 AS c FROM message GROUP ALL",
        )
        .await?
        .take(0)?;
    let output_tokens_total = rows
        .first()
        .and_then(|r| obj(r).map(|o| get_int(o, "c")))
        .unwrap_or(0);

    // Active sessions: distinct sessions with a message inside the
    // activity threshold (parameter on the ui entity; default matches
    // the Sessions page's liveness window).
    let active_secs = resolved_active_secs(kernel).await;
    let cutoff = chrono::Utc::now() - chrono::Duration::seconds(active_secs);
    let rows: Vec<Value> = kernel
        .db()
        .query("SELECT session FROM message WHERE valid_from > $cutoff GROUP BY session")
        .bind(("cutoff", cutoff))
        .await?
        .take(0)?;
    let sessions_active = rows.len() as i64;

    let agents = kernel
        .list_named_entities("node_agent", "attr_agent_descriptor")
        .await?
        .len() as i64;
    let sessions = kernel
        .list_named_entities("node_session", "attr_session_descriptor")
        .await?;
    let sessions_total = sessions.len() as i64;
    // uuid → "agent/uuid8" display identity.
    let mut identity: HashMap<String, String> = HashMap::new();
    for s in &sessions {
        let name = match &s.payload {
            Value::Object(o) => get_str(o, "name").unwrap_or("?").to_string(),
            _ => "?".to_string(),
        };
        let agent = name.split('/').next().unwrap_or("?").to_string();
        let uuid = superx_ops::record_uuid(&s.entity_id);
        identity.insert(uuid.clone(), format!("{agent}/{}", &uuid[..uuid.len().min(8)]));
    }

    let mut modules_total = 0i64;
    let mut modules_active = 0i64;
    for kind in [NodeKind::KernelModule, NodeKind::Adapter] {
        if let Ok(list) = kernel.list_with_status(kind).await {
            for s in &list {
                modules_total += 1;
                if s.lifecycle.short_tag() == "active" {
                    modules_active += 1;
                }
            }
        }
    }

    // ── the raw-message window walk: what the agents actually did ──
    let msgs: Vec<MessageRecord> = kernel
        .db()
        .query("SELECT * FROM message ORDER BY valid_from DESC LIMIT $limit")
        .bind(("limit", window))
        .await?
        .take(0)?;
    let mut tools: HashMap<String, i64> = HashMap::new();
    let mut lines_written = 0i64;
    let mut per_session: HashMap<String, SessAgg> = HashMap::new();
    for m in &msgs {
        let sid = superx_ops::record_uuid(&m.session);
        let agg = per_session.entry(sid).or_default();
        agg.messages += 1;
        let Some(raw) = &m.raw else { continue };
        // Claude-style usage + blocks: raw.message.{usage, content[]}.
        if let Some(Value::Object(msg)) = raw.get("message") {
            if let Some(Value::Object(usage)) = msg.get("usage") {
                agg.out_tokens += get_int(usage, "output_tokens");
            }
            if let Some(Value::Array(blocks)) = msg.get("content") {
                for b in blocks.iter() {
                    let Some(block) = obj(b) else { continue };
                    if get_str(block, "type") != Some("tool_use") {
                        continue;
                    }
                    let name = get_str(block, "name").unwrap_or("tool").to_string();
                    *tools.entry(name.clone()).or_insert(0) += 1;
                    if let Some(Value::Object(input)) = block.get("input") {
                        let n = block_lines(&name, input);
                        lines_written += n;
                        agg.lines += n;
                    }
                }
            }
        }
        // Gemini-style: raw.tokens.output + raw.toolCalls[].
        if let Some(Value::Object(toks)) = raw.get("tokens") {
            agg.out_tokens += get_int(toks, "output");
        }
        if let Some(Value::Array(calls)) = raw.get("toolCalls") {
            for c in calls.iter() {
                let Some(call) = obj(c) else { continue };
                let name = get_str(call, "name").unwrap_or("tool").to_string();
                *tools.entry(name).or_insert(0) += 1;
            }
        }
    }
    let tools_window: i64 = tools.values().sum();
    let mut tools: Vec<NameCount> = tools
        .into_iter()
        .map(|(name, value)| NameCount { name, value })
        .collect();
    tools.sort_by_key(|t| std::cmp::Reverse(t.value));

    let mut top: Vec<(String, SessAgg)> = per_session.into_iter().collect();
    top.sort_by_key(|(_, a)| std::cmp::Reverse(a.messages));
    let top_sessions: Vec<SessionStat> = top
        .into_iter()
        .take(6)
        .map(|(sid, a)| SessionStat {
            identity: identity
                .get(&sid)
                .cloned()
                .unwrap_or_else(|| sid.chars().take(8).collect()),
            session_id: sid,
            messages: a.messages,
            lines_written: a.lines,
            output_tokens: a.out_tokens,
        })
        .collect();

    // ── timeline / roles / boots (the former charts endpoint's data) ─
    let events = kernel.recent_telemetry(EVENT_WINDOW).await?;
    let mut per_minute: std::collections::BTreeMap<String, i64> = Default::default();
    let mut boots = Vec::new();
    for e in &events {
        *per_minute
            .entry(e.valid_from.format("%H:%M").to_string())
            .or_insert(0) += 1;
        if e.lifecycle_event == "boot_complete" {
            if let Value::Object(o) = &e.payload {
                boots.push(TimeCount {
                    t: e.valid_from.format("%m-%d %H:%M").to_string(),
                    value: get_int(o, "duration_ms"),
                });
            }
        }
    }
    boots.reverse(); // recent_telemetry is newest-first; charts read left→right
    let mut message_roles = Vec::new();
    let rows: Vec<Value> = kernel
        .db()
        .query("SELECT role, count() AS c FROM message GROUP BY role")
        .await?
        .take(0)?;
    for row in rows {
        if let Some(o) = obj(&row) {
            if let Some(role) = get_str(o, "role") {
                message_roles.push(NameCount {
                    name: role.to_string(),
                    value: get_int(o, "c"),
                });
            }
        }
    }

    Ok(StatsSummary {
        agents,
        sessions_total,
        sessions_active,
        modules_total,
        modules_active,
        events_total,
        messages_total,
        output_tokens_total,
        lines_written,
        tools_window,
        window_messages: window,
        events_per_minute: per_minute
            .into_iter()
            .map(|(t, value)| TimeCount { t, value })
            .collect(),
        message_roles,
        boot_durations: boots,
        tools,
        top_sessions,
    })
}

/// Resolve the active-session threshold from the ui module's
/// parameter, else the default.
async fn resolved_active_secs(kernel: &Kernel) -> i64 {
    let Ok(Some(entity)) = kernel
        .find_module_by_name(NodeKind::KernelModule, crate::MODULE_NAME)
        .await
    else {
        return DEFAULT_ACTIVE_SECS;
    };
    match kernel.get_parameter(entity, ACTIVE_SECS_PARAM).await {
        Ok(Some(Value::Number(n))) => n.to_int().filter(|&v| v > 0).unwrap_or(DEFAULT_ACTIVE_SECS),
        _ => DEFAULT_ACTIVE_SECS,
    }
}
