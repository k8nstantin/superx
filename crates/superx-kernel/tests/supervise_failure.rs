//! The failure policy (M0 review finding #1).
//!
//! Once starting is something that happens repeatedly, a module that
//! cannot start becomes an infinite retry loop — every pass appending
//! a `state_ledger` row and a telemetry event. At a 5s cadence that is
//! ~17k rows a day for one broken module, a failure mode that did not
//! exist while startup only ever ran at boot.
//!
//! This test binary registers its OWN failing module into
//! `KERNEL_MODULES` (linkme's slice is per-binary), which is why it
//! lives in a separate file from `supervise.rs` — the other tests must
//! keep a healthy inventory.

mod common;

use async_trait::async_trait;
use linkme::distributed_slice;
use superx_kernel::supervise::{
    reconcile_once, request_restart, BACKOFF_BASE_PARAM, BACKOFF_MAX_PARAM,
    RESTART_CEILING_PARAM,
};
use superx_kernel::types::Value;
use superx_kernel::{
    Kernel, KernelError, KernelModule, KernelModuleDescriptor, NodeKind, Result, KERNEL_MODULES,
};

use common::fresh_kernel;

const BROKEN: &str = "always_fails";

/// A module whose `startup()` never succeeds — a port already taken, a
/// missing dependency, a bad parameter. The kernel must bound its
/// enthusiasm.
struct AlwaysFails;

#[async_trait]
impl KernelModule for AlwaysFails {
    fn descriptor(&self) -> KernelModuleDescriptor {
        KernelModuleDescriptor {
            name: BROKEN,
            version: "0.0.0",
            kind: NodeKind::KernelModule,
            depends_on: &[],
            required_metamodel: &[],
        }
    }

    async fn startup(&self, _kernel: &Kernel) -> Result<()> {
        Err(KernelError::Module("cannot bind 127.0.0.1:5151".to_string()))
    }
}

#[distributed_slice(KERNEL_MODULES)]
static ALWAYS_FAILS_REGISTRATION: &'static (dyn KernelModule + Sync) = &AlwaysFails;

type R = std::result::Result<(), Box<dyn std::error::Error>>;

/// The anti-hammering property: after one failed start the kernel
/// waits before trying again. Twelve passes in a millisecond must not
/// mean twelve starts — which is exactly what the first cut of this
/// code did, appending a ledger row and an event every tick forever.
#[tokio::test]
async fn a_failed_start_backs_off_before_trying_again() -> R {
    let kernel = fresh_kernel().await?;
    superx_kernel::boot(&kernel).await?;
    assert!(
        !kernel.module_is_running(BROKEN),
        "a failed startup must not read as running, or nothing would ever retry it"
    );

    // Pin the backoff rather than relying on the default being longer
    // than this loop takes: on a slow machine twelve passes can exceed
    // a second, and the retry that follows is CORRECT behaviour being
    // read as a failure. The property under test is "a pending backoff
    // blocks a retry", so the backoff must be unambiguous.
    let knobs = kernel
        .find_module_by_name(NodeKind::KernelModule, "kernel")
        .await?
        .expect("the kernel's own entity");
    kernel
        .set_parameter(knobs, BACKOFF_BASE_PARAM, Value::Number(3_600_000.into()))
        .await?;

    reconcile_once(&kernel).await;
    assert_eq!(kernel.module_failure_count(BROKEN), 1, "one attempt recorded");

    for _ in 0..12 {
        reconcile_once(&kernel).await;
    }
    assert_eq!(
        kernel.module_failure_count(BROKEN),
        1,
        "still one: the kernel is waiting, not hammering"
    );
    assert!(!kernel.module_gave_up(BROKEN), "waiting is not giving up");
    Ok(())
}

/// And retry is not merely slow, it is finite: at the ceiling the
/// kernel stops entirely rather than retrying at the backoff cap
/// forever.
#[tokio::test]
async fn retries_stop_at_the_ceiling() -> R {
    let kernel = fresh_kernel().await?;
    superx_kernel::boot(&kernel).await?;
    let knobs = kernel
        .find_module_by_name(NodeKind::KernelModule, "kernel")
        .await?
        .expect("the kernel's own entity");
    // Collapse the backoff so the CEILING is what this test measures,
    // not the clock.
    for (param, value) in [
        (BACKOFF_BASE_PARAM, 1),
        (BACKOFF_MAX_PARAM, 1),
        (RESTART_CEILING_PARAM, 3),
    ] {
        kernel
            .set_parameter(knobs.clone(), param, Value::Number(value.into()))
            .await?;
    }

    for _ in 0..8 {
        reconcile_once(&kernel).await;
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    assert_eq!(
        kernel.module_failure_count(BROKEN),
        3,
        "eight passes, three starts: the kernel stopped at the ceiling"
    );
    assert!(
        kernel.module_gave_up(BROKEN),
        "and it records that it gave up, rather than going quietly silent"
    );

    // The count only moves when startup() ran and failed, so a frozen
    // count IS proof it was not called again.
    for _ in 0..4 {
        reconcile_once(&kernel).await;
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert_eq!(
        kernel.module_failure_count(BROKEN),
        3,
        "an abandoned module is not touched again"
    );
    Ok(())
}

/// A healthy module is unaffected by a broken neighbour — failure
/// isolation still holds through the reconciler, not just through boot.
#[tokio::test]
async fn a_broken_module_does_not_take_its_neighbours_down() -> R {
    let kernel = fresh_kernel().await?;
    superx_kernel::boot(&kernel).await?;
    reconcile_once(&kernel).await;

    assert!(kernel.module_is_running("capture"), "capture is unharmed");
    assert!(!kernel.module_is_running(BROKEN), "the broken one is not running");
    Ok(())
}

/// Asking for a restart is an operator saying "try again" — it must
/// clear an abandoned module's failure record, or the verb would be a
/// no-op exactly when it is most wanted.
#[tokio::test]
async fn an_explicit_restart_clears_an_abandoned_module() -> R {
    let kernel = fresh_kernel().await?;
    superx_kernel::boot(&kernel).await?;
    let kernel_entity = kernel
        .find_module_by_name(NodeKind::KernelModule, "kernel")
        .await?
        .expect("the kernel's own entity");
    kernel
        .set_parameter(
            kernel_entity,
            RESTART_CEILING_PARAM,
            Value::Number(1.into()),
        )
        .await?;
    reconcile_once(&kernel).await;
    assert!(kernel.module_gave_up(BROKEN), "abandoned at a ceiling of one");

    request_restart(&kernel, NodeKind::KernelModule, BROKEN).await?;
    assert!(
        !kernel.module_gave_up(BROKEN),
        "the operator's restart clears the record so the next pass tries again"
    );
    assert_eq!(kernel.module_failure_count(BROKEN), 0);
    Ok(())
}
