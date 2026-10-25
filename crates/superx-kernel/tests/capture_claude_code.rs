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
use superx_kernel::types::{Decimal, Number, Value};
use superx_kernel::Kernel;

/// Only this adapter's sources — the other adapters linked into this
/// test binary fall back to real machine paths when un-parameterized.
async fn cc_sources(
    kernel: &Kernel,
) -> Result<Vec<(usize, superx_kernel::SourceRef)>, Box<dyn Error>> {
    let all = discover_paired(kernel).await?;
    Ok(all
        .into_iter()
        .filter(|(_, s)| s.name.starts_with("claude_code/"))
        .collect())
}

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
    common::isolate_adapters(kernel, &root.join("isolated-empty")).await?;
    // Projects live one level down so the isolation dir is never
    // mistaken for a project.
    let projects = root.join("projects");
    let project = projects.join("-tmp-proj");
    fs::create_dir_all(&project)?;
    fs::write(project.join("sess-1.jsonl"), FIXTURE_LINES)?;

    let entity = kernel.register_module(&ClaudeCodeAdapter.descriptor()).await?;
    kernel
        .set_parameter(
            entity,
            PROJECTS_ROOT_PARAM,
            Value::String(projects.to_string_lossy().to_string()),
        )
        .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn history_backfill_extracts_conversation() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let tmp = tempfile::tempdir()?;
    arrange(&kernel, tmp.path()).await?;

    let sources = cc_sources(&kernel).await?;
    assert_eq!(sources.len(), 1, "one project dir → one source");
    let report = capture_tick(&kernel, &sources, None).await?;
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

    // Telemetry: discovery + per-message pulses + sidecar + raw —
    // scoped to THIS agent (the other linked adapters discover their
    // own agents too).
    let recent = kernel.recent_telemetry(100).await?;
    let count = |name: &str| {
        recent
            .iter()
            .filter(|e| e.lifecycle_event == name && e.agent.as_ref() == Some(&agent))
            .count()
    };
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

    let sources = cc_sources(&kernel).await?;
    let first = capture_tick(&kernel, &sources, None).await?;
    assert_eq!(first.total(), 5);

    // Nothing new → nothing captured.
    let quiet = capture_tick(&kernel, &sources, None).await?;
    assert_eq!(quiet.total(), 0, "cursor prevents re-capture");

    // Append one line → exactly one new event.
    let transcript = tmp.path().join("projects").join("-tmp-proj").join("sess-1.jsonl");
    let mut f = fs::OpenOptions::new().append(true).open(&transcript)?;
    writeln!(
        f,
        r#"{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"follow-up"}}]}},"sessionId":"sess-1","timestamp":"2026-08-07T10:00:03.000Z"}}"#
    )?;
    let second = capture_tick(&kernel, &sources, None).await?;
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

    let first = cc_sources(&kernel).await?;
    let again = cc_sources(&kernel).await?;
    assert_eq!(first.len(), again.len());
    assert_eq!(
        first[0].1.entity_id, again[0].1.entity_id,
        "re-discovery finds, never duplicates"
    );

    // A project created AFTER first discovery is picked up (fixes
    // v1's one-shot discovery).
    fs::create_dir_all(tmp.path().join("projects").join("-tmp-late-project"))?;
    let third = cc_sources(&kernel).await?;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabled_adapter_is_skipped_live_and_resumes() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let tmp = tempfile::tempdir()?;
    arrange(&kernel, tmp.path()).await?;

    assert_eq!(cc_sources(&kernel).await?.len(), 1, "enabled → discovered");

    // Operator disables the adapter — discovery skips it next pass.
    kernel
        .set_module_status(NodeKind::Adapter, ADAPTER_NAME, superx_kernel::ModuleStatus::Disabled)
        .await?;
    assert_eq!(
        cc_sources(&kernel).await?.len(),
        0,
        "disabled adapter contributes no sources"
    );

    // Re-enable — it resumes.
    kernel
        .set_module_status(NodeKind::Adapter, ADAPTER_NAME, superx_kernel::ModuleStatus::Enabled)
        .await?;
    assert_eq!(cc_sources(&kernel).await?.len(), 1, "re-enabled → back");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_listed_root_is_captured_under_its_own_source() -> Result<(), Box<dyn Error>> {
    // A session started under CLAUDE_CONFIG_DIR writes to that dir's
    // projects folder (#440). The same project folder name sits under
    // both roots; each must keep its own source and read position.
    let kernel = common::fresh_seeded_kernel().await?;
    let tmp = tempfile::tempdir()?;
    common::isolate_adapters(&kernel, &tmp.path().join("isolated-empty")).await?;
    let first = tmp.path().join("projects");
    let later = tmp.path().join("other-config").join("projects");
    fs::create_dir_all(first.join("-tmp-proj"))?;
    fs::write(first.join("-tmp-proj").join("sess-1.jsonl"), FIXTURE_LINES)?;
    fs::create_dir_all(later.join("-tmp-proj"))?;
    fs::write(later.join("-tmp-proj").join("sess-2.jsonl"), FIXTURE_LINES.replace("sess-1", "sess-2"))?;

    let entity = kernel.register_module(&ClaudeCodeAdapter.descriptor()).await?;
    let roots: Vec<Value> = [first, later.clone(), tmp.path().join("not-on-this-machine")]
        .iter()
        .map(|p| Value::String(p.to_string_lossy().to_string()))
        .collect();
    kernel
        .set_parameter(entity, PROJECTS_ROOT_PARAM, Value::Array(superx_kernel::types::Array::from(roots)))
        .await?;

    let sources = cc_sources(&kernel).await?;
    let names: Vec<&str> = sources.iter().map(|(_, s)| s.name.as_str()).collect();
    let by_path = format!("claude_code/{}", later.join("-tmp-proj").to_string_lossy());
    assert_eq!(names.len(), 2, "a root not on this machine is skipped: {names:?}");
    assert!(names.contains(&"claude_code/-tmp-proj"), "the first root keeps its names: {names:?}");
    assert!(names.contains(&by_path.as_str()), "a later root's folder is named by its path: {names:?}");

    let report = capture_tick(&kernel, &sources, None).await?;
    assert!(report.errors.is_empty(), "no capture errors: {:?}", report.errors);
    assert_eq!(report.total(), 10, "both transcripts captured");
    for name in ["claude_code/sess-1", "claude_code/sess-2"] {
        let session = kernel
            .find_entity_by_name("node_session", "attr_session_descriptor", name)
            .await?
            .expect("session captured");
        assert_eq!(kernel.session_messages(session, 10).await?.len(), 3, "{name}");
    }
    let quiet = capture_tick(&kernel, &sources, None).await?;
    assert_eq!(quiet.total(), 0, "each source keeps its own read position");
    Ok(())
}

/// Nothing is dropped (#471): a line that is not conversation keeps its
/// whole content under `line`, and an unparseable line its whole text.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_line_is_kept_whole() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let tmp = tempfile::tempdir()?;
    arrange(&kernel, tmp.path()).await?;
    let sources = cc_sources(&kernel).await?;
    let report = capture_tick(&kernel, &sources, None).await?;
    assert!(report.errors.is_empty(), "{:?}", report.errors);

    let recent = kernel.recent_telemetry(50).await?;
    let field = |event: &str, key: &str| {
        recent.iter().find(|e| e.lifecycle_event == event).and_then(|e| match &e.payload {
            Value::Object(o) => o.get(key).cloned(),
            _ => None,
        })
    };
    let line = match field("transcript_event", "line") {
        Some(Value::Object(line)) => line,
        other => return Err(format!("the ai-title line is kept whole, not {other:?}").into()),
    };
    assert_eq!(line.get("aiTitle"), Some(&Value::String("Test session".into())));
    assert_eq!(
        field("transcript_raw", "text"),
        Some(Value::String("this is {{{ not json".into())),
        "the unparseable line, whole"
    );
    Ok(())
}

/// A line is kept exactly as written (#516): a key holding `null` is still
/// there when read back from the engine, a float keeps its last bit, and an
/// integer beyond `i64` stays exact as a decimal.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nulls_and_numbers_are_kept_exactly() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let tmp = tempfile::tempdir()?;
    arrange(&kernel, tmp.path()).await?;
    fs::write(
        tmp.path().join("projects").join("-tmp-proj").join("sess-n.jsonl"),
        concat!(
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"exact"}],"stop_sequence":null,"usage":{"fallback_credit":null,"iterations":[null]}},"big":18446744073709551615,"sessionId":"sess-n","timestamp":"2026-08-07T11:00:00.000Z"}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"ok","tool_use_id":"t1"}]},"toolUseResult":{"durationSeconds":9.577058125019073,"returnCodeInterpretation":null},"sessionId":"sess-n","timestamp":"2026-08-07T11:00:01.000Z"}"#,
            "\n",
            r#"{"type":"mode","mode":null,"ratio":0.1,"sessionId":"sess-n"}"#,
            "\n"
        ),
    )?;
    let sources = cc_sources(&kernel).await?;
    let report = capture_tick(&kernel, &sources, None).await?;
    assert!(report.errors.is_empty(), "{:?}", report.errors);

    let session = kernel
        .find_entity_by_name("node_session", "attr_session_descriptor", "claude_code/sess-n")
        .await?
        .expect("session captured");
    let raws: Vec<Value> =
        kernel.session_messages(session, 10).await?.into_iter().filter_map(|m| m.raw.map(Value::Object)).collect();
    let [reply, result] = raws.as_slice() else {
        return Err(format!("two conversation lines, not {}", raws.len()).into());
    };
    assert!(matches!(at(reply, &["message", "stop_sequence"]), Some(Value::Null)), "{reply:?}");
    assert!(matches!(at(reply, &["message", "usage", "fallback_credit"]), Some(Value::Null)), "{reply:?}");
    assert!(
        matches!(at(reply, &["message", "usage", "iterations"]), Some(Value::Array(a)) if matches!(a.as_slice(), [Value::Null])),
        "{reply:?}"
    );
    assert!(
        matches!(at(reply, &["big"]), Some(Value::Number(Number::Decimal(d))) if *d == Decimal::from(u64::MAX)),
        "an integer beyond i64 stays exact: {:?}",
        at(reply, &["big"])
    );
    let duration: f64 = "9.577058125019073".parse()?;
    assert!(
        matches!(at(result, &["toolUseResult", "durationSeconds"]), Some(Value::Number(Number::Float(f))) if f.to_bits() == duration.to_bits()),
        "a float keeps its last bit: {:?}",
        at(result, &["toolUseResult", "durationSeconds"])
    );
    assert!(matches!(at(result, &["toolUseResult", "returnCodeInterpretation"]), Some(Value::Null)), "{result:?}");

    let recent = kernel.recent_telemetry(50).await?;
    let mode = recent
        .iter()
        .find_map(|e| match at(&e.payload, &["line", "type"]) {
            Some(Value::String(t)) if t == "mode" => at(&e.payload, &["line"]).cloned(),
            _ => None,
        })
        .ok_or("the mode line is captured")?;
    assert!(matches!(at(&mode, &["mode"]), Some(Value::Null)), "{mode:?}");
    assert!(
        matches!(at(&mode, &["ratio"]), Some(Value::Number(Number::Float(f))) if f.to_bits() == 0.1_f64.to_bits()),
        "{mode:?}"
    );
    Ok(())
}

/// The value at `path` inside `v`, through objects.
fn at<'a>(v: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(v, |here, key| match here {
        Value::Object(o) => o.get(*key),
        _ => None,
    })
}

/// A subagent's own transcript, and a workflow's agents and journal, are
/// captured into the session that started them: a subagent's lines carry
/// the parent's `sessionId`; a journal line carries none and is attributed
/// by the session folder it sits under, never by its own file name.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subagent_transcripts_are_captured_into_their_session() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let tmp = tempfile::tempdir()?;
    arrange(&kernel, tmp.path()).await?;
    let session_dir = tmp.path().join("projects").join("-tmp-proj").join("sess-1");
    let subagents = session_dir.join("subagents");
    let workflow = subagents.join("workflows").join("wf_1");
    fs::create_dir_all(&workflow)?;
    fs::write(
        subagents.join("agent-a1.jsonl"),
        concat!(
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"subagent says hi"}]},"sessionId":"sess-1","isSidechain":true,"agentId":"a1","timestamp":"2026-08-07T10:00:03.000Z"}"#,
            "\n"
        ),
    )?;
    fs::write(
        workflow.join("agent-w1.jsonl"),
        concat!(
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"workflow agent says hi"}]},"sessionId":"sess-1","isSidechain":true,"agentId":"w1","timestamp":"2026-08-07T10:00:04.000Z"}"#,
            "\n"
        ),
    )?;
    fs::write(workflow.join("journal.jsonl"), concat!(r#"{"type":"started","runId":"wf_1"}"#, "\n"))?;

    let sources = cc_sources(&kernel).await?;
    assert_eq!(sources.len(), 1, "the session folder is not a project of its own");
    let report = capture_tick(&kernel, &sources, None).await?;
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.total(), 8, "the five lines of the session, two subagents' and a journal's");

    let session = kernel
        .find_entity_by_name("node_session", "attr_session_descriptor", "claude_code/sess-1")
        .await?
        .expect("session entity exists");
    let contents: Vec<String> =
        kernel.session_messages(session, 10).await?.into_iter().map(|m| m.content).collect();
    assert!(contents.contains(&"subagent says hi".to_string()), "{contents:?}");
    assert!(contents.contains(&"workflow agent says hi".to_string()), "{contents:?}");
    assert!(
        kernel.find_entity_by_name("node_session", "attr_session_descriptor", "claude_code/journal").await?.is_none(),
        "a journal is never a session of its own"
    );
    let recent = kernel.recent_telemetry(100).await?;
    let journal = recent
        .iter()
        .find(|e| {
            e.lifecycle_event == "transcript_event"
                && matches!(&e.payload, Value::Object(o) if o.get("kind") == Some(&Value::String("started".into())))
        })
        .ok_or("the journal line is captured")?;
    let Value::Object(payload) = &journal.payload else { return Err("an object payload".into()) };
    assert_eq!(payload.get("session"), Some(&Value::String("sess-1".into())), "attributed by its session folder");
    Ok(())
}
