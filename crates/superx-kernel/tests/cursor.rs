//! End-to-end tests for the cursor verbs (`ensure_cursor_type`,
//! `write_cursor`, `latest_cursor`) — durable progress checkpoints
//! the capture loop (roadmap F10) will drive.

mod common;

use std::error::Error;

use surrealdb::types::{Object, SurrealValue, Value};
use superx_kernel::Kernel;

async fn source_entity(kernel: &Kernel) -> Result<surrealdb::types::RecordId, Box<dyn Error>> {
    Ok(kernel.create_entity("node_source", "admin").await?)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ensure_cursor_type_is_idempotent() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let first = kernel
        .ensure_cursor_type("transcript_watch", "telemetry", "JSONL transcript read position")
        .await?;
    let second = kernel
        .ensure_cursor_type("transcript_watch", "telemetry", "JSONL transcript read position")
        .await?;
    assert_eq!(first, second, "repeat call must return existing id");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ensure_cursor_type_refuses_invalid_category() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let err = kernel
        .ensure_cursor_type("bad", "quantum", "nope")
        .await
        .expect_err("engine must refuse category outside ['workload', 'telemetry']");
    let msg = err.to_string();
    assert!(
        msg.contains("workload") || msg.contains("telemetry") || msg.contains("category"),
        "expected ASSERT failure mentioning the category domain, got: {msg}",
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn write_then_latest_roundtrips_checkpoint() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let subject = source_entity(&kernel).await?;
    kernel
        .ensure_cursor_type("transcript_watch", "telemetry", "JSONL read position")
        .await?;

    let mut meta = Object::new();
    meta.insert("file".to_string(), Value::String("session-1.jsonl".to_string()));
    let row_id = kernel
        .write_cursor(
            subject.clone(),
            "transcript_watch",
            Some("line:42".to_string()),
            Some(meta),
        )
        .await?;
    assert_eq!(row_id.table, "cursor".into());

    let cursor = kernel
        .latest_cursor(subject.clone(), "transcript_watch")
        .await?
        .expect("checkpoint must be present");
    assert_eq!(cursor.id, row_id);
    assert_eq!(cursor.subject, subject);
    assert_eq!(cursor.last_processed, Some("line:42".to_string()));
    let meta = cursor.metadata.expect("metadata present");
    assert_eq!(
        meta.get("file"),
        Some(&Value::String("session-1.jsonl".to_string())),
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn latest_checkpoint_wins_and_history_is_append_only() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let subject = source_entity(&kernel).await?;
    kernel
        .ensure_cursor_type("transcript_watch", "telemetry", "JSONL read position")
        .await?;

    kernel
        .write_cursor(subject.clone(), "transcript_watch", Some("line:10".to_string()), None)
        .await?;
    kernel
        .write_cursor(subject.clone(), "transcript_watch", Some("line:99".to_string()), None)
        .await?;

    let cursor = kernel
        .latest_cursor(subject.clone(), "transcript_watch")
        .await?
        .expect("checkpoint must be present");
    assert_eq!(cursor.last_processed, Some("line:99".to_string()));

    // Append-only: both checkpoints exist.
    #[derive(SurrealValue)]
    struct CountRow {
        count: i64,
    }
    let counts: Vec<CountRow> = kernel
        .db()
        .query(
            "SELECT count() AS count FROM cursor \
             WHERE subject = $subject \
             GROUP ALL",
        )
        .bind(("subject", subject))
        .await?
        .take(0)?;
    assert_eq!(counts.first().map(|c| c.count), Some(2));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cursors_are_isolated_per_subject_and_type() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let subject_a = source_entity(&kernel).await?;
    let subject_b = source_entity(&kernel).await?;
    kernel
        .ensure_cursor_type("transcript_watch", "telemetry", "JSONL read position")
        .await?;
    kernel
        .ensure_cursor_type("run_resume", "workload", "workflow resume point")
        .await?;

    kernel
        .write_cursor(subject_a.clone(), "transcript_watch", Some("line:7".to_string()), None)
        .await?;

    // Other subject, same type → no checkpoint.
    assert!(kernel
        .latest_cursor(subject_b, "transcript_watch")
        .await?
        .is_none());
    // Same subject, other (seeded) type → no checkpoint.
    assert!(kernel
        .latest_cursor(subject_a, "run_resume")
        .await?
        .is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn latest_cursor_returns_none_for_unseeded_type() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let subject = source_entity(&kernel).await?;
    assert!(kernel
        .latest_cursor(subject, "never_seeded")
        .await?
        .is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn write_cursor_refuses_unseeded_type() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let subject = source_entity(&kernel).await?;
    let err = kernel
        .write_cursor(subject, "never_seeded", None, None)
        .await
        .expect_err("writing against an unseeded cursor_type must be refused");
    assert!(err.to_string().contains("not found"));
    Ok(())
}
