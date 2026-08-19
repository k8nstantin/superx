//! SuperX kernel — the core of the agentic operating system.
//!
//! Per [`BLUEPRINT.md`] §2 the kernel owns exactly five things:
//! substrate access, boot, the agent-telemetry capture engine, the
//! operator CLI surface, and the module system. This crate is built up
//! phase by phase; G2 lands the first two pillars of the library:
//!
//! 1. **Substrate access** — [`Kernel::connect_service`] signs in to
//!    the operator-provisioned SurrealDB as the `superx_kernel`
//!    service account (never root — skill §13), and the typed verbs
//!    [`Kernel::ensure_type_definition`], [`Kernel::find_type`],
//!    [`Kernel::create_entity`], [`Kernel::create_relation`],
//!    [`Kernel::supersede_state`], [`Kernel::current_state`].
//!    **SELECT + CREATE only** — the append-only invariant is enforced
//!    by what this API does not contain: there is no `update_*`, no
//!    `delete_*`, no `upsert_*` (skill §10).
//!
//! 2. **Telemetry primitive** — [`Kernel::log_telemetry`] +
//!    [`Kernel::recent_telemetry`] + [`Kernel::telemetry_since`].
//!    Every mutation by every layer above emits a typed event through
//!    this verb. Reads emit nothing — readers must not mutate the
//!    stream they observe.
//!
//! Plus the **kernel self-log** ([`logging::init`]) — a rolling file
//! log in a local `logs/` directory, independent of the substrate, so
//! boot diagnostics survive substrate unavailability.
//!
//! Landing in later phases: the module system + boot orchestration
//! (G3), the capture engine + agent adapters (G4–G5), the CLI surface
//! (G6).
//!
//! Schema knowledge lives in [`SCHEMA_DDL`] (`include_str!` of the
//! locked `schema/kernel.surql`). Production never applies it — that
//! is the operator's one-shot root step (skill §11); tests apply it to
//! a `mem://` engine.
//!
//! [`BLUEPRINT.md`]: https://github.com/k8nstantin/superx/blob/main/BLUEPRINT.md

pub mod adapters;
pub mod boot;
pub mod capture;
pub mod cursor;
pub mod error;
pub mod lifecycle;
pub mod logging;
pub mod message;
pub mod metamodel;
pub mod parameters;
pub mod provision;
pub mod registry;
pub mod schema;
pub mod substrate;
pub mod telemetry;

/// Substrate value/record types, re-exported for modules and adapters.
/// The blueprint's layering rule means no layer above the kernel
/// imports `surrealdb` directly — payload construction goes through
/// this re-export, which also guarantees a single surrealdb version
/// across the workspace.
pub use surrealdb::types;

/// The handle type [`Kernel::module_db`](crate::Kernel::module_db)
/// yields — the alias lets module crates name their own-db connection
/// without importing surrealdb directly (the layering rule).
pub type Db = surrealdb::Surreal<surrealdb::engine::any::Any>;

pub use boot::{boot, BootEntry, BootReport};
pub use capture::{
    AgentAdapter, DiscoveredSource, SourceRef, TickReport, ADAPTERS,
};
pub use cursor::Cursor;
pub use error::{KernelError, Result};
pub use message::{MessageRecord, NewMessage};
pub use lifecycle::LifecycleState;
pub use metamodel::{MetamodelType, REQUIRED_METAMODEL_TYPES};
pub use registry::{
    KernelModule, KernelModuleDescriptor, ModuleRecord, ModuleStatus, NodeKind,
    RegistryStatus, KERNEL_MODULES,
};
pub use schema::SCHEMA_DDL;
pub use substrate::{Kernel, NamedEntity};
pub use telemetry::TelemetryRecord;
