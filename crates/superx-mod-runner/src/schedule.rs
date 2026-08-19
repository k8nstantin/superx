//! Schedule-chain verbs (epic #189 R1, spec #191). Append-only SCD-2:
//! create/cancel are INSERTS; current = latest row per chain uid,
//! compared as PARSED datetimes (the #179 lesson).

use chrono::{DateTime, Duration, Utc};
use superx_kernel::types::{RecordId, Value};
use superx_kernel::{Db, KernelError, Result};

/// One schedule-chain row, as read back.
#[derive(Clone)]
pub struct ScheduleRow {
    pub uid: String,
    pub entity: String,
    pub run_at: String,
    pub recurrence: String,
    pub status: String,
    pub valid_from: String,
}

/// Mint a uuid7 record id (time-ordered — the historical log).
#[must_use]
pub fn new_id(table: &'static str) -> RecordId {
    RecordId::new(table, superx_kernel::types::Uuid::from(uuid::Uuid::now_v7()))
}

/// Parse `<n><s|m|h|d>` — whole non-negative numbers only (spec
/// #191). Returns the duration AND the normalized token.
///
/// # Errors
///
/// [`KernelError::Module`] naming the exact grammar.
pub fn parse_duration(raw: &str) -> Result<(Duration, String)> {
    let err = || {
        KernelError::Module(format!(
            "'{raw}' is not a duration — use <n><s|m|h|d>, e.g. 30s, 5m, 2h, 1d"
        ))
    };
    if raw.len() < 2 {
        return Err(err());
    }
    let (digits, unit) = raw.split_at(raw.len() - 1);
    let n: i64 = digits.parse().map_err(|_| err())?;
    if n < 0 || !digits.chars().all(|c| c.is_ascii_digit()) {
        return Err(err());
    }
    let duration = match unit {
        "s" => Duration::seconds(n),
        "m" => Duration::minutes(n),
        "h" => Duration::hours(n),
        "d" => Duration::days(n),
        _ => return Err(err()),
    };
    Ok((duration, format!("{n}{unit}")))
}

/// Create a schedule chain: "at run_at, kick entity". Returns the
/// chain uid.
///
/// # Errors
///
/// [`KernelError::Db`] for engine errors.
pub async fn create_schedule(
    db: &Db,
    entity_uuid: &str,
    run_at: DateTime<Utc>,
    recurrence: &str,
) -> Result<String> {
    let uid = uuid::Uuid::now_v7().to_string();
    insert_row(db, &uid, entity_uuid, &run_at.to_rfc3339(), recurrence, "scheduled").await?;
    Ok(uid)
}

/// Cancel a chain: append a `cancelled` row carrying the intent
/// fields forward. Refuses a chain that is already cancelled.
///
/// # Errors
///
/// [`KernelError::Module`] on double-cancel; [`KernelError::Db`] for
/// engine errors.
pub async fn cancel_schedule(db: &Db, uid: &str) -> Result<()> {
    let current = chain_current(db, uid).await?.ok_or_else(|| {
        KernelError::Module(format!("no schedule chain '{uid}'"))
    })?;
    if current.status == "cancelled" {
        return Err(KernelError::Module(format!(
            "schedule {uid} is already cancelled"
        )));
    }
    append_status(db, &current, "cancelled").await
}

async fn insert_row(
    db: &Db,
    uid: &str,
    entity: &str,
    run_at_rfc3339: &str,
    recurrence: &str,
    status: &str,
) -> Result<()> {
    db.query(
        "CREATE $id SET uid = $uid, entity = $entity, \
         run_at = type::datetime($run_at), recurrence = $recurrence, \
         status = $status, valid_from = time::now()",
    )
    .bind(("id", new_id("schedule")))
    .bind(("uid", uid.to_string()))
    .bind(("entity", entity.to_string()))
    .bind(("run_at", run_at_rfc3339.to_string()))
    .bind(("recurrence", recurrence.to_string()))
    .bind(("status", status.to_string()))
    .await?
    .check()?;
    Ok(())
}

/// Append a status row on an existing chain, carrying the intent
/// fields forward (fired/cancelled transitions).
///
/// # Errors
///
/// [`KernelError::Db`] for engine errors.
pub async fn append_status(db: &Db, current: &ScheduleRow, status: &str) -> Result<()> {
    insert_row(db, &current.uid, &current.entity, &current.run_at, &current.recurrence, status).await
}

/// Chain-current rows across ALL schedules (latest per uid), soonest
/// `run_at` first.
///
/// # Errors
///
/// [`KernelError::Db`] for engine errors.
pub async fn current_schedules(db: &Db) -> Result<Vec<ScheduleRow>> {
    let mut resp = db
        .query(
            "SELECT uid, entity, run_at, recurrence, status, valid_from \
             FROM schedule ORDER BY valid_from DESC",
        )
        .await?;
    let rows: Vec<Value> = resp.take(0)?;
    let mut latest: std::collections::HashMap<String, ScheduleRow> =
        std::collections::HashMap::new();
    for row in rows.iter().filter_map(parse_row) {
        // Rows arrive newest-first; first seen per uid IS the current.
        latest.entry(row.uid.clone()).or_insert(row);
    }
    let mut out: Vec<ScheduleRow> = latest.into_values().collect();
    out.sort_by(|a, b| a.run_at.cmp(&b.run_at).then(a.uid.cmp(&b.uid)));
    Ok(out)
}

/// The current row of one chain, if the chain exists.
///
/// # Errors
///
/// [`KernelError::Db`] for engine errors.
pub async fn chain_current(db: &Db, uid: &str) -> Result<Option<ScheduleRow>> {
    let mut resp = db
        .query(
            "SELECT uid, entity, run_at, recurrence, status, valid_from \
             FROM schedule WHERE uid = $uid ORDER BY valid_from DESC LIMIT 1",
        )
        .bind(("uid", uid.to_string()))
        .await?;
    let rows: Vec<Value> = resp.take(0)?;
    Ok(rows.first().and_then(parse_row))
}

/// A chain's full history, oldest first.
///
/// # Errors
///
/// [`KernelError::Db`] for engine errors.
pub async fn schedule_history(db: &Db, uid: &str) -> Result<Vec<ScheduleRow>> {
    let mut resp = db
        .query(
            "SELECT uid, entity, run_at, recurrence, status, valid_from \
             FROM schedule WHERE uid = $uid ORDER BY valid_from ASC",
        )
        .bind(("uid", uid.to_string()))
        .await?;
    let rows: Vec<Value> = resp.take(0)?;
    Ok(rows.iter().filter_map(parse_row).collect())
}

/// Resolve a uuid7 fragment to exactly one schedule chain.
///
/// # Errors
///
/// [`KernelError::Module`] for zero or multiple matches.
pub async fn resolve_schedule(db: &Db, fragment: &str) -> Result<String> {
    let all = current_schedules(db).await?;
    let hits: Vec<&ScheduleRow> = all.iter().filter(|r| r.uid.contains(fragment)).collect();
    match hits.len() {
        1 => Ok(hits[0].uid.clone()),
        0 => Err(KernelError::Module(format!(
            "no schedule matches '{fragment}' — see `superx runner queue`"
        ))),
        n => Err(KernelError::Module(format!(
            "'{fragment}' is ambiguous ({n} schedules match) — give more of the uuid"
        ))),
    }
}

fn parse_row(row: &Value) -> Option<ScheduleRow> {
    let Value::Object(o) = row else { return None };
    let s = |k: &str| match o.get(k) {
        Some(Value::String(v)) => Some(v.clone()),
        _ => None,
    };
    let dt = |k: &str| match o.get(k) {
        Some(Value::Datetime(d)) => Some(d.to_string()),
        _ => None,
    };
    Some(ScheduleRow {
        uid: s("uid")?,
        entity: s("entity")?,
        run_at: dt("run_at")?,
        recurrence: s("recurrence")?,
        status: s("status")?,
        valid_from: dt("valid_from")?,
    })
}
