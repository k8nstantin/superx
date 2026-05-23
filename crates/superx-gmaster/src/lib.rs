//! gmaster — graph master.
//!
//! Extracts knowledge graphs from source data into the SuperX
//! substrate. v1 covers Rust codebases via `tree-sitter-rust`; future
//! PRs add TypeScript / Go / Python / docs / PDFs / URLs / SQL / SCIP
//! ingest layered on the same pipeline. The "graphify" pattern is
//! ours — see ARCHITECTURE.md §0a-Pillar 2 ("Better than LlamaIndex
//! at structural+semantic fusion").
//!
//! ## Pipeline
//!
//! ```text
//! seed_gmaster_metamodel  →  extract (tree-sitter, in-memory)
//!                         →  persist (Kernel verbs → substrate)
//! ```
//!
//! Every node becomes one `entity` row plus one `state_ledger`
//! `attr_desc` row. Every edge becomes one `relation` row. Every run
//! emits one `gmaster_infer_complete` telemetry event.
//!
//! ## Action API
//!
//! `superx gmaster infer <path>` calls [`infer`]. Future actions
//! (cluster, query, export) land as additional pub fns on the
//! `gmaster` module, mirroring the CLI structure.

use std::path::Path;

use superx_kernel::{Kernel, KernelError, Result};

mod extract;
mod metamodel;
mod substrate;

pub use metamodel::seed_gmaster_metamodel;
pub use substrate::InferStats;

/// Extract a knowledge graph from every Rust file under `path` and
/// persist it into the substrate via kernel verbs.
///
/// Idempotently seeds the gmaster metamodel on every call, so the
/// caller does not need to invoke `seed_gmaster_metamodel` separately.
///
/// # Errors
///
/// [`KernelError::Db`] for substrate write failures;
/// [`KernelError::NotFound`] if the metamodel seed somehow fails to
/// materialise (the engine refused a `type_definition` CREATE — see
/// engine error for the cause).
pub async fn infer(kernel: &Kernel, path: &Path) -> Result<InferStats> {
    seed_gmaster_metamodel(kernel).await?;

    let graph = extract::extract_rust(path)
        .map_err(|e| KernelError::NotFound(format!("gmaster extract failed: {e}")))?;

    let stats = substrate::persist(kernel, &graph).await?;
    Ok(stats)
}
