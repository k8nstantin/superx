//! The watcher (#566): recon's second part, tested on SurrealDB's in-memory
//! engine with lines written as the Claude Code adapter writes them, at the
//! times each test gives; every look is given the time it judges at, so no
//! test waits for a threshold. One test per item of the task file's list,
//! named by the item it proves.

pub mod common;

use std::path::{Path, PathBuf};

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use chrono::{DateTime, TimeDelta, Utc};
use common::*;
use serde_json::{json, Value as Json};
use superx_kernel::adapters::claude_code::{ClaudeCodeAdapter, PROJECTS_ROOT_PARAM};
use superx_kernel::registry::KernelModule as _;
use superx_kernel::types::{Object, RecordId, Value};
use superx_kernel::Kernel;
use superx_mod_recon::instance::Instance;
use superx_mod_recon::params::{self, Params, ALL, NAMES};
use superx_mod_recon::watcher::line::{self, Came, Happened};
use superx_mod_recon::watcher::report::{Kind, Level};
use superx_mod_recon::watcher::words::{self, Class};
use superx_mod_recon::watcher::{spec, Snapshot, Watched, Watcher};
use superx_mod_recon::{registry, server};
use tower::ServiceExt as _;

const PORT: u16 = 5152;

/// An id the kernel would give a line it captured at `at`.
fn id_at(table: &str, at: DateTime<Utc>) -> RecordId {
    let stamp = uuid::Timestamp::from_unix(uuid::NoContext, u64::try_from(at.timestamp()).expect("after 1970"), at.timestamp_subsec_nanos());
    RecordId::new(table, superx_kernel::types::Uuid::from(uuid::Uuid::new_v7(stamp)))
}

/// A Claude Code session, its lines written as the adapter writes them: a
/// user or assistant line a message row, its text beside its parsed copy;
/// every other line a transcript_event, its payload whole.
struct Agent {
    kernel: Kernel,
    agent: RecordId,
    session: RecordId,
    key: String,
    transcript: String,
    cwd: String,
    at: DateTime<Utc>,
}

impl Agent {
    async fn new(kernel: &Kernel, key: &str, transcript: &Path, cwd: &Path, at: DateTime<Utc>) -> Self {
        let agent = kernel.create_entity("node_agent").await.expect("agent");
        let mut d = Object::new();
        d.insert("name".to_string(), Value::String("claude_code".to_string()));
        kernel.supersede_state(agent.clone(), "attr_agent_descriptor", Value::Object(d)).await.expect("agent descriptor");
        let session = kernel.create_entity("node_session").await.expect("session");
        let mut d = Object::new();
        d.insert("name".to_string(), Value::String(format!("claude_code/{key}")));
        d.insert("session".to_string(), Value::String(key.to_string()));
        kernel.supersede_state(session.clone(), "attr_session_descriptor", Value::Object(d)).await.expect("session descriptor");
        Self {
            kernel: kernel.clone(),
            agent,
            session,
            key: key.to_string(),
            transcript: transcript.to_string_lossy().into_owned(),
            cwd: cwd.to_string_lossy().into_owned(),
            at,
        }
    }

    /// The session's id, as the page names it.
    fn id(&self) -> String {
        superx_ops_uuid(&self.session)
    }

    fn stamp(&mut self, json: &mut Json) -> DateTime<Utc> {
        self.at += TimeDelta::seconds(1);
        json["timestamp"] = Json::String(self.at.to_rfc3339());
        json["sessionId"] = Json::String(self.key.clone());
        if json.get("cwd").is_none() {
            json["cwd"] = Json::String(self.cwd.clone());
        }
        self.at
    }

    /// A user or assistant line, as `message` keeps it.
    async fn message(&mut self, mut json: Json) -> DateTime<Utc> {
        let at = self.stamp(&mut json);
        self.kernel
            .db()
            .query(
                "CREATE $id CONTENT { session: $sess, agent: $agent, role: 'x', content: '', raw: $raw, raw_text: $text, \
                 emitted_at: $at, valid_from: $at }",
            )
            .bind(("id", id_at("message", at)))
            .bind(("sess", self.session.clone()))
            .bind(("agent", self.agent.clone()))
            .bind(("raw", Value::Object(superx_kernel::message::json_to_object(&json))))
            .bind(("text", json.to_string()))
            .bind(("at", at))
            .await
            .expect("line")
            .check()
            .expect("line taken");
        at
    }

    /// Any other line, as a `transcript_event` keeps it whole.
    async fn event(&mut self, mut json: Json) -> DateTime<Utc> {
        let at = self.stamp(&mut json);
        let payload = json!({
            "session": self.key, "kind": json["type"], "file": self.transcript, "line": json, "raw_text": json.to_string()
        });
        self.kernel
            .db()
            .query("CREATE $id CONTENT { lifecycle_event: 'transcript_event', payload: $payload, agent: $agent, valid_from: $at }")
            .bind(("id", id_at("telemetry_stream", at)))
            .bind(("payload", superx_kernel::message::value_from_json(&payload)))
            .bind(("agent", self.agent.clone()))
            .bind(("at", at))
            .await
            .expect("event")
            .check()
            .expect("event taken");
        at
    }

    async fn typed(&mut self, text: &str) -> DateTime<Utc> {
        self.message(json!({"type": "user", "origin": {"kind": "human"}, "message": {"role": "user", "content": text}})).await
    }

    async fn queued(&mut self, text: &str) -> DateTime<Utc> {
        self.event(json!({"type": "attachment", "attachment": {"type": "queued_command", "prompt": text, "origin": {"kind": "human"}}})).await
    }

    async fn interrupt(&mut self) -> DateTime<Utc> {
        self.message(json!({"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": "[Request interrupted by user]"}]}}))
            .await
    }

    async fn said(&mut self, text: &str) -> DateTime<Utc> {
        self.message(json!({"type": "assistant", "message": {"model": "claude-opus-5-5", "content": [{"type": "text", "text": text}]}})).await
    }

    async fn call(&mut self, id: &str, tool: &str, input: Json) -> DateTime<Utc> {
        self.message(json!({"type": "assistant", "message": {"model": "claude-opus-5-5", "content": [
            {"type": "tool_use", "id": id, "name": tool, "input": input}
        ]}}))
        .await
    }

    async fn result(&mut self, id: &str, failed: bool, content: &str) -> DateTime<Utc> {
        self.message(json!({"type": "user", "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": id, "is_error": failed, "content": content}
        ]}}))
        .await
    }

    async fn bash(&mut self, id: &str, command: &str) -> DateTime<Utc> {
        let at = self.call(id, "Bash", json!({"command": command})).await;
        self.result(id, false, "done").await;
        at
    }

    async fn notified(&mut self, call: &str) -> DateTime<Utc> {
        let text = format!("<task-notification> <tool-use-id>{call}</tool-use-id> <status>completed</status> </task-notification>");
        self.message(json!({"type": "user", "origin": {"kind": "task-notification"}, "message": {"role": "user", "content": text}})).await
    }

    async fn turn_end(&mut self) -> DateTime<Utc> {
        self.event(json!({"type": "system", "subtype": "turn_duration", "durationMs": 5})).await
    }
}

/// A record id's UUID, as text.
fn superx_ops_uuid(id: &RecordId) -> String {
    match &id.key {
        superx_kernel::types::RecordIdKey::Uuid(u) => u.to_string(),
        other => panic!("a UUID key, not {other:?}"),
    }
}

/// The parameters, with these set; every test sets the projects roots on
/// the adapter, never reading the environment's HOME.
fn with(set: &[(&'static str, Value)]) -> Params {
    params(set)
}

fn list(items: &[&str]) -> Value {
    Value::Array(items.iter().map(|s| Value::String((*s).to_string())).collect::<Vec<_>>().into())
}

/// The adapter's projects roots, set as the operator sets them.
async fn roots(kernel: &Kernel, roots: &[&Path]) {
    let adapter = kernel.register_module(&ClaudeCodeAdapter.descriptor()).await.expect("adapter");
    kernel.set_parameter(adapter, PROJECTS_ROOT_PARAM, strings(roots)).await.expect("roots");
}

/// A scratch folder with a projects root, a project, its transcript and its
/// checkout, by their real paths.
struct Place {
    dir: PathBuf,
    root: PathBuf,
    transcript: PathBuf,
    checkout: PathBuf,
}

fn place() -> Place {
    let dir = scratch();
    let root = dir.join("projects");
    let project = root.join("-work-checkout");
    std::fs::create_dir_all(project.join("memory")).expect("project");
    let transcript = project.join("sess.jsonl");
    std::fs::write(&transcript, "").expect("transcript");
    let checkout = dir.join("checkout");
    std::fs::create_dir_all(&checkout).expect("checkout");
    Place { dir, root, transcript, checkout }
}

async fn setup(key: &str) -> (Kernel, Place, Agent, DateTime<Utc>) {
    let kernel = kernel().await;
    let place = place();
    roots(&kernel, &[place.root.as_path()]).await;
    let t0 = Utc::now() - TimeDelta::hours(2);
    let agent = Agent::new(&kernel, key, &place.transcript, &place.checkout, t0).await;
    (kernel, place, agent, t0)
}

async fn look(watcher: &mut Watcher, kernel: &Kernel, params: &Params, now: DateTime<Utc>) -> Snapshot {
    watcher.look(kernel, params, now).await.expect("a look");
    watcher.snapshot(params, now, false)
}

fn kinds(s: &Snapshot) -> Vec<Kind> {
    let mut k: Vec<Kind> = s.reports.iter().map(|r| r.kind).collect();
    k.reverse();
    k
}

fn of(s: &Snapshot, kind: Kind) -> Vec<&superx_mod_recon::watcher::report::Report> {
    s.reports.iter().filter(|r| r.kind == kind).collect()
}

// Step 1 --------------------------------------------------------------------

#[test]
fn no_file_under_src_watcher_names_crate_crawler_or_crate_results_none_under_src_crawler_or_src_results_names_crate_watcher_and_every_query_under_src_watcher_is_a_select(
) {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let files = |dir: &Path| -> Vec<(PathBuf, String)> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).expect("a folder") {
                let p = e.expect("an entry").path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    let text = std::fs::read_to_string(&p).expect("a file");
                    out.push((p, text));
                }
            }
        }
        out
    };
    let watcher = files(&src.join("watcher"));
    assert!(watcher.len() >= 5, "the watcher's own files: {}", watcher.len());
    for (p, text) in &watcher {
        assert!(!text.contains("crate::crawler") && !text.contains("crate::results"), "{} reaches the crawler", p.display());
        for verb in ["CREATE ", "UPDATE ", "UPSERT ", "DELETE ", "INSERT ", "RELATE ", "DEFINE ", "REMOVE ", "ALTER "] {
            assert!(!text.contains(verb), "{} writes with {verb}", p.display());
        }
        for q in text.match_indices("FROM ").map(|(i, _)| &text[..i]) {
            let stmt = q.rsplit('"').next().unwrap_or_default();
            assert!(stmt.trim_start().starts_with("SELECT"), "{}: a query that is no SELECT: {stmt}", p.display());
        }
    }
    for part in ["crawler", "results"] {
        for (p, text) in files(&src.join(part)) {
            assert!(!text.contains("crate::watcher"), "{} reaches the watcher", p.display());
        }
    }
}

// Step 2 --------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_look_reads_the_lines_captured_since_the_look_before_judging_at_the_time_it_is_given_with_attr_recon_watch_secs_0_the_loop_reads_nothing_and_set_again_it_reads_every_line_since_its_last_look_a_stop_ends_it_in_its_wait_and_in_a_look(
) {
    let (kernel, _place, mut agent, t0) = setup("loop").await;
    let p = with(&[]);
    let mut watcher = Watcher::new(Utc::now(), None);
    agent.typed("build the thing").await;
    let first = look(&mut watcher, &kernel, &p, t0 + TimeDelta::minutes(1)).await;
    assert_eq!(first.lines_read, 1, "the line captured before the first look");
    assert_eq!(first.looked_at, Some(t0 + TimeDelta::minutes(1)), "a look judges at the time it is given");
    agent.said("on it").await;
    agent.said("done").await;
    let second = look(&mut watcher, &kernel, &p, t0 + TimeDelta::minutes(2)).await;
    assert_eq!(second.lines_read, 3, "the two since the look before, and none twice");

    // The loop, paused: it reads nothing and says so.
    let entity = registry::starting(&kernel).await.expect("registered");
    kernel.set_parameter(entity.clone(), params::WATCH_SECS_PARAM, number(0)).await.expect("paused");
    let watched = Watched::default();
    let (asked, stopped) = tokio::sync::watch::channel(false);
    let looping = tokio::spawn(superx_mod_recon::watcher::run(kernel.clone(), stopped, watched.clone()));
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    let paused = watched.snapshot();
    assert!(paused.paused && paused.lines_read == 0, "paused, nothing read: {paused:?}");
    asked.send(true).expect("stop");
    tokio::time::timeout(std::time::Duration::from_secs(2), looping).await.expect("a stop ends it in its wait").expect("ended");

    // Set again, a watcher that has not looked for a while reads every line
    // captured since its last look.
    agent.said("one").await;
    agent.said("two").await;
    let third = look(&mut watcher, &kernel, &p, t0 + TimeDelta::minutes(30)).await;
    assert_eq!(third.lines_read, 5, "every line since its last look");

    // A stop ends it in a look: asked for at once, the loop ends at once.
    kernel.set_parameter(entity, params::WATCH_SECS_PARAM, number(3600)).await.expect("a long interval");
    let (asked, stopped) = tokio::sync::watch::channel(false);
    let looping = tokio::spawn(superx_mod_recon::watcher::run(kernel.clone(), stopped, Watched::default()));
    asked.send(true).expect("stop");
    tokio::time::timeout(std::time::Duration::from_secs(2), looping).await.expect("a stop ends it in a look").expect("ended");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_look_that_cannot_read_changes_nothing_the_watcher_holds_and_the_next_look_that_reads_takes_every_line_since_the_last_look_that_read(
) {
    let (kernel, _place, mut agent, t0) = setup("outage").await;
    let p = with(&[]);
    let mut watcher = Watcher::new(Utc::now(), None);
    agent.typed("first").await;
    let before = look(&mut watcher, &kernel, &p, t0 + TimeDelta::minutes(1)).await;
    agent.said("second").await;
    kernel.db().use_ns("superx").use_db("nowhere").await.expect("moved");
    let failed = watcher.look(&kernel, &p, t0 + TimeDelta::minutes(2)).await;
    assert!(failed.is_err(), "the look cannot read");
    assert_eq!(watcher.snapshot(&p, t0 + TimeDelta::minutes(2), false).lines_read, before.lines_read, "nothing changed");
    kernel.db().use_ns("superx").use_db("kernel").await.expect("back");
    sign_in(&kernel).await;
    let after = look(&mut watcher, &kernel, &p, t0 + TimeDelta::minutes(3)).await;
    assert_eq!(after.lines_read, 2, "the line written while it could not read is read now");
}

// Step 3 --------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn at_its_start_the_watcher_reads_back_the_window_and_no_line_before_it_the_lines_of_both_tables_come_once_each_in_the_order_of_their_ids_across_pages_and_looks_a_line_committed_after_a_newer_one_was_read_is_read_at_the_next_look(
) {
    let (kernel, _place, mut agent, t0) = setup("window").await;
    let p = with(&[(params::WATCH_WINDOW_SECS_PARAM, number(3600))]);
    agent.at = t0 - TimeDelta::hours(5);
    agent.typed("long before the window").await;
    agent.at = t0;
    // More lines than a page of either table.
    for i in 0..520 {
        agent.said(&format!("word {i}")).await;
    }
    for _ in 0..3 {
        agent.turn_end().await;
    }
    let started = t0 + TimeDelta::minutes(30);
    let mut watcher = Watcher::new(started, None);
    let s = look(&mut watcher, &kernel, &p, started).await;
    assert_eq!(s.lines_read, 523, "every line in the window, both tables, none before it");
    let again = look(&mut watcher, &kernel, &p, started).await;
    assert_eq!(again.lines_read, 523, "a look again reads none twice");

    // A line committed now with an id older than the newest read, within
    // the overlap, is read at the next look.
    let late = agent.at - TimeDelta::seconds(20);
    kernel
        .db()
        .query("CREATE $id CONTENT { session: $sess, agent: $agent, role: 'x', content: '', raw_text: $text, emitted_at: $at, valid_from: $at }")
        .bind(("id", id_at("message", late)))
        .bind(("sess", agent.session.clone()))
        .bind(("agent", agent.agent.clone()))
        .bind(("text", json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "late"}]}, "timestamp": late.to_rfc3339()}).to_string()))
        .bind(("at", late))
        .await
        .expect("late")
        .check()
        .expect("late taken");
    let s = look(&mut watcher, &kernel, &p, started).await;
    assert_eq!(s.lines_read, 524, "the late line, once");
    let session = &s.sessions[0];
    assert_eq!(session.said.as_ref().map(|t| t.text.as_str()), Some("word 519"), "the newest words stay the newest: {session:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_row_is_read_from_its_raw_text_else_from_its_parsed_copy_a_transcript_event_of_kind_file_is_no_line_a_row_with_no_line_with_raw_text_that_is_not_json_with_a_session_the_names_do_not_give_or_of_another_agent_is_counted_by_why_and_by_agent_and_judged_nowhere(
) {
    let (kernel, _place, mut agent, t0) = setup("rows").await;
    let p = with(&[]);
    agent.typed("from its text").await;
    let at = agent.at + TimeDelta::seconds(1);
    let write = |sql: &'static str, id: RecordId, text: Option<String>, raw: Option<Json>, sess: RecordId, who: RecordId| {
        let kernel = kernel.clone();
        async move {
            kernel
                .db()
                .query(sql)
                .bind(("id", id))
                .bind(("sess", sess))
                .bind(("agent", who))
                .bind(("text", text))
                .bind(("raw", raw.map(|r| Value::Object(superx_kernel::message::json_to_object(&r)))))
                .bind(("at", at))
                .await
                .expect("row")
                .check()
                .expect("row taken");
        }
    };
    const ROW: &str = "CREATE $id CONTENT { session: $sess, agent: $agent, role: 'x', content: '', raw_text: $text, raw: $raw, emitted_at: $at, valid_from: $at }";
    // Its parsed copy alone.
    write(ROW, id_at("message", at), None, Some(json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "from its copy"}]}})), agent.session.clone(), agent.agent.clone()).await;
    // Raw text that is not JSON, and a row with neither.
    write(ROW, id_at("message", at + TimeDelta::seconds(1)), Some("not json {".to_string()), None, agent.session.clone(), agent.agent.clone()).await;
    write(ROW, id_at("message", at + TimeDelta::seconds(2)), None, None, agent.session.clone(), agent.agent.clone()).await;
    // A session the names do not give.
    let stranger = kernel.create_entity("node_session").await.expect("a session with no name");
    write(ROW, id_at("message", at + TimeDelta::seconds(3)), Some("{}".to_string()), None, stranger, agent.agent.clone()).await;
    // Another agent's line.
    let gemini = kernel.create_entity("node_agent").await.expect("agent");
    let mut d = Object::new();
    d.insert("name".to_string(), Value::String("gemini_cli".to_string()));
    kernel.supersede_state(gemini.clone(), "attr_agent_descriptor", Value::Object(d)).await.expect("named");
    write(ROW, id_at("message", at + TimeDelta::seconds(4)), Some(json!({"type": "user"}).to_string()), None, agent.session.clone(), gemini).await;
    // A file beside the transcripts.
    kernel
        .db()
        .query("CREATE $id CONTENT { lifecycle_event: 'transcript_event', payload: { session: $key, kind: 'file', file: '/x/memory/MEMORY.md', line: { type: 'file' } }, agent: $agent, valid_from: $at }")
        .bind(("id", id_at("telemetry_stream", at + TimeDelta::seconds(5))))
        .bind(("key", agent.key.clone()))
        .bind(("agent", agent.agent.clone()))
        .bind(("at", at + TimeDelta::seconds(5)))
        .await
        .expect("file")
        .check()
        .expect("file taken");
    let mut watcher = Watcher::new(Utc::now(), None);
    let s = look(&mut watcher, &kernel, &p, t0 + TimeDelta::minutes(5)).await;
    assert_eq!(s.lines_read, 2, "the line from its text and the one from its copy");
    let said = s.sessions[0].said.as_ref().map(|t| t.text.clone());
    assert_eq!(said.as_deref(), Some("from its copy"), "a row with no text is read from its copy");
    let unread: Vec<(String, String, u64)> =
        s.unread.iter().map(|u| (format!("{:?}", u.why), u.agent.clone(), u.rows)).collect();
    assert_eq!(
        unread,
        [
            ("NoLine".to_string(), "claude_code".to_string(), 2),
            ("NoSession".to_string(), "claude_code".to_string(), 1),
            ("Agent".to_string(), "gemini_cli".to_string(), 1),
        ],
        "counted by why and by agent; the file is no line"
    );
    assert!(s.reports.is_empty(), "judged nowhere");
}

// Step 4 --------------------------------------------------------------------

#[test]
fn each_user_attachment_assistant_and_system_line_gives_what_step_4_reads_typed_queued_interrupt_shell_and_slash_messages_no_message_from_a_sidechain_meta_or_compact_summary_line_or_from_text_that_opens_with_a_tag_a_task_notification_ends_its_job_a_failed_result_with_its_error_whole_and_the_lines_a_result_wrote_words_from_the_main_thread_alone_calls_with_what_input_background_and_the_paths_they_write_a_bash_redirects_among_them_a_turn_end_a_compaction_and_a_recap_whole(
) {
    let home = Path::new("/home/op");
    let read = |j: Json| line::read(&j, Some(home)).happened;
    let prompt = |came, text: &str| vec![Happened::Prompt { came, text: text.to_string() }];
    assert_eq!(read(json!({"type": "user", "origin": {"kind": "human"}, "message": {"content": "go"}})), prompt(Came::Typed, "go"));
    assert_eq!(read(json!({"type": "user", "message": {"content": "no origin, plain"}})), prompt(Came::Typed, "no origin, plain"));
    assert_eq!(
        read(json!({"type": "attachment", "attachment": {"type": "queued_command", "prompt": "later", "origin": {"kind": "human"}}})),
        prompt(Came::Queued, "later")
    );
    assert_eq!(read(json!({"type": "user", "message": {"content": [{"type": "text", "text": "[Request interrupted by user]"}]}})), prompt(Came::Interrupt, "[Request interrupted by user]"));
    assert_eq!(read(json!({"type": "user", "origin": {"kind": "human"}, "message": {"content": "<bash-input>ls</bash-input>"}})), prompt(Came::Shell, "<bash-input>ls</bash-input>"));
    assert_eq!(read(json!({"type": "user", "origin": {"kind": "human"}, "message": {"content": "<command-name>/compact</command-name>"}})), prompt(Came::Command, "<command-name>/compact</command-name>"));
    for quiet in [
        json!({"type": "user", "isSidechain": true, "message": {"content": "a subagent's prompt"}}),
        json!({"type": "user", "isMeta": true, "message": {"content": "meta"}}),
        json!({"type": "user", "isCompactSummary": true, "message": {"content": "summary"}}),
        json!({"type": "user", "message": {"content": "<system-reminder>x</system-reminder>"}}),
    ] {
        assert!(read(quiet.clone()).is_empty(), "no one's message: {quiet}");
    }
    assert_eq!(
        read(json!({"type": "user", "origin": {"kind": "task-notification"}, "message": {"content": "<task-notification> <tool-use-id>toolu_9</tool-use-id> <status>completed</status>"}})),
        vec![Happened::JobEnded { call: Some("toolu_9".to_string()), status: Some("completed".to_string()) }]
    );
    let error = "Exit code 1\nthe whole error, every line of it";
    assert_eq!(
        read(json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "t1", "is_error": true, "content": error}]}, "toolUseResult": {"structuredPatch": [{"lines": ["+a", "+b", "-c", " d"]}]}})),
        vec![Happened::Result { call: "t1".to_string(), failed: true, error: Some(error.to_string()), lines: 2 }]
    );
    assert!(read(json!({"type": "assistant", "isSidechain": true, "message": {"content": [{"type": "text", "text": "a subagent's words"}]}})).is_empty());
    let reply = line::read(
        &json!({"type": "assistant", "cwd": "/work/repo", "message": {"model": "claude-opus-5-5", "content": [
            {"type": "text", "text": "Next: the review."},
            {"type": "tool_use", "id": "a", "name": "Bash", "input": {"command": "python3 fetch.py", "run_in_background": true}},
            {"type": "tool_use", "id": "b", "name": "Edit", "input": {"file_path": "/work/repo/src/x.rs", "old_string": "a", "new_string": "b"}},
            {"type": "tool_use", "id": "c", "name": "Bash", "input": {"command": "echo rule >> ~/.claude/projects/-w/memory/r.md && cat a 2>&1 | tee -a log/out.txt"}},
            {"type": "tool_use", "id": "d", "name": "Bash", "input": {"command": "sed -i '' 's/a/b/' src/y.rs"}}
        ]}}),
        Some(home),
    );
    assert_eq!(reply.model.as_deref(), Some("claude-opus-5-5"));
    assert_eq!(reply.happened[0], Happened::Said { text: "Next: the review.".to_string() });
    let writes = |i: usize| match &reply.happened[i] {
        Happened::Call { writes, .. } => writes.clone(),
        other => panic!("a call: {other:?}"),
    };
    assert!(matches!(&reply.happened[1], Happened::Call { background: true, what, input, .. } if what == "python3 fetch.py" && input["run_in_background"] == true));
    assert_eq!(writes(2), [PathBuf::from("/work/repo/src/x.rs")]);
    assert_eq!(writes(3), [PathBuf::from("/home/op/.claude/projects/-w/memory/r.md"), PathBuf::from("/work/repo/log/out.txt")], "a redirect and a tee, 2>&1 aside");
    assert_eq!(writes(4), [PathBuf::from("/work/repo/src/y.rs")], "sed -i's file, its script aside");
    assert_eq!(read(json!({"type": "system", "subtype": "turn_duration"})), vec![Happened::TurnEnded]);
    assert_eq!(read(json!({"type": "system", "subtype": "compact_boundary"})), vec![Happened::Compacted]);
    assert_eq!(read(json!({"type": "system", "subtype": "away_summary", "content": "the whole recap"})), vec![Happened::Recap { text: "the whole recap".to_string() }]);
}

// Step 5 --------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_watch_per_session_holds_its_lines_readings_whole_a_line_older_than_the_window_leaves_with_its_reports_while_the_orders_in_force_and_the_task_file_stay_a_watch_with_no_line_leaves_with_attr_recon_watch_sessions_set_the_most_recently_active_are_kept_and_at_its_fallback_all_are(
) {
    let (kernel, place, mut a, t0) = setup("one").await;
    let p = with(&[(params::WATCH_WINDOW_SECS_PARAM, number(3600))]);
    let long = "x".repeat(70_000);
    a.typed(&format!("ship it {long}")).await;
    a.typed("what is going on").await;
    let mut b = Agent::new(&kernel, "two", &place.transcript, &place.checkout, t0 + TimeDelta::minutes(30)).await;
    b.said("hello").await;
    let mut watcher = Watcher::new(t0 + TimeDelta::minutes(31), None);
    let s = look(&mut watcher, &kernel, &p, t0 + TimeDelta::minutes(31)).await;
    assert_eq!(s.sessions.len(), 2, "one watch per session");
    let first = s.sessions.iter().find(|w| w.id == a.id()).expect("watched");
    assert_eq!(first.orders.as_deref().map(str::len), Some(format!("ship it {long}").len()), "whole");
    assert_eq!(of(&s, Kind::StatusAsked).len(), 1);

    // An hour on: the first session's lines leave, its report with them, its
    // orders stay; the second's stay.
    let later = look(&mut watcher, &kernel, &p, t0 + TimeDelta::minutes(61)).await;
    assert!(later.sessions.iter().all(|w| w.id != a.id()), "a watch with no line leaves");
    assert!(of(&later, Kind::StatusAsked).is_empty(), "its report left with its line");
    a.at = t0 + TimeDelta::minutes(62);
    a.said("still here").await;
    let back = look(&mut watcher, &kernel, &p, t0 + TimeDelta::minutes(63)).await;
    let first = back.sessions.iter().find(|w| w.id == a.id()).expect("watched again");
    assert!(first.orders.is_none(), "a watch that left starts anew");

    // Orders stay while the watch holds any line.
    let mut c = Agent::new(&kernel, "three", &place.transcript, &place.checkout, t0 + TimeDelta::minutes(64)).await;
    c.typed("deploy the build").await;
    c.at = t0 + TimeDelta::minutes(130);
    c.said("working").await;
    let s = look(&mut watcher, &kernel, &p, t0 + TimeDelta::minutes(131)).await;
    let third = s.sessions.iter().find(|w| w.id == c.id()).expect("watched");
    assert_eq!(third.orders.as_deref(), Some("deploy the build"), "the orders stay though their message left the window");

    // A limit keeps the most recently active; at its fallback, all.
    let held = with(&[(params::WATCH_WINDOW_SECS_PARAM, number(36_000)), (params::WATCH_SESSIONS_PARAM, number(1))]);
    let mut limited = Watcher::new(Utc::now(), None);
    let s = look(&mut limited, &kernel, &held, t0 + TimeDelta::minutes(131)).await;
    assert_eq!(s.sessions.iter().map(|w| w.id.clone()).collect::<Vec<_>>(), [c.id()], "the most recently active");
    assert_eq!(params(&[]).watch_sessions, ALL);
    let mut all = Watcher::new(Utc::now(), None);
    let s = look(&mut all, &kernel, &with(&[(params::WATCH_WINDOW_SECS_PARAM, number(36_000))]), t0 + TimeDelta::minutes(131)).await;
    assert_eq!(s.sessions.len(), 3, "at its fallback, all");
    std::fs::remove_dir_all(&place.dir).ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sessions_state_is_stalled_working_behind_a_job_waiting_on_you_or_idle_as_step_5_gives_it() {
    let (kernel, _place, mut a, t0) = setup("states").await;
    let p = with(&[]);
    let mut watcher = Watcher::new(Utc::now(), None);
    let state = |s: &Snapshot| s.sessions[0].state;
    a.typed("go").await;
    let s = look(&mut watcher, &kernel, &p, a.at + TimeDelta::seconds(5)).await;
    assert_eq!(state(&s), "working");
    let s = look(&mut watcher, &kernel, &p, a.at + TimeDelta::minutes(11)).await;
    assert_eq!(state(&s), "stalled", "mid-turn, no call running, no line for attr_recon_watch_quiet_secs");
    a.call("long", "Bash", json!({"command": "cargo build"})).await;
    let s = look(&mut watcher, &kernel, &p, a.at + TimeDelta::minutes(30)).await;
    assert_eq!(state(&s), "working", "a call running is no stall");
    a.result("long", false, "built").await;
    a.call("job", "Bash", json!({"command": "watch logs", "run_in_background": true})).await;
    a.result("job", false, "running in the background").await;
    a.turn_end().await;
    let s = look(&mut watcher, &kernel, &p, a.at + TimeDelta::seconds(5)).await;
    assert_eq!(state(&s), "behind a job");
    a.notified("job").await;
    a.said("should I push?").await;
    a.turn_end().await;
    let s = look(&mut watcher, &kernel, &p, a.at + TimeDelta::seconds(5)).await;
    assert_eq!(state(&s), "waiting on you");
    a.typed("no").await;
    a.said("ok.").await;
    a.turn_end().await;
    let s = look(&mut watcher, &kernel, &p, a.at + TimeDelta::seconds(5)).await;
    assert_eq!(state(&s), "idle");
    a.typed("go again").await;
    a.interrupt().await;
    let s = look(&mut watcher, &kernel, &p, a.at + TimeDelta::minutes(11)).await;
    assert_eq!(state(&s), "idle", "an interrupt ends the turn: no stall after it");
    let _ = t0;
}

// Step 6 --------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn messages_are_classed_stop_status_ask_question_approval_and_order_in_that_precedence_by_whole_words_and_phrases_an_interrupt_a_stop_corrections_and_escalations_are_marked_shell_and_slash_commands_are_neither_classed_nor_marked(
) {
    let p = params(&[]);
    let c = |t: &str| words::class(t, false, &p);
    assert_eq!(c("stop, what is going on?"), Class::Stop, "a stop first");
    assert_eq!(c("what's going on with the build"), Class::StatusAsk, "a status phrase before a question");
    assert_eq!(c("why did the gate fail?"), Class::Question);
    assert_eq!(c("did it pass"), Class::Question, "a question word first");
    assert_eq!(c("yes do those and consider all"), Class::Approval);
    assert_eq!(c("do it"), Class::Approval, "a phrase");
    assert_eq!(c("fix the walk and push"), Class::Order);
    assert_eq!(c("the statusbar is wrong"), Class::Order, "whole words: statusbar is no status");
    assert_eq!(words::class("[Request interrupted by user]", true, &p), Class::Stop, "an interrupt is a stop");
    assert!(words::corrects("i told you to stop", &p) && !words::corrects("fine", &p));
    assert!(words::escalates("fucken fix it", &p) && !words::escalates("please fix it", &p));

    let (kernel, _place, mut a, t0) = setup("class").await;
    a.typed("<bash-input>git status</bash-input>").await;
    let mut watcher = Watcher::new(Utc::now(), None);
    let s = look(&mut watcher, &kernel, &p, t0 + TimeDelta::minutes(1)).await;
    let m = s.sessions[0].message.as_ref().expect("the shell command, kept");
    assert_eq!((m.came, m.class, m.correction, m.escalation), (Came::Shell, None, false, false), "{m:?}");
}

// Step 7 --------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_orders_in_force_are_the_newest_orders_words_or_an_approvals_with_the_proposal_before_it_unchanged_by_a_stop_a_status_ask_or_a_question_joined_by_the_task_files_words_kept_when_their_message_leaves_the_window_with_no_order_known_nothing_is_judged_unordered_and_the_session_says_so(
) {
    let (kernel, place, mut a, t0) = setup("orders").await;
    let p = with(&[]);
    let mut watcher = Watcher::new(Utc::now(), None);
    a.bash("x1", "git push origin main").await;
    let s = look(&mut watcher, &kernel, &p, t0 + TimeDelta::minutes(1)).await;
    assert!(s.sessions[0].orders.is_none() && of(&s, Kind::UnorderedAction).is_empty(), "no order known, nothing unordered");
    a.typed("fix the parser").await;
    a.said("I'd fix it, then commit and push the branch.").await;
    a.typed("yes").await;
    a.typed("why is it slow?").await;
    a.typed("what is going on").await;
    a.typed("stop").await;
    let s = look(&mut watcher, &kernel, &p, t0 + TimeDelta::minutes(2)).await;
    let orders = s.sessions[0].orders.clone().expect("orders");
    assert!(orders.contains("commit and push the branch") && orders.ends_with("yes"), "the approval with its proposal: {orders}");
    assert!(!orders.contains("why is it slow"), "a question, a status ask and a stop leave them");

    // A task file's words join them.
    std::fs::create_dir_all(place.checkout.join("specs/recon/tasks")).expect("specs");
    std::fs::write(
        place.checkout.join("specs/recon/tasks/9.xml"),
        r#"<task id="t9" version="1"><start>build it; deploy it</start><owns>src/**</owns><forbid>specs/**</forbid><done>done</done></task>"#,
    )
    .expect("task file");
    a.typed("specs/recon/tasks/9.xml").await;
    a.bash("x2", "tools/deploy.sh").await;
    let s = look(&mut watcher, &kernel, &p, t0 + TimeDelta::minutes(3)).await;
    assert!(of(&s, Kind::UnorderedAction).is_empty(), "the task file's words order the deploy: {:?}", kinds(&s));
    std::fs::remove_dir_all(&place.dir).ok();
}

// Step 8 --------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn quiet_fires_at_the_first_call_made_attr_recon_watch_quiet_secs_or_more_after_the_agents_last_words_once_attr_recon_watch_quiet_calls_calls_are_made_since_once_until_it_speaks_behind_a_job_at_the_turn_end_plus_attr_recon_watch_behind_secs_stalled_at_the_newest_line_plus_attr_recon_watch_quiet_secs_mid_turn_with_no_call_running_and_never_while_a_call_runs(
) {
    let (kernel, _place, mut a, t0) = setup("vanish").await;
    let p = with(&[(params::WATCH_QUIET_SECS_PARAM, number(60)), (params::WATCH_QUIET_CALLS_PARAM, number(3))]);
    let mut watcher = Watcher::new(Utc::now(), None);
    a.typed("go").await;
    for i in 0..3 {
        a.bash(&format!("q{i}"), "ls").await;
    }
    a.at += TimeDelta::seconds(60);
    let fired = a.bash("q3", "ls").await;
    a.bash("q4", "ls").await;
    let s = look(&mut watcher, &kernel, &p, a.at + TimeDelta::seconds(1)).await;
    let quiet = of(&s, Kind::Quiet);
    assert_eq!(quiet.len(), 1, "once until it speaks: {:?}", kinds(&s));
    assert_eq!((quiet[0].at, quiet[0].level), (fired, Level::Warn), "at the call that made it so");

    a.call("job", "Bash", json!({"command": "fetch logs", "run_in_background": true})).await;
    a.result("job", false, "started").await;
    let end = a.turn_end().await;
    let s = look(&mut watcher, &kernel, &p, end + TimeDelta::seconds(299)).await;
    assert!(of(&s, Kind::BehindAJob).is_empty(), "not yet");
    let s = look(&mut watcher, &kernel, &p, end + TimeDelta::seconds(300)).await;
    let behind = of(&s, Kind::BehindAJob);
    assert_eq!((behind.len(), behind[0].at), (1, end + TimeDelta::seconds(300)), "at the turn end plus attr_recon_watch_behind_secs");

    a.typed("go on").await;
    a.call("run", "Bash", json!({"command": "cargo test"})).await;
    let s = look(&mut watcher, &kernel, &p, a.at + TimeDelta::minutes(30)).await;
    assert!(of(&s, Kind::Stalled).is_empty(), "never while a call runs");
    let newest = a.result("run", false, "ok").await;
    let s = look(&mut watcher, &kernel, &p, newest + TimeDelta::seconds(60)).await;
    let stalled = of(&s, Kind::Stalled);
    assert_eq!((stalled.len(), stalled[0].at, stalled[0].level), (1, newest + TimeDelta::seconds(60), Level::Alert));
    let _ = t0;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_question_or_a_status_ask_whose_turn_ends_without_the_agents_words_is_unanswered_each_status_ask_is_reported_with_the_message_whole_a_turn_ending_on_the_agents_question_is_waiting_on_you(
) {
    let (kernel, _place, mut a, t0) = setup("asked").await;
    let p = with(&[]);
    let mut watcher = Watcher::new(Utc::now(), None);
    a.typed("why did it fail?").await;
    a.bash("b1", "ls").await;
    let end = a.turn_end().await;
    let status = "status? what's going on with all of it, every word kept";
    a.typed(status).await;
    a.said("Building; about ten minutes. Shall I push when green?").await;
    let asked = a.turn_end().await;
    let s = look(&mut watcher, &kernel, &p, t0 + TimeDelta::minutes(5)).await;
    let unanswered = of(&s, Kind::QuestionUnanswered);
    assert_eq!((unanswered.len(), unanswered[0].at), (1, end), "only the question left without words");
    let st = of(&s, Kind::StatusAsked);
    assert_eq!((st.len(), st[0].level, st[0].words[0].text.as_str()), (1, Level::Alert, status), "the message whole");
    let waiting = of(&s, Kind::WaitingOnYou);
    assert_eq!((waiting.len(), waiting[0].at, waiting[0].level), (1, asked, Level::Info));
}

// Step 9 --------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_act_after_a_question_and_before_the_agents_words_is_reported_once_and_none_after_it_answered_an_effect_none_of_whose_words_the_orders_in_force_hold_is_an_unordered_action_naming_its_patterns_and_one_whose_words_they_hold_is_not(
) {
    let (kernel, _place, mut a, t0) = setup("acts").await;
    let p = with(&[]);
    let mut watcher = Watcher::new(Utc::now(), None);
    a.typed("fix the walk").await;
    a.typed("is the walk bounded?").await;
    a.call("e1", "Edit", json!({"file_path": "/w/src/a.rs", "old_string": "a", "new_string": "b"})).await;
    a.call("e2", "Edit", json!({"file_path": "/w/src/b.rs", "old_string": "a", "new_string": "b"})).await;
    a.said("Yes, bounded.").await;
    a.call("e3", "Edit", json!({"file_path": "/w/src/c.rs", "old_string": "a", "new_string": "b"})).await;
    a.bash("p1", "git push --force origin main").await;
    a.bash("c1", "git commit -m fix").await;
    let s = look(&mut watcher, &kernel, &p, t0 + TimeDelta::minutes(5)).await;
    assert_eq!(of(&s, Kind::ActedOnQuestion).len(), 1, "once, and none after it answered: {:?}", kinds(&s));
    let unordered = of(&s, Kind::UnorderedAction);
    assert_eq!(unordered.len(), 2, "the push and the commit: {unordered:?}");
    assert!(unordered.iter().any(|r| r.what.contains("git push") && r.what.contains("git push --force")), "naming its patterns");
    a.typed("commit and push it").await;
    a.bash("c2", "git commit -m again").await;
    a.bash("p2", "git push origin main").await;
    let s = look(&mut watcher, &kernel, &p, t0 + TimeDelta::minutes(6)).await;
    assert_eq!(of(&s, Kind::UnorderedAction).len(), 2, "ordered now, nothing more");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scope_words_the_orders_in_force_do_not_hold_are_unordered_work_calls_after_a_stop_reaching_attr_recon_watch_stop_calls_before_a_word_are_a_stop_not_honoured_a_message_sharing_attr_recon_watch_repeat_share_percent_of_the_shorter_ones_words_with_one_sent_within_attr_recon_watch_repeat_secs_is_a_repeated_order_and_one_with_fewer_than_attr_recon_watch_order_words_words_is_not(
) {
    let (kernel, _place, mut a, t0) = setup("follow").await;
    let p = with(&[]);
    let mut watcher = Watcher::new(Utc::now(), None);
    a.typed("approve the scratch copy only").await;
    a.said("Next: the review of stage four, then a rehearsal.").await;
    a.typed("stop").await;
    a.bash("s1", "ls").await;
    let second = a.bash("s2", "ls").await;
    a.bash("s3", "ls").await;
    a.typed("drop and recreate the table now").await;
    let again = a.queued("drop and recreate the table now please").await;
    a.typed("ok").await;
    a.typed("ok").await;
    let s = look(&mut watcher, &kernel, &p, t0 + TimeDelta::minutes(5)).await;
    let work = of(&s, Kind::UnorderedWork);
    assert!(work.len() == 1 && work[0].what.contains("review") && work[0].what.contains("rehearsal"), "{work:?}");
    let stop = of(&s, Kind::StopNotHonoured);
    assert_eq!((stop.len(), stop[0].at), (1, second), "at the call that made the count");
    let repeated = of(&s, Kind::RepeatedOrder);
    assert_eq!((repeated.len(), repeated[0].at), (1, again), "the order said again; two short oks are not");
}

// Step 10 -------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_long_estimate_a_long_run_with_its_calls_by_tool_and_the_lines_written_a_struggle_at_attr_recon_watch_failures_failures_in_a_row_with_each_error_whole_ended_by_a_success(
) {
    let (kernel, _place, mut a, t0) = setup("trouble").await;
    let p = with(&[(params::WATCH_RUN_SECS_PARAM, number(60)), (params::WATCH_RUN_CALLS_PARAM, number(3))]);
    let mut watcher = Watcher::new(Utc::now(), None);
    a.typed("build stage four").await;
    a.said("It takes a few hours.").await;
    a.call("w1", "Write", json!({"file_path": "/w/a.rs", "content": "x"})).await;
    a.message(json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "w1", "content": "ok"}]}, "toolUseResult": {"type": "create", "content": "1\n2\n3\n"}})).await;
    a.at += TimeDelta::seconds(60);
    a.bash("b1", "cargo test").await;
    let run = a.bash("b2", "cargo test").await;
    for (i, e) in ["error one\nwhole", "error two", "error three"].iter().enumerate() {
        a.call(&format!("f{i}"), "Bash", json!({"command": "cargo build"})).await;
        a.result(&format!("f{i}"), true, e).await;
    }
    a.bash("ok", "true").await;
    a.call("f9", "Bash", json!({"command": "x"})).await;
    a.result("f9", true, "one more").await;
    let s = look(&mut watcher, &kernel, &p, t0 + TimeDelta::minutes(10)).await;
    assert_eq!(of(&s, Kind::LongEstimate).len(), 1);
    let long = of(&s, Kind::LongRun);
    assert_eq!((long.len(), long[0].at), (1, run), "{long:?}");
    assert!(long[0].what.contains("Bash 2") && long[0].what.contains("Write 1") && long[0].what.contains("3 lines written"), "{}", long[0].what);
    let struggle = of(&s, Kind::Struggling);
    assert_eq!(struggle.len(), 1, "once a streak; a success ends it");
    let errors: Vec<&str> = struggle[0].words.iter().map(|w| w.text.as_str()).collect();
    assert_eq!(errors, ["error one\nwhole", "error two", "error three"], "each error whole");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_memory_write_by_edit_by_write_and_by_a_bash_redirect_a_compaction_frustration_when_more_than_half_of_the_newest_attr_recon_watch_mood_prompts_messages_escalate_and_again_as_more_do(
) {
    let (kernel, place, mut a, t0) = setup("memory").await;
    let p = with(&[]);
    let mut watcher = Watcher::new(Utc::now(), None);
    let memory = place.root.join("-work-checkout/memory");
    a.typed("note it").await;
    a.call("m1", "Edit", json!({"file_path": memory.join("a.md"), "old_string": "x", "new_string": "y"})).await;
    a.call("m2", "Write", json!({"file_path": memory.join("b.md"), "content": "rule"})).await;
    a.call("m3", "Bash", json!({"command": format!("echo rule >> {}", memory.join("c.md").display())})).await;
    a.call("m4", "Write", json!({"file_path": place.checkout.join("not-memory.md"), "content": "x"})).await;
    a.event(json!({"type": "system", "subtype": "compact_boundary"})).await;
    for m in ["fix it", "fucken fix it", "what the fuck", "fix it", "shit", "fucking again"] {
        a.typed(m).await;
    }
    let s = look(&mut watcher, &kernel, &p, t0 + TimeDelta::minutes(5)).await;
    assert_eq!(of(&s, Kind::MemoryWritten).len(), 3, "an Edit, a Write and a redirect: {:?}", kinds(&s));
    assert_eq!(of(&s, Kind::Compacted).len(), 1);
    let mood: Vec<&str> = of(&s, Kind::Frustration).iter().rev().map(|r| r.what.as_str()).collect();
    assert_eq!(mood, ["3 of your newest 5 messages escalate", "4 of your newest 5 messages escalate"], "and again as more do");
    std::fs::remove_dir_all(&place.dir).ok();
}

// Steps 11 and 12 ---------------------------------------------------------------

const TASK: &str = r#"<?xml version="1.0"?>
<!-- <task version="0"> in a comment is no root -->
<task id="t9" name="nine" version="2">
  <start>Cut the branch and push it.</start>
  <build><module path="src/"><step n="1" name="a">Write src/lib.rs.</step></module></build>
  <gates><gate id="G1"><cmd>cargo test</cmd></gate></gates>
  <owns>src/**; tests/watcher.rs, the tests; docs/</owns>
  <forbid>schema/**, the structure; src/kernel/**; every other crate</forbid>
  <done>Done &amp; dusted.</done>
</task>"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_whose_message_names_a_task_file_under_a_spec_path_is_held_to_it_found_from_its_folders_nearest_ancestor_or_by_its_absolute_path_owns_and_forbid_entries_give_their_paths_with_a_folders_slash_and_an_unreadable_file_holds_it_to_nothing_and_says_why_a_changed_file_is_read_again(
) {
    let p = params(&[]);
    let place = place();
    let tasks = place.checkout.join("specs/recon/tasks");
    std::fs::create_dir_all(&tasks).expect("tasks");
    std::fs::write(tasks.join("9.xml"), TASK).expect("task file");
    let deep = place.checkout.join("src/inner");
    std::fs::create_dir_all(&deep).expect("deep");
    let named = spec::named("build `specs/recon/tasks/9.xml`, please.", &p.watch_spec_paths, deep.to_str()).expect("named");
    assert_eq!((named.path.as_str(), named.checkout.as_deref()), ("specs/recon/tasks/9.xml", Some(place.checkout.as_path())), "the nearest ancestor");
    let absolute = format!("{}/specs/recon/tasks/9.xml", place.checkout.display());
    let named = spec::named(&absolute, &p.watch_spec_paths, None).expect("absolute");
    assert_eq!(named.checkout.as_deref(), Some(place.checkout.as_path()));
    assert!(spec::named("specs/recon/tasks/ alone", &p.watch_spec_paths, None).is_none());

    let task = spec::TaskFile::parse(TASK).expect("parsed");
    assert_eq!(task.version, "2", "the root's, not the comment's");
    assert_eq!(task.owns, ["src/**", "tests/watcher.rs, the tests", "docs/"]);
    assert!(task.words.contains("Cut the branch and push it.") && task.words.contains("Write src/lib.rs.") && task.words.contains("cargo test") && task.words.contains("Done & dusted."));
    assert_eq!(spec::paths("tests/watcher.rs, the tests"), ["tests/watcher.rs"]);
    assert!(spec::matches("src/**", "src/a/b.rs") && spec::matches("docs/", "docs/x/y.md") && spec::matches("src/*.rs", "src/a.rs"));
    assert!(!spec::matches("src/*.rs", "src/a/b.rs") && spec::matches("a/**/b.rs", "a/b.rs"));

    // Held, unreadable, and read again when it changes.
    let (kernel, _, mut a, t0) = setup("held").await;
    a.cwd = deep.to_string_lossy().into_owned();
    a.typed("specs/recon/tasks/missing.xml").await;
    let mut watcher = Watcher::new(Utc::now(), None);
    let s = look(&mut watcher, &kernel, &p, t0 + TimeDelta::minutes(1)).await;
    let view = s.sessions[0].spec.clone().expect("a task file named");
    assert!(view.version.is_none() && view.why.is_some(), "an unreadable file holds it to nothing, and says why: {view:?}");
    a.typed("specs/recon/tasks/9.xml").await;
    let s = look(&mut watcher, &kernel, &p, t0 + TimeDelta::minutes(2)).await;
    let view = s.sessions[0].spec.clone().expect("held");
    assert_eq!((view.version.as_deref(), view.first_version.as_deref()), (Some("2"), Some("2")));
    std::thread::sleep(std::time::Duration::from_millis(1_100));
    std::fs::write(tasks.join("9.xml"), TASK.replace(r#"version="2""#, r#"version="3""#)).expect("changed");
    let s = look(&mut watcher, &kernel, &p, t0 + TimeDelta::minutes(3)).await;
    assert_eq!(s.sessions[0].spec.as_ref().and_then(|v| v.version.as_deref()), Some("3"), "read again");
    std::fs::remove_dir_all(&place.dir).ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_write_that_a_forbid_path_matches_and_no_owns_path_is_a_forbidden_file_a_write_in_the_checkout_that_no_owns_path_matches_is_outside_what_it_owns_and_one_outside_the_checkout_is_neither_a_changed_version_is_reported_once_a_stop_or_a_question_holds_whatever_the_file_orders_and_its_report_says_the_file_ordered_the_act(
) {
    let (kernel, place, mut a, t0) = setup("judged").await;
    let p = with(&[]);
    let tasks = place.checkout.join("specs/recon/tasks");
    std::fs::create_dir_all(&tasks).expect("tasks");
    std::fs::write(tasks.join("9.xml"), TASK).expect("task file");
    let mut watcher = Watcher::new(Utc::now(), None);
    a.typed("specs/recon/tasks/9.xml").await;
    let file = |rel: &str| place.checkout.join(rel);
    a.call("w1", "Write", json!({"file_path": file("schema/x.surql"), "content": "x"})).await;
    a.call("w2", "Write", json!({"file_path": file("README.md"), "content": "x"})).await;
    a.call("w3", "Write", json!({"file_path": file("src/lib.rs"), "content": "x"})).await;
    a.call("w4", "Write", json!({"file_path": "/tmp/elsewhere.rs", "content": "x"})).await;
    a.typed("did you push?").await;
    a.bash("p1", "git push origin feat").await;
    let s = look(&mut watcher, &kernel, &p, t0 + TimeDelta::minutes(1)).await;
    let forbidden = of(&s, Kind::ForbiddenFile);
    assert!(forbidden.len() == 1 && forbidden[0].what.contains("schema/x.surql") && forbidden[0].what.contains("schema/**"), "{forbidden:?}");
    let outside = of(&s, Kind::OutsideOwns);
    assert!(outside.len() == 1 && outside[0].what.contains("README.md") && !outside[0].what.contains("elsewhere"), "{outside:?}");
    let acted = of(&s, Kind::ActedOnQuestion);
    assert!(acted.len() == 1 && acted[0].what.contains("the task file's words order the act"), "{acted:?}");
    std::thread::sleep(std::time::Duration::from_millis(1_100));
    std::fs::write(tasks.join("9.xml"), TASK.replace(r#"version="2""#, r#"version="3""#)).expect("changed");
    look(&mut watcher, &kernel, &p, t0 + TimeDelta::minutes(2)).await;
    std::thread::sleep(std::time::Duration::from_millis(1_100));
    std::fs::write(tasks.join("9.xml"), TASK.replace(r#"version="2""#, r#"version="3""#).replace("Done", "Done again")).expect("touched");
    let s = look(&mut watcher, &kernel, &p, t0 + TimeDelta::minutes(3)).await;
    assert_eq!(of(&s, Kind::SpecVersionChanged).len(), 1, "once a version");
    std::fs::remove_dir_all(&place.dir).ok();
}

// Step 13 -------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_memory_folder_is_found_from_the_transcript_under_the_projects_roots_a_subagents_transcript_leading_to_its_projects_its_files_are_listed_with_their_size_modification_time_and_frontmatter_description_with_no_root_holding_the_transcript_there_is_none_the_roots_are_the_adapters_parameter_one_path_or_a_list(
) {
    let place = place();
    let memory = place.root.join("-work-checkout/memory");
    std::fs::write(memory.join("MEMORY.md"), "# index\n").expect("index");
    let rule = "---\nname: rule\ndescription: \"never merge on red\"\n---\nbody\n";
    std::fs::write(memory.join("rule.md"), rule).expect("rule");
    let roots = vec![place.dir.join("elsewhere"), place.root.clone()];
    let sub = place.root.join("-work-checkout/sess/subagents/agent-1.jsonl");
    assert_eq!(superx_mod_recon::watcher::memory::folder(&sub, &roots), Some(memory.clone()), "a subagent's transcript leads to its project's");
    assert_eq!(superx_mod_recon::watcher::memory::folder(Path::new("/nowhere/x.jsonl"), &roots), None);
    let files = superx_mod_recon::watcher::memory::list(&memory).expect("listed");
    let names: Vec<(&str, u64, Option<&str>)> = files.iter().map(|f| (f.name.as_str(), f.size, f.description.as_deref())).collect();
    assert_eq!(names, [("MEMORY.md", 8, None), ("rule.md", rule.len() as u64, Some("never merge on red"))]);
    assert!(files.iter().all(|f| f.modified.is_some()));

    let kernel = kernel().await;
    let adapter = kernel.register_module(&ClaudeCodeAdapter.descriptor()).await.expect("adapter");
    kernel.set_parameter(adapter.clone(), PROJECTS_ROOT_PARAM, Value::String(place.root.display().to_string())).await.expect("one path");
    assert_eq!(superx_mod_recon::watcher::memory::roots(&kernel, None).await.expect("roots"), std::slice::from_ref(&place.root));
    kernel.set_parameter(adapter, PROJECTS_ROOT_PARAM, strings(&[place.root.as_path(), place.dir.as_path()])).await.expect("a list");
    assert_eq!(superx_mod_recon::watcher::memory::roots(&kernel, None).await.expect("roots"), [place.root.clone(), place.dir.clone()]);

    let t0 = Utc::now() - TimeDelta::hours(1);
    let mut a = Agent::new(&kernel, "mem", &place.transcript, &place.checkout, t0).await;
    a.turn_end().await;
    let mut watcher = Watcher::new(Utc::now(), None);
    let s = look(&mut watcher, &kernel, &params(&[]), t0 + TimeDelta::minutes(1)).await;
    let m = &s.sessions[0].memory;
    assert_eq!((m.folder.as_deref(), m.files.len()), (Some(memory.display().to_string().as_str()), 2), "{m:?}");
    std::fs::remove_dir_all(&place.dir).ok();
}

// Step 14 -------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_report_carries_its_id_session_kind_level_time_what_and_its_words_whole_a_message_longer_than_any_page_shows_uncut_its_id_is_the_same_at_each_look_and_after_a_restart_that_reads_its_lines_back_and_no_report_is_made_twice_the_snapshot_holds_every_field_step_14_names_its_reports_newest_first(
) {
    let (kernel, _place, mut a, t0) = setup("reports").await;
    let p = with(&[]);
    let long = format!("status? {}", "every word ".repeat(20_000));
    let at = a.typed(&long).await;
    a.typed("what is going on").await;
    let mut watcher = Watcher::new(Utc::now(), None);
    let s = look(&mut watcher, &kernel, &p, t0 + TimeDelta::minutes(1)).await;
    let r = s.reports.iter().find(|r| r.at == at).expect("the status ask");
    assert_eq!((r.kind, r.level, r.session.as_str()), (Kind::StatusAsked, Level::Alert, a.id().as_str()));
    assert_eq!(r.words[0].text, long, "uncut");
    assert!(r.id.starts_with(&a.id()) && r.id.contains("status_asked"), "{}", r.id);
    let again = look(&mut watcher, &kernel, &p, t0 + TimeDelta::minutes(2)).await;
    assert_eq!(again.reports.len(), s.reports.len(), "no report made twice");
    let mut restarted = Watcher::new(Utc::now(), None);
    let after = look(&mut restarted, &kernel, &p, t0 + TimeDelta::minutes(3)).await;
    let ids = |s: &Snapshot| s.reports.iter().map(|r| r.id.clone()).collect::<Vec<_>>();
    assert_eq!(ids(&after), ids(&s), "the same ids after a restart that reads its lines back");
    assert!(after.reports.windows(2).all(|w| w[0].at >= w[1].at), "newest first");
    let json = serde_json::to_value(&after).expect("json");
    for key in ["looked_at", "every_secs", "paused", "window_from", "lines_read", "unread", "sessions", "reports"] {
        assert!(json.get(key).is_some(), "the snapshot holds {key}");
    }
    for key in ["id", "name", "key", "agent", "model", "folder", "state", "newest", "doing", "said", "message", "orders", "run", "jobs", "recap", "spec", "memory"] {
        assert!(json["sessions"][0].get(key).is_some(), "a session holds {key}");
    }
}

// Step 15 -------------------------------------------------------------------

#[test]
fn the_parameters_follow_todays_in_step_15s_order_with_their_fallbacks_group_shapes_and_inputs_attr_recon_watch_sessions_takes_all_attr_recon_watch_effects_takes_an_object_of_patterns_to_lists_of_words_and_is_shown_as_step_15_says_a_value_of_the_wrong_shape_is_reported_by_name_and_its_fallback_stands_in(
) {
    let watch_names = [
        params::WATCH_SECS_PARAM,
        params::WATCH_SESSIONS_PARAM,
        params::WATCH_WINDOW_SECS_PARAM,
        params::WATCH_QUIET_SECS_PARAM,
        params::WATCH_QUIET_CALLS_PARAM,
        params::WATCH_BEHIND_SECS_PARAM,
        params::WATCH_RUN_SECS_PARAM,
        params::WATCH_RUN_CALLS_PARAM,
        params::WATCH_STOP_CALLS_PARAM,
        params::WATCH_REPEAT_SECS_PARAM,
        params::WATCH_REPEAT_SHARE_PARAM,
        params::WATCH_ORDER_WORDS_PARAM,
        params::WATCH_MOOD_PROMPTS_PARAM,
        params::WATCH_FAILURES_PARAM,
        params::STOP_WORDS_PARAM,
        params::STATUS_WORDS_PARAM,
        params::QUESTION_WORDS_PARAM,
        params::YES_WORDS_PARAM,
        params::WATCH_SCOPE_WORDS_PARAM,
        params::WATCH_LONG_WORDS_PARAM,
        params::WATCH_SPEC_PATHS_PARAM,
        params::WATCH_EFFECTS_PARAM,
    ];
    assert_eq!(NAMES[NAMES.len() - watch_names.len()..], watch_names, "after today's, in step 15's order");
    for name in watch_names {
        let about = params::about(name).expect(name);
        assert_eq!(about.group, "watches", "{name}");
    }
    let r = params::from_values(&std::collections::BTreeMap::new());
    assert_eq!((r.params.watch_secs, r.params.watch_window_secs, r.params.watch_sessions), (5, 21_600, ALL));
    let shown = |name: &str| r.settings.iter().find(|s| s.name == name).expect(name).value.clone();
    assert_eq!(shown(params::WATCH_SESSIONS_PARAM), "all");
    assert!(shown(params::WATCH_EFFECTS_PARAM).starts_with("CommentToJira → jira, comment; createJiraIssue → jira, ticket, issue;"));
    assert_eq!(params::about(params::WATCH_EFFECTS_PARAM).expect("about").input, "pairs");

    let mut effects = Object::new();
    effects.insert("terraform apply".to_string(), list(&["apply", "terraform"]));
    let mut set = std::collections::BTreeMap::new();
    set.insert(params::WATCH_EFFECTS_PARAM, Value::Object(effects));
    set.insert(params::WATCH_SESSIONS_PARAM, Value::String("all".to_string()));
    let r = params::from_values(&set);
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    assert_eq!(r.params.watch_effects.get("terraform apply").map(Vec::len), Some(2));
    assert_eq!(r.params.watch_sessions, ALL);

    let mut wrong = Object::new();
    wrong.insert("x".to_string(), Value::String("not a list".to_string()));
    let mut set = std::collections::BTreeMap::new();
    set.insert(params::WATCH_EFFECTS_PARAM, Value::Object(wrong));
    set.insert(params::WATCH_SECS_PARAM, Value::String("5".to_string()));
    let r = params::from_values(&set);
    let named: Vec<&str> = r.problems.iter().map(|p| p.name).collect();
    assert_eq!(named, [params::WATCH_SECS_PARAM, params::WATCH_EFFECTS_PARAM], "each by name");
    assert_eq!((r.params.watch_secs, r.params.watch_effects.len()), (5, 15), "the fallbacks stand in");
}

// Step 16 -------------------------------------------------------------------

async fn ask(app: &axum::Router, host: &str, path: &str) -> (StatusCode, String) {
    let request = Request::builder().uri(path).header(header::HOST, host).body(Body::empty()).expect("a request");
    let answer = app.clone().oneshot(request).await.expect("an answer");
    let status = answer.status();
    let bytes = to_bytes(answer.into_body(), usize::MAX).await.expect("its body");
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn api_watch_answers_the_snapshot_and_api_watch_memory_a_file_whole_a_file_that_is_empty_holds_a_slash_or_a_backslash_or_is_dot_or_dot_dot_and_a_session_that_is_not_a_uuid_are_refused_http_400_a_session_not_held_one_with_no_memory_folder_and_a_file_not_listed_are_refused_http_404_both_routes_are_refused_to_another_name(
) {
    let (kernel, place, mut a, t0) = setup("routes").await;
    let memory = place.root.join("-work-checkout/memory");
    let rule = "---\ndescription: a rule\n---\nthe whole rule, every byte\n";
    std::fs::write(memory.join("rule.md"), rule).expect("rule");
    a.turn_end().await;
    let p = with(&[]);
    let mut watcher = Watcher::new(Utc::now(), None);
    watcher.look(&kernel, &p, t0 + TimeDelta::minutes(1)).await.expect("look");
    let watched = Watched::default();
    watched_publish(&watched, watcher.snapshot(&p, t0 + TimeDelta::minutes(1), false));
    let home = scratch();
    let instance = Instance {
        home: home.clone(),
        endpoint: "mem://".to_string(),
        namespace: "superx".to_string(),
        database: "kernel".to_string(),
        data_dir: home.join("db"),
        log_filter: "info".to_string(),
    };
    let app = server::app(kernel.clone(), instance, PORT, superx_mod_recon::crawler::progress::Progress::default(), watched);
    let own = format!("127.0.0.1:{PORT}");
    let (status, body) = ask(&app, &own, "/api/watch").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let json: Json = serde_json::from_str(&body).expect("json");
    assert_eq!(json["sessions"][0]["id"], a.id());
    let (status, body) = ask(&app, &own, &format!("/api/watch/memory?session={}&file=rule.md", a.id())).await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, rule), "whole");
    for bad in ["", "a/b", "a%5Cb", ".", ".."] {
        let (status, _) = ask(&app, &own, &format!("/api/watch/memory?session={}&file={bad}", a.id())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad:?}");
    }
    let (status, _) = ask(&app, &own, "/api/watch/memory?session=nope&file=rule.md").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "a session that is no UUID");
    let (status, _) = ask(&app, &own, &format!("/api/watch/memory?session={}&file=rule.md", uuid::Uuid::nil())).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "a session not held");
    let (status, _) = ask(&app, &own, &format!("/api/watch/memory?session={}&file=other.md", a.id())).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "a file not listed");
    for path in ["/api/watch", "/api/watch/memory?session=x&file=y"] {
        let (status, _) = ask(&app, &format!("evil.example:{PORT}"), path).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
    // A session with no memory folder.
    let mut b = Agent::new(&kernel, "far", Path::new("/nowhere/far.jsonl"), &place.checkout, t0).await;
    b.turn_end().await;
    let mut watcher = Watcher::new(Utc::now(), None);
    watcher.look(&kernel, &p, t0 + TimeDelta::minutes(2)).await.expect("look");
    let watched = Watched::default();
    watched_publish(&watched, watcher.snapshot(&p, t0 + TimeDelta::minutes(2), false));
    let app = server::app(kernel.clone(), Instance::read(&home).expect("instance"), PORT, superx_mod_recon::crawler::progress::Progress::default(), watched);
    let (status, _) = ask(&app, &own, &format!("/api/watch/memory?session={}&file=rule.md", b.id())).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "a session with no memory folder");
    std::fs::remove_dir_all(&place.dir).ok();
}

// Step 18 -------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_page_served_at_slash_is_the_bundle_built_from_the_sources_with_the_watcher_tab_its_script_names_the_view_watcher_and_the_routes_api_watch_and_api_watch_memory(
) {
    let kernel = kernel().await;
    let home = scratch();
    let instance = Instance::read(&home).expect("instance");
    let app = server::app(kernel, instance, PORT, superx_mod_recon::crawler::progress::Progress::default(), Watched::default());
    let own = format!("127.0.0.1:{PORT}");
    let (status, page) = ask(&app, &own, "/").await;
    assert_eq!(status, StatusCode::OK);
    let script = page.split("src=\"").nth(1).and_then(|s| s.split('"').next()).expect("the page's script");
    let (status, bundle) = ask(&app, &own, script).await;
    assert_eq!(status, StatusCode::OK, "{script}");
    assert!(["'watcher'", "\"watcher\"", "`watcher`"].iter().any(|q| bundle.contains(q)), "the bundle names the view watcher");
    for route in ["/api/watch", "/api/watch/memory"] {
        assert!(bundle.contains(route), "the bundle names {route}");
    }
}

/// Publish a snapshot as the watcher's loop does, for the routes to read.
fn watched_publish(watched: &Watched, snapshot: Snapshot) {
    watched.publish(snapshot);
}
