//! SuperX substrate kernel — the service-account-only access layer.
//!
//! Every interaction with the SurrealDB substrate flows through this
//! crate. The kernel signs in as the `superx` EDITOR service account
//! (per [SKILL.md §13]) and exposes only typed CREATE / SELECT verbs.
//! There is no `update_*` or `delete_*` method on this surface,
//! anywhere — the append-only invariant is enforced by what the API
//! does NOT contain (kernel-verb discipline, per [SKILL.md §10]).
//!
//! This crate is intentionally narrow at v0.1 — it owns the connection
//! and the one verb every subsequent feature needs (`log_telemetry`).
//! CREATE verbs for typed entities, relations, state, schedule, etc.
//! land in subsequent PRs strictly on top of this baseline.

pub mod error;
pub mod kernel;
pub mod schema;

pub use error::{KernelError, Result};
pub use kernel::{Kernel, TelemetryRecord};
pub use schema::SCHEMA_DDL;
