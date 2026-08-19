//! superx-mod-entities — the product graph substrate (epic #166).
//!
//! Typed entities as graph nodes (product, task, rag, model, document,
//! text — extensible at runtime), connected by native SurrealDB edges
//! into a many-to-many graph with full append-only history. Agents
//! will later travel this graph and execute the task nodes; this
//! module owns the substrate: construct, link, traverse, visualize.
//!
//! Module contract (epic #141): own database `superx/entities`, own
//! service account, own dir (`modules/entities/` — document files),
//! own log (`target: "entities"`), own CLI (`superx entities …`).

pub mod cli;
pub mod registry;

use async_trait::async_trait;
use superx_kernel::{
    Kernel, KernelModule, KernelModuleDescriptor, NodeKind, Result, KERNEL_MODULES,
};

/// The module's name — db name, dir name, CLI namespace, log target.
pub const MODULE_NAME: &str = "entities";

/// The module's own schema, applied by `superx modules provision
/// entities` into `superx/entities` (kernel schema untouched — §7).
pub const SCHEMA_DDL: &str = include_str!("../schema/entities.surql");

/// The graph substrate module. A ZST — all state lives in the
/// substrate, per the module contract.
pub struct EntitiesModule;

#[async_trait]
impl KernelModule for EntitiesModule {
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
        // Provisioning is an operator step. An unprovisioned module
        // must not fail the boot walk (the OS runs; the module waits
        // for `superx modules provision entities`). A provisioned-but-
        // broken registry, however, fails loudly and isolates.
        match kernel.module_db(MODULE_NAME).await {
            Ok(db) => {
                let seeded = registry::seed_types(&db).await?;
                if seeded > 0 {
                    tracing::info!(target: "entities", seeded, "type registry seeded");
                } else {
                    tracing::info!(target: "entities", "type registry current");
                }
            }
            Err(e) => {
                tracing::warn!(
                    target: "entities",
                    "own db unavailable — graph verbs disabled until provisioned: {e}"
                );
            }
        }
        Ok(())
    }

    fn schema_ddl(&self) -> Option<&'static str> {
        Some(SCHEMA_DDL)
    }

    fn needs_dir(&self) -> bool {
        true
    }

    async fn cli(&self, kernel: &Kernel, args: &[String]) -> Result<String> {
        cli::dispatch(kernel, args).await
    }
}

#[linkme::distributed_slice(KERNEL_MODULES)]
static ENTITIES_REGISTRATION: &'static (dyn KernelModule + Sync) = &EntitiesModule;
