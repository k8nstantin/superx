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

    let events = session_activity(&kernel, session.clone(), 100)
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

    let events = global_activity(&kernel, 100).await.expect("global");
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

    let events = session_activity(&kernel, session, 2).await.expect("activity");
    assert_eq!(events.len(), 2, "merged page truncates to limit");
    // Newest-N: the pinned-to-bottom view must end at the present.
    assert!(events[0].rendered.contains("msg-2"), "{}", events[0].rendered);
    assert!(events[1].rendered.contains("msg-3"), "{}", events[1].rendered);
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

    let a = session_activity(&kernel, session_a, 100).await.expect("a");
    assert_eq!(a.len(), 1, "agent A's session sees its own action: {a:#?}");
    assert!(a[0].rendered.contains("tool_call"));

    let b = session_activity(&kernel, session_b, 100).await.expect("b");
    assert!(
        b.is_empty(),
        "agent B's session must NOT see agent A's action: {b:#?}"
    );
}
