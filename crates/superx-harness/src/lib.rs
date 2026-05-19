//! # superx-harness — wasmtime fuel-metered Meta-Harness
//!
//! Implements the **verifiable proposal-scoring** primitive
//! (`ARCHITECTURE.md` §0c Cognitive Governance). Proposals (code, prompts,
//! model versions) are scored by running an operator-supplied wasm module
//! inside a wasmtime sandbox with strict fuel metering. A fuel-exhausted
//! module traps as `KernelError::SafetyViolation` — the proposal is rejected
//! rather than allowed to consume unbounded compute.
//!
//! ## Entry points
//!
//! - [`MetaHarness::evaluate`] — load wasm, set fuel budget (10 000 units
//!   today; will be a parameter in Roadmap #1b), call the `run_test`
//!   export, divide the returned `i32` by 100 to derive a `[0.0..1.0]`
//!   score, persist the score as `attr_score` on the proposal entity.
//! - [`MetaHarness::promote`] — read the current `attr_score`; if it clears
//!   the threshold, link the proposal to the tenant's substrate entity via
//!   `edge_promotes`. This is the substrate-side promotion step that turns
//!   "candidate" into "adopted."
//!
//! ## Design notes
//!
//! - **Fuel budget is hardcoded at 10 000.** Roadmap #1b moves this to
//!   `execution_params` so per-task tuning is a substrate write.
//! - **Wasm guest exports `run_test() -> i32` and that's it.** The harness
//!   does not provide host functions today; the wasm is pure-compute. When
//!   Roadmap #20 (Component Model) lands, the harness will offer
//!   capability-typed host imports the wasm can declare it needs.
//! - **Promote resolves the substrate dynamically** via a `SurrealQL` query on
//!   `state_ledger` rather than a hardcoded literal — promotions follow the
//!   tenant's current substrate entity, no matter how it was provisioned.
//!
//! Copyright (c) 2026 Constantin Alexander <constantin@dedomena.io>.
//! Licensed under the Apache License, Version 2.0.

#![deny(warnings)]
#![deny(clippy::pedantic)]

use superx_kernel::{Kernel, KernelError};
use wasmtime::{Engine, Module, Store, Linker, Config};
use serde_json::json;
use surrealdb::sql::Thing;

/// `MetaHarness`: The evaluation engine for verifiable system evolution.
pub struct MetaHarness<'a> {
    pub kernel: &'a Kernel,
}

#[derive(serde::Deserialize)] 
struct SubRes { 
    target: Thing 
}

impl<'a> MetaHarness<'a> {
    /// Creates a new `MetaHarness`.
    #[must_use]
    pub fn new(kernel: &'a Kernel) -> Self {
        Self { kernel }
    }

    /// `evaluate`: Sandboxed evaluation of a proposal node.
    ///
    /// # Errors
    /// Returns `KernelError::SafetyViolation` if fuel is exhausted.
    pub async fn evaluate(&self, proposal_id: &str, harness_wasm: &[u8]) -> Result<f64, KernelError> {
        tracing::info!("evaluating proposal {proposal_id} (wasm {} bytes)", harness_wasm.len());
        let mut config = Config::new();
        config.consume_fuel(true);
        let engine = Engine::new(&config).map_err(|e| KernelError::Integrity(e.to_string()))?;
        let module = Module::new(&engine, harness_wasm).map_err(|e| KernelError::Validation(e.to_string()))?;
        
        let mut store_obj = Store::new(&engine, 10_000); // 10k fuel limit
        store_obj.set_fuel(10_000).map_err(|e| KernelError::SafetyViolation(e.to_string()))?;

        let linker = Linker::new(&engine);
        let instance = linker.instantiate(&mut store_obj, &module).map_err(|e| KernelError::Integrity(e.to_string()))?;

        let run_test = instance.get_typed_func::<(), i32>(&mut store_obj, "run_test").map_err(|e| KernelError::Validation(e.to_string()))?;
        
        let score_raw = run_test.call(&mut store_obj, ()).map_err(|e| KernelError::SafetyViolation(e.to_string()))?;
        let final_score = f64::from(score_raw) / 100.0;

        // Record the score (Siloed by session auth)
        self.kernel.supersede_state(
            proposal_id, 
            "attr_score", 
            json!({"score": final_score}), 
            None
        ).await?;

        Ok(final_score)
    }

    /// `promote`: Promotes a proposal if it passes a certain score threshold.
    ///
    /// # Errors
    /// Returns `KernelError::Validation` if `proposal_id` is malformed; `KernelError::Database`
    /// if the substrate query fails; `KernelError::Integrity` if the tenant substrate is missing.
    pub async fn promote(&self, proposal_id: &str, threshold: f64) -> Result<bool, KernelError> {
        // 1. Fetch current score within SAME tenant. Parse the proposal id via the kernel
        //    helper so we reject malformed inputs at the boundary rather than silently
        //    constructing a phantom Thing.
        let target_thing = Kernel::parse_id(proposal_id)?;
        let query = "SELECT value_json, is_current, tenant_id FROM state_ledger WHERE target = $id AND `type` = type_definition:attr_score";
        let mut res = self.kernel.db.query(query)
            .bind(("id", target_thing.clone())).await?;
        
        let all_records: Vec<serde_json::Value> = res.take(0)?;
        tracing::debug!("raw attr_score rows for {proposal_id}: {all_records:?}");

        let mut score = 0.0;
        for rec in all_records {
            if rec.get("is_current").and_then(serde_json::Value::as_bool).unwrap_or(false) {
                if let Some(val) = rec.get("value_json") {
                    if let Some(s_val) = val.get("score").and_then(serde_json::Value::as_f64) {
                        score = s_val;
                        break;
                    }
                }
            }
        }

        if score >= threshold {
            tracing::info!("Proposal {proposal_id} passed threshold ({score} >= {threshold}). Promoting...");
            
            // 2. Resolve the tenant's substrate entity (UUIDv7) from the session
            let sub_query = "SELECT target FROM state_ledger WHERE target.type = type_definition:node_substrate AND tenant_id = $session_tenant AND is_current = true LIMIT 1";
            let mut sub_res = self.kernel.db.query(sub_query).await?;
            let substrate = sub_res.take::<Vec<SubRes>>(0)?.pop().map(|r| r.target.to_string()).ok_or_else(|| KernelError::Integrity("Substrate entity not found for tenant session".into()))?;

            // 3. Link proposal to the real substrate entity
            self.kernel.create_structural_edge(proposal_id, &substrate, "edge_promotes").await?;
            Ok(true)
        } else {
            tracing::info!("Proposal {proposal_id} failed threshold ({score} < {threshold}).");
            Ok(false)
        }
    }
}
