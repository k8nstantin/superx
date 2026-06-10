//! End-to-end tests for the lifecycle verbs (`mark_starting`,
//! `mark_active`, `mark_failed`, `mark_skipped`) and the hardened
//! lifecycle deserialization: corrupt substrate state surfaces as
//! `KernelError::Corrupt`, never as a silent `Enabled` default.

mod common;

use std::error::Error;

use surrealdb::types::{SurrealValue, Value};
use superx_kernel::{
    Kernel, KernelError, KernelModuleDescriptor, LifecycleState, NodeKind,
};

async fn register(kernel: &Kernel, name: &'static str) -> Result<surrealdb::types::RecordId, Box<dyn Error>> {
    let desc = KernelModuleDescriptor {
        name,
        version: "0.1.0",
        kind: NodeKind::KernelModule,
        category: "",
        depends_on: &[],
        required_metamodel: &[],
    };
    Ok(kernel.register_module(&desc).await?)
}

async fn lifecycle_of(kernel: &Kernel, name: &str) -> Result<LifecycleState, Box<dyn Error>> {
    Ok(kernel
        .detailed_status(NodeKind::KernelModule, name)
        .await?
        .expect("module must be registered")
        .lifecycle)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mark_starting_then_active_roundtrip() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    register(&kernel, "discovery").await?;

    kernel.mark_starting(NodeKind::KernelModule, "discovery").await?;
    assert_eq!(
        lifecycle_of(&kernel, "discovery").await?,
        LifecycleState::Starting,
    );

    kernel.mark_active(NodeKind::KernelModule, "discovery", 42).await?;
    assert_eq!(
        lifecycle_of(&kernel, "discovery").await?,
        LifecycleState::Active { startup_duration_ms: 42 },
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mark_failed_records_error_and_phase() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    register(&kernel, "capture").await?;

    kernel
        .mark_failed(NodeKind::KernelModule, "capture", "boom", "startup")
        .await?;
    assert_eq!(
        lifecycle_of(&kernel, "capture").await?,
        LifecycleState::Failed {
            error: "boom".to_string(),
            failed_during: "startup".to_string(),
        },
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mark_skipped_records_reason() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    register(&kernel, "emission").await?;

    kernel
        .mark_skipped(NodeKind::KernelModule, "emission", "dependency 'capture' failed")
        .await?;
    assert_eq!(
        lifecycle_of(&kernel, "emission").await?,
        LifecycleState::Skipped {
            reason: "dependency 'capture' failed".to_string(),
        },
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mark_verbs_return_not_found_for_unregistered() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let err = kernel
        .mark_starting(NodeKind::KernelModule, "ghost")
        .await
        .expect_err("unregistered module must be refused");
    assert!(
        matches!(err, KernelError::NotFound(_)),
        "expected NotFound, got: {err:?}",
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lifecycle_transitions_emit_telemetry_events() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    register(&kernel, "discovery").await?;

    kernel.mark_starting(NodeKind::KernelModule, "discovery").await?;
    kernel.mark_active(NodeKind::KernelModule, "discovery", 7).await?;
    kernel
        .mark_failed(NodeKind::KernelModule, "discovery", "boom", "runtime")
        .await?;
    kernel
        .mark_skipped(NodeKind::KernelModule, "discovery", "operator request")
        .await?;

    let events = kernel.recent_telemetry(50).await?;
    for expected in ["module_starting", "module_active", "module_failed", "module_skipped"] {
        assert!(
            events.iter().any(|e| e.lifecycle_event == expected),
            "missing telemetry event '{expected}'",
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lifecycle_history_is_append_only() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let entity_id = register(&kernel, "capture").await?;

    kernel.mark_starting(NodeKind::KernelModule, "capture").await?;
    kernel.mark_active(NodeKind::KernelModule, "capture", 3).await?;

    // register (Enabled) + starting + active = 3 lifecycle rows.
    let lifecycle_type = kernel.find_type("attr_lifecycle_state").await?;
    #[derive(SurrealValue)]
    struct CountRow {
        count: i64,
    }
    let counts: Vec<CountRow> = kernel
        .db()
        .query(
            "SELECT count() AS count FROM state_ledger \
             WHERE target = $target AND type = $type \
             GROUP ALL",
        )
        .bind(("target", entity_id))
        .bind(("type", lifecycle_type))
        .await?
        .take(0)?;
    assert_eq!(counts.first().map(|c| c.count), Some(3));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn corrupt_lifecycle_tag_surfaces_error_not_enabled() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let entity_id = register(&kernel, "discovery").await?;

    // Simulate a foreign / corrupt write through the public verb: a
    // lifecycle payload whose tag no kernel version ever produced.
    let mut payload = surrealdb::types::Object::new();
    payload.insert("tag".to_string(), Value::String("warp_drive".to_string()));
    payload.insert("error".to_string(), Value::None);
    payload.insert("failed_during".to_string(), Value::None);
    payload.insert("reason".to_string(), Value::None);
    payload.insert("startup_duration_ms".to_string(), Value::None);
    kernel
        .supersede_state(entity_id, "attr_lifecycle_state", Value::Object(payload))
        .await?;

    let err = kernel
        .detailed_status(NodeKind::KernelModule, "discovery")
        .await
        .expect_err("corrupt lifecycle state must surface as an error");
    assert!(
        matches!(err, KernelError::Corrupt(_)),
        "expected Corrupt, got: {err:?}",
    );
    Ok(())
}
