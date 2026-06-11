//! End-to-end tests for the transcript watcher, driven through the
//! real capture framework (`capture_tick`) against fixture transcript
//! directories — the heart of the FVP verified without timers.

use std::error::Error;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::PathBuf;

use surrealdb::engine::any::connect;
use surrealdb::opt::auth::Database;

use superx_driver_claude_code::{ClaudeCodeDriver, DRIVER_NAME, PROJECTS_ROOT_PARAM};
use superx_kernel::types::Value;
use superx_kernel::{Kernel, KernelModule, NodeKind, SCHEMA_DDL};
use superx_kernel_capture::{capture_tick, CaptureModule};
use superx_kernel_discovery::{run_probes, DiscoveryModule};

const TEST_PASSWORD: &str = "test-kernel-password-for-mem-engine";

/// Substrate with kernel/discovery/capture/driver metamodel seeded,
/// modules registered, fixture projects-root configured, and one
/// probe pass run so the source entities exist.
async fn fixture(projects: &[&str]) -> Result<(Kernel, PathBuf), Box<dyn Error>> {
    let db = connect("mem://").await?;
    db.use_ns("superx").use_db("kernel").await?;
    let ddl = SCHEMA_DDL.replace("$SUPERX_KERNEL_PASSWORD", TEST_PASSWORD);
    db.query(ddl).await?.check()?;
    db.signin(Database {
        namespace: "superx".to_string(),
        database: "kernel".to_string(),
        username: "superx_kernel".to_string(),
        password: TEST_PASSWORD.to_string(),
    })
    .await?;
    let kernel = Kernel::from_db(db);
    for t in superx_kernel::REQUIRED_METAMODEL_TYPES {
        kernel.ensure_type_definition(t.uid, t.category, t.memory_tier).await?;
    }
    for module in [
        &DiscoveryModule as &dyn KernelModule,
        &CaptureModule as &dyn KernelModule,
        &ClaudeCodeDriver as &dyn KernelModule,
    ] {
        let desc = module.descriptor();
        for t in desc.required_metamodel {
            kernel.ensure_type_definition(t.uid, t.category, t.memory_tier).await?;
        }
        kernel.register_module(&desc).await?;
    }

    let root = std::env::temp_dir()
        .join("superx-claude-code-watcher")
        .join(uuid::Uuid::now_v7().to_string());
    for p in projects {
        fs::create_dir_all(root.join(p))?;
    }
    let entity_id = kernel
        .detailed_status(NodeKind::Contribution, DRIVER_NAME)
        .await?
        .expect("driver registered")
        .entity_id;
    kernel
        .set_parameter(
            entity_id,
            PROJECTS_ROOT_PARAM,
            Value::String(root.display().to_string()),
        )
        .await?;
    run_probes(&kernel).await?;
    Ok((kernel, root))
}

fn append(path: &std::path::Path, lines: &[&str]) -> Result<(), Box<dyn Error>> {
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    for line in lines {
        writeln!(f, "{line}")?;
    }
    Ok(())
}

fn count_events(events: &[superx_kernel::TelemetryRecord], name: &str) -> usize {
    events.iter().filter(|e| e.lifecycle_event == name).count()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_tick_checkpoints_without_capturing_history() -> Result<(), Box<dyn Error>> {
    let (kernel, root) = fixture(&["-Users-tester-alpha"]).await?;
    let transcript = root.join("-Users-tester-alpha").join("session-1.jsonl");
    append(&transcript, &[r#"{"type":"user","uuid":"pre-existing"}"#])?;

    let report = capture_tick(&kernel).await?;
    assert_eq!(report.sources_polled, 1);
    assert_eq!(report.events_captured, 0, "capture-from-now: history is not replayed");
    assert_eq!(report.errors, 0);

    let events = kernel.recent_telemetry(100).await?;
    assert_eq!(count_events(&events, "transcript_event"), 0);

    fs::remove_dir_all(&root).ok();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn appended_lines_become_telemetry_and_cursor_resumes() -> Result<(), Box<dyn Error>> {
    let (kernel, root) = fixture(&["-Users-tester-alpha"]).await?;
    let transcript = root.join("-Users-tester-alpha").join("session-1.jsonl");
    append(&transcript, &[r#"{"type":"user","uuid":"old"}"#])?;

    capture_tick(&kernel).await?; // checkpoint at current end

    append(
        &transcript,
        &[
            r#"{"type":"user","uuid":"u-1","sessionId":"s-1"}"#,
            r#"{"type":"assistant","uuid":"a-1","sessionId":"s-1"}"#,
            "this line is not json {{{",
        ],
    )?;

    let report = capture_tick(&kernel).await?;
    assert_eq!(report.events_captured, 3);
    assert_eq!(report.errors, 0);

    let events = kernel.recent_telemetry(100).await?;
    assert_eq!(count_events(&events, "transcript_event"), 2);
    assert_eq!(
        count_events(&events, "transcript_raw"),
        1,
        "format drift degrades to raw capture, never a crash",
    );
    // Structured payloads carry the parsed fields.
    let structured = events
        .iter()
        .find(|e| e.lifecycle_event == "transcript_event")
        .expect("structured event present");
    let rendered = serde_json::to_string(&structured.payload)?;
    assert!(rendered.contains("claude_code"), "agent stamped: {rendered}");
    assert!(rendered.contains("session-1.jsonl"), "file stamped: {rendered}");

    // Third tick: nothing new — cursor advanced past everything.
    let report = capture_tick(&kernel).await?;
    assert_eq!(report.events_captured, 0, "cursor must resume, not replay");

    fs::remove_dir_all(&root).ok();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn new_file_appearing_mid_stream_is_captured_from_start() -> Result<(), Box<dyn Error>> {
    let (kernel, root) = fixture(&["-Users-tester-alpha"]).await?;
    let dir = root.join("-Users-tester-alpha");
    append(&dir.join("session-1.jsonl"), &[r#"{"type":"user","uuid":"old"}"#])?;

    capture_tick(&kernel).await?; // checkpoint

    // A brand-new session starts: new file, new activity.
    append(
        &dir.join("session-2.jsonl"),
        &[r#"{"type":"user","uuid":"fresh","sessionId":"s-2"}"#],
    )?;

    let report = capture_tick(&kernel).await?;
    assert_eq!(report.events_captured, 1, "new files are new activity");

    fs::remove_dir_all(&root).ok();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn partial_trailing_line_waits_for_completion() -> Result<(), Box<dyn Error>> {
    let (kernel, root) = fixture(&["-Users-tester-alpha"]).await?;
    let transcript = root.join("-Users-tester-alpha").join("session-1.jsonl");
    fs::write(&transcript, "")?;

    capture_tick(&kernel).await?; // checkpoint

    // A complete line plus an unterminated partial write.
    let chunk = concat!(r#"{"type":"user","uuid":"done"}"#, "\n", r#"{"type":"assis"#);
    let mut f = OpenOptions::new().append(true).open(&transcript)?;
    write!(f, "{chunk}")?;
    drop(f);

    let report = capture_tick(&kernel).await?;
    assert_eq!(report.events_captured, 1, "only the complete line is consumed");

    // The partial line completes; only it is captured next.
    let mut f = OpenOptions::new().append(true).open(&transcript)?;
    writeln!(f, r#"tant","uuid":"now-complete"}}"#)?;
    drop(f);

    let report = capture_tick(&kernel).await?;
    assert_eq!(report.events_captured, 1, "completed line captured exactly once");

    fs::remove_dir_all(&root).ok();
    Ok(())
}
