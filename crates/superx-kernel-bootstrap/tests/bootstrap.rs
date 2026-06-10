//! End-to-end tests for the boot orchestrator.
//!
//! A zoo of fake modules registers into `KERNEL_MODULES` via linkme in
//! this test binary — success, failure, panic, dependency chains,
//! cycles, unknown deps, per-module metamodel. Every test boots the
//! FULL inventory (the slice is binary-global; substrate state is
//! per-test via fresh `mem://` engines) and asserts the outcomes that
//! matter to it.

use std::error::Error;

use linkme::distributed_slice;
use surrealdb::engine::any::connect;
use surrealdb::opt::auth::Database;
use surrealdb::types::{Object, Value};

use superx_kernel::{
    Kernel, KernelModule, KernelModuleDescriptor, LifecycleState, MetamodelType, NodeKind,
    Result as KernelResult, KERNEL_MODULES, REQUIRED_METAMODEL_TYPES, SCHEMA_DDL,
};
use superx_kernel_bootstrap::{bootstrap, BootReport};

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
    Ok(Kernel::from_db(db))
}

fn outcome<'a>(report: &'a BootReport, name: &str) -> &'a LifecycleState {
    &report
        .entries
        .iter()
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("no boot entry for '{name}'"))
        .outcome
}

// ─────────────────────────────────────────────────────────────────────
// Fake inventory
// ─────────────────────────────────────────────────────────────────────

macro_rules! fake_module {
    ($struct_name:ident, $static_name:ident, $name:literal, $deps:expr, $metamodel:expr, $startup:expr) => {
        struct $struct_name;

        #[async_trait::async_trait]
        impl KernelModule for $struct_name {
            fn descriptor(&self) -> KernelModuleDescriptor {
                KernelModuleDescriptor {
                    name: $name,
                    version: "0.0.1",
                    kind: NodeKind::KernelModule,
                    category: "",
                    depends_on: $deps,
                    required_metamodel: $metamodel,
                }
            }

            async fn startup(&self, _kernel: &Kernel) -> KernelResult<()> {
                $startup
            }
        }

        #[distributed_slice(KERNEL_MODULES)]
        static $static_name: &'static (dyn KernelModule + Sync) = &$struct_name;
    };
}

fake_module!(TestOk, TEST_OK, "test_ok", &[], &[], Ok(()));
fake_module!(
    TestNeedsOk,
    TEST_NEEDS_OK,
    "test_needs_ok",
    &["test_ok"],
    &[],
    Ok(())
);
fake_module!(
    TestFailing,
    TEST_FAILING,
    "test_failing",
    &[],
    &[],
    Err(superx_kernel::KernelError::NotFound("boom".to_string()))
);
fake_module!(
    TestNeedsFailing,
    TEST_NEEDS_FAILING,
    "test_needs_failing",
    &["test_failing"],
    &[],
    Ok(())
);
fake_module!(
    TestPanicking,
    TEST_PANICKING,
    "test_panicking",
    &[],
    &[],
    panic!("kaboom")
);
fake_module!(
    TestCycleA,
    TEST_CYCLE_A,
    "test_cycle_a",
    &["test_cycle_b"],
    &[],
    Ok(())
);
fake_module!(
    TestCycleB,
    TEST_CYCLE_B,
    "test_cycle_b",
    &["test_cycle_a"],
    &[],
    Ok(())
);
fake_module!(
    TestStuckBehindCycle,
    TEST_STUCK_BEHIND_CYCLE,
    "test_stuck_behind_cycle",
    &["test_cycle_a"],
    &[],
    Ok(())
);
fake_module!(
    TestNeedsUnknown,
    TEST_NEEDS_UNKNOWN,
    "test_needs_unknown",
    &["no_such_module"],
    &[],
    Ok(())
);
fake_module!(
    TestWithMetamodel,
    TEST_WITH_METAMODEL,
    "test_with_metamodel",
    &[],
    &[MetamodelType {
        uid: "attr_test_bootstrap_param",
        category: "attribute",
        memory_tier: "working",
    }],
    Ok(())
);
fake_module!(
    TestDisableable,
    TEST_DISABLEABLE,
    "test_disableable",
    &[],
    &[],
    Ok(())
);
fake_module!(
    TestNeedsDisableable,
    TEST_NEEDS_DISABLEABLE,
    "test_needs_disableable",
    &["test_disableable"],
    &[],
    Ok(())
);

// ─────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn boot_walks_the_full_inventory_with_failure_isolation() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;
    let report = bootstrap(&kernel).await?;

    // Healthy chain: independent + dependent both Active; the
    // orchestrator's own registration too.
    assert!(matches!(outcome(&report, "test_ok"), LifecycleState::Active { .. }));
    assert!(matches!(outcome(&report, "test_needs_ok"), LifecycleState::Active { .. }));
    assert!(matches!(outcome(&report, "bootstrap"), LifecycleState::Active { .. }));

    // Failure is captured with the phase it failed in.
    match outcome(&report, "test_failing") {
        LifecycleState::Failed { error, failed_during } => {
            assert!(error.contains("boom"), "error captured: {error}");
            assert_eq!(failed_during, "startup");
        }
        other => panic!("test_failing must be Failed, got {other:?}"),
    }

    // Dependent of a failure is Skipped — never attempted, never Failed.
    match outcome(&report, "test_needs_failing") {
        LifecycleState::Skipped { reason } => {
            assert!(reason.contains("test_failing"), "reason names the dep: {reason}");
        }
        other => panic!("test_needs_failing must be Skipped, got {other:?}"),
    }

    // A panic inside startup() is isolated and recorded.
    match outcome(&report, "test_panicking") {
        LifecycleState::Failed { error, failed_during } => {
            assert!(error.contains("panicked"), "panic captured: {error}");
            assert_eq!(failed_during, "startup");
        }
        other => panic!("test_panicking must be Failed, got {other:?}"),
    }

    // Cycle members Fail at topo_sort; the module stuck behind the
    // cycle is Skipped, not Failed.
    for name in ["test_cycle_a", "test_cycle_b"] {
        match outcome(&report, name) {
            LifecycleState::Failed { failed_during, .. } => {
                assert_eq!(failed_during, "topo_sort");
            }
            other => panic!("{name} must be Failed(topo_sort), got {other:?}"),
        }
    }
    assert!(matches!(
        outcome(&report, "test_stuck_behind_cycle"),
        LifecycleState::Skipped { .. }
    ));

    // Unknown dependency name → Skipped with the name in the reason.
    match outcome(&report, "test_needs_unknown") {
        LifecycleState::Skipped { reason } => {
            assert!(reason.contains("no_such_module"), "reason: {reason}");
        }
        other => panic!("test_needs_unknown must be Skipped, got {other:?}"),
    }

    // Per-module metamodel was seeded.
    assert!(matches!(
        outcome(&report, "test_with_metamodel"),
        LifecycleState::Active { .. }
    ));
    assert!(kernel
        .find_type_opt("attr_test_bootstrap_param")
        .await?
        .is_some());

    // The substrate agrees with the report (report is a snapshot,
    // substrate is the truth).
    let substrate_view = kernel
        .detailed_status(NodeKind::KernelModule, "test_failing")
        .await?
        .expect("registered");
    assert!(matches!(substrate_view.lifecycle, LifecycleState::Failed { .. }));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reboot_is_idempotent() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;
    let first = bootstrap(&kernel).await?;
    let second = bootstrap(&kernel).await?;

    assert_eq!(first.active_count(), second.active_count());
    assert_eq!(first.entries.len(), second.entries.len());

    // No duplicate registry entities across boots.
    let listed = kernel.list_with_status(NodeKind::KernelModule).await?;
    let mut names: Vec<&str> = listed.iter().map(|s| s.name.as_str()).collect();
    let total = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), total, "re-boot must not duplicate registry entries");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabled_module_is_not_started_and_dependents_skip() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;

    // Pre-register the module and record the operator's disable —
    // exactly what a future `superx kernel modules disable` does.
    for t in REQUIRED_METAMODEL_TYPES {
        kernel
            .ensure_type_definition(t.uid, t.category, t.memory_tier)
            .await?;
    }
    let entity_id = kernel.register_module(&TestDisableable.descriptor()).await?;
    let mut payload = Object::new();
    payload.insert("value".to_string(), Value::String("disabled".to_string()));
    kernel
        .supersede_state(entity_id, "attr_module_status", Value::Object(payload))
        .await?;

    let report = bootstrap(&kernel).await?;

    assert!(matches!(
        outcome(&report, "test_disableable"),
        LifecycleState::Disabled
    ));
    match outcome(&report, "test_needs_disableable") {
        LifecycleState::Skipped { reason } => {
            assert!(reason.contains("test_disableable"), "reason: {reason}");
        }
        other => panic!("dependent of disabled module must be Skipped, got {other:?}"),
    }

    // Everything unrelated still boots — the system boots as much as
    // it can.
    assert!(matches!(outcome(&report, "test_ok"), LifecycleState::Active { .. }));
    Ok(())
}
