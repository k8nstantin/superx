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
  plan <entity-fragment>       DRY RUN: the execution waves the graph implies\n\
  runs [<schedule-fragment>]   firing history: task · status · versions\n\
  config [agent_cmd <cmd…> | max_parallel <n> | tick_secs <n> | plan_depth <n>]\n\
set attr_runner_agent_cmd (substrate parameter) to enable dispatch — unset refuses loudly";

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
        Some("plan") => plan_cmd(kernel, &args[1..]).await,
        Some("runs") => runs_cmd(kernel, &args[1..]).await,
        Some("config") => config_cmd(kernel, &args[1..]).await,
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

async fn plan_cmd(kernel: &Kernel, args: &[String]) -> Result<String> {
    let fragment = args.first().ok_or_else(usage)?;
    let depth = crate::plan::resolved_plan_depth(kernel).await;
    let graph = crate::plan::fetch_graph(kernel, fragment, depth).await?;
    let plan = crate::plan::compute_waves(&graph)?;
    Ok(crate::plan::render_plan(&plan))
}

async fn config_cmd(kernel: &Kernel, args: &[String]) -> Result<String> {
    use superx_kernel::types::Value;
    let entity = kernel
        .find_module_by_name(NodeKind::KernelModule, MODULE_NAME)
        .await?
        .ok_or_else(|| KernelError::Module("runner not registered — boot the OS once first".to_string()))?;
    match args.first().map(String::as_str) {
        None => {
            let cmd = crate::daemon::resolved_agent_cmd(kernel).await;
            Ok(format!(
                "runner config (substrate parameters):\n  agent_cmd:    {}\n  max_parallel: {}\n  tick_secs:    {}\n  plan_depth:   {}\n",
                cmd.unwrap_or_else(|| "(unset — dispatch refuses; set it to enable execution)".to_string()),
                crate::daemon::resolved_max_parallel(kernel).await,
                crate::daemon::resolved_tick_secs(kernel).await,
                crate::plan::resolved_plan_depth(kernel).await,
            ))
        }
        Some("agent_cmd") => {
            let value = args[1..].join(" ");
            if value.is_empty() {
                return Err(usage());
            }
            kernel
                .set_parameter(entity, crate::daemon::AGENT_CMD_PARAM, Value::String(value.clone()))
                .await?;
            Ok(format!("agent_cmd = {value}\n"))
        }
        Some(knob @ ("max_parallel" | "tick_secs" | "plan_depth")) => {
            let n: i64 = args
                .get(1)
                .and_then(|v| v.parse().ok())
                .filter(|&n| n > 0)
                .ok_or_else(usage)?;
            let param = match knob {
                "max_parallel" => crate::daemon::MAX_PARALLEL_PARAM,
                "tick_secs" => crate::daemon::TICK_PARAM,
                _ => crate::plan::PLAN_DEPTH_PARAM,
            };
            kernel.set_parameter(entity, param, Value::Number(n.into())).await?;
            Ok(format!("{knob} = {n}\n"))
        }
        _ => Err(usage()),
    }
}

async fn runs_cmd(kernel: &Kernel, args: &[String]) -> Result<String> {
    let db = kernel.module_db(MODULE_NAME).await?;
    let schedule_uid = match args.first() {
        Some(fragment) => Some(crate::schedule::resolve_schedule(&db, fragment).await?),
        None => None,
    };
    let rows = crate::run::current_runs(&db, schedule_uid.as_deref()).await?;
    if rows.is_empty() {
        return Ok("no runs yet\n".to_string());
    }
    let mut out = format!("runs ({}):\n", rows.len());
    for r in rows {
        out.push_str(&format!(
            "  {}  firing {}  task {}  {}{}{}\n",
            r.valid_from,
            &r.firing[..r.firing.len().min(13)],
            &r.task[..r.task.len().min(13)],
            r.status,
            r.output_ref.map(|o| format!("  → {o}")).unwrap_or_default(),
            r.detail.map(|d| format!("  ({d})")).unwrap_or_default(),
        ));
    }
    Ok(out)
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
