//! CLI runner + renderer tests against a mem:// substrate.

use std::error::Error;

use superx_kernel::capture::ensure_session;
use superx_kernel::types::{Object, Value};
use superx_kernel::{Kernel, NewMessage, REQUIRED_METAMODEL_TYPES, SCHEMA_DDL};

const TEST_PASSWORD: &str = "test-kernel-password-for-mem-engine";

async fn fresh_kernel() -> Result<Kernel, Box<dyn Error>> {
    let db = surrealdb::engine::any::connect("mem://").await?;
    db.use_ns("superx").use_db("kernel").await?;
    let ddl = SCHEMA_DDL.replace("$SUPERX_KERNEL_PASSWORD", TEST_PASSWORD);
    db.query(ddl).await?.check()?;
    db.signin(surrealdb::opt::auth::Database {
        namespace: "superx".to_string(),
        database: "kernel".to_string(),
        username: "superx_kernel".to_string(),
        password: TEST_PASSWORD.to_string(),
    })
    .await?;
    let kernel = Kernel::from_db(db);
    for t in REQUIRED_METAMODEL_TYPES {
        kernel
            .ensure_type_definition(t.uid, t.category, t.memory_tier)
            .await?;
    }
    Ok(kernel)
}

/// One agent + one session + two messages.
async fn seed_conversation(kernel: &Kernel) -> Result<(), Box<dyn Error>> {
    let agent = kernel.create_entity("node_agent").await?;
    let mut desc = Object::new();
    desc.insert("name".to_string(), Value::String("claude_code".to_string()));
    kernel
        .supersede_state(agent.clone(), "attr_agent_descriptor", Value::Object(desc))
        .await?;

    let session = ensure_session(kernel, &agent, "claude_code", "sess-42", "/tmp/x").await?;
    for (role, content) in [("user", "hello there"), ("assistant", "hi!\nsecond line")] {
        kernel
            .log_message(NewMessage {
                session: session.clone(),
                agent: agent.clone(),
                role: role.to_string(),
                content: content.to_string(),
                raw: None,
                seq: None,
                emitted_at: None,
            })
            .await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resolve_session_by_suffix_and_ambiguity() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;
    seed_conversation(&kernel).await?;

    // Exact, suffix, prefix, and middle fragments all resolve.
    assert!(superx::resolve_session(&kernel, "claude_code/sess-42").await.is_ok());
    assert!(superx::resolve_session(&kernel, "sess-42").await.is_ok());
    assert!(superx::resolve_session(&kernel, "claude_code/se").await.is_ok());
    assert!(superx::resolve_session(&kernel, "ess-4").await.is_ok());

    // The uuid7 identity resolves too — by full id and by fragment.
    let id = superx::resolve_session(&kernel, "sess-42").await?;
    let uuid = superx::record_uuid(&id);
    assert_eq!(superx::resolve_session(&kernel, &uuid).await?, id);
    assert_eq!(superx::resolve_session(&kernel, &uuid[..13]).await?, id);

    // Unknown fails with guidance.
    let err = superx::resolve_session(&kernel, "nope").await.unwrap_err();
    assert!(err.contains("superx sessions"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_renders_conversation_in_order() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;
    seed_conversation(&kernel).await?;

    let session = superx::resolve_session(&kernel, "sess-42").await?;
    let (text, high_water) = superx::run_read(&kernel, session).await?;
    assert!(high_water.is_some());
    let user_pos = text.find("user: hello there").expect("user line");
    let asst_pos = text.find("assistant: hi!").expect("assistant line");
    assert!(user_pos < asst_pos, "oldest first:\n{text}");
    assert!(
        text.contains("\n    second line"),
        "multiline content indented under the role line:\n{text}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sessions_lists_with_counts_and_agent_filter() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;
    seed_conversation(&kernel).await?;

    let all = superx::run_sessions(&kernel, None).await?;
    // Identity is <agent>/<uuid7>; the source-native id rides along.
    assert!(all.contains("claude_code/"), "{all}");
    assert!(all.contains("src=sess-42"), "{all}");
    assert!(all.contains("2 messages"), "{all}");
    let session = superx::resolve_session(&kernel, "sess-42").await.expect("by source id");
    let uuid = superx::record_uuid(&session);
    assert!(all.contains(&uuid), "listing shows the uuid7 identity: {all}");

    let filtered = superx::run_sessions(&kernel, Some("gemini_cli")).await?;
    assert!(filtered.contains("no sessions"), "{filtered}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actions_renders_and_filters_by_agent() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;
    seed_conversation(&kernel).await?;

    let agent = kernel
        .find_entity_by_name("node_agent", "attr_agent_descriptor", "claude_code")
        .await?
        .expect("agent");
    let mut payload = Object::new();
    payload.insert("role".to_string(), Value::String("user".to_string()));
    kernel
        .log_telemetry_for_agent(
            "message_captured",
            Value::Object(payload),
            None,
            Some(agent),
        )
        .await?;

    let (all, _) = superx::run_actions(&kernel, 20, None).await?;
    assert!(all.contains("message_captured"));
    assert!(all.contains("session_discovered"));

    let (filtered, _) = superx::run_actions(&kernel, 20, Some("claude_code")).await?;
    assert!(filtered.contains("message_captured"), "{filtered}");

    let missing = superx::run_actions(&kernel, 20, Some("ghost")).await;
    assert!(missing.is_err(), "unknown agent errors with guidance");
    Ok(())
}

#[test]
fn no_text_events_render_tool_summaries() {
    // Claude Code shape: assistant tool_use block with a description.
    let raw = superx_kernel::message::json_to_object(&serde_json::json!({
        "message": {"content": [
            {"type": "tool_use", "name": "Bash",
             "input": {"command": "git push", "description": "Commit and push G3"}},
            {"type": "thinking", "thinking": "hmm"}
        ]}
    }));
    let m = fake_message("assistant", "", Some(raw));
    let line = superx::render_message(&m);
    assert!(line.contains("⚙ Bash — Commit and push G3"), "{line}");
    assert!(line.contains("… thinking"), "{line}");
    assert!(!line.contains("no text"), "{line}");

    // Gemini shape: top-level toolCalls with status.
    let raw = superx_kernel::message::json_to_object(&serde_json::json!({
        "toolCalls": [{"name": "run_shell_command", "status": "success"}]
    }));
    let m = fake_message("assistant", "", Some(raw));
    let line = superx::render_message(&m);
    assert!(line.contains("⚙ run_shell_command [success]"), "{line}");

    // Truly opaque rows keep the placeholder.
    let m = fake_message("assistant", "", None);
    assert!(superx::render_message(&m).contains("(no text — see raw)"));
}

fn fake_message(
    role: &str,
    content: &str,
    raw: Option<superx_kernel::types::Object>,
) -> superx_kernel::MessageRecord {
    use superx_kernel::types::RecordId;
    let id = RecordId::new("message", superx_kernel::types::Uuid::from(uuid_v7()));
    let ent = |t: &'static str| RecordId::new(t, superx_kernel::types::Uuid::from(uuid_v7()));
    superx_kernel::MessageRecord {
        id,
        session: ent("entity"),
        agent: ent("entity"),
        role: role.to_string(),
        content: content.to_string(),
        raw,
        seq: None,
        emitted_at: None,
        valid_from: chrono::Utc::now(),
    }
}

fn uuid_v7() -> uuid::Uuid {
    uuid::Uuid::now_v7()
}
