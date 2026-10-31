//! The one crawler (#510), end to end: a kernel holding sessions' lines as
//! capture keeps them, real git repositories in a scratch directory, and
//! the module's own database, on the in-memory engine. What a pass writes
//! is read back from the database. Every pass after the first reads the
//! minute before the last one stopped, so these tests read lines again on
//! purpose.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::{DateTime, TimeDelta, TimeZone as _, Utc};
use superx_kernel::message::{json_to_object, line_to_object, line_value_from_json};
use superx_kernel::types::{Object, RecordId, Value};
use superx_kernel::{Kernel, REQUIRED_METAMODEL_TYPES};
use superx_mod_recon::crawler::{pass_on, Crawler, Pass, Stop};
use superx_mod_recon::params::{from_values, Params, EXCLUDE_PARAM, INCLUDE_PARAM};
use superx_mod_recon::{MODULE_NAME, SCHEMA_DDL};
use tokio::sync::watch;

const KERNEL_PASSWORD: &str = "test-kernel-password-for-mem-engine";

async fn booted_kernel() -> Kernel {
    let db = surrealdb::engine::any::connect("mem://").await.expect("mem");
    db.use_ns("superx").use_db("kernel").await.expect("nsdb");
    let ddl = superx_kernel::SCHEMA_DDL.replace("$SUPERX_KERNEL_PASSWORD", KERNEL_PASSWORD);
    db.query(ddl).await.expect("ddl").check().expect("ddl ok");
    db.signin(surrealdb::opt::auth::Database {
        namespace: "superx".to_string(),
        database: "kernel".to_string(),
        username: "superx_kernel".to_string(),
        password: KERNEL_PASSWORD.to_string(),
    })
    .await
    .expect("service sign-in");
    let kernel = Kernel::from_db(db);
    for t in REQUIRED_METAMODEL_TYPES {
        kernel.ensure_type_definition(t.uid, t.category, t.memory_tier).await.expect("metamodel");
    }
    kernel
}

async fn own_db() -> superx_kernel::Db {
    let db = surrealdb::engine::any::connect("mem://").await.expect("mem");
    db.use_ns("superx").use_db(MODULE_NAME).await.expect("nsdb");
    db.query(SCHEMA_DDL.replace("$SUPERX_MODULE_PASSWORD", "test-password"))
        .await
        .expect("provision")
        .check()
        .expect("provision is clean");
    db
}

/// An agent and one of its sessions, as capture registers them.
async fn agent_and_session(kernel: &Kernel, agent_name: &str, key: &str) -> (RecordId, RecordId) {
    let agent = kernel.create_entity("node_agent").await.expect("agent");
    let mut d = Object::new();
    d.insert("name".to_string(), Value::String(agent_name.to_string()));
    kernel.supersede_state(agent.clone(), "attr_agent_descriptor", Value::Object(d)).await.expect("agent descriptor");
    let session = kernel.create_entity("node_session").await.expect("session");
    let mut d = Object::new();
    d.insert("name".to_string(), Value::String(format!("{agent_name}/{key}")));
    d.insert("session".to_string(), Value::String(key.to_string()));
    kernel.supersede_state(session.clone(), "attr_session_descriptor", Value::Object(d)).await.expect("session descriptor");
    (agent, session)
}

fn start() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 25, 9, 0, 0).single().expect("time")
}

/// An id the kernel would give a line it captured at `at`.
fn id_at(table: &str, at: DateTime<Utc>) -> RecordId {
    let stamp = uuid::Timestamp::from_unix(
        uuid::NoContext,
        u64::try_from(at.timestamp()).expect("after 1970"),
        at.timestamp_subsec_nanos(),
    );
    RecordId::new(table, superx_kernel::types::Uuid::from(uuid::Uuid::new_v7(stamp)))
}

/// The session's lines, captured a tenth of a second apart from `from`.
struct Capture<'a> {
    kernel: &'a Kernel,
    agent: RecordId,
    session: RecordId,
    key: String,
    at: DateTime<Utc>,
}

impl Capture<'_> {
    fn tick(&mut self) -> DateTime<Utc> {
        self.at += TimeDelta::milliseconds(100);
        self.at
    }

    /// A conversation line, as `message` keeps it.
    async fn message(&mut self, json: serde_json::Value) {
        let at = self.tick();
        let mut json = json;
        json["timestamp"] = serde_json::Value::String(at.to_rfc3339());
        json["sessionId"] = serde_json::Value::String(self.key.clone());
        self.kernel
            .db()
            .query(
                "CREATE $id CONTENT { session: $sess, agent: $agent, role: 'x', content: '', raw: $raw, \
                 emitted_at: $at, valid_from: $at }",
            )
            .bind(("id", id_at("message", at)))
            .bind(("sess", self.session.clone()))
            .bind(("agent", self.agent.clone()))
            .bind(("raw", Value::Object(json_to_object(&json))))
            .bind(("at", at))
            .await
            .expect("line")
            .check()
            .expect("line taken");
    }

    /// Any other line, as a `transcript_event` keeps it whole.
    async fn event(&mut self, json: serde_json::Value) {
        let at = self.tick();
        let mut json = json;
        json["timestamp"] = serde_json::Value::String(at.to_rfc3339());
        let kind = json["type"].as_str().unwrap_or_default().to_string();
        let payload = serde_json::json!({ "kind": kind, "session": self.key, "file": "/t.jsonl", "line": json });
        self.kernel
            .db()
            .query("CREATE $id CONTENT { lifecycle_event: 'transcript_event', payload: $payload, agent: $agent, valid_from: $at }")
            .bind(("id", id_at("telemetry_stream", at)))
            .bind(("payload", Value::Object(json_to_object(&payload))))
            .bind(("agent", self.agent.clone()))
            .bind(("at", at))
            .await
            .expect("event")
            .check()
            .expect("event taken");
    }

    /// Something else the firehose holds, which is not a line.
    async fn noise(&mut self) {
        let at = self.tick();
        self.kernel
            .db()
            .query("CREATE $id CONTENT { lifecycle_event: 'message_captured', payload: { role: 'user' }, valid_from: $at }")
            .bind(("id", id_at("telemetry_stream", at)))
            .bind(("at", at))
            .await
            .expect("noise")
            .check()
            .expect("noise taken");
    }
}

fn usage(output: i64, thinking: i64) -> serde_json::Value {
    serde_json::json!({
        "input_tokens": 5, "cache_creation_input_tokens": 100, "cache_read_input_tokens": 2000, "output_tokens": output,
        "service_tier": "standard", "output_tokens_details": { "thinking_tokens": thinking },
        "server_tool_use": { "web_search_requests": 0, "web_fetch_requests": 0 }
    })
}

fn block(id: &str, index: i64, content: serde_json::Value, extra: serde_json::Value) -> serde_json::Value {
    let mut line = serde_json::json!({
        "type": "assistant", "cwd": "/repo", "gitBranch": "main", "entrypoint": "cli", "version": "2.1.304",
        "isSidechain": false, "requestId": "req", "effort": "max", "apiBlockIndex": index,
        "message": { "id": id, "model": "claude-opus-5-5", "stop_reason": "tool_use", "content": [content], "usage": usage(900, 600) }
    });
    if let (Some(l), serde_json::Value::Object(e)) = (line.as_object_mut(), extra) {
        l.extend(e);
    }
    line
}

fn result(call: &str, content: &str, reported: serde_json::Value, error: bool) -> serde_json::Value {
    serde_json::json!({
        "type": "user", "cwd": "/repo", "gitBranch": "main",
        "message": { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": call, "content": content, "is_error": error }] },
        "toolUseResult": reported
    })
}

/// A Claude Code session: a prompt, a reply that thinks, answers and
/// commits, an edit, a test run, an interrupted call, and the lines Claude
/// Code keeps beside the conversation.
async fn claude_session(c: &mut Capture<'_>) {
    c.message(serde_json::json!({ "type": "user", "cwd": "/repo", "gitBranch": "main", "origin": { "kind": "human" }, "message": { "role": "user", "content": "commit the fix" } })).await;
    c.noise().await;
    c.message(block("msg_1", 0, serde_json::json!({ "type": "thinking", "thinking": "", "signature": "s" }), serde_json::json!({ "thinkingDurationMs": 3100 }))).await;
    c.message(block("msg_1", 1, serde_json::json!({ "type": "text", "text": "Committing." }), serde_json::json!({}))).await;
    c.message(block("msg_1", 2, serde_json::json!({ "type": "tool_use", "id": "call_commit", "name": "Bash", "input": { "command": "git commit -m 'fix: x'", "description": "Commit" } }), serde_json::json!({}))).await;
    c.message(result("call_commit", "[main abc1234] fix: x", serde_json::json!({ "stdout": "[main abc1234] fix: x\n 1 file changed", "stderr": "", "interrupted": false }), false)).await;
    c.event(serde_json::json!({ "type": "attachment", "cwd": "/repo", "attachment": { "type": "queued_command", "prompt": "and run the tests", "origin": { "kind": "human" } } })).await;
    c.message(block("msg_2", 0, serde_json::json!({ "type": "tool_use", "id": "call_edit", "name": "Edit", "input": { "file_path": "/repo/a.rs", "old_string": "a", "new_string": "b" } }), serde_json::json!({}))).await;
    c.message(result("call_edit", "updated", serde_json::json!({ "filePath": "/repo/a.rs", "oldString": "a", "newString": "b", "userModified": false, "structuredPatch": [{ "lines": ["-a", "+b", "+c"] }] }), false)).await;
    c.message(block("msg_3", 0, serde_json::json!({ "type": "tool_use", "id": "call_test", "name": "Bash", "input": { "command": "cd /repo && cargo test" } }), serde_json::json!({}))).await;
    c.message(result("call_test", "ok", serde_json::json!({ "stdout": "    Finished `test` profile\ntest result: ok. 7 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.1s", "stderr": "" }), false)).await;
    c.message(block("msg_4", 0, serde_json::json!({ "type": "tool_use", "id": "call_long", "name": "Bash", "input": { "command": "sleep 100" } }), serde_json::json!({}))).await;
    c.message(serde_json::json!({ "type": "user", "cwd": "/repo", "message": { "role": "user", "content": [{ "type": "text", "text": "[Request interrupted by user for tool use]" }] } })).await;
    c.event(serde_json::json!({ "type": "system", "subtype": "turn_duration", "cwd": "/repo", "durationMs": 9000, "messageCount": 12 })).await;
    c.event(serde_json::json!({ "type": "system", "subtype": "compact_boundary", "cwd": "/repo", "compactMetadata": { "trigger": "auto", "preTokens": 900_000, "postTokens": 20_000, "cumulativeDroppedTokens": 880_000, "durationMs": 60_000 } })).await;
    c.event(serde_json::json!({ "type": "system", "subtype": "away_summary", "cwd": "/repo/sub", "gitBranch": "main", "content": "While you were away: committed." })).await;
}

async fn count(db: &superx_kernel::Db, table: &str) -> i64 {
    let n: Option<i64> =
        db.query(format!("RETURN (SELECT count() AS n FROM {table} GROUP ALL)[0].n")).await.expect("count").take(0).expect("n");
    n.unwrap_or(0)
}

async fn rows(db: &superx_kernel::Db, sql: &str) -> Vec<Value> {
    db.query(sql).await.expect("query").take(0).expect("rows")
}

fn text(v: &Value, key: &str) -> Option<String> {
    match v {
        Value::Object(o) => match o.get(key) {
            Some(Value::String(s)) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn int(v: &Value, key: &str) -> Option<i64> {
    match v {
        Value::Object(o) => match o.get(key) {
            Some(Value::Number(n)) => n.to_int(),
            _ => None,
        },
        _ => None,
    }
}

/// Rows in a known order: the engine's order of rows written within a
/// second is not one to rely on (#486).
fn sorted<T: Ord>(items: impl Iterator<Item = T>) -> Vec<T> {
    let mut v: Vec<T> = items.collect();
    v.sort();
    v
}

fn params(set: &[(&'static str, Value)]) -> Params {
    let values: BTreeMap<&'static str, Value> = set.iter().cloned().collect();
    from_values(&values).params
}

fn lines_read(p: &Pass) -> i64 {
    p.source("lines").map_or(0, |s| s.items)
}

fn lines_unread(p: &Pass) -> i64 {
    p.source("lines").map_or(0, |s| s.unread)
}

fn lines_rows(p: &Pass) -> i64 {
    p.source("lines").map_or(0, superx_mod_recon::crawler::SourceRun::rows_written)
}

fn never_stop() -> (watch::Sender<bool>, Stop) {
    let (asked, watched) = watch::channel(false);
    (asked, Stop::new(watched))
}

const TABLES: [&str; 15] = [
    "session", "session_place", "prompt", "reply", "reply_state", "thinking", "tool_call", "tool_result", "edit",
    "git_event", "test_run", "build_run", "turn", "compaction", "away",
];

async fn every_count(db: &superx_kernel::Db) -> Vec<(&'static str, i64)> {
    let mut out = Vec::new();
    for t in TABLES {
        out.push((t, count(db, t).await));
    }
    out
}

#[tokio::test]
async fn a_pass_reads_every_line_into_its_facts_and_a_second_pass_writes_nothing() {
    let kernel = booted_kernel().await;
    let db = own_db().await;
    let (agent, session) = agent_and_session(&kernel, "claude_code", "s1").await;
    let mut c = Capture { kernel: &kernel, agent, session, key: "s1".to_string(), at: start() };
    claude_session(&mut c).await;

    let (_asked, mut stop) = never_stop();
    let mut crawler = Crawler::default();
    let first = pass_on(&kernel, &db, &params(&[]), &mut crawler, &mut stop).await.expect("pass");
    assert_eq!((first.outcome, lines_read(&first), lines_unread(&first)), ("complete", 15, 0), "{first:?}");
    let counts = every_count(&db).await;
    assert_eq!(
        counts,
        vec![
            ("session", 1),
            ("session_place", 2),
            ("prompt", 3),
            ("reply", 4),
            ("reply_state", 5),
            ("thinking", 1),
            ("tool_call", 4),
            ("tool_result", 4),
            ("edit", 1),
            ("git_event", 1),
            ("test_run", 1),
            ("build_run", 1),
            ("turn", 1),
            ("compaction", 1),
            ("away", 1),
        ]
    );
    assert_eq!(
        lines_rows(&first),
        counts.iter().map(|(_, n)| n).sum::<i64>() + count(&db, "line_field").await,
        "every row is a fact of the lines or a field they showed"
    );
    assert_eq!(
        sorted(rows(&db, "SELECT path, exact FROM directory").await.iter().filter_map(|d| text(d, "path"))),
        ["/repo", "/repo/sub"],
        "the repos source followed both directories the session worked in: no repository holds them"
    );

    let s = rows(&db, "SELECT agent, agent_id, entrypoint, client_version FROM session").await;
    assert_eq!(
        (text(&s[0], "agent"), text(&s[0], "agent_id"), text(&s[0], "entrypoint"), text(&s[0], "client_version")),
        (Some("claude_code".into()), Some("s1".into()), None, None),
        "its first line names no entrypoint or version"
    );
    let kinds = rows(&db, "SELECT kind, text, at FROM prompt ORDER BY at").await;
    assert_eq!(
        kinds.iter().map(|p| text(p, "kind").unwrap_or_default()).collect::<Vec<_>>(),
        ["typed", "queued", "interrupt"]
    );
    let reply = rows(&db, "SELECT model, effort, stop_reason FROM reply WHERE agent_id = 'msg_1'").await;
    assert_eq!((text(&reply[0], "model"), text(&reply[0], "effort")), (Some("claude-opus-5-5".into()), Some("max".into())));
    let states = rows(&db, "SELECT text_chars, output_tokens, thinking_tokens FROM reply_state WHERE reply.agent_id = 'msg_1'").await;
    assert_eq!(
        sorted(states.iter().map(|s| (int(s, "text_chars"), int(s, "output_tokens"), int(s, "thinking_tokens")))),
        [(Some(0), Some(900), Some(600)), (Some(11), Some(900), Some(600))],
        "a state when the reply is first read, another when its text arrives"
    );
    let thinking = rows(&db, "SELECT position, ms FROM thinking").await;
    assert_eq!((int(&thinking[0], "position"), int(&thinking[0], "ms")), (Some(0), Some(3100)));
    let outcomes = rows(&db, "SELECT tool_call.call_id AS call, outcome FROM tool_result ORDER BY call").await;
    assert_eq!(
        outcomes.iter().map(|o| (text(o, "call").unwrap_or_default(), text(o, "outcome").unwrap_or_default())).collect::<Vec<_>>(),
        [
            ("call_commit".to_string(), "ok".to_string()),
            ("call_edit".to_string(), "ok".to_string()),
            ("call_long".to_string(), "interrupted".to_string()),
            ("call_test".to_string(), "ok".to_string()),
        ]
    );
    let git = rows(&db, "SELECT action, hash, branch, subject, dir FROM git_event").await;
    assert_eq!(
        (text(&git[0], "action"), text(&git[0], "hash"), text(&git[0], "branch"), text(&git[0], "subject"), text(&git[0], "dir")),
        (Some("commit".into()), Some("abc1234".into()), Some("main".into()), Some("fix: x".into()), Some("/repo".into()))
    );
    let test = rows(&db, "SELECT runner, passed, failed, ignored FROM test_run").await;
    assert_eq!((int(&test[0], "passed"), int(&test[0], "failed"), int(&test[0], "ignored")), (Some(7), Some(0), Some(1)));
    let edit = rows(&db, "SELECT path, added, removed, counted_from FROM edit").await;
    assert_eq!((int(&edit[0], "added"), int(&edit[0], "removed"), text(&edit[0], "counted_from")), (Some(2), Some(1), Some("patch".into())));
    let places = rows(&db, "SELECT cwd, at FROM session_place ORDER BY at").await;
    assert_eq!(places.iter().map(|p| text(p, "cwd").unwrap_or_default()).collect::<Vec<_>>(), ["/repo", "/repo/sub"]);

    let second = pass_on(&kernel, &db, &params(&[]), &mut crawler, &mut stop).await.expect("second pass");
    assert_eq!((second.outcome, second.rows_written), ("complete", 0), "{second:?}");
    assert!(lines_read(&second) > 0, "it read the minute before the last pass stopped");
    assert_eq!(every_count(&db).await, counts, "nothing written twice");

    let mut restarted = Crawler::default();
    let third = pass_on(&kernel, &db, &params(&[]), &mut restarted, &mut stop).await.expect("pass after a restart");
    assert_eq!(third.rows_written, 0, "{third:?}");
    assert_eq!(every_count(&db).await, counts, "nothing written twice after a restart");
    assert_eq!(count(&db, "pass").await, 3, "one row per pass");
    assert_eq!(count(&db, "pass_source").await, 6, "and one per source in it");
}

#[tokio::test]
async fn a_reply_read_again_from_the_middle_after_a_restart_counts_its_text_once() {
    let kernel = booted_kernel().await;
    let db = own_db().await;
    let (agent, session) = agent_and_session(&kernel, "claude_code", "s2").await;
    let mut c = Capture { kernel: &kernel, agent, session, key: "s2".to_string(), at: start() };
    c.message(block("msg_9", 0, serde_json::json!({ "type": "text", "text": "Part one." }), serde_json::json!({}))).await;
    c.message(block("msg_9", 1, serde_json::json!({ "type": "text", "text": "Part two!" }), serde_json::json!({}))).await;
    let (_asked, mut stop) = never_stop();
    pass_on(&kernel, &db, &params(&[]), &mut Crawler::default(), &mut stop).await.expect("first pass");

    // The reply goes on after the pass; the module restarts before the next.
    c.message(block("msg_9", 2, serde_json::json!({ "type": "text", "text": "And three." }), serde_json::json!({}))).await;
    c.message(block("msg_9", 3, serde_json::json!({ "type": "thinking", "thinking": "hmm", "signature": "s" }), serde_json::json!({}))).await;
    let after = pass_on(&kernel, &db, &params(&[]), &mut Crawler::default(), &mut stop).await.expect("pass after a restart");
    assert_eq!(after.outcome, "complete");
    let states = rows(&db, "SELECT text_chars FROM reply_state").await;
    assert_eq!(
        sorted(states.iter().map(|s| int(s, "text_chars"))),
        [Some(9), Some(18), Some(28)],
        "each line's text counted once, whichever pass read it"
    );
    assert_eq!(count(&db, "reply").await, 1);
    let thinking = rows(&db, "SELECT position, chars FROM thinking").await;
    assert_eq!((int(&thinking[0], "position"), int(&thinking[0], "chars")), (Some(3), Some(3)));
}

#[tokio::test]
async fn a_pass_that_was_never_recorded_is_read_again_without_writing_twice() {
    let kernel = booted_kernel().await;
    let db = own_db().await;
    let (agent, session) = agent_and_session(&kernel, "claude_code", "s3").await;
    let mut c = Capture { kernel: &kernel, agent, session, key: "s3".to_string(), at: start() };
    claude_session(&mut c).await;
    let (_asked, mut stop) = never_stop();
    pass_on(&kernel, &db, &params(&[]), &mut Crawler::default(), &mut stop).await.expect("pass");
    let counts = every_count(&db).await;
    // As if the process died after writing its rows, before its pass row.
    db.query("DELETE pass_source; DELETE pass;").await.expect("forget the pass").check().expect("forgotten");
    let again = pass_on(&kernel, &db, &params(&[]), &mut Crawler::default(), &mut stop).await.expect("the pass again");
    assert_eq!((lines_read(&again), again.rows_written), (15, 0), "{again:?}");
    assert_eq!(every_count(&db).await, counts);
}

#[tokio::test]
async fn a_pass_asked_to_stop_is_recorded_as_stopped() {
    let kernel = booted_kernel().await;
    let db = own_db().await;
    let (agent, session) = agent_and_session(&kernel, "claude_code", "s4").await;
    let mut c = Capture { kernel: &kernel, agent, session, key: "s4".to_string(), at: start() };
    claude_session(&mut c).await;
    let (asked, mut stop) = never_stop();
    asked.send_replace(true);
    let stopped = pass_on(&kernel, &db, &params(&[]), &mut Crawler::default(), &mut stop).await.expect("pass");
    assert_eq!((stopped.outcome, lines_read(&stopped)), ("stopped", 0));
    assert!(stopped.source("repos").is_none(), "a stopped pass runs no further source");
    let recorded = rows(&db, "SELECT outcome FROM pass").await;
    assert_eq!(text(&recorded[0], "outcome").as_deref(), Some("stopped"));
}

#[tokio::test]
async fn a_gemini_reply_grows_line_by_line_and_older_lines_read_again_change_nothing() {
    let kernel = booted_kernel().await;
    let db = own_db().await;
    let (agent, session) = agent_and_session(&kernel, "gemini_cli", "g1").await;
    let mut c = Capture { kernel: &kernel, agent, session, key: "g1".to_string(), at: start() };
    c.message(serde_json::json!({ "id": "u1", "type": "user", "content": [{ "text": "list the files" }] })).await;
    let emission = |content: &str, output: i64, thoughts: usize, status: &str| {
        serde_json::json!({
            "id": "g-1", "type": "gemini", "model": "gemini-3.1-pro-preview", "content": content,
            "tokens": { "input": 900, "output": output, "cached": 400, "thoughts": 50, "tool": 10, "total": 1000 },
            "thoughts": (0..thoughts).map(|i| serde_json::json!({ "subject": format!("step {i}"), "description": "x", "timestamp": "2026-10-25T09:00:00Z" })).collect::<Vec<_>>(),
            "toolCalls": [{ "id": "ls_1", "name": "run_shell_command", "status": status, "timestamp": "2026-10-25T09:00:00Z",
                "args": { "command": "ls", "dir_path": "/repo" },
                "result": [{ "functionResponse": { "id": "ls_1", "name": "run_shell_command", "response": { "output": "Output: a\nExit Code: 0" } } }] }]
        })
    };
    c.message(serde_json::json!({ "id": "g-1", "type": "gemini", "content": "" })).await;
    c.message(emission("", 10, 1, "executing")).await;
    c.message(emission("Here", 20, 2, "success")).await;
    c.message(emission("Here are the files.", 40, 2, "success")).await;
    let (_asked, mut stop) = never_stop();
    let mut crawler = Crawler::default();
    pass_on(&kernel, &db, &params(&[]), &mut crawler, &mut stop).await.expect("pass");
    assert_eq!(count(&db, "reply").await, 1);
    let reply = rows(&db, "SELECT kernel_message, model FROM reply").await;
    assert_eq!(text(&reply[0], "model").as_deref(), Some("gemini-3.1-pro-preview"));
    let states = rows(&db, "SELECT text_chars, output_tokens, input_tokens FROM reply_state").await;
    assert_eq!(
        sorted(states.iter().map(|s| (int(s, "text_chars"), int(s, "output_tokens"), int(s, "input_tokens")))),
        [(Some(0), Some(60), Some(500)), (Some(4), Some(70), Some(500)), (Some(19), Some(90), Some(500))]
    );
    assert_eq!((count(&db, "thinking").await, count(&db, "tool_call").await, count(&db, "tool_result").await), (2, 1, 1));
    assert_eq!(count(&db, "prompt").await, 1);
    let counts = every_count(&db).await;
    pass_on(&kernel, &db, &params(&[]), &mut Crawler::default(), &mut stop).await.expect("after a restart");
    assert_eq!(every_count(&db).await, counts, "older emissions read again add no state");
}

#[tokio::test]
async fn a_line_of_an_unknown_shape_is_counted_and_the_rest_is_read() {
    let kernel = booted_kernel().await;
    let db = own_db().await;
    let (agent, session) = agent_and_session(&kernel, "claude_code", "s5").await;
    let mut c = Capture { kernel: &kernel, agent, session, key: "s5".to_string(), at: start() };
    c.message(serde_json::json!({ "type": "hologram" })).await;
    c.message(block("msg_5", 0, serde_json::json!({ "type": "text", "text": "ok" }), serde_json::json!({}))).await;
    let (_asked, mut stop) = never_stop();
    let p = pass_on(&kernel, &db, &params(&[]), &mut Crawler::default(), &mut stop).await.expect("pass");
    assert_eq!((lines_read(&p), lines_unread(&p), p.outcome), (2, 1, "complete"));
    assert_eq!(count(&db, "reply").await, 1);
    let recorded = rows(&db, "SELECT items, unread, rows.reply AS replies FROM pass_source WHERE source = 'lines'").await;
    assert_eq!((int(&recorded[0], "unread"), int(&recorded[0], "replies")), (Some(1), Some(1)));
}

#[tokio::test]
async fn spans_without_an_index_keep_their_places_when_read_again() {
    let kernel = booted_kernel().await;
    let db = own_db().await;
    let (agent, session) = agent_and_session(&kernel, "claude_code", "s6").await;
    let mut c = Capture { kernel: &kernel, agent, session, key: "s6".to_string(), at: start() };
    // Before Claude Code wrote `apiBlockIndex`: a span's place is the number
    // of the reply's blocks before it.
    let unindexed = |content: serde_json::Value| {
        let mut line = block("msg_old", 0, content, serde_json::json!({}));
        line.as_object_mut().expect("object").remove("apiBlockIndex");
        line
    };
    c.message(unindexed(serde_json::json!({ "type": "thinking", "thinking": "a", "signature": "s" }))).await;
    c.message(unindexed(serde_json::json!({ "type": "text", "text": "b" }))).await;
    c.message(unindexed(serde_json::json!({ "type": "thinking", "thinking": "cc", "signature": "s" }))).await;
    let (_asked, mut stop) = never_stop();
    let mut crawler = Crawler::default();
    pass_on(&kernel, &db, &params(&[]), &mut crawler, &mut stop).await.expect("pass");
    let places = |rows: Vec<Value>| sorted(rows.iter().map(|t| (int(t, "position"), int(t, "chars"))));
    let first = places(rows(&db, "SELECT position, chars FROM thinking").await);
    assert_eq!(first, [(Some(0), Some(1)), (Some(2), Some(2))]);
    pass_on(&kernel, &db, &params(&[]), &mut crawler, &mut stop).await.expect("the same crawler again");
    pass_on(&kernel, &db, &params(&[]), &mut Crawler::default(), &mut stop).await.expect("after a restart");
    assert_eq!(places(rows(&db, "SELECT position, chars FROM thinking").await), first, "read again, the same places");
}

// The repositories the sessions worked in ------------------------------------

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@x", "-c", "init.defaultBranch=main"])
        .args(args)
        .status()
        .expect("git runs")
        .success();
    assert!(ok, "git {args:?} in {}", dir.display());
}

/// A repository with one commit on `main`.
fn repo(dir: &Path) {
    std::fs::create_dir_all(dir).expect("dir");
    git(dir, &["init", "-q"]);
    git(dir, &["commit", "-q", "--allow-empty", "-m", "first"]);
}

/// A scratch directory, by its real path: git names the real one.
fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("superx-recon-crawl-{}", superx_kernel::types::Uuid::new_v7()));
    std::fs::create_dir_all(&dir).expect("scratch");
    std::fs::canonicalize(&dir).expect("real path")
}

fn strings(paths: &[&Path]) -> Value {
    Value::Array(paths.iter().map(|p| Value::String(p.to_string_lossy().into_owned())).collect::<Vec<_>>().into())
}

/// The operator, typing in `dir`.
fn worked_in(dir: &Path) -> serde_json::Value {
    serde_json::json!({
        "type": "user", "cwd": dir, "gitBranch": "main", "origin": { "kind": "human" },
        "message": { "role": "user", "content": "work here" }
    })
}

/// The newest row for a directory, as `(checkout's root or "none", exact,
/// source)`.
async fn directory(db: &superx_kernel::Db, path: &Path) -> (String, bool, String) {
    let found = rows(
        db,
        &format!(
            "SELECT checkout.toplevel ?? 'none' AS top, exact, source, valid_from FROM directory \
             WHERE path = '{}' ORDER BY valid_from DESC",
            path.display()
        ),
    )
    .await;
    let newest = found.first().expect("a row for the directory");
    let exact = matches!(newest, Value::Object(o) if matches!(o.get("exact"), Some(Value::Bool(true))));
    (text(newest, "top").expect("top"), exact, text(newest, "source").expect("source"))
}

/// A checkout's newest state, as `(present, branch)`.
async fn state(db: &superx_kernel::Db, toplevel: &Path) -> (bool, Option<String>) {
    let found = rows(
        db,
        &format!(
            "SELECT present, branch, valid_from FROM checkout_state WHERE checkout.toplevel = '{}' ORDER BY valid_from DESC",
            toplevel.display()
        ),
    )
    .await;
    let newest = found.first().expect("a state for the checkout");
    let present = matches!(newest, Value::Object(o) if matches!(o.get("present"), Some(Value::Bool(true))));
    (present, text(newest, "branch"))
}

/// What the `repos` source counted in a pass, by name.
fn repos(p: &Pass, name: &str) -> i64 {
    p.source("repos").and_then(|s| s.counts.get(name).copied()).unwrap_or(0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_pass_reads_the_lines_and_follows_them_to_their_repositories() {
    let kernel = booted_kernel().await;
    let db = own_db().await;
    let (agent, session) = agent_and_session(&kernel, "claude_code", "r1").await;
    let mut c = Capture { kernel: &kernel, agent, session, key: "r1".to_string(), at: Utc::now() - TimeDelta::hours(2) };
    let root = scratch();
    let (a, wt, b, inc, d) = (root.join("a"), root.join("a-wt"), root.join("b"), root.join("c"), root.join("d"));
    repo(&a);
    std::fs::create_dir_all(a.join("src")).expect("src");
    git(&a, &["worktree", "add", "-q", "-b", "feat", &wt.to_string_lossy()]);
    repo(&b);
    repo(&inc);
    let plain = root.join("plain");
    std::fs::create_dir_all(&plain).expect("plain");
    let vanished = a.join("gone").join("deep");
    for dir in [a.join("src"), wt.clone(), b.clone(), plain.clone(), vanished.clone()] {
        c.message(worked_in(&dir)).await;
    }
    let p = params(&[(INCLUDE_PARAM, strings(&[&inc]))]);
    let (_asked, mut stop) = never_stop();
    let mut crawler = Crawler::default();

    // One pass: the lines, then the repositories they lead to.
    let first = pass_on(&kernel, &db, &p, &mut crawler, &mut stop).await.expect("first pass");
    assert_eq!(first.outcome, "complete", "{first:?}");
    assert_eq!(lines_read(&first), 5);
    assert_eq!(
        ["directories_read", "directories_new", "repositories_new", "repositories_known", "checkouts_new", "checkouts_known"]
            .map(|n| repos(&first, n)),
        [6, 6, 3, 3, 4, 4],
        "five directories the session worked in, one root named: {first:?}"
    );
    let names: Vec<String> = sorted(rows(&db, "SELECT name FROM repository").await.iter().filter_map(|r| text(r, "name")));
    assert_eq!(names, ["a", "b", "c"], "a worktree is its repository's");
    assert_eq!(directory(&db, &a.join("src")).await, (a.display().to_string(), true, "session".to_string()));
    assert_eq!(directory(&db, &wt).await, (wt.display().to_string(), true, "session".to_string()));
    assert_eq!(directory(&db, &plain).await, ("none".to_string(), true, "session".to_string()), "no repository holds it");
    assert_eq!(
        directory(&db, &vanished).await,
        (a.display().to_string(), false, "session".to_string()),
        "gone: its repository is known through its nearest folder, not exactly"
    );
    assert_eq!(directory(&db, &inc).await, (inc.display().to_string(), true, "include".to_string()));
    assert_eq!(state(&db, &a).await, (true, Some("main".to_string())));
    assert_eq!(state(&db, &wt).await, (true, Some("feat".to_string())));
    let recorded = rows(&db, "SELECT source, calls FROM pass_source WHERE source = 'repos'").await;
    assert!(int(&recorded[0], "calls").is_some_and(|n| n > 0), "every git call is counted: {recorded:?}");

    // Nothing new: the next pass follows nothing and writes nothing.
    let second = pass_on(&kernel, &db, &p, &mut crawler, &mut stop).await.expect("second pass");
    assert_eq!((second.rows_written, repos(&second, "directories_read")), (0, 0), "{second:?}");

    // The logs report a change first. Branches change in `a`, where the
    // session goes on working, and in the named root, where no session
    // is: the next pass reads `a`'s checkout because the lines it read were
    // written there, and leaves the other to the next full pass.
    git(&a, &["switch", "-q", "-c", "topic"]);
    git(&inc, &["switch", "-q", "-c", "elsewhere"]);
    c.at = Utc::now();
    c.message(worked_in(&a.join("src"))).await;
    let followed = pass_on(&kernel, &db, &p, &mut crawler, &mut stop).await.expect("a pass after the session worked");
    assert_eq!(state(&db, &a).await, (true, Some("topic".to_string())), "{followed:?}");
    assert_eq!(state(&db, &inc).await, (true, Some("main".to_string())), "no session worked there yet");
    assert!(repos(&followed, "checkouts_read") >= 1, "{followed:?}");

    // A repository's folder is gone. The next full pass (here, after a
    // restart) finds its checkout gone, and the repository stale.
    std::fs::remove_dir_all(&b).expect("remove b");
    let third = pass_on(&kernel, &db, &p, &mut Crawler::default(), &mut stop).await.expect("third pass");
    assert_eq!(
        ["checkouts_gone", "repositories_stale", "directories_moved"].map(|n| repos(&third, n)),
        [1, 1, 1],
        "{third:?}"
    );
    assert_eq!(state(&db, &b).await, (false, None));
    assert_eq!(directory(&db, &b).await, ("none".to_string(), false, "session".to_string()));
    assert_eq!(state(&db, &inc).await, (true, Some("elsewhere".to_string())), "a full pass reads every checkout");

    // It is back: the same repository and checkout, present again.
    repo(&b);
    let fourth = pass_on(&kernel, &db, &p, &mut Crawler::default(), &mut stop).await.expect("fourth pass");
    assert_eq!(["checkouts_back", "repositories_new", "checkouts_new"].map(|n| repos(&fourth, n)), [1, 0, 0], "{fourth:?}");
    assert_eq!(state(&db, &b).await, (true, Some("main".to_string())));

    // The session moves to a new repository: the same pass that reads the
    // line follows it there, without waiting for a full pass.
    repo(&d);
    c.at = Utc::now();
    c.message(worked_in(&d)).await;
    let fifth = pass_on(&kernel, &db, &p, &mut crawler, &mut stop).await.expect("fifth pass");
    assert_eq!(
        ["directories_read", "repositories_new", "checkouts_new", "repositories_known"].map(|n| repos(&fifth, n)),
        [1, 1, 1, 4],
        "{fifth:?}"
    );
    assert_eq!(state(&db, &d).await, (true, Some("main".to_string())));
    assert_eq!(count(&db, "pass").await, 6, "every pass is recorded");
    std::fs::remove_dir_all(&root).expect("clean up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_excluded_repository_is_never_listed() {
    let kernel = booted_kernel().await;
    let db = own_db().await;
    let (agent, session) = agent_and_session(&kernel, "claude_code", "r2").await;
    let mut c = Capture { kernel: &kernel, agent, session, key: "r2".to_string(), at: Utc::now() - TimeDelta::hours(1) };
    let root = scratch();
    let (a, b) = (root.join("a"), root.join("b"));
    repo(&a);
    repo(&b);
    c.message(worked_in(&a)).await;
    c.message(worked_in(&b)).await;
    let p = params(&[(EXCLUDE_PARAM, Value::Array(vec![Value::String("b".to_string())].into()))]);
    let (_asked, mut stop) = never_stop();
    let s = pass_on(&kernel, &db, &p, &mut Crawler::default(), &mut stop).await.expect("pass");
    assert_eq!(["excluded", "repositories_known", "checkouts_known"].map(|n| repos(&s, n)), [1, 1, 1], "{s:?}");
    let names: Vec<String> = rows(&db, "SELECT name FROM repository").await.iter().filter_map(|r| text(r, "name")).collect();
    assert_eq!(names, ["a"]);
    std::fs::remove_dir_all(&root).expect("clean up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_repositories_pause_while_their_crawl_is_0() {
    let kernel = booted_kernel().await;
    let db = own_db().await;
    let (agent, session) = agent_and_session(&kernel, "claude_code", "r3").await;
    let mut c = Capture { kernel: &kernel, agent, session, key: "r3".to_string(), at: Utc::now() - TimeDelta::hours(1) };
    let root = scratch();
    repo(&root.join("a"));
    c.message(worked_in(&root.join("a"))).await;
    let p = params(&[(superx_mod_recon::params::CRAWL_SECS_PARAM, Value::Number(superx_kernel::types::Number::Int(0)))]);
    let (_asked, mut stop) = never_stop();
    let s = pass_on(&kernel, &db, &p, &mut Crawler::default(), &mut stop).await.expect("pass");
    let repos_run = s.source("repos").expect("the source is recorded");
    assert_eq!((repos_run.items, repos_run.calls), (0, 0), "{s:?}");
    assert!(repos_run.note.as_deref().is_some_and(|n| n.contains("paused")), "{s:?}");
    assert_eq!(count(&db, "repository").await, 0);
    assert_eq!(lines_read(&s), 1, "the lines are read all the same");
    std::fs::remove_dir_all(&root).expect("clean up");
}

/// How long the test gives the crawler to start waiting, and then to end.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(200); // skill-allow: §9-duration — test fixture, not a policy
const LIMIT: std::time::Duration = std::time::Duration::from_secs(5); // skill-allow: §9-duration — test fixture, not a policy

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_crawler_ends_when_asked_even_while_it_waits() {
    // A test handle has no database of its own behind it, so the crawler
    // finds nothing to pass over and waits a minute before it looks again.
    let kernel = booted_kernel().await;
    let (asked, watched) = watch::channel(false);
    let crawler = tokio::spawn(superx_mod_recon::crawler::run(kernel, Stop::new(watched)));
    tokio::time::sleep(SETTLE).await;
    asked.send_replace(true);
    let ended = tokio::time::timeout(LIMIT, crawler).await;
    assert!(matches!(ended, Ok(Ok(()))), "it ended when asked: {ended:?}");
}

// Every line, in depth (#537) ----------------------------------------------

/// How the kernel kept a line: its text as written (`raw_text`), its parsed
/// copy, or both, as it has over time (#529, #542, #546).
#[derive(Debug, Clone, Copy)]
enum Kept {
    Both,
    Text,
    Parsed,
}

const KEPT: [Kept; 3] = [Kept::Both, Kept::Text, Kept::Parsed];

fn uuid_of(id: &RecordId) -> superx_kernel::types::Uuid {
    match &id.key {
        superx_kernel::types::RecordIdKey::Uuid(u) => *u,
        other => unreachable!("a line id is a UUID, not {other:?}"),
    }
}

impl Capture<'_> {
    /// The next line's time, and the line with it.
    fn stamped(&mut self, json: serde_json::Value) -> (serde_json::Value, DateTime<Utc>) {
        let at = self.tick();
        let mut json = json;
        json["timestamp"] = serde_json::Value::String(at.to_rfc3339());
        json["sessionId"] = serde_json::Value::String(self.key.clone());
        (json, at)
    }

    /// A conversation line kept as `kept` says; its id.
    async fn kept_message(&mut self, json: serde_json::Value, kept: Kept) -> superx_kernel::types::Uuid {
        let (json, at) = self.stamped(json);
        self.message_row(&json, &json, kept, at).await
    }

    /// A conversation line kept as both, its parsed copy `parsed` and its
    /// text `json`; its id.
    async fn differing_message(&mut self, json: serde_json::Value, parsed: serde_json::Value) -> superx_kernel::types::Uuid {
        let (json, at) = self.stamped(json);
        let mut parsed = parsed;
        parsed["timestamp"] = json["timestamp"].clone();
        parsed["sessionId"] = json["sessionId"].clone();
        self.message_row(&json, &parsed, Kept::Both, at).await
    }

    async fn message_row(
        &mut self,
        text: &serde_json::Value,
        parsed: &serde_json::Value,
        kept: Kept,
        at: DateTime<Utc>,
    ) -> superx_kernel::types::Uuid {
        let id = id_at("message", at);
        let raw = match kept {
            Kept::Text => Value::None,
            Kept::Both | Kept::Parsed => Value::Object(line_to_object(parsed)),
        };
        let raw_text = match kept {
            Kept::Parsed => Value::None,
            Kept::Both | Kept::Text => Value::String(serde_json::to_string(text).expect("json")),
        };
        self.kernel
            .db()
            .query(
                "CREATE $id CONTENT { session: $sess, agent: $agent, role: 'x', content: '', raw: $raw, \
                 raw_text: $raw_text, emitted_at: $at, valid_from: $at }",
            )
            .bind(("id", id.clone()))
            .bind(("sess", self.session.clone()))
            .bind(("agent", self.agent.clone()))
            .bind(("raw", raw))
            .bind(("raw_text", raw_text))
            .bind(("at", at))
            .await
            .expect("line")
            .check()
            .expect("line taken");
        uuid_of(&id)
    }

    /// A line kept as a `transcript_event`, as `kept` says, of the kind the
    /// kernel gives it: its type; `kind` where it has none.
    async fn kept_event(&mut self, json: serde_json::Value, kept: Kept, kind: Option<&str>) -> superx_kernel::types::Uuid {
        let (json, at) = self.stamped(json);
        self.event_row(&json, &json, kept, kind, at).await
    }

    async fn event_row(
        &mut self,
        text: &serde_json::Value,
        parsed: &serde_json::Value,
        kept: Kept,
        kind: Option<&str>,
        at: DateTime<Utc>,
    ) -> superx_kernel::types::Uuid {
        let kind = kind.map_or_else(|| parsed["type"].as_str().unwrap_or_default().to_string(), str::to_string);
        let mut payload = serde_json::json!({ "kind": kind, "session": self.key, "file": "/t.jsonl" });
        if !matches!(kept, Kept::Text) {
            payload["line"] = parsed.clone();
        }
        if !matches!(kept, Kept::Parsed) {
            payload["raw_text"] = serde_json::Value::String(serde_json::to_string(text).expect("json"));
        }
        self.telemetry_at("transcript_event", payload, true, at).await
    }

    /// A telemetry row of `lifecycle`, its payload as the adapter writes
    /// it, for this agent unless `agent` is false; its id.
    async fn telemetry(&mut self, lifecycle: &str, payload: serde_json::Value, agent: bool) -> superx_kernel::types::Uuid {
        let at = self.tick();
        self.telemetry_at(lifecycle, payload, agent, at).await
    }

    async fn telemetry_at(
        &mut self,
        lifecycle: &str,
        payload: serde_json::Value,
        agent: bool,
        at: DateTime<Utc>,
    ) -> superx_kernel::types::Uuid {
        let id = id_at("telemetry_stream", at);
        let sql = if agent {
            "CREATE $id CONTENT { lifecycle_event: $lifecycle, payload: $payload, agent: $agent, valid_from: $at }"
        } else {
            "CREATE $id CONTENT { lifecycle_event: $lifecycle, payload: $payload, valid_from: $at }"
        };
        self.kernel
            .db()
            .query(sql)
            .bind(("id", id.clone()))
            .bind(("lifecycle", lifecycle.to_string()))
            .bind(("payload", line_value_from_json(&payload)))
            .bind(("agent", self.agent.clone()))
            .bind(("at", at))
            .await
            .expect("telemetry")
            .check()
            .expect("telemetry taken");
        uuid_of(&id)
    }
}

async fn rows_where(db: &superx_kernel::Db, sql: &str, name: &str, value: Value) -> Vec<Value> {
    db.query(sql).bind((name.to_string(), value)).await.expect("query").take(0).expect("rows")
}

fn lines_count(p: &Pass, name: &str) -> i64 {
    p.source("lines").and_then(|s| s.counts.get(name).copied()).unwrap_or(0)
}

/// Every row of every table a session of the module's database holds, or
/// that hangs off one it holds: what the session's lines fed.
async fn fed(db: &superx_kernel::Db, kernel_session: &RecordId) -> Vec<Value> {
    let mut res = db
        .query(
            "LET $s = (SELECT VALUE id FROM session WHERE kernel_session = $k)[0];
             LET $r = (SELECT VALUE id FROM reply WHERE session = $s);
             LET $c = (SELECT VALUE id FROM tool_call WHERE reply INSIDE $r);
             SELECT * FROM session WHERE id = $s;
             SELECT * FROM session_place WHERE session = $s;
             SELECT * FROM prompt WHERE session = $s;
             SELECT * FROM trigger WHERE session = $s;
             SELECT * FROM turn WHERE session = $s;
             SELECT * FROM compaction WHERE session = $s;
             SELECT * FROM away WHERE session = $s;
             SELECT * FROM reply WHERE session = $s;
             SELECT * FROM reply_state WHERE reply INSIDE $r;
             SELECT * FROM thinking WHERE reply INSIDE $r;
             SELECT * FROM tool_call WHERE reply INSIDE $r;
             SELECT * FROM tool_result WHERE tool_call INSIDE $c;
             SELECT * FROM edit WHERE tool_call INSIDE $c;
             SELECT * FROM git_event WHERE tool_call INSIDE $c;
             SELECT * FROM test_run WHERE tool_call INSIDE $c;
             SELECT * FROM build_run WHERE tool_call INSIDE $c;",
        )
        .bind(("k", uuid_of(kernel_session)))
        .await
        .expect("query");
    let mut out = Vec::new();
    for i in 3..=18 {
        let rows: Vec<Value> = res.take(i).expect("rows");
        out.extend(rows);
    }
    out
}

/// A value of a line, found among the values a row holds: text as text or,
/// written as a time, as that time; a number, a flag; an object whole.
fn holds(v: &Value, want: &serde_json::Value) -> bool {
    match (v, want) {
        (Value::String(s), serde_json::Value::String(w)) => s == w,
        (Value::Datetime(d), serde_json::Value::String(w)) => {
            DateTime::parse_from_rfc3339(w).is_ok_and(|t| t.to_utc() == DateTime::<Utc>::from(*d))
        }
        (Value::Number(n), serde_json::Value::Number(w)) => w.as_i64().is_some() && n.to_int() == w.as_i64(),
        (Value::Bool(b), serde_json::Value::Bool(w)) => b == w,
        (Value::Object(_), serde_json::Value::Object(_)) if *v == line_value_from_json(want) => true,
        (Value::Array(items), _) => items.iter().any(|x| holds(x, want)),
        (Value::Object(o), _) => o.values().any(|x| holds(x, want)),
        _ => false,
    }
}

/// What a session's replies, their states and its turns hold, but the
/// links and times every line has of its own.
async fn shape(db: &superx_kernel::Db, kernel_session: &RecordId) -> [Vec<Vec<Option<String>>>; 3] {
    let k = Value::Uuid(uuid_of(kernel_session));
    let session = "(SELECT VALUE id FROM session WHERE kernel_session = $k)[0]";
    let replies = rows_where(db, &format!("SELECT * FROM reply WHERE session = {session}"), "k", k.clone()).await;
    let states =
        rows_where(db, &format!("SELECT * FROM reply_state WHERE reply.session = {session}"), "k", k.clone()).await;
    let turns = rows_where(db, &format!("SELECT * FROM turn WHERE session = {session}"), "k", k).await;
    [
        kept_rows(&replies, &["agent_id", "model", "effort", "request_id", "stop_reason", "subagent"]),
        kept_rows(&states, &["input_tokens", "cache_read_tokens", "output_tokens", "thinking_tokens", "text_chars", "usage"]),
        kept_rows(&turns, &["ms", "lines"]),
    ]
}

fn kept_rows(rows: &[Value], fields: &[&str]) -> Vec<Vec<Option<String>>> {
    sorted(rows.iter().map(|r| fields.iter().map(|f| match r {
        Value::Object(o) => o.get(*f).map(|v| format!("{v:?}")),
        _ => None,
    }).collect::<Vec<_>>()))
}

#[tokio::test]
async fn a_pass_reads_every_message_row_and_every_transcript_event_transcript_raw_and_desktop_event_row_in_id_order_a_side_files_content_included_and_no_other_telemetry_row() {
    let kernel = booted_kernel().await;
    let db = own_db().await;
    let (agent, session) = agent_and_session(&kernel, "claude_code", "e1").await;
    let (desktop, desktop_session) = agent_and_session(&kernel, "claude_desktop", "app").await;
    let mut c = Capture { kernel: &kernel, agent, session, key: "e1".to_string(), at: start() };
    let human = serde_json::json!({ "kind": "human" });
    let first = c
        .kept_message(serde_json::json!({ "type": "user", "cwd": "/a", "origin": human, "message": { "role": "user", "content": "start" } }), Kept::Both)
        .await;
    c.noise().await;
    // A line that feeds no table, between two conversation lines: read in
    // its place, it moves the session to /b before the next line moves it on.
    c.kept_event(serde_json::json!({ "type": "system", "subtype": "local_command", "cwd": "/b" }), Kept::Both, None).await;
    c.kept_message(serde_json::json!({ "type": "user", "cwd": "/c", "origin": human, "message": { "role": "user", "content": "then" } }), Kept::Both)
        .await;
    let side = c
        .telemetry(
            "transcript_event",
            serde_json::json!({ "kind": "file", "session": "e1", "file": "/p/e1/tool-results/out.txt",
                "line": { "type": "file", "path": "e1/tool-results/out.txt", "bytes": 5, "content": "hello" } }),
            true,
        )
        .await;
    c.telemetry("transcript_raw", serde_json::json!({ "file": "/p/e1.jsonl", "session": "e1", "snippet": "{cut", "text": "{cut", "raw_text": "{cut\n" }), true)
        .await;
    c.telemetry("tool_call", serde_json::json!({ "tool": "Bash", "session": "e1" }), true).await;
    let mut d = Capture { kernel: &kernel, agent: desktop, session: desktop_session, key: "app".to_string(), at: c.at };
    let last = d.telemetry("desktop_event", serde_json::json!({ "line": "2026-10-25 09:00:01 [info] Starting app", "file": "main.log" }), true).await;

    let (_asked, mut stop) = never_stop();
    let p = pass_on(&kernel, &db, &params(&[]), &mut Crawler::default(), &mut stop).await.expect("pass");
    let lines = p.source("lines").expect("lines");
    assert_eq!((lines.items, lines_count(&p, "lines_held"), lines.unread), (6, 6, 1), "two messages, four lines of telemetry; the cut line unread: {p:?}");
    assert_eq!((lines.from_id, lines.to_id), (Some(first), Some(last)));
    let places = rows(&db, "SELECT cwd, at FROM session_place ORDER BY at").await;
    assert_eq!(places.iter().filter_map(|r| text(r, "cwd")).collect::<Vec<_>>(), ["/a", "/b", "/c"], "read in the order they were captured");
    let content = rows(&db, "SELECT first_line FROM line_field WHERE agent = 'claude_code' AND kind = 'file' AND path = 'content'").await;
    assert_eq!(content.len(), 1, "the side file's content was read");
    assert!(matches!(&content[0], Value::Object(o) if o.get("first_line") == Some(&Value::Uuid(side))));
}

#[tokio::test]
async fn a_line_kept_as_its_raw_text_alone_as_its_parsed_copy_alone_and_as_both_is_read_into_the_same_rows_and_where_the_two_differ_its_rows_hold_the_raw_texts_values() {
    let kernel = booted_kernel().await;
    let db = own_db().await;
    let reply = |model: &str| {
        let mut line = block("msg_k", 0, serde_json::json!({ "type": "text", "text": "kept" }), serde_json::json!({}));
        line["message"]["model"] = serde_json::Value::String(model.to_string());
        line
    };
    let turn = |ms: i64| serde_json::json!({ "type": "system", "subtype": "turn_duration", "durationMs": ms, "messageCount": 4 });
    let record = |id: &str, model: &str, content: &str| serde_json::json!({ "id": id, "type": "gemini", "model": model, "content": content,
        "tokens": { "input": 10, "output": 5, "cached": 2, "thoughts": 1, "tool": 0, "total": 16 } });
    let mut sessions = Vec::new();
    for (i, kept) in KEPT.iter().enumerate() {
        let (agent, session) = agent_and_session(&kernel, "claude_code", &format!("k{i}")).await;
        let mut c = Capture { kernel: &kernel, agent, session: session.clone(), key: format!("k{i}"), at: start() + TimeDelta::minutes(i64::try_from(i).expect("small") * 5) };
        c.kept_message(reply("claude-opus-5-5"), *kept).await;
        c.kept_event(turn(7_000), *kept, None).await;
        let (gagent, gsession) = agent_and_session(&kernel, "gemini_cli", &format!("g{i}")).await;
        let mut g = Capture { kernel: &kernel, agent: gagent, session: gsession.clone(), key: format!("g{i}"), at: c.at };
        g.kept_message(record("g-a", "gemini-3.1-pro-preview", "Hi"), *kept).await;
        g.kept_event(record("g-b", "gemini-3.1-pro-preview", "Yo"), *kept, Some("superseded")).await;
        sessions.push((session, gsession));
    }
    let (agent, session) = agent_and_session(&kernel, "claude_code", "k-differ").await;
    let mut c = Capture { kernel: &kernel, agent, session: session.clone(), key: "k-differ".to_string(), at: start() + TimeDelta::minutes(30) };
    c.differing_message(reply("claude-from-text"), reply("claude-from-copy")).await;
    let (json, at) = c.stamped(turn(9_000));
    c.event_row(&json, &turn(1), Kept::Both, None, at).await;
    let (gagent, gsession) = agent_and_session(&kernel, "gemini_cli", "g-differ").await;
    let mut g = Capture { kernel: &kernel, agent: gagent, session: gsession.clone(), key: "g-differ".to_string(), at: c.at };
    g.differing_message(record("g-a", "gemini-from-text", "Hi"), record("g-a", "gemini-from-copy", "Hi")).await;

    let (_asked, mut stop) = never_stop();
    let p = pass_on(&kernel, &db, &params(&[]), &mut Crawler::default(), &mut stop).await.expect("pass");
    assert_eq!((p.outcome, lines_count(&p, "lines_held"), p.source("lines").map(|s| s.unread)), ("complete", 15, Some(0)), "{p:?}");
    let mut claude = Vec::new();
    let mut gemini = Vec::new();
    for (session, gsession) in &sessions {
        claude.push(shape(&db, session).await);
        gemini.push(shape(&db, gsession).await);
    }
    assert!(!claude[0][0].is_empty() && !claude[0][2].is_empty() && gemini[0][0].len() == 2, "{claude:?} {gemini:?}");
    assert!(claude.iter().all(|s| *s == claude[0]), "the same rows from each shape: {claude:#?}");
    assert!(gemini.iter().all(|s| *s == gemini[0]), "the same rows from each shape: {gemini:#?}");
    let differ = fed(&db, &session).await;
    let models: Vec<String> = differ.iter().filter_map(|r| text(r, "model")).collect();
    assert_eq!(models, ["claude-from-text"], "the text wins over its copy");
    assert_eq!(differ.iter().filter_map(|r| int(r, "ms")).collect::<Vec<_>>(), [9_000]);
    let gemini_models: Vec<String> = fed(&db, &gsession).await.iter().filter_map(|r| text(r, "model")).collect();
    assert_eq!(gemini_models, ["gemini-from-text"]);
}

#[tokio::test]
async fn a_gemini_superseded_write_is_read_as_the_record_it_holds_in_its_place_among_the_lines() {
    let kernel = booted_kernel().await;
    let db = own_db().await;
    let (agent, session) = agent_and_session(&kernel, "gemini_cli", "gs").await;
    let mut c = Capture { kernel: &kernel, agent, session, key: "gs".to_string(), at: start() };
    let write = |content: &str, output: i64| serde_json::json!({ "id": "g-1", "type": "gemini", "model": "gemini-3.1-pro-preview", "content": content,
        "tokens": { "input": 100, "output": output, "cached": 0, "thoughts": 0, "tool": 0, "total": 100 + output } });
    c.kept_message(serde_json::json!({ "id": "u-1", "type": "user", "content": [{ "text": "list the files" }] }), Kept::Both).await;
    let earlier = c.kept_event(write("Here", 20), Kept::Both, Some("superseded")).await;
    c.kept_message(write("Here are the files.", 40), Kept::Both).await;
    let (_asked, mut stop) = never_stop();
    let p = pass_on(&kernel, &db, &params(&[]), &mut Crawler::default(), &mut stop).await.expect("pass");
    assert_eq!((p.outcome, p.source("lines").map(|s| (s.items, s.unread))), ("complete", Some((3, 0))));
    let reply = rows(&db, "SELECT kernel_message FROM reply").await;
    assert_eq!(reply.len(), 1);
    assert!(matches!(&reply[0], Value::Object(o) if o.get("kernel_message") == Some(&Value::Uuid(earlier))), "the reply begins at its earlier write: {reply:?}");
    let states = rows(&db, "SELECT text_chars, output_tokens, valid_from FROM reply_state ORDER BY valid_from").await;
    assert_eq!(
        states.iter().map(|s| (int(s, "text_chars"), int(s, "output_tokens"))).collect::<Vec<_>>(),
        [(Some(4), Some(20)), (Some(19), Some(40))],
        "the earlier write first, then the record as it was last written"
    );
}

#[tokio::test]
async fn a_line_of_each_kind_that_feeds_no_table_is_counted_read_and_one_of_no_known_kind_not_json_or_of_no_agent_unread() {
    let kernel = booted_kernel().await;
    let db = own_db().await;
    let (agent, session) = agent_and_session(&kernel, "claude_code", "n1").await;
    let mut c = Capture { kernel: &kernel, agent, session, key: "n1".to_string(), at: start() };
    let mut read = 0;
    for kind in superx_mod_recon::crawler::lines::formats::claude_code::NO_TABLE {
        if kind == superx_mod_recon::crawler::lines::formats::claude_code::SIDE_FILE {
            c.telemetry("transcript_event", serde_json::json!({ "kind": "file", "session": "n1", "file": "/p/n1/x.md",
                "line": { "type": "file", "path": "n1/x.md", "bytes": 1, "content": "x" } }), true).await;
        } else {
            c.kept_event(serde_json::json!({ "type": kind }), Kept::Both, None).await;
        }
        read += 1;
    }
    c.kept_event(serde_json::json!({ "type": "system", "subtype": "local_command", "content": "ls" }), Kept::Both, None).await;
    c.kept_event(serde_json::json!({ "type": "attachment", "attachment": { "type": "todo_reminder" } }), Kept::Both, None).await;
    read += 2;
    let (gagent, gsession) = agent_and_session(&kernel, "gemini_cli", "n2").await;
    let mut g = Capture { kernel: &kernel, agent: gagent, session: gsession, key: "n2".to_string(), at: c.at };
    g.kept_event(serde_json::json!({ "sessionId": "n2", "projectHash": "h", "startTime": "2026-10-25T09:00:00Z" }), Kept::Both, Some("header")).await;
    g.kept_event(serde_json::json!({ "$set": { "lastUpdated": "2026-10-25T09:00:00Z" } }), Kept::Both, Some("$set")).await;
    for kind in ["info", "error", "warning"] {
        g.kept_event(serde_json::json!({ "type": kind, "content": "a notice" }), Kept::Both, None).await;
    }
    read += 5;
    let (dagent, dsession) = agent_and_session(&kernel, "claude_desktop", "n3").await;
    let mut d = Capture { kernel: &kernel, agent: dagent, session: dsession, key: "n3".to_string(), at: g.at };
    d.telemetry("desktop_event", serde_json::json!({ "line": "2026-10-25 09:00:00 [info] Update check", "file": "main.log" }), true).await;
    read += 1;
    // Unread: a kind no format knows, a line that is not JSON, a line the
    // kernel names no agent for.
    d.at = c.at.max(g.at).max(d.at);
    c.at = d.at;
    c.kept_event(serde_json::json!({ "type": "hologram" }), Kept::Both, None).await;
    c.telemetry("transcript_raw", serde_json::json!({ "file": "/p/n1.jsonl", "session": "n1", "snippet": "{x", "text": "{x", "raw_text": "{x" }), true).await;
    c.telemetry("transcript_event", serde_json::json!({ "kind": "mode", "session": "n1", "line": { "type": "mode" }, "raw_text": "{\"type\":\"mode\"}" }), false).await;

    let (_asked, mut stop) = never_stop();
    let p = pass_on(&kernel, &db, &params(&[]), &mut Crawler::default(), &mut stop).await.expect("pass");
    let lines = p.source("lines").expect("lines");
    assert_eq!((lines.items, lines.unread), (read + 3, 3), "{p:?}");
    assert_eq!(lines_count(&p, "lines_unused"), read + 3, "none of them fed a table");
    for table in ["prompt", "trigger", "reply", "turn", "compaction", "away", "tool_call"] {
        assert_eq!(count(&db, table).await, 0, "{table}");
    }
}

#[tokio::test]
async fn a_field_is_a_line_field_row_the_first_time_its_agent_kind_and_path_are_seen_and_never_again_while_stored_stands_and_again_when_stored_changes() {
    let kernel = booted_kernel().await;
    let db = own_db().await;
    let (agent, session) = agent_and_session(&kernel, "claude_code", "f1").await;
    let mut c = Capture { kernel: &kernel, agent, session, key: "f1".to_string(), at: start() };
    let line = |text: &str| serde_json::json!({ "type": "user", "cwd": "/w", "origin": { "kind": "human" }, "message": { "role": "user", "content": text } });
    let first = c.kept_message(line("one"), Kept::Both).await;
    let (_asked, mut stop) = never_stop();
    let mut crawler = Crawler::default();
    let p1 = pass_on(&kernel, &db, &params(&[]), &mut crawler, &mut stop).await.expect("first pass");
    let field = |path: &str| {
        format!(
            "SELECT stored, first_line, valid_from FROM line_field \
             WHERE agent = 'claude_code' AND kind = 'user' AND path = '{path}' ORDER BY valid_from"
        )
    };
    let stored = |rows: &[Value]| rows.iter().map(|r| matches!(r, Value::Object(o) if o.get("stored") == Some(&Value::Bool(true)))).collect::<Vec<_>>();
    let content = rows(&db, &field("message.content")).await;
    assert_eq!(stored(&content), [true], "a column holds the prompt's text");
    assert!(matches!(&content[0], Value::Object(o) if o.get("first_line") == Some(&Value::Uuid(first))));
    assert_eq!(stored(&rows(&db, &field("message.role")).await), [false], "no column holds the role");
    assert_eq!(lines_count(&p1, "fields_new"), count(&db, "line_field").await);

    c.at += TimeDelta::minutes(5);
    c.kept_message(line("two"), Kept::Both).await;
    let p2 = pass_on(&kernel, &db, &params(&[]), &mut crawler, &mut stop).await.expect("second pass");
    assert_eq!(lines_count(&p2, "fields_new"), 0, "{p2:?}");
    assert_eq!(rows(&db, &field("message.content")).await.len(), 1, "never again while stored stands");

    // An earlier build said a column held the role: the next pass after a
    // restart finds that changed, and says so in a row more.
    db.query(
        "CREATE line_field CONTENT { agent: 'claude_code', kind: 'user', path: 'message.role', \
         first_at: d'2026-10-25T09:00:00Z', first_line: $first, stored: true, valid_from: time::now() }",
    )
    .bind(("first", first))
    .await
    .expect("query")
    .check()
    .expect("an earlier build's row");
    c.at += TimeDelta::minutes(5);
    c.kept_message(line("three"), Kept::Both).await;
    let p3 = pass_on(&kernel, &db, &params(&[]), &mut Crawler::default(), &mut stop).await.expect("after a restart");
    let role = rows(&db, &field("message.role")).await;
    assert_eq!(stored(&role), [false, true, false], "{p3:?}");
    assert!(matches!(&role[2], Value::Object(o) if o.get("first_line") == Some(&Value::Uuid(first))), "where it was first seen");
    assert_eq!(rows(&db, &field("message.content")).await.len(), 1);
}

/// A path a format declares stored, the value a fixture line gives it, and
/// the session that line fed.
struct Proof {
    agent: &'static str,
    kind: &'static str,
    session: RecordId,
    paths: Vec<(&'static str, serde_json::Value)>,
}

/// One fixture session of its own, its lines captured from `at`.
async fn fixture<'a>(kernel: &'a Kernel, agent: &str, key: &str, at: DateTime<Utc>) -> Capture<'a> {
    let (agent, session) = agent_and_session(kernel, agent, key).await;
    Capture { kernel, agent, session, key: key.to_string(), at }
}

fn time_of(at: DateTime<Utc>) -> serde_json::Value {
    serde_json::Value::String(at.to_rfc3339())
}

/// Where a session works, how its agent was started and its version, as
/// every Claude Code line carries them.
fn header(json: serde_json::Value, name: &str) -> serde_json::Value {
    let mut json = json;
    json["cwd"] = serde_json::Value::String(format!("/cwd-{name}"));
    json["gitBranch"] = serde_json::Value::String(format!("branch-{name}"));
    json["entrypoint"] = serde_json::Value::String(format!("ep-{name}"));
    json["version"] = serde_json::Value::String(format!("9.0.0-{name}"));
    json
}

fn header_proofs(name: &str, at: DateTime<Utc>) -> Vec<(&'static str, serde_json::Value)> {
    vec![
        ("cwd", serde_json::Value::String(format!("/cwd-{name}"))),
        ("gitBranch", serde_json::Value::String(format!("branch-{name}"))),
        ("entrypoint", serde_json::Value::String(format!("ep-{name}"))),
        ("version", serde_json::Value::String(format!("9.0.0-{name}"))),
        ("timestamp", time_of(at)),
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_path_a_format_declares_stored_lands_in_a_column_of_the_rows_its_line_fed() {
    use serde_json::json;
    use superx_mod_recon::crawler::lines::formats::{claude_code, gemini_cli};
    let kernel = booted_kernel().await;
    let db = own_db().await;
    let at = |minute: i64| start() + TimeDelta::minutes(minute);
    let mut proofs: Vec<Proof> = Vec::new();

    // Claude Code: the operator's prompt, its text in blocks, three triggers.
    let mut c = fixture(&kernel, "claude_code", "s-user", at(0)).await;
    let (line, when) = c.stamped(header(
        json!({ "type": "user", "origin": { "kind": "human" }, "promptId": "pid-user", "promptSource": "source-user",
            "permissionMode": "mode-user", "turnPosition": { "turnIndex": 71001, "promptIndex": 71002 },
            "message": { "role": "user", "content": "prompt-string-user" } }),
        "user",
    ));
    c.message_row(&line, &line, Kept::Both, when).await;
    let mut paths = header_proofs("user", when);
    paths.extend([
        ("message.content", json!("prompt-string-user")),
        ("promptId", json!("pid-user")),
        ("promptSource", json!("source-user")),
        ("permissionMode", json!("mode-user")),
        ("turnPosition.turnIndex", json!(71001)),
        ("turnPosition.promptIndex", json!(71002)),
    ]);
    proofs.push(Proof { agent: claude_code::AGENT, kind: "user", session: c.session.clone(), paths });
    let mut c = fixture(&kernel, "claude_code", "s-blocks", at(1)).await;
    c.kept_message(json!({ "type": "user", "origin": { "kind": "human" }, "message": { "role": "user", "content": [{ "type": "text", "text": "prompt-block-text" }] } }), Kept::Both).await;
    proofs.push(Proof { agent: claude_code::AGENT, kind: "user", session: c.session.clone(), paths: vec![("message.content[text].text", json!("prompt-block-text"))] });
    let mut c = fixture(&kernel, "claude_code", "s-trig", at(2)).await;
    for origin in [json!({ "kind": "kind-from", "from": "from-trig" }), json!({ "kind": "kind-producer", "producer": "producer-trig" }), json!({ "kind": "kind-name", "name": "name-trig" })] {
        c.kept_message(json!({ "type": "user", "origin": origin, "message": { "role": "user", "content": "a trigger" } }), Kept::Both).await;
    }
    proofs.push(Proof {
        agent: claude_code::AGENT,
        kind: "user",
        session: c.session.clone(),
        paths: vec![("origin.kind", json!("kind-from")), ("origin.from", json!("from-trig")), ("origin.producer", json!("producer-trig")), ("origin.name", json!("name-trig"))],
    });

    // Claude Code's tool results, each answering a call of one reply.
    let mut c = fixture(&kernel, "claude_code", "s-res", at(3)).await;
    let calls: Vec<serde_json::Value> = [("c-fail", "Bash"), ("c-list", "Bash"), ("c-edit", "Edit"), ("c-git", "Bash"), ("c-hash", "Bash"), ("c-secs", "WebSearch"), ("c-shell", "Bash")]
        .iter()
        .map(|(id, name)| json!({ "type": "tool_use", "id": id, "name": name, "input": { "command": "echo" } }))
        .collect();
    c.kept_message(json!({ "type": "assistant", "message": { "id": "msg-res", "model": "m", "content": calls, "usage": usage(1, 0) } }), Kept::Both).await;
    let answer = |call: &str, content: serde_json::Value, error: bool, extra: serde_json::Value| {
        let mut line = json!({ "type": "user", "message": { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": call, "content": content, "is_error": error }] } });
        if let (Some(l), serde_json::Value::Object(e)) = (line.as_object_mut(), extra) {
            l.extend(e);
        }
        line
    };
    for line in [
        answer("c-fail", json!("failed-text"), true, json!({
            "toolUseResult": { "durationMs": 72001, "persistedOutputSize": 72002, "returnCodeInterpretation": "rc-note", "backgroundTaskId": "bg-task" },
            "permissionDecision": { "decision": "decision-x", "reasonType": "reason-type-x", "source": "decision-source-x", "reasonCode": "reason-code-x" },
            "toolDenialKind": "denial-x", "userFeedback": "feedback-x", "sandboxed": true, "sandboxViolations": [{ "host": "host-x" }] })),
        answer("c-list", json!([{ "type": "text", "text": "failed-block-text" }]), true, json!({})),
        answer("c-edit", json!("ok"), false, json!({ "toolUseResult": { "filePath": "/edited-file", "oldString": "a", "newString": "b", "userModified": true } })),
        answer("c-git", json!("ok"), false, json!({ "toolUseResult": { "gitOperation": {
            "pr": { "number": 72003, "url": "https://example.test/pr/72003", "action": "created" }, "push": { "branch": "push-branch" },
            "branch": { "ref": "merge-branch", "action": "merged" }, "commit": { "sha": "abcdef72004" } } } })),
        answer("c-hash", json!("ok"), false, json!({ "toolUseResult": { "gitOperation": { "commit": { "hash": "fedcba72005" } } } })),
        answer("c-secs", json!("ok"), false, json!({ "toolUseResult": { "durationSeconds": 7.5 } })),
        answer("c-shell", json!("ok"), false, json!({ "toolUseResult": { "stdout": "", "stderr": "", "bashEditDiff": { "files": [{ "filePath": "/shell-edited", "hunks": [{ "lines": ["+x"] }] }] } } })),
    ] {
        c.kept_message(line, Kept::Both).await;
    }
    proofs.push(Proof {
        agent: claude_code::AGENT,
        kind: "user",
        session: c.session.clone(),
        paths: vec![
            ("message.content[tool_result:*].content", json!("failed-text")),
            ("message.content[tool_result:*].content[].text", json!("failed-block-text")),
            ("toolUseResult[*].durationMs", json!(72001)),
            // Seconds, held in milliseconds.
            ("toolUseResult[*].durationSeconds", json!(7500)),
            ("toolUseResult[*].filePath", json!("/edited-file")),
            ("toolUseResult[*].userModified", json!(true)),
            ("toolUseResult[*].gitOperation.pr.number", json!(72003)),
            ("toolUseResult[*].gitOperation.pr.url", json!("https://example.test/pr/72003")),
            ("toolUseResult[*].gitOperation.push.branch", json!("push-branch")),
            ("toolUseResult[*].gitOperation.branch.ref", json!("merge-branch")),
            ("toolUseResult[*].gitOperation.commit.sha", json!("abcdef72004")),
            ("toolUseResult[*].gitOperation.commit.hash", json!("fedcba72005")),
            ("toolUseResult[*].persistedOutputSize", json!(72002)),
            ("toolUseResult[*].returnCodeInterpretation", json!("rc-note")),
            ("toolUseResult[*].backgroundTaskId", json!("bg-task")),
            ("toolUseResult[*].bashEditDiff.files[].filePath", json!("/shell-edited")),
            ("permissionDecision.decision", json!("decision-x")),
            ("permissionDecision.reasonType", json!("reason-type-x")),
            ("permissionDecision.source", json!("decision-source-x")),
            ("permissionDecision.reasonCode", json!("reason-code-x")),
            ("toolDenialKind", json!("denial-x")),
            ("userFeedback", json!("feedback-x")),
            ("sandboxed", json!(true)),
            ("sandboxViolations[].host", json!("host-x")),
        ],
    });

    // Claude Code's replies.
    let usage_asst = json!({ "input_tokens": 73010, "output_tokens": 73011, "cache_creation": { "ephemeral_1h_input_tokens": 73004, "ephemeral_5m_input_tokens": 73005 } });
    let mut c = fixture(&kernel, "claude_code", "s-asst", at(4)).await;
    let content = json!([
        { "type": "thinking", "thinking": "thinking-asst", "signature": "s" },
        { "type": "tool_use", "id": "call-fp", "name": "Read", "input": { "file_path": "/fp-asst", "command": "cmd-asst", "description": "desc-asst",
            "pattern": "pat-asst", "url": "url-asst", "query": "query-asst", "subagent_type": "subagent-asst", "run_in_background": true,
            "dangerouslyDisableSandbox": true, "timeout": 73006 } },
        { "type": "tool_use", "id": "call-nb", "name": "NotebookEdit", "input": { "notebook_path": "/nb-asst" } },
        { "type": "tool_use", "id": "call-path", "name": "Grep", "input": { "path": "/path-asst" } },
        { "type": "tool_use", "id": "call-skill", "name": "Skill", "input": { "skill": "skill-asst" } },
        { "type": "tool_use", "id": "call-agent", "name": "Agent", "input": { "model": "agent-model-asst" } }
    ]);
    let message = json!({ "id": "msg-asst", "model": "model-asst", "stop_reason": "stop-asst",
        "stop_details": { "type": "refusal", "category": "refusal-cat", "explanation": "refusal-expl" },
        "diagnostics": { "cache_miss_reason": { "type": "miss-type", "cache_missed_input_tokens": 73003 } },
        "usage": usage_asst, "content": content });
    let (line, when) = c.stamped(header(
        json!({ "type": "assistant", "requestId": "req-asst", "effort": "effort-asst", "perTurnEffort": "turn-effort-asst",
            "advisorModel": "advisor-asst", "requestedModel": "requested-asst", "isSidechain": true,
            "attributionSkill": "attr-skill", "attributionPlugin": "attr-plugin", "attributionMcpServer": "attr-mcp-server", "attributionMcpTool": "attr-mcp-tool",
            "apiBlockIndex": 73001, "thinkingDurationMs": 73002, "thinkingDisplay": "display-asst", "message": message }),
        "asst",
    ));
    c.message_row(&line, &line, Kept::Both, when).await;
    let mut paths = header_proofs("asst", when);
    paths.extend([
        ("message.id", json!("msg-asst")),
        ("requestId", json!("req-asst")),
        ("message.model", json!("model-asst")),
        ("requestedModel", json!("requested-asst")),
        ("advisorModel", json!("advisor-asst")),
        ("effort", json!("effort-asst")),
        ("perTurnEffort", json!("turn-effort-asst")),
        ("message.stop_reason", json!("stop-asst")),
        ("isSidechain", json!(true)),
        ("message.usage.**", usage_asst),
        ("attributionSkill", json!("attr-skill")),
        ("attributionPlugin", json!("attr-plugin")),
        ("attributionMcpServer", json!("attr-mcp-server")),
        ("attributionMcpTool", json!("attr-mcp-tool")),
        ("message.stop_details.category", json!("refusal-cat")),
        ("message.stop_details.explanation", json!("refusal-expl")),
        ("message.diagnostics.cache_miss_reason.type", json!("miss-type")),
        ("message.diagnostics.cache_miss_reason.cache_missed_input_tokens", json!(73003)),
        ("apiBlockIndex", json!(73001)),
        ("thinkingDurationMs", json!(73002)),
        ("thinkingDisplay", json!("display-asst")),
        ("message.content[thinking].thinking", json!("thinking-asst")),
        ("message.content[tool_use:*].id", json!("call-fp")),
        ("message.content[tool_use:*].name", json!("Read")),
        ("message.content[tool_use:*].input.file_path", json!("/fp-asst")),
        ("message.content[tool_use:*].input.notebook_path", json!("/nb-asst")),
        ("message.content[tool_use:*].input.path", json!("/path-asst")),
        ("message.content[tool_use:*].input.command", json!("cmd-asst")),
        ("message.content[tool_use:*].input.description", json!("desc-asst")),
        ("message.content[tool_use:*].input.pattern", json!("pat-asst")),
        ("message.content[tool_use:*].input.url", json!("url-asst")),
        ("message.content[tool_use:*].input.query", json!("query-asst")),
        ("message.content[tool_use:*].input.subagent_type", json!("subagent-asst")),
        ("message.content[tool_use:*].input.run_in_background", json!(true)),
        ("message.content[tool_use:*].input.dangerouslyDisableSandbox", json!(true)),
        ("message.content[tool_use:*].input.timeout", json!(73006)),
        ("message.content[tool_use:Skill].input.skill", json!("skill-asst")),
        ("message.content[tool_use:Agent].input.model", json!("agent-model-asst")),
    ]);
    proofs.push(Proof { agent: claude_code::AGENT, kind: "assistant", session: c.session.clone(), paths });
    let mut c = fixture(&kernel, "claude_code", "s-fallback", at(5)).await;
    c.kept_message(json!({ "type": "assistant", "message": { "id": "msg-fb", "model": "m", "usage": usage(1, 0),
        "content": [{ "type": "fallback", "from": { "model": "fallback-from" }, "to": { "model": "m" } }] } }), Kept::Both).await;
    proofs.push(Proof { agent: claude_code::AGENT, kind: "assistant", session: c.session.clone(), paths: vec![("message.content[fallback].from.model", json!("fallback-from"))] });

    // Claude Code's turns, compactions and recaps.
    for (minute, kind, name, line, own) in [
        (6, "system/turn_duration", "turn", json!({ "type": "system", "subtype": "turn_duration", "durationMs": 74001, "messageCount": 74002 }),
            vec![("durationMs", json!(74001)), ("messageCount", json!(74002))]),
        (7, "system/compact_boundary", "compact", json!({ "type": "system", "subtype": "compact_boundary",
            "compactMetadata": { "trigger": "manual", "preTokens": 75001, "postTokens": 75002, "cumulativeDroppedTokens": 75003, "durationMs": 75004 } }),
            vec![("compactMetadata.trigger", json!("manual")), ("compactMetadata.preTokens", json!(75001)), ("compactMetadata.postTokens", json!(75002)),
                 ("compactMetadata.cumulativeDroppedTokens", json!(75003)), ("compactMetadata.durationMs", json!(75004))]),
        (8, "system/away_summary", "away", json!({ "type": "system", "subtype": "away_summary", "content": "away-recap-text" }),
            vec![("content", json!("away-recap-text"))]),
    ] {
        let mut c = fixture(&kernel, "claude_code", &format!("s-{name}"), at(minute)).await;
        let (line, when) = c.stamped(header(line, name));
        c.event_row(&line, &line, Kept::Both, None, when).await;
        let mut paths = header_proofs(name, when);
        paths.extend(own);
        proofs.push(Proof { agent: claude_code::AGENT, kind, session: c.session.clone(), paths });
    }

    // Claude Code's queued commands: a prompt, its text in parts, triggers.
    let mut c = fixture(&kernel, "claude_code", "s-queued", at(9)).await;
    let (line, when) = c.stamped(header(
        json!({ "type": "attachment", "promptId": "pid-queued", "promptSource": "source-queued", "permissionMode": "mode-queued",
            "turnPosition": { "turnIndex": 76001, "promptIndex": 76002 },
            "attachment": { "type": "queued_command", "prompt": "queued-prompt-text", "origin": { "kind": "human" } } }),
        "queued",
    ));
    c.event_row(&line, &line, Kept::Both, None, when).await;
    for attachment in [
        json!({ "type": "queued_command", "prompt": [{ "type": "text", "text": "queued-part-text" }], "origin": { "kind": "human" } }),
        json!({ "type": "queued_command", "prompt": "queued-trigger", "origin": { "kind": "q-kind", "from": "q-from" } }),
        json!({ "type": "queued_command", "prompt": "queued-trigger", "origin": { "kind": "q-kind-2", "producer": "q-producer" } }),
        json!({ "type": "queued_command", "prompt": "queued-trigger", "origin": { "kind": "q-kind-3", "name": "q-name" } }),
        json!({ "type": "queued_command", "prompt": "queued-trigger", "commandMode": "q-mode" }),
    ] {
        c.kept_event(json!({ "type": "attachment", "attachment": attachment }), Kept::Both, None).await;
    }
    let mut paths = header_proofs("queued", when);
    paths.extend([
        ("attachment.prompt", json!("queued-prompt-text")),
        ("promptId", json!("pid-queued")),
        ("promptSource", json!("source-queued")),
        ("permissionMode", json!("mode-queued")),
        ("turnPosition.turnIndex", json!(76001)),
        ("turnPosition.promptIndex", json!(76002)),
        ("attachment.prompt[].text", json!("queued-part-text")),
        ("attachment.origin.kind", json!("q-kind")),
        ("attachment.origin.from", json!("q-from")),
        ("attachment.origin.producer", json!("q-producer")),
        ("attachment.origin.name", json!("q-name")),
        ("attachment.commandMode", json!("q-mode")),
    ]);
    proofs.push(Proof { agent: claude_code::AGENT, kind: "attachment/queued_command", session: c.session.clone(), paths });

    // Gemini CLI: the operator's prompt as text and in parts, and a reply.
    let mut g = fixture(&kernel, "gemini_cli", "g-user", at(10)).await;
    let (line, when) = g.stamped(json!({ "id": "gu-1", "type": "user", "content": "gem-prompt-string" }));
    g.message_row(&line, &line, Kept::Both, when).await;
    proofs.push(Proof { agent: gemini_cli::AGENT, kind: "user", session: g.session.clone(), paths: vec![("timestamp", time_of(when)), ("content", json!("gem-prompt-string"))] });
    let mut g = fixture(&kernel, "gemini_cli", "g-parts", at(11)).await;
    g.kept_message(json!({ "id": "gu-2", "type": "user", "content": [{ "text": "gem-part-text" }] }), Kept::Both).await;
    proofs.push(Proof { agent: gemini_cli::AGENT, kind: "user", session: g.session.clone(), paths: vec![("content[].text", json!("gem-part-text"))] });
    let tokens = json!({ "input": 77001, "output": 77002, "cached": 7, "thoughts": 6, "tool": 5, "total": 154021 });
    let mut g = fixture(&kernel, "gemini_cli", "g-reply", at(12)).await;
    let (line, when) = g.stamped(json!({ "id": "gem-id", "type": "gemini", "model": "gem-model", "content": "x", "tokens": tokens,
        "thoughts": [{ "subject": "gem-subject", "description": "gem-thought", "timestamp": "2026-10-25T09:30:00Z" }],
        "toolCalls": [
            { "id": "gc-read", "name": "read_file", "status": "error", "timestamp": "2026-10-25T09:31:00Z",
              "args": { "file_path": "/gem-fp", "description": "gem-desc", "pattern": "gem-pat", "url": "gem-url", "query": "gem-query", "agent_name": "gem-agent", "is_background": true },
              "result": [{ "functionResponse": { "response": { "output": "gem-output-text" } } }] },
            { "id": "gc-ls", "name": "list_directory", "status": "error", "args": { "path": "/gem-path" },
              "result": [{ "functionResponse": { "response": { "error": "gem-error-text" } } }] },
            { "id": "gc-glob", "name": "glob", "status": "success", "args": { "dir_path": "/gem-dir" }, "result": [] },
            { "id": "gc-shell", "name": "run_shell_command", "status": "success", "args": { "command": "gem-cmd" }, "result": [] },
            { "id": "gc-skill", "name": "activate_skill", "status": "success", "args": { "name": "gem-skill" }, "result": [] }
        ] }));
    g.message_row(&line, &line, Kept::Both, when).await;
    proofs.push(Proof {
        agent: gemini_cli::AGENT,
        kind: "gemini",
        session: g.session.clone(),
        paths: vec![
            ("timestamp", time_of(when)),
            ("id", json!("gem-id")),
            ("model", json!("gem-model")),
            ("tokens.**", tokens),
            ("thoughts[].description", json!("gem-thought")),
            ("thoughts[].subject", json!("gem-subject")),
            ("thoughts[].timestamp", json!("2026-10-25T09:30:00Z")),
            ("toolCalls[].id", json!("gc-read")),
            ("toolCalls[].name", json!("read_file")),
            ("toolCalls[].timestamp", json!("2026-10-25T09:31:00Z")),
            ("toolCalls[].args.file_path", json!("/gem-fp")),
            ("toolCalls[].args.path", json!("/gem-path")),
            ("toolCalls[].args.dir_path", json!("/gem-dir")),
            ("toolCalls[].args.command", json!("gem-cmd")),
            ("toolCalls[].args.description", json!("gem-desc")),
            ("toolCalls[].args.pattern", json!("gem-pat")),
            ("toolCalls[].args.url", json!("gem-url")),
            ("toolCalls[].args.query", json!("gem-query")),
            ("toolCalls[].args.agent_name", json!("gem-agent")),
            ("toolCalls[].args.is_background", json!(true)),
            ("toolCalls[].args.name", json!("gem-skill")),
            ("toolCalls[].result[].functionResponse.response.output", json!("gem-output-text")),
            ("toolCalls[].result[].functionResponse.response.error", json!("gem-error-text")),
        ],
    });

    let (_asked, mut stop) = never_stop();
    let p = pass_on(&kernel, &db, &params(&[]), &mut Crawler::default(), &mut stop).await.expect("pass");
    assert_eq!((p.outcome, p.source("lines").map(|s| s.unread)), ("complete", Some(0)), "{p:?}");

    // A fixture for every path each format declares, kind by kind.
    for (agent, every, declared) in [
        (claude_code::AGENT, claude_code::STORED_ON_EVERY_LINE, claude_code::STORED),
        (gemini_cli::AGENT, gemini_cli::STORED_ON_EVERY_LINE, gemini_cli::STORED),
    ] {
        for (kind, own) in declared {
            let want: std::collections::BTreeSet<&str> = every.iter().chain(own.iter()).copied().collect();
            let proven: std::collections::BTreeSet<&str> =
                proofs.iter().filter(|p| p.agent == agent && p.kind == *kind).flat_map(|p| p.paths.iter().map(|(path, _)| *path)).collect();
            assert_eq!(proven, want, "{agent} {kind}: a fixture value for every declared path, and none undeclared");
        }
    }
    // Each value in a column of the rows its line fed. A flag holds no value
    // of its own: it is found as a flag that is set.
    for proof in &proofs {
        let fed = fed(&db, &proof.session).await;
        for (path, value) in &proof.paths {
            assert!(fed.iter().any(|row| holds(row, value)), "{} {} {path}: {value} is in no column: {fed:#?}", proof.agent, proof.kind);
        }
    }
}

#[tokio::test]
async fn a_prompt_takes_its_prompt_id_source_permission_mode_turn_index_and_prompt_index_each_none_where_absent() {
    let kernel = booted_kernel().await;
    let db = own_db().await;
    let mut c = fixture(&kernel, "claude_code", "p3", start()).await;
    let human = serde_json::json!({ "kind": "human" });
    c.kept_message(serde_json::json!({ "type": "user", "origin": human, "promptId": "turn-1", "promptSource": "typed", "permissionMode": "acceptEdits",
        "turnPosition": { "turnIndex": 3, "promptIndex": 4 }, "message": { "role": "user", "content": "with all" } }), Kept::Both).await;
    c.kept_message(serde_json::json!({ "type": "user", "origin": human, "message": { "role": "user", "content": "with none" } }), Kept::Both).await;
    c.kept_event(serde_json::json!({ "type": "attachment", "promptId": "turn-3", "promptSource": "queued", "permissionMode": "plan",
        "turnPosition": { "turnIndex": 5, "promptIndex": 6 },
        "attachment": { "type": "queued_command", "prompt": "while busy", "origin": human } }), Kept::Both, None).await;
    let (_asked, mut stop) = never_stop();
    pass_on(&kernel, &db, &params(&[]), &mut Crawler::default(), &mut stop).await.expect("pass");
    let prompts = rows(&db, "SELECT kind, prompt_id, source, permission_mode, turn_index, prompt_index, at FROM prompt ORDER BY at").await;
    let got: Vec<_> = prompts
        .iter()
        .map(|p| (text(p, "kind"), text(p, "prompt_id"), text(p, "source"), text(p, "permission_mode"), int(p, "turn_index"), int(p, "prompt_index")))
        .collect();
    let some = |s: &str| Some(s.to_string());
    assert_eq!(
        got,
        [
            (some("typed"), some("turn-1"), some("typed"), some("acceptEdits"), Some(3), Some(4)),
            (some("typed"), None, None, None, None, None),
            (some("queued"), some("turn-3"), some("queued"), some("plan"), Some(5), Some(6)),
        ]
    );
}

#[tokio::test]
async fn a_task_notification_a_peer_a_coordinator_an_auto_continuation_a_queued_command_by_its_origin_or_its_command_mode_a_compact_summary_and_a_subagents_first_line_are_each_a_trigger_and_a_meta_line_shell_output_and_a_prompt_mode_queued_command_none() {
    use serde_json::json;
    let kernel = booted_kernel().await;
    let db = own_db().await;
    let mut c = fixture(&kernel, "claude_code", "t4", start()).await;
    let user = |extra: serde_json::Value, content: &str| {
        let mut line = json!({ "type": "user", "message": { "role": "user", "content": content } });
        if let (Some(l), serde_json::Value::Object(e)) = (line.as_object_mut(), extra) {
            l.extend(e);
        }
        line
    };
    let queued = |attachment: serde_json::Value| json!({ "type": "attachment", "attachment": attachment });
    for line in [
        user(json!({ "origin": { "kind": "task-notification", "producer": "Monitor" }, "promptId": "turn-9" }), "<task-notification>done</task-notification>"),
        user(json!({ "origin": { "kind": "peer", "from": "session-b" } }), "please review"),
        user(json!({ "origin": { "kind": "coordinator", "name": "lead" } }), "take the next step"),
        user(json!({ "origin": { "kind": "auto-continuation" } }), "continue"),
        user(json!({ "isCompactSummary": true }), "This session is being continued"),
        user(json!({ "isSidechain": true, "parentUuid": null }), "Investigate the failing tests"),
        user(json!({ "isMeta": true }), "<system-reminder>a reminder</system-reminder>"),
        user(json!({}), "<bash-stdout>ok</bash-stdout>"),
    ] {
        c.kept_message(line, Kept::Both).await;
    }
    for attachment in [
        json!({ "type": "queued_command", "prompt": "<task-notification>bg</task-notification>", "origin": { "kind": "task-notification", "from": "bg-1" } }),
        json!({ "type": "queued_command", "prompt": "a scheduled wakeup", "commandMode": "wakeup" }),
        json!({ "type": "queued_command", "prompt": "typed while busy", "commandMode": "prompt" }),
    ] {
        c.kept_event(queued(attachment), Kept::Both, None).await;
    }
    let (_asked, mut stop) = never_stop();
    let p = pass_on(&kernel, &db, &params(&[]), &mut Crawler::default(), &mut stop).await.expect("pass");
    assert_eq!(p.source("lines").map(|s| s.unread), Some(0));
    let triggers = rows(&db, "SELECT kind, source, prompt_id, text, chars, at FROM trigger ORDER BY at").await;
    let got: Vec<_> = triggers.iter().map(|t| (text(t, "kind"), text(t, "source"), text(t, "text"))).collect();
    let row = |kind: &str, source: Option<&str>, said: &str| (Some(kind.to_string()), source.map(str::to_string), Some(said.to_string()));
    assert_eq!(
        got,
        [
            row("task-notification", Some("Monitor"), "<task-notification>done</task-notification>"),
            row("peer", Some("session-b"), "please review"),
            row("coordinator", Some("lead"), "take the next step"),
            row("auto-continuation", None, "continue"),
            row("compact_summary", None, "This session is being continued"),
            row("subagent", None, "Investigate the failing tests"),
            row("task-notification", Some("bg-1"), "<task-notification>bg</task-notification>"),
            row("wakeup", None, "a scheduled wakeup"),
        ]
    );
    assert_eq!((text(&triggers[0], "prompt_id").as_deref(), int(&triggers[0], "chars")), (Some("turn-9"), Some(43)));
    assert_eq!(count(&db, "prompt").await, 0, "none of them is the operator's");
}

#[tokio::test]
async fn a_reply_takes_its_prompt_id_client_version_skill_plugin_mcp_server_mcp_tool_refusal_refusal_text_cache_miss_and_cache_missed_and_a_reply_state_its_cache_writes_by_duration_and_a_session_first_read_takes_its_prompt_id_from_its_newest_prompt_or_trigger_row() {
    use serde_json::json;
    let kernel = booted_kernel().await;
    let db = own_db().await;
    let mut c = fixture(&kernel, "claude_code", "r5", start()).await;
    c.kept_message(json!({ "type": "user", "origin": { "kind": "human" }, "promptId": "turn-1", "message": { "role": "user", "content": "go" } }), Kept::Both).await;
    let mut with_writes = usage(900, 600);
    with_writes["cache_creation"] = json!({ "ephemeral_1h_input_tokens": 300, "ephemeral_5m_input_tokens": 200 });
    c.kept_message(json!({ "type": "assistant", "version": "2.1.305", "promptId": "turn-1",
        "attributionSkill": "commit", "attributionPlugin": "git-tools", "attributionMcpServer": "github", "attributionMcpTool": "create_pr",
        "message": { "id": "msg_r", "model": "claude-opus-5-5", "stop_reason": "refusal",
            "stop_details": { "type": "refusal", "category": "cyber", "explanation": "declined" },
            "diagnostics": { "cache_miss_reason": { "type": "ttl_expired", "cache_missed_input_tokens": 1200 } },
            "content": [{ "type": "text", "text": "No." }], "usage": with_writes } }), Kept::Both).await;
    let (_asked, mut stop) = never_stop();
    let mut crawler = Crawler::default();
    pass_on(&kernel, &db, &params(&[]), &mut crawler, &mut stop).await.expect("pass");
    let reply = rows(&db, "SELECT * FROM reply").await;
    let got: Vec<Option<String>> = ["prompt_id", "client_version", "skill", "plugin", "mcp_server", "mcp_tool", "refusal", "refusal_text", "cache_miss"]
        .iter()
        .map(|f| text(&reply[0], f))
        .collect();
    let want: Vec<Option<String>> = ["turn-1", "2.1.305", "commit", "git-tools", "github", "create_pr", "cyber", "declined", "ttl_expired"]
        .iter()
        .map(|s| Some((*s).to_string()))
        .collect();
    assert_eq!(got, want);
    assert_eq!(int(&reply[0], "cache_missed"), Some(1200));
    let state = rows(&db, "SELECT cache_write_1h_tokens, cache_write_5m_tokens FROM reply_state").await;
    assert_eq!((int(&state[0], "cache_write_1h_tokens"), int(&state[0], "cache_write_5m_tokens")), (Some(300), Some(200)));

    // A session read again after a restart, from past its last prompt and
    // trigger: its reply's turn is the newest of their rows.
    let mut c = fixture(&kernel, "claude_code", "r5b", start() + TimeDelta::minutes(10)).await;
    c.kept_message(json!({ "type": "user", "origin": { "kind": "human" }, "promptId": "turn-a", "message": { "role": "user", "content": "first" } }), Kept::Both).await;
    c.kept_message(json!({ "type": "user", "origin": { "kind": "task-notification" }, "promptId": "turn-b", "message": { "role": "user", "content": "done" } }), Kept::Both).await;
    c.at += TimeDelta::minutes(2);
    c.kept_message(json!({ "type": "user", "isMeta": true, "message": { "role": "user", "content": "<system-reminder>x</system-reminder>" } }), Kept::Both).await;
    pass_on(&kernel, &db, &params(&[]), &mut crawler, &mut stop).await.expect("pass");
    c.at += TimeDelta::minutes(1);
    c.kept_message(json!({ "type": "assistant", "message": { "id": "msg_after", "model": "claude-opus-5-5", "content": [{ "type": "text", "text": "ok" }], "usage": usage(1, 0) } }), Kept::Both).await;
    let after = pass_on(&kernel, &db, &params(&[]), &mut Crawler::default(), &mut stop).await.expect("after a restart");
    let lines = after.source("lines").expect("lines");
    assert_eq!(lines.items, 2, "only the reminder and the reply are read again: {after:?}");
    let reply = rows(&db, "SELECT prompt_id FROM reply WHERE agent_id = 'msg_after'").await;
    assert_eq!(text(&reply[0], "prompt_id").as_deref(), Some("turn-b"), "the newest of the session's prompt and trigger rows");
}

/// A line of a reply written before Claude Code numbered its blocks.
fn unindexed(id: &str, content: serde_json::Value, extra: serde_json::Value) -> serde_json::Value {
    let mut line = block(id, 0, content, extra);
    line.as_object_mut().expect("object").remove("apiBlockIndex");
    line
}

#[tokio::test]
async fn a_reply_read_again_from_its_middle_with_a_subagents_lines_between_its_own_counts_each_of_its_lines_once_and_places_its_blocks_as_when_read_in_one_pass() {
    use serde_json::json;
    let mut results = Vec::new();
    for restart in [false, true] {
        let kernel = booted_kernel().await;
        let db = own_db().await;
        let mut c = fixture(&kernel, "claude_code", "r6", start()).await;
        c.kept_message(unindexed("msg_main", json!({ "type": "text", "text": "Alpha" }), json!({})), Kept::Both).await;
        c.kept_message(unindexed("msg_sub", json!({ "type": "text", "text": "sub" }), json!({ "isSidechain": true })), Kept::Both).await;
        c.kept_message(unindexed("msg_main", json!({ "type": "text", "text": "Beta!" }), json!({})), Kept::Both).await;
        c.kept_message(unindexed("msg_sub", json!({ "type": "text", "text": "sub2" }), json!({ "isSidechain": true })), Kept::Both).await;
        c.at += TimeDelta::minutes(5);
        c.kept_message(json!({ "type": "user", "isMeta": true, "message": { "role": "user", "content": "<system-reminder>x</system-reminder>" } }), Kept::Both).await;
        let (_asked, mut stop) = never_stop();
        if restart {
            pass_on(&kernel, &db, &params(&[]), &mut Crawler::default(), &mut stop).await.expect("first pass");
        }
        c.at += TimeDelta::minutes(1);
        c.kept_message(unindexed("msg_main", json!({ "type": "thinking", "thinking": "hmm", "signature": "s" }), json!({})), Kept::Both).await;
        let last = pass_on(&kernel, &db, &params(&[]), &mut Crawler::default(), &mut stop).await.expect("pass");
        if restart {
            assert_eq!(last.source("lines").map(|s| s.items), Some(2), "it began past the reply's first lines: {last:?}");
        }
        let thinking = rows(&db, "SELECT position, chars FROM thinking").await;
        let states = rows(&db, "SELECT text_chars FROM reply_state WHERE reply.agent_id = 'msg_main'").await;
        results.push((
            thinking.iter().map(|t| (int(t, "position"), int(t, "chars"))).collect::<Vec<_>>(),
            sorted(states.iter().map(|s| int(s, "text_chars"))),
        ));
    }
    assert_eq!(results[0], (vec![(Some(2), Some(3))], vec![Some(5), Some(10)]), "read in one pass");
    assert_eq!(results[1], results[0], "read again from its middle, the same");
}

#[tokio::test]
async fn a_call_takes_skill_model_and_timeout_ms_and_a_result_takes_the_permission_decisions_four_columns_its_denial_operator_feedback_sandboxed_persisted_bytes_return_note_and_background_task_and_its_text_when_it_did_not_end_ok() {
    use serde_json::json;
    let kernel = booted_kernel().await;
    let db = own_db().await;
    let mut c = fixture(&kernel, "claude_code", "c7", start()).await;
    let call = |id: &str, name: &str, input: serde_json::Value| json!({ "type": "tool_use", "id": id, "name": name, "input": input });
    c.kept_message(json!({ "type": "assistant", "message": { "id": "msg_c", "model": "claude-opus-5-5", "usage": usage(1, 0), "content": [
        call("c-skill", "Skill", json!({ "skill": "commit" })),
        call("c-agent", "Agent", json!({ "model": "claude-haiku-5-5", "prompt": "look" })),
        call("c-bash", "Bash", json!({ "command": "grep -r x", "timeout": 120000 }))
    ] } }), Kept::Both).await;
    let answer = |call: &str, content: &str, error: bool, extra: serde_json::Value| {
        let mut line = json!({ "type": "user", "message": { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": call, "content": content, "is_error": error }] } });
        if let (Some(l), serde_json::Value::Object(e)) = (line.as_object_mut(), extra) {
            l.extend(e);
        }
        line
    };
    for line in [
        answer("c-bash", "", false, json!({
            "permissionDecision": { "decision": "accept", "reasonType": "rule", "source": "config", "reasonCode": "rule:userSettings" },
            "sandboxed": true,
            "toolUseResult": { "stdout": "", "stderr": "", "persistedOutputSize": 50000, "returnCodeInterpretation": "No matches found", "backgroundTaskId": "task-9" } })),
        answer("c-agent", "The user doesn't want to proceed with this tool use. The tool use was rejected", true,
            json!({ "toolDenialKind": "user-rejected", "userFeedback": "use sonnet instead", "sandboxed": false })),
        answer("c-skill", "Skill not found", true, json!({})),
    ] {
        c.kept_message(line, Kept::Both).await;
    }
    let mut g = fixture(&kernel, "gemini_cli", "c7g", c.at).await;
    g.kept_message(json!({ "id": "g-c", "type": "gemini", "model": "gemini-3.1-pro-preview", "content": "",
        "toolCalls": [{ "id": "gs-1", "name": "activate_skill", "status": "success", "args": { "name": "refactor" }, "result": [] }] }), Kept::Both).await;
    let (_asked, mut stop) = never_stop();
    let p = pass_on(&kernel, &db, &params(&[]), &mut Crawler::default(), &mut stop).await.expect("pass");
    assert_eq!(p.source("lines").map(|s| s.unread), Some(0));
    let calls = rows(&db, "SELECT call_id, skill, model, timeout_ms FROM tool_call ORDER BY call_id").await;
    let got: Vec<_> = calls.iter().map(|c| (text(c, "call_id"), text(c, "skill"), text(c, "model"), int(c, "timeout_ms"))).collect();
    let some = |s: &str| Some(s.to_string());
    assert_eq!(
        got,
        [
            (some("c-agent"), None, some("claude-haiku-5-5"), None),
            (some("c-bash"), None, None, Some(120_000)),
            (some("c-skill"), some("commit"), None, None),
            (some("gs-1"), some("refactor"), None, None),
        ]
    );
    let results = rows(&db, "SELECT *, tool_call.call_id AS call FROM tool_result ORDER BY call").await;
    let field = |call: &str, f: &str| results.iter().find(|r| text(r, "call").as_deref() == Some(call)).and_then(|r| text(r, f));
    let flag = |call: &str| results.iter().find(|r| text(r, "call").as_deref() == Some(call)).and_then(|r| match r {
        Value::Object(o) => match o.get("sandboxed") {
            Some(Value::Bool(b)) => Some(*b),
            _ => None,
        },
        _ => None,
    });
    assert_eq!(
        ["decision", "decided_by", "decision_source", "decision_reason", "return_note", "background_task"].map(|f| field("c-bash", f)),
        [some("accept"), some("rule"), some("config"), some("rule:userSettings"), some("No matches found"), some("task-9")]
    );
    let bash = results.iter().find(|r| text(r, "call").as_deref() == Some("c-bash")).expect("a result");
    assert_eq!((int(bash, "persisted_bytes"), flag("c-bash"), field("c-bash", "text")), (Some(50_000), Some(true), None), "it ended ok: no text");
    assert_eq!(
        (field("c-agent", "outcome"), field("c-agent", "denial"), field("c-agent", "operator_feedback"), flag("c-agent")),
        (some("denied"), some("user-rejected"), some("use sonnet instead"), Some(false))
    );
    assert_eq!(field("c-agent", "text"), some("The user doesn't want to proceed with this tool use. The tool use was rejected"));
    assert_eq!((field("c-skill", "outcome"), field("c-skill", "text"), field("c-skill", "decision")), (some("error"), some("Skill not found"), None));
}

#[tokio::test]
async fn a_shell_commands_bash_edit_diff_is_one_edit_per_file_created_deleted_or_modified_counted_from_its_hunks_and_a_shared_diff_or_one_of_a_command_that_did_not_end_ok_is_none() {
    use serde_json::json;
    let kernel = booted_kernel().await;
    let db = own_db().await;
    let mut c = fixture(&kernel, "claude_code", "e7", start()).await;
    let bash = |id: &str| json!({ "type": "tool_use", "id": id, "name": "Bash", "input": { "command": "sed -i s/a/b/ *.rs" } });
    c.kept_message(json!({ "type": "assistant", "message": { "id": "msg_e", "model": "claude-opus-5-5", "usage": usage(1, 0),
        "content": [bash("b-ok"), bash("b-shared"), bash("b-failed")] } }), Kept::Both).await;
    let answer = |call: &str, error: bool, diff: serde_json::Value| json!({ "type": "user",
        "message": { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": call, "content": "", "is_error": error }] },
        "toolUseResult": { "stdout": "", "stderr": "", "bashEditDiff": diff } });
    let files = |name: &str| json!([{ "filePath": format!("/w/{name}.rs"), "hunks": [{ "lines": ["-a", "+b"] }] }]);
    for line in [
        answer("b-ok", false, json!({ "files": [
            { "filePath": "/w/a.rs", "hunks": [{ "lines": ["@@ -1,2 +1,2 @@", "-old", "+new", " same"] }] },
            { "filePath": "/w/new.rs", "created": true, "hunks": [{ "lines": ["+one", "+two"] }] },
            { "filePath": "/w/gone.rs", "deleted": true, "hunks": [{ "lines": ["-x", "-y", "-z"] }] }
        ] })),
        answer("b-shared", false, json!({ "shared": true, "files": files("shared") })),
        answer("b-failed", true, json!({ "files": files("failed") })),
    ] {
        c.kept_message(line, Kept::Both).await;
    }
    let (_asked, mut stop) = never_stop();
    let p = pass_on(&kernel, &db, &params(&[]), &mut Crawler::default(), &mut stop).await.expect("pass");
    assert_eq!(p.source("lines").map(|s| s.unread), Some(0));
    let edits = rows(&db, "SELECT path, kind, added, removed, counted_from, operator_changed FROM edit ORDER BY path").await;
    let got: Vec<_> = edits
        .iter()
        .map(|e| (text(e, "path"), text(e, "kind"), int(e, "added"), int(e, "removed"), text(e, "counted_from")))
        .collect();
    let edit = |path: &str, kind: &str, added, removed| (Some(path.to_string()), Some(kind.to_string()), Some(added), Some(removed), Some("shell".to_string()));
    assert_eq!(got, [edit("/w/a.rs", "modified", 1, 1), edit("/w/gone.rs", "deleted", 0, 3), edit("/w/new.rs", "created", 2, 0)]);
    assert!(edits.iter().all(|e| matches!(e, Value::Object(o) if o.get("operator_changed") == Some(&Value::Bool(false)))));
}

#[tokio::test]
async fn pass_source_counts_holds_lines_held_lines_unused_fields_seen_fields_new_and_fields_not_stored_and_status_prints_them_on_the_lines_sources_line_as_step_8_words_them() {
    use superx_mod_recon::credentials::Credentials;
    use superx_mod_recon::status::{holdings, text_of, Database, Folder};
    let kernel = booted_kernel().await;
    let db = own_db().await;
    let mut c = fixture(&kernel, "claude_code", "s8", start()).await;
    c.kept_message(serde_json::json!({ "type": "user", "origin": { "kind": "human" }, "message": { "role": "user", "content": "count me" } }), Kept::Both).await;
    c.kept_event(serde_json::json!({ "type": "file-history-snapshot", "messageId": "m", "snapshot": { "trackedFileBackups": {} } }), Kept::Both, None).await;
    c.kept_event(serde_json::json!({ "type": "hologram" }), Kept::Both, None).await;
    let (_asked, mut stop) = never_stop();
    let mut crawler = Crawler::default();
    let p = pass_on(&kernel, &db, &params(&[]), &mut crawler, &mut stop).await.expect("pass");
    let recorded = rows(&db, "SELECT counts FROM pass_source WHERE source = 'lines'").await;
    let counts = match &recorded[0] {
        Value::Object(o) => match o.get("counts") {
            Some(Value::Object(c)) => c.clone(),
            other => unreachable!("counts is an object: {other:?}"),
        },
        other => unreachable!("a row: {other:?}"),
    };
    let n = |name: &str| match counts.get(name) {
        Some(Value::Number(v)) => v.to_int().expect("a count"),
        other => unreachable!("{name}: {other:?}"),
    };
    let fields = count(&db, "line_field").await;
    let not_stored = rows(&db, "SELECT VALUE id FROM line_field WHERE stored = false").await.len();
    assert_eq!(
        (n("lines_held"), n("lines_unused"), n("fields_seen"), n("fields_new"), n("fields_not_stored")),
        (3, 2, fields, fields, i64::try_from(not_stored).expect("fits")),
        "three lines held and read, two fed no table, every field new on the first pass"
    );
    assert!(not_stored > 0 && i64::try_from(not_stored).expect("fits") < fields);
    let rows_written = p.source("lines").map(|s| s.rows_written()).expect("lines");
    let folder = Folder {
        credentials_path: PathBuf::from("/home/op/modules/recon/credentials.json"),
        credentials: Credentials::None,
        repos: PathBuf::from("/home/op/modules/recon/repos"),
    };
    let status = text_of(&Database::Ready(holdings(&db).await.expect("holdings")), &folder, &from_values(&BTreeMap::new()));
    let want = format!(
        "  lines     3 read of 3 held, 2 fed no table, 1 in a shape it does not know · {fields} fields, {fields} new, {not_stored} not stored · {rows_written} rows\n"
    );
    assert!(status.contains(&want), "{want}\n{status}");
}
