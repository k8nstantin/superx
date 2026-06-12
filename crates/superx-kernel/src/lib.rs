//! SuperX atomic kernel — the four-pillar core.
//!
//! What the kernel does, and only what it does:
//!
//! 1. **Bootstrap-as-init** — [`Kernel::connect_service`] signs in to
//!    the substrate as the `superx_kernel` EDITOR service account.
//!    Pure connect — no telemetry; readers must not mutate the stream
//!    they observe. The one `system_boot` event per real boot is
//!    emitted by `superx-kernel-bootstrap` (which also orchestrates
//!    the other kernel modules).
//!
//! 2. **Storage** — typed verbs over the SurrealDB substrate:
//!    [`Kernel::create_entity`], [`Kernel::create_relation`],
//!    [`Kernel::supersede_state`], [`Kernel::current_state`],
//!    [`Kernel::ensure_type_definition`], [`Kernel::find_type`],
//!    [`Kernel::find_type_opt`]. **SELECT + CREATE only** — no UPDATE,
//!    DELETE, UPSERT, DEFINE, REMOVE.
//!
//! 3. **Telemetry primitive** — [`Kernel::log_telemetry`] +
//!    [`Kernel::recent_telemetry`]. Every kernel mutation by every
//!    layer above emits a typed event through this verb.
//!
//! 4. **Kernel module registry** — the [`KernelModule`] trait +
//!    [`KernelModuleDescriptor`] + the [`KERNEL_MODULES`]
//!    distributed-slice. Every kernel module / driver / app crate
//!    registers a static reference of itself into [`KERNEL_MODULES`]
//!    via `#[linkme::distributed_slice]` at link time.
//!    `superx-kernel-bootstrap` iterates the slice; no central
//!    enumeration anywhere.
//!
//! Plus:
//! - Schema knowledge in [`SCHEMA_DDL`] (`include_str!` of the locked
//!   `schema/kernel.surql`).
//! - Required metamodel uids in [`REQUIRED_METAMODEL_TYPES`] — info,
//!   not behavior. `superx-kernel-bootstrap` seeds them at first run.
//!
//! ## What the kernel does NOT do
//!
//! - Does NOT seed its own metamodel (that's bootstrap's job).
//! - Does NOT know what specific kernel modules / drivers / apps
//!   exist (it iterates linkme).
//! - Does NOT enumerate categories like "driver" / "app" — those are
//!   open-ended strings in [`KernelModuleDescriptor::category`].
//! - Does NOT update or delete substrate rows (kernel-verb discipline).
//! - Does NOT root authentication (operator-only).
//!
//! See `project_superx_architecture.md` in memory for the locked
//! architectural canon.

pub mod cursor;
pub mod error;
pub mod lifecycle;
pub mod metamodel;
pub mod parameters;
pub mod registry;
pub mod schema;
pub mod substrate;
pub mod telemetry;

/// Substrate value/record types, re-exported for kernel modules,
/// drivers, and apps. The canon boundary rule says no layer above L0
/// imports `surrealdb` directly — payload construction goes through
/// this re-export, which also guarantees a single surrealdb version
/// across the workspace.
pub use surrealdb::types;

pub use cursor::Cursor;
pub use error::{KernelError, Result};
pub use lifecycle::LifecycleState;
pub use metamodel::{MetamodelType, REQUIRED_METAMODEL_TYPES};
pub use registry::{
    KernelModule, KernelModuleDescriptor, ModuleStatus, NodeKind, RegistryStatus,
    KERNEL_MODULES,
};
pub use schema::SCHEMA_DDL;
pub use substrate::{Kernel, NamedEntity};
pub use telemetry::TelemetryRecord;
