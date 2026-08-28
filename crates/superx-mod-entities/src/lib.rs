//! superx-mod-entities — entities, fields, labels and links (epic #353).
//!
//! EVERYTHING HAS A UUID7, A NAME AND LABELS. A thing, a field on a
//! thing, and a link between two things are built from the same parts; a
//! field is not a lesser kind of row with a key and a value. What a
//! thing IS, is the labels it carries, and a label is itself an entity —
//! one carrying the label `label`, with its meaning written as a field
//! on it.
//!
//! A LINK IS A FIELD THE ENGINE CAN WALK. It carries a name, labels,
//! history and an author like any other field; only its storage differs,
//! because SurrealDB grants traversal to relations and not to columns,
//! and role graphs get deep enough that the difference decides whether
//! the graph is readable at all.
//!
//! This module STORES and DESCRIBES. It enforces two things and no more:
//! a label it is given must exist, and content must be what its datatype
//! says. What a role does, whether a mandate binds, which links are
//! worth following — all of that is the reader's judgement, and this
//! module knows nothing about who the reader is.
//!
//! Module contract (epic #141): own database `superx/entities`, own
//! service account, own dir, own log (`target: "entities"`), own CLI
//! namespace.
//!
//! The schema is designed and locked first, code after (§11). This is
//! the ground cleared and the foundation laid; the verbs land on top of
//! it.

use async_trait::async_trait;
use superx_kernel::{
    Kernel, KernelModule, KernelModuleDescriptor, NodeKind, Result, KERNEL_MODULES,
};

/// The module's name — db name, dir name, CLI namespace, log target.
pub const MODULE_NAME: &str = "entities";

/// The module's own schema, applied by `superx modules provision
/// entities` into `superx/entities` (kernel schema untouched — §7).
pub const SCHEMA_DDL: &str = include_str!("../schema/entities.surql");

/// Mint a uuid7 record id in the module's own db — time-ordered ids are
/// the historical log (the kernel's convention; its own minter is
/// crate-private).
#[must_use]
pub fn new_id(table: &'static str) -> superx_kernel::types::RecordId {
    superx_kernel::types::RecordId::new(
        table,
        superx_kernel::types::Uuid::from(uuid::Uuid::now_v7()),
    )
}

/// The entities module. A ZST — all state lives in the substrate, per
/// the module contract.
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
        // Provisioning is an operator step. An unprovisioned module must
        // not fail the boot walk: the OS runs, and the module waits for
        // `superx modules provision entities`.
        match kernel.module_db(MODULE_NAME).await {
            Ok(_) => tracing::info!(target: "entities", "substrate ready"),
            Err(e) => tracing::warn!(
                target: "entities",
                "own db unavailable — entity verbs disabled until provisioned: {e}"
            ),
        }
        Ok(())
    }

    fn schema_ddl(&self) -> Option<&'static str> {
        Some(SCHEMA_DDL)
    }

    fn needs_dir(&self) -> bool {
        true
    }

    async fn cli(&self, _kernel: &Kernel, _args: &[String]) -> Result<String> {
        Err(superx_kernel::KernelError::Module(
            "entities: no verbs yet — the schema is laid, the code is being written \
             on top of it (epic #353)"
                .to_string(),
        ))
    }
}

#[linkme::distributed_slice(KERNEL_MODULES)]
static ENTITIES_REGISTRATION: &'static (dyn KernelModule + Sync) = &EntitiesModule;
