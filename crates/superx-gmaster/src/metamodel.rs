//! Per-crate metamodel — the `type_definition` rows gmaster owns.
//!
//! `seed_gmaster_metamodel` calls `Kernel::ensure_type_definition` for
//! each entry, idempotently — safe to invoke on every `gmaster infer`
//! run. Existing rows are detected by uid and skipped.
//!
//! Keeping these here (rather than in `superx-kernel`'s
//! `METAMODEL_TYPES`) keeps the kernel narrow per the operator's
//! "per-crate metamodel" standing decision.

use superx_kernel::{Kernel, Result};

struct GmasterType {
    uid: &'static str,
    category: &'static str,
    memory_tier: &'static str,
}

const GMASTER_METAMODEL_TYPES: &[GmasterType] = &[
    // Node types — the building blocks of an inferred codebase graph.
    GmasterType { uid: "node_code_file", category: "node", memory_tier: "working" },
    GmasterType { uid: "node_function",  category: "node", memory_tier: "working" },
    GmasterType { uid: "node_class",     category: "node", memory_tier: "working" },
    GmasterType { uid: "node_module",    category: "node", memory_tier: "working" },
    // Edge types — relationships between code nodes.
    GmasterType { uid: "edge_defines",   category: "edge", memory_tier: "working" },
    GmasterType { uid: "edge_imports",   category: "edge", memory_tier: "working" },
    GmasterType { uid: "edge_calls",     category: "edge", memory_tier: "working" },
    // Attribute types — what we write into state_ledger for each
    // node. `attr_desc` is also seeded by the kernel's bootstrap;
    // `ensure_type_definition` is idempotent so listing it here too
    // makes the crate self-sufficient when called outside the
    // `superx bootstrap` flow (e.g. integration tests, MCP, etc.).
    GmasterType { uid: "attr_desc",      category: "attribute", memory_tier: "working" },
];

/// Idempotently seed every `type_definition` row gmaster relies on.
pub async fn seed_gmaster_metamodel(kernel: &Kernel) -> Result<()> {
    for spec in GMASTER_METAMODEL_TYPES {
        kernel
            .ensure_type_definition(spec.uid, spec.category, spec.memory_tier)
            .await?;
    }
    Ok(())
}
