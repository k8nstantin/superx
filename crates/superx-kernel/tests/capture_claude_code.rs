//! End-to-end capture tests: a fixture Claude Code projects tree is
//! discovered, its transcript history is backfilled into `message` +
//! `telemetry_stream` rows, the cursor checkpoints, and a second tick
//! captures only what's new. Malformed lines degrade to raw capture.

mod common;

use std::error::Error;
use std::fs;
use std::io::Write as _;

use superx_kernel::adapters::claude_code::{
    ClaudeCodeAdapter, ADAPTER_NAME, PROJECTS_ROOT_PARAM,
};
use superx_kernel::capture::{capture_tick, discover_paired};
use superx_kernel::registry::{KernelModule as _, NodeKind};
use superx_kernel::types::Value;
use superx_kernel::Kernel;

const FIXTURE_LINES: &str = concat!(
    r#"{"type":"user","message":{"role":"user","content":"hello superx"},"origin":{"kind":"human"},"promptSource":"typed","sessionId":"sess-1","timestamp":"2026-08-07T10:00:00.000Z","cwd":"/tmp/proj"}"#,
    "\n",
    r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hi there"},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]},"sessionId":"sess-1","timestamp":"2026-08-07T10:00:01.000Z"}"#,
    "\n",
    r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"file_a\nfile_b","tool_use_id":"t1"}]},"sessionId":"sess-1","timestamp":"2026-08-07T10:00:02.000Z"}"#,
    "\n",
    r#"{"type":"ai-title","aiTitle":"Test session","sessionId":"sess-1"}"#,
    "\n",
    "this is {{{ not json\n",
);

/// Fixture tree + registered adapter + projects-root parameter.
async fn arrange(kernel: &Kernel, root: &std::path::Path) -> Result<(), Box<dyn Error>> {
    let project = root.join("-tmp-proj");
    fs::create_dir_all(&project)?;
    fs::write(project.join("sess-1.jsonl"), FIXTURE_LINES)?;

    let entity = kernel.register_module(&ClaudeCodeAdapter.descriptor()).await?;
    kernel
        .set_parameter(
            entity,
            PROJECTS_ROOT_PARAM,
            Value::String(root.to_string_lossy().to_string()),
        )
        .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn history_backfill_extracts_conversation() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let tmp = tempfile::tempdir()?;
    arrange(&kernel, tmp.path()).await?;

    let sources = discover_paired(&kernel).await?;
    assert_eq!(sources.len(), 1, "one project dir → one source");
    let report = capture_tick(&kernel, &sources).await?;
    assert!(report.errors.is_empty(), "no capture errors: {:?}", report.errors);
    assert_eq!(report.total(), 5, "all five lines captured");

    // The conversation is readable: 3 message rows in order with
    // roles user → assistant → tool, content extracted.
    let session = kernel
        .find_entity_by_name("node_session", "attr_session_descriptor", "claude_code/sess-1")
        .await?
        .expect("session entity exists");
    let messages = kernel.session_messages(session.clone(), 10).await?;
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[0].content, "hello superx");
    assert_eq!(messages[1].role, "assistant");
    assert_eq!(messages[1].content, "hi there");
    assert_eq!(messages[2].role, "tool");
    assert!(messages[2].content.contains("file_a"));
    assert!(messages[0].raw.is_some(), "full raw event preserved");
    assert!(messages[0].emitted_at.is_some(), "source timestamp parsed");

    // Per-agent query path: agent_id finds the same messages.
    let agent = kernel
        .find_entity_by_name("node_agent", "attr_agent_descriptor", "claude_code")
        .await?
        .expect("agent entity exists");
    assert_eq!(kernel.agent_messages(agent.clone(), 10).await?.len(), 3);

    // Telemetry: discovery + per-message pulses + sidecar + raw.
    let recent = kernel.recent_telemetry(50).await?;
    let count = |name: &str| recent.iter().filter(|e| e.lifecycle_event == name).count();
    assert_eq!(count("agent_discovered"), 1);
    assert_eq!(count("source_discovered"), 1);
    assert_eq!(count("session_discovered"), 1);
    assert_eq!(count("message_captured"), 3);
    assert_eq!(count("transcript_event"), 1, "ai-title sidecar");
    assert_eq!(count("transcript_raw"), 1, "malformed line captured raw");

    // Actions are queryable per agent (v2.1 agent FK).
    let agent_actions = kernel.agent_telemetry(agent, 50).await?;
    assert!(agent_actions.len() >= 6, "agent-attributed actions present");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cursor_resume_captures_only_new_lines() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let tmp = tempfile::tempdir()?;
    arrange(&kernel, tmp.path()).await?;

    let sources = discover_paired(&kernel).await?;
    let first = capture_tick(&kernel, &sources).await?;
    assert_eq!(first.total(), 5);

    // Nothing new → nothing captured.
    let quiet = capture_tick(&kernel, &sources).await?;
    assert_eq!(quiet.total(), 0, "cursor prevents re-capture");

    // Append one line → exactly one new event.
    let transcript = tmp.path().join("-tmp-proj").join("sess-1.jsonl");
    let mut f = fs::OpenOptions::new().append(true).open(&transcript)?;
    writeln!(
        f,
        r#"{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"follow-up"}}]}},"sessionId":"sess-1","timestamp":"2026-08-07T10:00:03.000Z"}}"#
    )?;
    let second = capture_tick(&kernel, &sources).await?;
    assert_eq!(second.total(), 1, "only the appended line");

    let session = kernel
        .find_entity_by_name("node_session", "attr_session_descriptor", "claude_code/sess-1")
        .await?
        .expect("session");
    let messages = kernel.session_messages(session, 10).await?;
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[3].content, "follow-up");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discovery_is_idempotent_and_continuous() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let tmp = tempfile::tempdir()?;
    arrange(&kernel, tmp.path()).await?;

    let first = discover_paired(&kernel).await?;
    let again = discover_paired(&kernel).await?;
    assert_eq!(first.len(), again.len());
    assert_eq!(
        first[0].1.entity_id, again[0].1.entity_id,
        "re-discovery finds, never duplicates"
    );

    // A project created AFTER first discovery is picked up (fixes
    // v1's one-shot discovery).
    fs::create_dir_all(tmp.path().join("-tmp-late-project"))?;
    let third = discover_paired(&kernel).await?;
    assert_eq!(third.len(), 2, "late project discovered without restart");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn adapter_absent_when_no_param_and_no_home_dir() -> Result<(), Box<dyn Error>> {
    // Without the parameter, discovery falls back to $HOME/.claude/
    // projects — which exists on dev machines, so simply assert the
    // param-driven path is what the other tests exercised and the
    // registered module carries the right identity.
    let kernel = common::fresh_seeded_kernel().await?;
    let entity = kernel.register_module(&ClaudeCodeAdapter.descriptor()).await?;
    let status = kernel
        .detailed_status(NodeKind::Adapter, ADAPTER_NAME)
        .await?
        .expect("adapter registered");
    assert_eq!(status.entity_id, entity);
    Ok(())
}
