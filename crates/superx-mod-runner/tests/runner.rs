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

mod firing {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use superx_kernel::types::RecordId;
    use superx_mod_entities::{edges, graph, nodes, texts};
    use superx_mod_runner::exec::{fire, Exchange, FiringConfig, NO_INSTRUCTIONS_DETAIL, UNSET_CMD_DETAIL};
    use superx_mod_runner::{run, schedule};

    use super::{entities_db, fresh_db};

    /// Test exchange over a real entities mem db; optionally rewrites
    /// a task's instructs text after the first graph read (D27).
    struct FixtureExchange {
        edb: superx_kernel::Db,
        root: RecordId,
        graph_reads: AtomicUsize,
        instruct_edit: Option<(RecordId, String)>,
        edited: AtomicBool,
    }

    impl FixtureExchange {
        fn new(edb: superx_kernel::Db, root: RecordId) -> Self {
            Self { edb, root, graph_reads: AtomicUsize::new(0), instruct_edit: None, edited: AtomicBool::new(false) }
        }
    }

    #[async_trait::async_trait]
    impl Exchange for FixtureExchange {
        async fn graph(&self, _root: &str, depth: usize) -> superx_kernel::Result<superx_mod_runner::plan::Graph> {
            let reads = self.graph_reads.fetch_add(1, Ordering::SeqCst);
            if reads >= 1 && !self.edited.swap(true, Ordering::SeqCst) {
                if let Some((task, new_text)) = &self.instruct_edit {
                    texts::set_role_text(&self.edb, task, "instructs", new_text).await?;
                }
            }
            let sub = graph::subgraph(&self.edb, &self.root, depth, false).await?;
            let json = graph::to_json(&sub, &self.root);
            Ok(serde_json::from_value(json).expect("contract parses"))
        }

        /// Mirrors the live exchange (#286): the output is a COMMENT on
        /// the task, authored by the agent in this run — not a node in
        /// the product graph an agent then has to walk past.
        ///
        /// It goes through `texts::add_comment` and returns exactly what
        /// the entities CLI returns, because a fixture that is kinder
        /// than the real path proves a property of itself rather than
        /// of the runner. That used to be a carrier id; since #302
        /// retired the carrier it is the note uid, and this follows the
        /// real path rather than preserving the old shape.
        async fn write_back(
            &self,
            task_uid: &str,
            run_uid: &str,
            output: &str,
        ) -> superx_kernel::Result<String> {
            let task = nodes::resolve_entity(&self.edb, task_uid).await?;
            let author = superx_mod_entities::notes::Author::claimed("agent", Some(run_uid), None)?;
            texts::add_comment(&self.edb, &task, output, &author).await
        }
    }

    fn stub_script(body: &str) -> String {
        use std::os::unix::fs::PermissionsExt as _;
        let path = std::env::temp_dir().join(format!("sx-stub-{}.sh", uuid::Uuid::now_v7()));
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write stub");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        path.to_string_lossy().to_string()
    }

    async fn due_row(db: &superx_kernel::Db, entity: &RecordId) -> schedule::ScheduleRow {
        let uid = schedule::create_schedule(db, &superx_ops::record_uuid(entity), chrono::Utc::now(), "none")
            .await
            .expect("schedule");
        schedule::chain_current(db, &uid).await.expect("read").expect("row")
    }

    fn config(cmd: Option<String>) -> FiringConfig {
        FiringConfig { agent_cmd: cmd, max_parallel: 2, plan_depth: 10 }
    }

    async fn all_run_rows(db: &superx_kernel::Db) -> Vec<(String, String, String)> {
        // (task, status, valid_from) in insertion order.
        let mut resp = db
            .query("SELECT task, status, valid_from FROM run ORDER BY valid_from ASC")
            .await
            .expect("query");
        let rows: Vec<superx_kernel::types::Value> = resp.take(0).expect("take");
        rows.iter()
            .filter_map(|r| match r {
                superx_kernel::types::Value::Object(o) => Some((
                    match o.get("task") { Some(superx_kernel::types::Value::String(s)) => s.clone(), _ => return None },
                    match o.get("status") { Some(superx_kernel::types::Value::String(s)) => s.clone(), _ => return None },
                    match o.get("valid_from") { Some(superx_kernel::types::Value::Datetime(d)) => d.to_string(), _ => return None },
                )),
                _ => None,
            })
            .collect()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn chain_executes_in_order_with_writeback_and_version_pin() {
        let edb = entities_db().await;
        let rdb = fresh_db().await;
        let kernel = superx_kernel::Kernel::from_db(rdb.clone());

        let product = nodes::create_entity(&edb, "product", "P", None, None).await.expect("p");
        let t1 = nodes::create_entity(&edb, "task", "first", None, None).await.expect("t1");
        texts::set_role_text(&edb, &t1, "describes", "do the thing")
            .await
            .expect("orders");
        let t2 = nodes::create_entity(&edb, "task", "second", None, None).await.expect("t2");
        texts::set_role_text(&edb, &t2, "describes", "do the thing")
            .await
            .expect("orders");
        edges::link(&edb, &product, &t1, "contains").await.expect("c1");
        edges::link(&edb, &product, &t2, "contains").await.expect("c2");
        edges::link(&edb, &t2, &t1, "depends_on").await.expect("d");
        texts::set_role_text(&edb, &t1, "instructs", "do the first thing").await.expect("i");
        // The run pins the version of the NOTE it was dispatched under
        // (#286), so that is what the expectation reads — and it reads it
        // from the export, which is what the runner actually saw.
        let instruct_version = superx_mod_entities::notes::for_entity(&edb, &t1, false)
            .await
            .expect("notes")
            .into_iter()
            .find(|n| n.label == "instructions")
            .and_then(|n| n.valid_from)
            .expect("the instructions note is dated")
            .to_rfc3339();

        let exchange = FixtureExchange::new(edb.clone(), product.clone());
        let due = due_row(&rdb, &product).await;
        let script = stub_script("echo \"did it\"");
        let report = fire(&kernel, &rdb, &exchange, &config(Some(script)), &due).await.expect("fire");
        assert_eq!((report.done, report.failed, report.cancelled), (2, 0, false));

        // Dependency chronology: t1's done row precedes t2's dispatch.
        let rows = all_run_rows(&rdb).await;
        let t1_uuid = superx_ops::record_uuid(&t1);
        let t2_uuid = superx_ops::record_uuid(&t2);
        let t1_done = rows.iter().position(|(t, s, _)| *t == t1_uuid && s == "done").expect("t1 done");
        let t2_dispatched = rows.iter().position(|(t, s, _)| *t == t2_uuid && s == "dispatched").expect("t2 dispatched");
        assert!(t1_done < t2_dispatched, "{rows:?}");

        // The instruct version rode the run chain (D27 pin).
        let runs = run::current_runs(&rdb, None).await.expect("runs");
        let t1_run = runs.iter().find(|r| r.task == t1_uuid).expect("t1 run");
        assert_eq!(t1_run.status, "done");
        assert_eq!(t1_run.instruct_version.as_deref(), Some(instruct_version.as_str()));
        let output_ref = t1_run.output_ref.clone().expect("the run points at its output");

        // The result is a COMMENT ON THE TASK (#286), authored by the
        // agent — not a node in the product graph the next agent then has
        // to walk past and recognise as words rather than work.
        let notes = superx_mod_entities::notes::for_entity(&edb, &t1, false).await.expect("notes");
        let output = notes
            .iter()
            .find(|n| n.label == "comments")
            .expect("the run's output is a comment on the task");
        assert!(output.body.contains("did it"));
        assert_eq!(output.author_kind.as_deref(), Some("agent"), "attributed to the agent");
        assert_eq!(
            output.author_uid.as_deref(),
            Some(t1_run.uid.as_str()),
            "and to the run that produced it"
        );
        assert!(
            !output_ref.is_empty(),
            "the run points at something — an empty output_ref is a run that recorded nothing"
        );
        // The property is that the reference points at something REAL —
        // a run pointing at nothing records nothing. Since #302 retired
        // the carrier it is a note uid rather than an entity id, so it
        // is resolved where notes live.
        let note = superx_mod_entities::notes::current(&edb, &output_ref)
            .await
            .expect("read")
            .expect("whatever the run points at can be found");
        assert_eq!(note.label, "comments", "the output is a comment on the task");
        assert_eq!(
            note.author_kind.as_deref(),
            Some("agent"),
            "authored by the agent that ran it, not by the operator"
        );

        // And it did NOT become a node in the graph.
        assert!(
            !edges::expand(&edb, std::slice::from_ref(&t1), false)
                .await
                .expect("expand")
                .into_iter()
                .any(|e| e.rel_type == "produced" && e.active),
            "a run's output no longer hangs off the task as an entity"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn parallel_wave_overlaps() {
        let edb = entities_db().await;
        let rdb = fresh_db().await;
        let kernel = superx_kernel::Kernel::from_db(rdb.clone());

        let product = nodes::create_entity(&edb, "product", "P", None, None).await.expect("p");
        for name in ["a", "b"] {
            let t = nodes::create_entity(&edb, "task", name, None, None).await.expect("t");
            edges::link(&edb, &product, &t, "contains").await.expect("c");
            // A task's description IS its orders, so a task without one
            // is not a task anybody could run.
            texts::set_role_text(&edb, &t, "describes", "do the thing").await.expect("d");
        }
        let log = std::env::temp_dir().join(format!("sx-log-{}", uuid::Uuid::now_v7()));
        let script = stub_script(&format!(
            "echo S >> {log}\nsleep 0.4\necho E >> {log}\necho out",
            log = log.to_string_lossy()
        ));
        let exchange = FixtureExchange::new(edb.clone(), product.clone());
        let due = due_row(&rdb, &product).await;
        let report = fire(&kernel, &rdb, &exchange, &config(Some(script)), &due).await.expect("fire");
        assert_eq!(report.done, 2);

        let marks = std::fs::read_to_string(&log).expect("log");
        let seq: Vec<&str> = marks.lines().collect();
        assert_eq!(seq.len(), 4, "{seq:?}");
        assert_eq!(seq[0], "S");
        assert_eq!(seq[1], "S", "both started before either ended (parallel): {seq:?}");
        let _ = std::fs::remove_file(&log);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn failure_blocks_dependents() {
        let edb = entities_db().await;
        let rdb = fresh_db().await;
        let kernel = superx_kernel::Kernel::from_db(rdb.clone());

        let product = nodes::create_entity(&edb, "product", "P", None, None).await.expect("p");
        let t1 = nodes::create_entity(&edb, "task", "breaks", None, None).await.expect("t1");
        texts::set_role_text(&edb, &t1, "describes", "do the thing")
            .await
            .expect("orders");
        let t2 = nodes::create_entity(&edb, "task", "after", None, None).await.expect("t2");
        texts::set_role_text(&edb, &t2, "describes", "do the thing")
            .await
            .expect("orders");
        edges::link(&edb, &product, &t1, "contains").await.expect("c1");
        edges::link(&edb, &product, &t2, "contains").await.expect("c2");
        edges::link(&edb, &t2, &t1, "depends_on").await.expect("d");

        let script = stub_script("echo boom >&2\nexit 1");
        let exchange = FixtureExchange::new(edb.clone(), product.clone());
        let due = due_row(&rdb, &product).await;
        let report = fire(&kernel, &rdb, &exchange, &config(Some(script)), &due).await.expect("fire");
        assert_eq!((report.done, report.failed), (0, 1));

        let runs = run::current_runs(&rdb, None).await.expect("runs");
        assert_eq!(runs.len(), 1, "the dependent never dispatched: {runs:?}");
        assert_eq!(runs[0].status, "failed");
        let detail = runs[0].detail.as_deref().unwrap_or("");
        assert!(detail.contains("exit") && detail.contains("boom"), "{detail}");
    }

    /// A task with nothing to do used to dispatch anyway, with an empty
    /// Instructions section — an agent sent off with a heading and
    /// nothing under it, whose output is then indistinguishable from
    /// work actually done.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_task_with_no_orders_is_refused_rather_than_dispatched() {
        let edb = entities_db().await;
        let rdb = fresh_db().await;
        let kernel = superx_kernel::Kernel::from_db(rdb.clone());

        let product = nodes::create_entity(&edb, "product", "P", None, None).await.expect("p");
        let t = nodes::create_entity(&edb, "task", "nothing to do", None, None).await.expect("t");
        edges::link(&edb, &product, &t, "contains").await.expect("c");

        let exchange = FixtureExchange::new(edb.clone(), product.clone());
        let due = due_row(&rdb, &product).await;
        let script = stub_script("echo ran");
        let report = fire(&kernel, &rdb, &exchange, &config(Some(script)), &due).await.expect("fire");

        assert_eq!((report.done, report.failed), (0, 1));
        let runs = run::current_runs(&rdb, None).await.expect("runs");
        assert_eq!(runs[0].status, "failed");
        assert_eq!(runs[0].detail.as_deref(), Some(NO_INSTRUCTIONS_DETAIL));
        assert!(
            superx_mod_entities::notes::for_entity(&edb, &t, false).await.expect("notes").is_empty(),
            "nothing was written back, because nothing ran"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn unset_agent_cmd_fails_loudly() {
        let edb = entities_db().await;
        let rdb = fresh_db().await;
        let kernel = superx_kernel::Kernel::from_db(rdb.clone());
        let product = nodes::create_entity(&edb, "product", "P", None, None).await.expect("p");
        let t = nodes::create_entity(&edb, "task", "t", None, None).await.expect("t");
        texts::set_role_text(&edb, &t, "describes", "do the thing")
            .await
            .expect("orders");
        edges::link(&edb, &product, &t, "contains").await.expect("c");

        let exchange = FixtureExchange::new(edb.clone(), product.clone());
        let due = due_row(&rdb, &product).await;
        let report = fire(&kernel, &rdb, &exchange, &config(None), &due).await.expect("fire");
        assert_eq!(report.failed, 1);
        let runs = run::current_runs(&rdb, None).await.expect("runs");
        assert_eq!(runs[0].detail.as_deref(), Some(UNSET_CMD_DETAIL));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cancel_between_waves_stops_dispatch() {
        let edb = entities_db().await;
        let rdb = fresh_db().await;
        let kernel = superx_kernel::Kernel::from_db(rdb.clone());
        let product = nodes::create_entity(&edb, "product", "P", None, None).await.expect("p");
        let t1 = nodes::create_entity(&edb, "task", "slow", None, None).await.expect("t1");
        texts::set_role_text(&edb, &t1, "describes", "do the thing")
            .await
            .expect("orders");
        let t2 = nodes::create_entity(&edb, "task", "never", None, None).await.expect("t2");
        texts::set_role_text(&edb, &t2, "describes", "do the thing")
            .await
            .expect("orders");
        edges::link(&edb, &product, &t1, "contains").await.expect("c1");
        edges::link(&edb, &product, &t2, "contains").await.expect("c2");
        edges::link(&edb, &t2, &t1, "depends_on").await.expect("d");

        let script = stub_script("sleep 0.4\necho out");
        let exchange = FixtureExchange::new(edb.clone(), product.clone());
        let due = due_row(&rdb, &product).await;

        // Cancel while wave 1 is in flight.
        let rdb2 = rdb.clone();
        let uid = due.uid.clone();
        let canceller = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            schedule::cancel_schedule(&rdb2, &uid).await.expect("cancel");
        });
        let report = fire(&kernel, &rdb, &exchange, &config(Some(script)), &due).await.expect("fire");
        canceller.await.expect("join");

        assert!(report.cancelled);
        assert_eq!(report.done, 1, "in-flight task finishes and records (D27)");
        let runs = run::current_runs(&rdb, None).await.expect("runs");
        assert_eq!(runs.len(), 1, "wave 2 never dispatched: {runs:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn instruct_edit_between_waves_pins_new_version() {
        let edb = entities_db().await;
        let rdb = fresh_db().await;
        let kernel = superx_kernel::Kernel::from_db(rdb.clone());
        let product = nodes::create_entity(&edb, "product", "P", None, None).await.expect("p");
        let t1 = nodes::create_entity(&edb, "task", "one", None, None).await.expect("t1");
        texts::set_role_text(&edb, &t1, "describes", "do the thing")
            .await
            .expect("orders");
        let t2 = nodes::create_entity(&edb, "task", "two", None, None).await.expect("t2");
        texts::set_role_text(&edb, &t2, "describes", "do the thing")
            .await
            .expect("orders");
        edges::link(&edb, &product, &t1, "contains").await.expect("c1");
        edges::link(&edb, &product, &t2, "contains").await.expect("c2");
        edges::link(&edb, &t2, &t1, "depends_on").await.expect("d");
        let (i2, _) = texts::set_role_text(&edb, &t2, "instructs", "old orders").await.expect("i2");
        // The version the RUNNER pins is the note's `valid_from` as
        // rfc3339 (`graph.rs` builds the node contract that way), so
        // read it from the same place. Comparing against a carrier's
        // state version — which is what this did before #302 retired
        // the carrier — would compare two different clocks and pass
        // whatever happened.
        let old_version = superx_mod_entities::notes::current(&edb, &i2)
            .await
            .expect("q")
            .expect("note")
            .valid_from
            .map(|t| t.to_rfc3339())
            .expect("a written note has a version");

        let mut exchange = FixtureExchange::new(edb.clone(), product.clone());
        exchange.instruct_edit = Some((t2.clone(), "NEW orders".to_string()));
        let script = stub_script("echo out");
        let due = due_row(&rdb, &product).await;
        let report = fire(&kernel, &rdb, &exchange, &config(Some(script)), &due).await.expect("fire");
        assert_eq!(report.done, 2);

        let runs = run::current_runs(&rdb, None).await.expect("runs");
        let t2_run = runs.iter().find(|r| r.task == superx_ops::record_uuid(&t2)).expect("t2");
        let pinned = t2_run.instruct_version.as_deref().expect("pinned");
        assert_ne!(pinned, old_version, "the run pins the CURRENT instruct version (D27)");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn recurrence_re_enqueues_and_cancel_stops_it() {
        let edb = entities_db().await;
        let rdb = fresh_db().await;
        let kernel = superx_kernel::Kernel::from_db(rdb.clone());
        let product = nodes::create_entity(&edb, "product", "P", None, None).await.expect("p");

        let uid = schedule::create_schedule(&rdb, &superx_ops::record_uuid(&product), chrono::Utc::now(), "every:2s")
            .await
            .expect("schedule");
        let due = schedule::chain_current(&rdb, &uid).await.expect("q").expect("row");
        let exchange = FixtureExchange::new(edb.clone(), product.clone());
        fire(&kernel, &rdb, &exchange, &config(None), &due).await.expect("fire");

        superx_mod_runner::recurrence::re_enqueue(&kernel, &rdb, &due).await;
        let current = schedule::chain_current(&rdb, &uid).await.expect("q").expect("row");
        assert_eq!(current.status, "scheduled", "re-enqueued");
        let history = schedule::schedule_history(&rdb, &uid).await.expect("h");
        assert_eq!(
            history.iter().map(|r| r.status.as_str()).collect::<Vec<_>>(),
            vec!["scheduled", "fired", "scheduled"],
            "the chain IS the perpetual history"
        );

        schedule::cancel_schedule(&rdb, &uid).await.expect("cancel");
        superx_mod_runner::recurrence::re_enqueue(&kernel, &rdb, &due).await;
        let current = schedule::chain_current(&rdb, &uid).await.expect("q").expect("row");
        assert_eq!(current.status, "cancelled", "cancel beats recurrence");

        // 'none' rests at fired.
        let uid2 = schedule::create_schedule(&rdb, "e", chrono::Utc::now(), "none").await.expect("s2");
        let due2 = schedule::chain_current(&rdb, &uid2).await.expect("q").expect("row");
        schedule::append_status(&rdb, &due2, "fired").await.expect("fired");
        superx_mod_runner::recurrence::re_enqueue(&kernel, &rdb, &due2).await;
        let current = schedule::chain_current(&rdb, &uid2).await.expect("q").expect("row");
        assert_eq!(current.status, "fired");
    }
}
