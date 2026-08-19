//! End-to-end tests for the module registry verbs.

mod common;

use std::error::Error;

use superx_kernel::{KernelModuleDescriptor, ModuleStatus, NodeKind};

fn descriptor(name: &'static str, version: &'static str) -> KernelModuleDescriptor {
    KernelModuleDescriptor {
        name,
        version,
        kind: NodeKind::KernelModule,
        depends_on: &[],
        required_metamodel: &[],
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn register_is_idempotent_by_name() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let first = kernel.register_module(&descriptor("mod_a", "0.1.0")).await?;
    let second = kernel.register_module(&descriptor("mod_a", "0.1.0")).await?;
    assert_eq!(first, second, "re-registration returns the same entity");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn descriptor_supersession_updates_version() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    kernel.register_module(&descriptor("mod_b", "0.1.0")).await?;
    kernel.register_module(&descriptor("mod_b", "0.2.0")).await?;

    let status = kernel
        .detailed_status(NodeKind::KernelModule, "mod_b")
        .await?
        .expect("registered module has a status");
    assert_eq!(status.version, "0.2.0", "latest descriptor wins");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_with_status_sees_all_registered() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    kernel.register_module(&descriptor("mod_c", "0.1.0")).await?;
    kernel.register_module(&descriptor("mod_d", "0.1.0")).await?;

    let list = kernel.list_with_status(NodeKind::KernelModule).await?;
    let names: Vec<&str> = list.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"mod_c") && names.contains(&"mod_d"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn detailed_status_none_for_unregistered() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    assert!(kernel
        .detailed_status(NodeKind::KernelModule, "ghost")
        .await?
        .is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn adapters_and_modules_are_separate_namespaces() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    kernel.register_module(&descriptor("same_name", "0.1.0")).await?;
    let adapter = KernelModuleDescriptor {
        kind: NodeKind::Adapter,
        ..descriptor("same_name", "0.1.0")
    };
    let adapter_id = kernel.register_module(&adapter).await?;
    let module_id = kernel
        .detailed_status(NodeKind::KernelModule, "same_name")
        .await?
        .expect("module exists")
        .entity_id;
    assert_ne!(adapter_id, module_id, "kinds have distinct entities");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_module_status_roundtrip_and_not_found() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    kernel.register_module(&descriptor("togglable", "0.1.0")).await?;

    kernel
        .set_module_status(NodeKind::KernelModule, "togglable", ModuleStatus::Disabled)
        .await?;
    assert_eq!(
        kernel.module_status(NodeKind::KernelModule, "togglable").await?,
        Some(ModuleStatus::Disabled)
    );
    kernel
        .set_module_status(NodeKind::KernelModule, "togglable", ModuleStatus::Enabled)
        .await?;
    assert_eq!(
        kernel.module_status(NodeKind::KernelModule, "togglable").await?,
        Some(ModuleStatus::Enabled)
    );

    let err = kernel
        .set_module_status(NodeKind::KernelModule, "ghost", ModuleStatus::Disabled)
        .await
        .expect_err("unregistered module errors");
    assert!(err.to_string().contains("not found"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn module_ledger_tracks_registrations_and_provisioning() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_seeded_kernel().await?;
    let desc = descriptor("ledgered", "0.1.0");

    // First registration → one ledger row, unprovisioned.
    let entity = kernel.register_module(&desc).await?;
    let first = kernel
        .latest_module_record("ledgered")
        .await?
        .expect("ledger row after registration");
    assert_eq!(first.uid, "ledgered");
    assert_eq!(first.entity, entity);
    assert_eq!(first.kind, "kernel_module");
    assert!(!first.provisioned);

    // Provisioning fact appends a provisioned=true row.
    kernel
        .append_module_record(&desc, entity.clone(), true)
        .await?;
    assert!(kernel
        .latest_module_record("ledgered")
        .await?
        .expect("row")
        .provisioned);

    // Re-registration (a reboot) carries the provisioned fact forward
    // and preserves history append-only.
    let v2 = descriptor("ledgered", "0.2.0");
    kernel.register_module(&v2).await?;
    let latest = kernel.latest_module_record("ledgered").await?.expect("row");
    assert!(latest.provisioned, "registration never un-provisions");
    assert_eq!(latest.version, "0.2.0", "version tracks the binary");
    let history = kernel.module_history("ledgered", 10).await?;
    assert_eq!(history.len(), 3, "every change is a row: register, provision, re-register");
    Ok(())
}
