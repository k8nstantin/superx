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

// ---------------------------------------------------------------- R2 --
// Wave planning against REAL graphs built with the entities module's
// own verbs (dev-dependency), serialized through the ONE shared JSON
// producer (`graph::to_json`) the CLI uses — the exact contract.

async fn entities_db() -> superx_kernel::Db {
    let db = surrealdb::engine::any::connect("mem://").await.expect("mem");
    db.use_ns("superx").use_db("entities").await.expect("nsdb");
    let ddl = superx_mod_entities::SCHEMA_DDL.replace("$SUPERX_MODULE_PASSWORD", "t");
    db.query(ddl).await.expect("ddl").check().expect("ok");
    superx_mod_entities::registry::seed_types(&db).await.expect("seed");
    db
}

async fn graph_of(db: &superx_kernel::Db, root: &superx_kernel::types::RecordId) -> superx_mod_runner::plan::Graph {
    let sub = superx_mod_entities::graph::subgraph(db, root, 20, false).await.expect("subgraph");
    let json = superx_mod_entities::graph::to_json(&sub, root);
    serde_json::from_value(json).expect("contract parses")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn waves_of_the_superx_example() {
    use superx_mod_entities::{edges, nodes};
    use superx_mod_runner::plan;
    let db = entities_db().await;

    let product = nodes::create_entity(&db, "product", "SuperX", None, None).await.expect("p");
    let kernel_t = nodes::create_entity(&db, "task", "Modify the kernel", None, None).await.expect("k");
    let mod_a = nodes::create_entity(&db, "task", "Build module A", None, None).await.expect("a");
    let mod_b = nodes::create_entity(&db, "task", "Build module B", None, None).await.expect("b");
    let qa_a = nodes::create_entity(&db, "task", "QA module A", None, None).await.expect("qa");
    let qa_b = nodes::create_entity(&db, "task", "QA module B", None, None).await.expect("qb");
    for t in [&kernel_t, &mod_a, &mod_b, &qa_a, &qa_b] {
        edges::link(&db, &product, t, "contains").await.expect("contain");
    }
    edges::link(&db, &mod_a, &kernel_t, "depends_on").await.expect("d1");
    edges::link(&db, &mod_b, &kernel_t, "depends_on").await.expect("d2");
    edges::link(&db, &qa_a, &mod_a, "depends_on").await.expect("d3");
    edges::link(&db, &qa_b, &mod_b, "depends_on").await.expect("d4");

    let plan = plan::compute_waves(&graph_of(&db, &product).await).expect("plan");
    assert!(plan.warnings.is_empty(), "{:?}", plan.warnings);
    let names: Vec<Vec<&str>> = plan
        .waves
        .iter()
        .map(|w| w.iter().map(|t| t.name.as_str()).collect())
        .collect();
    assert_eq!(names.len(), 3, "{names:?}");
    assert_eq!(names[0], vec!["Modify the kernel"]);
    assert_eq!(names[1].len(), 2, "modules build in parallel: {names:?}");
    assert!(names[1].contains(&"Build module A") && names[1].contains(&"Build module B"));
    assert_eq!(names[2].len(), 2, "QA in parallel after each: {names:?}");

    let rendered = plan::render_plan(&plan);
    assert!(rendered.contains("(parallel)"), "{rendered}");
    assert!(rendered.contains("5 tasks in 3 waves"), "{rendered}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cycles_refused_with_names() {
    use superx_mod_entities::{edges, nodes};
    use superx_mod_runner::plan;
    let db = entities_db().await;
    let root = nodes::create_entity(&db, "product", "P", None, None).await.expect("p");
    let a = nodes::create_entity(&db, "task", "task-a", None, None).await.expect("a");
    let b = nodes::create_entity(&db, "task", "task-b", None, None).await.expect("b");
    edges::link(&db, &root, &a, "contains").await.expect("c1");
    edges::link(&db, &root, &b, "contains").await.expect("c2");
    edges::link(&db, &a, &b, "depends_on").await.expect("d1");
    edges::link(&db, &b, &a, "depends_on").await.expect("d2");

    let err = plan::compute_waves(&graph_of(&db, &root).await).unwrap_err().to_string();
    assert!(err.contains("cycle"), "{err}");
    assert!(err.contains("task-a") && err.contains("task-b"), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_tasks_never_execute_and_external_deps_warn() {
    use superx_mod_entities::{edges, nodes, texts};
    use superx_mod_runner::plan;
    let db = entities_db().await;

    // A product with texts/repo but ZERO tasks plans to zero waves.
    let empty = nodes::create_entity(&db, "product", "Docs only", None, None).await.expect("p");
    texts::set_role_text(&db, &empty, "describes", "no tasks here").await.expect("t");
    let plan0 = plan::compute_waves(&graph_of(&db, &empty).await).expect("plan");
    assert_eq!(plan0.waves.len(), 0);

    // A task depending on a NON-task (the repo) warns and runs wave 1.
    let root = nodes::create_entity(&db, "product", "P2", None, None).await.expect("p2");
    let task = nodes::create_entity(&db, "task", "build", None, None).await.expect("t2");
    let repo = nodes::create_entity(&db, "repo", "the repo", None, None).await.expect("r");
    edges::link(&db, &root, &task, "contains").await.expect("c");
    edges::link(&db, &root, &repo, "contains").await.expect("c2");
    edges::link(&db, &task, &repo, "depends_on").await.expect("d");
    let plan1 = plan::compute_waves(&graph_of(&db, &root).await).expect("plan");
    assert_eq!(plan1.waves.len(), 1);
    assert_eq!(plan1.warnings.len(), 1, "{:?}", plan1.warnings);
    assert!(plan1.warnings[0].contains("treated satisfied"));
}
