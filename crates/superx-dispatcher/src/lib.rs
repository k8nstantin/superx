//! # superx-dispatcher — concrete `Dispatcher` implementations
//!
//! Composes the existing tool blades (`superx-compiler`, `superx-harness`,
//! `superx-ingest`) behind `CapabilityGovernor::check_capability` so the
//! `superx-runner` `RunnerBlade` has a real, capability-gated dispatch
//! path. Both `superx-cli` and `superx-mcp` import [`KernelDispatcher`]
//! so the two binaries share one source-of-truth implementation.
//!
//! ## Routed kinds (v0.1)
//!
//! | schedule `kind` | input shape | blade |
//! | --- | --- | --- |
//! | `compile` | target + run on the schedule row | `CompilerBlade::compile` |
//! | `promote` | threshold from `attr_config.promote_threshold` | `MetaHarness::promote` |
//! | `ingest`  | file path from target's current `attr_desc.text` | `UniversalIngestor::ingest(FileSource)` |
//!
//! Unknown kinds return [`KernelError::Validation`] with a rationale
//! string so the runner records a clean `failed` transition and the
//! root cause surfaces in telemetry instead of silently misbehaving.
//!
//! ## Layering boundaries
//!
//! - **No model calls here.** `KernelDispatcher` is the Runner's
//!   mechanical executor (per ARCHITECTURE.md §0c-2). Scheduling
//!   judgement lives in the future `SchedulerBlade`.
//! - **No raw substrate writes.** All persistence flows through the
//!   blades' kernel verbs under the `superx` service account.
//! - **No schema changes.** Routed kinds reuse existing metamodel
//!   types (`attr_desc`, `attr_config`). New kinds requiring richer
//!   attributes (`evaluate` → `attr_evaluator_wasm`) live behind a §7
//!   operator-approval gate.
//!
//! Copyright (c) 2026 Constantin Alexander <constantin@dedomena.io>.
//! Licensed under the Apache License, Version 2.0.

#![deny(warnings)]
#![deny(clippy::pedantic)]

use std::sync::Arc;

use async_trait::async_trait;
use superx_kernel::{Kernel, KernelError};

/// Concrete `Dispatcher` impl shared by the CLI runner subcommand and
/// the MCP background-runner task. See crate-level docs for routed
/// kinds and layering boundaries.
pub struct KernelDispatcher {
    pub kernel: Arc<Kernel>,
    pub agent_id: String,
}

impl KernelDispatcher {
    /// Construct a `KernelDispatcher` for the given kernel + agent
    /// identity. The agent must already exist in the tenant's substrate
    /// and hold `edge_has_capability` to each tool the runner expects
    /// to dispatch.
    #[must_use]
    pub fn new(kernel: Arc<Kernel>, agent_id: String) -> Self {
        Self { kernel, agent_id }
    }
}

#[async_trait]
impl superx_runner::Dispatcher for KernelDispatcher {
    async fn dispatch(
        &self,
        kind: &str,
        target: &surrealdb::sql::Thing,
        run: &surrealdb::sql::Thing,
    ) -> Result<(), KernelError> {
        // Capability gate first — every dispatch is governor-checked.
        let tool_uid = format!("tool_{kind}");
        let gov = superx_agent::CapabilityGovernor::new(&self.kernel);
        gov.check_capability(&self.agent_id, &tool_uid).await?;

        // Render record-id literals at the dispatch boundary — the
        // blades' v2 APIs all take `&str` record-id literals.
        let target_id = thing_to_record_id_literal(target);
        let run_id = thing_to_record_id_literal(run);

        match kind {
            "compile" => {
                let compiler = superx_compiler::CompilerBlade::new(&self.kernel, None);
                compiler.compile(&target_id, &run_id, None).await.map(|_xml| ())
            }
            "promote" => {
                // Threshold comes from substrate `attr_config.promote_threshold`
                // (bootstrap-seeded, operator-overridable). Per §9 the runtime
                // policy decision lives in the parameter store, not as a
                // hardcoded constant in dispatch code.
                let threshold: f64 = self
                    .kernel
                    .get_parameter("promote_threshold", 0.8_f64)
                    .await;
                let harness = superx_harness::MetaHarness::new(&self.kernel);
                harness.promote(&target_id, threshold).await.map(|_| ())
            }
            "ingest" => {
                // Per ARCHITECTURE.md §8 ("schedule is a dumb queue"), the
                // source path lives on the target entity (a `node_source_external`
                // or similar source-typed node), not on the schedule row.
                // The agent reads the current `attr_desc.text` to get the path.
                #[derive(serde::Deserialize)]
                #[allow(dead_code)]
                struct DescRow {
                    value_json: serde_json::Value,
                    valid_from: chrono::DateTime<chrono::Utc>,
                }
                let attr_desc = self.kernel.type_thing("attr_desc")?;
                let mut res = self
                    .kernel
                    .db
                    .query(
                        "SELECT value_json, valid_from FROM state_ledger \
                         WHERE target = $t AND `type` = $ty \
                         ORDER BY valid_from DESC LIMIT 1",
                    )
                    .bind(("t", target.clone()))
                    .bind(("ty", attr_desc))
                    .await?;
                let row = res.take::<Vec<DescRow>>(0)?.pop().ok_or_else(|| {
                    KernelError::Validation(format!(
                        "ingest dispatcher: target {target_id} has no \
                         attr_desc — the file path must live there per \
                         ARCHITECTURE.md §8"
                    ))
                })?;
                let path = row
                    .value_json
                    .get("text")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        KernelError::Validation(format!(
                            "ingest dispatcher: target {target_id} \
                             attr_desc.text is missing or not a string"
                        ))
                    })?
                    .to_string();

                let ingestor = superx_ingest::UniversalIngestor::new(&self.kernel);
                let source = Box::new(superx_ingest::FileSource { path });
                ingestor.ingest(source, &run_id).await.map(|_root_id| ())
            }
            other => Err(KernelError::Validation(format!(
                "runner dispatcher: kind `{other}` not yet implemented — \
                 needs target-entity attr-resolution (ARCHITECTURE.md §8); \
                 see follow-up PRs"
            ))),
        }
    }
}

/// Render a `Thing` as its canonical `<table>:<uuid>` record-id literal
/// without going through `Display` (which adds backtick escaping for
/// hyphen-bearing UUIDs and breaks `Kernel::parse_id` round-trip).
///
/// Exposed (rather than private) because both the CLI's and MCP's
/// startup code occasionally need the same conversion for log lines.
#[must_use]
pub fn thing_to_record_id_literal(t: &surrealdb::sql::Thing) -> String {
    let id = match &t.id {
        surrealdb::sql::Id::Uuid(u) => u.to_raw(),
        surrealdb::sql::Id::String(s) => s.clone(),
        other => format!("{other:?}"),
    };
    format!("{}:{}", t.tb, id)
}
