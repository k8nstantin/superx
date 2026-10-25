//! Claude Desktop adapter tests: every record kept whole (#522), multiline
//! folding, the native host's bracketed records, a log rewritten in place,
//! a rotated log read to its end. Telemetry only — the app stores no
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
async fn every_record_becomes_an_event() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let tmp = tempfile::tempdir()?;
    arrange(&kernel, tmp.path()).await?;

    let sources = desktop_sources(&kernel).await?;
    assert_eq!(sources.len(), 1, "main.log only (no rotated/duplicate files)");

    let report = capture_tick(&kernel, &sources, None).await?;
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.total(), 6, "every record, the health checks and feature refreshes too; the error folded");

    let recent = kernel.recent_telemetry(20).await?;
    let events: Vec<&superx_kernel::TelemetryRecord> = recent
        .iter()
        .filter(|e| e.lifecycle_event == "desktop_event")
        .collect();
    assert_eq!(events.len(), 6);
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
async fn a_log_rewritten_in_place_is_read_from_its_start() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let tmp = tempfile::tempdir()?;
    arrange(&kernel, tmp.path()).await?;

    let sources = desktop_sources(&kernel).await?;
    assert_eq!(capture_tick(&kernel, &sources, None).await?.total(), 6);
    assert_eq!(capture_tick(&kernel, &sources, None).await?.total(), 0, "cursor holds");

    // The same file, rewritten shorter in place.
    fs::write(
        tmp.path().join("main.log"),
        "2026-08-07 11:00:00 [info] Starting app v1.0.2340\n",
    )?;
    assert_eq!(
        capture_tick(&kernel, &sources, None).await?.total(),
        1,
        "shrunken file re-captured from zero"
    );

    // Appends keep flowing after the reset.
    let mut f = fs::OpenOptions::new()
        .append(true)
        .open(tmp.path().join("main.log"))?;
    writeln!(f, "2026-08-07 11:00:01 [info] beforeQuit handler fired, going down")?;
    assert_eq!(capture_tick(&kernel, &sources, None).await?.total(), 1);
    Ok(())
}

/// A rotated log is read to its end before the new one starts (#522):
/// electron-log renames `main.log` to `main1.log`, and the lines written
/// since the last read are in the renamed file.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rotated_log_is_read_to_its_end_before_the_new_one() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let tmp = tempfile::tempdir()?;
    arrange(&kernel, tmp.path()).await?;
    let sources = desktop_sources(&kernel).await?;
    assert_eq!(capture_tick(&kernel, &sources, None).await?.total(), 6);

    let log = tmp.path().join("main.log");
    let mut f = fs::OpenOptions::new().append(true).open(&log)?;
    writeln!(f, "2026-08-07 10:00:06 [info] written before the rotation")?;
    writeln!(f, "2026-08-07 10:00:07 [info] the last line of the old log")?;
    drop(f);
    fs::rename(&log, tmp.path().join("main1.log"))?;
    fs::write(&log, "2026-08-07 10:00:08 [info] the first line of the new log\n")?;

    assert_eq!(capture_tick(&kernel, &sources, None).await?.total(), 3, "the old log's 2 last lines, then the new log's 1");
    let recent = kernel.recent_telemetry(50).await?;
    let lines: Vec<String> = recent
        .iter()
        .filter(|e| e.lifecycle_event == "desktop_event")
        .map(|e| format!("{:?}", e.payload))
        .collect();
    for wanted in ["written before the rotation", "the last line of the old log", "the first line of the new log"] {
        assert!(lines.iter().any(|l| l.contains(wanted)), "{wanted}: {lines:?}");
    }
    assert_eq!(capture_tick(&kernel, &sources, None).await?.total(), 0, "nothing read twice");
    Ok(())
}

/// The Chrome native host writes bracketed dates; each of its lines is a
/// record of its own, kept whole (#522).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_native_hosts_bracketed_lines_are_each_a_record() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let tmp = tempfile::tempdir()?;
    arrange(&kernel, tmp.path()).await?;
    fs::write(
        tmp.path().join("chrome-native-host.log"),
        concat!(
            "[2026-08-24 12:19:35 INFO chrome-native-host] Chrome native host starting (version 0.1.0)\n",
            "[2026-08-24 12:19:35 INFO chrome-native-host] Socket created successfully\n",
            "[2026-10-20 09:44:18 INFO chrome-native-host] Entering main message loop\n",
        ),
    )?;
    let sources = desktop_sources(&kernel).await?;
    assert_eq!(sources.len(), 2, "main.log and chrome-native-host.log");
    let report = capture_tick(&kernel, &sources, None).await?;
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.total(), 9, "main.log's 6 records and the native host's 3");
    let recent = kernel.recent_telemetry(50).await?;
    let host: Vec<String> = recent
        .iter()
        .filter(|e| e.lifecycle_event == "desktop_event" && format!("{:?}", e.payload).contains("chrome-native-host]"))
        .map(|e| format!("{:?}", e.payload))
        .collect();
    assert_eq!(host.len(), 3, "one event per line: {host:?}");
    assert!(host.iter().all(|l| l.matches("chrome-native-host]").count() == 1), "never folded together: {host:?}");
    Ok(())
}
