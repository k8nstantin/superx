//! Run-chain verbs (epic #189 S1/S3, phase R3): one chain per
//! (firing, task); every status transition is an appended row.

use superx_kernel::types::Value;
use superx_kernel::{Db, Result};

use crate::schedule::new_id;

/// One run-chain row, as read back.
#[derive(Clone, Debug)]
pub struct RunRow {
    pub uid: String,
    pub schedule: String,
    pub firing: String,
    pub task: String,
    pub status: String,
    pub instruct_version: Option<String>,
    pub output_ref: Option<String>,
    pub detail: Option<String>,
    pub valid_from: String,
}

/// Append one run-chain row. `uid` stays constant across a chain;
/// status moves dispatched → done|failed|cancelled.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
#[allow(clippy::too_many_arguments)] // skill-allow: allow — the run row IS this wide; a params struct would only rename the width
pub async fn append_run_row(
    db: &Db,
    uid: &str,
    schedule: &str,
    firing: &str,
    task: &str,
    status: &str,
    instruct_version: Option<&str>,
    output_ref: Option<&str>,
    detail: Option<&str>,
) -> Result<()> {
    let mut statement = String::from(
        "CREATE $id SET uid = $uid, schedule = $schedule, firing = $firing, \
         task = $task, status = $status, valid_from = time::now()",
    );
    if instruct_version.is_some() {
        statement.push_str(", instruct_version = $instruct_version");
    }
    if output_ref.is_some() {
        statement.push_str(", output_ref = $output_ref");
    }
    if detail.is_some() {
        statement.push_str(", detail = $detail");
    }
    let mut query = db
        .query(statement)
        .bind(("id", new_id("run")))
        .bind(("uid", uid.to_string()))
        .bind(("schedule", schedule.to_string()))
        .bind(("firing", firing.to_string()))
        .bind(("task", task.to_string()))
        .bind(("status", status.to_string()));
    if let Some(v) = instruct_version {
        query = query.bind(("instruct_version", v.to_string()));
    }
    if let Some(v) = output_ref {
        query = query.bind(("output_ref", v.to_string()));
    }
    if let Some(v) = detail {
        query = query.bind(("detail", v.to_string()));
    }
    query.await?.check()?;
    Ok(())
}

/// Chain-current run rows, newest firing first; optionally filtered
/// to one schedule chain.
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn current_runs(db: &Db, schedule: Option<&str>) -> Result<Vec<RunRow>> {
    let mut resp = if let Some(s) = schedule {
        db.query(
            "SELECT uid, schedule, firing, task, status, instruct_version, \
             output_ref, detail, valid_from FROM run \
             WHERE schedule = $schedule ORDER BY valid_from DESC",
        )
        .bind(("schedule", s.to_string()))
        .await?
    } else {
        db.query(
            "SELECT uid, schedule, firing, task, status, instruct_version, \
             output_ref, detail, valid_from FROM run ORDER BY valid_from DESC",
        )
        .await?
    };
    let rows: Vec<Value> = resp.take(0)?;
    let mut latest: std::collections::HashMap<String, RunRow> = std::collections::HashMap::new();
    for row in rows.iter().filter_map(parse_run) {
        latest.entry(row.uid.clone()).or_insert(row); // newest-first: first seen wins
    }
    let mut out: Vec<RunRow> = latest.into_values().collect();
    out.sort_by(|a, b| b.valid_from.cmp(&a.valid_from).then(a.uid.cmp(&b.uid)));
    Ok(out)
}

/// The done-task uids of one firing (dependency satisfaction, D25).
///
/// # Errors
///
/// [`superx_kernel::KernelError::Db`] for engine errors.
pub async fn done_tasks(db: &Db, firing: &str) -> Result<std::collections::HashSet<String>> {
    let all = current_runs(db, None).await?;
    Ok(all
        .into_iter()
        .filter(|r| r.firing == firing && r.status == "done")
        .map(|r| r.task)
        .collect())
}

fn parse_run(row: &Value) -> Option<RunRow> {
    let Value::Object(o) = row else { return None };
    let s = |k: &str| match o.get(k) {
        Some(Value::String(v)) => Some(v.clone()),
        _ => None,
    };
    Some(RunRow {
        uid: s("uid")?,
        schedule: s("schedule")?,
        firing: s("firing")?,
        task: s("task")?,
        status: s("status")?,
        instruct_version: s("instruct_version"),
        output_ref: s("output_ref"),
        detail: s("detail"),
        valid_from: match o.get("valid_from") {
            Some(Value::Datetime(d)) => d.to_string(),
            _ => String::new(),
        },
    })
}
