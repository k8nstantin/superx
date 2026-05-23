//! Substrate adapter — takes the in-memory typed graph produced by
//! [`crate::extract`] and persists every node + edge through the
//! kernel verbs. Every entity gets an `attr_desc` row carrying its
//! name + file path so downstream consumers (CompilerBlade,
//! SchedulerBlade, ask-verb) can recover human-readable context
//! via [`Kernel::current_state`].
//!
//! This is the only place in the crate that issues substrate writes.
//! Keeping it isolated means `extract` stays a pure transformation
//! and `pipeline` stays purely an orchestrator.

use surrealdb::types::{RecordId, Value};
use superx_kernel::{Kernel, Result};

use crate::extract::{ExtractedGraph, GmEdge, GmNode};

/// Stats returned by [`crate::infer`] after a complete run. Useful
/// for the CLI's success line and for downstream consumers that want
/// a quick summary without re-querying the substrate.
#[derive(Debug, Default, Clone)]
pub struct InferStats {
    pub files: usize,
    pub functions: usize,
    pub classes: usize,
    pub modules: usize,
    pub edges_defines: usize,
    pub edges_imports: usize,
}

pub(crate) async fn persist(
    kernel: &Kernel,
    graph: &ExtractedGraph,
) -> Result<InferStats> {
    let mut stats = InferStats::default();

    // First pass: create every entity and remember its substrate
    // RecordId at the same index as the in-memory node.
    let mut entity_ids: Vec<RecordId> = Vec::with_capacity(graph.nodes.len());
    for node in &graph.nodes {
        let id = kernel
            .create_entity(node.kind.type_uid(), "admin")
            .await?;
        // Attach attr_desc with the human-readable context so the
        // substrate isn't just opaque-UUID rows.
        kernel
            .supersede_state(id.clone(), "attr_desc", node_desc_payload(node))
            .await?;
        entity_ids.push(id);
        count_node(&mut stats, node);
    }

    // Second pass: create every relation, using the substrate ids we
    // just built. (Edges reference nodes by in-memory index, so we
    // resolve via entity_ids[edge.from] / entity_ids[edge.to].)
    for edge in &graph.edges {
        let in_id = entity_ids[edge.from].clone();
        let out_id = entity_ids[edge.to].clone();
        kernel
            .create_relation(in_id, out_id, edge.kind.type_uid(), true)
            .await?;
        count_edge(&mut stats, edge);
    }

    // Final telemetry summary so an operator running `superx stats`
    // sees the run as one row alongside all the per-node /
    // per-edge events.
    kernel
        .log_telemetry(
            "gmaster_infer_complete",
            infer_complete_payload(&stats),
            None,
        )
        .await?;

    Ok(stats)
}

fn node_desc_payload(node: &GmNode) -> Value {
    let mut obj = surrealdb::types::Object::new();
    obj.insert(
        "text".to_string(),
        Value::String(node.name.clone()),
    );
    obj.insert(
        "file".to_string(),
        Value::String(node.file.display().to_string()),
    );
    obj.insert(
        "start_byte".to_string(),
        Value::Number((node.start_byte as i64).into()),
    );
    obj.insert(
        "end_byte".to_string(),
        Value::Number((node.end_byte as i64).into()),
    );
    Value::Object(obj)
}

fn infer_complete_payload(stats: &InferStats) -> Value {
    let mut obj = surrealdb::types::Object::new();
    obj.insert("files".to_string(), Value::Number((stats.files as i64).into()));
    obj.insert("functions".to_string(), Value::Number((stats.functions as i64).into()));
    obj.insert("classes".to_string(), Value::Number((stats.classes as i64).into()));
    obj.insert("modules".to_string(), Value::Number((stats.modules as i64).into()));
    obj.insert("edges_defines".to_string(), Value::Number((stats.edges_defines as i64).into()));
    obj.insert("edges_imports".to_string(), Value::Number((stats.edges_imports as i64).into()));
    Value::Object(obj)
}

fn count_node(stats: &mut InferStats, node: &GmNode) {
    match node.kind {
        crate::extract::GmNodeKind::CodeFile => stats.files += 1,
        crate::extract::GmNodeKind::Function => stats.functions += 1,
        crate::extract::GmNodeKind::Class => stats.classes += 1,
        crate::extract::GmNodeKind::Module => stats.modules += 1,
    }
}

fn count_edge(stats: &mut InferStats, edge: &GmEdge) {
    match edge.kind {
        crate::extract::GmEdgeKind::Defines => stats.edges_defines += 1,
        crate::extract::GmEdgeKind::Imports => stats.edges_imports += 1,
    }
}
