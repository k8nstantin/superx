//! End-to-end tests for the telemetry verbs (`log_telemetry`,
//! `recent_telemetry`, `telemetry_since`) — the boundary conditions
//! the future live tail depends on, plus `subject` attribution.

mod common;

use std::error::Error;

use surrealdb::types::{Object, Value};
use superx_kernel::Kernel;

async fn emit(kernel: &Kernel, event: &str) -> Result<(), Box<dyn Error>> {
    let mut payload = Object::new();
    payload.insert("k".to_string(), Value::String(event.to_string()));
    kernel
        .log_telemetry(event, Value::Object(payload), None)
        .await?;
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
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subject_attribution_roundtrips_for_any_entity_type() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_kernel().await?;
    // Any node type may be a telemetry subject — the schema
    // deliberately does not ASSERT a specific type (SUPERX_SCHEMA.md
    // §3.7).
    kernel
        .ensure_type_definition("node_source", "node", "core")
        .await?;
    let source = kernel.create_entity("node_source").await?;

    let mut payload = Object::new();
    payload.insert("line".to_string(), Value::String("hello".to_string()));
    kernel
        .log_telemetry(
            "transcript_event",
            Value::Object(payload),
            Some(source.clone()),
        )
        .await?;

    let latest = kernel.recent_telemetry(1).await?;
    assert_eq!(latest[0].lifecycle_event, "transcript_event");
    assert_eq!(
        latest[0].subject.as_ref(),
        Some(&source),
        "subject FK survives the roundtrip typed",
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subject_refuses_non_entity_record() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_kernel().await?;
    kernel
        .ensure_type_definition("node_y", "node", "core")
        .await?;
    // A type_definition id is not an entity id — the record<entity>
    // field type must refuse it.
    let not_an_entity = kernel.find_type("node_y").await?;
    let err = kernel
        .log_telemetry("bad_subject", Value::None, Some(not_an_entity))
        .await
        .expect_err("engine must refuse a non-entity subject record");
    let msg = err.to_string();
    assert!(
        msg.contains("entity") || msg.contains("subject"),
        "expected field-type refusal mentioning entity/subject, got: {msg}",
    );
    Ok(())
}
