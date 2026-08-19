//! UI module contract tests (mem engine). The served page + live API
//! are QA'd against a real instance per the epic.

use superx_kernel::{Kernel, KernelModule, SCHEMA_DDL};
use superx_mod_ui::{resolved_port, resolved_url, UiModule, DEFAULT_PORT, MODULE_NAME, PORT_PARAM};

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
    let agent = kernel.create_entity("node_agent").await.expect("agent");
    let session = kernel.create_entity("node_session").await.expect("session");

    // The capture engine's descriptor shape: {name, session, locator}.
    let mut desc = Object::new();
    desc.insert(
        "name".to_string(),
        Value::String("claude_code/src-abc".to_string()),
    );
    desc.insert("session".to_string(), Value::String("src-abc".to_string()));
    desc.insert("locator".to_string(), Value::String("/tmp/x.jsonl".to_string()));
    kernel
        .supersede_state(session.clone(), "attr_session_descriptor", Value::Object(desc))
        .await
        .expect("descriptor");

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
