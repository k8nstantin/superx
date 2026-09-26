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

use crate::api::{
    AgentSplit, HeatCell, InsightsSummary, ModuleHealth, NameCount, TimeCount, TokenTotals, TableStat,
};
use crate::stats::{OUT_TOKENS_SQL, REPLY_KEY_SQL};

/// The "is capture alive?" window.
const RECENT_SECS: i64 = 3600; // skill-allow: §9-const — display window for the capture-lag tile

/// Newest module lifecycle events scanned for per-module health (#367).
/// A working instance emits a few hundred a day; this covers days.
const HEALTH_SCAN: u32 = 3000; // skill-allow: §9-const — aggregation page bound

/// What the module-health lamp calls recent.
const HEALTH_RECENT_SECS: i64 = 86_400; // skill-allow: §9-const — display window for the module-health lamp

/// The lifecycle events a module emits, newest first, with the error
/// text a failure carries. `payload.error` is NONE on every other
/// event, and NONE is simply not a string.
const HEALTH_QUERY: &str = "SELECT payload.name AS name, lifecycle_event AS event,
        payload.error AS error, payload.startup_duration_ms AS startup_ms, valid_from
     FROM telemetry_stream
     WHERE lifecycle_event IN ['module_starting', 'module_started', 'module_active',
        'module_stopped', 'module_failed', 'module_start_failed', 'module_start_abandoned',
        'module_disabled', 'module_provisioned']
     ORDER BY valid_from DESC LIMIT $limit";

fn is_failure(event: &str) -> bool {
    matches!(event, "module_failed" | "module_start_failed" | "module_start_abandoned")
}

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

/// Everything the Status page's deep panels need, in one pass.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
/// Rows sampled per table to measure an average row (#398).
const SIZE_SAMPLE: usize = 64; // skill-allow: §9-const — read-path bound, not a policy tunable

/// What the substrate holds, table by table (#398).
///
/// SurrealDB reports no storage size and the kernel does not hand a
/// module the datastore path, so this measures the DATA instead of the
/// files: every table is counted, a sample of its rows is serialised,
/// and the average row is multiplied by the count. RocksDB compresses
/// what it writes, so the on-disk figure will be smaller — this is
/// what the rows weigh, which is the question a row count raises.
async fn table_stats(kernel: &Kernel) -> Result<Vec<TableStat>> {
    // The table list comes from the engine, so a module's own tables
    // are counted without this knowing their names.
    let info: Vec<Value> = kernel.db().query("INFO FOR DB").await?.take(0)?;
    let mut names: Vec<String> = info
        .first()
        .and_then(obj)
        .and_then(|o| o.get("tables"))
        .and_then(obj)
        .map(|t| t.keys().cloned().collect())
        .unwrap_or_default();
    names.sort_unstable();

    let mut out = Vec::new();
    for name in names {
        // The name comes from the engine's own catalogue, never from a
        // request, so it cannot carry anything but a table name.
        let count: Vec<Value> = kernel
            .db()
            .query(format!("SELECT count() AS c FROM {name} GROUP ALL"))
            .await?
            .take(0)?;
        let rows = count
            .first()
            .and_then(obj)
            .map(|o| get_int(o, "c"))
            .unwrap_or(0);
        if rows == 0 {
            out.push(TableStat { name, rows: 0, bytes_est: 0, avg_row_bytes: 0, sampled: 0 });
            continue;
        }
        let sample: Vec<Value> = kernel
            .db()
            .query(format!("SELECT * FROM {name} LIMIT {SIZE_SAMPLE}"))
            .await?
            .take(0)?;
        let measured: Vec<usize> = sample
            .iter()
            .filter_map(|r| serde_json::to_string(r).ok().map(|s| s.len()))
            .collect();
        let avg = if measured.is_empty() {
            0
        } else {
            (measured.iter().sum::<usize>() / measured.len()) as i64
        };
        out.push(TableStat {
            name,
            rows,
            bytes_est: avg.saturating_mul(rows),
            avg_row_bytes: avg,
            sampled: measured.len() as i64,
        });
    }
    out.sort_by_key(|t| std::cmp::Reverse(t.bytes_est));
    Ok(out)
}

pub async fn insights_summary(kernel: &Kernel) -> Result<InsightsSummary> {
    insights_summary_on(kernel, chrono::Offset::fix(&chrono::Utc)).await
}

/// [`insights_summary`] with its days and hours on the viewer's `clock`
/// (#415 review), as the Status page's own charts are.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn insights_summary_on(kernel: &Kernel, clock: chrono::FixedOffset) -> Result<InsightsSummary> {
    // ── one pass over the messages ──────────────────────────────────
    // The calendar, the week's rhythm, the token totals, the models and the
    // agents all count REPLIES (#409, #415 review), and each used to re-read
    // the whole message table to fold them: six passes, ten of the fourteen
    // seconds this took on a 37k-row replay, polled every minute — and as
    // capture landed rows between them, two token totals of one page
    // disagreed. One pass folds the rows into replies; the rest read that.
    //
    // A reply's lines share its key, its agent and its model, so folding on
    // all three is folding on the key. Claude Code repeats a reply's usage
    // on every line, and Gemini re-emits a record as it streams, so each
    // counter keeps its reply's largest value. Gemini's `input` includes
    // what it read from cache, and its `output` excludes its `thoughts` —
    // both adjusted to read like Claude's. A row with no usage folds to
    // zeros and still counts as a message.
    //
    // By the AGENT'S clock, not ours: `emitted_at` is the source's own
    // timestamp, `valid_from` merely when capture first saw the row, and
    // bucketing on the latter draws the ingest run (#239) — shifted by the
    // viewer's offset from UTC, so the days and hours are the viewer's.
    let shift = clock.local_minus_utc();
    let at = if shift >= 0 { format!("(at + {shift}s)") } else { format!("(at - {}s)", -shift) };
    let mut res = kernel
        .db()
        .query(format!(
            "LET $r = (SELECT agent, (raw.message.model ?? raw.model) AS model, {REPLY_KEY_SQL} AS k,
                    time::min(emitted_at ?? valid_from) AS at,
                    math::max(raw.message.usage.input_tokens
                        ?? ((raw.tokens.input ?? 0) - (raw.tokens.cached ?? 0))) AS input,
                    math::max({OUT_TOKENS_SQL} ?? 0) AS output,
                    math::max(raw.message.usage.cache_read_input_tokens ?? raw.tokens.cached ?? 0) AS cache_read,
                    math::max(raw.message.usage.cache_creation_input_tokens ?? 0) AS cache_write
                FROM message GROUP BY agent, model, k);
             SELECT time::format({at}, '%Y-%m-%d') AS t, count() AS value FROM $r GROUP BY t ORDER BY t;
             SELECT time::hour({at}) AS hour, time::wday({at}) AS weekday, count() AS value
                FROM $r GROUP BY hour, weekday;
             SELECT math::sum(input) AS input, math::sum(output) AS output,
                    math::sum(cache_read) AS cache_read, math::sum(cache_write) AS cache_write
                FROM $r GROUP ALL;
             SELECT model, count() AS value FROM $r WHERE model != NONE GROUP BY model;
             SELECT agent, count() AS messages, math::sum(output) AS output FROM $r GROUP BY agent;"
        ))
        .await?;
    let days: Vec<Value> = res.take(1)?;
    let cells: Vec<Value> = res.take(2)?;
    let totals: Vec<Value> = res.take(3)?;
    let by_model: Vec<Value> = res.take(4)?;
    let by_agent: Vec<Value> = res.take(5)?;

    // ── the work calendar ───────────────────────────────────────────
    let events_per_day = days
        .iter()
        .filter_map(obj)
        .filter_map(|o| {
            Some(TimeCount {
                t: get_str(o, "t")?.to_string(),
                value: get_int(o, "value"),
            })
        })
        .collect();

    // ── the week's rhythm: hour × weekday, same clock as above ──────
    let hour_weekday = cells
        .iter()
        .filter_map(obj)
        .map(|o| HeatCell {
            hour: get_int(o, "hour"),
            weekday: get_int(o, "weekday"),
            value: get_int(o, "value"),
        })
        .collect();

    // ── token economics ─────────────────────────────────────────────
    let t = totals.first().and_then(obj);
    let tokens = TokenTotals {
        input: t.map_or(0, |o| get_int(o, "input")),
        output: t.map_or(0, |o| get_int(o, "output")),
        cache_read: t.map_or(0, |o| get_int(o, "cache_read")),
        cache_write: t.map_or(0, |o| get_int(o, "cache_write")),
    };

    // ── which models did the work ───────────────────────────────────
    // Replies, not lines (#409): a model that thinks and calls tools in
    // one reply writes more lines per reply, so counting lines tilted the
    // split toward it. `<synthetic>` is the runtime's own marker, not a
    // model (#367).
    let mut models: Vec<NameCount> = by_model
        .iter()
        .filter_map(obj)
        .filter_map(|o| {
            Some(NameCount {
                name: get_str(o, "model").filter(|m| !m.starts_with('<'))?.to_string(),
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
    let agent_of = |o: &Object| match o.get("agent") {
        Some(Value::RecordId(r)) => superx_ops::record_uuid(r),
        _ => String::new(),
    };
    // Replies, once each, as every "messages" on the page (#415 review):
    // counted in rows, Claude's three lines a reply out-weighed Gemini's
    // one record in "who did the work".
    let mut per_agent: Vec<AgentSplit> = by_agent
        .iter()
        .filter_map(obj)
        .map(|o| AgentSplit {
            name: agent_name.get(&agent_of(o)).cloned().unwrap_or_else(|| "unattributed".into()),
            messages: get_int(o, "messages"),
            output_tokens: get_int(o, "output"),
        })
        .collect();
    per_agent.sort_by_key(|a| std::cmp::Reverse(a.messages));

    // ── the telemetry stream, in two passes (#415 review) ───────────
    // Six queries each read the whole stream: 4.4 of the 9 seconds this
    // took on a replay. One grouped pass says what capture spends itself
    // on, whether it is alive, how busy the last hour was and how often each
    // module ever failed; one bounded scan of module lifecycle events says
    // how each module is and what it cost to start.
    let now = chrono::Utc::now();
    let cutoff = now - chrono::Duration::seconds(RECENT_SECS);
    let groups: Vec<Value> = kernel
        .db()
        .query(
            // `count(cond)` counts the rows where `cond` holds.
            "SELECT lifecycle_event AS kind, payload.name AS name, count() AS n,
                    time::max(valid_from) AS newest, count(valid_from > $cutoff) AS recent
             FROM telemetry_stream GROUP BY kind, name",
        )
        .bind(("cutoff", cutoff))
        .await?
        .take(0)?;
    let mut kinds: HashMap<String, i64> = HashMap::new();
    let mut failures_total: HashMap<String, i64> = HashMap::new();
    let mut last_event_at_dt: Option<chrono::DateTime<chrono::Utc>> = None;
    let mut events_last_hour = 0i64;
    for g in groups.iter().filter_map(obj) {
        let Some(kind) = get_str(g, "kind") else { continue };
        let n = get_int(g, "n");
        *kinds.entry(kind.to_string()).or_insert(0) += n;
        events_last_hour += get_int(g, "recent");
        if let Some(Value::Datetime(d)) = g.get("newest") {
            last_event_at_dt = Some(last_event_at_dt.map_or(**d, |t| t.max(**d)));
        }
        if is_failure(kind) {
            if let Some(name) = get_str(g, "name") {
                *failures_total.entry(name.to_string()).or_insert(0) += n;
            }
        }
    }

    // ── what capture actually spends itself on ──────────────────────
    let mut event_kinds: Vec<NameCount> = kinds.into_iter().map(|(name, value)| NameCount { name, value }).collect();
    event_kinds.sort_by(|a, b| b.value.cmp(&a.value).then(a.name.cmp(&b.name)));

    // ── is capture alive? ───────────────────────────────────────────
    let last_event_secs = last_event_at_dt.map(|d| (now - d).num_seconds().max(0));
    let last_event_at = last_event_at_dt.map(|d| d.to_rfc3339());

    // ── module health and startup cost: what happened to each (#367) ─
    let recent_cut = now - chrono::Duration::seconds(HEALTH_RECENT_SECS);
    let mut health: Vec<ModuleHealth> = Vec::new();
    let mut module_startup: Vec<NameCount> = Vec::new();
    let lifecycle: Vec<Value> = kernel
        .db()
        .query(HEALTH_QUERY)
        .bind(("limit", HEALTH_SCAN))
        .await?
        .take(0)?;
    for row in lifecycle.iter().filter_map(obj) {
        let (Some(name), Some(event)) = (get_str(row, "name"), get_str(row, "event")) else {
            continue;
        };
        let at = match row.get("valid_from") {
            Some(Value::Datetime(d)) => **d,
            _ => continue,
        };
        // Newest-first: a module's first `module_active` is its latest
        // boot's startup cost.
        if event == "module_active" && !module_startup.iter().any(|s| s.name == name) {
            module_startup.push(NameCount { name: name.to_string(), value: get_int(row, "startup_ms") });
        }
        let idx = match health.iter().position(|h| h.name == name) {
            Some(i) => i,
            None => {
                // Newest-first: the first sighting is the newest event.
                health.push(ModuleHealth {
                    name: name.to_string(),
                    last_event: event.to_string(),
                    last_event_secs: (now - at).num_seconds().max(0),
                    last_event_at: Some(at.to_rfc3339()),
                    failures_recent: 0,
                    failures_total: 0,
                    last_error: None,
                    last_error_at: None,
                });
                health.len() - 1
            }
        };
        if is_failure(event) {
            if at > recent_cut {
                health[idx].failures_recent += 1;
            }
            if health[idx].last_error.is_none() {
                if let Some(error) = get_str(row, "error") {
                    health[idx].last_error = Some(error.to_string());
                    health[idx].last_error_at = Some(at.to_rfc3339());
                }
            }
        }
    }
    module_startup.sort_by_key(|s| std::cmp::Reverse(s.value));
    for (name, total) in failures_total {
        match health.iter_mut().find(|h| h.name == name) {
            Some(h) => h.failures_total = total,
            // Failed beyond the scan and never seen since: still a
            // module with a history worth showing.
            None => health.push(ModuleHealth {
                name,
                last_event: String::new(),
                last_event_secs: -1,
                last_event_at: None,
                failures_recent: 0,
                failures_total: total,
                last_error: None,
                last_error_at: None,
            }),
        }
    }
    health.sort_by(|a, b| {
        b.failures_recent
            .cmp(&a.failures_recent)
            .then(b.failures_total.cmp(&a.failures_total))
            .then(a.name.cmp(&b.name))
    });
    let module_health = health;

    // What the substrate holds (#398).
    let tables = table_stats(kernel).await?;
    let db_rows_total = tables.iter().map(|t| t.rows).sum();
    let db_bytes_est = tables.iter().map(|t| t.bytes_est).sum();

    Ok(InsightsSummary {
        events_per_day,
        hour_weekday,
        tokens,
        models,
        per_agent,
        event_kinds,
        module_startup,
        last_event_secs,
        last_event_at,
        events_last_hour,
        module_health,
        tables,
        db_rows_total,
        db_bytes_est,
    })
}
