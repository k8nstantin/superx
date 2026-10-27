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
    assert!(messages[0].raw.is_none(), "no parsed copy: the line is kept as its text (#541)");
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
/// whole text under `raw_text`, with no parsed copy (#541), and an
/// unparseable line its whole text.
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
    assert_eq!(
        field("transcript_event", "raw_text"),
        Some(Value::String(r#"{"type":"ai-title","aiTitle":"Test session","sessionId":"sess-1"}"#.into())),
        "the ai-title line, whole, as its text"
    );
    assert_eq!(field("transcript_event", "line"), None, "no parsed copy (#541)");
    assert_eq!(
        field("transcript_raw", "text"),
        Some(Value::String("this is {{{ not json".into())),
        "the unparseable line, whole"
    );
    Ok(())
}

/// A line is kept exactly as written (#516), as its text (#541): a key
/// holding `null`, a float's every digit and an integer beyond `i64` are
/// there character for character, and no parsed copy is written.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nulls_and_numbers_are_kept_exactly() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let tmp = tempfile::tempdir()?;
    arrange(&kernel, tmp.path()).await?;
    let reply = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"exact"}],"stop_sequence":null,"usage":{"fallback_credit":null,"iterations":[null]}},"big":18446744073709551615,"sessionId":"sess-n","timestamp":"2026-08-07T11:00:00.000Z"}"#;
    let result = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"ok","tool_use_id":"t1"}]},"toolUseResult":{"durationSeconds":9.577058125019073,"returnCodeInterpretation":null},"sessionId":"sess-n","timestamp":"2026-08-07T11:00:01.000Z"}"#;
    let mode = r#"{"type":"mode","mode":null,"ratio":0.1,"sessionId":"sess-n"}"#;
    fs::write(
        tmp.path().join("projects").join("-tmp-proj").join("sess-n.jsonl"),
        format!("{reply}\n{result}\n{mode}\n"),
    )?;
    let sources = cc_sources(&kernel).await?;
    let report = capture_tick(&kernel, &sources, None).await?;
    assert!(report.errors.is_empty(), "{:?}", report.errors);

    let session = kernel
        .find_entity_by_name("node_session", "attr_session_descriptor", "claude_code/sess-n")
        .await?
        .expect("session captured");
    let messages = kernel.session_messages(session.clone(), 10).await?;
    assert_eq!(messages.len(), 2, "two conversation lines");
    assert!(messages.iter().all(|m| m.raw.is_none()), "no parsed copy (#541)");
    let rows: Vec<Value> = kernel
        .db()
        .query("SELECT raw_text, valid_from FROM message WHERE session = $sess ORDER BY valid_from")
        .bind(("sess", session))
        .await?
        .take(0)?;
    let texts: Vec<Option<&Value>> = rows.iter().map(|r| at(r, &["raw_text"])).collect();
    assert_eq!(
        texts,
        vec![Some(&Value::String(reply.into())), Some(&Value::String(result.into()))],
        "each line, nulls and numbers as written"
    );

    let recent = kernel.recent_telemetry(50).await?;
    let mode_kept = recent.iter().any(|e| {
        matches!(at(&e.payload, &["raw_text"]), Some(Value::String(t)) if t == mode) && at(&e.payload, &["line"]).is_none()
    });
    assert!(mode_kept, "the mode line, as written, with no parsed copy");
    Ok(())
}

/// A line nested as deep as an agent writes it is kept as its text (#541):
/// capture writes no parsed copy, so how deep a line nests is never its
/// concern. A Jira tool result nested 23 levels deep, its comments in
/// Atlassian Document Format, was waiting when capture stopped for 25 hours.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deeply_nested_line_is_kept_as_its_text() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let tmp = tempfile::tempdir()?;
    arrange(&kernel, tmp.path()).await?;
    // Eighty levels: deeper than that line, within what serde_json parses.
    let deep = format!("{}{}", r#"{"content":["#.repeat(40), "]}".repeat(40));
    let user = format!(
        r#"{{"type":"user","message":{{"role":"user","content":"deep"}},"mcpMeta":{deep},"sessionId":"sess-d","timestamp":"2026-08-07T13:00:00.000Z"}}"#
    );
    let attachment = format!(r#"{{"type":"attachment","attachment":{deep},"sessionId":"sess-d"}}"#);
    fs::write(
        tmp.path().join("projects").join("-tmp-proj").join("sess-d.jsonl"),
        format!("{user}\n{attachment}\n"),
    )?;
    let sources = cc_sources(&kernel).await?;
    let report = capture_tick(&kernel, &sources, None).await?;
    assert!(report.errors.is_empty(), "{:?}", report.errors);

    let session = kernel
        .find_entity_by_name("node_session", "attr_session_descriptor", "claude_code/sess-d")
        .await?
        .expect("session captured");
    let messages = kernel.session_messages(session.clone(), 10).await?;
    assert_eq!(messages.len(), 1, "the user line is a message");
    assert_eq!(messages[0].content, "deep");
    assert!(messages[0].raw.is_none(), "no parsed copy (#541)");
    let rows: Vec<Value> = kernel
        .db()
        .query("SELECT raw_text FROM message WHERE session = $sess")
        .bind(("sess", session))
        .await?
        .take(0)?;
    assert_eq!(rows.first().and_then(|r| at(r, &["raw_text"])), Some(&Value::String(user.clone())), "as written");

    let recent = kernel.recent_telemetry(100).await?;
    let kept = recent.iter().any(|e| {
        e.lifecycle_event == "transcript_event"
            && matches!(at(&e.payload, &["raw_text"]), Some(Value::String(t)) if *t == attachment)
            && at(&e.payload, &["line"]).is_none()
    });
    assert!(kept, "the attachment line, as written, with no parsed copy");
    Ok(())
}

/// The files beside the transcripts are captured whole (#520): a saved tool
/// output and a subagent's description into their session, a memory file as
/// the project's; again only when one changes; a subagent's transcript stays
/// lines, never a file.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn files_beside_the_transcripts_are_captured_whole() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let tmp = tempfile::tempdir()?;
    arrange(&kernel, tmp.path()).await?;
    let project = tmp.path().join("projects").join("-tmp-proj");
    let session_dir = project.join("sess-1");
    fs::create_dir_all(session_dir.join("tool-results"))?;
    fs::create_dir_all(session_dir.join("subagents"))?;
    fs::create_dir_all(project.join("memory"))?;
    fs::write(session_dir.join("tool-results").join("toolu_1.txt"), "a long output\nline two\n")?;
    fs::write(session_dir.join("subagents").join("agent-a1.meta.json"), r#"{"agentType":"general-purpose","description":"look"}"#)?;
    fs::write(
        session_dir.join("subagents").join("agent-a1.jsonl"),
        concat!(
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"subagent says hi"}]},"sessionId":"sess-1","isSidechain":true,"timestamp":"2026-08-07T10:00:03.000Z"}"#,
            "\n"
        ),
    )?;
    fs::write(project.join("memory").join("MEMORY.md"), "# Memory\n")?;

    let sources = cc_sources(&kernel).await?;
    let first = capture_tick(&kernel, &sources, None).await?;
    assert!(first.errors.is_empty(), "{:?}", first.errors);
    assert_eq!(first.total(), 9, "the session's 5 lines, the subagent's 1 and 3 files");
    let files = file_events(&kernel).await?;
    assert_eq!(files.len(), 3, "{files:?}");
    assert!(files.contains(&("sess-1/tool-results/toolu_1.txt".into(), Some("sess-1".into()), "a long output\nline two\n".into())), "{files:?}");
    assert!(
        files.contains(&(
            "sess-1/subagents/agent-a1.meta.json".into(),
            Some("sess-1".into()),
            r#"{"agentType":"general-purpose","description":"look"}"#.into()
        )),
        "{files:?}"
    );
    assert!(files.contains(&("memory/MEMORY.md".into(), None, "# Memory\n".into())), "the project's own: {files:?}");

    let quiet = capture_tick(&kernel, &sources, None).await?;
    assert_eq!(quiet.total(), 0, "an unchanged file is not captured again");

    fs::write(project.join("memory").join("MEMORY.md"), "# Memory\n- a new line\n")?;
    let changed = capture_tick(&kernel, &sources, None).await?;
    assert_eq!(changed.total(), 1, "only the changed file");
    let files = file_events(&kernel).await?;
    assert_eq!(files.len(), 4, "{files:?}");
    assert!(files.contains(&("memory/MEMORY.md".into(), None, "# Memory\n- a new line\n".into())), "{files:?}");
    Ok(())
}

/// Every `file` event captured: its path in the project, its session, and
/// its content.
async fn file_events(kernel: &Kernel) -> Result<Vec<(String, Option<String>, String)>, Box<dyn Error>> {
    Ok(kernel
        .recent_telemetry(200)
        .await?
        .into_iter()
        .filter(|e| e.lifecycle_event == "transcript_event")
        .filter(|e| matches!(at(&e.payload, &["kind"]), Some(Value::String(k)) if k == "file"))
        .map(|e| {
            let text = |path: &[&str]| match at(&e.payload, path) {
                Some(Value::String(s)) => Some(s.clone()),
                _ => None,
            };
            (
                text(&["line", "path"]).unwrap_or_default(),
                text(&["session"]),
                text(&["line", "content"]).unwrap_or_default(),
            )
        })
        .collect())
}

/// Every line keeps its exact text (#528): a key's order, a number's
/// spelling and the spacing a parser does not keep. A
/// message holds it in `raw_text`; any other line in its payload's
/// `raw_text`; an unparseable line too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_line_keeps_its_exact_text() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let tmp = tempfile::tempdir()?;
    arrange(&kernel, tmp.path()).await?;
    let user = r#"{"type":"user","z":1,"message":{"role":"user","content":"hi"},"a":1.0,"sessionId":"sess-x","timestamp":"2026-08-07T12:00:00.000Z"}"#;
    let mode = r#"{"type":"mode",  "mode":"normal","b":2,"a":1,"sessionId":"sess-x"}"#;
    let broken = "not json {";
    fs::write(
        tmp.path().join("projects").join("-tmp-proj").join("sess-x.jsonl"),
        format!("{user}\n{mode}\n{broken}\n"),
    )?;
    let sources = cc_sources(&kernel).await?;
    let report = capture_tick(&kernel, &sources, None).await?;
    assert!(report.errors.is_empty(), "{:?}", report.errors);

    let session = kernel
        .find_entity_by_name("node_session", "attr_session_descriptor", "claude_code/sess-x")
        .await?
        .expect("session captured");
    let rows: Vec<Value> = kernel
        .db()
        .query("SELECT raw_text, valid_from FROM message WHERE session = $sess ORDER BY valid_from")
        .bind(("sess", session))
        .await?
        .take(0)?;
    let texts: Vec<Option<&Value>> = rows.iter().map(|r| at(r, &["raw_text"])).collect();
    assert_eq!(texts, vec![Some(&Value::String(user.into()))], "the message, exactly as written");

    let recent = kernel.recent_telemetry(100).await?;
    let text_of = |event: &str, wanted: &str| {
        recent.iter().any(|e| {
            e.lifecycle_event == event && matches!(at(&e.payload, &["raw_text"]), Some(Value::String(s)) if s == wanted)
        })
    };
    assert!(text_of("transcript_event", mode), "the mode line, exactly as written");
    assert!(text_of("transcript_raw", broken), "the unparseable line, exactly as written");
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
