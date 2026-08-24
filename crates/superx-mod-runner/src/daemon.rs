//! The runner's daemon loop (epic #189 S3, phase R3): tick, find due
//! schedules, fire them. Lives in the OS daemon beside capture;
//! spawned once at startup, honors live disable per tick, and picks
//! up mid-life provisioning (each tick re-tries `module_db`).

use chrono::Utc;
use superx_kernel::capture::module_enabled;
use superx_kernel::{Kernel, NodeKind};

use crate::exec::{fire, CliExchange, FiringConfig};
use crate::{plan, schedule, MODULE_NAME};

/// Executor command parameter — NO fallback by design (epic S2): an
/// unset command makes dispatch fail loudly; nothing spawns agents
/// the operator never configured.
///
/// The kernel parameter names below are no longer where these settings
/// LIVE (#284) — they are where they may still be found, once, so a
/// value set before the module owned its settings is adopted rather
/// than lost.
pub const AGENT_CMD_PARAM: &str = "attr_runner_agent_cmd";
/// Concurrent dispatch ceiling per firing.
pub const MAX_PARALLEL_PARAM: &str = "attr_runner_max_parallel";
const DEFAULT_MAX_PARALLEL: usize = 2; // skill-allow: §9-const — bootstrap fallback, param-overridable (attr_runner_max_parallel)
/// Due-schedule poll cadence.
pub const TICK_PARAM: &str = "attr_runner_tick_secs";
const DEFAULT_TICK_SECS: u64 = 5; // skill-allow: §9-const — bootstrap fallback, param-overridable (attr_runner_tick_secs)

/// Spawn the scheduler loop.
///
/// The kernel owns single-start (M0): it only calls `startup()` for a
/// module it is not already running. A process-lifetime `AtomicBool`
/// used to guard this, which was correct while nothing could ever
/// stop a module — and fatal the moment one can, because a latched
/// static means the loop never comes back after a restart.
pub fn spawn_once(kernel: Kernel) {
    let stop = kernel.module_token(MODULE_NAME);
    tokio::spawn(run_loop(kernel, stop));
}

async fn run_loop(kernel: Kernel, stop: superx_kernel::supervise::CancelToken) {
    tracing::info!(target: "runner", "scheduler loop up");
    loop {
        let tick = resolved_u64(&kernel, TICK_PARAM, DEFAULT_TICK_SECS).await;
        tokio::time::sleep(std::time::Duration::from_secs(tick.max(1))).await;

        // Stopped means gone, not idling: the tick boundary is where a
        // firing-free moment exists to leave on.
        if stop.is_cancelled() {
            tracing::info!(target: "runner", "scheduler loop stopped");
            return;
        }

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
                agent_cmd: resolved_agent_cmd(&kernel).await,
                max_parallel: resolved_max_parallel(&kernel).await,
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

/// Resolved executor command (public for `runner config`).
pub async fn resolved_agent_cmd(kernel: &Kernel) -> Option<String> {
    if let Some(own) = crate::params::load(kernel).agent_cmd {
        return Some(own);
    }
    if let Some(adopted) = resolved_string(kernel, AGENT_CMD_PARAM).await {
        let value = adopted.clone();
        crate::params::adopt(kernel, |s| s.agent_cmd = Some(value));
        return Some(adopted);
    }
    None
}

/// Resolved parallel ceiling (public for `runner config`).
pub async fn resolved_max_parallel(kernel: &Kernel) -> usize {
    if let Some(own) = crate::params::load(kernel).max_parallel {
        return own.max(1);
    }
    match kernel_u64(kernel, MAX_PARALLEL_PARAM).await {
        Some(set) => {
            let adopted = usize::try_from(set).unwrap_or(DEFAULT_MAX_PARALLEL).max(1);
            crate::params::adopt(kernel, |s| s.max_parallel = Some(adopted));
            adopted
        }
        None => DEFAULT_MAX_PARALLEL,
    }
}

/// Resolved tick cadence (public for `runner config`).
pub async fn resolved_tick_secs(kernel: &Kernel) -> u64 {
    if let Some(own) = crate::params::load(kernel).tick_secs {
        return own.max(1);
    }
    match kernel_u64(kernel, TICK_PARAM).await {
        Some(set) => {
            let adopted = set.max(1);
            crate::params::adopt(kernel, |s| s.tick_secs = Some(adopted));
            adopted
        }
        None => DEFAULT_TICK_SECS,
    }
}

/// The kernel's value for a numeric parameter, if it HAS one.
///
/// The defaulting reader below cannot distinguish "the operator chose
/// the default" from "the operator chose nothing", and adoption turns on
/// exactly that difference: a value set deliberately must move into the
/// module's own settings even when it happens to equal the default, or
/// the record of having chosen it is lost when the storage moves.
pub(crate) async fn kernel_u64(kernel: &Kernel, param: &str) -> Option<u64> {
    let entity = kernel
        .find_module_by_name(NodeKind::KernelModule, MODULE_NAME)
        .await
        .ok()
        .flatten()?;
    match kernel.get_parameter(entity, param).await {
        Ok(Some(superx_kernel::types::Value::Number(n))) => {
            n.to_int().and_then(|i| u64::try_from(i).ok())
        }
        _ => None,
    }
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
