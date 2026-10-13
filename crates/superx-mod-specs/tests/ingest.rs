//! Ingest end to end: transcript lines in the shape the kernel captures
//! them, into an in-memory kernel; the module's own schema in a second
//! in-memory database; every query this module issues runs against both.

use serde_json::{json, Value as Json};
use superx_kernel::types::{Object, RecordId, Value};
use superx_kernel::{Kernel, NewMessage, SCHEMA_DDL as KERNEL_DDL};
use superx_mod_specs::{ingest, store, SCHEMA_DDL};

const ROOT: &str = include_str!("fixtures/root.xml");
const RULES: &str = include_str!("fixtures/rules.xml");
const TASK: &str = include_str!("fixtures/task.xml");
const BRANCH: &str = "ENG-1-the-tool-v3-sonnet-5-5-max";

async fn fresh_kernel() -> Kernel {
    let db = surrealdb::engine::any::connect("mem://").await.expect("mem");
    db.use_ns("superx").use_db("kernel").await.expect("nsdb");
    db.query(KERNEL_DDL.replace("$SUPERX_KERNEL_PASSWORD", "pw")).await.expect("ddl").check().expect("ddl ok");
    let kernel = Kernel::from_db(db);
    for t in superx_kernel::REQUIRED_METAMODEL_TYPES {
        kernel.ensure_type_definition(t.uid, t.category, t.memory_tier).await.expect("seed");
    }
    kernel
}

async fn specs_db() -> superx_kernel::Db {
    let db = surrealdb::engine::any::connect("mem://").await.expect("mem");
    db.use_ns("superx").use_db("specs").await.expect("nsdb");
    db.query(SCHEMA_DDL.replace("$SUPERX_MODULE_PASSWORD", "pw")).await.expect("ddl").check().expect("ddl ok");
    db
}

/// A session the way the kernel's capture leaves it, fed one line at a
/// time in transcript order.
struct Session<'k> {
    kernel: &'k Kernel,
    agent: RecordId,
    id: RecordId,
    n: usize,
}

impl<'k> Session<'k> {
    async fn new(kernel: &'k Kernel, key: &str) -> Self {
        let agent = kernel.create_entity("node_agent").await.expect("agent");
        let id = kernel.create_entity("node_session").await.expect("session");
        let mut desc = Object::new();
        desc.insert("name".to_string(), Value::String(format!("claude_code/{key}")));
        desc.insert("session".to_string(), Value::String(key.to_string()));
        desc.insert("locator".to_string(), Value::String("/tmp/x.jsonl".to_string()));
        kernel.supersede_state(id.clone(), "attr_session_descriptor", Value::Object(desc)).await.expect("descriptor");
        Self { kernel, agent, id, n: 0 }
    }

    async fn line(&self, role: &str, content: &str, raw: Json) {
        self.kernel
            .log_message(NewMessage {
                session: self.id.clone(),
                agent: self.agent.clone(),
                role: role.to_string(),
                content: content.to_string(),
                raw: Some(superx_kernel::message::json_to_object(&raw)),
                seq: None,
                emitted_at: None,
            })
            .await
            .expect("message");
    }

    async fn say(&self, text: &str) {
        self.line("user", text, json!({"type": "user", "origin": {"kind": "human"}, "gitBranch": "main",
            "message": {"role": "user", "content": text}}))
            .await;
    }

    /// A Read call and its result, the file's bytes as the tool returns them.
    async fn read(&mut self, path: &str, content: &str, whole: bool) {
        self.n += 1;
        let id = format!("toolu_{}", self.n);
        let lines = content.lines().count();
        self.line("assistant", "", json!({"type": "assistant", "gitBranch": BRANCH,
            "message": {"model": "claude-sonnet-5-5", "content": [
                {"type": "tool_use", "id": id, "name": "Read", "input": {"file_path": path}}]}}))
            .await;
        self.line("tool", "", json!({"type": "user", "gitBranch": BRANCH,
            "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": "1\t…"}]},
            "toolUseResult": {"type": "text", "file": {"filePath": path, "content": content,
                "startLine": 1, "numLines": if whole { lines } else { 1 }, "totalLines": lines}}}))
            .await;
    }

    async fn edit(&mut self, path: &str) {
        self.n += 1;
        self.line("assistant", "", json!({"type": "assistant", "gitBranch": BRANCH,
            "message": {"model": "claude-sonnet-5-5", "content": [
                {"type": "text", "text": "Writing the tool."},
                {"type": "tool_use", "id": format!("toolu_{}", self.n), "name": "Edit",
                 "input": {"file_path": path, "old_string": "a", "new_string": "b"}}]}}))
            .await;
    }
}

async fn count(db: &superx_kernel::Db, table: &str) -> i64 {
    let rows: Vec<Value> = db.query(format!("SELECT count() AS c FROM {table} GROUP ALL")).await.expect("count").take(0).expect("rows");
    rows.first()
        .and_then(|r| match r {
            Value::Object(o) => match o.get("c") {
                Some(Value::Number(n)) => n.to_int(),
                _ => None,
            },
            _ => None,
        })
        .unwrap_or(0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_build_is_recorded_with_what_it_read_and_judged() {
    let kernel = fresh_kernel().await;
    let db = specs_db().await;

    // A build that reads in the root's order, then writes.
    let mut good = Session::new(&kernel, "good-build").await;
    good.say("Build specs/tasks/ENG-1.xml").await;
    good.read("/r/specs/tasks/ENG-1.xml", TASK, true).await;
    good.read("/r/app/core/door.py", "print('door')\n", true).await;
    good.read("/r/specs/root.xml", ROOT, true).await;
    good.read("/r/specs/rules.xml", RULES, true).await;
    good.read("/r/specs/tasks/ENG-1.xml", TASK, true).await;
    good.read("/r/config/app.json", "{}\n", true).await;
    good.edit("/r/app/tool.py").await;
    good.read("/r/app/tool.py", "x\n", true).await;

    // A build that reads the platform last, and never reads the spec again.
    let mut late = Session::new(&kernel, "late-platform").await;
    late.say("Build specs/tasks/ENG-1.xml").await;
    late.read("/r/specs/root.xml", ROOT, true).await;
    late.read("/r/specs/rules.xml", RULES, true).await;
    late.read("/r/specs/tasks/ENG-1.xml", TASK, true).await;
    late.read("/r/app/core/door.py", "print('door')\n", true).await;
    late.edit("/r/app/tool.py").await;

    // A build that stopped at its kickoff and wrote nothing.
    let mut stopped = Session::new(&kernel, "stopped").await;
    stopped.say("Build specs/tasks/ENG-1.xml").await;
    stopped.read("/r/specs/root.xml", ROOT, true).await;
    stopped.read("/r/specs/tasks/ENG-1.xml", TASK, true).await;

    // A build that read only its task file, then wrote: it never read the
    // root, and still owed it its order.
    let mut skipped = Session::new(&kernel, "skipped-root").await;
    skipped.say("Build specs/tasks/ENG-1.xml.").await;
    skipped.read("/r/specs/tasks/ENG-1.xml", TASK, true).await;
    skipped.edit("/r/app/tool.py").await;

    // A design session: it reads the same files, but it opened with a
    // conversation, not a kickoff line.
    let mut design = Session::new(&kernel, "design").await;
    design.say("lets continue with the spec").await;
    design.read("/r/specs/tasks/ENG-1.xml", TASK, true).await;
    design.edit("/r/specs/tasks/ENG-1.xml").await;

    let report = ingest::ingest(&kernel, &db).await.expect("ingest");
    assert_eq!((report.sessions, report.builds, report.runs_written), (5, 4, 4), "{report:?}");
    assert_eq!(report.docs_written, 3, "root, rules and task, once each however often they were read");
    assert_eq!(count(&db, "spec_doc").await, 3);
    assert_eq!(count(&db, "spec_clause").await, 2 + 2 + 9, "the root's steps, the rules, the task's clauses");

    let runs = store::current_runs(&db).await.expect("runs");
    assert_eq!(runs.len(), 4, "the design session is no build");
    let by_key = |key: &str| {
        runs.iter()
            .find(|(r, _)| superx_mod_specs::session::get_str(r, "session_key").as_deref() == Some(key))
            .expect("run")
    };

    let (run, findings) = by_key("good-build");
    let s = |k: &str| superx_mod_specs::session::get_str(run, k);
    assert_eq!(s("key").as_deref(), Some("ENG-1"));
    assert_eq!(superx_mod_specs::session::get_int(run, "version"), Some(3));
    assert_eq!(s("branch").as_deref(), Some(BRANCH));
    assert_eq!(s("label").as_deref(), Some("sonnet-5-5-max"), "what the branch says after <key>-<name>-v<version>-");
    assert_eq!(s("model").as_deref(), Some("claude-sonnet-5-5"));
    assert_eq!(s("first_write").as_deref(), Some("/r/app/tool.py"));
    match run {
        Value::Object(o) => match o.get("reads") {
            Some(Value::Array(a)) => assert_eq!(a.len(), 6, "the reads before the first write, and not the one after"),
            other => panic!("reads: {other:?}"),
        },
        _ => panic!("row"),
    }
    assert_eq!(findings.len(), 1);
    let f = |k: &str| superx_mod_specs::session::get_str(&findings[0], k);
    assert_eq!(f("check").as_deref(), Some("read-order"));
    assert_eq!(f("result").as_deref(), Some("complied"), "{:?}", f("note"));

    let (_, findings) = by_key("late-platform");
    let f = |k: &str| superx_mod_specs::session::get_str(&findings[0], k);
    assert_eq!(f("result").as_deref(), Some("breached"));
    assert!(f("note").unwrap_or_default().contains("root.xml, rules.xml, the task file"), "{:?}", f("note"));

    let (run, findings) = by_key("stopped");
    assert_eq!(superx_mod_specs::session::get_str(run, "first_write"), None);
    let f = |k: &str| superx_mod_specs::session::get_str(&findings[0], k);
    assert_eq!(f("result").as_deref(), Some("no-evidence"), "a build that wrote nothing broke no order: {:?}", f("note"));

    let (run, findings) = by_key("skipped-root");
    assert_eq!(superx_mod_specs::session::get_str(run, "root_doc"), None, "the session read no root");
    let f = |k: &str| superx_mod_specs::session::get_str(&findings[0], k);
    assert_eq!(f("result").as_deref(), Some("breached"), "{:?}", f("note"));
    let note = f("note").unwrap_or_default();
    assert!(note.contains("did not read, in order: the platform code, root.xml, rules.xml"), "{note}");
    assert!(note.contains("judged against the root first read by another session"), "{note}");

    // Nothing changed: a second ingest writes nothing.
    let again = ingest::ingest(&kernel, &db).await.expect("ingest again");
    assert_eq!((again.runs_written, again.runs_unchanged, again.docs_written), (0, 4, 0), "{again:?}");
    assert_eq!(count(&db, "spec_run").await, 4);
    assert_eq!(count(&db, "spec_finding").await, 4);

    // The finding cites the clause it was checked against.
    let clause: Vec<Value> = db
        .query("SELECT VALUE clause.text FROM spec_finding WHERE result = 'complied'")
        .await
        .expect("clause")
        .take(0)
        .expect("rows");
    assert!(matches!(clause.first(), Some(Value::String(t)) if t.contains("in this order")), "{clause:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_long_session_is_read_page_by_page() {
    let kernel = fresh_kernel().await;
    let db = specs_db().await;
    let mut s = Session::new(&kernel, "long").await;
    s.say("Build specs/tasks/ENG-1.xml").await;
    s.read("/r/specs/tasks/ENG-1.xml", TASK, true).await;
    s.read("/r/app/core/door.py", "x\n", true).await;
    s.read("/r/specs/root.xml", ROOT, true).await;
    s.read("/r/specs/rules.xml", RULES, true).await;
    s.read("/r/specs/tasks/ENG-1.xml", TASK, true).await;
    s.read("/r/config/app.json", "{}\n", true).await;
    // More than one page of reads before the write.
    let pages = usize::try_from(store::PAGE_ROWS).expect("fits");
    for i in 0..pages {
        s.read(&format!("/r/docs/{i}.md"), "x\n", true).await;
    }
    s.edit("/r/app/tool.py").await;

    let (rows, cut) = store::timeline(&kernel, &s.id).await.expect("timeline");
    assert!(!cut);
    assert_eq!(rows.len(), 2 * (6 + pages) + 1, "every row, across pages, once");
    let report = ingest::ingest(&kernel, &db).await.expect("ingest");
    assert_eq!(report.runs_written, 1);
    let runs = store::current_runs(&db).await.expect("runs");
    let first_write = superx_mod_specs::session::get_str(&runs[0].0, "first_write");
    assert_eq!(first_write.as_deref(), Some("/r/app/tool.py"), "the write after the page boundary is found");
}
