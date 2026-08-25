//! UI module contract tests (mem engine). The served page + live API
//! are QA'd against a real instance per the epic.

use superx_kernel::{Kernel, KernelModule, SCHEMA_DDL};
use superx_mod_ui::{
    resolved_context_window, resolved_port, resolved_url, UiModule, CONTEXT_WINDOW_PARAM,
    DEFAULT_CONTEXT_WINDOW, DEFAULT_PORT, MODULE_NAME, PORT_PARAM,
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
                     "input": {"new_string": "x\ny"}}
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
    kernel
        .log_message(msg(serde_json::json!({
            "model": "gemini-2.5-pro",
            "tokens": {"input": 10, "output": 5, "cached": 100}
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

    // An Edit: three lines replace one — added and removed are
    // different numbers, which the old lines_written could not say.
    log_tool_message(&kernel, &session, &agent, serde_json::json!({
        "cwd": "/Users/dev/projects/superx", "gitBranch": "feat/cockpit",
        "message": {"usage": {"output_tokens": 120, "output_tokens_details": {"thinking_tokens": 45}},
            "content": [{"type": "tool_use", "id": "t1", "name": "Edit",
                "input": {"file_path": "/Users/dev/projects/superx/crates/mod/src/stats.rs",
                          "old_string": "one line", "new_string": "a\nb\nc"}}]}
    })).await;

    // A test run, a build, and a git push — three shell classes.
    for cmd in ["cargo test --workspace", "cargo build --release", "git push -u origin HEAD"] {
        log_tool_message(&kernel, &session, &agent, serde_json::json!({
            "cwd": "/Users/dev/projects/superx",
            "message": {"content": [{"type": "tool_use", "id": "c", "name": "Bash",
                "input": {"command": cmd}}]}
        })).await;
    }

    // A read, an MCP call, a web fetch, a delegated subagent.
    for (name, input) in [
        ("Read", serde_json::json!({"file_path": "/Users/dev/projects/superx/README.md"})),
        ("mcp__gdx__search", serde_json::json!({})),
        ("WebFetch", serde_json::json!({})),
        ("Task", serde_json::json!({})),
    ] {
        log_tool_message(&kernel, &session, &agent, serde_json::json!({
            "cwd": "/Users/dev/projects/superx",
            "message": {"content": [{"type": "tool_use", "id": "x", "name": name, "input": input}]}
        })).await;
    }

    let s = superx_mod_ui::stats::stats_summary(&kernel, 500).await.expect("stats");

    // Code output, with the add/remove split the old figure lacked.
    assert_eq!(s.lines_added, 3, "new_string lines");
    assert_eq!(s.lines_removed, 1, "old_string lines");
    assert_eq!(s.files_touched, 2, "the edited file and the read one");
    assert_eq!(s.writes_window, 1);
    assert_eq!(s.reads_window, 1);

    // Language and directory mix come from the paths themselves.
    assert_eq!(s.languages.iter().find(|l| l.name == "rs").map(|l| l.value), Some(1));
    assert_eq!(s.languages.iter().find(|l| l.name == "md").map(|l| l.value), Some(1));
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
    assert_eq!(repeat.name, "cargo test");
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
    let a1 = kernel.create_entity("node_agent").await.expect("a1");
    let a2 = kernel.create_entity("node_agent").await.expect("a2");
    let s1 = kernel.create_entity("node_session").await.expect("s1");
    let s2 = kernel.create_entity("node_session").await.expect("s2");

    // Agent 1 writes in repo A with the fast model.
    kernel.log_message(superx_kernel::NewMessage {
        session: s1.clone(), agent: a1.clone(), role: "assistant".into(), content: String::new(),
        raw: Some(superx_kernel::message::json_to_object(&serde_json::json!({
            "cwd": "/w/alpha", "gitBranch": "main",
            "message": {"model": "claude-fable-5", "usage": {"output_tokens": 300},
                "content": [{"type": "tool_use", "id": "e1", "name": "Write",
                    "input": {"file_path": "/w/alpha/src/a.rs", "content": "1\n2\n3"}}]}}))),
        seq: None, emitted_at: None,
    }).await.expect("m1");

    // Agent 2 works repo B, runs a test suite, and the output says
    // what happened — the seam we had never read.
    kernel.log_message(superx_kernel::NewMessage {
        session: s2.clone(), agent: a2.clone(), role: "assistant".into(), content: String::new(),
        raw: Some(superx_kernel::message::json_to_object(&serde_json::json!({
            "cwd": "/w/beta",
            "message": {"model": "claude-opus-5",
                "content": [{"type": "tool_use", "id": "b1", "name": "Bash",
                    "input": {"command": "cargo test --workspace"}}]}}))),
        seq: None, emitted_at: None,
    }).await.expect("m2");
    kernel.log_message(superx_kernel::NewMessage {
        session: s2.clone(), agent: a2.clone(), role: "tool".into(), content: String::new(),
        raw: Some(superx_kernel::message::json_to_object(&serde_json::json!({
            "cwd": "/w/beta",
            "message": {"content": [{"type": "tool_result", "tool_use_id": "b1", "is_error": true,
                "content": "running 9 tests\ntest result: FAILED. 7 passed; 2 failed; 0 ignored\nerror[E0382]: borrow of moved value\nerror: could not compile `beta`"}]}}))),
        seq: None, emitted_at: None,
    }).await.expect("m3");

    // The operator had to step in, and the agent ran out of context.
    kernel.log_message(superx_kernel::NewMessage {
        session: s2.clone(), agent: a2.clone(), role: "user".into(), content: "stop".into(),
        raw: Some(superx_kernel::message::json_to_object(&serde_json::json!({
            "cwd": "/w/beta", "interruptedMessageId": "abc",
            "message": {"content": []}}))),
        seq: None, emitted_at: None,
    }).await.expect("m4");
    kernel.log_message(superx_kernel::NewMessage {
        session: s2.clone(), agent: a2.clone(), role: "system".into(), content: String::new(),
        raw: Some(superx_kernel::message::json_to_object(&serde_json::json!({
            "cwd": "/w/beta", "isCompactSummary": true, "message": {"content": []}}))),
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
