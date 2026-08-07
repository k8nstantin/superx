//! End-to-end tests for the lifecycle verbs: every `mark_*` writes a
//! state row AND emits a subject-attributed telemetry event; corrupt
//! rows surface as errors, never defaults.

mod common;

use std::error::Error;

use surrealdb::types::{Object, Value};
use superx_kernel::{KernelModuleDescriptor, LifecycleState, NodeKind};

fn descriptor(name: &'static str) -> KernelModuleDescriptor {
    KernelModuleDescriptor {
        name,
        version: "0.1.0",
        kind: NodeKind::KernelModule,
        depends_on: &[],
        required_metamodel: &[],
    }
}

async fn lifecycle_of(
    kernel: &superx_kernel::Kernel,
    name: &str,
) -> Result<LifecycleState, Box<dyn Error>> {
    Ok(kernel
        .detailed_status(NodeKind::KernelModule, name)
        .await?
        .expect("registered")
        .lifecycle)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mark_transitions_roundtrip() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    kernel.register_module(&descriptor("lc_mod")).await?;

    kernel.mark_starting(NodeKind::KernelModule, "lc_mod").await?;
    assert_eq!(lifecycle_of(&kernel, "lc_mod").await?, LifecycleState::Starting);

    kernel
        .mark_active(NodeKind::KernelModule, "lc_mod", 42)
        .await?;
    assert_eq!(
        lifecycle_of(&kernel, "lc_mod").await?,
        LifecycleState::Active {
            startup_duration_ms: 42
        }
    );

    kernel
        .mark_failed(NodeKind::KernelModule, "lc_mod", "boom", "startup")
        .await?;
    assert_eq!(
        lifecycle_of(&kernel, "lc_mod").await?,
        LifecycleState::Failed {
            error: "boom".into(),
            failed_during: "startup".into()
        }
    );

    kernel
        .mark_skipped(NodeKind::KernelModule, "lc_mod", "dep failed")
        .await?;
    assert_eq!(
        lifecycle_of(&kernel, "lc_mod").await?,
        LifecycleState::Skipped {
            reason: "dep failed".into()
        }
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transitions_emit_subject_attributed_telemetry() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let entity_id = kernel.register_module(&descriptor("lc_tel")).await?;

    kernel.mark_starting(NodeKind::KernelModule, "lc_tel").await?;
    kernel.mark_active(NodeKind::KernelModule, "lc_tel", 7).await?;

    let recent = kernel.recent_telemetry(10).await?;
    let active = recent
        .iter()
        .find(|e| e.lifecycle_event == "module_active")
        .expect("module_active event emitted");
    assert_eq!(
        active.subject.as_ref(),
        Some(&entity_id),
        "lifecycle events are attributed to the module entity"
    );
    assert!(recent
        .iter()
        .any(|e| e.lifecycle_event == "module_starting"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unregistered_module_is_not_found() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let err = kernel
        .mark_starting(NodeKind::KernelModule, "ghost")
        .await
        .expect_err("unregistered module must error");
    assert!(err.to_string().contains("not found"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn corrupt_lifecycle_tag_errors_never_defaults() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let entity_id = kernel.register_module(&descriptor("lc_bad")).await?;

    // A foreign write with an unknown tag.
    let mut payload = Object::new();
    payload.insert("tag".to_string(), Value::String("transcended".to_string()));
    payload.insert("error".to_string(), Value::None);
    payload.insert("failed_during".to_string(), Value::None);
    payload.insert("reason".to_string(), Value::None);
    payload.insert("startup_duration_ms".to_string(), Value::None);
    kernel
        .supersede_state(entity_id, "attr_lifecycle_state", Value::Object(payload))
        .await?;

    let err = kernel
        .detailed_status(NodeKind::KernelModule, "lc_bad")
        .await
        .expect_err("corrupt lifecycle must surface, never default");
    assert!(err.to_string().contains("corrupt"));
    Ok(())
}
