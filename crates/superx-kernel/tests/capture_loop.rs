//! The capture loop (#368, #570): a pass that fails is tried again at
//! the next tick. On 2026-11-04 an answer that dropped the substrate's
//! socket failed capture's request in flight, and capture ended there
//! without a word; 44 minutes were captured only by a restart.

mod common;

use std::error::Error;
use std::fs;
use std::time::Duration;

use superx_kernel::adapters::claude_code::{ClaudeCodeAdapter, PROJECTS_ROOT_PARAM};
use superx_kernel::capture::run_loop;
use superx_kernel::registry::KernelModule as _;
use superx_kernel::types::Value;

const BEFORE: &str = concat!(
    r#"{"type":"user","message":{"role":"user","content":"before the outage"},"origin":{"kind":"human"},"#,
    r#""sessionId":"sess-before","timestamp":"2026-11-04T12:00:00.000Z","cwd":"/tmp/proj"}"#,
    "\n",
);

const LINE: &str = concat!(
    r#"{"type":"user","message":{"role":"user","content":"after the outage"},"origin":{"kind":"human"},"#,
    r#""sessionId":"sess-after","timestamp":"2026-11-04T13:00:00.000Z","cwd":"/tmp/proj"}"#,
    "\n",
);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_pass_is_tried_again_at_the_next_tick_and_capture_goes_on() -> Result<(), Box<dyn Error>> {
    // One kernel, shared, never cloned: a clone of the client opens a
    // session of its own, and the outage below must reach the loop's.
    let kernel = std::sync::Arc::new(common::fresh_seeded_kernel().await?);
    let tmp = tempfile::tempdir()?;
    common::isolate_adapters(&kernel, &tmp.path().join("isolated-empty")).await?;
    let projects = tmp.path().join("projects");
    let project = projects.join("-tmp-proj");
    fs::create_dir_all(&project)?;
    let adapter = kernel.register_module(&ClaudeCodeAdapter.descriptor()).await?;
    kernel
        .set_parameter(adapter, PROJECTS_ROOT_PARAM, Value::String(projects.to_string_lossy().to_string()))
        .await?;

    let (stop, stopped) = tokio::sync::watch::channel(false);
    let looping = tokio::spawn({
        let kernel = std::sync::Arc::clone(&kernel);
        async move { run_loop(&kernel, stopped).await }
    });

    // Capture runs: a line written now is captured.
    fs::write(project.join("sess-before.jsonl"), BEFORE)?;
    assert!(captured(&kernel, "claude_code/sess-before").await?, "capture runs before the outage");

    // Every query of a pass now fails: the loop's session points at a
    // database that does not exist, as a socket that dropped mid-pass
    // fails every request in flight on it.
    kernel.db().use_ns("superx").use_db("nowhere").await?;
    let failing = kernel.db().query("SELECT * FROM entity LIMIT 1").await.and_then(|r| r.check()).map(|_| ());
    assert!(failing.is_err(), "the outage is real: a query fails ({failing:?})");
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(!looping.is_finished(), "a failed pass does not end capture");

    // The substrate answers again; a line written now is captured.
    kernel.db().use_ns("superx").use_db("kernel").await?;
    common::sign_in(&kernel).await?;
    fs::write(project.join("sess-after.jsonl"), LINE)?;
    assert!(
        captured(&kernel, "claude_code/sess-after").await?,
        "capture went on: the line written after the outage was captured"
    );

    stop.send(true)?;
    looping.await??;
    Ok(())
}

/// Whether capture made the session `name` within twenty seconds, the
/// loop ticking every two.
async fn captured(kernel: &superx_kernel::Kernel, name: &str) -> Result<bool, Box<dyn Error>> {
    for _ in 0..40 {
        if kernel.find_entity_by_name("node_session", "attr_session_descriptor", name).await?.is_some() {
            return Ok(true);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Ok(false)
}
