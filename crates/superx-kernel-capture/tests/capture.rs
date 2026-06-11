//! Framework-level tests for the capture loop: dispatch by probe
//! name and per-watcher error isolation, with fake watchers
//! registered via linkme in this test binary.

use std::error::Error;
use std::sync::atomic::{AtomicU64, Ordering};

use linkme::distributed_slice;
use surrealdb::engine::any::connect;
use surrealdb::opt::auth::Database;

use superx_kernel::types::{Object, Value};
use superx_kernel::{Kernel, KernelError, Result as KernelResult, SCHEMA_DDL};
use superx_kernel_capture::{capture_tick, CaptureSource, SourceRef, CAPTURE_SOURCES};

const TEST_PASSWORD: &str = "test-kernel-password-for-mem-engine";

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
    kernel
        .ensure_type_definition("attr_source_descriptor", "attribute", "working")
        .await?;
    Ok(kernel)
}

/// Create a `node_source` with the descriptor shape discovery writes.
async fn make_source(kernel: &Kernel, name: &str, probe: &str) -> Result<(), Box<dyn Error>> {
    let id = kernel.create_entity("node_source", "user").await?;
    let mut desc = Object::new();
    desc.insert("name".to_string(), Value::String(name.to_string()));
    desc.insert("locator".to_string(), Value::String(name.to_string()));
    desc.insert("agent".to_string(), Value::String("test_agent".to_string()));
    desc.insert("probe".to_string(), Value::String(probe.to_string()));
    kernel
        .supersede_state(id, "attr_source_descriptor", Value::Object(desc))
        .await?;
    Ok(())
}

static OK_POLLS: AtomicU64 = AtomicU64::new(0);

struct OkWatcher;

#[async_trait::async_trait]
impl CaptureSource for OkWatcher {
    fn probe_name(&self) -> &'static str {
        "test_ok_probe"
    }

    async fn poll(&self, _kernel: &Kernel, _source: &SourceRef) -> KernelResult<u64> {
        OK_POLLS.fetch_add(1, Ordering::SeqCst);
        Ok(5)
    }
}

#[distributed_slice(CAPTURE_SOURCES)]
static OK_WATCHER: &'static (dyn CaptureSource + Sync) = &OkWatcher;

struct FailingWatcher;

#[async_trait::async_trait]
impl CaptureSource for FailingWatcher {
    fn probe_name(&self) -> &'static str {
        "test_failing_probe"
    }

    async fn poll(&self, _kernel: &Kernel, _source: &SourceRef) -> KernelResult<u64> {
        Err(KernelError::Module("watcher exploded".to_string()))
    }
}

#[distributed_slice(CAPTURE_SOURCES)]
static FAILING_WATCHER: &'static (dyn CaptureSource + Sync) = &FailingWatcher;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tick_dispatches_by_probe_and_isolates_watcher_errors() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;
    make_source(&kernel, "/src/ok", "test_ok_probe").await?;
    make_source(&kernel, "/src/bad", "test_failing_probe").await?;
    make_source(&kernel, "/src/orphan", "no_such_probe").await?;

    let report = capture_tick(&kernel).await?;

    // Orphan source (no watcher in this binary) is skipped, not an
    // error; the failing watcher is isolated; the ok watcher's events
    // are counted.
    assert_eq!(report.sources_polled, 2);
    assert_eq!(report.events_captured, 5);
    assert_eq!(report.errors, 1);
    assert!(OK_POLLS.load(Ordering::SeqCst) >= 1);

    let events = kernel.recent_telemetry(20).await?;
    let err_event = events
        .iter()
        .find(|e| e.lifecycle_event == "capture_error")
        .expect("watcher error must be telemetry, not silence");
    let rendered = serde_json::to_string(&err_event.payload)?;
    assert!(rendered.contains("watcher exploded"), "payload: {rendered}");
    Ok(())
}
