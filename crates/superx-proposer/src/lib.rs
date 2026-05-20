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
//! - **Allowed edge-type list is metamodel-derived** under v2 — the proposer
//!   reads every `type_definition` row with `category = 'edge'` at the
//!   start of each call and uses it as the whitelist + the fallback. Adding
//!   a new edge type is a substrate write (`CREATE type_definition CONTENT
//!   { uid: 'edge_new_kind', category: 'edge', ... }`), not a code change.
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
        run_id: &str,
    ) -> Result<String, KernelError> {
        assert!(!from_id.is_empty(), "Source ID mandatory");
        assert!(!to_id.is_empty(), "Target ID mandatory");

        tracing::info!("Agent proposing relation between {from_id} and {to_id}");

        // 0. Read the allowed edge-type whitelist from the metamodel. Adding
        //    a new edge type is a substrate write (CREATE type_definition
        //    CONTENT {uid:'edge_…', category:'edge', …}) — never a code
        //    change. Reads come back as the row's uid so we can match the
        //    model's text output.
        #[derive(serde::Deserialize)]
        struct UidRow { uid: String }
        let mut allowed_res = self.kernel.db
            .query("SELECT uid FROM type_definition WHERE category = 'edge'")
            .await?;
        let allowed: Vec<String> = allowed_res
            .take::<Vec<UidRow>>(0)?
            .into_iter()
            .map(|r| r.uid)
            .collect();
        if allowed.is_empty() {
            return Err(KernelError::Integrity("no edge types seeded in type_definition".into()));
        }
        let fallback_edge = allowed.iter()
            .find(|u| u.as_str() == "edge_semantic")
            .cloned()
            .unwrap_or_else(|| allowed[0].clone());

        // 1. Fetch contexts (Tier-aware)
        let from_xml = self.kernel.compile_context(from_id, run_id, None).await?;
        let to_xml = self.kernel.compile_context(to_id, run_id, None).await?;

        // 2. Local inference. Prompt enumerates the live whitelist so the
        //    model knows the universe at any point in the substrate's life.
        let mut engine = self.inference.lock().await;
        let allowed_csv = allowed.join(", ");
        let prompt = format!(
            "Analyze these two components and decide the structural relation between A and B.\n\n\
            Component A:\n{from_xml}\n\n\
            Component B:\n{to_xml}\n\n\
            Respond with ONLY one of: [{allowed_csv}]"
        );

        let proposal_type = engine.predict(&prompt, 10)
            .map_err(|e| KernelError::Validation(format!("inference failed: {e}")))?;
        let sanitized_type = proposal_type.trim().to_lowercase();
        let final_type = if allowed.iter().any(|u| u == &sanitized_type) {
            sanitized_type
        } else {
            fallback_edge.clone()
        };

        // 3. Create the proposal entity under v2: typed FKs, pure INSERT,
        //    Id::Uuid(UUIDv7), no UPSERT. Engine refuses anything else
        //    under the superx service account.
        let node_proposal = self.kernel.type_thing("node_proposal")?;
        let session_tenant_str = self.kernel.session_tenant().await?;
        let tenant_thing = Kernel::parse_id(&format!("entity:{session_tenant_str}"))?;

        let proposal_uuid = uuid::Uuid::now_v7();
        let proposal_thing = surrealdb::sql::Thing::from((
            "entity",
            surrealdb::sql::Id::Uuid(surrealdb::sql::Uuid::from(proposal_uuid)),
        ));
        let proposal_record_id = format!("entity:{proposal_uuid}");

        self.kernel.db.query(
            "CREATE entity CONTENT { \
                id: $id, \
                type: $type, \
                tenant: $tenant, \
                role: 'user' \
            }"
        )
            .bind(("id", proposal_thing))
            .bind(("type", node_proposal))
            .bind(("tenant", tenant_thing))
            .await?.check()?;

        self.kernel.supersede_state(
            &proposal_record_id,
            "attr_desc",
            json!({"text": format!("Proposed {final_type} between {from_id} and {to_id}")}),
            None
        ).await?;

        // 4. Link proposal to the participants
        self.kernel.create_structural_edge(&proposal_record_id, from_id, "edge_owns").await?;
        self.kernel.create_structural_edge(&proposal_record_id, to_id, "edge_owns").await?;

        // 5. Log decision
        self.kernel.log_telemetry(
            json!({"from": from_id, "to": to_id, "type": final_type, "proposal": proposal_record_id}),
            "relation_proposed",
            None
        ).await?;

        Ok(proposal_record_id)
    }
}
