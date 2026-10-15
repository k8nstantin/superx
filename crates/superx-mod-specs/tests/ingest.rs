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
    key: String,
    model: &'static str,
    n: usize,
}

impl<'k> Session<'k> {
    async fn new(kernel: &'k Kernel, key: &str) -> Self {
        Self::with_model(kernel, key, "claude-sonnet-5-5").await
    }

    async fn with_model(kernel: &'k Kernel, key: &str, model: &'static str) -> Self {
        let agent = kernel.create_entity("node_agent").await.expect("agent");
        let id = kernel.create_entity("node_session").await.expect("session");
        let mut desc = Object::new();
        desc.insert("name".to_string(), Value::String(format!("claude_code/{key}")));
        desc.insert("session".to_string(), Value::String(key.to_string()));
        desc.insert("locator".to_string(), Value::String("/tmp/x.jsonl".to_string()));
        kernel.supersede_state(id.clone(), "attr_session_descriptor", Value::Object(desc)).await.expect("descriptor");
        Self { kernel, agent, id, key: key.to_string(), model, n: 0 }
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
        // Launched in the repository `/r`, as a build is.
        self.line("user", text, json!({"type": "user", "origin": {"kind": "human"}, "gitBranch": "main", "cwd": "/r",
            "message": {"role": "user", "content": text}}))
            .await;
    }

    /// A Read call and its result, the file's bytes as the tool returns them.
    /// A fresh call id, unique across sessions as real ones are.
    fn call(&mut self) -> String {
        self.n += 1;
        format!("toolu_{}_{}", self.key, self.n)
    }

    async fn read(&mut self, path: &str, content: &str, whole: bool) {
        let id = self.call();
        let lines = content.lines().count();
        self.line("assistant", "", json!({"type": "assistant", "gitBranch": BRANCH,
            "message": {"model": self.model, "content": [
                {"type": "tool_use", "id": id, "name": "Read", "input": {"file_path": path}}]}}))
            .await;
        self.line("tool", "", json!({"type": "user", "gitBranch": BRANCH,
            "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": "1\t…"}]},
            "toolUseResult": {"type": "text", "file": {"filePath": path, "content": content,
                "startLine": 1, "numLines": if whole { lines } else { 1 }, "totalLines": lines}}}))
            .await;
    }

    async fn edit(&mut self, path: &str) {
        let id = self.call();
        self.line("assistant", "", json!({"type": "assistant", "gitBranch": BRANCH,
            "message": {"model": self.model, "content": [
                {"type": "text", "text": "Writing the tool."},
                {"type": "tool_use", "id": id, "name": "Edit",
                 "input": {"file_path": path, "old_string": "a", "new_string": "b"}}]}}))
            .await;
    }

    /// A Write call: a whole file's content.
    async fn write(&mut self, path: &str, content: &str) {
        let id = self.call();
        self.line("assistant", "", json!({"type": "assistant", "gitBranch": "sandbox",
            "message": {"model": self.model, "content": [
                {"type": "tool_use", "id": id, "name": "Write", "input": {"file_path": path, "content": content}}]}}))
            .await;
    }

    /// A shell call and its result.
    async fn bash(&mut self, command: &str, stdout: &str, failed: bool) {
        let id = self.call();
        self.line("assistant", "", json!({"type": "assistant", "gitBranch": "sandbox",
            "message": {"model": self.model, "content": [
                {"type": "tool_use", "id": id, "name": "Bash", "input": {"command": command}}]}}))
            .await;
        self.line("tool", "", json!({"type": "user", "gitBranch": "sandbox",
            "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": id, "content": stdout, "is_error": failed}]},
            "toolUseResult": {"stdout": stdout, "stderr": "", "interrupted": false}}))
            .await;
    }
}

/// The result and note of one check among a run's findings.
fn verdict(findings: &[Value], check: &str) -> (String, String) {
    let f = findings
        .iter()
        .find(|f| superx_mod_specs::session::get_str(f, "check").as_deref() == Some(check))
        .unwrap_or_else(|| panic!("no {check} finding"));
    (
        superx_mod_specs::session::get_str(f, "result").unwrap_or_default(),
        superx_mod_specs::session::get_str(f, "note").unwrap_or_default(),
    )
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
    // Its gates, run in the session, and its PR body with the gate table —
    // a scratch file outside the repository, which is not the repository's.
    good.bash("cd /r && python -m lint --check", "0 violations", false).await;
    good.bash("gcloud run jobs execute door --args app,tool,check --wait", "Execution door-1 completed: run 20261015t010000z", false).await;
    good.write(
        "/s/scratch/pr_body.md",
        "## Gates\n\n| id | kind | exit | matched | run id |\n|---|---|---|---|---|\n\
         | G1 | local | 0 | 0 violations | local |\n| G2 | door | 0 | the finding line | `20261015t010000z` |\n",
    )
    .await;

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

    // A build that reads in order, then writes what its task forbids and
    // what it does not own.
    let mut overreach = Session::new(&kernel, "overreach").await;
    overreach.say("Build specs/tasks/ENG-1.xml").await;
    overreach.read("/r/app/core/door.py", "x\n", true).await;
    overreach.read("/r/specs/root.xml", ROOT, true).await;
    overreach.read("/r/specs/rules.xml", RULES, true).await;
    overreach.read("/r/specs/tasks/ENG-1.xml", TASK, true).await;
    overreach.read("/r/config/app.json", "{}\n", true).await;
    overreach.edit("/r/app/tool.py").await;
    overreach.edit("/r/design/a.json").await;
    overreach.write("/r/README.md", "notes").await;
    overreach.edit("/r/design/a.json").await;

    // A design session: it reads the same files, but it opened with a
    // conversation, not a kickoff line.
    let mut design = Session::new(&kernel, "design").await;
    design.say("lets continue with the spec").await;
    design.read("/r/specs/tasks/ENG-1.xml", TASK, true).await;
    design.edit("/r/specs/tasks/ENG-1.xml").await;

    let report = ingest::ingest(&kernel, &db).await.expect("ingest");
    assert_eq!((report.sessions, report.builds, report.runs_written), (6, 5, 5), "{report:?}");
    assert_eq!(report.docs_written, 3, "root, rules and task, once each however often they were read");
    assert_eq!(count(&db, "spec_doc").await, 3);
    assert_eq!(count(&db, "spec_clause").await, 2 + 2 + 9, "the root's steps, the rules, the task's clauses");

    let runs = store::current_runs(&db).await.expect("runs");
    assert_eq!(runs.len(), 5, "the design session is no build");
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
    let checks: Vec<String> = findings.iter().filter_map(|f| superx_mod_specs::session::get_str(f, "check")).collect();
    assert_eq!(checks, ["read-order", "ownership", "gates"], "every check, in the order they were first recorded");
    let (result, note) = verdict(findings, "gates");
    assert_eq!(result, "complied", "{note}");
    let (result, note) = verdict(findings, "read-order");
    assert_eq!(result, "complied", "{note}");
    let (result, note) = verdict(findings, "ownership");
    assert_eq!(result, "complied", "{note}");
    assert!(note.contains("(1 writes in the repository)"), "the scratch file outside it is not judged: {note}");

    let (_, findings) = by_key("late-platform");
    let (result, note) = verdict(findings, "read-order");
    assert_eq!(result, "breached");
    assert!(note.contains("root.xml, rules.xml, the task file"), "{note}");
    assert_eq!(verdict(findings, "ownership").0, "complied", "a check is judged on its own");

    let (run, findings) = by_key("stopped");
    assert_eq!(superx_mod_specs::session::get_str(run, "first_write"), None);
    let (result, note) = verdict(findings, "read-order");
    assert_eq!(result, "no-evidence", "a build that wrote nothing broke no order: {note}");
    assert_eq!(verdict(findings, "ownership").0, "no-evidence", "nor wrote anything it did not own");

    let (_, findings) = by_key("overreach");
    assert_eq!(verdict(findings, "read-order").0, "complied");
    assert_eq!(verdict(findings, "gates").0, "no-evidence", "it never wrote a gate table");
    let (result, note) = verdict(findings, "ownership");
    assert_eq!(result, "breached", "{note}");
    assert_eq!(note, "wrote what the task file forbids: design/a.json; wrote what the task file does not own: README.md");
    let ownership = findings
        .iter()
        .find(|f| superx_mod_specs::session::get_str(f, "check").as_deref() == Some("ownership"))
        .expect("ownership");
    match ownership {
        Value::Object(o) => match o.get("evidence") {
            Some(Value::Array(a)) => assert_eq!(a.len(), 2, "one call per path, the first that wrote it"),
            other => panic!("evidence: {other:?}"),
        },
        _ => panic!("finding"),
    }

    let (run, findings) = by_key("skipped-root");
    assert_eq!(superx_mod_specs::session::get_str(run, "root_doc"), None, "the session read no root");
    let (result, note) = verdict(findings, "read-order");
    assert_eq!(result, "breached", "{note}");
    assert!(note.contains("did not read, in order: the platform code, root.xml, rules.xml"), "{note}");
    assert!(note.contains("judged against the root first read by another session"), "{note}");

    // A verdict the module no longer gives — as after the check learned to
    // read better — is re-judged: a new finding is appended to the same
    // run, and it is the one that stands.
    let good_run: Vec<superx_kernel::types::RecordId> = db
        .query("SELECT VALUE id FROM spec_run WHERE session_key = 'good-build'")
        .await
        .expect("run")
        .take(0)
        .expect("ids");
    db.query("CREATE spec_finding CONTENT { run: $run, check: 'read-order', result: 'no-evidence', \
              note: 'an older verdict', evidence: [], valid_from: time::now() }")
        .bind(("run", good_run[0].clone()))
        .await
        .expect("stale")
        .check()
        .expect("stale finding written");
    let rejudge = ingest::ingest(&kernel, &db).await.expect("rejudge");
    assert_eq!((rejudge.runs_written, rejudge.rejudged), (0, 1), "{rejudge:?}");
    let runs = store::current_runs(&db).await.expect("runs");
    let (_, findings) = runs
        .iter()
        .find(|(r, _)| superx_mod_specs::session::get_str(r, "session_key").as_deref() == Some("good-build"))
        .expect("run");
    assert_eq!(findings.len(), 3, "one finding per check: the newest");
    assert_eq!(verdict(findings, "read-order").0, "complied");

    // Nothing changed: a second ingest writes nothing.
    let again = ingest::ingest(&kernel, &db).await.expect("ingest again");
    assert_eq!((again.runs_written, again.runs_unchanged, again.rejudged, again.docs_written), (0, 5, 0, 0), "{again:?}");
    assert_eq!(count(&db, "spec_run").await, 5);
    assert_eq!(count(&db, "spec_finding").await, 5 * 3 + 2, "the stale finding and its re-judgement are kept: append-only");

    // The finding cites the clause it was checked against.
    let clause: Vec<Value> = db
        .query("SELECT VALUE clause.text FROM spec_finding WHERE result = 'complied' AND check = 'read-order'")
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_ledger_ties_spec_versions_builds_and_branches() {
    let kernel = fresh_kernel().await;
    let db = specs_db().await;
    let task_v4 = TASK.replace("version=\"3\"", "version=\"4\"").replace("proves=\"step 1\"", "proves=\"step 1, on the config\"");

    // The design session writes version 3, by a message file.
    let mut design = Session::with_model(&kernel, "design-1", "claude-opus-5-5").await;
    design.say("lets write version 3").await;
    design.read("/w/specs/root.xml", ROOT, true).await;
    design.read("/w/specs/tasks/ENG-1.xml", TASK, true).await;
    design.write("/s/scratch/commit_msg.txt", "ENG-7811 spec v0: ENG-1 file version 3\n\nThe first version.").await;
    design
        .bash(
            "S=/s/scratch; cd /w && git add specs/tasks/ENG-1.xml && git commit -q -F $S/commit_msg.txt && git log --oneline -1",
            "abc1234 ENG-7811 spec v0: ENG-1 file version 3",
            false,
        )
        .await;

    // A build of version 3, on its branch, that follows its read order.
    let mut build = Session::new(&kernel, "build-v3").await;
    build.say("Build specs/tasks/ENG-1.xml").await;
    build.read("/r/specs/tasks/ENG-1.xml", TASK, true).await;
    build.read("/r/app/core/door.py", "x\n", true).await;
    build.read("/r/specs/root.xml", ROOT, true).await;
    build.read("/r/specs/rules.xml", RULES, true).await;
    build.read("/r/specs/tasks/ENG-1.xml", TASK, true).await;
    build.read("/r/config/app.json", "{}\n", true).await;
    build.bash("cd /r && git checkout -b ENG-1-the-tool-v3-sonnet-5-5-max", "", false).await;
    build.edit("/r/app/tool.py").await;

    // The design session reviews it, deletes its branch, writes version 4
    // by a here-document — and a commit that failed names a version 9.
    design.bash("git push origin --delete ENG-1-the-tool-v3-sonnet-5-5-max", " - [deleted] ENG-1-the-tool-v3-sonnet-5-5-max", false).await;
    design
        .bash(
            "cat > $S/m4.txt <<'EOF'\nENG-7811 spec v0: ENG-1 file version 4 from the review of its v3 build: G2 runs on the config\nEOF\ngit commit -q -F $S/m4.txt && git log --oneline -1",
            "def5678 ENG-7811 spec v0: ENG-1 file version 4 from the review of its v3 build: G2 runs on the config",
            false,
        )
        .await;
    design.read("/w/specs/tasks/ENG-1.xml", &task_v4, true).await;
    design.bash("git commit -q -m \"ENG-7811 spec v0: ENG-1 file version 9\"", "nothing to commit", true).await;

    let report = ingest::ingest(&kernel, &db).await.expect("ingest");
    assert_eq!((report.builds, report.revisions_written, report.branch_events_written), (1, 2, 2), "{report:?}");

    let revisions: Vec<Value> =
        db.query("SELECT * FROM spec_revision ORDER BY version ASC").await.expect("revisions").take(0).expect("rows");
    let s = |r: &Value, k: &str| superx_mod_specs::session::get_str(r, k);
    let i = |r: &Value, k: &str| superx_mod_specs::session::get_int(r, k);
    assert_eq!(revisions.len(), 2, "a failed commit is no revision");
    assert_eq!((i(&revisions[0], "version"), s(&revisions[0], "commit").as_deref()), (Some(3), Some("abc1234")));
    assert_eq!(s(&revisions[0], "model").as_deref(), Some("claude-opus-5-5"));
    assert!(s(&revisions[0], "message").unwrap_or_default().contains("The first version."), "the message comes from the file the session wrote");
    assert_eq!((i(&revisions[1], "version"), i(&revisions[1], "reviewed")), (Some(4), Some(3)));
    assert_eq!(s(&revisions[1], "commit").as_deref(), Some("def5678"));

    let ledger = superx_mod_specs::ledger::render(&db, Some("ENG-1")).await.expect("ledger");
    for want in [
        "v3  committed",
        "by claude-opus-5-5 (session design-1)",
        "read-order: complied",
        "branch ENG-1-the-tool-v3-sonnet-5-5-max: deleted on the remote",
        "v4  committed",
        "from the review of its v3 build",
        "changed: gate G2",
        "1 builds, 0 merged; ownership kept by 1, broken by 0; read-order kept by 1, broken by 0; 1 spec versions committed after a build — committed by claude-opus-5-5 ×1",
    ] {
        assert!(ledger.contains(want), "the ledger lacks {want:?}:\n{ledger}");
    }

    // A second ingest adds nothing.
    let again = ingest::ingest(&kernel, &db).await.expect("again");
    assert_eq!((again.revisions_written, again.branch_events_written, again.runs_written), (0, 0, 0), "{again:?}");
}
