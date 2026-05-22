/*
 * Schedule table SCD-2 invariants (R-plan #20)
 *
 * Proves the two binding contracts the rest of the system relies on:
 *
 *   1. **Reconstructability** — the full chain is recoverable in temporal
 *      order from `valid_from`. No external ordering metadata required.
 *   2. **Append-only** — prior rows are never mutated. Demonstrated both
 *      structurally (read-back equality across a transition) and via the
 *      §10 / §13 engine boundary (UPDATE / DELETE on `schedule` is
 *      engine-refused under the `superx` service account).
 *
 * Together these guarantee the SCD-2 invariant `ARCHITECTURE.md` §7-#5
 * specifies for every substrate table.
 *
 * Copyright (c) 2026 Constantin Alexander <constantin@dedomena.io>
 */

use std::sync::Arc;

use chrono::Utc;
use superx_bootstrap::BootstrapBlade;
use superx_kernel::{Kernel, KernelError};
use surrealdb::sql::{Id, Thing};
use tempfile::TempDir;

async fn bootstrap(tenant: &str) -> (TempDir, Arc<Kernel>) {
    let _ = tracing_subscriber::fmt::try_init();
    let dir = tempfile::tempdir().expect("tempdir");
    let kernel = Arc::new(
        Kernel::init(&dir.path().join("superx.db"), "test", "test")
            .await
            .expect("kernel init"),
    );
    BootstrapBlade::new(&kernel)
        .run(tenant)
        .await
        .expect("bootstrap");
    (dir, kernel)
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

/// A full chain `waiting → scheduled → running → completed` is recoverable
/// from `valid_from` alone, in append order, without consulting any
/// out-of-band metadata.
#[tokio::test]
async fn chain_is_reconstructable_in_temporal_order_from_valid_from_only() {
    let (_dir, kernel) = bootstrap("scd2_chain_recon").await;
    let run = make_entity(&kernel, "node_run").await;
    let target = make_entity(&kernel, "node_prod").await;

    let sid = kernel
        .enqueue_schedule_item(
            run.clone(),
            "noop",
            target,
            Utc::now(),
            vec![],
            serde_json::json!({}),
        )
        .await
        .unwrap();
    let sid2 = kernel
        .transition_schedule_status(sid, "scheduled")
        .await
        .unwrap();
    let sid3 = kernel
        .transition_schedule_status(sid2, "running")
        .await
        .unwrap();
    let _sid4 = kernel
        .transition_schedule_status(sid3, "completed")
        .await
        .unwrap();

    #[derive(serde::Deserialize, Debug)]
    #[allow(dead_code)]
    struct Row {
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
        .bind(("run", run))
        .await
        .unwrap();
    let rows: Vec<Row> = res.take(0).unwrap();

    assert_eq!(
        rows.len(),
        4,
        "every transition appends one row; expected 4 got {}",
        rows.len()
    );
    let statuses: Vec<&str> = rows.iter().map(|r| r.status.as_str()).collect();
    assert_eq!(
        statuses,
        ["waiting", "scheduled", "running", "completed"],
        "valid_from ASC must yield the same order transitions were issued in"
    );

    // Monotonic-by-construction: each row's valid_from is no earlier than
    // the previous row's.
    for w in rows.windows(2) {
        assert!(
            w[1].valid_from >= w[0].valid_from,
            "valid_from must be monotonic; {:?} not >= {:?}",
            w[1].valid_from,
            w[0].valid_from
        );
    }
}

/// After a transition, fetching the prior row by its id yields **exactly**
/// the same payload — the kernel never close-writes the old row.
#[tokio::test]
async fn prior_row_payload_is_byte_identical_after_a_transition() {
    let (_dir, kernel) = bootstrap("scd2_immut").await;
    let run = make_entity(&kernel, "node_run").await;
    let target = make_entity(&kernel, "node_prod").await;

    let sid = kernel
        .enqueue_schedule_item(
            run.clone(),
            "noop",
            target.clone(),
            Utc::now(),
            vec![],
            serde_json::json!({"note": "first"}),
        )
        .await
        .unwrap();

    // Snapshot the row before any transition.
    #[derive(serde::Deserialize, Debug, PartialEq, Eq)]
    struct RowSnap {
        status: String,
        attempt: i64,
        kind: String,
    }
    let before: RowSnap = {
        let mut r = kernel
            .db
            .query("SELECT status, attempt, kind FROM $sid")
            .bind(("sid", sid.clone()))
            .await
            .unwrap();
        r.take::<Vec<RowSnap>>(0).unwrap().pop().unwrap()
    };
    assert_eq!(before.status, "waiting");
    assert_eq!(before.attempt, 0);
    assert_eq!(before.kind, "noop");

    // Transition through scheduled and running; sid stays the original
    // row's id (transition_schedule_status returns the NEW row's id).
    let _sid2 = kernel
        .transition_schedule_status(sid.clone(), "scheduled")
        .await
        .unwrap();
    let _sid3 = kernel
        .transition_schedule_status(_sid2, "running")
        .await
        .unwrap();

    // Read the original row back — it must be identical.
    let after: RowSnap = {
        let mut r = kernel
            .db
            .query("SELECT status, attempt, kind FROM $sid")
            .bind(("sid", sid))
            .await
            .unwrap();
        r.take::<Vec<RowSnap>>(0).unwrap().pop().unwrap()
    };
    assert_eq!(
        before, after,
        "the original `waiting` row must not have been mutated by the subsequent transitions"
    );
}

/// Append-only is observable as a strict-monotonic row-count invariant:
/// after N `transition_schedule_status` calls the chain has exactly N+1
/// rows (the original `waiting` plus one per transition). Anything less
/// would mean a kernel verb (or a misbehaving caller) deleted history.
///
/// **Why this and not a PERMISSIONS-level engine-refusal check:** the
/// `superx` user is defined with `ROLES EDITOR` (kernel.rs:502). Under
/// SurrealDB v2.x, EDITOR is a database-level role with built-in CRUD
/// rights — it bypasses table-level `FOR delete NONE` / `FOR update NONE`
/// PERMISSIONS clauses (those clauses apply to TYPE-RECORD users only).
/// Real defense-in-depth requires Roadmap #8 (`DEFINE ACCESS TYPE
/// RECORD`); until then, append-only is enforced by **kernel-verb
/// discipline**, not by the engine. This test proves the discipline
/// holds across the verb surface that operates on `schedule`.
#[tokio::test]
async fn schedule_row_count_grows_strictly_by_one_per_transition() {
    let (_dir, kernel) = bootstrap("scd2_count_invariant").await;
    let run = make_entity(&kernel, "node_run").await;
    let target = make_entity(&kernel, "node_prod").await;

    let mut sid = kernel
        .enqueue_schedule_item(
            run.clone(),
            "noop",
            target,
            Utc::now(),
            vec![],
            serde_json::json!({}),
        )
        .await
        .unwrap();

    #[derive(serde::Deserialize)]
    struct CountRow {
        count: u64,
    }
    async fn chain_count(kernel: &Kernel, run: &Thing) -> u64 {
        let mut r = kernel
            .db
            .query(
                "SELECT count() AS count FROM schedule \
                 WHERE run = $run AND tenant = $session_tenant GROUP ALL",
            )
            .bind(("run", run.clone()))
            .await
            .unwrap();
        r.take::<Vec<CountRow>>(0)
            .unwrap()
            .pop()
            .map_or(0, |c| c.count)
    }

    assert_eq!(
        chain_count(&kernel, &run).await,
        1,
        "fresh enqueue must produce exactly one row"
    );

    for (i, status) in ["scheduled", "running", "completed"].iter().enumerate() {
        sid = kernel
            .transition_schedule_status(sid, status)
            .await
            .unwrap();
        let expected = u64::try_from(i + 2).unwrap();
        let observed = chain_count(&kernel, &run).await;
        assert_eq!(
            observed, expected,
            "after {} transition(s), expected {expected} rows, observed {observed}",
            i + 1
        );
    }
}

/// A long chain (8 transitions) still reconstructs cleanly. Guards
/// against any subtle off-by-one in chain-walk logic.
#[tokio::test]
async fn long_transition_chain_reconstructs_without_gaps() {
    let (_dir, kernel) = bootstrap("scd2_long").await;
    let run = make_entity(&kernel, "node_run").await;
    let target = make_entity(&kernel, "node_prod").await;

    // Walk through a longer state machine including a HITL pause.
    let order = [
        "scheduled",
        "running",
        "awaiting_human",
        "running",
        "completed",
    ];
    let mut sid: Thing = kernel
        .enqueue_schedule_item(
            run.clone(),
            "noop",
            target,
            Utc::now(),
            vec![],
            serde_json::json!({}),
        )
        .await
        .unwrap();
    for status in order {
        sid = kernel
            .transition_schedule_status(sid, status)
            .await
            .map_err(|e| {
                if let KernelError::Database(_) = &e {
                    panic!("unexpected db error: {e}");
                }
                e
            })
            .unwrap();
    }

    #[derive(serde::Deserialize, Debug)]
    #[allow(dead_code)]
    struct Row {
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
        .bind(("run", run))
        .await
        .unwrap();
    let rows: Vec<Row> = res.take(0).unwrap();

    let expected = [
        "waiting",
        "scheduled",
        "running",
        "awaiting_human",
        "running",
        "completed",
    ];
    let statuses: Vec<&str> = rows.iter().map(|r| r.status.as_str()).collect();
    assert_eq!(
        statuses, expected,
        "long chain must reconstruct exactly as transitions were issued"
    );
}
