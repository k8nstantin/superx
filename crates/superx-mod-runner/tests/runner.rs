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

// Wave planning and the firing engine were exercised here against real
// graphs built by calling the entities module's own verbs — a crate
// dependency D17/D24 forbid, and the reason rewriting that module broke
// a file in this one. Removed rather than rewritten: testing the runner
// against another module's internals is exactly the coupling being cut,
// and the runner is itself due a rebuild.
//
// The `prompt` module below is the shape the replacement takes: a
// `Graph` deserialized from the contract, no entities crate anywhere.

// ------------------------------------------------------------ R3/R4 --
// The firing engine against REAL entity graphs and stub executor
// scripts (spec #193/#194 acceptance floors).

mod prompt {
    use superx_mod_runner::exec::build_prompt;
    use superx_mod_runner::plan::Graph;

    fn graph(json: serde_json::Value) -> Graph {
        serde_json::from_value(json).expect("the export contract parses")
    }

    fn note(uid: &str, label: &str, body: &str, version: &str) -> serde_json::Value {
        serde_json::json!({
            "uid": uid, "label": label, "body": body, "parent_uid": null,
            "author_kind": "operator", "via_uid": null, "version": version
        })
    }

    /// A task scheduled on its own IS the root of its own graph. Its
    /// description is the orders; printing it again as "About the
    /// product" tells the agent the same thing twice and calls the
    /// second one context.
    #[test]
    fn a_task_that_is_its_own_root_is_not_told_its_orders_twice() {
        let g = graph(serde_json::json!({
            "root": "task-1", "truncated_at_depth": false, "edges": [],
            "nodes": [{
                "uid": "task-1", "type": "task", "name": "Build it",
                "content": null, "attributes": null, "version": "", "depth": 0,
                "notes": [note("n1", "description", "DO THE THING", "v1")]
            }],
        }));
        let (prompt, version) = build_prompt(&g, "task-1");
        assert_eq!(prompt.matches("DO THE THING").count(), 1, "{prompt}");
        assert_eq!(version.as_deref(), Some("v1"), "and the run still pins what it read");
        assert!(!prompt.contains("About the product"), "{prompt}");
    }

    /// Under a product, the product's description IS context and belongs
    /// in the prompt — the deduplication must not cost that.
    #[test]
    fn a_task_under_a_product_still_gets_the_product_description() {
        let g = graph(serde_json::json!({
            "root": "prod-1", "truncated_at_depth": false,
            "edges": [{ "edge_uid": "e1", "from": "prod-1", "to": "task-1", "rel": "contains" }],
            "nodes": [
                { "uid": "prod-1", "type": "product", "name": "P", "content": null,
                  "attributes": null, "version": "", "depth": 0,
                  "notes": [note("n0", "description", "THE PRODUCT", "v0")] },
                { "uid": "task-1", "type": "task", "name": "Build it", "content": null,
                  "attributes": null, "version": "", "depth": 1,
                  "notes": [note("n1", "description", "DO THE THING", "v1")] },
            ],
        }));
        let (prompt, _) = build_prompt(&g, "task-1");
        assert!(prompt.contains("Instructions:\nDO THE THING"), "{prompt}");
        assert!(prompt.contains("About the product:\nTHE PRODUCT"), "{prompt}");
    }

    /// An explicit `instructions` note wins over the description, because
    /// prose migrated from a legacy `instructs` edge landed there and
    /// saying it explicitly should still count.
    #[test]
    fn explicit_instructions_win_over_the_description() {
        let g = graph(serde_json::json!({
            "root": "prod-1", "truncated_at_depth": false, "edges": [],
            "nodes": [{
                "uid": "task-1", "type": "task", "name": "Build it", "content": null,
                "attributes": null, "version": "", "depth": 1,
                "notes": [
                    note("n1", "description", "WHAT IT IS", "v1"),
                    note("n2", "instructions", "WHAT TO DO", "v2"),
                ]
            }],
        }));
        let (prompt, version) = build_prompt(&g, "task-1");
        assert!(prompt.contains("Instructions:\nWHAT TO DO"), "{prompt}");
        assert!(!prompt.contains("WHAT IT IS"), "{prompt}");
        assert_eq!(version.as_deref(), Some("v2"), "the pin follows what was actually sent");
    }

    /// A previous run's output used to arrive as "linked context" through
    /// a `produced` edge — the graph pollution this change exists to end,
    /// coming back in by a different door.
    #[test]
    fn a_text_carrier_never_becomes_linked_context() {
        let g = graph(serde_json::json!({
            "root": "prod-1", "truncated_at_depth": false,
            "edges": [
                { "edge_uid": "e1", "from": "task-1", "to": "old-output", "rel": "produced" },
                { "edge_uid": "e2", "from": "task-1", "to": "repo-1", "rel": "consults" },
            ],
            "nodes": [
                { "uid": "task-1", "type": "task", "name": "Build it", "content": null,
                  "attributes": null, "version": "", "depth": 1,
                  "notes": [note("n1", "description", "DO IT", "v1")] },
                { "uid": "old-output", "type": "text", "name": "result",
                  "content": "LAST RUN SAID THIS", "attributes": null,
                  "version": "", "depth": 2, "notes": [] },
                { "uid": "repo-1", "type": "repo", "name": "superx", "content": null,
                  "attributes": null, "version": "", "depth": 2, "notes": [] },
            ],
        }));
        let (prompt, _) = build_prompt(&g, "task-1");
        assert!(
            !prompt.contains("[produced]"),
            "a carrier is words, not context: {prompt}"
        );
        assert!(
            !prompt.contains("LAST RUN SAID THIS"),
            "and its words do not leak in either: {prompt}"
        );
        assert!(
            prompt.contains("- [consults] superx (repo)"),
            "a real linked entity still is: {prompt}"
        );
    }
}

