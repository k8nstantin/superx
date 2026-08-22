//! Module lifecycle, made real (epic M0).
//!
//! `KernelModule::shutdown` was declared and called by nothing in the
//! workspace, so `modules disable` only ever *paused* a module: its
//! handlers refused per tick while its socket stayed bound, and
//! `superx stop` killed servers mid-flight with no flush. Epic #141
//! specified otherwise — "disable → shutdown(), enable → startup(); no
//! OS restart" and "disable = server stops (live contract proof)".
//! This module pays that debt.
//!
//! Three pieces, and the split matters:
//!
//! - a **stop signal per module** ([`CancelToken`]) that the module's
//!   own tasks watch, so a server closes its listener instead of being
//!   torn down with the process. The kernel cannot stop a module's
//!   tasks for it — only the module knows what "finish cleanly" means.
//! - a **reconciler** that converges the substrate's desired state
//!   onto this process's actual state. `superx modules disable` runs in
//!   a *different process* from the daemon, so it cannot reach into the
//!   daemon's memory; it writes the substrate, and the daemon notices.
//! - a **failure policy**, because the moment starting is something
//!   that happens repeatedly, a module that cannot start becomes an
//!   infinite retry loop. Backoff, then a ceiling, then stop trying.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use tokio::sync::watch;

use crate::registry::{KernelModule, KernelModuleDescriptor, KERNEL_MODULES};
use crate::types::Value;
use crate::{Kernel, NodeKind, Result};

/// How often the daemon converges desired state onto actual state.
pub const RECONCILE_PARAM: &str = "attr_kernel_reconcile_secs";
const DEFAULT_RECONCILE_SECS: u64 = 5; // skill-allow: §9-const — bootstrap fallback, param-overridable (attr_kernel_reconcile_secs)

/// First retry delay after a failed start; doubles per attempt.
pub const BACKOFF_BASE_PARAM: &str = "attr_kernel_module_backoff_base_ms";
const DEFAULT_BACKOFF_BASE_MS: u64 = 1_000; // skill-allow: §9-const — bootstrap fallback, param-overridable (attr_kernel_module_backoff_base_ms)
/// Ceiling on the doubling.
pub const BACKOFF_MAX_PARAM: &str = "attr_kernel_module_backoff_max_ms";
const DEFAULT_BACKOFF_MAX_MS: u64 = 60_000; // skill-allow: §9-const — bootstrap fallback, param-overridable (attr_kernel_module_backoff_max_ms)
/// Failed starts inside the window before the kernel stops trying.
pub const RESTART_CEILING_PARAM: &str = "attr_kernel_module_restart_ceiling";
const DEFAULT_RESTART_CEILING: u32 = 5; // skill-allow: §9-const — bootstrap fallback, param-overridable (attr_kernel_module_restart_ceiling)
/// The window those failures are counted in.
pub const RESTART_WINDOW_PARAM: &str = "attr_kernel_module_restart_window_secs";
const DEFAULT_RESTART_WINDOW_SECS: u64 = 300; // skill-allow: §9-const — bootstrap fallback, param-overridable (attr_kernel_module_restart_window_secs)

/// Set on a module's own entity by `superx modules restart <name>`:
/// an RFC3339 instant. Due, and newer than the module's start ⇒ cycle.
pub const RESTART_PARAM: &str = "attr_module_restart_at";

/// A module's stop signal.
///
/// Handed out by [`Kernel::module_token`] and cloneable. A module's
/// tasks await [`CancelToken::cancelled`] (axum's
/// `with_graceful_shutdown` takes it directly) or poll
/// [`CancelToken::is_cancelled`] between units of work.
#[derive(Clone)]
pub struct CancelToken(watch::Receiver<bool>);

impl CancelToken {
    /// Resolves once this module has been asked to stop.
    ///
    /// Also resolves if the kernel dropped the sender: a vanished
    /// kernel means stop, never "wait forever".
    pub async fn cancelled(mut self) {
        loop {
            if *self.0.borrow_and_update() {
                return;
            }
            if self.0.changed().await.is_err() {
                return;
            }
        }
    }

    /// Non-blocking check, for loops that work in discrete units.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        *self.0.borrow()
    }
}

/// Why a module is being stopped. It decides what the substrate is
/// told: an operator disable is a lifecycle fact worth recording, a
/// restart's stop half is not (the start that follows records it), and
/// at process exit there is nobody left to read a new row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum StopReason {
    Disabled,
    Restarting,
    Shutdown,
}

/// A module running in THIS process.
struct ModuleRuntime {
    stop: watch::Sender<bool>,
    started_at: DateTime<Utc>,
}

/// A module that failed to start, and what the kernel intends to do
/// about it. Bounded retry lives here.
struct ModuleFailure {
    /// Failed starts inside the current window.
    count: u32,
    /// When the current counting window opened.
    window_start: DateTime<Utc>,
    /// Earliest next attempt — the backoff.
    next_attempt_at: DateTime<Utc>,
    /// The ceiling was hit: stop trying until an operator intervenes.
    gave_up: bool,
}

/// Per-process supervision state: what is running, and what is broken.
#[derive(Default)]
pub(crate) struct Supervision {
    running: HashMap<String, ModuleRuntime>,
    failures: HashMap<String, ModuleFailure>,
}

/// Shared across `Kernel` clones — a clone that could not see the
/// running set would hand out tokens nobody cancels.
pub(crate) type RuntimeTable = Arc<Mutex<Supervision>>;

impl Kernel {
    /// This module's stop signal, created on first ask.
    ///
    /// Called by a module inside `startup()`; the token stays valid
    /// until the kernel stops the module, and a later start gets a
    /// fresh one (a stopped module's token stays cancelled forever, so
    /// reusing it would kill the new server immediately).
    #[must_use]
    pub fn module_token(&self, module: &str) -> CancelToken {
        let mut state = self.supervision();
        let entry = state.running.entry(module.to_string()).or_insert_with(|| {
            let (stop, _) = watch::channel(false);
            ModuleRuntime {
                stop,
                started_at: Utc::now(),
            }
        });
        CancelToken(entry.stop.subscribe())
    }

    /// Is this module running in this process?
    #[must_use]
    pub fn module_is_running(&self, module: &str) -> bool {
        self.supervision().running.contains_key(module)
    }

    /// When this module last started in this process.
    #[must_use]
    pub fn module_started_at(&self, module: &str) -> Option<DateTime<Utc>> {
        self.supervision().running.get(module).map(|r| r.started_at)
    }

    /// Names of every module running in this process.
    #[must_use]
    pub fn running_modules(&self) -> Vec<String> {
        let mut names: Vec<String> = self.supervision().running.keys().cloned().collect();
        names.sort();
        names
    }

    /// Has the kernel stopped trying to start this module?
    #[must_use]
    pub fn module_gave_up(&self, module: &str) -> bool {
        self.supervision()
            .failures
            .get(module)
            .is_some_and(|f| f.gave_up)
    }

    /// Failed starts recorded for this module in the current window.
    #[must_use]
    pub fn module_failure_count(&self, module: &str) -> u32 {
        self.supervision()
            .failures
            .get(module)
            .map_or(0, |f| f.count)
    }

    /// Record that a module started, stamping its start instant.
    /// Idempotent: an existing token is kept so tasks already watching
    /// it stay attached.
    pub(crate) fn note_module_running(&self, module: &str) {
        let mut state = self.supervision();
        match state.running.get_mut(module) {
            Some(entry) => entry.started_at = Utc::now(),
            None => {
                let (stop, _) = watch::channel(false);
                state.running.insert(
                    module.to_string(),
                    ModuleRuntime {
                        stop,
                        started_at: Utc::now(),
                    },
                );
            }
        }
    }

    /// Signal this module's tasks to stop and forget it. Returns false
    /// if it was not running.
    pub(crate) fn signal_module_stop(&self, module: &str) -> bool {
        let Some(entry) = self.supervision().running.remove(module) else {
            return false;
        };
        // A send error means nobody is listening — the module never
        // took its token. Stopping is still the right outcome.
        let _ignored = entry.stop.send(true);
        true
    }

    fn supervision(&self) -> std::sync::MutexGuard<'_, Supervision> {
        // A poisoned mutex means a thread panicked while holding it.
        // This map's invariants are simple enough to recover, and a
        // kernel that refuses to supervise because of an unrelated
        // panic is worse than one that carries on.
        match self.runtime.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// Spawn the reconciler for the life of the daemon. `stop` is the
/// daemon's own shutdown signal — the same one the capture loop takes.
pub fn spawn_reconciler(kernel: Kernel, stop: watch::Receiver<bool>) {
    tokio::spawn(reconcile_loop(kernel, stop));
}

async fn reconcile_loop(kernel: Kernel, mut stop: watch::Receiver<bool>) {
    tracing::info!(target: "kernel", "module reconciler up");
    loop {
        let secs = resolved_u64(&kernel, RECONCILE_PARAM, DEFAULT_RECONCILE_SECS)
            .await
            .max(1);
        tokio::select! {
            () = tokio::time::sleep(Duration::from_secs(secs)) => {}
            _ = stop.changed() => return,
        }
        if *stop.borrow() {
            return;
        }
        reconcile_once(&kernel).await;
    }
}

/// One convergence pass. Never returns an error: a module that cannot
/// be read this tick is left exactly as it is and retried next tick.
/// A reconciler that gives up on a transient substrate hiccup is worse
/// than one that waits.
pub async fn reconcile_once(kernel: &Kernel) {
    for module in KERNEL_MODULES {
        let desc = module.descriptor();
        let Ok(enabled) = crate::capture::module_enabled(kernel, desc.kind, desc.name).await else {
            continue;
        };
        let running = kernel.module_is_running(desc.name);
        match (enabled, running) {
            (true, false) => {
                if may_start(kernel, desc.name) {
                    start(kernel, *module, &desc).await;
                }
            }
            (false, true) => stop_module(kernel, *module, &desc, StopReason::Disabled).await,
            (true, true) => {
                if restart_requested(kernel, &desc).await {
                    stop_module(kernel, *module, &desc, StopReason::Restarting).await;
                    start(kernel, *module, &desc).await;
                }
            }
            (false, false) => {}
        }
    }
}

/// Stop every module running in this process, dependents first, so a
/// module is never torn out from under something still using it.
/// Called on daemon shutdown: servers close and buffers flush before
/// the process exits.
pub async fn shutdown_all(kernel: &Kernel) {
    let by_name: HashMap<&str, &'static (dyn KernelModule + Sync)> = KERNEL_MODULES
        .iter()
        .map(|m| (m.descriptor().name, *m))
        .collect();
    let deps: HashMap<&str, &'static [&'static str]> = KERNEL_MODULES
        .iter()
        .map(|m| {
            let d = m.descriptor();
            (d.name, d.depends_on)
        })
        .collect();
    for name in stop_order(&kernel.running_modules(), &deps) {
        if let Some(module) = by_name.get(name.as_str()) {
            let desc = module.descriptor();
            stop_module(kernel, *module, &desc, StopReason::Shutdown).await;
        }
    }
}

/// Order a set of running modules so that nothing is stopped while
/// something still-to-be-stopped depends on it — boot's topological
/// order, reversed. A dependency cycle (which boot refuses) degrades
/// to "stop the rest in name order" rather than looping forever.
fn stop_order(running: &[String], deps: &HashMap<&str, &'static [&'static str]>) -> Vec<String> {
    let mut remaining: Vec<String> = running.to_vec();
    let mut order: Vec<String> = Vec::with_capacity(remaining.len());
    while !remaining.is_empty() {
        let ready: Vec<String> = remaining
            .iter()
            .filter(|name| {
                !remaining.iter().any(|other| {
                    other != *name
                        && deps
                            .get(other.as_str())
                            .is_some_and(|d| d.contains(&name.as_str()))
                })
            })
            .cloned()
            .collect();
        if ready.is_empty() {
            order.append(&mut remaining);
            break;
        }
        remaining.retain(|r| !ready.contains(r));
        order.extend(ready);
    }
    order
}

/// May the kernel attempt a start right now? False while a backoff is
/// pending, and false forever once the ceiling has been hit — until an
/// operator disables and re-enables, or asks for a restart.
fn may_start(kernel: &Kernel, module: &str) -> bool {
    let state = kernel.supervision();
    match state.failures.get(module) {
        None => true,
        Some(f) if f.gave_up => false,
        Some(f) => Utc::now() >= f.next_attempt_at,
    }
}

async fn start(
    kernel: &Kernel,
    module: &'static (dyn KernelModule + Sync),
    desc: &KernelModuleDescriptor,
) {
    // A stale token from a previous life would arrive already
    // cancelled; clear it so startup() gets a fresh one.
    let _was_running = kernel.signal_module_stop(desc.name);
    let began = Instant::now();
    match module.startup(kernel).await {
        Ok(()) => {
            let elapsed_ms = u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX);
            kernel.note_module_running(desc.name);
            kernel.supervision().failures.remove(desc.name);
            tracing::info!(target: "kernel", module = desc.name, elapsed_ms, "module started");
            emit(kernel, "module_started", desc).await;
            if let Err(e) = kernel.mark_active(desc.kind, desc.name, elapsed_ms).await {
                tracing::warn!(target: "kernel", %e, "mark_active failed");
            }
        }
        Err(e) => {
            // A module that asked for its token before failing would
            // otherwise look running forever, and the reconciler would
            // never retry it.
            kernel.signal_module_stop(desc.name);
            let error = e.to_string();
            let (attempts, gave_up) = record_failure(kernel, desc.name).await;
            let detail = if gave_up {
                format!("{error} (gave up after {attempts} failed starts)")
            } else {
                error
            };
            tracing::warn!(
                target: "kernel", module = desc.name, attempts, gave_up, detail = %detail,
                "module start failed"
            );
            emit(
                kernel,
                if gave_up {
                    "module_start_abandoned"
                } else {
                    "module_start_failed"
                },
                desc,
            )
            .await;
            if let Err(e) = kernel
                .mark_failed(desc.kind, desc.name, &detail, "startup")
                .await
            {
                tracing::warn!(target: "kernel", %e, "mark_failed failed");
            }
        }
    }
}

/// Record a failed start and decide the next attempt. Returns
/// `(attempts in window, gave up)`.
async fn record_failure(kernel: &Kernel, module: &str) -> (u32, bool) {
    // The knobs are read only on the failure path: a healthy instance
    // pays nothing for a policy it never exercises.
    let base_ms = resolved_u64(kernel, BACKOFF_BASE_PARAM, DEFAULT_BACKOFF_BASE_MS).await;
    let max_ms = resolved_u64(kernel, BACKOFF_MAX_PARAM, DEFAULT_BACKOFF_MAX_MS).await;
    let ceiling = u32::try_from(
        resolved_u64(
            kernel,
            RESTART_CEILING_PARAM,
            u64::from(DEFAULT_RESTART_CEILING),
        )
        .await,
    )
    .unwrap_or(DEFAULT_RESTART_CEILING);
    // An absurd window clamps to "effectively forever", which is the
    // conservative direction: the counter never resets, so the ceiling
    // still bounds the retries.
    let window_secs = i64::try_from(
        resolved_u64(kernel, RESTART_WINDOW_PARAM, DEFAULT_RESTART_WINDOW_SECS).await,
    )
    .unwrap_or(i64::MAX);

    let now = Utc::now();
    let mut state = kernel.supervision();
    let entry = state
        .failures
        .entry(module.to_string())
        .or_insert(ModuleFailure {
            count: 0,
            window_start: now,
            next_attempt_at: now,
            gave_up: false,
        });
    // A quiet spell reopens the window: this is a rate, not a tally.
    if (now - entry.window_start).num_seconds() > window_secs {
        entry.window_start = now;
        entry.count = 0;
    }
    entry.count += 1;
    let shift = entry.count.saturating_sub(1).min(16);
    let delay_ms = base_ms.saturating_mul(1u64 << shift).min(max_ms);
    entry.next_attempt_at = now + chrono::Duration::milliseconds(
        i64::try_from(delay_ms).unwrap_or(i64::MAX),
    );
    entry.gave_up = ceiling > 0 && entry.count >= ceiling;
    (entry.count, entry.gave_up)
}

async fn stop_module(
    kernel: &Kernel,
    module: &'static (dyn KernelModule + Sync),
    desc: &KernelModuleDescriptor,
    reason: StopReason,
) {
    // Signal first, then ask: the module's tasks get their stop notice
    // before shutdown() runs, so shutdown() can await them.
    kernel.signal_module_stop(desc.name);
    if let Err(e) = module.shutdown(kernel).await {
        tracing::warn!(target: "kernel", module = desc.name, error = %e, "module shutdown reported an error");
    }
    tracing::info!(target: "kernel", module = desc.name, ?reason, "module stopped");
    emit(kernel, "module_stopped", desc).await;
    // An operator disable is a lifecycle fact: without this the
    // registry keeps reporting `active` for a module whose port is
    // gone. A restart's stop half is not recorded — the start that
    // follows it writes the truth — and at process exit there is
    // nobody left to read a new row.
    if reason == StopReason::Disabled {
        if let Err(e) = kernel.mark_disabled(desc.kind, desc.name).await {
            tracing::warn!(target: "kernel", %e, "mark_disabled failed");
        }
    }
}

/// Is a restart due? The request must be in the past (so a
/// future-dated or clock-skewed value waits for its time instead of
/// firing every tick forever) and newer than the module's start (so it
/// fires once, not on every pass).
async fn restart_requested(kernel: &Kernel, desc: &KernelModuleDescriptor) -> bool {
    let Ok(Some(entity)) = kernel.find_module_by_name(desc.kind, desc.name).await else {
        return false;
    };
    let Ok(Some(Value::String(at))) = kernel.get_parameter(entity, RESTART_PARAM).await else {
        return false;
    };
    let Ok(requested) = DateTime::parse_from_rfc3339(&at) else {
        // Debug, not warn: this is read every tick, and an operator
        // typo must not fill the log forever.
        tracing::debug!(target: "kernel", module = desc.name, value = %at, "unparseable restart request");
        return false;
    };
    let requested = requested.with_timezone(&Utc);
    let now = Utc::now();
    requested <= now
        && kernel
            .module_started_at(desc.name)
            .is_some_and(|started| requested > started)
}

/// A substrate parameter on the kernel's own entity, with a marked
/// fallback.
async fn resolved_u64(kernel: &Kernel, param: &str, fallback: u64) -> u64 {
    let Ok(Some(entity)) = kernel
        .find_module_by_name(NodeKind::KernelModule, "kernel")
        .await
    else {
        return fallback;
    };
    match kernel.get_parameter(entity, param).await {
        Ok(Some(Value::Number(n))) => n
            .to_int()
            .and_then(|i| u64::try_from(i).ok())
            .filter(|&v| v > 0)
            .unwrap_or(fallback),
        _ => fallback,
    }
}

/// Request a restart: stamp the module's entity with now. The daemon
/// converges on its next tick; nothing here reaches into its memory.
///
/// # Errors
///
/// Verb errors pass through: an unknown module, or a substrate refusal.
pub async fn request_restart(kernel: &Kernel, kind: NodeKind, name: &str) -> Result<()> {
    let entity = kernel
        .find_module_by_name(kind, name)
        .await?
        .ok_or_else(|| crate::KernelError::Module(format!("module '{name}' is not registered")))?;
    kernel
        .set_parameter(
            entity,
            RESTART_PARAM,
            Value::String(Utc::now().to_rfc3339()),
        )
        .await?;
    // An explicit operator restart clears an abandoned module's
    // failure record: "try again" is the whole point of asking.
    kernel.supervision().failures.remove(name);
    Ok(())
}

async fn emit(kernel: &Kernel, event: &str, desc: &KernelModuleDescriptor) {
    let subject = kernel
        .find_module_by_name(desc.kind, desc.name)
        .await
        .ok()
        .flatten();
    let payload = crate::message::value_from_json(&serde_json::json!({
        "module": desc.name,
        "version": desc.version,
    }));
    if let Err(e) = kernel.log_telemetry(event, payload, subject).await {
        tracing::warn!(target: "kernel", %e, "lifecycle telemetry write failed");
    }
}

#[cfg(test)]
mod tests {
    use super::stop_order;
    use std::collections::HashMap;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    /// Dependents stop before the things they depend on. Reversed, and
    /// a dependency would be pulled out from under a live dependent.
    #[test]
    fn dependents_stop_first() {
        let deps: HashMap<&str, &'static [&'static str]> =
            [("ui", &["entities"][..]), ("entities", &[][..])]
                .into_iter()
                .collect();
        // Alphabetical order would stop `entities` first — the bug.
        let order = stop_order(&names(&["entities", "ui"]), &deps);
        assert_eq!(order, names(&["ui", "entities"]));
    }

    #[test]
    fn a_chain_unwinds_from_the_top() {
        let deps: HashMap<&str, &'static [&'static str]> = [
            ("a", &[][..]),
            ("b", &["a"][..]),
            ("c", &["b"][..]),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            stop_order(&names(&["a", "b", "c"]), &deps),
            names(&["c", "b", "a"])
        );
    }

    /// A dependency on something that is not running must not stall
    /// the walk — that would leave modules never stopped at all.
    #[test]
    fn a_dependency_that_is_not_running_is_ignored() {
        let deps: HashMap<&str, &'static [&'static str]> = [("ui", &["absent"][..])]
            .into_iter()
            .collect();
        assert_eq!(stop_order(&names(&["ui"]), &deps), names(&["ui"]));
    }

    /// A cycle is impossible through boot, which refuses them — but if
    /// one ever appears, stop everything rather than loop forever.
    #[test]
    fn a_cycle_still_terminates() {
        let deps: HashMap<&str, &'static [&'static str]> =
            [("x", &["y"][..]), ("y", &["x"][..])].into_iter().collect();
        let order = stop_order(&names(&["x", "y"]), &deps);
        assert_eq!(order.len(), 2, "everything is stopped exactly once");
    }
}
