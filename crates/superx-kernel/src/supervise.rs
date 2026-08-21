//! Module lifecycle, made real (epic M0).
//!
//! `KernelModule::shutdown` was declared and never called by anything
//! in the workspace, so `modules disable` only ever *paused* a module:
//! its handlers refused per tick while its socket stayed bound, and
//! `superx stop` killed servers mid-flight with no flush. Epic #141
//! specified otherwise — "disable → shutdown(), enable → startup(); no
//! OS restart" and "disable = server stops (live contract proof)".
//! This module pays that debt.
//!
//! Two pieces, and the split matters:
//!
//! - a **stop signal per module** ([`CancelToken`]) that the module's
//!   own tasks watch, so a server closes its listener instead of being
//!   torn down with the process. The kernel cannot stop a module's
//!   tasks for it — only the module knows what "finish cleanly" means.
//! - a **reconciler** that converges the substrate's desired state
//!   onto this process's actual state. `superx modules disable` runs in
//!   a *different process* from the daemon, so it cannot reach into the
//!   daemon's memory; it writes the substrate, and the daemon notices.
//!   Desired state in the substrate, actual state in the process,
//!   converge on a tick: the same shape the runner's frontier uses.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::watch;

use crate::registry::{KernelModule, KernelModuleDescriptor, KERNEL_MODULES};
use crate::types::Value;
use crate::{Kernel, NodeKind, Result};

/// How often the daemon converges desired state onto actual state.
pub const RECONCILE_PARAM: &str = "attr_kernel_reconcile_secs";
const DEFAULT_RECONCILE_SECS: u64 = 5; // skill-allow: §9-const — bootstrap fallback, param-overridable (attr_kernel_reconcile_secs)

/// Set on a module's own entity by `superx modules restart <name>`:
/// an RFC3339 instant. Newer than the module's start ⇒ cycle it.
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

/// What the kernel knows about a module running in THIS process.
pub(crate) struct ModuleRuntime {
    stop: watch::Sender<bool>,
    started_at: DateTime<Utc>,
}

/// The per-process module table. Shared across `Kernel` clones — a
/// clone that could not see the running set would hand out tokens
/// nobody cancels.
pub(crate) type RuntimeTable = Arc<Mutex<HashMap<String, ModuleRuntime>>>;

impl Kernel {
    /// This module's stop signal, created on first ask.
    ///
    /// Called by a module inside `startup()`; the token stays valid
    /// until the kernel stops the module, and a later start gets a
    /// fresh one (a stopped module's token stays cancelled forever, so
    /// reusing it would kill the new server immediately).
    #[must_use]
    pub fn module_token(&self, module: &str) -> CancelToken {
        let mut table = self.runtime_table();
        let entry = table.entry(module.to_string()).or_insert_with(|| {
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
        self.runtime_table().contains_key(module)
    }

    /// When this module last started in this process.
    #[must_use]
    pub fn module_started_at(&self, module: &str) -> Option<DateTime<Utc>> {
        self.runtime_table().get(module).map(|r| r.started_at)
    }

    /// Names of every module running in this process.
    #[must_use]
    pub fn running_modules(&self) -> Vec<String> {
        let mut names: Vec<String> = self.runtime_table().keys().cloned().collect();
        names.sort();
        names
    }

    /// Record that a module started, stamping its start instant.
    /// Idempotent: an existing token is kept so tasks already watching
    /// it stay attached.
    pub(crate) fn note_module_running(&self, module: &str) {
        let mut table = self.runtime_table();
        match table.get_mut(module) {
            Some(entry) => entry.started_at = Utc::now(),
            None => {
                let (stop, _) = watch::channel(false);
                table.insert(
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
        let Some(entry) = self.runtime_table().remove(module) else {
            return false;
        };
        // A send error means nobody is listening — the module never
        // took its token. Stopping is still the right outcome.
        let _ignored = entry.stop.send(true);
        true
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
        let secs = resolved_reconcile_secs(&kernel).await.max(1);
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
            (true, false) => start(kernel, *module, &desc).await,
            (false, true) => stop_module(kernel, *module, &desc).await,
            (true, true) => {
                if restart_requested(kernel, &desc).await {
                    stop_module(kernel, *module, &desc).await;
                    start(kernel, *module, &desc).await;
                }
            }
            (false, false) => {}
        }
    }
}

/// Stop every module running in this process, in reverse-name order.
/// Called on daemon shutdown so servers close and buffers flush before
/// the process exits.
pub async fn shutdown_all(kernel: &Kernel) {
    let by_name: HashMap<&str, &'static (dyn KernelModule + Sync)> = KERNEL_MODULES
        .iter()
        .map(|m| (m.descriptor().name, *m))
        .collect();
    for name in kernel.running_modules() {
        if let Some(module) = by_name.get(name.as_str()) {
            let desc = module.descriptor();
            stop_module(kernel, *module, &desc).await;
        }
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
    match module.startup(kernel).await {
        Ok(()) => {
            kernel.note_module_running(desc.name);
            tracing::info!(target: "kernel", module = desc.name, "module started");
            emit(kernel, "module_started", desc).await;
            let _ = kernel
                .mark_active(desc.kind, desc.name, 0)
                .await
                .inspect_err(|e| tracing::warn!(target: "kernel", %e, "mark_active failed"));
        }
        Err(e) => {
            // A module that asked for its token before failing would
            // otherwise look running forever, and the reconciler would
            // never retry it.
            kernel.signal_module_stop(desc.name);
            let error = e.to_string();
            tracing::warn!(target: "kernel", module = desc.name, %error, "module start failed");
            emit(kernel, "module_start_failed", desc).await;
            let _ = kernel
                .mark_failed(desc.kind, desc.name, &error, "startup")
                .await
                .inspect_err(|e| tracing::warn!(target: "kernel", %e, "mark_failed failed"));
        }
    }
}

async fn stop_module(
    kernel: &Kernel,
    module: &'static (dyn KernelModule + Sync),
    desc: &KernelModuleDescriptor,
) {
    // Signal first, then ask: the module's tasks get their stop notice
    // before shutdown() runs, so shutdown() can await them.
    kernel.signal_module_stop(desc.name);
    if let Err(e) = module.shutdown(kernel).await {
        tracing::warn!(target: "kernel", module = desc.name, error = %e, "module shutdown reported an error");
    }
    tracing::info!(target: "kernel", module = desc.name, "module stopped");
    emit(kernel, "module_stopped", desc).await;
}

/// Has a restart been asked for since this module started?
async fn restart_requested(kernel: &Kernel, desc: &KernelModuleDescriptor) -> bool {
    let Ok(Some(entity)) = kernel.find_module_by_name(desc.kind, desc.name).await else {
        return false;
    };
    let Ok(Some(Value::String(at))) = kernel.get_parameter(entity, RESTART_PARAM).await else {
        return false;
    };
    let Ok(requested) = DateTime::parse_from_rfc3339(&at) else {
        tracing::warn!(target: "kernel", module = desc.name, value = %at, "unparseable restart request");
        return false;
    };
    kernel
        .module_started_at(desc.name)
        .is_some_and(|started| requested.with_timezone(&Utc) > started)
}

/// The reconcile cadence: substrate parameter with a marked fallback.
pub async fn resolved_reconcile_secs(kernel: &Kernel) -> u64 {
    let Ok(Some(entity)) = kernel
        .find_module_by_name(NodeKind::KernelModule, "kernel")
        .await
    else {
        return DEFAULT_RECONCILE_SECS;
    };
    match kernel.get_parameter(entity, RECONCILE_PARAM).await {
        Ok(Some(Value::Number(n))) => n
            .to_int()
            .and_then(|i| u64::try_from(i).ok())
            .filter(|&s| s > 0)
            .unwrap_or(DEFAULT_RECONCILE_SECS),
        _ => DEFAULT_RECONCILE_SECS,
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

/// The runtime table accessor, panic-free on a poisoned lock: a
/// poisoned mutex means a thread panicked while holding it, and the
/// map's invariants are simple enough to recover.
impl Kernel {
    fn runtime_table(&self) -> std::sync::MutexGuard<'_, HashMap<String, ModuleRuntime>> {
        match self.runtime.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}
