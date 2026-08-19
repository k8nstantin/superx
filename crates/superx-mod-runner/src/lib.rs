//! superx-mod-runner — the scheduler (epic #189).
//!
//! Schedules ENTITIES: a schedule row is "at run_at, kick entity X"
//! and nothing else (§8/§9 — the graph is the plan). Phase R1 ships
//! the intent layer: schedule/queue/cancel with full SCD-2 history.
//! Planning (R2), the executor loop (R3), and recurrence (R4) build
//! on this per the epic's authoritative spec.

pub mod cli;
pub mod daemon;
pub mod exec;
pub mod plan;
pub mod recurrence;
pub mod run;
pub mod schedule;

use async_trait::async_trait;
use superx_kernel::{
    Kernel, KernelModule, KernelModuleDescriptor, NodeKind, Result, KERNEL_MODULES,
};

/// The module's name — db name, CLI namespace, log target.
pub const MODULE_NAME: &str = "runner";

/// The module's own schema (epic #189 S1), applied by
/// `superx modules provision runner` into `superx/runner`.
pub const SCHEMA_DDL: &str = include_str!("../schema/runner.surql");

/// The scheduler module. A ZST — all state lives in the substrate.
pub struct RunnerModule;

#[async_trait]
impl KernelModule for RunnerModule {
    fn descriptor(&self) -> KernelModuleDescriptor {
        KernelModuleDescriptor {
            name: MODULE_NAME,
            version: env!("CARGO_PKG_VERSION"),
            kind: NodeKind::KernelModule,
            depends_on: &[],
            required_metamodel: &[],
        }
    }

    async fn startup(&self, kernel: &Kernel) -> Result<()> {
        // R1 has nothing to seed; the executor loop arrives in R3.
        // An unprovisioned module must not fail the boot walk.
        match kernel.module_db(MODULE_NAME).await {
            Ok(_) => tracing::info!(target: "runner", "schedule substrate ready"),
            Err(e) => tracing::warn!(
                target: "runner",
                "own db unavailable — scheduling idle until provisioned: {e}"
            ),
        }
        // The loop lives regardless: it re-checks enablement and
        // provisioning every tick (live provision/enable, D27 spirit).
        crate::daemon::spawn_once(kernel.clone());
        Ok(())
    }

    fn schema_ddl(&self) -> Option<&'static str> {
        Some(SCHEMA_DDL)
    }

    async fn cli(&self, kernel: &Kernel, args: &[String]) -> Result<String> {
        cli::dispatch(kernel, args).await
    }
}

#[linkme::distributed_slice(KERNEL_MODULES)]
static RUNNER_REGISTRATION: &'static (dyn KernelModule + Sync) = &RunnerModule;
