//! End-to-end tests for the discovery framework: fake probes register
//! via linkme in this test binary; `run_probes` turns their findings
//! into substrate entities + telemetry with per-probe failure
//! isolation.

use std::error::Error;

use linkme::distributed_slice;
use surrealdb::engine::any::connect;
use surrealdb::opt::auth::Database;
use surrealdb::types::SurrealValue;

use superx_kernel::{
    Kernel, KernelError, KernelModule, Result as KernelResult, SCHEMA_DDL,
};
use superx_kernel_discovery::{
    run_probes, DiscoveredAgent, DiscoveredSource, DiscoveryModule, DiscoveryProbe,
    DISCOVERY_PROBES,
};

const TEST_PASSWORD: &str = "test-kernel-password-for-mem-engine";

/// Fresh mem:// substrate with the kernel + discovery metamodel
/// seeded (bootstrap's job in production; the fixture's here).
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
        kernel
            .ensure_type_definition(t.uid, t.category, t.memory_tier)
            .await?;
    }
    for t in DiscoveryModule.descriptor().required_metamodel {
        kernel
            .ensure_type_definition(t.uid, t.category, t.memory_tier)
            .await?;
    }
    Ok(kernel)
}

// ─────────────────────────────────────────────────────────────────────
// Fake probes
// ─────────────────────────────────────────────────────────────────────

struct OkProbe;

#[async_trait::async_trait]
impl DiscoveryProbe for OkProbe {
    fn name(&self) -> &'static str {
        "test_ok_probe"
    }

    async fn probe(&self, _kernel: &Kernel) -> KernelResult<Vec<DiscoveredAgent>> {
        Ok(vec![DiscoveredAgent {
            name: "claude_test".to_string(),
            sources: vec![
                DiscoveredSource {
                    name: "/tmp/projects/alpha".to_string(),
                    locator: "/tmp/projects/alpha".to_string(),
                },
                DiscoveredSource {
                    name: "/tmp/projects/beta".to_string(),
                    locator: "/tmp/projects/beta".to_string(),
                },
            ],
        }])
    }
}

#[distributed_slice(DISCOVERY_PROBES)]
static OK_PROBE: &'static (dyn DiscoveryProbe + Sync) = &OkProbe;

struct FailingProbe;

#[async_trait::async_trait]
impl DiscoveryProbe for FailingProbe {
    fn name(&self) -> &'static str {
        "test_failing_probe"
    }

    async fn probe(&self, _kernel: &Kernel) -> KernelResult<Vec<DiscoveredAgent>> {
        Err(KernelError::Module("config unreadable".to_string()))
    }
}

#[distributed_slice(DISCOVERY_PROBES)]
static FAILING_PROBE: &'static (dyn DiscoveryProbe + Sync) = &FailingProbe;

// ─────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn probes_register_agents_sources_and_telemetry() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;

    // The failing probe makes run_probes return a Module error — but
    // only AFTER the ok probe's findings are fully registered.
    let err = run_probes(&kernel).await.expect_err("failing probe must surface");
    assert!(
        matches!(&err, KernelError::Module(m) if m.contains("test_failing_probe")),
        "got: {err:?}",
    );

    // Agent + sources registered.
    assert!(kernel
        .find_entity_by_name("node_agent", "attr_agent_descriptor", "claude_test")
        .await?
        .is_some());
    for source in ["/tmp/projects/alpha", "/tmp/projects/beta"] {
        assert!(kernel
            .find_entity_by_name("node_source", "attr_source_descriptor", source)
            .await?
            .is_some());
    }

    // Telemetry: the capture story starts here.
    let events = kernel.recent_telemetry(50).await?;
    let count = |name: &str| events.iter().filter(|e| e.lifecycle_event == name).count();
    assert_eq!(count("agent_discovered"), 1);
    assert_eq!(count("source_discovered"), 2);
    assert_eq!(count("probe_failed"), 1);
    assert_eq!(count("discovery_census"), 1, "census proves the scan ran");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rerun_is_idempotent_no_duplicates() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;

    let _ = run_probes(&kernel).await; // first run (Module error from failing probe is expected)
    let _ = run_probes(&kernel).await; // second run

    // Exactly one agent entity and two source entities exist.
    let agent_type = kernel.find_type("node_agent").await?;
    let source_type = kernel.find_type("node_source").await?;
    #[derive(SurrealValue)]
    struct CountRow {
        count: i64,
    }
    for (type_id, expected) in [(agent_type, 1), (source_type, 2)] {
        let counts: Vec<CountRow> = kernel
            .db()
            .query("SELECT count() AS count FROM entity WHERE type = $type GROUP ALL")
            .bind(("type", type_id))
            .await?
            .take(0)?;
        assert_eq!(counts.first().map(|c| c.count), Some(expected));
    }

    // Second run discovered nothing new — only "known" counts; one
    // agent_discovered event total across both runs.
    let events = kernel.recent_telemetry(100).await?;
    let discovered = events
        .iter()
        .filter(|e| e.lifecycle_event == "agent_discovered")
        .count();
    assert_eq!(discovered, 1, "re-run must not re-discover");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn module_startup_is_run_probes() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;
    // The KernelModule face of the framework: startup == one probe
    // pass, with the failing probe surfacing as a Module error that
    // bootstrap will record as Failed.
    let err = DiscoveryModule
        .startup(&kernel)
        .await
        .expect_err("failing probe surfaces through startup");
    assert!(matches!(err, KernelError::Module(_)));
    Ok(())
}
