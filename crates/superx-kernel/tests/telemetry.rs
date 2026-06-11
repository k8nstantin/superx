//! End-to-end tests for the telemetry verbs (`log_telemetry`,
//! `recent_telemetry`, `telemetry_since`) — boundary conditions the
//! `stats --live` tail depends on, plus the table's only typed FK
//! (`run.type.uid = 'node_run'`), exercised nowhere else.

mod common;

use std::error::Error;

use surrealdb::types::{Object, Value};
use superx_kernel::Kernel;

async fn emit(kernel: &Kernel, event: &str) -> Result<(), Box<dyn Error>> {
    let mut payload = Object::new();
    payload.insert("k".to_string(), Value::String(event.to_string()));
    kernel.log_telemetry(event, Value::Object(payload), None).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recent_telemetry_is_newest_first_and_limited() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_kernel().await?;
    for name in ["first", "second", "third"] {
        emit(&kernel, name).await?;
    }

    let two = kernel.recent_telemetry(2).await?;
    assert_eq!(two.len(), 2, "limit respected");
    assert_eq!(two[0].lifecycle_event, "third", "newest first");
    assert_eq!(two[1].lifecycle_event, "second");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn telemetry_since_boundary_is_exclusive_and_ascending() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_kernel().await?;
    emit(&kernel, "before").await?;

    // The boundary event: rows at EXACTLY `after` must be excluded.
    let all = kernel.recent_telemetry(10).await?;
    let boundary = all
        .iter()
        .find(|e| e.lifecycle_event == "before")
        .expect("boundary event present");
    let after = boundary.valid_from;

    emit(&kernel, "later_a").await?;
    emit(&kernel, "later_b").await?;

    let since = kernel.telemetry_since(after, 10).await?;
    assert!(
        since.iter().all(|e| e.lifecycle_event != "before"),
        "strictly-greater boundary: the `after` row itself is excluded",
    );
    let names: Vec<&str> = since.iter().map(|e| e.lifecycle_event.as_str()).collect();
    let pos = |n: &str| names.iter().position(|x| *x == n);
    assert!(
        pos("later_a") < pos("later_b"),
        "ascending order (oldest first): {names:?}",
    );

    // Limit bound.
    let one = kernel.telemetry_since(after, 1).await?;
    assert_eq!(one.len(), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn log_telemetry_with_run_fk_is_engine_checked() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;

    // A real node_run target satisfies the run FK ASSERT.
    let run = kernel.create_entity("node_run", "user").await?;
    let mut payload = Object::new();
    payload.insert("k".to_string(), Value::String("run-scoped".to_string()));
    kernel
        .log_telemetry("run_scoped_event", Value::Object(payload), Some(run.clone()))
        .await?;
    let events = kernel.recent_telemetry(5).await?;
    let row = events
        .iter()
        .find(|e| e.lifecycle_event == "run_scoped_event")
        .expect("run-scoped event present");
    assert_eq!(row.run.as_ref(), Some(&run));

    // A non-run entity must be refused by the engine ASSERT
    // (`run.type.uid = 'node_run'`) — the §12 debugging surface.
    let agent = kernel.create_entity("node_agent", "user").await?;
    let mut payload = Object::new();
    payload.insert("k".to_string(), Value::String("bad".to_string()));
    let err = kernel
        .log_telemetry("bad_run_event", Value::Object(payload), Some(agent))
        .await
        .expect_err("engine must refuse a non-node_run run FK");
    let msg = err.to_string();
    assert!(
        msg.contains("run") || msg.contains("node_run") || msg.contains("ASSERT"),
        "expected run-FK ASSERT refusal, got: {msg}",
    );
    Ok(())
}
