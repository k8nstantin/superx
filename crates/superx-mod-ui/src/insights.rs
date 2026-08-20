//! Deep statistics (issue #237) — the data the OS gathers but nothing
//! ever read: the four token counters, model names, the per-agent link
//! on every message, the telemetry kind mix, module startup cost.
//!
//! Everything here aggregates IN THE ENGINE (`GROUP BY`, `math::sum`)
//! rather than walking rows in Rust, so the cost does not grow with
//! history the way [`crate::stats`]'s window walk does. Pure SELECT —
//! readers never mutate the stream they observe.

use std::collections::HashMap;

use superx_kernel::types::{Object, Value};
use superx_kernel::{Kernel, Result};

use crate::api::{AgentSplit, HeatCell, InsightsSummary, NameCount, TimeCount, TokenTotals};

/// Newest `module_active` events scanned for per-module startup cost —
/// one row per module per boot, so this covers many boots.
const STARTUP_SCAN: u32 = 400; // skill-allow: §9-const — aggregation page bound

/// The "is capture alive?" window.
const RECENT_SECS: i64 = 3600; // skill-allow: §9-const — display window for the capture-lag tile

/// Gemini stores `input`/`output`/`cached`; Claude Code stores the four
/// `*_input_tokens` counters. One query covers both — a missing field
/// coalesces to 0 rather than dropping the row.
const TOKENS_QUERY: &str = "SELECT
        math::sum(raw.message.usage.input_tokens ?? raw.tokens.input ?? 0) AS input,
        math::sum(raw.message.usage.output_tokens ?? raw.tokens.output ?? 0) AS output,
        math::sum(raw.message.usage.cache_read_input_tokens ?? raw.tokens.cached ?? 0)
            AS cache_read,
        math::sum(raw.message.usage.cache_creation_input_tokens ?? 0) AS cache_write
     FROM message GROUP ALL";

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

async fn rows(kernel: &Kernel, query: &'static str) -> Result<Vec<Value>> {
    Ok(kernel.db().query(query).await?.take(0)?)
}

/// Everything the Status page's deep panels need, in one pass.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn insights_summary(kernel: &Kernel) -> Result<InsightsSummary> {
    // ── the work calendar: every day the OS captured anything ───────
    let events_per_day = rows(
        kernel,
        "SELECT time::format(valid_from, '%Y-%m-%d') AS t, count() AS value
         FROM telemetry_stream GROUP BY t ORDER BY t",
    )
    .await?
    .iter()
    .filter_map(obj)
    .filter_map(|o| {
        Some(TimeCount {
            t: get_str(o, "t")?.to_string(),
            value: get_int(o, "value"),
        })
    })
    .collect();

    // ── the week's rhythm: hour × weekday ───────────────────────────
    let hour_weekday = rows(
        kernel,
        "SELECT time::hour(valid_from) AS hour, time::wday(valid_from) AS weekday,
                count() AS value
         FROM telemetry_stream GROUP BY hour, weekday",
    )
    .await?
    .iter()
    .filter_map(obj)
    .map(|o| HeatCell {
        hour: get_int(o, "hour"),
        weekday: get_int(o, "weekday"),
        value: get_int(o, "value"),
    })
    .collect();

    // ── token economics ─────────────────────────────────────────────
    let t = rows(kernel, TOKENS_QUERY).await?;
    let t = t.first().and_then(obj);
    let tokens = TokenTotals {
        input: t.map_or(0, |o| get_int(o, "input")),
        output: t.map_or(0, |o| get_int(o, "output")),
        cache_read: t.map_or(0, |o| get_int(o, "cache_read")),
        cache_write: t.map_or(0, |o| get_int(o, "cache_write")),
    };

    // ── which models did the work ───────────────────────────────────
    let mut models: Vec<NameCount> = rows(
        kernel,
        "SELECT raw.message.model ?? raw.model AS model, count() AS value
         FROM message WHERE raw.message.model != NONE OR raw.model != NONE
         GROUP BY model",
    )
    .await?
    .iter()
    .filter_map(obj)
    .filter_map(|o| {
        Some(NameCount {
            name: get_str(o, "model")?.to_string(),
            value: get_int(o, "value"),
        })
    })
    .collect();
    models.sort_by_key(|m| std::cmp::Reverse(m.value));

    // ── per agent: message.agent is indexed and, until now, unread ──
    let mut agent_name: HashMap<String, String> = HashMap::new();
    for a in kernel
        .list_named_entities("node_agent", "attr_agent_descriptor")
        .await?
    {
        let name = match &a.payload {
            Value::Object(o) => get_str(o, "name").unwrap_or("?").to_string(),
            _ => "?".to_string(),
        };
        agent_name.insert(superx_ops::record_uuid(&a.entity_id), name);
    }
    let mut per_agent: Vec<AgentSplit> = rows(
        kernel,
        "SELECT agent, count() AS messages,
                math::sum(raw.message.usage.output_tokens ?? raw.tokens.output ?? 0) AS output
         FROM message GROUP BY agent",
    )
    .await?
    .iter()
    .filter_map(obj)
    .map(|o| {
        let uuid = match o.get("agent") {
            Some(Value::RecordId(r)) => superx_ops::record_uuid(r),
            _ => String::new(),
        };
        AgentSplit {
            name: agent_name.get(&uuid).cloned().unwrap_or_else(|| "unattributed".into()),
            messages: get_int(o, "messages"),
            output_tokens: get_int(o, "output"),
        }
    })
    .collect();
    per_agent.sort_by_key(|a| std::cmp::Reverse(a.messages));

    // ── what capture actually spends itself on ──────────────────────
    let mut event_kinds: Vec<NameCount> = rows(
        kernel,
        "SELECT lifecycle_event AS name, count() AS value
         FROM telemetry_stream GROUP BY name",
    )
    .await?
    .iter()
    .filter_map(obj)
    .filter_map(|o| {
        Some(NameCount {
            name: get_str(o, "name")?.to_string(),
            value: get_int(o, "value"),
        })
    })
    .collect();
    event_kinds.sort_by_key(|k| std::cmp::Reverse(k.value));

    // ── per-module startup cost: newest reading per module ──────────
    let startup: Vec<Value> = kernel
        .db()
        .query(
            // valid_from stays in the projection: the engine requires
            // the ordering idiom to be selected.
            "SELECT payload.name AS name, payload.startup_duration_ms AS value, valid_from
             FROM telemetry_stream WHERE lifecycle_event = 'module_active'
             ORDER BY valid_from DESC LIMIT $limit",
        )
        .bind(("limit", STARTUP_SCAN))
        .await?
        .take(0)?;
    let mut seen: Vec<NameCount> = Vec::new();
    for row in startup.iter().filter_map(obj) {
        let Some(name) = get_str(row, "name") else { continue };
        if seen.iter().any(|s| s.name == name) {
            continue; // newest wins — the rows arrive newest-first
        }
        seen.push(NameCount {
            name: name.to_string(),
            value: get_int(row, "value"),
        });
    }
    seen.sort_by_key(|s| std::cmp::Reverse(s.value));
    let module_startup = seen;

    // ── is capture alive? ───────────────────────────────────────────
    let newest: Vec<Value> = kernel
        .db()
        .query("SELECT valid_from FROM telemetry_stream ORDER BY valid_from DESC LIMIT 1")
        .await?
        .take(0)?;
    let last_event_secs = newest.first().and_then(obj).and_then(|o| match o.get("valid_from") {
        Some(Value::Datetime(d)) => Some((chrono::Utc::now() - **d).num_seconds().max(0)),
        _ => None,
    });
    let cutoff = chrono::Utc::now() - chrono::Duration::seconds(RECENT_SECS);
    let recent: Vec<Value> = kernel
        .db()
        .query("SELECT count() AS c FROM telemetry_stream WHERE valid_from > $cutoff GROUP ALL")
        .bind(("cutoff", cutoff))
        .await?
        .take(0)?;
    let events_last_hour = recent.first().and_then(obj).map_or(0, |o| get_int(o, "c"));

    Ok(InsightsSummary {
        events_per_day,
        hour_weekday,
        tokens,
        models,
        per_agent,
        event_kinds,
        module_startup,
        last_event_secs,
        events_last_hour,
    })
}
