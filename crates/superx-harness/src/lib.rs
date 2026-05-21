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

/// `MetaHarness`: The evaluation engine for verifiable system evolution.
pub struct MetaHarness<'a> {
    pub kernel: &'a Kernel,
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
        #[derive(serde::Deserialize)]
        #[allow(dead_code)]
        struct ScoreRow {
            value_json: serde_json::Value,
            valid_from: chrono::DateTime<chrono::Utc>,
        }

        // 1. Fetch the proposal's current attr_score. Under v2 "current" is
        //    the most-recent row by `valid_from` (no `is_current` column);
        //    the type FK is a typed `record<type_definition>` Thing, not a
        //    named-id literal; tenant is filtered by the schema PERMISSIONS
        //    so no explicit `tenant_id` predicate is needed at the SELECT.
        let target_thing = Kernel::parse_id(proposal_id)?;
        let attr_score = self.kernel.type_thing("attr_score")?;

        let mut res = self.kernel.db
            .query("SELECT value_json, valid_from FROM state_ledger \
                    WHERE target = $id AND `type` = $ty \
                    ORDER BY valid_from DESC LIMIT 1")
            .bind(("id", target_thing))
            .bind(("ty", attr_score))
            .await?;
        let row = res.take::<Vec<ScoreRow>>(0)?.pop();
        let score = row
            .and_then(|r| r.value_json.get("score").and_then(serde_json::Value::as_f64))
            .unwrap_or(0.0);

        if score >= threshold {
            tracing::info!("Proposal {proposal_id} passed threshold ({score} >= {threshold}). Promoting...");

            // 2. The substrate Thing is exactly the session's `$session_tenant`
            //    — no extra query needed; the kernel's typed accessor returns
            //    it directly.
            let substrate_thing = self.kernel.session_tenant_thing().await?;
            let substrate_id = match &substrate_thing.id {
                surrealdb::sql::Id::Uuid(u) => format!("entity:{}", u.to_raw()),
                surrealdb::sql::Id::String(s) => format!("entity:{s}"),
                other => return Err(KernelError::Integrity(format!(
                    "session substrate id has unexpected form: {other:?}"
                ))),
            };

            // 3. Link proposal to the substrate entity via `edge_promotes`.
            self.kernel.create_structural_edge(proposal_id, &substrate_id, "edge_promotes").await?;
            Ok(true)
        } else {
            tracing::info!("Proposal {proposal_id} failed threshold ({score} < {threshold}).");
            Ok(false)
        }
    }
}
