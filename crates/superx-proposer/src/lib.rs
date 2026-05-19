//! # superx-proposer — LLM-driven structural-edge proposer
//!
//! On-demand variant of the edge-proposer pattern. Given two entity ids, calls
//! the local inference engine to decide whether the relation is `edge_owns`,
//! `edge_implements`, or `edge_semantic`, and writes a `node_proposal` row
//! that the Meta-Harness can later score and (if accepted) promote.
//!
//! The future background variant `EdgeProposerBlade` (Roadmap #25 in
//! `ARCHITECTURE.md` §8) will subscribe to `telemetry_stream` and propose
//! edges continuously without needing an operator to trigger them.
//!
//! ## Entry point
//!
//! [`ProposerBlade::propose_relation`] — analyses two nodes via inference,
//! creates a `node_proposal` entity, links it to both participants, and emits
//! a `relation_proposed` telemetry event.
//!
//! ## Design notes
//!
//! - **Allowed edge-type list is hardcoded** in `propose_relation` today; a
//!   future commit will derive it from a metamodel query so adding a new
//!   edge type is a substrate write, not a code change (audit finding M6).
//! - **Inference failure surfaces as `KernelError::Validation`** — not
//!   `SafetyViolation`. The safety-violation taxonomy is reserved for
//!   NASA-rule-bounded-loop violations and tenant-coercion attempts.
//!
//! Copyright (c) 2026 Constantin Alexander <constantin@dedomena.io>.
//! Licensed under the Apache License, Version 2.0.

#![deny(warnings)]
#![deny(clippy::pedantic)]

use superx_kernel::{Kernel, KernelError};
use superx_inference::InferenceEngine;
use std::sync::Arc;
use tokio::sync::Mutex;
use serde_json::json;

/// `ProposerBlade`: The logic for autonomous relationship mapping.
pub struct ProposerBlade<'a> {
    pub kernel: &'a Kernel,
    pub inference: Arc<Mutex<InferenceEngine>>,
}

impl<'a> ProposerBlade<'a> {
    /// Creates a new `ProposerBlade`.
    #[must_use]
    pub fn new(kernel: &'a Kernel, inference: Arc<Mutex<InferenceEngine>>) -> Self {
        Self { kernel, inference }
    }

    /// `propose_relation`: Analyzes two nodes and proposes a structural edge.
    ///
    /// # Panics
    /// Panics if `from_id` or `to_id` are empty.
    ///
    /// # Errors
    /// Returns `KernelError` if distillation or substrate update fails.
    pub async fn propose_relation(
        &self, 
        from_id: &str, 
        to_id: &str, 
        run_id: &str
    ) -> Result<String, KernelError> {
        assert!(!from_id.is_empty(), "Source ID mandatory");
        assert!(!to_id.is_empty(), "Target ID mandatory");

        tracing::info!("Agent proposing relation between {from_id} and {to_id}");

        // 1. Fetch contexts (Tier-aware)
        let from_xml = self.kernel.compile_context(from_id, run_id, None).await?;
        let to_xml = self.kernel.compile_context(to_id, run_id, None).await?;

        // 2. Local Inference: Ask agent to decide relation type
        let mut engine = self.inference.lock().await;
        let prompt = format!(
            "Analyze these two components and decide if Component A owns, implements, or is semantically related to Component B.\n\n\
            Component A:\n{from_xml}\n\n\
            Component B:\n{to_xml}\n\n\
            Respond with ONLY one of: [edge_owns, edge_implements, edge_semantic]"
        );

        // Inference failures are *validation* failures (the model failed to produce
        // a usable answer), not NASA-rule-bounded-loop safety violations. Conflating
        // them dilutes the safety-violation taxonomy used by Mandate-1 alerting.
        let proposal_type = engine.predict(&prompt, 10)
            .map_err(|e| KernelError::Validation(format!("inference failed: {e}")))?;
        let sanitized_type = proposal_type.trim().to_lowercase();
        
        // Default to semantic if response is malformed
        let final_type = if ["edge_owns", "edge_implements", "edge_semantic"].contains(&sanitized_type.as_str()) {
            sanitized_type
        } else {
            "edge_semantic".to_string()
        };

        // 3. Create the proposal node (UUIDv7)
        let proposal_uuid = uuid::Uuid::now_v7().to_string();
        let proposal_record_id = format!("entity:{proposal_uuid}");
        
        self.kernel.db.query("INSERT INTO entity { id: type::thing($id), tenant_id: $session_tenant, type: type_definition:node_proposal }")
            .bind(("id", proposal_record_id.clone())).await?.check()?;

        self.kernel.supersede_state(
            &proposal_record_id, 
            "attr_desc", 
            json!({"text": format!("Proposed {final_type} between {from_id} and {to_id}")}), 
            Some(run_id.to_string())
        ).await?;

        // 4. Link proposal to the participants
        self.kernel.create_structural_edge(&proposal_record_id, from_id, "edge_owns").await?;
        self.kernel.create_structural_edge(&proposal_record_id, to_id, "edge_owns").await?;

        // 5. Log decision
        self.kernel.log_telemetry(
            json!({"from": from_id, "to": to_id, "type": final_type, "proposal": proposal_record_id}),
            "relation_proposed",
            Some(run_id.to_string())
        ).await?;

        Ok(proposal_record_id)
    }
}
