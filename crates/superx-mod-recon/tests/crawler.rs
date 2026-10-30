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
use superx_kernel::message::json_to_object;
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
    assert_eq!(lines_rows(&first), counts.iter().map(|(_, n)| n).sum::<i64>());
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
