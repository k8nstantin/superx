//! Gemini CLI adapter tests: header/`$set` lines kept whole, part-array
//! user text, every write of a streamed record kept, tool-call actions, a
//! file read whole from its start into its header's session (#518).

mod common;

use std::error::Error;
use std::fs;
use std::io::Write as _;

use superx_kernel::adapters::gemini_cli::{GeminiCliAdapter, ROOT_PARAM};
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
async fn every_write_is_kept_and_the_last_is_the_message() -> Result<(), Box<dyn Error>> {
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
        "user + the assistant record's last write: {:?}",
        messages.iter().map(|m| (&m.role, &m.content)).collect::<Vec<_>>()
    );
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[0].content, "hello gemini\nsecond part", "parts joined");
    assert_eq!(messages[1].role, "assistant");
    assert_eq!(messages[1].content, "here you go", "last re-emission wins");
    let raw = Value::Object(messages[1].raw.clone().expect("raw preserved"));
    assert_eq!(
        at(&raw, &["toolCalls"]).and_then(|c| match c {
            Value::Array(calls) => calls.first().and_then(|call| at(call, &["resultDisplay"])).cloned(),
            _ => None,
        }),
        Some(Value::Array(vec![Value::Array(vec![Value::String("junk".into())].into())].into())),
        "the call's resultDisplay is kept as written"
    );

    let recent = kernel.recent_telemetry(50).await?;
    let count = |name: &str| recent.iter().filter(|e| e.lifecycle_event == name).count();
    assert_eq!(count("tool_call"), 1, "a call is one action event, from its record's last write");
    let earlier = recent
        .iter()
        .find_map(|e| match at(&e.payload, &["kind"]) {
            Some(Value::String(kind)) if kind == "superseded" => at(&e.payload, &["line"]).cloned(),
            _ => None,
        })
        .ok_or("the record's earlier write is kept whole")?;
    assert_eq!(at(&earlier, &["id"]), Some(&Value::String("g-1".into())));
    assert_eq!(at(&earlier, &["content"]), Some(&Value::String(String::new())), "{earlier:?}");
    assert!(matches!(at(&earlier, &["thoughts"]), Some(Value::Array(_))), "{earlier:?}");
    assert!(count("transcript_event") >= 2, "info line and the earlier write captured");
    Ok(())
}

/// The value at `path` inside `v`, through objects.
fn at<'a>(v: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(v, |here, key| match here {
        Value::Object(o) => o.get(*key),
        _ => None,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_file_is_read_whole_from_its_start() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let tmp = tempfile::tempdir()?;
    arrange(&kernel, tmp.path()).await?;

    // The parameter that once capped a first read is set the old way, and
    // does nothing: no part of a file is skipped (#518).
    let entity = kernel.register_module(&GeminiCliAdapter.descriptor()).await?;
    kernel.set_parameter(entity, "attr_gemini_cli_backfill_max_bytes", Value::Number(120.into())).await?;
    let chats = tmp.path().join("tmp").join("proj-slug").join("chats");
    let file = chats.join("session-2026-08-07T09-00-gsess1.jsonl");
    let mut f = fs::OpenOptions::new().append(true).open(&file)?;
    writeln!(f, r#"{{"id":"u-9","type":"user","content":[{{"text":"tail msg"}}]}}"#)?;

    let sources = gemini_sources(&kernel).await?;
    let report = capture_tick(&kernel, &sources, None).await?;
    assert!(report.errors.is_empty(), "{:?}", report.errors);

    let recent = kernel.recent_telemetry(50).await?;
    assert!(recent.iter().all(|e| e.lifecycle_event != "backfill_capped"), "nothing is skipped");
    let session = kernel
        .find_entity_by_name("node_session", "attr_session_descriptor", "gemini_cli/g-sess-1")
        .await?
        .expect("session entity from header line");
    let contents: Vec<String> = kernel.session_messages(session, 20).await?.into_iter().map(|m| m.content).collect();
    assert!(contents.contains(&"hello gemini\nsecond part".to_string()), "the head is read: {contents:?}");
    assert!(contents.contains(&"tail msg".to_string()), "the tail is read: {contents:?}");
    Ok(())
}

/// A read that starts past the header still captures into the header's
/// session (#518): only the header names it, and Gemini's records carry no
/// `sessionId`. A later read once split a live chat across two sessions.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_later_read_captures_into_the_headers_session() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let tmp = tempfile::tempdir()?;
    arrange(&kernel, tmp.path()).await?;
    let sources = gemini_sources(&kernel).await?;
    let first = capture_tick(&kernel, &sources, None).await?;
    assert!(first.errors.is_empty(), "{:?}", first.errors);

    let chats = tmp.path().join("tmp").join("proj-slug").join("chats");
    let mut f = fs::OpenOptions::new().append(true).open(chats.join("session-2026-08-07T09-00-gsess1.jsonl"))?;
    writeln!(f, r#"{{"id":"u-2","type":"user","timestamp":"2026-08-07T09:01:00.000Z","content":[{{"text":"a later message"}}]}}"#)?;
    let later = capture_tick(&kernel, &sources, None).await?;
    assert!(later.errors.is_empty(), "{:?}", later.errors);
    assert!(later.total() >= 1, "the later line is read");

    let session = kernel
        .find_entity_by_name("node_session", "attr_session_descriptor", "gemini_cli/g-sess-1")
        .await?
        .expect("session entity from header line");
    let contents: Vec<String> = kernel.session_messages(session, 20).await?.into_iter().map(|m| m.content).collect();
    assert!(contents.contains(&"a later message".to_string()), "{contents:?}");
    assert!(
        kernel
            .find_entity_by_name("node_session", "attr_session_descriptor", "gemini_cli/session-2026-08-07T09-00-gsess1")
            .await?
            .is_none(),
        "no session is named after the file"
    );
    Ok(())
}

/// An attachment is kept as written (#518): its whole `inlineData`, never
/// cut to a few hundred characters.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_attachment_is_kept_whole() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let tmp = tempfile::tempdir()?;
    arrange(&kernel, tmp.path()).await?;
    let data = "A".repeat(1000);
    let chats = tmp.path().join("tmp").join("proj-slug").join("chats");
    let mut f = fs::OpenOptions::new().append(true).open(chats.join("session-2026-08-07T09-00-gsess1.jsonl"))?;
    writeln!(
        f,
        r#"{{"id":"u-3","type":"user","content":[{{"text":"see this"}},{{"inlineData":{{"mimeType":"image/png","data":"{data}"}}}}]}}"#
    )?;
    let sources = gemini_sources(&kernel).await?;
    let report = capture_tick(&kernel, &sources, None).await?;
    assert!(report.errors.is_empty(), "{:?}", report.errors);

    let session = kernel
        .find_entity_by_name("node_session", "attr_session_descriptor", "gemini_cli/g-sess-1")
        .await?
        .expect("session entity from header line");
    let messages = kernel.session_messages(session, 20).await?;
    let raw = messages.iter().find(|m| m.content == "see this").and_then(|m| m.raw.clone()).ok_or("the record is captured")?;
    let raw = Value::Object(raw);
    let kept = at(&raw, &["content"]).and_then(|c| match c {
        Value::Array(parts) => parts.get(1).and_then(|part| at(part, &["inlineData", "data"])).cloned(),
        _ => None,
    });
    assert_eq!(kept, Some(Value::String(data)), "the whole attachment");
    Ok(())
}

/// The header, every `$set` and every info line are kept whole (#471):
/// the header and `$set` lines were folded away unseen.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn header_set_and_info_lines_are_kept_whole() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let tmp = tempfile::tempdir()?;
    arrange(&kernel, tmp.path()).await?;
    let sources = gemini_sources(&kernel).await?;
    let report = capture_tick(&kernel, &sources, None).await?;
    assert!(report.errors.is_empty(), "{:?}", report.errors);

    let recent = kernel.recent_telemetry(50).await?;
    let kept: Vec<String> = recent
        .iter()
        .filter(|e| e.lifecycle_event == "transcript_event")
        .filter_map(|e| match &e.payload {
            Value::Object(o) => match (o.get("kind"), o.get("line")) {
                (Some(Value::String(kind)), Some(Value::Object(_))) => Some(kind.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    for kind in ["header", "$set", "info"] {
        assert!(kept.iter().any(|k| k == kind), "{kind} is kept whole: {kept:?}");
    }
    Ok(())
}

/// A record and a header keep their nulls (#516): read back from the
/// engine, a key holding `null` is still there.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_record_keeps_its_nulls() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let tmp = tempfile::tempdir()?;
    arrange(&kernel, tmp.path()).await?;
    let chats = tmp.path().join("tmp").join("proj-slug").join("chats");
    let body = [
        r#"{"sessionId":"g-sess-n","projectHash":"abc","startTime":"2026-08-07T10:00:00.000Z","lastUpdated":"2026-08-07T10:00:00.000Z","kind":"main","summary":null}"#,
        r#"{"id":"u-n","type":"user","timestamp":"2026-08-07T10:00:01.000Z","content":[{"text":"hi"}],"displayContent":null}"#,
    ]
    .join("\n");
    fs::write(chats.join("session-2026-08-07T10-00-gsessn.jsonl"), body + "\n")?;
    let sources = gemini_sources(&kernel).await?;
    let report = capture_tick(&kernel, &sources, None).await?;
    assert!(report.errors.is_empty(), "{:?}", report.errors);

    let session = kernel
        .find_entity_by_name("node_session", "attr_session_descriptor", "gemini_cli/g-sess-n")
        .await?
        .expect("session entity from header line");
    let messages = kernel.session_messages(session, 10).await?;
    let raw = messages.first().and_then(|m| m.raw.as_ref()).ok_or("the user record is captured")?;
    assert!(matches!(raw.get("displayContent"), Some(Value::Null)), "{raw:?}");

    let recent = kernel.recent_telemetry(50).await?;
    let header = recent
        .iter()
        .find_map(|e| match &e.payload {
            Value::Object(o) if matches!(o.get("session"), Some(Value::String(s)) if s == "g-sess-n")
                && matches!(o.get("kind"), Some(Value::String(k)) if k == "header") =>
            {
                o.get("line").cloned()
            }
            _ => None,
        })
        .ok_or("the header is kept whole")?;
    assert!(matches!(&header, Value::Object(o) if matches!(o.get("summary"), Some(Value::Null))), "{header:?}");
    Ok(())
}
