//! End-to-end tests for the Claude Code discovery probe, driven
//! through the real discovery framework against fixture directories.

use std::error::Error;
use std::fs;
use std::path::PathBuf;

use surrealdb::engine::any::connect;
use surrealdb::opt::auth::Database;

use superx_driver_claude_code::{
    ClaudeCodeDriver, AGENT_NAME, DRIVER_NAME, PROJECTS_ROOT_PARAM,
};
use superx_kernel::types::Value;
use superx_kernel::{Kernel, KernelModule, NodeKind, SCHEMA_DDL};
use superx_kernel_discovery::{run_probes, DiscoveryModule};

const TEST_PASSWORD: &str = "test-kernel-password-for-mem-engine";

/// Fresh substrate with kernel + discovery + driver metamodel seeded
/// and both modules registered (bootstrap's job in production).
async fn fresh_kernel() -> Result<Kernel, Box<dyn Error>> {
    let db = connect("mem://").await?;
    db.use_ns("superx").use_db("kernel").await?;
    let ddl = SCHEMA_DDL.replace("$SUPERX_KERNEL_PASSWORD", TEST_PASSWORD);
    db.query(ddl).await?.check()?;
    db.signin(Database {
        namespace: "superx".to_string(),
        database: "kernel".to_string(),
        username: "superx_kernel".to_string(),
        password: TEST_PASSWORD.to_string(),
    })
    .await?;
    let kernel = Kernel::from_db(db);
    for t in superx_kernel::REQUIRED_METAMODEL_TYPES {
        kernel.ensure_type_definition(t.uid, t.category, t.memory_tier).await?;
    }
    for module in [
        &DiscoveryModule as &dyn KernelModule,
        &ClaudeCodeDriver as &dyn KernelModule,
    ] {
        let desc = module.descriptor();
        for t in desc.required_metamodel {
            kernel.ensure_type_definition(t.uid, t.category, t.memory_tier).await?;
        }
        kernel.register_module(&desc).await?;
    }
    Ok(kernel)
}

/// Unique fixture directory under the system tempdir.
fn fixture_root() -> PathBuf {
    std::env::temp_dir()
        .join("superx-claude-code-probe")
        .join(uuid::Uuid::now_v7().to_string())
}

async fn set_projects_root(kernel: &Kernel, path: &std::path::Path) -> Result<(), Box<dyn Error>> {
    let entity_id = kernel
        .detailed_status(NodeKind::Contribution, DRIVER_NAME)
        .await?
        .expect("driver registered")
        .entity_id;
    kernel
        .set_parameter(
            entity_id,
            PROJECTS_ROOT_PARAM,
            Value::String(path.display().to_string()),
        )
        .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn probe_discovers_one_source_per_project_dir() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;

    let root = fixture_root();
    let project_a = root.join("-Users-tester-alpha");
    let project_b = root.join("-Users-tester-beta");
    fs::create_dir_all(&project_a)?;
    fs::create_dir_all(&project_b)?;
    fs::write(project_a.join("session-1.jsonl"), "{\"type\":\"user\"}\n")?;
    set_projects_root(&kernel, &root).await?;

    run_probes(&kernel).await?;

    // Agent registered once; one source per project directory.
    assert!(kernel
        .find_entity_by_name("node_agent", "attr_agent_descriptor", AGENT_NAME)
        .await?
        .is_some());
    for dir in [&project_a, &project_b] {
        assert!(
            kernel
                .find_entity_by_name(
                    "node_source",
                    "attr_source_descriptor",
                    &dir.display().to_string(),
                )
                .await?
                .is_some(),
            "missing source for {}",
            dir.display(),
        );
    }

    let events = kernel.recent_telemetry(50).await?;
    let count = |name: &str| events.iter().filter(|e| e.lifecycle_event == name).count();
    assert_eq!(count("agent_discovered"), 1);
    assert_eq!(count("source_discovered"), 2);

    fs::remove_dir_all(&root).ok();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn probe_is_empty_when_root_does_not_exist() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;
    set_projects_root(&kernel, &fixture_root().join("never-created")).await?;

    run_probes(&kernel).await?;

    assert!(kernel
        .find_entity_by_name("node_agent", "attr_agent_descriptor", AGENT_NAME)
        .await?
        .is_none());
    // The census still proves the scan ran.
    let events = kernel.recent_telemetry(20).await?;
    assert!(events.iter().any(|e| e.lifecycle_event == "discovery_census"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabled_driver_probes_nothing() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;

    let root = fixture_root();
    fs::create_dir_all(root.join("-Users-tester-alpha"))?;
    set_projects_root(&kernel, &root).await?;

    // Operator disables the driver.
    let entity_id = kernel
        .detailed_status(NodeKind::Contribution, DRIVER_NAME)
        .await?
        .expect("driver registered")
        .entity_id;
    let mut payload = superx_kernel::types::Object::new();
    payload.insert("value".to_string(), Value::String("disabled".to_string()));
    kernel
        .supersede_state(entity_id, "attr_module_status", Value::Object(payload))
        .await?;

    run_probes(&kernel).await?;

    assert!(
        kernel
            .find_entity_by_name("node_agent", "attr_agent_descriptor", AGENT_NAME)
            .await?
            .is_none(),
        "disabled driver must not discover",
    );
    fs::remove_dir_all(&root).ok();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rerun_is_idempotent() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;

    let root = fixture_root();
    fs::create_dir_all(root.join("-Users-tester-alpha"))?;
    set_projects_root(&kernel, &root).await?;

    run_probes(&kernel).await?;
    run_probes(&kernel).await?;

    let events = kernel.recent_telemetry(100).await?;
    let discovered = events
        .iter()
        .filter(|e| e.lifecycle_event == "agent_discovered")
        .count();
    assert_eq!(discovered, 1, "re-probe must not re-discover");
    fs::remove_dir_all(&root).ok();
    Ok(())
}
