//! `superx runner …` — the scheduler's CLI (epic #189 S4, phase R1
//! subset: schedule/queue/cancel). Entity fragments resolve THROUGH
//! the entities module's CLI via the kernel dispatch (D24).

use chrono::Utc;
use superx_kernel::{Kernel, KernelError, NodeKind, Result};

use crate::{schedule, MODULE_NAME};

const USAGE: &str = "usage: superx runner <command>\n\
  schedule <entity-fragment> [--at <rfc3339> | --in <n><s|m|h|d>] [--every <n><s|m|h|d>]\n\
  queue                        chain-current schedules, soonest first\n\
  cancel <schedule-fragment>\n\
(plan lands in R2 #192; the executor loop in R3 #193; recurrence firing in R4 #194)";

/// Route a `superx runner …` invocation.
///
/// # Errors
///
/// [`KernelError::Module`] with usage on unknown commands; verb
/// errors pass through.
pub async fn dispatch(kernel: &Kernel, args: &[String]) -> Result<String> {
    match args.first().map(String::as_str) {
        Some("schedule") => schedule_cmd(kernel, &args[1..]).await,
        Some("queue") => queue_cmd(kernel).await,
        Some("cancel") => cancel_cmd(kernel, &args[1..]).await,
        _ => Err(KernelError::Module(USAGE.to_string())),
    }
}

async fn schedule_cmd(kernel: &Kernel, args: &[String]) -> Result<String> {
    let db = kernel.module_db(MODULE_NAME).await?;
    let fragment = args.first().ok_or_else(usage)?;
    let mut run_at = Utc::now();
    let mut recurrence = "none".to_string();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--at" => {
                let raw = args.get(i + 1).ok_or_else(usage)?;
                run_at = chrono::DateTime::parse_from_rfc3339(raw)
                    .map_err(|e| {
                        KernelError::Module(format!("'{raw}' is not RFC3339: {e}"))
                    })?
                    .with_timezone(&Utc);
                i += 2;
            }
            "--in" => {
                let raw = args.get(i + 1).ok_or_else(usage)?;
                let (duration, _) = schedule::parse_duration(raw)?;
                run_at = Utc::now() + duration;
                i += 2;
            }
            "--every" => {
                let raw = args.get(i + 1).ok_or_else(usage)?;
                let (_, normalized) = schedule::parse_duration(raw)?;
                recurrence = format!("every:{normalized}");
                i += 2;
            }
            _ => return Err(usage()),
        }
    }

    // The target lives in the entities module — resolve through its
    // CLI via the kernel dispatch (D24); its errors surface verbatim.
    let (entity_uuid, entity_name) = resolve_entity(kernel, fragment).await?;
    let uid = schedule::create_schedule(&db, &entity_uuid, run_at, &recurrence).await?;
    emit(
        kernel,
        "schedule_created",
        serde_json::json!({
            "schedule": uid,
            "entity": entity_uuid,
            "entity_name": entity_name,
            "run_at": run_at.to_rfc3339(),
            "recurrence": recurrence,
        }),
    )
    .await;
    Ok(format!("{uid}\n"))
}

async fn queue_cmd(kernel: &Kernel) -> Result<String> {
    let db = kernel.module_db(MODULE_NAME).await?;
    let rows: Vec<schedule::ScheduleRow> = schedule::current_schedules(&db)
        .await?
        .into_iter()
        .filter(|r| r.status == "scheduled")
        .collect();
    if rows.is_empty() {
        return Ok("queue is empty — `superx runner schedule <entity>`\n".to_string());
    }
    let mut out = format!("queue ({}):\n", rows.len());
    for row in rows {
        let name = resolve_entity(kernel, &row.entity)
            .await
            .map(|(_, name)| name)
            .unwrap_or_else(|_| format!("{} (unresolved)", row.entity));
        out.push_str(&format!(
            "  {}  {}  at {}  {}\n",
            row.uid, name, row.run_at, row.recurrence
        ));
    }
    Ok(out)
}

async fn cancel_cmd(kernel: &Kernel, args: &[String]) -> Result<String> {
    let db = kernel.module_db(MODULE_NAME).await?;
    let fragment = args.first().ok_or_else(usage)?;
    let uid = schedule::resolve_schedule(&db, fragment).await?;
    schedule::cancel_schedule(&db, &uid).await?;
    emit(kernel, "schedule_cancelled", serde_json::json!({ "schedule": uid })).await;
    Ok(format!("schedule {uid} cancelled (chain history kept)\n"))
}

/// Resolve an entity fragment to (uuid, name) through the entities
/// module's `show` output: line 1 is
/// `entity <uuid> · type <t> · created <ts>`.
async fn resolve_entity(kernel: &Kernel, fragment: &str) -> Result<(String, String)> {
    let shown = superx_ops::run_module_cli(
        kernel,
        &["entities".to_string(), "show".to_string(), fragment.to_string()],
    )
    .await
    .map_err(KernelError::Module)?;
    let first = shown.lines().next().unwrap_or_default();
    let uuid = first
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| KernelError::Module(format!("unparseable entities reply: {first}")))?
        .to_string();
    // The one-line label: `name: <label>` on the state block.
    let name = shown
        .lines()
        .find_map(|l| l.trim_start().strip_prefix("name: "))
        .unwrap_or("(unnamed)")
        .to_string();
    Ok((uuid, name))
}

/// Telemetry into the one firehose, attributed to this module; a
/// telemetry failure never fails the verb that already committed.
async fn emit(kernel: &Kernel, event: &str, payload: serde_json::Value) {
    let subject = kernel
        .find_module_by_name(NodeKind::KernelModule, MODULE_NAME)
        .await
        .ok()
        .flatten();
    if let Err(e) = kernel
        .log_telemetry(event, superx_kernel::message::value_from_json(&payload), subject)
        .await
    {
        tracing::warn!(target: "runner", "telemetry write failed: {e}");
    }
}

fn usage() -> KernelError {
    KernelError::Module(USAGE.to_string())
}
