//! UI module contract tests (mem engine). The served page + live API
//! are QA'd against a real instance per the epic.

use superx_kernel::{Kernel, KernelModule, SCHEMA_DDL};
use superx_mod_ui::{
    resolved_context_window, resolved_default_range, resolved_port, resolved_url, UiModule,
    CONTEXT_WINDOW_PARAM, DEFAULT_CONTEXT_WINDOW, DEFAULT_PORT, DEFAULT_RANGE,
    DEFAULT_RANGE_PARAM, MODULE_NAME, PORT_PARAM,
};

const TEST_PASSWORD: &str = "test-kernel-password-for-mem-engine";

async fn fresh_kernel() -> Kernel {
    let db = surrealdb::engine::any::connect("mem://").await.expect("mem");
    db.use_ns("superx").use_db("kernel").await.expect("nsdb");
    let ddl = SCHEMA_DDL.replace("$SUPERX_KERNEL_PASSWORD", TEST_PASSWORD);
    db.query(ddl).await.expect("ddl").check().expect("ddl ok");
    let kernel = Kernel::from_db(db);
    for t in superx_kernel::REQUIRED_METAMODEL_TYPES {
        kernel
            .ensure_type_definition(t.uid, t.category, t.memory_tier)
            .await
            .expect("seed");
    }
    kernel
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn port_defaults_then_follows_the_parameter() {
    let kernel = fresh_kernel().await;
    assert_eq!(resolved_port(&kernel).await, DEFAULT_PORT, "unregistered → default");

    let entity = kernel
        .register_module(&UiModule.descriptor())
        .await
        .expect("register");
    assert_eq!(resolved_port(&kernel).await, DEFAULT_PORT, "no param → default");

    kernel
        .set_parameter(entity, PORT_PARAM, superx_kernel::types::Value::Number(7777.into()))
        .await
        .expect("param");
    assert_eq!(resolved_port(&kernel).await, 7777, "parameter wins");
    assert_eq!(resolved_url(&kernel).await, "http://127.0.0.1:7777");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn context_window_defaults_then_follows_the_parameter() {
    let kernel = fresh_kernel().await;
    assert_eq!(
        resolved_context_window(&kernel).await,
        DEFAULT_CONTEXT_WINDOW,
        "unregistered → default"
    );
    let entity = kernel
        .register_module(&UiModule.descriptor())
        .await
        .expect("register");
    kernel
        .set_parameter(
            entity,
            CONTEXT_WINDOW_PARAM,
            superx_kernel::types::Value::Number(200_000.into()),
        )
        .await
        .expect("param");
    assert_eq!(resolved_context_window(&kernel).await, 200_000, "parameter wins");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ui_cli_reports_url_and_usage() {
    let kernel = fresh_kernel().await;
    kernel.register_module(&UiModule.descriptor()).await.expect("register");

    let url = UiModule.cli(&kernel, &["url".to_string()]).await.expect("url");
    assert!(url.contains("http://127.0.0.1:"), "{url}");

    let err = UiModule
        .cli(&kernel, &["bogus".to_string()])
        .await
        .expect_err("usage");
    assert!(err.to_string().contains("usage: superx ui url"));
}

#[test]
fn descriptor_and_facilities() {
    let d = UiModule.descriptor();
    assert_eq!(d.name, MODULE_NAME);
    assert!(UiModule.needs_dir());
    assert!(UiModule.schema_ddl().is_some(), "owns data objects");
}

// ─────────────────────────────────────────────────────────────────────
// Session activity — the merged per-session stream (issue #172)
// ─────────────────────────────────────────────────────────────────────

/// Seed one agent (with its descriptor, so name resolution works) and
/// one of its sessions (capture-engine descriptor shape:
/// `{name: "<agent>/<key>", session: "<key>", locator}`).
async fn seed_agent_and_session(
    kernel: &superx_kernel::Kernel,
    agent_name: &str,
    src_key: &str,
) -> (
    superx_kernel::types::RecordId,
    superx_kernel::types::RecordId,
) {
    use superx_kernel::types::{Object, Value};
    let agent = kernel.create_entity("node_agent").await.expect("agent");
    let mut agent_desc = Object::new();
    agent_desc.insert("name".to_string(), Value::String(agent_name.to_string()));
    kernel
        .supersede_state(agent.clone(), "attr_agent_descriptor", Value::Object(agent_desc))
        .await
        .expect("agent descriptor");
    let session = kernel.create_entity("node_session").await.expect("session");
    let mut desc = Object::new();
    desc.insert(
        "name".to_string(),
        Value::String(format!("{agent_name}/{src_key}")),
    );
    desc.insert("session".to_string(), Value::String(src_key.to_string()));
    desc.insert("locator".to_string(), Value::String("/tmp/x.jsonl".to_string()));
    kernel
        .supersede_state(session.clone(), "attr_session_descriptor", Value::Object(desc))
        .await
        .expect("session descriptor");
    (agent, session)
}

#[test]
fn session_key_of_reads_only_object_payloads() {
    use superx_kernel::types::{Object, Value};
    use superx_mod_ui::activity::session_key_of;

    let mut payload = Object::new();
    payload.insert("session".to_string(), Value::String("src-abc".to_string()));
    assert_eq!(
        session_key_of(&Value::Object(payload)),
        Some("src-abc".to_string())
    );
    assert_eq!(session_key_of(&Value::String("ui".to_string())), None);
    assert_eq!(session_key_of(&Value::Object(Object::new())), None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_activity_merges_messages_and_actions_for_one_session() {
    use superx_kernel::types::{Object, Value};
    use superx_mod_ui::activity::session_activity;

    let kernel = fresh_kernel().await;
    let (agent, session) = seed_agent_and_session(&kernel, "claude_code", "src-abc").await;

    // 1. A conversation message.
    kernel
        .log_message(superx_kernel::NewMessage {
            session: session.clone(),
            agent: agent.clone(),
            role: "user".to_string(),
            content: "hello".to_string(),
            raw: None,
            seq: None,
            emitted_at: None,
        })
        .await
        .expect("message");

    // 2. An action whose SUBJECT is the session (message_captured shape).
    let mut captured = Object::new();
    captured.insert("session".to_string(), Value::String("src-abc".to_string()));
    captured.insert("role".to_string(), Value::String("user".to_string()));
    kernel
        .log_telemetry_for_agent(
            "message_captured",
            Value::Object(captured),
            Some(session.clone()),
            Some(agent.clone()),
        )
        .await
        .expect("captured");

    // 3. An action bound only via payload.session (tool_call shape:
    //    subject is NOT the session entity).
    let mut tool = Object::new();
    tool.insert("session".to_string(), Value::String("src-abc".to_string()));
    tool.insert("tool".to_string(), Value::String("Bash".to_string()));
    kernel
        .log_telemetry_for_agent(
            "tool_call",
            Value::Object(tool),
            Some(agent.clone()),
            Some(agent.clone()),
        )
        .await
        .expect("tool_call");

    // 4. A GLOBAL event — must never appear in a session's activity.
    kernel
        .log_telemetry("system_boot", Value::String("global".to_string()), None)
        .await
        .expect("boot");

    let events = session_activity(&kernel, session.clone(), 100, None, None)
        .await
        .expect("activity");

    assert_eq!(events.len(), 3, "message + 2 actions, no global event: {events:#?}");
    assert_eq!(events[0].kind, "message");
    assert_eq!(events[0].role.as_deref(), Some("user"));
    assert!(events[0].rendered.contains("hello"), "{}", events[0].rendered);
    assert_eq!(events[1].kind, "action");
    assert!(events[1].rendered.contains("message_captured"), "{}", events[1].rendered);
    assert_eq!(events[2].kind, "action");
    assert!(events[2].rendered.contains("tool_call"), "{}", events[2].rendered);
    // Chronological: capture order is preserved across both tables.
    assert!(events[0].valid_from <= events[1].valid_from);
    assert!(events[1].valid_from <= events[2].valid_from);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn global_activity_includes_everything_including_no_session_events() {
    use superx_kernel::types::Value;
    use superx_mod_ui::activity::global_activity;

    let kernel = fresh_kernel().await;
    let (agent, session) = seed_agent_and_session(&kernel, "claude_code", "src-glob").await;
    kernel
        .log_message(superx_kernel::NewMessage {
            session: session.clone(),
            agent: agent.clone(),
            role: "user".to_string(),
            content: "hello global".to_string(),
            raw: None,
            seq: None,
            emitted_at: None,
        })
        .await
        .expect("message");
    kernel
        .log_telemetry("system_boot", Value::String("global".to_string()), None)
        .await
        .expect("boot");

    let events = global_activity(&kernel, 100, None, None).await.expect("global");
    // The global feed carries the message, its session, AND the
    // no-session OS event — everyone and everything, one place.
    assert!(
        events.iter().any(|e| e.kind == "message" && e.rendered.contains("hello global")),
        "{events:#?}"
    );
    assert!(
        events.iter().any(|e| e.kind == "action" && e.rendered.contains("system_boot")),
        "{events:#?}"
    );
    // Chronological, and message rows carry their session id for
    // client-side attribution.
    assert!(events.windows(2).all(|w| w[0].valid_from <= w[1].valid_from));
    assert!(events
        .iter()
        .filter(|e| e.kind == "message")
        .all(|e| e.session_id.is_some()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stats_summary_mines_tools_lines_and_sessions_from_raw_events() {
    use superx_kernel::message::json_to_object;
    use superx_mod_ui::stats::stats_summary;

    let kernel = fresh_kernel().await;
    let (agent, session) = seed_agent_and_session(&kernel, "claude_code", "src-stats").await;

    // Claude-style: Write (3 lines) + Bash, with 30 output tokens.
    kernel
        .log_message(superx_kernel::NewMessage {
            session: session.clone(),
            agent: agent.clone(),
            role: "assistant".to_string(),
            content: String::new(),
            raw: Some(json_to_object(&serde_json::json!({
                "message": {
                    "usage": {"output_tokens": 30},
                    "content": [
                        {"type": "tool_use", "name": "Write",
                         "input": {"file_path": "a.rs", "content": "a\nb\nc"}},
                        {"type": "tool_use", "name": "Bash",
                         "input": {"command": "ls"}}
                    ]
                }
            }))),
            seq: None,
            emitted_at: None,
        })
        .await
        .expect("write msg");
    // Claude-style: Edit (2 lines).
    kernel
        .log_message(superx_kernel::NewMessage {
            session: session.clone(),
            agent: agent.clone(),
            role: "assistant".to_string(),
            content: String::new(),
            raw: Some(json_to_object(&serde_json::json!({
                "message": {"content": [
                    {"type": "tool_use", "name": "Edit",
                     "input": {"file_path": "b.rs", "new_string": "x\ny"}}
                ]}
            }))),
            seq: None,
            emitted_at: None,
        })
        .await
        .expect("edit msg");
    // Gemini-style: a toolCall + 5 output tokens.
    kernel
        .log_message(superx_kernel::NewMessage {
            session: session.clone(),
            agent: agent.clone(),
            role: "assistant".to_string(),
            content: String::new(),
            raw: Some(json_to_object(&serde_json::json!({
                "tokens": {"output": 5},
                "toolCalls": [{"name": "web_search"}]
            }))),
            seq: None,
            emitted_at: None,
        })
        .await
        .expect("gemini msg");

    let s = stats_summary(&kernel, 100).await.expect("stats");
    assert_eq!(s.messages_total, 3);
    assert_eq!(s.output_tokens_total, 35, "30 claude + 5 gemini");
    assert_eq!(s.lines_written, 5, "Write 3 + Edit 2");
    assert_eq!(s.tools_window, 4, "Write + Bash + Edit + web_search");
    let tool = |n: &str| s.tools.iter().find(|t| t.name == n).map(|t| t.value);
    assert_eq!(tool("Write"), Some(1));
    assert_eq!(tool("Bash"), Some(1));
    assert_eq!(tool("Edit"), Some(1));
    assert_eq!(tool("web_search"), Some(1));
    assert_eq!(s.sessions_active, 1, "messages are fresh");
    assert_eq!(s.window_messages, 100);
    assert_eq!(s.top_sessions.len(), 1);
    let top = &s.top_sessions[0];
    assert_eq!(top.messages, 3);
    assert_eq!(top.lines_written, 5);
    assert_eq!(top.output_tokens, 35);
    assert!(top.identity.starts_with("claude_code/"), "{}", top.identity);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stats_summary_scores_tool_outcomes_across_both_agent_shapes() {
    use superx_kernel::message::json_to_object;
    use superx_mod_ui::stats::stats_summary;

    let kernel = fresh_kernel().await;
    let (agent, session) = seed_agent_and_session(&kernel, "claude_code", "src-outcome").await;
    let msg = |raw: serde_json::Value| superx_kernel::NewMessage {
        session: session.clone(),
        agent: agent.clone(),
        role: "assistant".to_string(),
        content: String::new(),
        raw: Some(json_to_object(&raw)),
        seq: None,
        emitted_at: None,
    };

    // Claude Code: the call, then its verdict in a LATER message.
    kernel
        .log_message(msg(serde_json::json!({
            "message": {"content": [
                {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "ls"}},
                {"type": "tool_use", "id": "t2", "name": "Bash", "input": {"command": "nope"}},
                {"type": "tool_use", "id": "t3", "name": "Read", "input": {"file_path": "a"}}
            ]}
        })))
        .await
        .expect("calls");
    kernel
        .log_message(msg(serde_json::json!({
            "message": {"content": [
                {"type": "tool_result", "tool_use_id": "t1", "is_error": false},
                {"type": "tool_result", "tool_use_id": "t2", "is_error": true}
            ]}
        })))
        .await
        .expect("results");
    // Gemini: the verdict rides on the call itself.
    kernel
        .log_message(msg(serde_json::json!({
            "toolCalls": [
                {"name": "run_shell_command", "status": "success"},
                {"name": "run_shell_command", "status": "error"},
                {"name": "replace", "status": "cancelled"}
            ]
        })))
        .await
        .expect("gemini");

    let s = stats_summary(&kernel, 100).await.expect("stats");
    let out = |n: &str| s.tool_outcomes.iter().find(|t| t.name == n).expect("tool");
    let bash = out("Bash");
    assert_eq!((bash.ok, bash.failed), (1, 1), "t1 ok, t2 failed");
    // t3 never got a result inside the window — counted, not guessed.
    let read = out("Read");
    assert_eq!((read.ok, read.failed, read.unknown), (0, 0, 1));
    let shell = out("run_shell_command");
    assert_eq!((shell.ok, shell.failed), (1, 1));
    assert_eq!(out("replace").cancelled, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn events_per_minute_stays_ordered_across_midnight() {
    use superx_mod_ui::stats::stats_summary;

    let kernel = fresh_kernel().await;
    // Two events either side of midnight, logged oldest-last so the
    // 23:5x event cannot win by insertion order alone.
    for t in ["2026-08-19T23:58:00Z", "2026-08-20T00:03:00Z"] {
        kernel
            .db()
            .query(
                "CREATE telemetry_stream SET lifecycle_event = 'probe', payload = {},
                 valid_from = <datetime>$t",
            )
            .bind(("t", t.to_string()))
            .await
            .expect("seed")
            .check()
            .expect("ok");
    }
    let s = stats_summary(&kernel, 10).await.expect("stats");
    let labels: Vec<&str> = s.events_per_minute.iter().map(|p| p.t.as_str()).collect();
    assert_eq!(labels, vec!["23:58", "00:03"], "the night crossing reads left→right");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn insights_summary_reads_what_nothing_read_before() {
    use superx_kernel::message::json_to_object;
    use superx_mod_ui::insights::insights_summary;

    let kernel = fresh_kernel().await;
    let (agent, session) = seed_agent_and_session(&kernel, "claude_code", "src-deep").await;
    let msg = |raw: serde_json::Value| superx_kernel::NewMessage {
        session: session.clone(),
        agent: agent.clone(),
        role: "assistant".to_string(),
        content: String::new(),
        raw: Some(json_to_object(&raw)),
        seq: None,
        emitted_at: None,
    };
    kernel
        .log_message(msg(serde_json::json!({
            "message": {
                "model": "claude-fable-5",
                "usage": {"input_tokens": 100, "output_tokens": 20,
                          "cache_read_input_tokens": 900, "cache_creation_input_tokens": 50}
            }
        })))
        .await
        .expect("cc msg");
    // Gemini's `input` INCLUDES what it read from cache (#409): 110 in,
    // 100 of them cached, is 10 sent fresh.
    kernel
        .log_message(msg(serde_json::json!({
            "model": "gemini-2.5-pro",
            "tokens": {"input": 110, "output": 5, "cached": 100}
        })))
        .await
        .expect("gemini msg");
    kernel
        .db()
        .query(
            "CREATE telemetry_stream SET lifecycle_event = 'module_active',
             payload = { name: 'entities', startup_duration_ms: 187 }, valid_from = time::now()",
        )
        .await
        .expect("seed startup")
        .check()
        .expect("ok");

    let i = insights_summary(&kernel).await.expect("insights");

    // Both token vocabularies land in one set of counters.
    assert_eq!(i.tokens.input, 110, "100 claude + 10 gemini");
    assert_eq!(i.tokens.output, 25);
    assert_eq!(i.tokens.cache_read, 1000, "900 claude + 100 gemini cached");
    assert_eq!(i.tokens.cache_write, 50);

    // Model names — read here for the first time.
    let model = |n: &str| i.models.iter().find(|m| m.name == n).map(|m| m.value);
    assert_eq!(model("claude-fable-5"), Some(1));
    assert_eq!(model("gemini-2.5-pro"), Some(1));

    // The per-agent link resolves to the agent's name, not a raw id.
    let cc = i.per_agent.iter().find(|a| a.name == "claude_code").expect("agent split");
    assert_eq!(cc.messages, 2);
    assert_eq!(cc.output_tokens, 25);

    // Capture health and the kind mix.
    assert!(i.event_kinds.iter().any(|k| k.name == "module_active"));
    assert!(i.events_last_hour >= 1);
    assert!(i.last_event_secs.is_some_and(|s| s < 60), "just captured");
    assert_eq!(
        i.module_startup.iter().find(|m| m.name == "entities").map(|m| m.value),
        Some(187)
    );

    // The calendar and the week grid both have today in them.
    assert!(!i.events_per_day.is_empty());
    assert!(i.hour_weekday.iter().any(|c| c.value > 0));
    assert!(i.hour_weekday.iter().all(|c| (1..=7).contains(&c.weekday) && c.hour < 24));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_calendar_follows_the_agents_clock_not_the_capture_run() {
    use superx_mod_ui::insights::insights_summary;

    let kernel = fresh_kernel().await;
    let (agent, session) = seed_agent_and_session(&kernel, "gemini_cli", "src-clock").await;
    // A conversation from May, ingested today — the shape of every
    // backfill. Wednesday 2026-05-20, 14:00 UTC.
    let emitted: chrono::DateTime<chrono::Utc> = "2026-05-20T14:30:00Z".parse().expect("ts");
    for _ in 0..3 {
        kernel
            .log_message(superx_kernel::NewMessage {
                session: session.clone(),
                agent: agent.clone(),
                role: "assistant".to_string(),
                content: "old work".to_string(),
                raw: None,
                seq: None,
                emitted_at: Some(emitted),
            })
            .await
            .expect("backfilled msg");
    }

    let i = insights_summary(&kernel).await.expect("insights");
    let day = i.events_per_day.iter().find(|d| d.t == "2026-05-20");
    assert_eq!(day.map(|d| d.value), Some(3), "the calendar day is May, not today");
    assert!(
        i.events_per_day.iter().all(|d| d.t == "2026-05-20"),
        "capture time must not appear as a second day: {:?}",
        i.events_per_day.iter().map(|d| &d.t).collect::<Vec<_>>()
    );
    // 2026-05-20 is a Wednesday → time::wday 3, hour 14.
    let cell = i.hour_weekday.iter().find(|c| c.value == 3).expect("cell");
    assert_eq!((cell.hour, cell.weekday), (14, 3));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_token_stats_mines_usage_from_raw_events() {
    use superx_kernel::message::json_to_object;
    use superx_mod_ui::activity::session_token_stats;

    let kernel = fresh_kernel().await;
    let (agent, session) = seed_agent_and_session(&kernel, "claude_code", "src-tok").await;

    // Two assistant turns with Claude Code usage shapes; the SECOND is
    // the newest, so context comes from it and outputs sum across both.
    for (input, cache_read, output) in [(100i64, 1_000i64, 50i64), (10, 2_000, 20)] {
        kernel
            .log_message(superx_kernel::NewMessage {
                session: session.clone(),
                agent: agent.clone(),
                role: "assistant".to_string(),
                content: "hi".to_string(),
                raw: Some(json_to_object(&serde_json::json!({
                    "message": {"usage": {
                        "input_tokens": input,
                        "cache_read_input_tokens": cache_read,
                        "output_tokens": output
                    }}
                }))),
                seq: None,
                emitted_at: None,
            })
            .await
            .expect("message");
    }
    // A usage-less message (user prompt) must not disturb the stats.
    kernel
        .log_message(superx_kernel::NewMessage {
            session: session.clone(),
            agent: agent.clone(),
            role: "user".to_string(),
            content: "hello".to_string(),
            raw: None,
            seq: None,
            emitted_at: None,
        })
        .await
        .expect("plain message");

    let (context, output_total) = session_token_stats(&kernel, session)
        .await
        .expect("stats");
    assert_eq!(output_total, Some(70), "sum of output_tokens across turns");
    assert_eq!(context, Some(2_010), "newest usage: input + cache_read");

    // A session with no usage data reports nothing, never zeroes.
    let (_a2, empty) = seed_agent_and_session(&kernel, "gemini_cli", "src-empty").await;
    let (c, o) = session_token_stats(&kernel, empty).await.expect("empty");
    assert_eq!((c, o), (None, None));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_activity_keeps_the_newest_rows_when_truncated() {
    use superx_mod_ui::activity::session_activity;

    let kernel = fresh_kernel().await;
    let (agent, session) = seed_agent_and_session(&kernel, "claude_code", "src-new").await;
    for i in 0..4 {
        kernel
            .log_message(superx_kernel::NewMessage {
                session: session.clone(),
                agent: agent.clone(),
                role: "user".to_string(),
                content: format!("msg-{i}"),
                raw: None,
                seq: None,
                emitted_at: None,
            })
            .await
            .expect("message");
    }

    let events = session_activity(&kernel, session, 2, None, None).await.expect("activity");
    assert_eq!(events.len(), 2, "merged page truncates to limit");
    // Newest-N: the pinned-to-bottom view must end at the present.
    assert!(events[0].rendered.contains("msg-2"), "{}", events[0].rendered);
    assert!(events[1].rendered.contains("msg-3"), "{}", events[1].rendered);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_reaches_all_history_across_messages_and_actions() {
    use superx_kernel::types::{Object, Value};
    use superx_mod_ui::activity::{global_activity, session_activity};

    let kernel = fresh_kernel().await;
    let (agent, session) = seed_agent_and_session(&kernel, "claude_code", "src-find").await;
    for text in ["the needle is here", "haystack one", "haystack two"] {
        kernel
            .log_message(superx_kernel::NewMessage {
                session: session.clone(),
                agent: agent.clone(),
                role: "user".to_string(),
                content: text.to_string(),
                raw: None,
                seq: None,
                emitted_at: None,
            })
            .await
            .expect("message");
    }
    // An action whose payload — not its name — carries the keyword.
    let mut p = Object::new();
    p.insert("session".to_string(), Value::String("src-find".to_string()));
    p.insert("tool".to_string(), Value::String("NeedleTool".to_string()));
    kernel
        .log_telemetry_for_agent(
            "tool_call",
            Value::Object(p),
            Some(agent.clone()),
            Some(agent.clone()),
        )
        .await
        .expect("action");

    // Message text and action payloads both match, case-insensitively.
    let hits = global_activity(&kernel, 100, None, Some("NEEDLE"))
        .await
        .expect("search");
    assert_eq!(hits.len(), 2, "one message + one action: {hits:#?}");
    assert!(hits.iter().any(|h| h.kind == "message"));
    assert!(hits.iter().any(|h| h.kind == "action"));

    // The event NAME is searchable too.
    let by_name = global_activity(&kernel, 100, None, Some("tool_call"))
        .await
        .expect("by name");
    assert_eq!(by_name.len(), 1);

    // Search composes with the session scope and with paging.
    let scoped = session_activity(&kernel, session.clone(), 100, None, Some("haystack"))
        .await
        .expect("scoped");
    assert_eq!(scoped.len(), 2, "both haystacks, no needle: {scoped:#?}");
    let cut: chrono::DateTime<chrono::Utc> = scoped[1].valid_from.parse().expect("rfc3339");
    let older = session_activity(&kernel, session.clone(), 100, Some(cut), Some("haystack"))
        .await
        .expect("older page of matches");
    assert_eq!(older.len(), 1, "paging a search walks only its matches");

    // A keyword that matches nothing returns nothing — never everything.
    assert!(global_activity(&kernel, 100, None, Some("zzz-not-here"))
        .await
        .expect("miss")
        .is_empty());

    // Not every payload is an object: `module_provisioned` emits a bare
    // string. Reading a field off it must yield nothing, not an error
    // that takes the whole search down.
    kernel
        .log_telemetry("module_provisioned", Value::String("needlemod".to_string()), None)
        .await
        .expect("bare payload");
    let after_bare = global_activity(&kernel, 100, None, Some("needle"))
        .await
        .expect("search survives a non-object payload");
    assert_eq!(after_bare.len(), 2, "the bare-payload row neither matches nor breaks it");
    assert_eq!(
        global_activity(&kernel, 100, None, Some("module_provisioned"))
            .await
            .expect("by name")
            .len(),
        1,
        "and it is still findable by its event name"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_model_is_the_newest_one_the_session_used() {
    use superx_kernel::message::json_to_object;
    use superx_mod_ui::activity::session_model;

    let kernel = fresh_kernel().await;
    let (agent, session) = seed_agent_and_session(&kernel, "claude_code", "src-model").await;
    assert_eq!(
        session_model(&kernel, session.clone()).await.expect("none yet"),
        None,
        "no message names a model yet"
    );

    for model in ["claude-opus-5", "claude-fable-5"] {
        kernel
            .log_message(superx_kernel::NewMessage {
                session: session.clone(),
                agent: agent.clone(),
                role: "assistant".to_string(),
                content: String::new(),
                raw: Some(json_to_object(&serde_json::json!({ "message": { "model": model } }))),
                seq: None,
                emitted_at: None,
            })
            .await
            .expect("message");
    }
    // The operator switched mid-session: the CURRENT model wins.
    assert_eq!(
        session_model(&kernel, session.clone()).await.expect("model"),
        Some("claude-fable-5".to_string())
    );

    // `<synthetic>` is the runtime writing a line itself, not a model
    // (#367). The Status page filtered it; the Sessions page showed it
    // as the session's model (#388). The newest REAL answer wins, even
    // behind a run of runtime lines.
    for _ in 0..3 {
        kernel
            .log_message(superx_kernel::NewMessage {
                session: session.clone(),
                agent: agent.clone(),
                role: "assistant".to_string(),
                content: String::new(),
                raw: Some(json_to_object(&serde_json::json!({ "message": { "model": "<synthetic>" } }))),
                seq: None,
                emitted_at: None,
            })
            .await
            .expect("synthetic message");
    }
    assert_eq!(
        session_model(&kernel, session.clone()).await.expect("model"),
        Some("claude-fable-5".to_string()),
        "the sentinel is not a model"
    );

    // Gemini names it at the root instead — same answer, one field over.
    let (agent2, session2) = seed_agent_and_session(&kernel, "gemini_cli", "src-model-2").await;
    kernel
        .log_message(superx_kernel::NewMessage {
            session: session2.clone(),
            agent: agent2,
            role: "assistant".to_string(),
            content: String::new(),
            raw: Some(json_to_object(&serde_json::json!({ "model": "gemini-3.1-pro" }))),
            seq: None,
            emitted_at: None,
        })
        .await
        .expect("gemini message");
    assert_eq!(
        session_model(&kernel, session2).await.expect("gemini model"),
        Some("gemini-3.1-pro".to_string())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn feeds_page_backwards_through_the_whole_history() {
    use superx_kernel::types::{Object, Value};
    use superx_mod_ui::activity::{global_activity, session_activity};

    let kernel = fresh_kernel().await;
    let (agent, session) = seed_agent_and_session(&kernel, "claude_code", "src-page").await;

    // 25 messages and 25 actions in the one session: 50 rows, more
    // than any single page below.
    for n in 0..25 {
        kernel
            .log_message(superx_kernel::NewMessage {
                session: session.clone(),
                agent: agent.clone(),
                role: "user".to_string(),
                content: format!("m{n}"),
                raw: None,
                seq: None,
                emitted_at: None,
            })
            .await
            .expect("message");
        let mut p = Object::new();
        p.insert("session".to_string(), Value::String("src-page".to_string()));
        p.insert("tool".to_string(), Value::String(format!("t{n}")));
        kernel
            .log_telemetry_for_agent(
                "tool_call",
                Value::Object(p),
                Some(agent.clone()),
                Some(agent.clone()),
            )
            .await
            .expect("action");
    }

    // Walk the session feed backwards a page at a time, exactly as the
    // dashboard does: cursor = the oldest row currently held.
    let page = 10;
    let mut seen: Vec<String> = Vec::new();
    let mut cursor: Option<chrono::DateTime<chrono::Utc>> = None;
    for _ in 0..12 {
        let rows = session_activity(&kernel, session.clone(), page, cursor, None)
            .await
            .expect("page");
        if rows.is_empty() {
            break;
        }
        assert!(rows.len() <= page as usize, "a page never exceeds its limit");
        // Oldest-first within the page, and every page older than the last.
        let oldest = &rows[0];
        cursor = Some(oldest.valid_from.parse().expect("rfc3339"));
        for r in &rows {
            seen.push(r.id.clone());
        }
    }
    let unique: std::collections::HashSet<&String> = seen.iter().collect();
    assert_eq!(unique.len(), 50, "every row reachable exactly once: {}", seen.len());
    assert_eq!(seen.len(), 50, "no row served twice across page boundaries");

    // A cursor older than everything ends the walk rather than looping.
    let ancient: chrono::DateTime<chrono::Utc> = "2000-01-01T00:00:00Z".parse().expect("ts");
    assert!(
        session_activity(&kernel, session.clone(), page, Some(ancient), None)
            .await
            .expect("empty")
            .is_empty(),
        "before the beginning is empty, which is how the UI stops"
    );

    // The global feed pages the same way — 50 session rows plus the
    // module/boot events any instance carries.
    let first = global_activity(&kernel, 10, None, None).await.expect("global");
    assert_eq!(first.len(), 10);
    let cut: chrono::DateTime<chrono::Utc> = first[0].valid_from.parse().expect("rfc3339");
    let older = global_activity(&kernel, 10, Some(cut), None).await.expect("older");
    assert!(!older.is_empty(), "there IS more history behind the first page");
    assert!(
        older.iter().all(|o| o.valid_from < first[0].valid_from),
        "a page is strictly older than the cursor"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_activity_never_bleeds_across_agents_sharing_a_source_key() {
    use superx_kernel::types::{Object, Value};
    use superx_mod_ui::activity::session_activity;

    let kernel = fresh_kernel().await;
    // Two agents whose sessions collide on the shared fallback key.
    let (agent_a, session_a) =
        seed_agent_and_session(&kernel, "claude_code", "unknown-session").await;
    let (_agent_b, session_b) =
        seed_agent_and_session(&kernel, "gemini_cli", "unknown-session").await;

    // An action bound only via payload.session, emitted by agent A.
    let mut tool = Object::new();
    tool.insert(
        "session".to_string(),
        Value::String("unknown-session".to_string()),
    );
    tool.insert("tool".to_string(), Value::String("Bash".to_string()));
    kernel
        .log_telemetry_for_agent(
            "tool_call",
            Value::Object(tool),
            Some(agent_a.clone()),
            Some(agent_a.clone()),
        )
        .await
        .expect("tool_call");

    let a = session_activity(&kernel, session_a, 100, None, None).await.expect("a");
    assert_eq!(a.len(), 1, "agent A's session sees its own action: {a:#?}");
    assert!(a[0].rendered.contains("tool_call"));

    let b = session_activity(&kernel, session_b, 100, None, None).await.expect("b");
    assert!(
        b.is_empty(),
        "agent B's session must NOT see agent A's action: {b:#?}"
    );
}

// ─────────────────────────────────────────────────────────────────────
// The coder's instruments (issue #308) — what the window did to code.
// ─────────────────────────────────────────────────────────────────────

/// One captured assistant message carrying tool_use blocks, in the
/// shape Claude Code actually writes.
async fn log_tool_message(kernel: &Kernel, session: &superx_kernel::types::RecordId,
                          agent: &superx_kernel::types::RecordId, raw: serde_json::Value) {
    kernel
        .log_message(superx_kernel::NewMessage {
            session: session.clone(),
            agent: agent.clone(),
            role: "assistant".to_string(),
            content: String::new(),
            raw: Some(superx_kernel::message::json_to_object(&raw)),
            seq: None,
            emitted_at: None,
        })
        .await
        .expect("message");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cockpit_instruments_read_the_work() {
    let kernel = fresh_kernel().await;
    let agent = kernel.create_entity("node_agent").await.expect("agent");
    let session = kernel.create_entity("node_session").await.expect("session");
    let repo = TestRepo::new("superx", "feat/cockpit");
    let cwd = repo.cwd();

    // An Edit: three lines replace one — added and removed are
    // different numbers, which the old lines_written could not say.
    log_tool_message(&kernel, &session, &agent, serde_json::json!({
        "cwd": cwd, "gitBranch": "feat/cockpit",
        "message": {"usage": {"output_tokens": 120, "output_tokens_details": {"thinking_tokens": 45}},
            "content": [{"type": "tool_use", "id": "t1", "name": "Edit",
                "input": {"file_path": repo.file("crates/mod/src/stats.rs"),
                          "old_string": "one line", "new_string": "a\nb\nc"}}]}
    })).await;

    // A test run, a build, and a git push — three shell classes.
    for cmd in ["cargo test --workspace", "cargo build --release", "git push -u origin HEAD"] {
        log_tool_message(&kernel, &session, &agent, serde_json::json!({
            "cwd": cwd,
            "message": {"content": [{"type": "tool_use", "id": "c", "name": "Bash",
                "input": {"command": cmd}}]}
        })).await;
    }

    // A read, an MCP call, a web fetch, a delegated subagent.
    for (name, input) in [
        ("Read", serde_json::json!({"file_path": repo.file("README.md")})),
        ("mcp__gdx__search", serde_json::json!({})),
        ("WebFetch", serde_json::json!({})),
        ("Task", serde_json::json!({})),
    ] {
        log_tool_message(&kernel, &session, &agent, serde_json::json!({
            "cwd": cwd,
            "message": {"content": [{"type": "tool_use", "id": "x", "name": name, "input": input}]}
        })).await;
    }

    let s = superx_mod_ui::stats::stats_summary(&kernel, 500).await.expect("stats");

    // Code output, with the add/remove split the old figure lacked.
    assert_eq!(s.lines_added, 3, "new_string lines");
    assert_eq!(s.lines_removed, 1, "old_string lines");
    // Reading is not touching (#412): the edited file, not the read one.
    assert_eq!(s.files_touched, 1, "the edited file");
    assert_eq!(s.writes_window, 1);
    assert_eq!(s.reads_window, 1);

    // Language and directory mix come from the paths themselves.
    assert_eq!(s.languages.iter().find(|l| l.name == "rs").map(|l| l.value), Some(1));
    assert_eq!(s.languages.iter().find(|l| l.name == "md").map(|l| l.value), None, "only read");
    assert!(s.dirs.iter().any(|d| d.name.ends_with("mod/src")), "{:?}", s.dirs);

    // Commands carry their subcommand, and are classified.
    let names: Vec<&str> = s.commands.iter().map(|c| c.name.as_str()).collect();
    assert!(names.contains(&"cargo test"), "{names:?}");
    assert!(names.contains(&"cargo build"), "{names:?}");
    assert!(names.contains(&"git push"), "{names:?}");
    assert_eq!(s.tests_run, 1);
    assert_eq!(s.builds_run, 1);
    assert_eq!(s.git_ops, 1);

    // Call classes.
    assert_eq!(s.mcp_calls, 1);
    assert_eq!(s.web_calls, 1);
    assert_eq!(s.subagent_calls, 1);
    assert_eq!(s.thinking_tokens, 45);

    // The project the work happened in, with its branch.
    // One row per project, labelled with its newest branch — not one
    // row per (project, branch) pair, which would split the count.
    assert_eq!(s.projects.len(), 1, "{:?}", s.projects);
    assert_eq!(s.projects[0].name, "superx · feat/cockpit");
    assert_eq!(s.projects[0].value, 8, "every message counted once");

    // 24×7 instruments: everything just written is inside the hour.
    assert!(s.messages_last_hour >= 8, "got {}", s.messages_last_hour);
    assert!(s.tokens_last_hour >= 120, "got {}", s.tokens_last_hour);
    // One hour of work — but a run that straddles :59:59 → :00:00
    // legitimately touches two buckets, so this must not be `== 1`.
    assert!(
        (1..=2).contains(&s.active_hours_24h),
        "one hour of work, two if the run crossed the boundary: {}",
        s.active_hours_24h
    );
}

/// The shell shapes agents actually emit (review of #311). `cd repo &&
/// cargo test` used to label `cd` and score zero tests, which made the
/// whole command mix read wrong on real data.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn command_mix_reads_real_agent_shells() {
    struct Case {
        cmd: &'static str,
        expect: &'static [&'static str],
        tests: i64,
        builds: i64,
        git: i64,
    }
    const CASES: &[Case] = &[
        // The chain — the common case, and the one that was broken.
        Case { cmd: "cd /Users/x/repo && cargo test --workspace",
               expect: &["cargo test"], tests: 1, builds: 0, git: 0 },
        Case { cmd: "cd frontend && npm run build",
               expect: &["npm run build"], tests: 0, builds: 1, git: 0 },
        // A flag's VALUE is not a subcommand.
        Case { cmd: "git -C /repo status",
               expect: &["git status"], tests: 0, builds: 0, git: 1 },
        // Subshell punctuation is not a program.
        Case { cmd: "( cd x && make )",
               expect: &["make"], tests: 0, builds: 1, git: 0 },
        // Every stage of a chain counts.
        Case { cmd: "cargo build --release && cargo test && git push",
               expect: &["cargo build", "cargo test", "git push"], tests: 1, builds: 1, git: 1 },
        // Env prefixes and pipes.
        Case { cmd: "RUST_LOG=debug cargo clippy -- -D warnings | tail -5",
               expect: &["cargo clippy", "tail"], tests: 0, builds: 0, git: 0 },
        // Plain program, no subcommand vocabulary.
        Case { cmd: "python3 tools/skill_audit.py",
               expect: &["python3"], tests: 0, builds: 0, git: 0 },
        // `2>&1` is a redirection, not a sequencer: live QA found a
        // stage labelled `1` topping the command mix (#334).
        Case { cmd: "cargo test --workspace 2>&1 | tail -5",
               expect: &["cargo test", "tail"], tests: 1, builds: 0, git: 0 },
        // A heredoc body is DATA. Every line of this embedded script
        // was being read as a shell call, producing `let`, `assert`,
        // `\"\"\"` and `if` as top commands (#334).
        Case { cmd: "python3 - <<'PYEOF'\nlet x = 1\nassert x\nif True:\n    print(\"\"\")\nPYEOF\ngit status",
               expect: &["python3", "git status"], tests: 0, builds: 0, git: 1 },
    ];

    for c in CASES {
        let kernel = fresh_kernel().await;
        let agent = kernel.create_entity("node_agent").await.expect("agent");
        let session = kernel.create_entity("node_session").await.expect("session");
        log_tool_message(&kernel, &session, &agent, serde_json::json!({
            "message": {"content": [{"type": "tool_use", "id": "b", "name": "Bash",
                "input": {"command": c.cmd}}]}
        })).await;
        let s = superx_mod_ui::stats::stats_summary(&kernel, 100).await.expect("stats");
        let got: Vec<&str> = s.commands.iter().map(|x| x.name.as_str()).collect();
        for want in c.expect {
            assert!(got.contains(want), "`{}` → {got:?}, expected to contain {want:?}", c.cmd);
        }
        assert_eq!(got.len(), c.expect.len(), "`{}` → {got:?}", c.cmd);
        assert_eq!(s.tests_run, c.tests, "tests for `{}`", c.cmd);
        assert_eq!(s.builds_run, c.builds, "builds for `{}`", c.cmd);
        assert_eq!(s.git_ops, c.git, "git for `{}`", c.cmd);
    }
}

/// A dotfile is a name, not a language (review of #311).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dotfiles_are_not_languages() {
    let kernel = fresh_kernel().await;
    let agent = kernel.create_entity("node_agent").await.expect("agent");
    let session = kernel.create_entity("node_session").await.expect("session");
    for path in ["/repo/.gitignore", "/repo/src/main.rs"] {
        log_tool_message(&kernel, &session, &agent, serde_json::json!({
            "message": {"content": [{"type": "tool_use", "id": "e", "name": "Edit",
                "input": {"file_path": path, "old_string": "a", "new_string": "b"}}]}
        })).await;
    }
    let s = superx_mod_ui::stats::stats_summary(&kernel, 100).await.expect("stats");
    let langs: Vec<&str> = s.languages.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(langs, vec!["rs"], "only the real extension counts: {langs:?}");
    assert_eq!(s.files_touched, 2, "both files still counted as touched");
}

/// Churn, undo detection, and the struggle instruments (issue #324).
/// The ratio is the insight: replaced ÷ (added + replaced). A window
/// that rewrites the same lines scores high; greenfield scores zero.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn churn_and_rework_signals() {
    let kernel = fresh_kernel().await;
    let agent = kernel.create_entity("node_agent").await.expect("agent");
    let session = kernel.create_entity("node_session").await.expect("session");
    let edit = |path: &str, old: &str, new: &str| {
        serde_json::json!({"message": {"content": [{"type": "tool_use", "id": "e", "name": "Edit",
            "input": {"file_path": path, "old_string": old, "new_string": new}}]}})
    };

    // Greenfield: a Write replaces nothing.
    log_tool_message(&kernel, &session, &agent, serde_json::json!({
        "message": {"usage": {"output_tokens": 400},
            "content": [{"type": "tool_use", "id": "w", "name": "Write",
                "input": {"file_path": "/r/src/new.rs", "content": "a\nb\nc\nd"}}]}
    })).await;

    // A real edit, then an edit that PUTS THE FIRST TEXT BACK — the
    // undo. Snippets are long enough to be evidence.
    let original = "fn handler() { the original body here }";
    let replacement = "fn handler() { a different body entirely }";
    log_tool_message(&kernel, &session, &agent, edit("/r/src/lib.rs", original, replacement)).await;
    log_tool_message(&kernel, &session, &agent, edit("/r/src/lib.rs", replacement, original)).await;
    // A third touch makes lib.rs a thrash file (3+ touches).
    log_tool_message(&kernel, &session, &agent, edit("/r/src/lib.rs", original, replacement)).await;

    // The same command four times — fighting something.
    for _ in 0..4 {
        log_tool_message(&kernel, &session, &agent, serde_json::json!({
            "message": {"content": [{"type": "tool_use", "id": "b", "name": "Bash",
                "input": {"command": "cargo test --workspace"}}]}
        })).await;
    }

    let s = superx_mod_ui::stats::stats_summary(&kernel, 500).await.expect("stats");

    // Churn: 4 written lines + 3 edits of one line each = 7 added,
    // 3 replaced. The ratio is what the operator reads.
    assert_eq!(s.lines_added, 7, "4 written + 3 edited");
    assert_eq!(s.lines_removed, 3, "three single-line replacements");
    assert!(!s.churn.is_empty(), "the chart has a point");
    let charted_added: i64 = s.churn.iter().map(|p| p.added).sum();
    let charted_removed: i64 = s.churn.iter().map(|p| p.removed).sum();
    assert_eq!(charted_added, s.lines_added, "series totals match the tile");
    assert_eq!(charted_removed, s.lines_removed);

    // A→B, B→A, A→B is a flip-flop: TWO undo relationships. Edit 2's
    // work was thrown away by edit 3, and edit 3's by edit 4. Counting
    // both is the honest reading of "how much work was discarded".
    assert_eq!(s.reverts, 2, "two edits had their work undone later");

    // lib.rs was touched three times; new.rs once.
    assert_eq!(s.thrash_files, 1, "only lib.rs crossed the threshold");

    // Struggle: the repeated command surfaces with its count.
    let repeat = s.top_repeat.expect("a command ran 3+ times");
    assert_eq!(repeat.name, "cargo test --workspace", "the whole line, not the program (#367)");
    assert_eq!(repeat.value, 4);

    // Economics and shape.
    assert_eq!(s.out_tokens_window, 400, "window tokens, for tokens-per-line");
    assert_eq!(s.max_concurrent_sessions, 1, "one session was live");
    assert!(s.longest_quiet_mins < 5, "these all landed together");
}

/// Many agents across many repos (issue #325), quality mined from what
/// commands printed (#327), and the model dimension (#328).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repos_models_and_quality_are_separable() {
    let kernel = fresh_kernel().await;
    let alpha = TestRepo::new("alpha", "main");
    let beta = TestRepo::new("beta", "main");
    let a1 = kernel.create_entity("node_agent").await.expect("a1");
    let a2 = kernel.create_entity("node_agent").await.expect("a2");
    let s1 = kernel.create_entity("node_session").await.expect("s1");
    let s2 = kernel.create_entity("node_session").await.expect("s2");

    // Agent 1 writes in repo A with the fast model.
    kernel.log_message(superx_kernel::NewMessage {
        session: s1.clone(), agent: a1.clone(), role: "assistant".into(), content: String::new(),
        raw: Some(superx_kernel::message::json_to_object(&serde_json::json!({
            "cwd": alpha.cwd(), "gitBranch": "main",
            "message": {"model": "claude-fable-5", "usage": {"output_tokens": 300},
                "content": [{"type": "tool_use", "id": "e1", "name": "Write",
                    "input": {"file_path": alpha.file("src/a.rs"), "content": "1\n2\n3"}}]}}))),
        seq: None, emitted_at: None,
    }).await.expect("m1");

    // Agent 2 works repo B, runs a test suite, and the output says
    // what happened — the seam we had never read.
    kernel.log_message(superx_kernel::NewMessage {
        session: s2.clone(), agent: a2.clone(), role: "assistant".into(), content: String::new(),
        raw: Some(superx_kernel::message::json_to_object(&serde_json::json!({
            "cwd": beta.cwd(),
            "message": {"model": "claude-opus-5",
                "content": [{"type": "tool_use", "id": "b1", "name": "Bash",
                    "input": {"command": "cargo test --workspace"}}]}}))),
        seq: None, emitted_at: None,
    }).await.expect("m2");
    kernel.log_message(superx_kernel::NewMessage {
        session: s2.clone(), agent: a2.clone(), role: "tool".into(), content: String::new(),
        raw: Some(superx_kernel::message::json_to_object(&serde_json::json!({
            "cwd": beta.cwd(),
            "message": {"content": [{"type": "tool_result", "tool_use_id": "b1", "is_error": true,
                "content": "running 9 tests\ntest result: FAILED. 7 passed; 2 failed; 0 ignored\nerror[E0382]: borrow of moved value\nerror: could not compile `beta`"}]}}))),
        seq: None, emitted_at: None,
    }).await.expect("m3");

    // The operator had to step in, and the agent ran out of context.
    kernel.log_message(superx_kernel::NewMessage {
        session: s2.clone(), agent: a2.clone(), role: "user".into(), content: "stop".into(),
        raw: Some(superx_kernel::message::json_to_object(&serde_json::json!({
            "cwd": beta.cwd(), "interruptedMessageId": "abc",
            "message": {"content": []}}))),
        seq: None, emitted_at: None,
    }).await.expect("m4");
    kernel.log_message(superx_kernel::NewMessage {
        session: s2.clone(), agent: a2.clone(), role: "system".into(), content: String::new(),
        raw: Some(superx_kernel::message::json_to_object(&serde_json::json!({
            "cwd": beta.cwd(), "isCompactSummary": true, "message": {"content": []}}))),
        seq: None, emitted_at: None,
    }).await.expect("m5");

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");

    // Range is echoed, and this sample did not hit the cap.
    assert_eq!(s.range, "24h");
    assert!(!s.truncated);

    // Two repos, separable — one busy repo cannot hide the other.
    assert_eq!(s.repos.len(), 2, "{:?}", s.repos.iter().map(|r| &r.name).collect::<Vec<_>>());
    let alpha = s.repos.iter().find(|r| r.name == "alpha").expect("alpha");
    assert_eq!(alpha.branch.as_deref(), Some("main"));
    assert_eq!(alpha.lines_added, 3);
    assert_eq!(alpha.agents, 1);
    let beta = s.repos.iter().find(|r| r.name == "beta").expect("beta");
    assert_eq!(beta.tests_run, 1, "the test call is attributed to beta");
    assert_eq!(beta.tool_failures, 1);
    assert_eq!(beta.lines_added, 0, "beta wrote no code in this window");

    // Quality, read out of the tool output.
    assert_eq!(s.tests_passed, 7);
    assert_eq!(s.tests_failed, 2);
    assert_eq!(s.compile_errors, 2, "error[E0382] and could-not-compile");
    assert_eq!(s.interventions, 1, "the operator interrupted once");
    assert_eq!(s.compactions, 1, "context was exhausted once");

    // The model dimension: who produced what.
    let fable = s.models.iter().find(|m| m.name == "claude-fable-5").expect("fable");
    assert_eq!(fable.lines_added, 3);
    assert_eq!(fable.out_tokens, 300);
    let opus = s.models.iter().find(|m| m.name == "claude-opus-5").expect("opus");
    assert_eq!(opus.tool_failures, 1, "the failure lands on the model that caused it");

    // Both sessions are live, and each says what it is doing.
    assert_eq!(s.live.len(), 2, "{:?}", s.live.len());
    let busiest = &s.live[0];
    assert!(busiest.messages >= 1);
    assert!(busiest.repo.is_some(), "a live row names its repo");
    assert!(busiest.idle_secs < 300);
}

/// The negative cases the review of #330 exposed: output must only be
/// scored when a SHELL call produced it, the tally lives at the END of
/// long output, and `passed;` must count exactly like `passed,`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quality_scoring_only_trusts_shell_output() {
    let kernel = fresh_kernel().await;
    let agent = kernel.create_entity("node_agent").await.expect("agent");
    let session = kernel.create_entity("node_session").await.expect("session");

    let result = |id: &str, body: String| {
        serde_json::json!({"message": {"content": [
            {"type": "tool_result", "tool_use_id": id, "is_error": false, "content": body}]}})
    };
    let call = |id: &str, tool: &str| {
        serde_json::json!({"message": {"model": "claude-fable-5", "content": [
            {"type": "tool_use", "id": id, "name": tool, "input": {"command": "cargo test"}}]}})
    };

    // Real order: the call happens, THEN its result. A FILE whose
    // text mentions tests and errors must score nothing — Read output
    // is a file body, not a report.
    let poison = "// docs: test result: ok. 999 passed; 42 failed\nerror[E0999]: in a comment".to_string();
    log_tool_message(&kernel, &session, &agent, serde_json::json!({
        "message": {"model": "claude-fable-5", "content": [
            {"type": "tool_use", "id": "r1", "name": "Read", "input": {"file_path": "/r/README.md"}}]}})).await;
    log_tool_message(&kernel, &session, &agent, result("r1", poison)).await;

    // A long shell run whose summary sits far past the head, in
    // cargo's semicolon-separated form.
    let mut long: String = (0..500).map(|i| format!("running case {i}\n")).collect();
    long.push_str("test result: ok. 12 passed; 3 failed; 0 ignored\n");
    log_tool_message(&kernel, &session, &agent, call("b1", "Bash")).await;
    log_tool_message(&kernel, &session, &agent, result("b1", long)).await;

    // And the REVERSE order — result before call, as interleaved
    // sidechains produce — must score too, not vanish.
    log_tool_message(&kernel, &session, &agent, result("b2", "1 passed\n".to_string())).await;
    log_tool_message(&kernel, &session, &agent, call("b2", "Bash")).await;

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");

    // ONLY the shell run counted, and its tail was read.
    assert_eq!(s.tests_passed, 13, "12 from the tail + 1 from the reversed pair");
    assert_eq!(s.tests_failed, 3, "`passed;` and `failed;` count alike");
    assert_eq!(s.compile_errors, 0, "a comment in a file is not a diagnostic");

    // The model table has no meaningless row: tool_result messages
    // carry no model and are not attributed.
    assert!(
        s.models.iter().all(|m| m.name != "unknown"),
        "no unknown row: {:?}",
        s.models.iter().map(|m| &m.name).collect::<Vec<_>>()
    );

    // The fixed window is by definition the newest N, so it is never
    // reported as a truncated sample.
    let w = superx_mod_ui::stats::stats_summary(&kernel, 2).await.expect("window");
    assert!(!w.truncated, "the default window is not a truncated range");
    assert_eq!(w.range, "window");
}

/// Churn has two causes and they are separable (#337): a rewrite that
/// follows a human turn is the design moving; one with nobody steering
/// is the agent rewriting its own work. Plus the effort dimension and
/// the cost of waiting.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn churn_is_attributed_and_effort_is_measured() {
    let kernel = fresh_kernel().await;
    let demo = TestRepo::new("demo", "main");
    let agent = kernel.create_entity("node_agent").await.expect("agent");
    let steered = kernel.create_entity("node_session").await.expect("s1");
    let alone = kernel.create_entity("node_session").await.expect("s2");

    let edit = |path: &str, old: &str, new: &str| {
        serde_json::json!({"cwd": demo.cwd(), "effort": "high",
            "message": {"model": "claude-fable-5",
                "usage": {"output_tokens": 90, "output_tokens_details": {"thinking_tokens": 40}},
                "content": [{"type": "tool_use", "id": "e", "name": "Edit",
                    "input": {"file_path": path, "old_string": old, "new_string": new}}]}})
    };

    // Session A: the operator says something, THEN the agent rewrites.
    kernel.log_message(superx_kernel::NewMessage {
        session: steered.clone(), agent: agent.clone(), role: "user".into(),
        content: "actually, do it the other way".into(), raw: None, seq: None, emitted_at: None,
    }).await.expect("human turn");
    log_tool_message(&kernel, &steered, &agent,
        edit(demo.file("a.rs"), "one\ntwo", "three\nfour")).await;

    // Session B: no instruction — the agent is going in circles.
    log_tool_message(&kernel, &alone, &agent,
        edit(demo.file("b.rs"), "alpha\nbeta\ngamma", "delta")).await;

    // A long operation, and a command that had to be stopped.
    kernel.log_message(superx_kernel::NewMessage {
        session: alone.clone(), agent: agent.clone(), role: "system".into(), content: String::new(),
        raw: Some(superx_kernel::message::json_to_object(&serde_json::json!({
            "durationMs": 620000, "slug": "cargo-build", "message": {"content": []}}))),
        seq: None, emitted_at: None,
    }).await.expect("slow op");
    kernel.log_message(superx_kernel::NewMessage {
        session: alone.clone(), agent: agent.clone(), role: "tool".into(), content: String::new(),
        raw: Some(superx_kernel::message::json_to_object(&serde_json::json!({
            "toolUseResult": {"interrupted": true, "stdout": ""}, "message": {"content": []}}))),
        seq: None, emitted_at: None,
    }).await.expect("interrupted");

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");

    // The steered rewrite replaced 2 lines; the unsteered one replaced 3.
    assert_eq!(s.churn_directed, 2, "the design moved — an instruction preceded it");
    assert_eq!(s.churn_self, 3, "nobody was steering — the agent rewrote itself");

    // The repo carries the same split, so "why is this repo churning?"
    // has an answer rather than a number.
    let repo = s.repos.iter().find(|r| r.name == "demo").expect("demo repo");
    assert_eq!(repo.churn_directed, 2);
    assert_eq!(repo.churn_self, 3);

    // Reasoning level against churn and productivity.
    let high = s.efforts.iter().find(|e| e.name == "high").expect("high effort");
    assert_eq!(high.lines_added, 3, "2 + 1 new lines");
    assert_eq!(high.lines_removed, 5, "2 + 3 replaced");
    assert_eq!(high.thinking_tokens, 80, "40 per edit message");
    assert!(high.messages >= 2);

    // What the agents waited on, and what had to be killed.
    assert_eq!(s.wait_ms_total, 620_000, "one ten-minute operation");
    assert_eq!(s.slowest.first().map(|o| o.label.as_str()), Some("cargo-build"));
    assert_eq!(s.interrupted_calls, 1);

    // Both sessions appear on the timeline with real bounds.
    assert_eq!(s.timeline.len(), 2, "{:?}", s.timeline.len());
    assert!(s.timeline.iter().all(|t| !t.start.is_empty() && t.messages > 0));
}

/// Two questions the operator asked of the telemetry (#337): what does
/// each agent cost per line it produced, and how much of my material
/// left this machine? Both are measured, not estimated — the token
/// counts are the vendor's own, and the bytes are the text the tool
/// results carried into the next prompt.
#[tokio::test]
async fn productivity_and_exposure_are_measured_per_agent() {
    let kernel = fresh_kernel().await;
    let other = TestRepo::new("other", "main");
    let superx = TestRepo::new("superx", "main");
    let (fast, fast_s) = seed_agent_and_session(&kernel, "claude_code", "aaa").await;
    let (slow, slow_s) = seed_agent_and_session(&kernel, "gemini_cli", "bbb").await;

    // A productive agent: 40k tokens in, 3 lines out.
    log_tool_message(&kernel, &fast_s, &fast, serde_json::json!({
        "cwd": superx.cwd(),
        "message": {"model": "claude-fable-5",
            "usage": {"input_tokens": 1_000, "cache_creation_input_tokens": 39_000,
                      "cache_read_input_tokens": 500_000, "output_tokens": 700},
            "content": [{"type": "tool_use", "id": "w1", "name": "Write",
                "input": {"file_path": superx.file("a.rs"), "content": "one\ntwo\nthree"}}]}})).await;

    // An expensive one: the same 40k in, a single line out.
    log_tool_message(&kernel, &slow_s, &slow, serde_json::json!({
        "cwd": other.cwd(),
        "message": {"model": "gemini-3-pro",
            "usage": {"input_tokens": 40_000, "output_tokens": 200},
            "content": [{"type": "tool_use", "id": "w2", "name": "Write",
                "input": {"file_path": other.file("b.rs"), "content": "solo"}}]}})).await;

    // Reads: one inside the working directory, one far outside it —
    // and the outside one comes back holding a private key.
    log_tool_message(&kernel, &fast_s, &fast, serde_json::json!({
        "cwd": superx.cwd(),
        "message": {"model": "claude-fable-5", "content": [
            {"type": "tool_use", "id": "r1", "name": "Read",
                "input": {"file_path": superx.file("src/lib.rs")}},
            {"type": "tool_use", "id": "r2", "name": "Read",
                "input": {"file_path": "/home/me/.ssh/id_rsa"}},
            {"type": "tool_use", "id": "r3", "name": "Read",
                "input": {"file_path": "/private/tmp/claude-1/scratchpad/notes.md"}},
            {"type": "image", "source": {"type": "base64"}}]}})).await;
    log_tool_message(&kernel, &fast_s, &fast, serde_json::json!({
        "cwd": superx.cwd(),
        "message": {"model": "claude-fable-5", "content": [
            {"type": "tool_result", "tool_use_id": "r1", "content": "fn main() {}"},
            // Assembled at run time: this file, read, holds no key block.
            {"type": "tool_result", "tool_use_id": "r2",
                "content": format!("-----BEGIN OPENSSH PRIVATE KEY-----\n{}\n",
                    "b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW")}]}})).await;

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");

    // Per agent: the productive one is first, and each carries the
    // tokens it spent so cost-per-line is a division, not a guess.
    let a = s.agent_stats.iter().find(|a| a.name == "claude_code").expect("claude_code");
    let b = s.agent_stats.iter().find(|a| a.name == "gemini_cli").expect("gemini_cli");
    assert_eq!(a.lines_added, 3);
    assert_eq!(b.lines_added, 1);
    assert_eq!(a.in_tokens, 40_000, "fresh prompt + what the vendor cached");
    assert_eq!(b.in_tokens, 40_000, "same spend, a third of the output");
    assert_eq!(a.out_tokens, 700);
    assert_eq!(a.repos, 1);
    assert_eq!(a.sessions, 1);
    assert_eq!(s.agent_stats[0].name, "claude_code", "most productive first");

    // Exposure: what left, and what the vendor kept.
    let e = &s.exposure;
    assert_eq!(e.input_tokens, 41_000, "1k + 40k sent fresh");
    assert_eq!(e.cache_write_tokens, 39_000, "written to the vendor's store");
    assert_eq!(e.cache_read_tokens, 500_000, "served back out of it");
    assert_eq!(e.files_read, 3, "three distinct files pulled into prompts");
    assert_eq!(e.repos_exposed, 1);
    assert_eq!(e.attachments, 1, "one image sent");
    assert!(e.content_bytes >= 12, "the file text the results carried");

    // The two signals that are worth waking up for.
    // ~/.ssh counts; the agent's own scratchpad does not — it is
    // outside the working directory by design, and counting it buries
    // the reads that matter.
    assert_eq!(e.outside_reads, 1, "~/.ssh counts, the scratchpad does not");
    assert_eq!(e.secret_hits, 1);
    assert_eq!(e.secret_paths, vec!["/home/me/.ssh/id_rsa".to_string()],
        "the leak is named, not just counted");
}

/// The work cube (#340): the same totals mean different things
/// depending on WHO did them, WHERE, and WHEN. Two agents in one repo
/// in the same hour is the case a flat series cannot show.
#[tokio::test]
async fn work_is_cubed_by_agent_repo_and_hour() {
    let kernel = fresh_kernel().await;
    let lake = TestRepo::new("lake", "main");
    let superx = TestRepo::new("superx", "main");
    let (a1, s1) = seed_agent_and_session(&kernel, "claude_code", "aaa").await;
    let (a2, s2) = seed_agent_and_session(&kernel, "gemini_cli", "bbb").await;

    let write = |cwd: &str, path: &str, body: &str| serde_json::json!({
        "cwd": cwd, "message": {"model": "claude-fable-5",
            "usage": {"output_tokens": 50},
            "content": [{"type": "tool_use", "id": "w", "name": "Write",
                "input": {"file_path": path, "content": body}}]}});

    // Agent one writes in superx, then crosses into the data lake.
    log_tool_message(&kernel, &s1, &a1, write(superx.cwd(), superx.file("new.rs"), "a\nb\nc")).await;
    log_tool_message(&kernel, &s1, &a1, write(lake.cwd(), lake.file("x.py"), "one")).await;
    // Agent two works the same repo as agent one.
    log_tool_message(&kernel, &s2, &a2, write(superx.cwd(), superx.file("other.rs"), "z")).await;

    // A file that already existed: its oldest event is an Edit.
    log_tool_message(&kernel, &s1, &a1, serde_json::json!({
        "cwd": superx.cwd(), "message": {"content": [{"type": "tool_use", "id": "e", "name": "Edit",
            "input": {"file_path": superx.file("old.rs"), "old_string": "was", "new_string": "is"}}]}})).await;

    // Compaction: the agent stopped for two minutes and resumed with
    // less of its own history.
    kernel.log_message(superx_kernel::NewMessage {
        session: s1.clone(), agent: a1.clone(), role: "system".into(),
        content: "Conversation compacted".into(),
        raw: Some(superx_kernel::message::json_to_object(&serde_json::json!({
            "cwd": superx.cwd(), "subtype": "compact_boundary",
            "compactMetadata": {"trigger": "auto", "preTokens": 1_000_958, "durationMs": 134_803},
            "message": {"content": []}}))),
        seq: None, emitted_at: None,
    }).await.expect("compaction");

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");

    // The cube: one cell per (agent, repo, hour).
    let cell = |agent: &str, repo: &str| {
        s.work_cells.iter().find(|c| c.agent == agent && c.repo == repo)
            .unwrap_or_else(|| panic!("no cell for {agent} in {repo}: {:?}",
                s.work_cells.iter().map(|c| (&c.agent, &c.repo)).collect::<Vec<_>>()))
    };
    // 3 lines from the Write plus the 1-line Edit below — same agent,
    // same repo, same hour, so the same cell.
    assert_eq!(cell("claude_code", "superx").added, 4);
    assert_eq!(cell("claude_code", "superx").removed, 1, "the Edit replaced a line");
    assert_eq!(cell("claude_code", "lake").added, 1);
    assert_eq!(cell("gemini_cli", "superx").added, 1, "same repo, different agent, own cell");
    assert_eq!(cell("claude_code", "superx").files, 2, "new.rs and old.rs");

    // Crossing repos mid-session is thrash when it is frequent.
    // superx → lake → superx: two crossings, and that is the point.
    // An agent that keeps leaving is not progressing in either repo.
    assert_eq!(s.repo_switches, 2);
    let a = s.agent_stats.iter().find(|a| a.name == "claude_code").expect("agent");
    assert_eq!(a.repo_switches, 2);
    assert_eq!(s.agent_stats.iter().find(|a| a.name == "gemini_cli").expect("g").repo_switches, 0);

    // New files against files that already existed.
    assert_eq!(s.files_created, 3, "new.rs, x.py, other.rs");
    assert_eq!(s.files_modified, 1, "old.rs was edited, not created");

    // Compaction, per session and in total.
    assert_eq!(s.compaction_total_ms, Some(134_803), "two minutes of dead time");
    let c = s.compaction_sessions.first().expect("a compacted session");
    assert_eq!(c.count, 1);
    assert_eq!(c.auto, 1);
    assert_eq!(c.manual, 0);
    assert_eq!(c.pre_tokens_max, 1_000_958, "it hit the ceiling");
    assert_eq!(c.agent, "claude_code");
    assert_eq!(a.compactions, 1);
    assert_eq!(a.compaction_ms, 134_803);
}

/// Two numbers that separate a thrashing agent from a moving design
/// (#340): how long code survived before something rewrote it, and how
/// long a write waited for its verification.
#[tokio::test]
async fn survival_and_verification_latency_are_measured() {
    let kernel = fresh_kernel().await;
    let superx = TestRepo::new("superx", "main");
    let (agent, session) = seed_agent_and_session(&kernel, "claude_code", "aaa").await;
    let t0 = chrono::Utc::now() - chrono::Duration::hours(3);

    let at = |mins: i64, body: serde_json::Value| {
        let mut m = superx_kernel::NewMessage {
            session: session.clone(), agent: agent.clone(), role: "assistant".into(),
            content: String::new(),
            raw: Some(superx_kernel::message::json_to_object(&body)),
            seq: None, emitted_at: Some(t0 + chrono::Duration::minutes(mins)),
        };
        m.content = String::new();
        m
    };
    let edit = |old: &str, new: &str| serde_json::json!({
        "cwd": superx.cwd(), "message": {"content": [{"type": "tool_use", "id": "e", "name": "Edit",
            "input": {"file_path": superx.file("a.rs"), "old_string": old, "new_string": new}}]}});
    let shell = |cmd: &str| serde_json::json!({
        "cwd": superx.cwd(), "message": {"content": [{"type": "tool_use", "id": "b", "name": "Bash",
            "input": {"command": cmd}}]}});

    // Snippets must clear the 12-character noise floor `snippet_key`
    // applies — a three-word fragment is not evidence of anything.
    const FIRST: &str = "let value = compute_the_thing();";
    const SECOND: &str = "let value = compute_it_differently();";

    // t+0   the agent writes FIRST
    // t+30  it runs the tests      → 30 minutes before anything is checked
    // t+50  it rewrites FIRST away → FIRST survived 50 minutes
    kernel.log_message(at(0, edit("let value = stub_placeholder();", FIRST))).await.expect("write");
    kernel.log_message(at(30, shell("cargo test --workspace"))).await.expect("verify");
    kernel.log_message(at(50, edit(FIRST, SECOND))).await.expect("rewrite");

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");

    assert_eq!(s.survival_p50_mins, 50, "`first` lived fifty minutes before being replaced");
    assert_eq!(s.edit_to_verify_p50_secs, 1800, "half an hour before anything was checked");
    let repo = s.repos.iter().find(|r| r.name == "superx").expect("repo");
    assert_eq!(repo.survival_p50_mins, 50, "the repo carries its own half-life");
    let a = s.agent_stats.iter().find(|a| a.name == "claude_code").expect("agent");
    assert_eq!(a.edit_to_verify_p50_secs, 1800);
}

/// The live panel says what a session is COSTING and what it is doing
/// to the code (#343). Effort rides its own messages, not the model's,
/// and lines added alone reads `0` for a session deep in a rewrite —
/// so both halves are carried.
#[tokio::test]
async fn a_live_row_carries_effort_and_both_halves_of_the_churn() {
    let kernel = fresh_kernel().await;
    let (a1, s1) = seed_agent_and_session(&kernel, "claude_code", "aaa").await;

    // Three new lines, on a message that names the MODEL and no
    // effort.
    log_tool_message(&kernel, &s1, &a1, serde_json::json!({
        "cwd": "/w/superx",
        "message": {"model": "claude-opus-5", "usage": {"output_tokens": 40},
            "content": [{"type": "tool_use", "id": "w", "name": "Write",
                "input": {"file_path": "/w/superx/new.rs", "content": "a\nb\nc"}}]}})).await;
    // Then two existing lines rewritten into one — a net LOSS of a
    // line, which `lines_added` on its own cannot express — on a
    // message that names the EFFORT and no model.
    //
    // The split is the point: carry effort alongside the model and
    // this row comes back with none, because the message that states
    // the effort has no model to hang it on. Newest-first, so this is
    // the first message the walk sees.
    log_tool_message(&kernel, &s1, &a1, serde_json::json!({
        "cwd": "/w/superx", "effort": "xhigh",
        "message": {"content": [{"type": "tool_use", "id": "e", "name": "Edit",
            "input": {"file_path": "/w/superx/old.rs",
                      "old_string": "one\ntwo", "new_string": "uno"}}]}})).await;
    // Newest of all, and it states an EMPTY effort. A first-sighting
    // latch that accepts `""` would take this as the answer and bury
    // the real one below it — the row would report no effort at all.
    log_tool_message(&kernel, &s1, &a1, serde_json::json!({
        "cwd": "/w/superx", "effort": "", "message": {"content": []}})).await;

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");

    assert_eq!(s.live.len(), 1, "{:?}",
        s.live.iter().map(|l| &l.identity).collect::<Vec<_>>());
    let row = &s.live[0];
    assert_eq!(row.model.as_deref(), Some("claude-opus-5"));
    assert_eq!(row.effort.as_deref(), Some("xhigh"),
        "effort is read off its OWN message, not the model's — and an empty one never masks it");
    assert_eq!(row.lines_added, 4, "3 from the Write, 1 from the Edit's new_string");
    assert_eq!(row.lines_removed, 2, "the Edit replaced two lines");
}

/// Branch is a DIMENSION, not a label (#350). Two branches worked in
/// one repo must separate — and each must carry its own churn split,
/// its own outcome and its own quality, or one cannot be said to be
/// worse than the other.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn churn_and_quality_separate_by_branch() {
    let kernel = fresh_kernel().await;
    let (a1, s1) = seed_agent_and_session(&kernel, "claude_code", "aaa").await;
    let (a2, s2) = seed_agent_and_session(&kernel, "gemini_cli", "bbb").await;
    // One repository, two checkouts: the main one on feat/good and a
    // worktree on feat/bad. Every line carries `gitBranch: feat/good` —
    // Claude Code writes the branch of the directory the session was
    // LAUNCHED in, whatever checkout it works in — so the worktree's rows
    // are only told apart because the branch is read from git (#411).
    let superx = TestRepo::new("superx", "feat/good");
    let bad_tree = superx.worktree("bad", "feat/bad");
    let bad_file = leak(format!("{bad_tree}/b.rs"));

    let write = |cwd: &str, path: &str, body: &str| serde_json::json!({
        "cwd": cwd, "gitBranch": "feat/good",
        "message": {"model": "claude-opus-5", "usage": {"output_tokens": 10},
            "content": [{"type": "tool_use", "id": "w", "name": "Write",
                "input": {"file_path": path, "content": body}}]}});
    let edit = |cwd: &str, path: &str, old: &str, new: &str| serde_json::json!({
        "cwd": cwd, "gitBranch": "feat/good",
        "message": {"content": [{"type": "tool_use", "id": "e", "name": "Edit",
            "input": {"file_path": path, "old_string": old, "new_string": new}}]}});

    // Branch A: writes three lines and never rewrites them.
    log_tool_message(&kernel, &s1, &a1, write(superx.cwd(), superx.file("a.rs"), "a\nb\nc")).await;
    // Branch B, same repo: writes one line then rewrites two, with no
    // human turn behind it — self-churn.
    log_tool_message(&kernel, &s2, &a2, write(bad_tree, bad_file, "x")).await;
    log_tool_message(&kernel, &s2, &a2, edit(bad_tree, bad_file, "one\ntwo", "uno")).await;

    // A failing call on the bad branch, so the failure RATE is
    // exercised: `pct` already scales by 100, and multiplying before
    // it made one failure in a hundred calls read as a hundred.
    log_tool_message(&kernel, &s2, &a2, serde_json::json!({
        "cwd": bad_tree, "gitBranch": "feat/good",
        "message": {"content": [{"type": "tool_use", "id": "b1", "name": "Bash",
            "input": {"command": "ls /nope"}}]}})).await;
    // The verdict rides a LATER message: the walk is newest-first, so
    // the result must be seen before the call it belongs to.
    log_tool_message(&kernel, &s2, &a2, serde_json::json!({
        "cwd": bad_tree, "gitBranch": "feat/good",
        "message": {"content": [
            {"type": "tool_result", "tool_use_id": "b1", "is_error": true}]}})).await;

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");

    // One repo, but TWO branch rows — the separation is the point.
    assert_eq!(s.repos.len(), 1, "both branches are the same checkout");
    let br = |name: &str| {
        s.branches.iter().find(|b| b.branch == name).unwrap_or_else(|| {
            panic!("no row for {name}: {:?}",
                s.branches.iter().map(|b| (&b.repo, &b.branch)).collect::<Vec<_>>())
        })
    };
    assert_eq!(s.branches.len(), 2, "{:?}",
        s.branches.iter().map(|b| &b.branch).collect::<Vec<_>>());

    let good = br("feat/good");
    assert_eq!(good.repo, "superx");
    assert_eq!(good.lines_added, 3);
    assert_eq!(good.lines_removed, 0, "nothing was rewritten here");
    assert_eq!(good.self_churn_pct, 0);
    assert_eq!(good.agents, 1);

    let bad = br("feat/bad");
    assert_eq!(bad.lines_added, 2, "1 from the Write, 1 from the Edit");
    assert_eq!(bad.lines_removed, 2, "the Edit replaced two lines");
    assert_eq!(bad.churn_self, 2, "no human turn preceded it");
    assert_eq!(bad.churn_directed, 0);
    assert_eq!(bad.self_churn_pct, 100, "every replaced line was unasked");
    assert_eq!(bad.rework_pct, 100, "it removed as much as it added");
    // 1 failure across 3 calls is 33 per 100 — not 100, which is what
    // a double scaling produced.
    assert_eq!(bad.tool_calls, 3, "Write, Edit and the Bash");
    assert_eq!(bad.tool_failures, 1);
    assert_eq!(bad.failures_per_100, 33, "one in three, not saturated");

    // Quality ranks them, and the worse branch sorts FIRST — the one
    // with the most to fix is the one to look at.
    assert!(bad.quality_pct >= 0 && good.quality_pct >= 0, "both are scorable");
    assert!(bad.quality_pct < good.quality_pct,
        "bad={} good={}", bad.quality_pct, good.quality_pct);
    assert_eq!(s.branches[0].branch, "feat/bad", "worst first");

    // No tests ran on either, so the pass rate must read as ABSENT
    // rather than as total failure.
    assert_eq!(good.test_pass_pct, -1, "untested is not failed");

    // The agent dimension carries the same split, so agents compare on
    // outcome and not just volume.
    let ga = s.agent_stats.iter().find(|a| a.name == "gemini_cli").expect("gemini");
    assert_eq!(ga.churn_self, 2);
    assert_eq!(ga.churn_directed, 0);
}

/// The outcome plumbing #354's review found untested: a command's
/// tallies must reach the branch, the agent AND the reasoning level
/// that ran it, and a failure must land on the branch whichever of the
/// two resolution arms pairs it with its call.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn command_outcomes_reach_branch_agent_and_effort() {
    let kernel = fresh_kernel().await;
    let superx = TestRepo::new("superx", "feat/x");
    let (a1, s1) = seed_agent_and_session(&kernel, "claude_code", "aaa").await;

    // Output is scored when the CALL resolves the stashed text, so the
    // result must be the NEWER message — logged second, seen first by
    // a newest-first walk.
    log_tool_message(&kernel, &s1, &a1, serde_json::json!({
        "cwd": superx.cwd(), "gitBranch": "feat/x", "effort": "high",
        "message": {"model": "claude-opus-5", "content": [
            {"type": "tool_use", "id": "t1", "name": "Bash",
             "input": {"command": "cargo test --workspace"}}]}})).await;
    log_tool_message(&kernel, &s1, &a1, serde_json::json!({
        "cwd": superx.cwd(), "gitBranch": "feat/x",
        "message": {"content": [{"type": "tool_result", "tool_use_id": "t1",
            "is_error": false,
            "content": "test result: ok. 7 passed; 2 failed; 0 ignored"}]}})).await;

    // The OTHER arm, which is where the bug was: the CALL is seen first
    // — so it is the newer message — and the verdict then resolves
    // through `call_names`. That arm attributed to agents, models and
    // repos but not branches, so a failing branch reported a clean
    // failure rate and scored full marks on tool success.
    log_tool_message(&kernel, &s1, &a1, serde_json::json!({
        "cwd": superx.cwd(), "gitBranch": "feat/x",
        "message": {"content": [{"type": "tool_result", "tool_use_id": "t2",
            "is_error": true}]}})).await;
    log_tool_message(&kernel, &s1, &a1, serde_json::json!({
        "cwd": superx.cwd(), "gitBranch": "feat/x",
        "message": {"model": "claude-opus-5", "content": [
            {"type": "tool_use", "id": "t2", "name": "Bash",
             "input": {"command": "cargo build"}}]}})).await;

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");

    let b = s.branches.iter().find(|b| b.branch == "feat/x").expect("branch row");
    assert_eq!(b.tests_passed, 7, "the tally reached the branch");
    assert_eq!(b.tests_failed, 2);
    assert_eq!(b.test_pass_pct, 77, "7 of 9");
    assert_eq!(b.tests_run, 1, "one test invocation, so -1 could not mean 'never ran'");
    // The point of the fix: the failure that resolved through the OTHER
    // arm is on the branch, not silently dropped.
    assert_eq!(b.tool_failures, 1, "the call-first failure reached the branch");
    assert!(b.tool_calls >= 2, "calls = {}", b.tool_calls);

    let a = s.agent_stats.iter().find(|a| a.name == "claude_code").expect("agent");
    assert_eq!(a.tests_passed, 7, "and the agent, so agents compare on outcome");
    assert_eq!(a.tests_failed, 2);

    let e = s.efforts.iter().find(|e| e.name == "high").expect("effort row");
    assert_eq!(e.tests_passed, 7, "efforts carried these fields and nothing ever set them");
    assert_eq!(e.tests_failed, 2);
}

// ── the cockpit (#367) ──────────────────────────────────────────────

/// The landing range is a substrate decision, not a literal in the
/// page (#367, §9).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_range_defaults_then_follows_the_parameter() {
    let kernel = fresh_kernel().await;
    assert_eq!(resolved_default_range(&kernel).await, DEFAULT_RANGE, "unregistered → default");
    let entity = kernel
        .register_module(&UiModule.descriptor())
        .await
        .expect("register");
    assert_eq!(resolved_default_range(&kernel).await, DEFAULT_RANGE, "no param → default");
    kernel
        .set_parameter(
            entity.clone(),
            DEFAULT_RANGE_PARAM,
            superx_kernel::types::Value::String("7d".into()),
        )
        .await
        .expect("param");
    assert_eq!(resolved_default_range(&kernel).await, "7d", "parameter wins");
    kernel
        .set_parameter(
            entity,
            DEFAULT_RANGE_PARAM,
            superx_kernel::types::Value::String("fortnight".into()),
        )
        .await
        .expect("param");
    assert_eq!(
        resolved_default_range(&kernel).await,
        DEFAULT_RANGE,
        "a range the API does not know falls back rather than 400ing every load"
    );
}

/// One `LIMIT 20000` reset the connection on `30d` and `all`. The
/// paged walk must return the same rows — newest first, none lost,
/// none twice — including rows that share the boundary instant of a
/// page, and it must honour the cap and the time bound (#367).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn range_walk_pages_without_losing_or_repeating_rows() {
    use superx_mod_ui::stats::walk_messages;
    let kernel = fresh_kernel().await;
    let (agent, session) = seed_agent_and_session(&kernel, "claude_code", "walk").await;
    for i in 0..7 {
        log_tool_message(&kernel, &session, &agent, serde_json::json!({"seq": i, "message": {"content": []}}))
            .await;
    }
    // Three rows at ONE instant, straddling a page boundary of two.
    kernel
        .db()
        .query(
            "LET $t = time::now() + 1h;
             CREATE message SET session = $s, agent = $a, role = 'assistant', content = '', valid_from = $t;
             CREATE message SET session = $s, agent = $a, role = 'assistant', content = '', valid_from = $t;
             CREATE message SET session = $s, agent = $a, role = 'assistant', content = '', valid_from = $t;",
        )
        .bind(("s", session.clone()))
        .bind(("a", agent.clone()))
        .await
        .expect("seed")
        .check()
        .expect("ok");

    let all = walk_messages(&kernel, None, 100, 2).await.expect("walk");
    assert_eq!(all.len(), 10, "seven logged + three at one instant, every one of them once");
    let ids: std::collections::HashSet<String> =
        all.iter().map(|m| superx_ops::record_uuid(&m.id)).collect();
    assert_eq!(ids.len(), 10, "no row twice");
    assert!(
        all.windows(2).all(|w| w[0].valid_from >= w[1].valid_from),
        "newest first across page joins"
    );

    let capped = walk_messages(&kernel, None, 5, 2).await.expect("walk");
    assert_eq!(capped.len(), 5, "the cap holds across pages");
    // The engine's order among rows at one instant is its own; what
    // must hold is that the newest instant's three rows are all there.
    let newest = all[0].valid_from;
    assert_eq!(
        capped.iter().filter(|m| m.valid_from == newest).count(),
        3,
        "the newest five begin with the three rows at the newest instant"
    );
    assert!(
        capped.iter().all(|m| ids.contains(&superx_ops::record_uuid(&m.id))),
        "and every one is a row of the full walk"
    );

    let future = chrono::Utc::now() + chrono::Duration::hours(2);
    let none = walk_messages(&kernel, Some(future), 100, 2).await.expect("walk");
    assert!(none.is_empty(), "the time bound applies to every page");

    // The same walk through the public aggregation on a bounded range.
    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");
    assert_eq!(s.range, "24h");
    assert!(!s.truncated);
}

/// An agent that reads through the shell is reading. `cat | head` is
/// a read with a path; `sed -i` is not a read; `echo` is nothing; and a
/// chain that ends in `cargo test` is a verification (#367).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_inspection_counts_as_reading() {
    let kernel = fresh_kernel().await;
    let superx = TestRepo::new("superx", "main");
    let (a1, s1) = seed_agent_and_session(&kernel, "claude_code", "reader").await;
    let (a2, s2) = seed_agent_and_session(&kernel, "claude_code", "verifier").await;
    let shell = |cmd: &str| {
        serde_json::json!({"cwd": superx.cwd(), "message": {"content": [
            {"type": "tool_use", "id": "b", "name": "Bash", "input": {"command": cmd}}]}})
    };
    log_tool_message(&kernel, &s1, &a1, shell("sed -i 's/a/b/' src/x.rs")).await;
    log_tool_message(&kernel, &s1, &a1, shell("echo '--- doing ---'")).await;
    log_tool_message(&kernel, &s1, &a1, shell("cat crates/foo.rs | head -20")).await;
    log_tool_message(&kernel, &s2, &a2, shell("cat /other/repo/secret.txt")).await;
    log_tool_message(&kernel, &s2, &a2, shell("cat README.md && cargo test --workspace")).await;

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");

    assert_eq!(s.reads_window, 2, "cat|head and the lone cat; not sed -i, not echo, not the test chain");
    assert_eq!(s.tests_run, 1);
    let e = &s.exposure;
    assert_eq!(e.files_read, 2, "the two paths the reads named");
    assert_eq!(e.outside_reads, 1, "/other/repo is not the directory the agent was working in");

    let reader = s.live.iter().find(|l| l.identity == superx_ops::record_uuid(&s1)).expect("reader row");
    // The reader also ran `sed -i`, which is a write (#374), and a write
    // outranks a read in the state ladder — the row says what the
    // session changed, not only what it looked at.
    assert_eq!(reader.doing, "writing");
    assert_eq!(
        reader.files_now,
        vec![superx.file("crates/foo.rs").to_string(), superx.file("src/x.rs").to_string()],
        "relative paths, resolved against cwd — the read first (newest), then the in-place edit"
    );
    assert_eq!(s.writes_window, 1, "sed -i is the one write");
    let verifier = s.live.iter().find(|l| l.identity == superx_ops::record_uuid(&s2)).expect("verifier row");
    assert_eq!(verifier.doing, "verifying", "the test run outranks the read");

    // The sortie log names the repo each session flew in (#367).
    assert_eq!(s.timeline.len(), 2);
    assert!(
        s.timeline.iter().all(|t| t.repo.as_deref() == Some("superx")),
        "{:?}",
        s.timeline.iter().map(|t| t.repo.clone()).collect::<Vec<_>>()
    );
}

/// `<synthetic>` is the runtime writing a line itself. It is not a
/// model, and must not top the live panel or the model table (#367).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn synthetic_is_not_a_model() {
    let kernel = fresh_kernel().await;
    let (a, s1) = seed_agent_and_session(&kernel, "claude_code", "syn").await;
    log_tool_message(&kernel, &s1, &a, serde_json::json!({
        "message": {"model": "claude-opus-5", "usage": {"output_tokens": 30}, "content": []}})).await;
    // Newest, so a first-sighting latch would take it.
    log_tool_message(&kernel, &s1, &a, serde_json::json!({
        "message": {"model": "<synthetic>", "usage": {"output_tokens": 0}, "content": []}})).await;

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");
    assert!(s.models.iter().all(|m| m.name != "<synthetic>"), "{:?}", s.models);
    assert_eq!(s.models.iter().find(|m| m.name == "claude-opus-5").map(|m| m.messages), Some(1));
    assert_eq!(s.live.len(), 1);
    assert_eq!(s.live[0].model.as_deref(), Some("claude-opus-5"), "the synthetic line does not mask the model");
}

/// The repeat signal reads the whole command line. `echo` a hundred
/// times is scaffolding; the same `cargo test -p …` three times is the
/// shape of fighting something (#367).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn top_repeat_reads_whole_command_lines() {
    let kernel = fresh_kernel().await;
    let (a, s1) = seed_agent_and_session(&kernel, "claude_code", "rep").await;
    let shell = |cmd: &str| {
        serde_json::json!({"message": {"content": [
            {"type": "tool_use", "id": "b", "name": "Bash", "input": {"command": cmd}}]}})
    };
    for _ in 0..5 {
        log_tool_message(&kernel, &s1, &a, shell("echo '--- section ---'")).await;
    }
    log_tool_message(&kernel, &s1, &a, shell("git status")).await;
    log_tool_message(&kernel, &s1, &a, shell("git status")).await;
    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");
    assert!(s.top_repeat.is_none(), "five echoes and two git status are not a fight: {:?}", s.top_repeat);
    assert_eq!(s.commands.iter().find(|c| c.name == "echo").map(|c| c.value), Some(5), "echo still counts in the mix");

    for _ in 0..3 {
        log_tool_message(&kernel, &s1, &a, shell("echo start;  cargo test -p superx-mod-ui   2>&1 | tail -3")).await;
    }
    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");
    let r = s.top_repeat.expect("three identical lines");
    assert_eq!(r.name, "cargo test -p superx-mod-ui && tail -3", "noise dropped, redirection dropped, whitespace folded");
    assert_eq!(r.value, 3);
}

/// Context pressure per live agent — the altitude gauge (#367).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_rows_carry_context_pressure() {
    let kernel = fresh_kernel().await;
    let (a, s1) = seed_agent_and_session(&kernel, "claude_code", "ctx").await;
    log_tool_message(&kernel, &s1, &a, serde_json::json!({
        "message": {"model": "claude-opus-5", "usage": {"input_tokens": 100_000,
            "cache_read_input_tokens": 300_000, "cache_creation_input_tokens": 50_000,
            "output_tokens": 10}, "content": []}})).await;
    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");
    assert_eq!(s.live.len(), 1);
    assert_eq!(s.live[0].context_tokens, Some(450_000), "fresh + served + stored");
    assert_eq!(s.live[0].context_pct, Some(45), "against the default 1M window");

    let entity = kernel.register_module(&UiModule.descriptor()).await.expect("register");
    kernel
        .set_parameter(entity, CONTEXT_WINDOW_PARAM, superx_kernel::types::Value::Number(500_000.into()))
        .await
        .expect("param");
    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");
    assert_eq!(s.live[0].context_pct, Some(90), "the ceiling is the parameter");

    // A session that never reported usage reads as unknown, not zero.
    let (b, s2) = seed_agent_and_session(&kernel, "gemini_cli", "noctx").await;
    log_tool_message(&kernel, &s2, &b, serde_json::json!({"message": {"content": []}})).await;
    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");
    let row = s.live.iter().find(|l| l.identity == superx_ops::record_uuid(&s2)).expect("row");
    assert_eq!(row.context_tokens, None);
    assert_eq!(row.context_pct, None);
}

/// The substrate held every module failure and nothing read them.
/// Per module: newest event and its age, failures recent and total,
/// and what the newest failure said (#367).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn module_health_reads_failures_and_last_event() {
    use superx_mod_ui::insights::insights_summary;
    let kernel = fresh_kernel().await;
    kernel
        .db()
        .query(
            "CREATE telemetry_stream SET lifecycle_event = 'module_start_failed',
                payload = { name: 'runner' }, valid_from = time::now() - 3d;
             CREATE telemetry_stream SET lifecycle_event = 'module_failed',
                payload = { name: 'runner', error: 'boom', failed_during: 'startup' },
                valid_from = time::now() - 1h;
             CREATE telemetry_stream SET lifecycle_event = 'module_active',
                payload = { name: 'ui', startup_duration_ms: 12 }, valid_from = time::now();",
        )
        .await
        .expect("seed")
        .check()
        .expect("ok");

    let i = insights_summary(&kernel).await.expect("insights");
    let runner = i.module_health.iter().find(|h| h.name == "runner").expect("runner");
    assert_eq!(runner.last_event, "module_failed");
    assert!((3000..4000).contains(&runner.last_event_secs), "about an hour: {}", runner.last_event_secs);
    assert_eq!(runner.failures_recent, 1, "the three-day-old one is not recent");
    assert_eq!(runner.failures_total, 2);
    assert_eq!(runner.last_error.as_deref(), Some("boom"));
    let ui = i.module_health.iter().find(|h| h.name == "ui").expect("ui");
    assert_eq!(ui.last_event, "module_active");
    assert_eq!(ui.failures_total, 0);
    assert_eq!(ui.last_error, None);
    assert_eq!(i.module_health[0].name, "runner", "the module with the most to fix sorts first");
}

/// A message without a model — a tool_result, or an assistant turn
/// whose model field is missing — attributes NOTHING to a model row.
/// Half of #330's fix had landed: the message count skipped the
/// sentinel while lines, tokens, failures and reverts still went to an
/// `unknown` row with zero messages (#345).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_message_without_a_model_attributes_nothing_to_a_model_row() {
    let kernel = fresh_kernel().await;
    let agent = kernel.create_entity("node_agent").await.expect("agent");
    let session = kernel.create_entity("node_session").await.expect("session");

    // A model-less turn writes a line, spends tokens and runs a shell
    // call that will fail…
    log_tool_message(&kernel, &session, &agent, serde_json::json!({
        "cwd": "/w/superx",
        "message": {"usage": {"output_tokens": 500}, "content": [
            {"type": "tool_use", "id": "n1", "name": "Edit",
             "input": {"file_path": "/w/superx/src/b.rs",
                       "old_string": "seed text that will be replaced later",
                       "new_string": "keep this line here"}},
            {"type": "tool_use", "id": "n2", "name": "Bash", "input": {"command": "cargo test"}}]}
    })).await;
    log_tool_message(&kernel, &session, &agent, serde_json::json!({
        "message": {"content": [
            {"type": "tool_result", "tool_use_id": "n2", "is_error": true, "content": "boom"}]}
    })).await;
    // …and a real model then throws that line away — a revert that
    // belongs to the model-less author, i.e. to nobody.
    log_tool_message(&kernel, &session, &agent, serde_json::json!({
        "cwd": "/w/superx",
        "message": {"model": "claude-opus-5", "usage": {"output_tokens": 30}, "content": [
            {"type": "tool_use", "id": "m1", "name": "Edit",
             "input": {"file_path": "/w/superx/src/b.rs",
                       "old_string": "keep this line here", "new_string": "a\nb\nc"}}]}
    })).await;

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");

    // The work itself is all counted…
    assert_eq!(s.lines_added, 4, "1 + 3");
    assert_eq!(s.lines_removed, 2, "1 + 1");
    assert_eq!(s.reverts, 1, "the model-less line was thrown away");

    // …but only the model that exists has a row, carrying only its own.
    let names: Vec<&str> = s.models.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, vec!["claude-opus-5"], "no sentinel row");
    let opus = &s.models[0];
    assert_eq!((opus.messages, opus.lines_added, opus.lines_removed), (1, 3, 1));
    assert_eq!(opus.out_tokens, 30, "the 500 model-less tokens went to no row");
    assert_eq!(opus.tool_failures, 0, "the model-less failure went to no row");
    assert_eq!(opus.reverts, 0, "the model-less revert went to no row");

    // The live row names the model that was seen, never the sentinel.
    assert_eq!(s.live.len(), 1);
    assert_eq!(s.live[0].model.as_deref(), Some("claude-opus-5"));
}

/// NotebookEdit was a write tool that wrote nothing: its text rides
/// `new_source` and its file `notebook_path`, names neither counter
/// nor path reader knew, so a session rewriting notebook cells read
/// as idle (#346). Its replaced half is unknown by construction and
/// is reported as none, not invented.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notebook_edits_count_their_lines_and_their_file() {
    let kernel = fresh_kernel().await;
    let superx = TestRepo::new("superx", "main");
    let agent = kernel.create_entity("node_agent").await.expect("agent");
    let session = kernel.create_entity("node_session").await.expect("session");
    let nb = superx.file("nb/analysis.ipynb");

    for input in [
        serde_json::json!({"notebook_path": nb, "cell_id": "c1", "edit_mode": "replace",
                           "new_source": "import x\ny = 1\nprint(y)"}),
        serde_json::json!({"notebook_path": nb, "cell_id": "c1", "edit_mode": "insert",
                           "cell_type": "markdown", "new_source": "# title"}),
        serde_json::json!({"notebook_path": nb, "cell_id": "c2", "edit_mode": "delete",
                           "new_source": ""}),
        // The default edit_mode is replace.
        serde_json::json!({"notebook_path": nb, "cell_id": "c3", "new_source": "z = 2"}),
    ] {
        log_tool_message(&kernel, &session, &agent, serde_json::json!({
            "cwd": superx.cwd(),
            "message": {"model": "claude-opus-5", "content": [
                {"type": "tool_use", "id": "e", "name": "NotebookEdit", "input": input}]}
        })).await;
    }
    log_tool_message(&kernel, &session, &agent, serde_json::json!({
        "cwd": superx.cwd(),
        "message": {"model": "claude-opus-5", "content": [
            {"type": "tool_use", "id": "r", "name": "NotebookRead", "input": {"notebook_path": nb}}]}
    })).await;

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");

    assert_eq!(s.lines_added, 5, "3 replaced-in + 1 inserted + 0 deleted + 1 default-replace");
    assert_eq!(s.lines_removed, 0, "the prior cell text is not in the call — none invented");
    assert_eq!(s.replaced_unknown, 3, "replace, delete and the default replace; not the insert (#383)");
    assert_eq!(s.writes_window, 4, "every NotebookEdit is a write, the delete included");
    assert_eq!(s.reads_window, 1);

    // The notebook is a file the agent touched, in its language, and
    // it existed before the window — a cell edit is not a file created.
    assert_eq!(s.files_touched, 1, "one notebook, however many cells");
    // Languages count the edits, as they do for every other extension —
    // the read is not one (#412).
    assert_eq!(s.languages.iter().find(|l| l.name == "ipynb").map(|l| l.value), Some(4));
    assert_eq!((s.files_created, s.files_modified), (0, 1));

    // The live row shows the notebook under the agent's hands.
    assert_eq!(s.live.len(), 1);
    assert_eq!(s.live[0].lines_added, 5);
    assert_eq!(s.live[0].files_now, vec![nb.to_string()]);
}

/// One captured message with an explicit agent-clock timestamp — the
/// shape a backfill produces: `emitted_at` days ago, `valid_from` now.
async fn log_tool_message_at(kernel: &Kernel, session: &superx_kernel::types::RecordId,
                             agent: &superx_kernel::types::RecordId, raw: serde_json::Value,
                             emitted: chrono::DateTime<chrono::Utc>) {
    kernel
        .log_message(superx_kernel::NewMessage {
            session: session.clone(),
            agent: agent.clone(),
            role: "assistant".to_string(),
            content: String::new(),
            raw: Some(superx_kernel::message::json_to_object(&raw)),
            seq: None,
            emitted_at: Some(emitted),
        })
        .await
        .expect("message");
}

/// A throwaway git repository for the tests that need a real checkout
/// (#411): a repository's name, its branches and its worktrees are read
/// from git, not from the last folder of a path. Canonical from the start
/// — macOS keeps the temp dir behind a symlink and git answers with the
/// real path. Removed when dropped.
struct TestRepo {
    base: std::path::PathBuf,
    root: std::path::PathBuf,
}

impl TestRepo {
    /// A repository called `name`, on `branch`, with one commit.
    fn new(name: &str, branch: &str) -> Self {
        Self::new_dated(name, branch, chrono::Utc::now())
    }

    /// The same, with the first commit made at `when`.
    fn new_dated(name: &str, branch: &str, when: chrono::DateTime<chrono::Utc>) -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let tmp = std::fs::canonicalize(std::env::temp_dir()).expect("temp dir");
        let base = tmp.join(format!(
            "superx-repo-{}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let root = base.join(name);
        std::fs::create_dir_all(&root).expect("repo dir");
        let repo = Self { base, root };
        repo.git(&["init", "-q", "-b", branch]);
        // The identity this machine's agents commit as, in the repo's own
        // config — which is where the comparison reads it (#414).
        repo.git(&["config", "user.email", "t@t"]);
        repo.git(&["config", "user.name", "t"]);
        // Worktrees live inside the checkout, and are ignored there — as
        // a real repository's `.claude/worktrees/` is.
        std::fs::write(repo.root.join(".git/info/exclude"), ".claude/worktrees/\n").expect("exclude");
        let root = repo.cwd();
        repo.commit(root, when, "t@t", "init", &[("README", "x\n")]);
        repo
    }

    /// Write `files` into the checkout at `dir` and commit them at `when`
    /// as `email` — author and committer time both, as a real commit has.
    fn commit(&self, dir: &str, when: chrono::DateTime<chrono::Utc>, email: &str, msg: &str,
              files: &[(&str, &str)]) {
        for (rel, body) in files {
            let path = std::path::Path::new(dir).join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("dir");
            }
            std::fs::write(&path, body).expect("write");
        }
        let stamp = format!("@{} +0000", when.timestamp());
        let run = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C").arg(dir)
                .args(["-c", &format!("user.email={email}"), "-c", "user.name=a", "-c", "commit.gpgsign=false"])
                .args(args)
                .env("GIT_AUTHOR_DATE", &stamp)
                .env("GIT_COMMITTER_DATE", &stamp)
                .output()
                .expect("git runs");
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        run(&["add", "-A"]);
        run(&["commit", "-q", "-m", msg]);
    }

    fn git(&self, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
            .args(args)
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    /// The checkout's root, as a transcript's `cwd` carries it. Paths are
    /// leaked into `&'static str` so a fixture closure can take them as
    /// the `&str` a transcript field is — a test process is short-lived.
    fn cwd(&self) -> &'static str {
        leak(self.root.to_string_lossy().into_owned())
    }

    /// A subdirectory of the checkout, created.
    fn sub(&self, rel: &str) -> &'static str {
        let p = self.root.join(rel);
        std::fs::create_dir_all(&p).expect("subdir");
        leak(p.to_string_lossy().into_owned())
    }

    /// A file path inside the checkout.
    fn file(&self, rel: &str) -> &'static str {
        leak(self.root.join(rel).to_string_lossy().into_owned())
    }

    /// A linked worktree on a new branch, where this repository's agents
    /// keep them: `.claude/worktrees/<name>`.
    fn worktree(&self, name: &str, branch: &str) -> &'static str {
        let path = self.root.join(".claude/worktrees").join(name);
        self.git(&["worktree", "add", "-q", "-b", branch, &path.to_string_lossy()]);
        leak(path.to_string_lossy().into_owned())
    }
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

impl Drop for TestRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

/// Range membership follows the agent's clock. A restart that backfills
/// six days of transcripts captures them within a minute; that work
/// must not all land inside "last hour", and a failure at 17:00 agent
/// time belongs in the 17:00 bucket, not the capture hour (#372).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn range_membership_follows_the_agents_clock_not_the_capture_run() {
    let kernel = fresh_kernel().await;
    let agent = kernel.create_entity("node_agent").await.expect("agent");
    let session = kernel.create_entity("node_session").await.expect("session");
    let now = chrono::Utc::now();
    let edit = |id: &str, lines: &str| serde_json::json!({
        "cwd": "/w/superx",
        "message": {"model": "claude-opus-5", "content": [
            {"type": "tool_use", "id": id, "name": "Edit",
             "input": {"file_path": "/w/superx/src/a.rs", "old_string": "x", "new_string": lines}}]}
    });

    // Backfilled: the work happened three days ago, captured just now.
    log_tool_message_at(&kernel, &session, &agent, edit("old", "a\nb\nc\nd\ne"),
                        now - chrono::Duration::days(3)).await;
    // Live: ten minutes ago.
    log_tool_message_at(&kernel, &session, &agent, edit("new", "p\nq"),
                        now - chrono::Duration::minutes(10)).await;
    // A shell call that failed at 17:00 yesterday, agent time.
    let seventeen = chrono::Timelike::with_hour(&(now - chrono::Duration::days(1)), 17).expect("17:00");
    log_tool_message_at(&kernel, &session, &agent, serde_json::json!({
        "cwd": "/w/superx",
        "message": {"model": "claude-opus-5", "content": [
            {"type": "tool_use", "id": "f1", "name": "Bash", "input": {"command": "cargo test"}}]}
    }), seventeen).await;
    log_tool_message_at(&kernel, &session, &agent, serde_json::json!({
        "message": {"content": [
            {"type": "tool_result", "tool_use_id": "f1", "is_error": true, "content": "boom"}]}
    }), seventeen + chrono::Duration::seconds(5)).await;

    let hour = superx_mod_ui::stats::stats_for_range(&kernel, 500, "1h").await.expect("1h");
    assert_eq!(hour.lines_added, 2, "only the work that happened in the last hour");
    assert!(!hour.truncated);
    let week = superx_mod_ui::stats::stats_for_range(&kernel, 500, "7d").await.expect("7d");
    assert_eq!(week.lines_added, 7, "both edits happened inside seven days");
    let all = superx_mod_ui::stats::stats_for_range(&kernel, 500, "all").await.expect("all");
    assert_eq!(all.lines_added, 7);

    // The failure sits in the hour the agent saw it.
    let bucket = week.fail_by_hour.iter().find(|h| h.failures > 0).expect("a failure bucket");
    assert_eq!(bucket.hour, 17, "{:?}", week.fail_by_hour);
    assert_eq!(week.fail_by_hour.iter().map(|h| h.failures).sum::<i64>(), 1);
}

/// The fixed window is by definition the newest N and never a sample.
/// Every other range is one the moment the row cap cuts it short —
/// `all` included, which has no cutoff but has the cap, and used to
/// report "every row in range" over a 20,000-row sample of 39,000
/// (#372).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_range_but_the_window_admits_its_row_cap() {
    let kernel = fresh_kernel().await;
    let agent = kernel.create_entity("node_agent").await.expect("agent");
    let session = kernel.create_entity("node_session").await.expect("session");
    for _ in 0..3 {
        log_tool_message(&kernel, &session, &agent, serde_json::json!({
            "message": {"model": "claude-opus-5", "content": []}})).await;
    }
    let s = |range: &'static str, cap: u32| {
        superx_mod_ui::stats::stats_for_range_capped(&kernel, 2, range, cap, chrono::Offset::fix(&chrono::Utc))
    };
    assert!(s("all", 2).await.expect("all").truncated, "three rows, cap two: a sample");
    assert!(s("24h", 2).await.expect("24h").truncated);
    assert!(!s("all", 20).await.expect("all").truncated, "room for every row");
    assert!(!s("window", 2).await.expect("window").truncated, "the newest N is never a sample");
}

/// Edits made through the shell are writes (#374). Under an operating
/// mode that edits with `python3 - <<EOF`, `cat > file <<EOF` and
/// `sed -i`, a session read `—` while it rewrote four files. The
/// heredoc that goes straight into a file has its lines on the line;
/// every other shell write is of unknown size and reports none — never
/// zero by omission. Output capture, sinks and git are not writes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_edits_count_as_writing() {
    let kernel = fresh_kernel().await;
    let agent = kernel.create_entity("node_agent").await.expect("agent");
    let session = kernel.create_entity("node_session").await.expect("session");
    let shell = |cmd: &str| serde_json::json!({
        "cwd": "/w/superx",
        "message": {"model": "claude-opus-5", "content": [
            {"type": "tool_use", "id": "b", "name": "Bash", "input": {"command": cmd}}]}
    });
    // A whole file through a heredoc: the lines are on the line.
    log_tool_message(&kernel, &session, &agent,
        shell("cat > src/gen.rs <<'EOF'\nfn a() {}\nfn b() {}\nfn c() {}\nEOF")).await;
    // A script that rewrites a file: a write of unknown size, naming its file.
    log_tool_message(&kernel, &session, &agent,
        shell("python3 - <<'PYEOF'\nimport pathlib\np = pathlib.Path('/w/superx/src/stats.rs')\ns = p.read_text()\np.write_text(s.replace('a', 'b'))\nPYEOF")).await;
    // In place, relative to cwd, chained with a verification.
    log_tool_message(&kernel, &session, &agent, shell("sed -i 's/old/new/' src/lib.rs && cargo test -p x")).await;
    // Not writes: an inspection with a sink, output captured to scratch, git.
    log_tool_message(&kernel, &session, &agent, shell("cat src/lib.rs | grep foo > /dev/null")).await;
    log_tool_message(&kernel, &session, &agent, shell("cargo test --workspace > /tmp/out.txt 2>&1")).await;
    log_tool_message(&kernel, &session, &agent, shell("git commit -m x && git push")).await;

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");
    assert_eq!(s.writes_window, 3, "the heredoc, the script and sed -i");
    assert_eq!(s.lines_added, 3, "only the heredoc's lines are on the line");
    assert_eq!(s.lines_removed, 0, "what a shell edit replaced is unknown, not zero");
    assert_eq!(s.replaced_unknown, 3, "and the page can say so: three edits of unknown replaced size (#383)");
    assert_eq!(s.reads_window, 1, "the piped grep is reading; a sink is not a file");
    assert_eq!(s.tests_run, 2, "both cargo test runs still count");

    // The files under the agent's hands, absolute, scratch excluded.
    let files: Vec<&str> = s.files.iter().map(|f| f.name.as_str()).collect();
    for f in ["/w/superx/src/gen.rs", "/w/superx/src/stats.rs", "/w/superx/src/lib.rs"] {
        assert!(files.contains(&f), "{f} missing from {files:?}");
    }
    assert!(!files.iter().any(|f| f.starts_with("/tmp") || f.starts_with("/dev")), "{files:?}");
    assert_eq!(s.languages.iter().find(|l| l.name == "rs").map(|l| l.value), Some(3), "the three writes; a shell read feeds exposure, not languages");

    // The live row shows the work.
    assert_eq!(s.live.len(), 1);
    let row = &s.live[0];
    assert_eq!(row.lines_added, 3);
    assert_eq!(row.replaced_unknown, 3);
    for f in ["/w/superx/src/gen.rs", "/w/superx/src/stats.rs", "/w/superx/src/lib.rs"] {
        assert!(row.files_now.iter().any(|x| x == f), "{f} not on the live row: {:?}", row.files_now);
    }
}

/// What shipped is read from the shell (#381): commits, pushes, PRs
/// opened and merged, and the lines git said a commit carried — in
/// either order of call and result, since sidechains interleave. A
/// quiet commit is counted and carries no lines; looking at a PR and a
/// local merge ship nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shipping_is_read_from_the_shell() {
    let kernel = fresh_kernel().await;
    let superx = TestRepo::new("superx", "main");
    let agent = kernel.create_entity("node_agent").await.expect("agent");
    let session = kernel.create_entity("node_session").await.expect("session");
    let call = |id: &str, cmd: &str| serde_json::json!({
        "cwd": superx.cwd(),
        "message": {"model": "claude-opus-5", "content": [
            {"type": "tool_use", "id": id, "name": "Bash", "input": {"command": cmd}}]}
    });
    let result = |id: &str, body: &str| serde_json::json!({
        "message": {"content": [
            {"type": "tool_result", "tool_use_id": id, "is_error": false, "content": body}]}
    });
    // Real order: the call, then what it printed.
    log_tool_message(&kernel, &session, &agent, call("c1", "git add crates/x.rs && git commit -m 'one'")).await;
    log_tool_message(&kernel, &session, &agent,
        result("c1", "[feat/x 5db4a18f] one\n 3 files changed, 138 insertions(+), 28 deletions(-)\n")).await;
    // A quiet commit prints nothing: counted, no lines.
    log_tool_message(&kernel, &session, &agent, call("c2", "git commit -q -m 'two'")).await;
    log_tool_message(&kernel, &session, &agent, result("c2", "")).await;
    // Reversed — the result before its call, as a sidechain lands it.
    log_tool_message(&kernel, &session, &agent, result("c3", "[feat/x 9c4d6cc5] three\n 1 file changed, 12 insertions(+)\n")).await;
    log_tool_message(&kernel, &session, &agent, call("c3", "git commit -m 'three'")).await;
    log_tool_message(&kernel, &session, &agent, call("p1", "git push -u origin HEAD")).await;
    log_tool_message(&kernel, &session, &agent, result("p1", "branch 'feat/x' set up to track 'origin/feat/x'.\n")).await;
    log_tool_message(&kernel, &session, &agent, call("g1", "gh pr create --base main --title t --body b")).await;
    log_tool_message(&kernel, &session, &agent, result("g1", "https://github.com/o/r/pull/379\n")).await;
    log_tool_message(&kernel, &session, &agent, call("g2", "gh pr merge 379 --squash --admin")).await;
    log_tool_message(&kernel, &session, &agent, result("g2", "✓ Squashed and merged pull request o/r#379\n")).await;
    // Not shipping: looking at a PR, a local merge.
    log_tool_message(&kernel, &session, &agent, call("v1", "gh pr view 379 --json state")).await;
    log_tool_message(&kernel, &session, &agent, call("v2", "git merge --no-edit origin/main")).await;
    // Not shipped either, whatever the command says (#412): a merge the
    // branch policy refused, a create that failed, nothing to commit, and
    // a push the auto-mode classifier stopped before it ran.
    log_tool_message(&kernel, &session, &agent, call("m2", "gh pr merge 380 --squash")).await;
    log_tool_message(&kernel, &session, &agent, result("m2",
        "X Pull request o/r#380 is not mergeable: the base branch policy prohibits the merge.\n")).await;
    log_tool_message(&kernel, &session, &agent, call("g3", "gh pr create --base main --title t")).await;
    log_tool_message(&kernel, &session, &agent, result("g3",
        "pull request create failed: GraphQL: No commits between main and feat/x (createPullRequest)\n")).await;
    log_tool_message(&kernel, &session, &agent, call("c4", "git commit -m 'four'")).await;
    log_tool_message(&kernel, &session, &agent, result("c4", "On branch feat/x\nnothing to commit, working tree clean\n")).await;
    log_tool_message(&kernel, &session, &agent, call("p2", "git push origin main")).await;
    log_tool_message(&kernel, &session, &agent, serde_json::json!({
        "toolDenialKind": "automode-blocked",
        "message": {"content": [{"type": "tool_result", "tool_use_id": "p2", "is_error": true,
            "content": "Permission for this action was denied by the Claude Code auto mode classifier."}]}
    })).await;

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");
    assert_eq!((s.commits, s.pushes, s.prs_opened, s.prs_merged), (3, 1, 1, 1));
    assert_eq!((s.committed_added, s.committed_removed), (150, 28), "138 + 12 in, 28 out; the quiet commit carried none");
    assert_eq!(s.commits_with_stat, 2, "two of the three commits printed what they committed");

    // The live row names the newest thing shipped, not the newest command.
    assert_eq!(s.live.len(), 1);
    let row = &s.live[0];
    assert_eq!(row.shipped.as_deref(), Some("PR #379 merged"));
    assert!(row.shipped_at.is_some());
}

/// Churn as the repository saw it (#386): what landed on the main line
/// of the repositories the agents worked in, read with git — however
/// the edits were made. A side branch has not landed; a working
/// directory that is not a repository is counted unreadable.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn landed_lines_are_read_from_the_repositories() {
    let kernel = fresh_kernel().await;
    let agent = kernel.create_entity("node_agent").await.expect("agent");
    let session = kernel.create_entity("node_session").await.expect("session");

    let stamp = format!("{}-{}", std::process::id(), chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0));
    let dir = std::env::temp_dir().join(format!("superx-landed-{stamp}"));
    let scratch = std::env::temp_dir().join(format!("superx-not-a-repo-{stamp}"));
    std::fs::create_dir_all(dir.join("src")).expect("repo dir");
    std::fs::create_dir_all(&scratch).expect("scratch dir");
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg("-C").arg(&dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
            .args(args)
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("a.rs"), "one\ntwo\nthree\n").expect("write");
    git(&["add", "a.rs"]);
    git(&["commit", "-q", "-m", "first"]);
    // Replace one line, add two: +3 −1 as the repository counts it.
    std::fs::write(dir.join("a.rs"), "one\nTWO\nthree\nfour\nfive\n").expect("write");
    git(&["add", "a.rs"]);
    git(&["commit", "-q", "-m", "second"]);
    // Work on a side branch has not landed.
    git(&["checkout", "-q", "-b", "side"]);
    std::fs::write(dir.join("b.rs"), "x\n".repeat(10)).expect("write");
    git(&["add", "b.rs"]);
    git(&["commit", "-q", "-m", "side work"]);
    git(&["checkout", "-q", "main"]);

    let shell = |cwd: &std::path::Path| serde_json::json!({
        "cwd": cwd.to_string_lossy(),
        "message": {"model": "claude-opus-5", "content": [
            {"type": "tool_use", "id": "b", "name": "Bash", "input": {"command": "ls"}}]}
    });
    // From a subdirectory of the repo, and from somewhere that is not one.
    log_tool_message(&kernel, &session, &agent, shell(&dir.join("src"))).await;
    log_tool_message(&kernel, &session, &agent, shell(&scratch)).await;

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&scratch);

    assert_eq!(s.landed.commits, 2, "the two on main; the side branch has not landed");
    assert_eq!((s.landed.added, s.landed.removed), (6, 1), "3 + 3 in, 1 out");
    assert_eq!(s.landed.unreadable, 1, "the scratch directory is not a repository");
    assert_eq!(s.landed.repos.len(), 1, "{:?}", s.landed.repos);
    let repo = &s.landed.repos[0];
    assert_eq!(repo.name, dir.file_name().expect("name").to_string_lossy());
    assert_eq!(repo.branch, "main");
    assert_eq!((repo.commits, repo.added, repo.removed), (2, 6, 1));
    assert!(!s.landed.series.is_empty(), "an hourly point for the commit hour");
}

/// Steering is the half of churn a shell edit can still answer (#388).
/// The lines it replaced are unknown, but whether anyone asked for it
/// is not — so the split is counted in edits when it cannot be counted
/// in lines, and On course, Why the churn and the Unasked columns keep
/// reading under an operating mode that edits through the shell. A
/// file written end to end is a creation, not a rewrite.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steering_reads_in_edits_when_replaced_lines_are_unknown() {
    let kernel = fresh_kernel().await;
    let superx = TestRepo::new("superx", "feat/x");
    // Steering is per session: one the operator redirected, one left
    // to itself — the same shape as the line-counted test above.
    let (agent, steered) = seed_agent_and_session(&kernel, "claude_code", "steered").await;
    let (agent2, alone) = seed_agent_and_session(&kernel, "claude_code", "alone").await;
    let shell = |cmd: &str| serde_json::json!({
        "cwd": superx.cwd(), "gitBranch": "feat/x",
        "message": {"model": "claude-opus-5", "content": [
            {"type": "tool_use", "id": "b", "name": "Bash", "input": {"command": cmd}}]}
    });

    // The operator says something, then the agent rewrites: directed.
    kernel.log_message(superx_kernel::NewMessage {
        session: steered.clone(), agent: agent.clone(), role: "user".into(),
        content: "do it the other way".into(), raw: None, seq: None, emitted_at: None,
    }).await.expect("human turn");
    log_tool_message(&kernel, &steered, &agent, shell("sed -i 's/a/b/' src/a.rs")).await;

    // The other session rewrites twice with nobody steering…
    for f in ["src/b.rs", "src/c.rs"] {
        log_tool_message(&kernel, &alone, &agent2, shell(
            &format!("python3 - <<'PY'\nimport pathlib\np = pathlib.Path('/w/superx/{f}')\np.write_text(p.read_text() + 'x')\nPY"))).await;
    }
    // …and writes one file end to end: a creation, not a rewrite.
    log_tool_message(&kernel, &alone, &agent2,
        shell("cat > src/new.rs <<'EOF'\nfn fresh() {}\nEOF")).await;

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");

    // The transcript still cannot say HOW MANY lines went…
    assert_eq!(s.lines_removed, 0);
    assert_eq!((s.churn_directed, s.churn_self), (0, 0));
    // …but it can say how many rewrites there were, and who asked.
    assert_eq!(s.edits_directed, 1, "the sed followed a human turn");
    assert_eq!(s.edits_self, 2, "the two scripts had nobody steering");
    assert_eq!(s.replaced_unknown, 4, "every shell write, the creation included");

    // The created/modified split sees shell writes now.
    assert_eq!(s.files_created, 1, "the heredoc wrote a whole file");
    assert_eq!(s.files_modified, 3, "the three it edited in place");

    // The circling signal falls back the same way, per session.
    let row = |id: &superx_kernel::types::RecordId| {
        let uuid = superx_ops::record_uuid(id);
        s.live.iter().find(|l| l.identity == uuid).expect("live row")
    };
    assert_eq!(row(&steered).self_churn_pct, 0, "it was asked for");
    assert_eq!(row(&alone).self_churn_pct, 100, "nobody asked");

    // So do the branch and agent rows.
    let b = s.branches.iter().find(|b| b.branch == "feat/x").expect("branch row");
    assert_eq!((b.edits_directed, b.edits_self), (1, 2));
    assert_eq!(b.self_churn_pct, 66, "two of three rewrites unasked");
    assert_eq!(
        (
            s.agent_stats.iter().map(|a| a.edits_directed).sum::<i64>(),
            s.agent_stats.iter().map(|a| a.edits_self).sum::<i64>(),
        ),
        (1, 2),
        "the agent rows carry it too"
    );
}

/// An extension is a language, not a version suffix. `superx.prev-5001959`
/// ranked as a language on the live page beside `rs` and `py` (#388).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_version_suffix_is_not_a_language() {
    let kernel = fresh_kernel().await;
    let agent = kernel.create_entity("node_agent").await.expect("agent");
    let session = kernel.create_entity("node_session").await.expect("session");
    // Written, not read: a read is not file work (#412).
    for path in ["/w/superx/src/a.rs", "/w/bin/superx.prev-5001959", "/w/bin/superx", "/w/notes/plan.2026-09-11"] {
        log_tool_message(&kernel, &session, &agent, serde_json::json!({
            "cwd": "/w/superx",
            "message": {"model": "claude-opus-5", "content": [
                {"type": "tool_use", "id": "w", "name": "Write", "input": {"file_path": path, "content": "x"}}]}
        })).await;
    }
    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");
    let langs: Vec<&str> = s.languages.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(langs, vec!["rs"], "{langs:?}");
    assert_eq!(s.files_touched, 4, "every file is still a file");
}

/// Tokens over time, and the pair the operator actually switches (#391).
/// Every other series moved over time; tokens were a total, so nothing
/// said WHEN the money went. And model and reasoning level ride the
/// same messages, so comparing them needs one key, with the sample size
/// beside it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn burn_moves_over_time_and_the_model_effort_pair_is_one_key() {
    let kernel = fresh_kernel().await;
    let (agent, steered) = seed_agent_and_session(&kernel, "claude_code", "burn-a").await;
    let (agent2, alone) = seed_agent_and_session(&kernel, "claude_code", "burn-b").await;
    let spend = |model: &str, effort: &str, out: i64, think: i64, inp: i64, cache: i64| serde_json::json!({
        "cwd": "/w/superx", "effort": effort,
        "message": {"model": model, "usage": {
            "output_tokens": out, "input_tokens": inp, "cache_read_input_tokens": cache,
            "output_tokens_details": {"thinking_tokens": think}},
            "content": []}
    });

    // One pair spends after a human turn; the other flies alone.
    kernel.log_message(superx_kernel::NewMessage {
        session: steered.clone(), agent: agent.clone(), role: "user".into(),
        content: "go".into(), raw: None, seq: None, emitted_at: None,
    }).await.expect("human turn");
    log_tool_message(&kernel, &steered, &agent, spend("claude-opus-5", "max", 1000, 300, 50, 900)).await;
    log_tool_message(&kernel, &alone, &agent2, spend("claude-fable-5-1", "xhigh", 400, 200, 10, 100)).await;
    log_tool_message(&kernel, &alone, &agent2, spend("claude-fable-5-1", "xhigh", 600, 100, 20, 200)).await;

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");

    // The burn series carries every kind of token, in one bucket here.
    assert_eq!(s.burn.len(), 1, "one hour of spending: {:?}", s.burn.iter().map(|b| &b.t).collect::<Vec<_>>());
    let b = &s.burn[0];
    assert_eq!((b.out, b.thinking, b.input, b.cache_read), (2000, 600, 80, 1200));
    assert_eq!(b.out, s.out_tokens_window, "the series sums to the range's output");

    // Unattended: everything the second session spent, and nothing the
    // first did — the operator had just spoken to it.
    assert_eq!(s.unattended_out_tokens, 1000);
    assert_eq!(s.human_turns, 1);

    // The pair is one key, with its sample size.
    assert_eq!(s.model_effort.len(), 2, "{:?}", s.model_effort.iter().map(|p| (&p.model, &p.effort)).collect::<Vec<_>>());
    let pair = |m: &str| s.model_effort.iter().find(|p| p.model == m).expect("pair");
    let fable = pair("claude-fable-5-1");
    assert_eq!((fable.effort.as_str(), fable.sessions, fable.messages), ("xhigh", 1, 2));
    assert_eq!((fable.out_tokens, fable.thinking_tokens), (1000, 300));
    let opus = pair("claude-opus-5");
    assert_eq!((opus.effort.as_str(), opus.sessions, opus.messages), ("max", 1, 1));
    assert_eq!((opus.out_tokens, opus.thinking_tokens), (1000, 300));

    // Being stopped or refused belongs to whatever the session was
    // running. Those lines name no model — a denial rides the result,
    // an interruption rides your turn — so attributing them to the
    // message's own pair attributed them to nothing, and every pair
    // reported that nobody had ever had to step in (#391).
    kernel.log_message(superx_kernel::NewMessage {
        session: alone.clone(), agent: agent2.clone(), role: "tool".into(), content: String::new(),
        raw: Some(superx_kernel::message::json_to_object(&serde_json::json!({
            "toolDenialKind": "permission", "message": {"content": []}}))),
        seq: None, emitted_at: None,
    }).await.expect("denial");
    kernel.log_message(superx_kernel::NewMessage {
        session: alone.clone(), agent: agent2.clone(), role: "user".into(),
        content: "no, stop".into(),
        raw: Some(superx_kernel::message::json_to_object(&serde_json::json!({
            "userFeedback": true, "message": {"content": []}}))),
        seq: None, emitted_at: None,
    }).await.expect("intervention");

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");
    assert_eq!((s.denials, s.interventions), (1, 1), "the range counts them");
    let fable = s.model_effort.iter().find(|p| p.model == "claude-fable-5-1").expect("pair");
    assert_eq!((fable.denials, fable.interventions), (1, 1), "and so does the pair that was running");
    let opus = s.model_effort.iter().find(|p| p.model == "claude-opus-5").expect("pair");
    assert_eq!((opus.denials, opus.interventions), (0, 0), "the other session was not stopped");
}

/// The gates are an ORDER (#392): a pull request counts as gated when
/// tests, clippy and the audit all ran after the last change before it.
/// A write after the checks re-opens the question. A pull request from
/// a session that changed nothing is neither gated nor ungated, so the
/// two never have to sum to the number opened.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pull_request_is_gated_only_when_the_checks_ran_after_the_last_change() {
    let kernel = fresh_kernel().await;
    let superx = TestRepo::new("superx", "main");
    let agent = kernel.create_entity("node_agent").await.expect("agent");
    let good = kernel.create_entity("node_session").await.expect("s1");
    let bad = kernel.create_entity("node_session").await.expect("s2");
    let clean = kernel.create_entity("node_session").await.expect("s3");
    let red = kernel.create_entity("node_session").await.expect("s4");
    let now = chrono::Utc::now();
    let at = |mins: i64| now - chrono::Duration::minutes(mins);
    let write = |id: &str, path: &str| serde_json::json!({"cwd": superx.cwd(),
        "message": {"model": "claude-opus-5", "effort": "max", "content": [
            {"type": "tool_use", "id": id, "name": "Write",
             "input": {"file_path": path, "content": "fn a() {}"}}]}});
    let shell = |id: &str, cmd: &str| serde_json::json!({"cwd": superx.cwd(),
        "message": {"model": "claude-opus-5", "effort": "max", "content": [
            {"type": "tool_use", "id": id, "name": "Bash", "input": {"command": cmd}}]}});
    let out = |id: &str, text: &str| serde_json::json!({"cwd": superx.cwd(),
        "message": {"content": [{"type": "tool_result", "tool_use_id": id, "content": text}]}});
    let tested = "test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured";
    let red_test = "test result: FAILED. 11 passed; 1 failed; 0 ignored";
    let linted = "    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.2s";
    let audited = "✅ SKILL AUDIT CLEAN";
    let pr = "https://github.com/o/superx/pull/12";
    // One session's run of calls, each followed by what it printed.
    let run = |calls: Vec<(&'static str, &'static str, Option<&'static str>, i64)>| calls;

    // Gated: write, then all three checks passing, then the pull request.
    for (id, cmd, printed, mins) in run(vec![
        ("g1", "cargo test --workspace", Some(tested), 40),
        ("g2", "cargo clippy --workspace -- -D warnings", Some(linted), 39),
        ("g3", "python3 tools/skill_audit.py", Some(audited), 38),
        ("g4", "gh pr create --base main", Some(pr), 30),
    ]) {
        log_tool_message_at(&kernel, &good, &agent, shell(id, cmd), at(mins)).await;
        if let Some(text) = printed {
            log_tool_message_at(&kernel, &good, &agent, out(id, text), at(mins) + chrono::Duration::seconds(5)).await;
        }
    }
    log_tool_message_at(&kernel, &good, &agent, write("g0", superx.file("a.rs")), at(50)).await;

    // Ungated: the checks ran, then it wrote again, then opened.
    log_tool_message_at(&kernel, &bad, &agent, write("b0", superx.file("b.rs")), at(50)).await;
    for (id, cmd, printed, mins) in run(vec![
        ("b1", "cargo test --workspace", Some(tested), 45),
        ("b2", "cargo clippy --workspace -- -D warnings", Some(linted), 44),
        ("b3", "python3 tools/skill_audit.py", Some(audited), 43),
    ]) {
        log_tool_message_at(&kernel, &bad, &agent, shell(id, cmd), at(mins)).await;
        if let Some(text) = printed {
            log_tool_message_at(&kernel, &bad, &agent, out(id, text), at(mins) + chrono::Duration::seconds(5)).await;
        }
    }
    log_tool_message_at(&kernel, &bad, &agent, write("b4", superx.file("b.rs")), at(20)).await;
    log_tool_message_at(&kernel, &bad, &agent, shell("b5", "gh pr create --base main"), at(10)).await;
    log_tool_message_at(&kernel, &bad, &agent, out("b5", pr), at(10) + chrono::Duration::seconds(5)).await;

    // Ungated too: every check ran, but the tests FAILED (#412).
    log_tool_message_at(&kernel, &red, &agent, write("r0", superx.file("c.rs")), at(50)).await;
    for (id, cmd, printed, mins) in run(vec![
        ("r1", "cargo test --workspace 2>&1 | tail -3", Some(red_test), 45),
        ("r2", "cargo clippy --workspace -- -D warnings", Some(linted), 44),
        ("r3", "python3 tools/skill_audit.py", Some(audited), 43),
        ("r4", "gh pr create --base main", Some(pr), 40),
    ]) {
        log_tool_message_at(&kernel, &red, &agent, shell(id, cmd), at(mins)).await;
        if let Some(text) = printed {
            log_tool_message_at(&kernel, &red, &agent, out(id, text), at(mins) + chrono::Duration::seconds(5)).await;
        }
    }

    // Neither: it opened a pull request without changing anything.
    log_tool_message_at(&kernel, &clean, &agent, shell("c1", "gh pr create --base main"), at(5)).await;
    log_tool_message_at(&kernel, &clean, &agent, out("c1", pr), at(5) + chrono::Duration::seconds(5)).await;

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");
    assert_eq!(s.prs_opened, 4);
    assert_eq!(s.prs_gated, 1, "only the one whose checks passed after its last write");
    assert_eq!(s.prs_ungated, 2, "the one that wrote again, and the one whose tests failed");
}

/// The line a module lane must never cross (#392): the kernel's own
/// crate, and schema files. Ordinary paths are not deviations, and a
/// shell edit into the kernel is caught as surely as a `Write` is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn writing_into_the_kernel_or_a_schema_file_is_recorded() {
    let kernel = fresh_kernel().await;
    let agent = kernel.create_entity("node_agent").await.expect("agent");
    let session = kernel.create_entity("node_session").await.expect("session");
    let write = |path: &str| serde_json::json!({"cwd": "/w/superx",
        "message": {"model": "claude-opus-5", "content": [
            {"type": "tool_use", "id": "e", "name": "Write",
             "input": {"file_path": path, "content": "x"}}]}});
    log_tool_message(&kernel, &session, &agent, write("/w/superx/crates/superx-mod-ui/src/stats.rs")).await;
    log_tool_message(&kernel, &session, &agent, write("/w/superx/crates/superx-kernel/src/capture.rs")).await;
    log_tool_message(&kernel, &session, &agent, serde_json::json!({"cwd": "/w/superx",
        "message": {"model": "claude-opus-5", "content": [
            {"type": "tool_use", "id": "b", "name": "Bash",
             "input": {"command": "sed -i 's/a/b/' schema/kernel.surql"}}]}})).await;

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");
    assert_eq!(s.bright_line_writes, 2, "the kernel crate and the schema file, not the module's own source");
    let mut named: Vec<&str> = s.bright_line_paths.iter().map(String::as_str).collect();
    named.sort_unstable();
    assert_eq!(named, vec!["/w/superx/crates/superx-kernel/src/capture.rs", "/w/superx/schema/kernel.surql"]);
}

/// How hard the machine was working (#395): how many sessions spoke and
/// how many repositories were open in each bucket, beside what moved.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn intensity_counts_the_fronts_open_in_each_bucket() {
    let kernel = fresh_kernel().await;
    let alpha = TestRepo::new("alpha", "main");
    let beta = TestRepo::new("beta", "main");
    let agent = kernel.create_entity("node_agent").await.expect("agent");
    let one = kernel.create_entity("node_session").await.expect("s1");
    let two = kernel.create_entity("node_session").await.expect("s2");
    let now = chrono::Utc::now();
    let hour_ago = now - chrono::Duration::hours(1);
    let msg = |cwd: &str| serde_json::json!({"cwd": cwd,
        "message": {"model": "claude-opus-5", "usage": {"output_tokens": 100}, "content": [
            {"type": "tool_use", "id": "e", "name": "Write",
             "input": {"file_path": format!("{cwd}/a.rs"), "content": "a\nb"}}]}});

    // One hour: two sessions across two repositories.
    log_tool_message_at(&kernel, &one, &agent, msg(alpha.cwd()), hour_ago).await;
    log_tool_message_at(&kernel, &two, &agent, msg(beta.cwd()), hour_ago).await;
    // The next: one session, one repository.
    log_tool_message_at(&kernel, &one, &agent, msg(alpha.cwd()), now).await;

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");
    assert_eq!(s.intensity.len(), 2, "{:?}", s.intensity.iter().map(|i| &i.t).collect::<Vec<_>>());
    let busy = s.intensity.iter().max_by_key(|i| i.sessions).expect("busiest");
    assert_eq!((busy.sessions, busy.repos), (2, 2));
    assert_eq!(busy.lines_added, 4, "two lines from each");
    assert_eq!(busy.out_tokens, 200);
    assert_eq!((s.peak_sessions, s.peak_repos), (2, 2));

    // And each sortie says what it was like, not only how long it was.
    let span = s.timeline.iter().find(|t| t.identity == superx_ops::record_uuid(&one)).expect("sortie");
    assert_eq!(span.repos, 1, "that session never left its repository");
    assert_eq!(span.lines_added, 4);
    assert_eq!(span.out_tokens, 200);
}

/// What the substrate holds (#398). The engine reports no storage size
/// and the kernel does not hand a module the datastore path, so the
/// tables are enumerated from the engine's own catalogue, counted, and
/// weighed from a sample of their rows.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_substrate_reports_its_tables_rows_and_weight() {
    let kernel = fresh_kernel().await;
    let agent = kernel.create_entity("node_agent").await.expect("agent");
    let session = kernel.create_entity("node_session").await.expect("session");
    for i in 0..5 {
        log_tool_message(&kernel, &session, &agent, serde_json::json!({
            "cwd": "/w/superx",
            "message": {"model": "claude-opus-5", "content": [
                {"type": "tool_use", "id": format!("t{i}"), "name": "Read",
                 "input": {"file_path": format!("/w/superx/{i}.rs")}}]}})).await;
    }

    let i = superx_mod_ui::insights::insights_summary(&kernel).await.expect("insights");

    let message = i.tables.iter().find(|t| t.name == "message").expect("the message table");
    assert_eq!(message.rows, 5, "every message logged");
    assert!(message.sampled > 0 && message.sampled <= message.rows, "sampled {}", message.sampled);
    assert!(message.avg_row_bytes > 0, "a row weighs something");
    assert_eq!(message.bytes_est, message.avg_row_bytes * message.rows);

    // Entities were created too, so more than one table is reported and
    // the totals are the sum of the parts.
    assert!(i.tables.len() >= 2, "{:?}", i.tables.iter().map(|t| &t.name).collect::<Vec<_>>());
    assert_eq!(i.db_rows_total, i.tables.iter().map(|t| t.rows).sum::<i64>());
    assert_eq!(i.db_bytes_est, i.tables.iter().map(|t| t.bytes_est).sum::<i64>());

    // Biggest first, so the page can read the top row as the answer.
    let bytes: Vec<i64> = i.tables.iter().map(|t| t.bytes_est).collect();
    assert!(bytes.windows(2).all(|w| w[0] >= w[1]), "{bytes:?}");
}

/// An age computed on the server is stale the moment it is sent, and
/// staler still when the answer is cached or the panel refreshes once a
/// minute — two panels showed two different capture lags for the same
/// instance (#400). Every age now travels as the timestamp it is
/// measured from, so the page can count up from it and all of them
/// agree.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_age_travels_as_the_moment_it_is_measured_from() {
    let kernel = fresh_kernel().await;
    let agent = kernel.create_entity("node_agent").await.expect("agent");
    let session = kernel.create_entity("node_session").await.expect("session");
    log_tool_message(&kernel, &session, &agent, serde_json::json!({
        "cwd": "/w/superx",
        "message": {"model": "claude-opus-5", "content": [
            {"type": "tool_use", "id": "r", "name": "Read", "input": {"file_path": "/w/superx/a.rs"}}]}})).await;
    kernel
        .log_telemetry(
            "probe",
            superx_kernel::types::Value::Object(superx_kernel::message::json_to_object(
                &serde_json::json!({}),
            )),
            None,
        )
        .await
        .expect("telemetry");

    let i = superx_mod_ui::insights::insights_summary(&kernel).await.expect("insights");
    let at = i.last_event_at.as_deref().expect("the newest event carries its moment");
    let parsed = chrono::DateTime::parse_from_rfc3339(at).expect("rfc3339");
    let drift = (chrono::Utc::now() - parsed.with_timezone(&chrono::Utc)).num_seconds();
    assert!((0..120).contains(&drift), "the moment is recent and in the past: {drift}s");

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");
    let row = s.live.first().expect("a live row");
    let seen = row.last_seen_at.as_deref().expect("the row carries when it was last heard");
    assert!(chrono::DateTime::parse_from_rfc3339(seen).is_ok(), "rfc3339: {seen}");
    // The server's own view is kept beside it, and the two agree now.
    assert!((row.idle_secs - drift).abs() < 120, "idle {} against {drift}", row.idle_secs);
}

/// Is one model better than another (#403)? Outcomes land on the model
/// and reasoning level that MADE the call — a result line names no model
/// — and the tokens ride beside them, so cost and quality compare on the
/// same rows. (The time and repository slices #403 added had no reader
/// on the page and were removed in #413.)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn outcomes_land_on_the_model_that_made_the_call() {
    let kernel = fresh_kernel().await;
    let agent = kernel.create_entity("node_agent").await.expect("agent");
    let session = kernel.create_entity("node_session").await.expect("session");
    let now = chrono::Utc::now();
    let call = |model: &str, cwd: &str, id: &str| serde_json::json!({
        "cwd": cwd, "effort": "max",
        "message": {"model": model, "usage": {"output_tokens": 50}, "content": [
            {"type": "tool_use", "id": id, "name": "Bash", "input": {"command": "cargo test"}}]}});
    let result = |id: &str, failed: bool| serde_json::json!({
        "message": {"content": [
            {"type": "tool_result", "tool_use_id": id, "is_error": failed,
             "content": "test result: ok. 4 passed; 1 failed; 0 ignored"}]}});

    log_tool_message_at(&kernel, &session, &agent, call("claude-opus-5", "/w/shared", "a"), now).await;
    log_tool_message_at(&kernel, &session, &agent, result("a", true), now).await;
    log_tool_message_at(&kernel, &session, &agent, call("claude-fable-5", "/w/shared", "b"), now).await;
    log_tool_message_at(&kernel, &session, &agent, result("b", false), now).await;
    let before = now - chrono::Duration::hours(2);
    log_tool_message_at(&kernel, &session, &agent, call("claude-opus-5", "/w/alone", "c"), before).await;
    log_tool_message_at(&kernel, &session, &agent, result("c", false), before).await;

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");
    let pair = |m: &str| s.model_effort.iter().find(|p| p.model == m && p.effort == "max").expect(m);
    let opus = pair("claude-opus-5");
    assert_eq!((opus.tool_calls, opus.tool_failures), (2, 1));
    let fable = pair("claude-fable-5");
    assert_eq!((fable.tool_calls, fable.tool_failures), (1, 0));
    assert!(opus.tests_passed > 0 && fable.tests_passed > 0, "tests reach the pair that ran them");
    assert_eq!(s.model_effort.iter().map(|p| p.out_tokens).sum::<i64>(), s.out_tokens_window);
}

/// A reply is counted once, however many lines carry it (#409). Claude
/// Code writes one line per content block — thinking, text, each tool
/// call — and repeats the reply's whole `usage` on every one, under the
/// same `message.id`. Gemini re-emits a record as it streams, fuller
/// each time, under the same `id`. Every token figure summed rows, so a
/// three-block reply cost three times what it did.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reply_split_across_lines_is_counted_once() {
    let kernel = fresh_kernel().await;
    let (agent, session) = seed_agent_and_session(&kernel, "claude_code", "split").await;
    let usage = serde_json::json!({
        "input_tokens": 10, "cache_creation_input_tokens": 50,
        "cache_read_input_tokens": 1_000, "output_tokens": 100,
        "output_tokens_details": {"thinking_tokens": 40}
    });
    // One reply, three lines, in the order Claude Code writes them.
    for block in [
        serde_json::json!({"type": "thinking", "thinking": "…"}),
        serde_json::json!({"type": "text", "text": "Reading the file."}),
        serde_json::json!({"type": "tool_use", "id": "t1", "name": "Read",
                           "input": {"file_path": "/w/a.rs"}}),
    ] {
        log_tool_message(&kernel, &session, &agent, serde_json::json!({
            "effort": "max",
            "message": {"id": "msg_A", "model": "claude-opus-5", "usage": usage,
                        "content": [block]}
        })).await;
    }
    // A second, one-line reply.
    log_tool_message(&kernel, &session, &agent, serde_json::json!({
        "effort": "max",
        "message": {"id": "msg_B", "model": "claude-opus-5",
                    "usage": {"output_tokens": 20, "input_tokens": 5},
                    "content": [{"type": "text", "text": "Done."}]}
    })).await;
    // A Gemini record emitted twice as it streamed: the newer is fuller.
    let (gagent, gsession) = seed_agent_and_session(&kernel, "gemini_cli", "g").await;
    for (out, thoughts) in [(5, 0), (7, 3)] {
        log_tool_message(&kernel, &gsession, &gagent, serde_json::json!({
            "id": "g-1", "type": "gemini", "model": "gemini-3.1-pro",
            "tokens": {"input": 400, "output": out, "cached": 100, "thoughts": thoughts,
                       "tool": 0, "total": 400 + out + thoughts}
        })).await;
    }

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");
    assert_eq!(s.out_tokens_window, 100 + 20 + 10, "each reply once; Gemini output + thoughts");
    assert_eq!(s.output_tokens_total, 130, "the engine-side total agrees");
    assert_eq!(s.tokens_last_hour, 130);
    assert_eq!(s.thinking_tokens, 40 + 3);
    assert_eq!(s.exposure.input_tokens, 10 + 5 + 300, "Gemini input less what it read from cache");
    assert_eq!(s.exposure.cache_read_tokens, 1_000 + 100);
    assert_eq!(s.exposure.cache_write_tokens, 50);
    let opus = s.models.iter().find(|m| m.name == "claude-opus-5").expect("opus row");
    assert_eq!((opus.messages, opus.out_tokens), (2, 120), "two replies, not four lines");
    let gem = s.models.iter().find(|m| m.name == "gemini-3.1-pro").expect("gemini names its model");
    assert_eq!((gem.messages, gem.out_tokens), (1, 10));
    let pair = s.model_effort.iter().find(|p| p.model == "claude-opus-5").expect("pair");
    assert_eq!((pair.messages, pair.out_tokens, pair.thinking_tokens), (2, 120, 40));
    assert_eq!(s.tools_window, 1, "the Read, once");
    let burn: i64 = s.burn.iter().map(|b| b.out).sum();
    assert_eq!(burn, 130, "the burn series counts replies too");

    let (_, out) = superx_mod_ui::activity::session_token_stats(&kernel, session.clone())
        .await
        .expect("session tokens");
    assert_eq!(out, Some(120), "the Sessions page counts replies");
    let (ctx, gout) = superx_mod_ui::activity::session_token_stats(&kernel, gsession)
        .await
        .expect("gemini session tokens");
    assert_eq!(gout, Some(10));
    assert_eq!(ctx, Some(400), "Gemini's prompt is its input, not its total");

    let i = superx_mod_ui::insights::insights_summary(&kernel).await.expect("insights");
    assert_eq!(i.tokens.output, 130);
    assert_eq!(i.tokens.input, 315);
    assert_eq!(i.tokens.cache_read, 1_100);
    assert_eq!(i.tokens.cache_write, 50);
    let replies = |name: &str| i.models.iter().find(|m| m.name == name).map(|m| m.value);
    assert_eq!(replies("claude-opus-5"), Some(2));
    assert_eq!(replies("gemini-3.1-pro"), Some(1));
    let claude = i.per_agent.iter().find(|a| a.name == "claude_code").expect("agent row");
    assert_eq!((claude.messages, claude.output_tokens), (2, 120), "replies, as every other count; output per reply");
}

/// A session's context is the prompt its newest REAL reply answered (#415
/// review): the `<synthetic>` stand-in the runtime writes for an API error
/// carries all-zero usage, and taking it blanked the Sessions bar.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_synthetic_reply_does_not_blank_the_context() {
    let kernel = fresh_kernel().await;
    let (agent, session) = seed_agent_and_session(&kernel, "claude_code", "ctx").await;
    let now = chrono::Utc::now();
    log_tool_message_at(&kernel, &session, &agent, serde_json::json!({
        "message": {"id": "real", "model": "claude-opus-5", "content": [],
            "usage": {"input_tokens": 3, "cache_read_input_tokens": 90_000,
                      "cache_creation_input_tokens": 7, "output_tokens": 50}}}),
        now - chrono::Duration::minutes(2)).await;
    log_tool_message_at(&kernel, &session, &agent, serde_json::json!({
        "message": {"id": "err", "model": "<synthetic>", "content": [],
            "usage": {"input_tokens": 0, "cache_read_input_tokens": 0,
                      "cache_creation_input_tokens": 0, "output_tokens": 0}}}),
        now - chrono::Duration::minutes(1)).await;

    let (ctx, out) = superx_mod_ui::activity::session_token_stats(&kernel, session)
        .await
        .expect("session tokens");
    assert_eq!(ctx, Some(90_010), "the real reply's prompt");
    assert_eq!(out, Some(50));
}

/// Every "Msgs" on the page counts the same thing (#415 review): a reply
/// once, however many lines Claude Code wrote for it, and every other row
/// as itself. The model table counted replies while the repo, branch,
/// live, agent, top-session and timeline rows counted lines, 3.45 to a
/// reply, under the same label.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_msgs_counts_a_reply_once() {
    let kernel = fresh_kernel().await;
    let repo = TestRepo::new("superx", "main");
    let (agent, session) = seed_agent_and_session(&kernel, "claude_code", "msgs").await;
    // A prompt, as Claude Code writes one.
    kernel.log_message(superx_kernel::NewMessage {
        session: session.clone(), agent: agent.clone(), role: "user".into(), content: "go".into(),
        raw: Some(superx_kernel::message::json_to_object(&serde_json::json!({
            "type": "user", "origin": {"kind": "human"}, "cwd": repo.cwd(),
            "message": {"role": "user", "content": "go"}}))),
        seq: None, emitted_at: Some(chrono::Utc::now()),
    }).await.expect("prompt");
    for block in [
        serde_json::json!({"type": "thinking", "thinking": "…"}),
        serde_json::json!({"type": "text", "text": "Reading."}),
        serde_json::json!({"type": "tool_use", "id": "t1", "name": "Read", "input": {"file_path": repo.file("a.rs")}}),
    ] {
        log_tool_message(&kernel, &session, &agent, serde_json::json!({
            "cwd": repo.cwd(),
            "message": {"id": "msg_A", "model": "claude-opus-5", "usage": {"output_tokens": 9}, "content": [block]}
        })).await;
    }
    log_tool_message(&kernel, &session, &agent, serde_json::json!({
        "cwd": repo.cwd(),
        "message": {"content": [{"type": "tool_result", "tool_use_id": "t1", "content": "fn a() {}"}]}
    })).await;

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");
    // The prompt, the reply and the tool's result.
    assert_eq!(s.messages_last_hour, 3);
    assert_eq!(s.live[0].messages, 3);
    assert_eq!(s.top_sessions[0].messages, 3);
    assert_eq!(s.timeline[0].messages, 3);
    let claude = s.agent_stats.iter().find(|a| a.name == "claude_code").expect("agent");
    assert_eq!(claude.messages, 3);
    assert_eq!(s.repos[0].messages, 3);
    assert_eq!(s.branches[0].messages, 3);
    let i = superx_mod_ui::insights::insights_summary(&kernel).await.expect("insights");
    assert_eq!(i.per_agent.iter().find(|a| a.name == "claude_code").map(|a| a.messages), Some(3));
}

/// Lines come from the diff Claude Code recorded, not from the call's
/// strings (#410). An Edit must carry enough unchanged text around the
/// change to be unique, so its strings counted that context as removed
/// AND added; a Write over a file counted all of it as new; a failed
/// edit counted lines that never changed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lines_are_read_from_the_recorded_diff() {
    let kernel = fresh_kernel().await;
    let repo = TestRepo::new("superx", "main");
    let agent = kernel.create_entity("node_agent").await.expect("agent");
    let session = kernel.create_entity("node_session").await.expect("session");
    let call = |id: &str, name: &str, input: serde_json::Value| serde_json::json!({
        "cwd": repo.cwd(),
        "message": {"model": "claude-opus-5", "content": [
            {"type": "tool_use", "id": id, "name": name, "input": input}]}});
    let result = |id: &str, is_error: bool, tur: serde_json::Value| serde_json::json!({
        "cwd": repo.cwd(), "toolUseResult": tur,
        "message": {"content": [{"type": "tool_result", "tool_use_id": id, "is_error": is_error,
            "content": "ok"}]}});

    // One line inserted between three unchanged ones.
    log_tool_message(&kernel, &session, &agent, call("e1", "Edit", serde_json::json!({
        "file_path": repo.file("a.rs"), "old_string": "fn a() {\n    one();\n}",
        "new_string": "fn a() {\n    one();\n    two();\n}"}))).await;
    log_tool_message(&kernel, &session, &agent, result("e1", false, serde_json::json!({
        "filePath": repo.file("a.rs"),
        "structuredPatch": [{"oldStart": 1, "oldLines": 3, "newStart": 1, "newLines": 4,
            "lines": [" fn a() {", "     one();", "+    two();", " }"]}]}))).await;
    // A Write over an existing file: the diff is +2 −1, not +5.
    log_tool_message(&kernel, &session, &agent, call("w1", "Write", serde_json::json!({
        "file_path": repo.file("b.rs"), "content": "1\n2\n3\n4\n5"}))).await;
    log_tool_message(&kernel, &session, &agent, result("w1", false, serde_json::json!({
        "type": "update", "filePath": repo.file("b.rs"),
        "structuredPatch": [{"oldStart": 1, "oldLines": 4, "newStart": 1, "newLines": 5,
            "lines": [" 1", "-x", "+2", "+3", " 4", " 5"]}]}))).await;
    // A new file: every line is added, and the file was created.
    log_tool_message(&kernel, &session, &agent, call("w2", "Write", serde_json::json!({
        "file_path": repo.file("c.rs"), "content": "a\nb"}))).await;
    log_tool_message(&kernel, &session, &agent, result("w2", false, serde_json::json!({
        "type": "create", "filePath": repo.file("c.rs"), "content": "a\nb", "structuredPatch": []}))).await;
    // An edit that failed changed nothing.
    log_tool_message(&kernel, &session, &agent, call("e2", "Edit", serde_json::json!({
        "file_path": repo.file("d.rs"), "old_string": "missing", "new_string": "x\ny\nz"}))).await;
    log_tool_message(&kernel, &session, &agent, result("e2", true,
        serde_json::json!("Error: String to replace not found in file."))).await;

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");
    assert_eq!(s.lines_added, 1 + 2 + 2, "the inserted line, the Write's diff, the new file");
    assert_eq!(s.lines_removed, 1, "only the line the Write replaced");
    assert_eq!(s.writes_window, 3, "the failed edit wrote nothing");
    assert_eq!((s.files_created, s.files_modified), (1, 2), "created, per the result; b.rs was there");
    assert_eq!(s.edits_self + s.edits_directed, 1, "an insertion is not a rewrite; the Write over b.rs is");
}

/// The repository is the one git says a directory belongs to, and the
/// branch is the one that checkout was on (#411). A subdirectory is its
/// repository, a worktree is its repository's, stepping between them is
/// not a switch, and a directory outside any repository is none.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repos_and_branches_are_read_from_git() {
    let kernel = fresh_kernel().await;
    let repo = TestRepo::new("superx", "main");
    let tree = repo.worktree("cockpit", "fix/409-read-true");
    let src = repo.sub("crates/mod/src");
    let tree_src = leak(format!("{tree}/crates"));
    std::fs::create_dir_all(tree_src).expect("worktree subdir");
    let elsewhere = std::fs::canonicalize(std::env::temp_dir()).expect("tmp");
    let elsewhere = leak(elsewhere.to_string_lossy().into_owned());
    let (agent, session) = seed_agent_and_session(&kernel, "claude_code", "wt").await;
    let at = |cwd: &str| serde_json::json!({
        // The launch directory's branch, as Claude Code stamps every line.
        "cwd": cwd, "gitBranch": "main",
        "message": {"model": "claude-opus-5", "content": [
            {"type": "tool_use", "id": "b", "name": "Bash", "input": {"command": "ls"}}]}});
    for cwd in [repo.cwd(), src, tree, tree_src, elsewhere] {
        log_tool_message(&kernel, &session, &agent, at(cwd)).await;
    }

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");
    let names: Vec<&str> = s.repos.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, vec!["superx"], "one repository, not `src`, `cockpit` or the temp dir");
    assert_eq!(s.repos[0].messages, 4, "the temp dir is in no repository");
    assert_eq!(s.repo_switches, 0, "moving between a repo's checkouts and folders is not a switch");
    let mut branches: Vec<&str> = s.branches.iter().map(|b| b.branch.as_str()).collect();
    branches.sort_unstable();
    assert_eq!(branches, vec!["fix/409-read-true", "main"],
        "the worktree's rows carry ITS branch, not the launch directory's");
    let names: Vec<&str> = s.projects.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names.len(), 1, "projects are repositories: {names:?}");
}

/// A read is not a write, `grep -i` is not an in-place edit, a heredoc
/// into the scratchpad is neither, a shell edit inside a worktree is a
/// write, and a refused call did nothing at all (#412).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_walk_reads_what_a_call_actually_did() {
    let kernel = fresh_kernel().await;
    let repo = TestRepo::new("superx", "main");
    let tree = repo.worktree("w", "feat/x");
    let agent = kernel.create_entity("node_agent").await.expect("agent");
    let session = kernel.create_entity("node_session").await.expect("session");
    let shell = |id: &str, cwd: &str, cmd: &str| serde_json::json!({
        "cwd": cwd, "message": {"model": "claude-opus-5", "content": [
            {"type": "tool_use", "id": id, "name": "Bash", "input": {"command": cmd}}]}});
    let read = |id: &str, path: &str| serde_json::json!({
        "cwd": repo.cwd(), "message": {"model": "claude-opus-5", "content": [
            {"type": "tool_use", "id": id, "name": "Read", "input": {"file_path": path}}]}});

    // Read three times: not a thrash file, not an existing file touched.
    for id in ["r1", "r2", "r3"] {
        log_tool_message(&kernel, &session, &agent, read(id, repo.file("lib.rs"))).await;
    }
    // Case-insensitive search: a read.
    log_tool_message(&kernel, &session, &agent, shell("g", repo.cwd(), "grep -i fixme src/lib.rs")).await;
    // A heredoc into the scratchpad: neither a read nor a write.
    log_tool_message(&kernel, &session, &agent,
        shell("s", repo.cwd(), "cat > /private/tmp/claude-1/scratchpad/notes.md <<'EOF'\nnotes\nEOF")).await;
    // A heredoc into a file in a worktree: a write.
    log_tool_message(&kernel, &session, &agent,
        shell("h", tree, "cat > crates/new.rs <<'EOF'\nfn x() {}\nfn y() {}\nEOF")).await;
    // An in-place sed: a write.
    log_tool_message(&kernel, &session, &agent, shell("i", repo.cwd(), "sed -i '' 's/a/b/' src/lib.rs")).await;
    // A refused write ran nothing.
    log_tool_message(&kernel, &session, &agent, shell("x", repo.cwd(), "cat > src/gone.rs <<'EOF'\nx\nEOF")).await;
    log_tool_message(&kernel, &session, &agent, serde_json::json!({
        "cwd": repo.cwd(), "toolDenialKind": "user-rejected",
        "message": {"content": [{"type": "tool_result", "tool_use_id": "x", "is_error": true,
            "content": "The user doesn't want to proceed with this tool use."}]}})).await;

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");
    assert_eq!(s.writes_window, 2, "the worktree heredoc and the sed; not grep -i, not the scratchpad, not the refused one");
    assert_eq!(s.reads_window, 4, "three Reads and the grep");
    assert_eq!(s.lines_added, 2, "the worktree heredoc's two lines");
    assert_eq!(s.thrash_files, 0, "reading a file three times is not thrash");
    assert_eq!(s.files_modified, 1, "src/lib.rs was edited in place; lib.rs was only read");
    assert_eq!(s.files_created, 1, "the heredoc wrote a file end to end");
    assert!(s.files.iter().all(|f| !f.name.contains("scratchpad")), "{:?}", s.files);
}

/// A shell call reads and writes where it STANDS (#412): a `cd` moves the
/// rest of its chain, one inside `( … )` only the rest of its subshell;
/// `..` resolves as the filesystem would; and a git range is no file.
/// Every path used to resolve against the transcript's `cwd`, so a heredoc
/// written after `cd` into the scratchpad was a repository file, and a
/// `cat` after `cd /etc` read inside the repository.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_shell_call_reads_and_writes_where_it_stands() {
    let kernel = fresh_kernel().await;
    let repo = TestRepo::new("superx", "main");
    repo.sub("ui");
    repo.sub("src");
    let (agent, session) = seed_agent_and_session(&kernel, "claude_code", "cd").await;
    let shell = |id: &str, cmd: &str| serde_json::json!({
        "cwd": repo.cwd(), "message": {"model": "claude-opus-5", "content": [
            {"type": "tool_use", "id": id, "name": "Bash", "input": {"command": cmd}}]}});
    for (id, cmd) in [
        // Into the scratchpad, by `cd`: no file of the repository.
        ("w1", "cd /private/tmp/claude-1/scratchpad && cat > notes.md <<'EOF'\nnotes\nEOF"),
        // Into a subdirectory, by `cd`: the file is there.
        ("w2", "cd ui && cat > app.ts <<'EOF'\nexport const a = 1;\nEOF"),
        // A subshell's `cd` ends with the subshell.
        ("w3", "(cd ui && npm run build) && cat > top.rs <<'EOF'\nfn top() {}\nEOF"),
        // Reads: `..` resolves to the README a Read names below…
        ("r1", "cat src/../README"),
        // …a range is not a path, the words after `--` are…
        ("r2", "git diff origin/main...HEAD"),
        ("r3", "git log --oneline -- src/lib.rs"),
        // …`cd` moves a read out of the repository…
        ("r4", "cd /etc && cat ssh/sshd_config"),
        // …and a pattern, a program or a flag's value is no file: `/^fn`
        // and `/` would each have been a read outside the repository.
        ("r6", "sed -n '/^fn x/,/^}/p' src/lib.rs"),
        ("r7", "grep -n -v '/generated/' ui/app.ts"),
        ("r8", "awk -F / '/src/' src/main.rs | grep -v '/dist/'"),
        // A quoted path is one path, spaces and all; a brace expansion
        // names files the line does not spell.
        ("r9", "cat \"/opt/My Files/notes.txt\""),
        ("r10", "head -5 src/{a,b}.rs"),
    ] {
        log_tool_message(&kernel, &session, &agent, shell(id, cmd)).await;
    }
    log_tool_message(&kernel, &session, &agent, serde_json::json!({
        "cwd": repo.cwd(), "message": {"model": "claude-opus-5", "content": [
            {"type": "tool_use", "id": "r5", "name": "Read", "input": {"file_path": repo.file("README")}}]}})).await;

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");
    assert_eq!(s.writes_window, 2, "ui/app.ts and top.rs; the scratchpad note is not work");
    let names: Vec<&str> = s.files.iter().map(|f| f.name.as_str()).collect();
    assert!(names.iter().any(|n| n.ends_with("/ui/app.ts")), "{names:?}");
    assert!(names.iter().any(|n| n.ends_with("/superx/top.rs")), "the subshell's cd ended: {names:?}");
    assert!(names.iter().all(|n| !n.contains("scratchpad") && !n.ends_with("/ui/top.rs")), "{names:?}");
    assert_eq!(s.exposure.files_read, 6,
        "README (twice, one path), src/lib.rs (twice), sshd_config, ui/app.ts, src/main.rs and \
         the notes; never `origin/main...HEAD`, a pattern, a separator or a brace expansion");
    assert_eq!(s.exposure.outside_reads, 2, "the cat after `cd /etc` and the notes, and nothing else");
}

/// One call writing two files from two heredocs wrote two texts, not one
/// text twice (#406); the same text written into a second file IS a copy.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_heredoc_is_its_own_files_text() {
    let kernel = fresh_kernel().await;
    let repo = TestRepo::new("superx", "main");
    let (agent, session) = seed_agent_and_session(&kernel, "claude_code", "dup").await;
    let shell = |id: &str, cmd: &str| serde_json::json!({
        "cwd": repo.cwd(), "message": {"model": "claude-opus-5", "content": [
            {"type": "tool_use", "id": id, "name": "Bash", "input": {"command": cmd}}]}});
    log_tool_message(&kernel, &session, &agent, shell("a",
        "cat > a.rs <<'A'\nfn alpha() { one(); }\nA\ncat > b.rs <<'B'\nfn beta() { two(); }\nB")).await;
    log_tool_message(&kernel, &session, &agent, shell("c",
        "cat > c.rs <<'EOF'\nfn alpha() { one(); }\nEOF")).await;

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");
    assert_eq!(s.duplicates.len(), 1, "{:?}", s.duplicates);
    let mut paths: Vec<&str> = s.duplicates[0].paths.iter().map(String::as_str).collect();
    paths.sort_unstable();
    assert_eq!(paths, vec![repo.file("a.rs"), repo.file("c.rs")], "b.rs holds another text");
    assert_eq!(s.lines_added, 3);
}

/// A gate whose output went to a file is read by the exit code the
/// session echoed for it (#412): `TEST_EXIT=0` passes and `TEST_EXIT=101`
/// fails — and `latest=1` is no test's exit code.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_gate_run_into_a_file_is_read_by_the_exit_it_echoed() {
    let kernel = fresh_kernel().await;
    let repo = TestRepo::new("superx", "main");
    let agent = kernel.create_entity("node_agent").await.expect("agent");
    let now = chrono::Utc::now();
    let at = |mins: i64| now - chrono::Duration::minutes(mins);
    let write = |id: &str| serde_json::json!({"cwd": repo.cwd(),
        "message": {"model": "claude-opus-5", "content": [
            {"type": "tool_use", "id": id, "name": "Write",
             "input": {"file_path": repo.file("a.rs"), "content": "fn a() {}"}}]}});
    let shell = |id: &str, cmd: &str| serde_json::json!({"cwd": repo.cwd(),
        "message": {"model": "claude-opus-5", "content": [
            {"type": "tool_use", "id": id, "name": "Bash", "input": {"command": cmd}}]}});
    let out = |id: &str, text: &str| serde_json::json!({"cwd": repo.cwd(),
        "message": {"content": [{"type": "tool_result", "tool_use_id": id, "content": text}]}});
    for (tag, tested) in [("ok", "TEST_EXIT=0\nlatest=1"), ("red", "TEST_EXIT=101")] {
        let session = kernel.create_entity("node_session").await.expect("session");
        log_tool_message_at(&kernel, &session, &agent, write(&format!("{tag}0")), at(50)).await;
        for (i, (cmd, printed)) in [
            ("cargo test --workspace > /tmp/t.log 2>&1; echo TEST_EXIT=$?", tested),
            ("cargo clippy --workspace -- -D warnings >/dev/null 2>&1; echo CLIPPY=$?", "CLIPPY=0"),
            ("python3 tools/skill_audit.py | tail -1", "✅ SKILL AUDIT CLEAN"),
            ("gh pr create --base main", "https://github.com/o/superx/pull/7"),
        ]
        .into_iter()
        .enumerate()
        {
            let id = format!("{tag}{}", i + 1);
            let mins = 40 - i as i64;
            log_tool_message_at(&kernel, &session, &agent, shell(&id, cmd), at(mins)).await;
            log_tool_message_at(&kernel, &session, &agent, out(&id, printed),
                at(mins) + chrono::Duration::seconds(5)).await;
        }
    }

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");
    assert_eq!(s.prs_opened, 2);
    assert_eq!(s.prs_gated, 1, "the run that echoed TEST_EXIT=0");
    assert_eq!(s.prs_ungated, 1, "the run that echoed TEST_EXIT=101");
}

/// A shell call is judged file by file (#415 review). One call that edits
/// the repository through a variable and logs to the scratchpad is a
/// write; one that writes a scratch note and edits a repository file lends
/// the note's lines and its "created" to nothing. `perl -MList::Util` and
/// `sed -fscript.sed` edit nothing; `find -exec sed -i` and `xargs perl
/// -pi` edit files the line never names.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_shell_call_is_judged_file_by_file() {
    let kernel = fresh_kernel().await;
    let repo = TestRepo::new("superx", "main");
    repo.sub("src");
    let (agent, session) = seed_agent_and_session(&kernel, "claude_code", "files").await;
    let shell = |id: &str, cmd: &str| serde_json::json!({
        "cwd": repo.cwd(), "message": {"model": "claude-opus-5", "content": [
            {"type": "tool_use", "id": id, "name": "Bash", "input": {"command": cmd}}]}});
    let scratch = "/private/tmp/claude-1/scratchpad";
    for (id, cmd) in [
        // Writes: through a variable, logging to scratch…
        ("w1", format!("python3 - <<'EOF'\np = 'src/stats.rs'\ns = open(p).read()\nopen(p, 'w').write(s)\nEOF\ncargo check > {scratch}/chk.log 2>&1")),
        // …a scratch note beside an in-place edit…
        ("w2", format!("cat > {scratch}/notes.md <<'EOF'\none\ntwo\nthree\nEOF\nsed -i 's/a/b/' src/lib.rs")),
        // …and edits handed to another program.
        ("w3", "find src -name '*.rs' -exec sed -i 's/a/b/' {} +".to_string()),
        ("w4", "grep -rl foo src | xargs perl -pi -e 's/foo/bar/'".to_string()),
        // Not writes.
        ("n1", "perl -MList::Util=sum -e 'print sum(1, 2)'".to_string()),
        ("n2", "perl -Ilib -e 'print 1'".to_string()),
        ("n3", "sed -fscript.sed src/lib.rs".to_string()),
        // Neither a read nor a write: it deletes what it finds.
        ("n4", format!("find {scratch} -name '*.log' -delete")),
    ] {
        log_tool_message(&kernel, &session, &agent, shell(id, &cmd)).await;
    }

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");
    assert_eq!(s.writes_window, 4, "the variable write, the sed beside the note, find -exec and xargs");
    assert_eq!(s.lines_added, 0, "the note's three lines went to the scratchpad");
    assert_eq!((s.files_created, s.files_modified), (0, 1), "src/lib.rs was edited, not created");
    assert_eq!(s.reads_window, 1, "sed -f reads with a script; the perl one-liners are neither");
}

/// Exposure is every stage that looked (#413): a credentials file read in
/// a chain that also asked git something went to the vendor all the
/// same, while a stage whose output went elsewhere showed the model
/// nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exposure_is_every_stage_that_looked() {
    let kernel = fresh_kernel().await;
    let repo = TestRepo::new("superx", "main");
    let (agent, session) = seed_agent_and_session(&kernel, "claude_code", "exposed").await;
    let shell = |id: &str, cmd: &str| serde_json::json!({
        "cwd": repo.cwd(), "message": {"model": "claude-opus-5", "content": [
            {"type": "tool_use", "id": id, "name": "Bash", "input": {"command": cmd}}]}});
    log_tool_message(&kernel, &session, &agent,
        shell("a", "ls ~/.netrc && grep -i bitbucket -A2 ~/.netrc; git config --get credential.helper")).await;
    log_tool_message(&kernel, &session, &agent, shell("b", "cat /etc/hosts > /dev/null && git status")).await;

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");
    assert_eq!(s.exposure.outside_reads, 1, "~/.netrc, once for the call; not /etc/hosts, sent to /dev/null");
    assert_eq!(s.exposure.files_read, 1, "{:?}", s.exposure);
    assert_eq!(s.reads_window, 1, "the second call only looked; the first also asked git");
}

/// What shipped is what each stage printed (#415 review): a commit is not
/// undone by the clean `git status` after it; a commit or push the call
/// may never have reached counts only if its output shows it; a commit's
/// lines are the stat under its own header; and a hook's bracketed line
/// is no commit.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn what_shipped_is_what_each_stage_printed() {
    let kernel = fresh_kernel().await;
    let repo = TestRepo::new("superx", "main");
    let (agent, session) = seed_agent_and_session(&kernel, "claude_code", "ship").await;
    let now = chrono::Utc::now();
    let call = |id: &str, cmd: &str| serde_json::json!({
        "cwd": repo.cwd(), "message": {"model": "claude-opus-5", "content": [
            {"type": "tool_use", "id": id, "name": "Bash", "input": {"command": cmd}}]}});
    let result = |id: &str, failed: bool, body: &str| serde_json::json!({
        "message": {"content": [
            {"type": "tool_result", "tool_use_id": id, "is_error": failed, "content": body}]}});
    let mut at = now - chrono::Duration::minutes(6);
    for (id, cmd, failed, printed) in [
        ("c1", "git commit -q -m one && git status", false,
         "On branch main\nnothing to commit, working tree clean\n"),
        ("c2", "git add gone.rs && git commit -q -m two", true,
         "Exit code 128\nfatal: pathspec 'gone.rs' did not match any files"),
        ("c3", "git commit --allow-empty -m three && git show --stat HEAD~1", false,
         "[main 1a2b3c4d] three\ncommit 9f8e7d6c\nAuthor: t <t@t>\n\n    two\n\n a.rs | 3 +++\n 1 file changed, 3 insertions(+)\n"),
        ("p1", "sleep 600 && git push -q", true, "Command timed out after 2m 0.0s"),
        ("p2", "git push -q", false, ""),
        ("c4", "git commit -m four", false,
         "[WARNING] Unstaged files detected.\n[main 5e6f7a8b] four\n 2 files changed, 7 insertions(+), 1 deletion(-)\n"),
    ] {
        log_tool_message_at(&kernel, &session, &agent, call(id, cmd), at).await;
        log_tool_message_at(&kernel, &session, &agent, result(id, failed, printed), at + chrono::Duration::seconds(5)).await;
        at += chrono::Duration::minutes(1);
    }

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");
    assert_eq!(s.commits, 3, "one, three and four; not the commit a failed add never reached");
    assert_eq!(s.pushes, 1, "the quiet push that returned; not the one that timed out");
    assert_eq!(s.commits_with_stat, 1, "four's own stat; three is empty, and git show's stat is not its");
    assert_eq!((s.committed_added, s.committed_removed), (7, 1));
    assert_eq!(s.live.len(), 1);
    assert_eq!(s.live[0].shipped.as_deref(), Some("commit 5e6f7a8b"), "the hook's [WARNING] line is no commit");
}

/// A gate's echoed exit code is its verdict (#415 review) — another
/// command's `error:` line does not overturn `CLIPPY=0` — unless the gate
/// was piped, when `$?` was the pipe's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_echoed_exit_is_the_verdict_unless_the_gate_was_piped() {
    let kernel = fresh_kernel().await;
    let repo = TestRepo::new("superx", "main");
    let agent = kernel.create_entity("node_agent").await.expect("agent");
    let now = chrono::Utc::now();
    let at = |mins: i64| now - chrono::Duration::minutes(mins);
    let write = |id: &str| serde_json::json!({"cwd": repo.cwd(),
        "message": {"model": "claude-opus-5", "content": [
            {"type": "tool_use", "id": id, "name": "Write",
             "input": {"file_path": repo.file("a.rs"), "content": "fn a() {}"}}]}});
    let shell = |id: &str, cmd: &str| serde_json::json!({"cwd": repo.cwd(),
        "message": {"model": "claude-opus-5", "content": [
            {"type": "tool_use", "id": id, "name": "Bash", "input": {"command": cmd}}]}});
    let out = |id: &str, text: &str| serde_json::json!({"cwd": repo.cwd(),
        "message": {"content": [{"type": "tool_result", "tool_use_id": id, "content": text}]}});
    for (tag, test_cmd, test_out) in [
        ("ok", "cargo test --workspace > /tmp/t.log 2>&1; echo TEST_EXIT=$?", "TEST_EXIT=0"),
        ("piped", "cargo test --workspace 2>&1 | tail -3; echo TEST_EXIT=$?",
         "test result: FAILED. 11 passed; 1 failed; 0 ignored\nTEST_EXIT=0"),
    ] {
        let session = kernel.create_entity("node_session").await.expect("session");
        log_tool_message_at(&kernel, &session, &agent, write(&format!("{tag}0")), at(50)).await;
        for (i, (cmd, printed)) in [
            (test_cmd, test_out),
            ("cargo clippy --workspace -- -D warnings > /tmp/c.log 2>&1; echo CLIPPY=$?; git push --dry-run",
             "CLIPPY=0\nerror: failed to push some refs to 'origin'"),
            ("python3 tools/skill_audit.py", "✅ SKILL AUDIT CLEAN"),
            ("gh pr create --base main", "https://github.com/o/superx/pull/8"),
        ]
        .into_iter()
        .enumerate()
        {
            let id = format!("{tag}{}", i + 1);
            let mins = 40 - i as i64;
            log_tool_message_at(&kernel, &session, &agent, shell(&id, cmd), at(mins)).await;
            log_tool_message_at(&kernel, &session, &agent, out(&id, printed),
                at(mins) + chrono::Duration::seconds(5)).await;
        }
    }

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");
    assert_eq!(s.prs_opened, 2);
    assert_eq!(s.prs_gated, 1, "CLIPPY=0 stands; the unpiped TEST_EXIT=0 stands");
    assert_eq!(s.prs_ungated, 1, "the piped run's TEST_EXIT=0 is tail's; its tests failed");
}

/// A refused test run verified nothing (#415 review): the session is not
/// "verifying" because the operator stopped `cargo test` before it ran.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_test_run_verified_nothing() {
    let kernel = fresh_kernel().await;
    let (agent, session) = seed_agent_and_session(&kernel, "claude_code", "refused").await;
    log_tool_message(&kernel, &session, &agent, serde_json::json!({
        "cwd": "/w/superx", "message": {"id": "r", "model": "claude-opus-5", "content": [
            {"type": "tool_use", "id": "t", "name": "Bash", "input": {"command": "cargo test --workspace"}}]}})).await;
    log_tool_message(&kernel, &session, &agent, serde_json::json!({
        "toolDenialKind": "user-rejected",
        "message": {"content": [{"type": "tool_result", "tool_use_id": "t", "is_error": true,
            "content": "The user doesn't want to proceed with this tool use."}]}})).await;

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");
    assert_eq!(s.live.len(), 1);
    assert_eq!(s.live[0].doing, "working", "a refused run is a call, not a verification");
    assert_eq!(s.tests_run, 0);
}

/// What a live session is doing is what its NEWEST tool call is doing
/// (#413) — not the strongest thing it did anywhere in the range.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_live_session_is_doing_what_its_newest_call_does() {
    let kernel = fresh_kernel().await;
    let (agent, session) = seed_agent_and_session(&kernel, "claude_code", "now").await;
    let now = chrono::Utc::now();
    let call = |id: &str, name: &str, input: serde_json::Value| serde_json::json!({
        "cwd": "/w/superx", "message": {"id": id, "model": "claude-opus-5", "content": [
            {"type": "tool_use", "id": id, "name": name, "input": input}]}});
    // An hour ago it ran the tests; now it is reading.
    log_tool_message_at(&kernel, &session, &agent,
        call("t", "Bash", serde_json::json!({"command": "cargo test"})), now - chrono::Duration::hours(1)).await;
    log_tool_message_at(&kernel, &session, &agent,
        call("r", "Read", serde_json::json!({"file_path": "/w/superx/a.rs"})), now).await;

    // Another session read, then fetched a page: it is working, not
    // thinking — a fetch, a browser or an MCP call is none of the others.
    let (agent2, fetching) = seed_agent_and_session(&kernel, "claude_code", "fetch").await;
    log_tool_message_at(&kernel, &fetching, &agent2,
        call("r2", "Read", serde_json::json!({"file_path": "/w/superx/b.rs"})), now - chrono::Duration::minutes(2)).await;
    log_tool_message_at(&kernel, &fetching, &agent2,
        call("f", "WebFetch", serde_json::json!({"url": "https://docs.rs"})), now).await;

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");
    assert_eq!(s.live.len(), 2);
    let doing = |tool: &str| {
        s.live.iter().find(|l| l.last_tool.as_deref() == Some(tool)).map(|l| l.doing.as_str())
    };
    assert_eq!(doing("Read"), Some("reading"), "the newest call, not the test run an hour ago");
    assert_eq!(doing("WebFetch"), Some("working"));
}

/// Unknown is not zero (#413): compactions whose timing was never
/// captured cost an unknown amount, and a median under a minute is under
/// a minute, not "no data".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_reads_unknown_and_a_short_median_reads_short() {
    let kernel = fresh_kernel().await;
    let (agent, session) = seed_agent_and_session(&kernel, "claude_code", "u").await;
    let now = chrono::Utc::now();
    // A compaction summary, whose boundary line (and its timing) capture
    // does not keep.
    log_tool_message_at(&kernel, &session, &agent, serde_json::json!({
        "isCompactSummary": true, "message": {"content": "Summary"}}), now).await;
    // Two human turns thirty seconds apart.
    for secs in [60, 30] {
        kernel.log_message(superx_kernel::NewMessage {
            session: session.clone(), agent: agent.clone(), role: "user".into(),
            content: "go on".into(), raw: None, seq: None,
            emitted_at: Some(now - chrono::Duration::seconds(secs)),
        }).await.expect("turn");
    }
    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");
    assert_eq!(s.compactions, 1);
    assert_eq!(s.compaction_total_ms, None, "compacted, but the cost is unknown");
    assert_eq!(s.autonomy_p50_mins, 0, "under a minute is 0, not the -1 of no data");
    assert_eq!(s.survival_p50_mins, -1, "nothing was rewritten: no data");
    assert!(s.active_hours_range >= 1);
}

/// Coverage counts the 24 clock hours the strip draws (#415 review): this
/// hour and the 23 before it. A message a second before the first of them
/// is under 24 hours old and sits in a 25th bucket, which let the tile read
/// 25/24.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn coverage_counts_the_hours_the_strip_draws() {
    let kernel = fresh_kernel().await;
    let (agent, session) = seed_agent_and_session(&kernel, "claude_code", "cov").await;
    let now = chrono::Utc::now();
    let this_hour = now - chrono::Duration::seconds(now.timestamp().rem_euclid(3600));
    for at in [now, this_hour - chrono::Duration::hours(23) - chrono::Duration::seconds(1)] {
        log_tool_message_at(&kernel, &session, &agent, serde_json::json!({
            "message": {"content": [{"type": "text", "text": "."}]}}), at).await;
    }
    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");
    assert_eq!(s.active_hours_24h, 1, "{:?}", s.active_hours);
}

/// The page's hours and days are the viewer's (#415 review): at UTC−4 a
/// reply written at 01:30 UTC ran at 21:30 the evening before, and every
/// chart that buckets by hour or day — the burn series, the hour × weekday
/// heatmap, the work calendar — says so, as the Sortie log already did.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hours_and_days_are_the_viewers() {
    let kernel = fresh_kernel().await;
    let (agent, session) = seed_agent_and_session(&kernel, "claude_code", "tz").await;
    let now = chrono::Utc::now();
    let mut at = now.date_naive().and_hms_opt(1, 30, 0).expect("01:30").and_utc();
    if at > now {
        at -= chrono::Duration::days(1);
    }
    log_tool_message_at(&kernel, &session, &agent, serde_json::json!({
        "message": {"id": "r", "model": "claude-opus-5", "usage": {"output_tokens": 5},
            "content": [{"type": "text", "text": "."}]}}), at).await;
    let clock = chrono::FixedOffset::west_opt(4 * 3600).expect("UTC-4");
    let local = at.with_timezone(&clock);

    let s = superx_mod_ui::stats::stats_for_range_on(&kernel, 500, "24h", clock).await.expect("stats");
    let hour = local.format("%Y-%m-%dT%H").to_string();
    assert!(s.burn.iter().any(|b| b.t == hour && b.out == 5), "{hour} in {:?}", s.burn.iter().map(|b| &b.t).collect::<Vec<_>>());
    let i = superx_mod_ui::insights::insights_summary_on(&kernel, clock).await.expect("insights");
    assert_eq!(i.hour_weekday.iter().map(|c| c.hour).collect::<Vec<_>>(), vec![21]);
    let day = local.format("%Y-%m-%d").to_string();
    assert_eq!(i.events_per_day.iter().map(|d| d.t.clone()).collect::<Vec<_>>(), vec![day]);
}

/// A refusal belongs to the model that made the refused call, even after
/// the session switched to another (#413). It used to go to whatever the
/// session was running NOW.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refusal_lands_on_the_model_whose_call_was_refused() {
    let kernel = fresh_kernel().await;
    let (agent, session) = seed_agent_and_session(&kernel, "claude_code", "switch").await;
    let reply = |id: &str, model: &str| serde_json::json!({
        "cwd": "/w/superx", "effort": "max",
        "message": {"id": id, "model": model, "content": [
            {"type": "tool_use", "id": id, "name": "Bash", "input": {"command": "rm -rf target"}}]}});
    log_tool_message(&kernel, &session, &agent, reply("m1", "claude-fable-5")).await;
    log_tool_message(&kernel, &session, &agent, serde_json::json!({
        "toolDenialKind": "user-rejected",
        "message": {"content": [{"type": "tool_result", "tool_use_id": "m1", "is_error": true,
            "content": "The user doesn't want to proceed with this tool use."}]}})).await;
    // Then the operator switched models, and the new one worked on.
    log_tool_message(&kernel, &session, &agent, reply("m2", "claude-opus-5")).await;

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");
    let pair = |m: &str| s.model_effort.iter().find(|p| p.model == m).expect(m);
    assert_eq!(pair("claude-fable-5").denials, 1, "the refused call was fable's");
    assert_eq!(pair("claude-opus-5").denials, 0, "switching to opus later does not make it opus's");
}

/// A credential is its shape, not its prefix (#413): this module names
/// every prefix it looks for, so reading its own source lit the lamp,
/// while a real token pasted into a command went by unscanned.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_secret_is_a_shape_and_commands_are_scanned() {
    let kernel = fresh_kernel().await;
    let (agent, session) = seed_agent_and_session(&kernel, "claude_code", "sec").await;
    // A source file that merely NAMES the prefixes.
    log_tool_message(&kernel, &session, &agent, serde_json::json!({
        "cwd": "/w/superx", "message": {"content": [
            {"type": "tool_use", "id": "r", "name": "Read", "input": {"file_path": "/w/superx/stats.rs"}}]}})).await;
    log_tool_message(&kernel, &session, &agent, serde_json::json!({
        "message": {"content": [{"type": "tool_result", "tool_use_id": "r",
            "content": "text.contains(\"AKIA\") || text.contains(\"ghp_\")\n\
                        text.contains(\"-----BEGIN \") && text.contains(\"PRIVATE KEY-----\")"}]}})).await;
    // A real key block, as a Read result carries it: numbered lines.
    log_tool_message(&kernel, &session, &agent, serde_json::json!({
        "cwd": "/w/superx", "message": {"content": [
            {"type": "tool_use", "id": "k", "name": "Read", "input": {"file_path": "/w/superx/id_rsa"}}]}})).await;
    log_tool_message(&kernel, &session, &agent, serde_json::json!({
        "message": {"content": [{"type": "tool_result", "tool_use_id": "k",
            "content": format!("     1\t-----BEGIN RSA PRIVATE KEY-----\n     2\t{}", "MIIEowIBAAKCAQEA7bq98s1hd3kqQeLJ0n4YVmZ8xRk4dL0oPq2sT5uVwXyZ")}]}})).await;
    // A command carrying something shaped like a real token.
    let token = format!("ATATT3x{}", "Fq9Zb2Kd".repeat(6));
    log_tool_message(&kernel, &session, &agent, serde_json::json!({
        "cwd": "/w/superx", "message": {"content": [
            {"type": "tool_use", "id": "c", "name": "Bash",
             "input": {"command": format!("curl -u me:{token} https://example.atlassian.net")}}]}})).await;

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");
    assert_eq!(s.exposure.secret_hits, 2, "the command's token and the key block; not the source that names the markers");
    assert_eq!(s.exposure.secret_paths.len(), 2, "{:?}", s.exposure.secret_paths);
    assert!(s.exposure.secret_paths.iter().any(|p| p.starts_with("Bash input in")), "{:?}", s.exposure.secret_paths);
    assert!(s.exposure.secret_paths.iter().any(|p| p.ends_with("id_rsa")), "{:?}", s.exposure.secret_paths);
}

/// A private key is found however a tool shows it (#415 review): in a
/// service-account JSON file and an `.env` value, one line with `\n`
/// escapes; in `grep -n -A2` output; encrypted, behind its headers, in a
/// `Read` result. Prose naming the markers, and a marker over a short
/// line, are not keys. The fixtures are assembled at run time, so reading
/// this file is not a leak.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_private_key_is_found_however_a_tool_shows_it() {
    let kernel = fresh_kernel().await;
    let (agent, session) = seed_agent_and_session(&kernel, "claude_code", "keys").await;
    let body = "MIIEvQIBADANBgkqhkiG9w0BAQEFAASC".repeat(2);
    let begin = |kind: &str| format!("-----BEGIN {kind}PRIVATE KEY-----");
    let end = format!("-----END {}PRIVATE KEY-----", "");
    let shown = [
        format!("{{\"type\": \"service_account\", \"private_key\": \"{}\\n{body}\\n{end}\\n\"}}", begin("")),
        format!("PRIVATE_KEY=\"{}\\n{body}\\n{end}\"", begin("RSA ")),
        format!("1:{}\n2-{body}\n3-{body}", begin("OPENSSH ")),
        format!("     1\t{}\n     2\tProc-Type: 4,ENCRYPTED\n     3\tDEK-Info: AES-128-CBC,{}\n     4\t\n     5\t{body}",
            begin("RSA "), "0123456789ABCDEF".repeat(2)),
        // Not keys.
        format!("/// A block opens {} and the body follows\n/// on the next line, as base64.", begin("")),
        format!("{}\nMIIEvQIBADAN\n{end}", begin("")),
    ];
    for (i, text) in shown.iter().enumerate() {
        let id = format!("k{i}");
        log_tool_message(&kernel, &session, &agent, serde_json::json!({
            "cwd": "/w/superx", "message": {"content": [
                {"type": "tool_use", "id": id, "name": "Read", "input": {"file_path": format!("/w/superx/key{i}")}}]}})).await;
        log_tool_message(&kernel, &session, &agent, serde_json::json!({
            "message": {"content": [{"type": "tool_result", "tool_use_id": id, "content": text}]}})).await;
    }

    let s = superx_mod_ui::stats::stats_for_range(&kernel, 500, "24h").await.expect("stats");
    assert_eq!(s.exposure.secret_hits, 4, "{:?}", s.exposure.secret_paths);
    let mut paths = s.exposure.secret_paths.clone();
    paths.sort();
    assert_eq!(paths, vec!["/w/superx/key0", "/w/superx/key1", "/w/superx/key2", "/w/superx/key3"]);
}

/// The model comparison reads git as the work actually moved (#414).
///
/// One repository, two checkouts. Fable writes a branch in a worktree;
/// Opus, in the main checkout, squash-merges it as the account that
/// clicks merge. The squash is Fable's work — credited by when the
/// branch was written, not by when it merged — and it is counted once,
/// not once per checkout, and blamed at the main line though the main
/// checkout has another branch out. Fable's abandoned branch is never
/// landed; a teammate's branch is not this machine's work at all. And a
/// second repository whose main line took nothing is not judged.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_comparison_reads_git_as_the_work_moved() {
    let kernel = fresh_kernel().await;
    let now = chrono::Utc::now();
    let t0 = now - chrono::Duration::hours(10);
    let h = |x: f64| t0 + chrono::Duration::seconds((x * 3600.0) as i64);
    let repo = TestRepo::new_dated("superx", "main", t0 - chrono::Duration::hours(1));
    let wt = repo.worktree("wt", "feat/a");
    let wt2 = repo.worktree("wt2", "feat/b");
    let wt3 = repo.worktree("wt3", "feat/c");
    let main = repo.cwd();
    let ten: String = (1..=10).map(|i| format!("line {i}\n")).collect();
    let fifteen: String = (1..=15).map(|i| format!("line {i}\n")).collect();

    // Fable's branch, two commits; its abandoned branch; a teammate's.
    repo.commit(wt, h(0.5), "t@t", "feat: a", &[("a.rs", &ten)]);
    repo.commit(wt, h(1.5), "t@t", "feat: more a", &[("a.rs", &fifteen)]);
    // More on the same branch that the squash did not take — and the
    // branch is still checked out, so that is work in flight.
    repo.commit(wt, h(1.8), "t@t", "feat: d", &[("d.rs", "1\n2\n3\n4\n")]);
    repo.commit(wt2, h(1.0), "t@t", "feat: b", &[("b.rs", "1\n2\n3\n4\n5\n6\n7\n")]);
    // feat/b's worktree is gone: the branch is abandoned, not in flight.
    repo.git(&["worktree", "remove", "--force", wt2]);
    repo.commit(wt3, h(1.0), "mate@x", "feat: c", &[("c.rs", "1\n2\n3\n4\n5\n")]);
    // The squash, on main, as the merging account, while Opus works.
    repo.commit(main, h(3.5), "noreply@github.com", "feat: a (#1)", &[("a.rs", &fifteen)]);
    // The main checkout moves to a branch where a.rs is gone.
    repo.git(&["checkout", "-q", "-b", "feat/other"]);
    repo.git(&["rm", "-q", "a.rs"]);
    repo.commit(main, h(4.5), "t@t", "chore: drop a", &[]);

    // A second repository: its main line has only its first commit, from
    // long before; the work goes to another branch.
    let lake = TestRepo::new_dated("lake", "main", now - chrono::Duration::days(200));
    lake.git(&["checkout", "-q", "-b", "sandbox"]);
    lake.commit(lake.cwd(), h(5.5), "t@t", "feat: lake", &[("l.py", "x = 1\n")]);

    let reply = |id: &str, model: &str, cwd: &str| serde_json::json!({
        "cwd": cwd, "effort": "max",
        "message": {"id": id, "model": model, "usage": {"output_tokens": 100},
            "content": [{"type": "text", "text": "working"}]}});
    let (agent, fable) = seed_agent_and_session(&kernel, "claude_code", "fable").await;
    let (_, opus) = seed_agent_and_session(&kernel, "claude_code", "opus").await;
    let (_, laker) = seed_agent_and_session(&kernel, "claude_code", "lake").await;
    log_tool_message_at(&kernel, &fable, &agent, reply("f1", "claude-fable-5", wt), h(0.0)).await;
    log_tool_message_at(&kernel, &fable, &agent, reply("f2", "claude-fable-5-1", wt), h(2.0)).await;
    log_tool_message_at(&kernel, &opus, &agent, reply("o1", "claude-opus-5", main), h(3.0)).await;
    log_tool_message_at(&kernel, &opus, &agent, reply("o2", "claude-opus-5", main), h(4.9)).await;
    log_tool_message_at(&kernel, &laker, &agent, reply("l1", "claude-fable-5", lake.cwd()), h(5.0)).await;
    log_tool_message_at(&kernel, &laker, &agent, reply("l2", "claude-fable-5", lake.cwd()), h(6.0)).await;

    let runs = superx_mod_ui::thrown::model_runs(&kernel).await.expect("runs");
    let fable_run = runs.iter().find(|r| r.session.ends_with("/fable")).expect("fable run");
    assert_eq!(fable_run.model, "fable", "two point releases, one family, one run");
    assert_eq!(fable_run.messages, 2);
    assert_eq!(fable_run.minutes, 5, "two hours apart is one gap, capped at the live threshold");

    let c = superx_mod_ui::compare::compare(&runs, &std::collections::HashMap::new()).await;
    let row = |m: &str| c.deviations.iter().find(|d| d.model == m);
    let f = row("fable").expect("fable row");
    assert_eq!(f.added, 15, "the squash, once — not once per checkout");
    assert_eq!(f.alive, 15, "blamed at main, not at the branch the checkout has out");
    assert_eq!(f.commits, 1);
    assert_eq!(f.abandoned_lines, 7, "fable's abandoned branch; not the teammate's");
    assert_eq!(f.abandoned_commits, 1);
    assert_eq!(f.in_flight_lines, 4, "on a branch still checked out: not landed, not abandoned");
    assert!(row("opus").is_none_or(|o| o.added == 0), "merging it did not make it opus's");
    assert_eq!(c.unjudged.len(), 1, "{:?}", c.unjudged.iter().map(|u| &u.repo).collect::<Vec<_>>());
    assert_eq!(c.unjudged[0].repo, "lake");
    assert!(c.repos.iter().all(|r| r.repo == "superx"), "one repository row, not one per checkout");
}

/// A merged branch is deleted — here every one is — and with it the only
/// record of when its work was done (#415 review). Its commits are still
/// in the reflog of the checkout that made them, so its squash is credited
/// by when the branch was written, not by when it merged: the model that
/// wrote it, not the one running when it landed. The amend's leftover, in
/// that reflog too, is nobody's abandoned work.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_squash_whose_branch_is_gone_is_credited_to_who_wrote_it() {
    let kernel = fresh_kernel().await;
    let now = chrono::Utc::now();
    let t0 = now - chrono::Duration::hours(10);
    let h = |x: f64| t0 + chrono::Duration::seconds((x * 3600.0) as i64);
    let repo = TestRepo::new_dated("superx", "main", t0 - chrono::Duration::hours(1));
    let wt = repo.worktree("wt", "feat/a");
    let main = repo.cwd();
    let ten: String = (1..=10).map(|i| format!("line {i}\n")).collect();
    let fifteen: String = (1..=15).map(|i| format!("line {i}\n")).collect();

    // Fable writes the branch, then amends it.
    repo.commit(wt, h(1.0), "t@t", "feat: a", &[("a.rs", &ten)]);
    std::fs::write(std::path::Path::new(wt).join("a.rs"), &fifteen).expect("amend");
    repo.git(&["-C", wt, "commit", "-q", "-a", "--amend", "--no-edit"]);
    // The squash lands on main as the merging account, while Opus works.
    repo.commit(main, h(3.5), "noreply@github.com", "feat: a (#1)", &[("a.rs", &fifteen)]);
    // The checkout moves on and the branch is deleted, as merged branches are.
    repo.git(&["-C", wt, "checkout", "-q", "--detach"]);
    repo.git(&["branch", "-q", "-D", "feat/a"]);

    let reply = |id: &str, model: &str, cwd: &str| serde_json::json!({
        "cwd": cwd, "message": {"id": id, "model": model, "usage": {"output_tokens": 100},
            "content": [{"type": "text", "text": "working"}]}});
    let (agent, fable) = seed_agent_and_session(&kernel, "claude_code", "fable").await;
    let (_, opus) = seed_agent_and_session(&kernel, "claude_code", "opus").await;
    log_tool_message_at(&kernel, &fable, &agent, reply("f1", "claude-fable-5", wt), h(0.5)).await;
    log_tool_message_at(&kernel, &fable, &agent, reply("f2", "claude-fable-5", wt), h(1.5)).await;
    log_tool_message_at(&kernel, &opus, &agent, reply("o1", "claude-opus-5", main), h(3.0)).await;
    log_tool_message_at(&kernel, &opus, &agent, reply("o2", "claude-opus-5", main), h(4.0)).await;

    let runs = superx_mod_ui::thrown::model_runs(&kernel).await.expect("runs");
    let c = superx_mod_ui::compare::compare(&runs, &std::collections::HashMap::new()).await;
    let row = |m: &str| c.deviations.iter().find(|d| d.model == m);
    let f = row("fable").expect("fable wrote the branch");
    assert_eq!((f.added, f.commits), (15, 1), "the squash, by when the branch was written");
    assert_eq!(f.abandoned_lines, 0, "the amend's leftover is no abandoned work");
    assert!(row("opus").is_none_or(|o| o.added == 0), "merging it did not make it opus's");
}
