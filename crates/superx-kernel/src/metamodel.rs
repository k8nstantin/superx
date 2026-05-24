//! The kernel's required metamodel — type_definition uids that MUST
//! exist for the schema's FK ASSERTs to resolve + for the registry +
//! lifecycle primitives to function.
//!
//! These are PUBLISHED as a const slice — discoverable information,
//! not behavior. The kernel itself does NOT seed them; that's
//! `superx-kernel-bootstrap`'s job (or, in tests, the test fixture's).
//!
//! Other layers (kernel modules, drivers, apps) publish their own
//! per-crate REQUIRED_METAMODEL_TYPES slices; the orchestrator
//! aggregates them all at boot time.

/// Spec for one type_definition row the kernel relies on.
#[derive(Debug, Clone, Copy)]
pub struct MetamodelType {
    pub uid: &'static str,
    pub category: &'static str,    // "node" | "edge" | "attribute"
    pub memory_tier: &'static str, // "core" | "working" | "archival" | "recall"
}

/// The kernel's required metamodel — 11 type_definition uids:
///
/// **Identity (FK ASSERT requirements from schema/kernel.surql):**
/// - `node_run` — workflow run identity (execution_params.run, schedule.run, telemetry_stream.run)
/// - `node_agent` — agent identity (execution_params.agent)
/// - `node_source` — telemetry capture source (cursor.subject when category='telemetry')
///
/// **Registry (what's installed in this binary):**
/// - `node_kernel_module` — entries for kernel modules registered via linkme
/// - `node_contribution` — entries for drivers / apps / future categories
///
/// **Lifecycle & status:**
/// - `attr_module_descriptor` — descriptor payload (name, version, deps, kind)
/// - `attr_module_status` — enabled / disabled
/// - `attr_lifecycle_state` — Enabled / Starting / Active / Failed / Skipped / Disabled
///
/// **Contribution-specific (for drivers / apps / future categories):**
/// - `attr_contribution_category` — open-ended category string (`driver`, `app`, `model_integration`, `quantum_module`, …) — NOT hardcoded into table structure
/// - `attr_contribution_database` — the database name the contribution owns
/// - `attr_contribution_schema_dag` — the contribution's DAG mirror (compiled from its own DB)
pub const REQUIRED_METAMODEL_TYPES: &[MetamodelType] = &[
    // Identity
    MetamodelType { uid: "node_run",                       category: "node",      memory_tier: "core" },
    MetamodelType { uid: "node_agent",                     category: "node",      memory_tier: "core" },
    MetamodelType { uid: "node_source",                    category: "node",      memory_tier: "core" },
    // Registry
    MetamodelType { uid: "node_kernel_module",             category: "node",      memory_tier: "core" },
    MetamodelType { uid: "node_contribution",              category: "node",      memory_tier: "core" },
    // Lifecycle & status (used by every layer)
    MetamodelType { uid: "attr_module_descriptor",         category: "attribute", memory_tier: "working" },
    MetamodelType { uid: "attr_module_status",             category: "attribute", memory_tier: "working" },
    MetamodelType { uid: "attr_lifecycle_state",           category: "attribute", memory_tier: "working" },
    // Contribution-specific
    MetamodelType { uid: "attr_contribution_category",     category: "attribute", memory_tier: "working" },
    MetamodelType { uid: "attr_contribution_database",     category: "attribute", memory_tier: "working" },
    MetamodelType { uid: "attr_contribution_schema_dag",   category: "attribute", memory_tier: "working" },
];
