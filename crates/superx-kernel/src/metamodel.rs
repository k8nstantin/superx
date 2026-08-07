//! The kernel's required metamodel — `type_definition` uids that MUST
//! exist for the schema's FK ASSERTs to resolve and for the registry,
//! lifecycle, and capture primitives to function.
//!
//! Published as a const slice — discoverable information, not
//! behavior. Seeding happens in [`crate::boot::boot`] (or, in tests,
//! the test fixture). Modules and adapters publish their own
//! `required_metamodel` slices on their descriptors; boot aggregates
//! them all.

/// Spec for one type_definition row the kernel relies on.
#[derive(Debug, Clone, Copy)]
pub struct MetamodelType {
    pub uid: &'static str,
    pub category: &'static str,    // "node" | "edge" | "attribute"
    pub memory_tier: &'static str, // "core" | "working" | "archival" | "recall"
}

/// The kernel's required metamodel — 11 type_definition uids:
///
/// **Identity (FK ASSERT requirements from schema/kernel.surql v2.1):**
/// - `node_agent` — a discovered coding agent (`message.agent`,
///   `telemetry_stream.agent`); the entity row id IS the agent_id
/// - `node_source` — one capturable stream an agent emits
///   (`cursor.subject` for telemetry cursors)
/// - `node_session` — one conversation (`message.session`)
///
/// **Registry (what's installed in this binary):**
/// - `node_kernel_module` — kernel modules registered via linkme
/// - `node_adapter` — per-agent capture adapters
///
/// **Lifecycle & status:**
/// - `attr_module_descriptor` — descriptor payload (name, version, deps)
/// - `attr_module_status` — enabled / disabled (operator intent)
/// - `attr_lifecycle_state` — enabled / starting / active / failed / skipped / disabled
///
/// **Capture identity descriptors:**
/// - `attr_agent_descriptor` — `{name, locator, adapter}` on node_agent
/// - `attr_source_descriptor` — `{name, locator}` on node_source
/// - `attr_session_descriptor` — `{name, locator, …}` on node_session
pub const REQUIRED_METAMODEL_TYPES: &[MetamodelType] = &[
    // Identity
    MetamodelType { uid: "node_agent",             category: "node",      memory_tier: "core" },
    MetamodelType { uid: "node_source",            category: "node",      memory_tier: "core" },
    MetamodelType { uid: "node_session",           category: "node",      memory_tier: "core" },
    // Registry
    MetamodelType { uid: "node_kernel_module",     category: "node",      memory_tier: "core" },
    MetamodelType { uid: "node_adapter",           category: "node",      memory_tier: "core" },
    // Lifecycle & status
    MetamodelType { uid: "attr_module_descriptor", category: "attribute", memory_tier: "working" },
    MetamodelType { uid: "attr_module_status",     category: "attribute", memory_tier: "working" },
    MetamodelType { uid: "attr_lifecycle_state",   category: "attribute", memory_tier: "working" },
    // Capture identity descriptors
    MetamodelType { uid: "attr_agent_descriptor",  category: "attribute", memory_tier: "working" },
    MetamodelType { uid: "attr_source_descriptor", category: "attribute", memory_tier: "working" },
    MetamodelType { uid: "attr_session_descriptor", category: "attribute", memory_tier: "working" },
];
