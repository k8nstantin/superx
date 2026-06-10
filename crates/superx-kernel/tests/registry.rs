//! End-to-end tests for the kernel-module registry verbs
//! (`register_module`, `list_with_status`, `detailed_status`).
//!
//! These are the verbs `superx-kernel-bootstrap` (roadmap F6) drives;
//! they are verified here, before any consumer exists.

mod common;

use std::error::Error;

use surrealdb::types::SurrealValue;
use superx_kernel::{
    KernelModuleDescriptor, LifecycleState, NodeKind,
};

fn descriptor(
    name: &'static str,
    version: &'static str,
    kind: NodeKind,
    category: &'static str,
) -> KernelModuleDescriptor {
    KernelModuleDescriptor {
        name,
        version,
        kind,
        category,
        depends_on: &[],
        required_metamodel: &[],
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn register_module_creates_entity_with_initial_lifecycle() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let desc = descriptor("discovery", "0.1.0", NodeKind::KernelModule, "");

    let entity_id = kernel.register_module(&desc).await?;
    assert_eq!(entity_id.table, "entity".into());

    let status = kernel
        .detailed_status(NodeKind::KernelModule, "discovery")
        .await?
        .expect("registered module must have a status");
    assert_eq!(status.name, "discovery");
    assert_eq!(status.version, "0.1.0");
    assert_eq!(status.kind, NodeKind::KernelModule);
    assert_eq!(status.category, "");
    assert_eq!(status.entity_id, entity_id);
    assert_eq!(status.lifecycle, LifecycleState::Enabled);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn register_module_is_idempotent() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let desc = descriptor("capture", "0.1.0", NodeKind::KernelModule, "");

    let first = kernel.register_module(&desc).await?;
    let second = kernel.register_module(&desc).await?;
    assert_eq!(first, second, "re-registration must return the same entity");

    let listed = kernel.list_with_status(NodeKind::KernelModule).await?;
    assert_eq!(listed.len(), 1, "no duplicate registry entries");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn register_module_supersedes_descriptor() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;

    let v1 = descriptor("emission", "0.1.0", NodeKind::KernelModule, "");
    let entity_id = kernel.register_module(&v1).await?;

    let v2 = descriptor("emission", "0.2.0", NodeKind::KernelModule, "");
    let same_id = kernel.register_module(&v2).await?;
    assert_eq!(entity_id, same_id);

    let status = kernel
        .detailed_status(NodeKind::KernelModule, "emission")
        .await?
        .expect("registered module must have a status");
    assert_eq!(status.version, "0.2.0", "latest descriptor must win");

    // Append-only proof: two descriptor rows exist for this entity.
    let desc_type = kernel.find_type("attr_module_descriptor").await?;
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
        .bind(("type", desc_type))
        .await?
        .take(0)?;
    assert_eq!(counts.first().map(|c| c.count), Some(2));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_with_status_filters_by_kind() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;

    kernel
        .register_module(&descriptor("parameters", "0.1.0", NodeKind::KernelModule, ""))
        .await?;
    kernel
        .register_module(&descriptor(
            "driver_claude_code",
            "0.1.0",
            NodeKind::Contribution,
            "driver",
        ))
        .await?;

    let kernel_modules = kernel.list_with_status(NodeKind::KernelModule).await?;
    assert_eq!(kernel_modules.len(), 1);
    assert_eq!(kernel_modules[0].name, "parameters");

    let contributions = kernel.list_with_status(NodeKind::Contribution).await?;
    assert_eq!(contributions.len(), 1);
    assert_eq!(contributions[0].name, "driver_claude_code");
    assert_eq!(contributions[0].category, "driver");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn detailed_status_returns_none_for_unregistered() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let status = kernel
        .detailed_status(NodeKind::KernelModule, "never_registered")
        .await?;
    assert!(status.is_none());
    Ok(())
}
