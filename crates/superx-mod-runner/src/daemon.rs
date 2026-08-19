//! The runner's daemon loop (epic #189 S3, phase R3): tick, find due
//! schedules, fire them. Lives in the OS daemon beside capture;
//! spawned once at startup, honors live disable per tick, and picks
//! up mid-life provisioning (each tick re-tries `module_db`).

use std::sync::atomic::{AtomicBool, Ordering};

use chrono::Utc;
use superx_kernel::capture::module_enabled;
use superx_kernel::{Kernel, NodeKind};

use crate::exec::{fire, CliExchange, FiringConfig};
use crate::{plan, schedule, MODULE_NAME};

/// Executor command parameter — NO fallback by design (epic S2): an
/// unset command makes dispatch fail loudly; nothing spawns agents
/// the operator never configured.
pub const AGENT_CMD_PARAM: &str = "attr_runner_agent_cmd";
/// Concurrent dispatch ceiling per firing.
pub const MAX_PARALLEL_PARAM: &str = "attr_runner_max_parallel";
const DEFAULT_MAX_PARALLEL: usize = 2; // skill-allow: §9-const — bootstrap fallback, param-overridable (attr_runner_max_parallel)
/// Due-schedule poll cadence.
pub const TICK_PARAM: &str = "attr_runner_tick_secs";
const DEFAULT_TICK_SECS: u64 = 5; // skill-allow: §9-const — bootstrap fallback, param-overridable (attr_runner_tick_secs)

static LOOP_STARTED: AtomicBool = AtomicBool::new(false);

/// Spawn the loop exactly once per process (startup re-runs on live
/// enable; the guard keeps it single).
pub fn spawn_once(kernel: Kernel) {
    if LOOP_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::spawn(run_loop(kernel));
}

async fn run_loop(kernel: Kernel) {
    tracing::info!(target: "runner", "scheduler loop up");
    loop {
        let tick = resolved_u64(&kernel, TICK_PARAM, DEFAULT_TICK_SECS).await;
        tokio::time::sleep(std::time::Duration::from_secs(tick.max(1))).await;

        match module_enabled(&kernel, NodeKind::KernelModule, MODULE_NAME).await {
            Ok(true) => {}
            _ => continue, // disabled (or unreadable): idle, stay alive
        }
        let Ok(db) = kernel.module_db(MODULE_NAME).await else {
            continue; // unprovisioned: idle until the operator provisions
        };

        let due: Vec<schedule::ScheduleRow> = match schedule::current_schedules(&db).await {
            Ok(rows) => rows
                .into_iter()
                .filter(|r| r.status == "scheduled" && is_due(&r.run_at))
                .collect(),
            Err(e) => {
                tracing::warn!(target: "runner", "due scan failed: {e}");
                continue;
            }
        };

        for row in due {
            let config = FiringConfig {
                agent_cmd: resolved_string(&kernel, AGENT_CMD_PARAM).await,
                max_parallel: resolved_u64(&kernel, MAX_PARALLEL_PARAM, DEFAULT_MAX_PARALLEL as u64)
                    .await
                    .max(1) as usize,
                plan_depth: plan::resolved_plan_depth(&kernel).await,
            };
            let exchange = CliExchange { kernel: kernel.clone() };
            match fire(&kernel, &db, &exchange, &config, &row).await {
                Ok(report) => {
                    tracing::info!(
                        target: "runner",
                        schedule = row.uid, firing = report.firing,
                        done = report.done, failed = report.failed,
                        cancelled = report.cancelled, "firing complete"
                    );
                    crate::recurrence::re_enqueue(&kernel, &db, &row).await;
                }
                Err(e) => {
                    tracing::warn!(target: "runner", schedule = row.uid, "firing errored: {e}");
                }
            }
        }
    }
}

fn is_due(run_at: &str) -> bool {
    chrono::DateTime::parse_from_rfc3339(run_at)
        .map(|t| t.with_timezone(&Utc) <= Utc::now())
        .unwrap_or(false)
}

async fn resolved_u64(kernel: &Kernel, param: &str, fallback: u64) -> u64 {
    let Ok(Some(entity)) = kernel
        .find_module_by_name(NodeKind::KernelModule, MODULE_NAME)
        .await
    else {
        return fallback;
    };
    match kernel.get_parameter(entity, param).await {
        Ok(Some(superx_kernel::types::Value::Number(n))) => n
            .to_int()
            .and_then(|i| u64::try_from(i).ok())
            .unwrap_or(fallback),
        _ => fallback,
    }
}

async fn resolved_string(kernel: &Kernel, param: &str) -> Option<String> {
    let entity = kernel
        .find_module_by_name(NodeKind::KernelModule, MODULE_NAME)
        .await
        .ok()
        .flatten()?;
    match kernel.get_parameter(entity, param).await {
        Ok(Some(superx_kernel::types::Value::String(s))) if !s.trim().is_empty() => Some(s),
        _ => None,
    }
}
