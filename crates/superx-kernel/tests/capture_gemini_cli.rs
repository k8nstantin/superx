//! Gemini CLI adapter tests: header/`$set` folding, part-array user
//! text, re-emission dedupe, tool-call actions, backfill cap.

mod common;

use std::error::Error;
use std::fs;
use std::io::Write as _;

use superx_kernel::adapters::gemini_cli::{
    GeminiCliAdapter, BACKFILL_MAX_PARAM, ROOT_PARAM,
};
use superx_kernel::capture::{capture_tick, discover_paired};
use superx_kernel::registry::KernelModule as _;
use superx_kernel::types::Value;
use superx_kernel::Kernel;

const HEADER: &str = r#"{"sessionId":"g-sess-1","projectHash":"abc","startTime":"2026-08-07T09:00:00.000Z","lastUpdated":"2026-08-07T09:00:00.000Z","kind":"main"}"#;
const SET_LINE: &str = r#"{"$set":{"lastUpdated":"2026-08-07T09:00:01.000Z"}}"#;
const USER_LINE: &str = r#"{"id":"u-1","type":"user","timestamp":"2026-08-07T09:00:02.000Z","content":[{"text":"hello gemini"},{"text":"second part"}]}"#;
// The same assistant record re-emitted twice — the fuller one last.
const GEMINI_PARTIAL: &str = r#"{"id":"g-1","type":"gemini","timestamp":"2026-08-07T09:00:03.000Z","content":"","thoughts":[{"subject":"thinking"}]}"#;
const GEMINI_FULL: &str = r#"{"id":"g-1","type":"gemini","timestamp":"2026-08-07T09:00:04.000Z","content":"here you go","model":"gemini-3.1-pro","toolCalls":[{"id":"tc1","name":"run_shell_command","args":{"command":"ls"},"status":"success","resultDisplay":[["junk"]]}]}"#;
const INFO_LINE: &str = r#"{"id":"i-1","type":"info","timestamp":"2026-08-07T09:00:05.000Z","content":"[API notice]"}"#;

async fn arrange(kernel: &Kernel, root: &std::path::Path) -> Result<(), Box<dyn Error>> {
    common::isolate_adapters(kernel, &root.join("isolated-empty")).await?;
    let chats = root.join("tmp").join("proj-slug").join("chats");
    fs::create_dir_all(&chats)?;
    let body = [HEADER, SET_LINE, USER_LINE, GEMINI_PARTIAL, GEMINI_FULL, INFO_LINE].join("\n");
    fs::write(chats.join("session-2026-08-07T09-00-gsess1.jsonl"), body + "\n")?;

    let entity = kernel.register_module(&GeminiCliAdapter.descriptor()).await?;
    kernel
        .set_parameter(
            entity,
            ROOT_PARAM,
            Value::String(root.to_string_lossy().to_string()),
        )
        .await?;
    Ok(())
}

/// Find the gemini source among all adapters' discoveries (the Claude
/// Code + Desktop adapters are linked into this test binary too, but
/// have no parameter set — on machines where their default roots
/// exist they may surface sources, so filter by adapter name).
async fn gemini_sources(
    kernel: &Kernel,
) -> Result<Vec<(usize, superx_kernel::SourceRef)>, Box<dyn Error>> {
    let all = discover_paired(kernel).await?;
    Ok(all
        .into_iter()
        .filter(|(_, s)| s.name.starts_with("gemini_cli/"))
        .collect())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conversation_extracted_with_dedupe_and_tools() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let tmp = tempfile::tempdir()?;
    arrange(&kernel, tmp.path()).await?;

    let sources = gemini_sources(&kernel).await?;
    assert_eq!(sources.len(), 1, "one project slug → one source");
    let report = capture_tick(&kernel, &sources, None).await?;
    assert!(report.errors.is_empty(), "{:?}", report.errors);

    let session = kernel
        .find_entity_by_name("node_session", "attr_session_descriptor", "gemini_cli/g-sess-1")
        .await?
        .expect("session entity from header line");
    let messages = kernel.session_messages(session, 10).await?;
    assert_eq!(
        messages.len(),
        2,
        "user + ONE deduped assistant (re-emission collapsed): {:?}",
        messages.iter().map(|m| (&m.role, &m.content)).collect::<Vec<_>>()
    );
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[0].content, "hello gemini\nsecond part", "parts joined");
    assert_eq!(messages[1].role, "assistant");
    assert_eq!(messages[1].content, "here you go", "last re-emission wins");
    let raw = messages[1].raw.as_ref().expect("raw preserved");
    assert!(
        !format!("{raw:?}").contains("resultDisplay"),
        "render junk stripped from raw"
    );

    let recent = kernel.recent_telemetry(50).await?;
    let count = |name: &str| recent.iter().filter(|e| e.lifecycle_event == name).count();
    assert_eq!(count("tool_call"), 1, "tool call becomes an action event");
    assert!(count("transcript_event") >= 1, "info line captured");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn backfill_cap_skips_oversized_history() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let tmp = tempfile::tempdir()?;
    arrange(&kernel, tmp.path()).await?;

    // Cap the backfill window very low, then bulk up the file so only
    // the tail fits.
    let entity = kernel.register_module(&GeminiCliAdapter.descriptor()).await?;
    kernel
        .set_parameter(entity, BACKFILL_MAX_PARAM, Value::Number(120.into()))
        .await?;
    let chats = tmp.path().join("tmp").join("proj-slug").join("chats");
    let file = chats.join("session-2026-08-07T09-00-gsess1.jsonl");
    let mut f = fs::OpenOptions::new().append(true).open(&file)?;
    // Tail line that must be captured (within the 120-byte window).
    writeln!(f, r#"{{"id":"u-9","type":"user","content":[{{"text":"tail msg"}}]}}"#)?;

    let sources = gemini_sources(&kernel).await?;
    let report = capture_tick(&kernel, &sources, None).await?;
    assert!(report.errors.is_empty());

    // The oversized head was skipped: telemetry says so, and the
    // early "hello gemini" user message was never ingested.
    let recent = kernel.recent_telemetry(50).await?;
    assert!(
        recent.iter().any(|e| e.lifecycle_event == "backfill_capped"),
        "cap is announced, never silent"
    );
    let agent = kernel
        .find_entity_by_name("node_agent", "attr_agent_descriptor", "gemini_cli")
        .await?
        .expect("agent");
    let msgs = kernel.agent_messages(agent, 20).await?;
    assert!(
        msgs.iter().all(|m| m.content != "hello gemini\nsecond part"),
        "head beyond the cap not ingested"
    );
    assert!(
        msgs.iter().any(|m| m.content == "tail msg"),
        "tail within the cap ingested"
    );
    Ok(())
}
