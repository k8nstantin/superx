//! # superx-runner — `RunnerBlade` (the Terraform-`apply` analog)
//!
//! Implements roadmap #2 (`ARCHITECTURE.md` §8) — the **pure mechanical
//! executor** of the schedule table. Pops a due schedule chain whose
//! current state is `waiting` and walks it through the SCD-2 lifecycle:
//!
//! ```text
//! waiting → scheduled → running → completed | failed
//! ```
//!
//! Each transition is a kernel verb call (`Kernel::transition_schedule_status`)
//! which is engine-enforced append-only and emits a typed
//! `schedule_transitioned` telemetry row per step.
//!
//! ## What `RunnerBlade` does **not** do
//!
//! - **No local-model calls.** Scheduling decisions (retry vs. abandon,
//!   reorder, escalate to HITL) live in the future `SchedulerBlade`
//!   (roadmap #31). The runner is a faithful executor of whatever the
//!   scheduler queued.
//! - **No DAG traversal yet.** The full `petgraph::toposort` + parallel
//!   walk to `attr_config.max_concurrent` lands in a follow-up; this
//!   scaffold proves the SCD-2 transition loop end-to-end.
//! - **No real tool dispatch yet.** The [`Dispatcher`] trait is the seam
//!   where `CapabilityGovernor` + the right tool blade plug in. This
//!   scaffold ships [`NoopDispatcher`] as the default; the wiring into
//!   `superx-cli` / `superx-mcp` background loops + a real dispatcher
//!   implementation arrives in the next PR (task #19).
//!
//! ## Entry points
//!
//! - [`RunnerBlade::new`] / [`RunnerBlade::with_dispatcher`] — construct.
//! - [`RunnerBlade::tick`] — one batch: scan for due waiting chains in
//!   the current session's tenant and process each through the lifecycle.
//!   Returns the number of chains processed so callers can decide
//!   whether to spin-wait or back off.
//!
//! ## SCD-2 chain semantics
//!
//! The `schedule` table is append-only — every status transition is a new
//! row, never an in-place UPDATE. A *chain* is the set of rows that share
//! the same `run` FK; the chain's current state is the most-recent row by
//! `valid_from`. `RunnerBlade` picks up only chains whose most-recent row
//! is `waiting` and whose `due_at` has elapsed.
//!
//! Copyright (c) 2026 Constantin Alexander <constantin@dedomena.io>.
//! Licensed under the Apache License, Version 2.0.

#![deny(warnings)]
#![deny(clippy::pedantic)]

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use surrealdb::sql::Thing;
use superx_kernel::{Kernel, KernelError};

/// The seam where the runner's mechanical loop meets the real
/// `CapabilityGovernor` + tool-blade dispatch. The next PR (task #19)
/// will provide a production implementation; this scaffold ships
/// [`NoopDispatcher`] as the default so the loop is testable in
/// isolation from the dispatch surface.
///
/// Implementations MUST be `Send + Sync` because the runner is intended
/// to be spawned in a `tokio` background task.
#[async_trait]
pub trait Dispatcher: Send + Sync {
    /// Execute one schedule item's effective work — typically: check
    /// capability via `CapabilityGovernor`, dispatch to the named tool
    /// blade, write artifacts back to the substrate as superseded state.
    ///
    /// Receives the schedule row's typed `kind` plus the `target` and
    /// `run` `Thing` FKs. Implementations are free to ignore any of
    /// these (e.g. the [`NoopDispatcher`]) but the trait surface is
    /// stable so concrete dispatchers don't break their `RunnerBlade`
    /// integration when the runner adds context.
    ///
    /// # Errors
    /// Returns any `KernelError` raised by the underlying dispatch
    /// path. The runner translates an `Err` into a `failed` schedule
    /// transition; an `Ok(())` into `completed`.
    async fn dispatch(
        &self,
        kind: &str,
        target: &Thing,
        run: &Thing,
    ) -> Result<(), KernelError>;
}

/// Default dispatcher — logs the dispatch at INFO and returns `Ok(())`.
/// Useful for tests that exercise the SCD-2 loop without depending on a
/// tool registry, and for CLI/MCP boot paths where the operator hasn't
/// yet supplied an agent identity for capability-checked dispatch.
pub struct NoopDispatcher;

#[async_trait]
impl Dispatcher for NoopDispatcher {
    async fn dispatch(
        &self,
        kind: &str,
        target: &Thing,
        run: &Thing,
    ) -> Result<(), KernelError> {
        tracing::info!("noop dispatch: kind={kind} target={target} run={run}");
        Ok(())
    }
}

/// `RunnerBlade`: pure mechanical schedule consumer. See crate-level docs
/// for the lifecycle and design boundaries.
pub struct RunnerBlade<'a> {
    kernel: &'a Kernel,
    dispatcher: Arc<dyn Dispatcher>,
}

impl<'a> RunnerBlade<'a> {
    /// Construct a `RunnerBlade` with the default [`NoopDispatcher`].
    /// Suitable for tests + the v0.1 scaffolding; production wiring uses
    /// [`Self::with_dispatcher`] to inject the real governor + tool
    /// dispatcher.
    #[must_use]
    pub fn new(kernel: &'a Kernel) -> Self {
        Self {
            kernel,
            dispatcher: Arc::new(NoopDispatcher),
        }
    }

    /// Construct a `RunnerBlade` with a caller-supplied [`Dispatcher`].
    #[must_use]
    pub fn with_dispatcher(kernel: &'a Kernel, dispatcher: Arc<dyn Dispatcher>) -> Self {
        Self { kernel, dispatcher }
    }

    /// Process one batch of due waiting chains. Returns the number of
    /// chains that completed at least one transition (whether they ended
    /// in `completed` or `failed`).
    ///
    /// Callers (the CLI / MCP background loops) typically wrap this in
    /// a `loop { tick(); sleep(...) }` driven by an interval read from
    /// `attr_config.runner_tick_ms` — `RunnerBlade` does not own its
    /// own timer because the cadence is a config knob, not a code
    /// constant.
    ///
    /// # Errors
    /// Returns the first `KernelError` raised by a transition or by the
    /// underlying SELECT. Per-row dispatch errors are caught and
    /// translated into a `failed` transition; they do not abort the
    /// batch.
    pub async fn tick(&self) -> Result<u32, KernelError> {
        let chains = self.list_current_waiting().await?;
        let mut processed = 0_u32;
        for chain in chains {
            self.run_chain(chain).await?;
            processed = processed.saturating_add(1);
        }
        Ok(processed)
    }

    /// Find each `(run)`-keyed schedule chain whose most-recent row has
    /// status `waiting` and whose `due_at` has elapsed. The schema does
    /// not have an explicit "chain head" pointer; instead the chain is
    /// reconstructed at read time by `ORDER BY run ASC, valid_from DESC`
    /// and keeping the first row per `run`.
    async fn list_current_waiting(&self) -> Result<Vec<ChainHead>, KernelError> {
        // Compare against SurrealDB's own wall clock — avoids any
        // chrono → SurrealQL datetime bind-coercion surprises.
        let mut res = self
            .kernel
            .db
            .query(
                "SELECT id, run, kind, target, status, valid_from FROM schedule \
                 WHERE tenant = $session_tenant AND due_at <= time::now() \
                 ORDER BY run ASC, valid_from DESC",
            )
            .await?;
        let rows: Vec<ChainHead> = res.take(0)?;

        // Keep the first (most-recent) row per `run`; that's the chain's
        // current state. Then admit only those still in `waiting`. Key
        // the dedup map by `run.to_string()` — `Thing` itself contains
        // interior types that `Hash`-key linters flag as "mutable", and
        // the string form is stable per row id.
        let mut current: HashMap<String, ChainHead> = HashMap::new();
        for row in rows {
            current.entry(row.run.to_string()).or_insert(row);
        }
        Ok(current
            .into_values()
            .filter(|r| r.status == "waiting")
            .collect())
    }

    /// Walk one chain through `waiting → scheduled → running →
    /// completed|failed`. Each transition is a kernel verb call which
    /// emits a `schedule_transitioned` telemetry event.
    async fn run_chain(&self, head: ChainHead) -> Result<(), KernelError> {
        let scheduled = self
            .kernel
            .transition_schedule_status(head.id.clone(), "scheduled")
            .await?;
        let running = self
            .kernel
            .transition_schedule_status(scheduled, "running")
            .await?;

        let outcome_status = match self.dispatcher.dispatch(&head.kind, &head.target, &head.run).await {
            Ok(()) => "completed",
            Err(e) => {
                tracing::warn!(
                    "dispatch failed for kind={} target={} run={}: {e}",
                    head.kind,
                    head.target,
                    head.run
                );
                "failed"
            }
        };
        let _final_row = self
            .kernel
            .transition_schedule_status(running, outcome_status)
            .await?;
        Ok(())
    }
}

/// One row of the schedule scan — just enough to identify a chain's
/// current state and dispatch its work.
#[derive(Deserialize)]
struct ChainHead {
    id: Thing,
    run: Thing,
    kind: String,
    target: Thing,
    status: String,
    #[allow(dead_code)] // surfaces only via `ORDER BY valid_from`
    valid_from: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use superx_kernel::Kernel;
    use surrealdb::sql::Id;
    use tempfile::tempdir;

    async fn setup() -> (tempfile::TempDir, Kernel) {
        let _ = tracing_subscriber::fmt::try_init();
        let dir = tempdir().unwrap();
        let kernel = Kernel::init(&dir.path().join("superx.db"), "test", "test")
            .await
            .unwrap();
        (dir, kernel)
    }

    async fn provision_tenant(kernel: &Kernel, tenant: &str) -> String {
        let ns = uuid::Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let substrate_uuid = uuid::Uuid::new_v5(&ns, tenant.as_bytes());
        kernel
            .set_session_auth(&substrate_uuid.to_string(), "admin")
            .await
            .unwrap();
        let node_substrate = kernel.type_thing("node_substrate").unwrap();
        let substrate_thing = Thing::from((
            "entity",
            Id::Uuid(surrealdb::sql::Uuid::from(substrate_uuid)),
        ));
        kernel
            .db
            .query(
                "CREATE entity CONTENT { \
                    id: $id, type: $type, tenant: NONE, role: 'admin' \
                }",
            )
            .bind(("id", substrate_thing))
            .bind(("type", node_substrate))
            .await
            .unwrap()
            .check()
            .unwrap();
        format!("entity:{substrate_uuid}")
    }

    async fn make_entity(kernel: &Kernel, type_uid: &str) -> Thing {
        let thing = Thing::from((
            "entity",
            Id::Uuid(surrealdb::sql::Uuid::from(uuid::Uuid::now_v7())),
        ));
        let type_thing = kernel.type_thing(type_uid).unwrap();
        let tenant = kernel.session_tenant_thing().await.unwrap();
        kernel
            .db
            .query(
                "CREATE entity CONTENT { \
                    id: $id, type: $type, tenant: $tenant, role: 'user' \
                }",
            )
            .bind(("id", thing.clone()))
            .bind(("type", type_thing))
            .bind(("tenant", tenant))
            .await
            .unwrap()
            .check()
            .unwrap();
        thing
    }

    #[tokio::test]
    async fn tick_processes_a_single_waiting_chain_to_completed() {
        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct Row {
            status: String,
            valid_from: DateTime<Utc>,
        }

        let (_dir, kernel) = setup().await;
        let _sub = provision_tenant(&kernel, "runner_basic").await;
        let run = make_entity(&kernel, "node_run").await;
        let target = make_entity(&kernel, "node_prod").await;

        let _sid = kernel
            .enqueue_schedule_item(
                run.clone(),
                "noop",
                target.clone(),
                Utc::now(),
                vec![],
                serde_json::json!({}),
            )
            .await
            .unwrap();

        let runner = RunnerBlade::new(&kernel);
        let processed = runner.tick().await.unwrap();
        assert_eq!(processed, 1, "tick must consume exactly one due chain");

        // History: waiting (enqueue) → scheduled → running → completed.
        let mut res = kernel
            .db
            .query(
                "SELECT status, valid_from FROM schedule \
                 WHERE run = $run AND tenant = $session_tenant \
                 ORDER BY valid_from ASC",
            )
            .bind(("run", run))
            .await
            .unwrap();
        let rows: Vec<Row> = res.take(0).unwrap();
        let statuses: Vec<&str> = rows.iter().map(|r| r.status.as_str()).collect();
        assert_eq!(
            statuses,
            ["waiting", "scheduled", "running", "completed"],
            "the full SCD-2 chain must be appended in order"
        );
    }

    #[tokio::test]
    async fn tick_skips_chains_not_yet_due() {
        let (_dir, kernel) = setup().await;
        let _sub = provision_tenant(&kernel, "runner_future").await;
        let run = make_entity(&kernel, "node_run").await;
        let target = make_entity(&kernel, "node_prod").await;

        let future = Utc::now() + chrono::Duration::hours(1);
        kernel
            .enqueue_schedule_item(
                run.clone(),
                "noop",
                target,
                future,
                vec![],
                serde_json::json!({}),
            )
            .await
            .unwrap();

        let runner = RunnerBlade::new(&kernel);
        let processed = runner.tick().await.unwrap();
        assert_eq!(processed, 0, "future-dated chain must not be picked up");
    }

    #[tokio::test]
    async fn tick_does_not_reprocess_an_already_running_chain() {
        let (_dir, kernel) = setup().await;
        let _sub = provision_tenant(&kernel, "runner_idem").await;
        let run = make_entity(&kernel, "node_run").await;
        let target = make_entity(&kernel, "node_prod").await;

        kernel
            .enqueue_schedule_item(
                run,
                "noop",
                target,
                Utc::now(),
                vec![],
                serde_json::json!({}),
            )
            .await
            .unwrap();

        let runner = RunnerBlade::new(&kernel);
        let first = runner.tick().await.unwrap();
        let second = runner.tick().await.unwrap();
        assert_eq!(first, 1);
        assert_eq!(second, 0, "second tick must find no chain still in `waiting`");
    }

    #[tokio::test]
    async fn dispatch_error_transitions_chain_to_failed() {
        struct FailingDispatcher {
            calls: AtomicU32,
        }
        #[async_trait]
        impl Dispatcher for FailingDispatcher {
            async fn dispatch(
                &self,
                _kind: &str,
                _target: &Thing,
                _run: &Thing,
            ) -> Result<(), KernelError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Err(KernelError::Validation("synthetic dispatch error".into()))
            }
        }

        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct Row {
            status: String,
            valid_from: DateTime<Utc>,
        }

        let (_dir, kernel) = setup().await;
        let _sub = provision_tenant(&kernel, "runner_fail").await;
        let run = make_entity(&kernel, "node_run").await;
        let target = make_entity(&kernel, "node_prod").await;

        kernel
            .enqueue_schedule_item(
                run.clone(),
                "boom",
                target,
                Utc::now(),
                vec![],
                serde_json::json!({}),
            )
            .await
            .unwrap();

        let dispatcher = Arc::new(FailingDispatcher {
            calls: AtomicU32::new(0),
        });
        let runner = RunnerBlade::with_dispatcher(&kernel, dispatcher.clone());
        runner.tick().await.unwrap();
        assert_eq!(dispatcher.calls.load(Ordering::SeqCst), 1);

        let mut res = kernel
            .db
            .query(
                "SELECT status, valid_from FROM schedule \
                 WHERE run = $run AND tenant = $session_tenant \
                 ORDER BY valid_from DESC LIMIT 1",
            )
            .bind(("run", run))
            .await
            .unwrap();
        let latest: Row = res.take::<Vec<Row>>(0).unwrap().pop().unwrap();
        assert_eq!(latest.status, "failed");
    }
}
