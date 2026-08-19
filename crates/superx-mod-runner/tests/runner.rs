//! Contract tests for the runner module (epic #189, phase R1 per
//! spec #191): facilities, schema teeth, schedule chains, duration
//! grammar — all on `mem://`.

use chrono::{Duration, Utc};
use superx_kernel::KernelModule as _;
use superx_mod_runner::{schedule, RunnerModule, MODULE_NAME, SCHEMA_DDL};

async fn fresh_db() -> superx_kernel::Db {
    let db = surrealdb::engine::any::connect("mem://").await.expect("mem");
    db.use_ns("superx").use_db("runner").await.expect("nsdb");
    let ddl = SCHEMA_DDL.replace("$SUPERX_MODULE_PASSWORD", "test-password");
    db.query(ddl).await.expect("ddl").check().expect("schema applies clean");
    db
}

#[test]
fn facilities_declared() {
    let d = RunnerModule.descriptor();
    assert_eq!(d.name, MODULE_NAME);
    assert!(RunnerModule.schema_ddl().is_some(), "owns superx/runner");
    assert!(!RunnerModule.needs_dir(), "no files in v1");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn schema_reapply_is_idempotent() {
    let db = fresh_db().await;
    let ddl = SCHEMA_DDL.replace("$SUPERX_MODULE_PASSWORD", "test-password");
    db.query(ddl).await.expect("query").check().expect("IF NOT EXISTS everywhere");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn schedule_chain_lifecycle() {
    let db = fresh_db().await;
    let run_at = Utc::now() + Duration::hours(1);
    let uid = schedule::create_schedule(&db, "01a0-entity-uuid", run_at, "every:1d")
        .await
        .expect("create");

    let queue = schedule::current_schedules(&db).await.expect("queue");
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0].uid, uid);
    assert_eq!(queue[0].status, "scheduled");
    assert_eq!(queue[0].recurrence, "every:1d");

    schedule::cancel_schedule(&db, &uid).await.expect("cancel");
    let queue = schedule::current_schedules(&db).await.expect("queue");
    assert_eq!(queue[0].status, "cancelled", "chain-current is the cancel row");

    let history = schedule::schedule_history(&db, &uid).await.expect("history");
    assert_eq!(history.len(), 2, "scheduled then cancelled — both rows kept");
    assert_eq!(history[0].status, "scheduled");
    assert_eq!(history[1].status, "cancelled");
    assert_eq!(history[1].entity, "01a0-entity-uuid", "intent carried forward");

    let double = schedule::cancel_schedule(&db, &uid).await;
    assert!(double.unwrap_err().to_string().contains("already cancelled"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fragment_resolution() {
    let db = fresh_db().await;
    let a = schedule::create_schedule(&db, "e-a", Utc::now(), "none").await.expect("a");
    schedule::create_schedule(&db, "e-b", Utc::now(), "none").await.expect("b");

    assert_eq!(schedule::resolve_schedule(&db, &a).await.expect("unique"), a);
    let none = schedule::resolve_schedule(&db, "zzzz").await;
    assert!(none.unwrap_err().to_string().contains("no schedule matches"));
}

#[test]
fn duration_grammar() {
    for (raw, secs) in [("30s", 30), ("5m", 300), ("2h", 7200), ("1d", 86400), ("0s", 0)] {
        let (d, normalized) = schedule::parse_duration(raw).expect(raw);
        assert_eq!(d.num_seconds(), secs);
        assert_eq!(normalized, raw);
    }
    for bad in ["5x", "-1m", "1.5h", "m", "", "10"] {
        assert!(schedule::parse_duration(bad).is_err(), "{bad} must be rejected");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn schema_asserts_status() {
    let db = fresh_db().await;
    let refused = db
        .query(
            "CREATE schedule SET uid = 'u', entity = 'e', run_at = time::now(), \
             recurrence = 'none', status = 'bogus', valid_from = time::now()",
        )
        .await
        .expect("query")
        .check();
    assert!(refused.is_err(), "ASSERT INSIDE rejects unknown statuses");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_usage_and_unprovisioned_honesty() {
    let db = fresh_db().await;
    let kernel = superx_kernel::Kernel::from_db(db);
    let usage = RunnerModule.cli(&kernel, &[]).await;
    assert!(usage.unwrap_err().to_string().contains("usage: superx runner"));
    let honest = RunnerModule.cli(&kernel, &["queue".to_string()]).await;
    assert!(honest.is_err(), "test handle has no module_db — errors honestly");
}
