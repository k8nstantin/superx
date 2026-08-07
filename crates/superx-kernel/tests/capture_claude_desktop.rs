//! Claude Desktop adapter tests: signal-vs-noise filtering, multiline
//! folding, rotation tolerance. Telemetry only — the app stores no
//! conversations locally.

mod common;

use std::error::Error;
use std::fs;
use std::io::Write as _;

use superx_kernel::adapters::claude_desktop::{ClaudeDesktopAdapter, LOGS_ROOT_PARAM};
use superx_kernel::capture::{capture_tick, discover_paired};
use superx_kernel::registry::KernelModule as _;
use superx_kernel::types::Value;
use superx_kernel::Kernel;

const LOG_BODY: &str = "\
2026-08-07 10:00:00 [info] Health check fetch failed\n\
2026-08-07 10:00:01 [info] features refreshed: 213 features loaded\n\
2026-08-07 10:00:02 [info] Starting app v1.0.2339\n\
2026-08-07 10:00:03 [info] [Claude in Chrome] MCP server registered\n\
2026-08-07 10:00:04 [error] update failed with Error\n\
  code: ENOENT\n\
  domain: autoUpdater\n\
2026-08-07 10:00:05 [info] Health check fetch failed\n";

async fn arrange(kernel: &Kernel, root: &std::path::Path) -> Result<(), Box<dyn Error>> {
    common::isolate_adapters(kernel, &root.join("isolated-empty")).await?;
    fs::create_dir_all(root)?;
    fs::write(root.join("main.log"), LOG_BODY)?;
    // Rotated + duplicate files that must NOT be discovered.
    fs::write(root.join("main1.log"), "old rotated content\n")?;
    fs::write(root.join("unknown-window.log"), "duplicate renderer log\n")?;

    let entity = kernel
        .register_module(&ClaudeDesktopAdapter.descriptor())
        .await?;
    kernel
        .set_parameter(
            entity,
            LOGS_ROOT_PARAM,
            Value::String(root.to_string_lossy().to_string()),
        )
        .await?;
    Ok(())
}

async fn desktop_sources(
    kernel: &Kernel,
) -> Result<Vec<(usize, superx_kernel::SourceRef)>, Box<dyn Error>> {
    let all = discover_paired(kernel).await?;
    Ok(all
        .into_iter()
        .filter(|(_, s)| s.name.starts_with("claude_desktop/"))
        .collect())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn signal_lines_become_events_noise_skipped() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let tmp = tempfile::tempdir()?;
    arrange(&kernel, tmp.path()).await?;

    let sources = desktop_sources(&kernel).await?;
    assert_eq!(sources.len(), 1, "main.log only (no rotated/duplicate files)");

    let report = capture_tick(&kernel, &sources).await?;
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(
        report.total(),
        3,
        "Starting app + MCP + folded error; health checks and feature refreshes skipped"
    );

    let recent = kernel.recent_telemetry(20).await?;
    let events: Vec<&superx_kernel::TelemetryRecord> = recent
        .iter()
        .filter(|e| e.lifecycle_event == "desktop_event")
        .collect();
    assert_eq!(events.len(), 3);
    // The multiline error record was folded into one event.
    let folded = events
        .iter()
        .find(|e| format!("{:?}", e.payload).contains("ENOENT"))
        .expect("folded error event");
    assert!(
        format!("{:?}", folded.payload).contains("autoUpdater"),
        "continuation lines folded into the parent record"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rotation_resets_offset_and_resumes() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let tmp = tempfile::tempdir()?;
    arrange(&kernel, tmp.path()).await?;

    let sources = desktop_sources(&kernel).await?;
    assert_eq!(capture_tick(&kernel, &sources).await?.total(), 3);
    assert_eq!(capture_tick(&kernel, &sources).await?.total(), 0, "cursor holds");

    // Simulate rotation: a fresh, shorter active file.
    fs::write(
        tmp.path().join("main.log"),
        "2026-08-07 11:00:00 [info] Starting app v1.0.2340\n",
    )?;
    assert_eq!(
        capture_tick(&kernel, &sources).await?.total(),
        1,
        "shrunken file re-captured from zero"
    );

    // Appends keep flowing after the reset.
    let mut f = fs::OpenOptions::new()
        .append(true)
        .open(tmp.path().join("main.log"))?;
    writeln!(f, "2026-08-07 11:00:01 [info] beforeQuit handler fired, going down")?;
    assert_eq!(capture_tick(&kernel, &sources).await?.total(), 1);
    Ok(())
}
