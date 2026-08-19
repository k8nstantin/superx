//! End-to-end boot-orchestrator tests: fake modules registered via
//! linkme in THIS test binary exercise success, failure, dependency
//! skipping, panic isolation, cycles, and operator-disable — all in a
//! single boot pass (the KERNEL_MODULES slice is link-time global, so
//! one boot sees them all; assertions pick out their module by name).

mod common;

use std::error::Error;

use linkme::distributed_slice;
use surrealdb::types::{Object, Value};
use superx_kernel::{
    boot, Kernel, KernelError, KernelModule, KernelModuleDescriptor, LifecycleState, NodeKind,
    KERNEL_MODULES,
};

macro_rules! fake_module {
    ($ty:ident, $static_name:ident, $name:literal, deps: $deps:expr, body: $body:expr) => {
        struct $ty;
        #[async_trait::async_trait]
        impl KernelModule for $ty {
            fn descriptor(&self) -> KernelModuleDescriptor {
                KernelModuleDescriptor {
                    name: $name,
                    version: "0.0.1",
                    kind: NodeKind::KernelModule,
                    depends_on: $deps,
                    required_metamodel: &[],
                }
            }
            async fn startup(&self, _kernel: &Kernel) -> superx_kernel::Result<()> {
                $body
            }
        }
        #[distributed_slice(KERNEL_MODULES)]
        static $static_name: &'static (dyn KernelModule + Sync) = &$ty;
    };
}

fake_module!(OkModule, OK_MODULE, "fake_ok", deps: &[], body: Ok(()));
fake_module!(FailModule, FAIL_MODULE, "fake_fail", deps: &[],
    body: Err(KernelError::Module("deliberate test failure".into())));
fake_module!(DependentModule, DEPENDENT_MODULE, "fake_dependent", deps: &["fake_fail"],
    body: Ok(()));
fake_module!(PanicModule, PANIC_MODULE, "fake_panic", deps: &[],
    body: panic!("deliberate test panic"));
fake_module!(CycleA, CYCLE_A, "fake_cycle_a", deps: &["fake_cycle_b"], body: Ok(()));
fake_module!(CycleB, CYCLE_B, "fake_cycle_b", deps: &["fake_cycle_a"], body: Ok(()));
fake_module!(UnknownDep, UNKNOWN_DEP, "fake_unknown_dep", deps: &["no_such_module"],
    body: Ok(()));
fake_module!(DisabledModule, DISABLED_MODULE, "fake_disabled", deps: &[], body: Ok(()));

fn outcome_of<'r>(report: &'r boot::BootReport, name: &str) -> &'r LifecycleState {
    &report
        .entries
        .iter()
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("entry for {name}"))
        .outcome
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn boot_isolates_failures_and_continues() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_kernel().await?;

    // Pre-register + disable fake_disabled so boot sees operator
    // intent (registration is idempotent; boot re-registers).
    for t in superx_kernel::REQUIRED_METAMODEL_TYPES {
        kernel
            .ensure_type_definition(t.uid, t.category, t.memory_tier)
            .await?;
    }
    let disabled_id = kernel
        .register_module(&DisabledModule.descriptor())
        .await?;
    let mut payload = Object::new();
    payload.insert("value".to_string(), Value::String("disabled".to_string()));
    kernel
        .supersede_state(disabled_id, "attr_module_status", Value::Object(payload))
        .await?;

    let report = boot::boot(&kernel).await?;

    // The kernel's own entry and the healthy fake are Active.
    assert!(matches!(
        outcome_of(&report, "kernel"),
        LifecycleState::Active { .. }
    ));
    assert!(matches!(
        outcome_of(&report, "fake_ok"),
        LifecycleState::Active { .. }
    ));

    // Failure is captured, not fatal.
    match outcome_of(&report, "fake_fail") {
        LifecycleState::Failed {
            error,
            failed_during,
        } => {
            assert!(error.contains("deliberate test failure"));
            assert_eq!(failed_during, "startup");
        }
        other => panic!("fake_fail should be Failed, got {other:?}"),
    }

    // Dependent of the failure is Skipped, not Failed.
    match outcome_of(&report, "fake_dependent") {
        LifecycleState::Skipped { reason } => assert!(reason.contains("fake_fail")),
        other => panic!("fake_dependent should be Skipped, got {other:?}"),
    }

    // Panics are caught and recorded.
    match outcome_of(&report, "fake_panic") {
        LifecycleState::Failed { error, .. } => assert!(error.contains("panicked")),
        other => panic!("fake_panic should be Failed, got {other:?}"),
    }

    // Cycle members fail during topo_sort.
    for name in ["fake_cycle_a", "fake_cycle_b"] {
        match outcome_of(&report, name) {
            LifecycleState::Failed { failed_during, .. } => {
                assert_eq!(failed_during, "topo_sort")
            }
            other => panic!("{name} should be Failed(topo_sort), got {other:?}"),
        }
    }

    // Unknown dependency → Skipped with a reason naming it.
    match outcome_of(&report, "fake_unknown_dep") {
        LifecycleState::Skipped { reason } => assert!(reason.contains("no_such_module")),
        other => panic!("fake_unknown_dep should be Skipped, got {other:?}"),
    }

    // Operator-disabled module is not attempted.
    assert!(matches!(
        outcome_of(&report, "fake_disabled"),
        LifecycleState::Disabled
    ));

    // The boot emitted exactly one system_boot (start marker) and one
    // boot_complete (end-of-walk marker, with the duration record).
    let recent = kernel.recent_telemetry(200).await?;
    let count = |name: &str| recent.iter().filter(|e| e.lifecycle_event == name).count();
    assert_eq!(count("system_boot"), 1, "one system_boot per real boot");
    assert_eq!(count("boot_complete"), 1, "one boot_complete per real boot");
    let done = recent
        .iter()
        .find(|e| e.lifecycle_event == "boot_complete")
        .expect("boot_complete present");
    let payload = format!("{:?}", done.payload);
    assert!(payload.contains("active"), "{payload}");
    assert!(payload.contains("duration_ms"), "{payload}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reboot_is_idempotent_no_duplicate_entities() -> Result<(), Box<dyn Error>> {
    let kernel = common::fresh_kernel().await?;
    boot::boot(&kernel).await?;
    boot::boot(&kernel).await?;

    // Each module name has exactly ONE registry entity despite two
    // boots (find-then-create idempotency).
    let modules = kernel.list_with_status(NodeKind::KernelModule).await?;
    let mut names: Vec<&str> = modules.iter().map(|s| s.name.as_str()).collect();
    let before = names.len();
    names.dedup();
    names.sort_unstable();
    names.dedup();
    assert_eq!(before, names.len(), "no duplicate registry entities");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn boot_survives_a_broken_ledger() -> Result<(), Box<dyn Error>> {
    // Issue #158 end-to-end: an incompatible ledger → boot must still
    // bring the core up. The whole point of being modular.
    let kernel = common::fresh_kernel().await?;
    kernel
        .db()
        .query(include_str!("fixtures/ledger_sabotage.surql"))
        .await?
        .check()?;

    let report = boot::boot(&kernel).await?;
    assert!(
        report.active_count() > 0,
        "core modules active despite the missing ledger: {:?}",
        report
            .entries
            .iter()
            .map(|e| (&e.name, e.outcome.short_tag()))
            .collect::<Vec<_>>()
    );
    assert!(matches!(
        outcome_of(&report, "kernel"),
        LifecycleState::Active { .. }
    ));
    Ok(())
}
