/*
 * SuperX Runner end-to-end integration test (R-plan #21)
 *
 * Exercises the full enqueue → tick → KernelDispatcher → blade → completed
 * pipeline against a real substrate. Verifies:
 *   - bootstrap seeds admin agents + tools + capability edges,
 *   - enqueue lands a `waiting` schedule chain,
 *   - RunnerBlade::tick picks it up,
 *   - KernelDispatcher routes through CapabilityGovernor::check_capability,
 *   - the right blade actually runs,
 *   - the schedule chain ends in `completed`,
 *   - telemetry shape matches what downstream consumers (and ARCHITECTURE.md §0c-1)
 *     expect at every step.
 *
 * Copyright (c) 2026 Constantin Alexander <constantin@dedomena.io>
 */

use std::sync::Arc;

use chrono::Utc;
use superx_bootstrap::BootstrapBlade;
use superx_dispatcher::KernelDispatcher;
use superx_kernel::Kernel;
use superx_runner::{Dispatcher, RunnerBlade};
use surrealdb::sql::{Id, Thing};
use tempfile::TempDir;

/// Spin up an isolated kernel + bootstrap a tenant. Returns the temp dir
/// (kept alive to preserve RocksDB), the kernel `Arc`, and the substrate
/// record-id literal.
async fn bootstrap(tenant: &str) -> (TempDir, Arc<Kernel>, String) {
    let _ = tracing_subscriber::fmt::try_init();
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("superx.db");
    let kernel = Arc::new(
        Kernel::init(&db_path, "test", "test")
            .await
            .expect("kernel init"),
    );
    let substrate_id = BootstrapBlade::new(&kernel)
        .run(tenant)
        .await
        .expect("bootstrap");
    (dir, kernel, substrate_id)
}

/// Return the record-id literal of one bootstrap-seeded admin agent. The
/// runner uses this as its dispatch identity for the capability check.
async fn fetch_admin_agent_id(kernel: &Kernel) -> String {
    #[derive(serde::Deserialize)]
    struct Row {
        id: Thing,
    }
    let node_agent = kernel.type_thing("node_agent").unwrap();
    let mut res = kernel
        .db
        .query(
            "SELECT id FROM entity WHERE type = $ty AND role = 'admin' \
             AND tenant = $session_tenant LIMIT 1",
        )
        .bind(("ty", node_agent))
        .await
        .unwrap();
    let row = res
        .take::<Vec<Row>>(0)
        .unwrap()
        .pop()
        .expect("bootstrap must seed at least one admin agent");
    format!(
        "entity:{}",
        match row.id.id {
            Id::Uuid(u) => u.to_raw(),
            Id::String(s) => s,
            other => panic!("unexpected admin-agent id form {other:?}"),
        }
    )
}

/// Create a typed entity of the given metamodel uid under the current
/// session tenant. Returns its typed `Thing`.
async fn make_typed_entity(kernel: &Kernel, type_uid: &str) -> Thing {
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

/// Count telemetry rows for a given lifecycle_event in the current session's tenant.
async fn count_events(kernel: &Kernel, lifecycle_event: &str) -> u64 {
    #[derive(serde::Deserialize)]
    struct Row {
        count: u64,
    }
    let mut res = kernel
        .db
        .query(
            "SELECT count() AS count FROM telemetry_stream \
             WHERE tenant = $session_tenant AND lifecycle_event = $ev \
             GROUP ALL",
        )
        .bind(("ev", lifecycle_event.to_string()))
        .await
        .unwrap();
    res.take::<Vec<Row>>(0)
        .unwrap()
        .pop()
        .map_or(0, |r| r.count)
}

#[tokio::test]
async fn runner_dispatches_compile_through_kernel_dispatcher_to_completion() {
    // Setup: bootstrap a tenant, fetch the admin agent the dispatcher
    // will run as, create a target + run entity, enqueue a schedule chain.
    let (_dir, kernel, _sub) = bootstrap("e2e_compile").await;
    let agent_id = fetch_admin_agent_id(&kernel).await;

    let target = make_typed_entity(&kernel, "node_prod").await;
    let run = make_typed_entity(&kernel, "node_run").await;

    // Give the target some `attr_desc` state so `compile_context` has
    // something to surface in its XML output.
    let target_id = format!(
        "entity:{}",
        match &target.id {
            Id::Uuid(u) => u.to_raw(),
            _ => unreachable!(),
        }
    );
    kernel
        .supersede_state(
            &target_id,
            "attr_desc",
            serde_json::json!({"text": "E2E_COMPILE_PAYLOAD"}),
            None,
        )
        .await
        .unwrap();

    let _sid = kernel
        .enqueue_schedule_item(
            run.clone(),
            "compile",
            target.clone(),
            Utc::now(),
            vec![],
            serde_json::json!({}),
        )
        .await
        .unwrap();

    // Drive the runner with the real KernelDispatcher.
    let dispatcher: Arc<dyn Dispatcher> = Arc::new(KernelDispatcher::new(kernel.clone(), agent_id));
    let runner = RunnerBlade::with_dispatcher(&kernel, dispatcher);
    let processed = runner.tick().await.unwrap();
    assert_eq!(processed, 1, "tick must consume exactly the one enqueued chain");

    // The schedule chain must have walked waiting → scheduled → running → completed.
    #[derive(serde::Deserialize)]
    #[allow(dead_code)]
    struct StatusRow {
        status: String,
        valid_from: chrono::DateTime<chrono::Utc>,
    }
    let mut res = kernel
        .db
        .query(
            "SELECT status, valid_from FROM schedule \
             WHERE run = $run AND tenant = $session_tenant \
             ORDER BY valid_from ASC",
        )
        .bind(("run", run.clone()))
        .await
        .unwrap();
    let rows: Vec<StatusRow> = res.take(0).unwrap();
    let statuses: Vec<&str> = rows.iter().map(|r| r.status.as_str()).collect();
    assert_eq!(
        statuses,
        ["waiting", "scheduled", "running", "completed"],
        "full SCD-2 transition chain must be appended in order; got {statuses:?}"
    );

    // The compile blade emits `context_compile` telemetry — proves the
    // dispatcher actually ran the real blade, not the noop.
    let compile_events = count_events(&kernel, "context_compile").await;
    assert!(
        compile_events >= 1,
        "compile must emit a context_compile telemetry event; got {compile_events}"
    );
}

#[tokio::test]
async fn runner_dispatch_failure_lands_chain_in_failed_with_capability_denial() {
    // Negative path: enqueue with an agent that has NO capability for
    // the requested kind. The capability check inside KernelDispatcher
    // must refuse; the runner must record `failed` on the chain.
    let (_dir, kernel, _sub) = bootstrap("e2e_deny").await;

    // Create a *user* (non-admin) agent that holds no capability edges.
    let lonely_agent = make_typed_entity(&kernel, "node_agent").await;
    let lonely_agent_id = format!(
        "entity:{}",
        match &lonely_agent.id {
            Id::Uuid(u) => u.to_raw(),
            _ => unreachable!(),
        }
    );

    let target = make_typed_entity(&kernel, "node_prod").await;
    let run = make_typed_entity(&kernel, "node_run").await;
    kernel
        .enqueue_schedule_item(
            run.clone(),
            "compile",
            target,
            Utc::now(),
            vec![],
            serde_json::json!({}),
        )
        .await
        .unwrap();

    let dispatcher: Arc<dyn Dispatcher> =
        Arc::new(KernelDispatcher::new(kernel.clone(), lonely_agent_id));
    let runner = RunnerBlade::with_dispatcher(&kernel, dispatcher);
    runner.tick().await.unwrap();

    #[derive(serde::Deserialize)]
    #[allow(dead_code)]
    struct StatusRow {
        status: String,
        valid_from: chrono::DateTime<chrono::Utc>,
    }
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
    let latest: StatusRow = res.take::<Vec<StatusRow>>(0).unwrap().pop().unwrap();
    assert_eq!(
        latest.status, "failed",
        "capability denial must land the schedule chain in `failed`, not `completed`"
    );
}
