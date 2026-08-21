//! Module lifecycle, made real (M0).
//!
//! Every test here fails against the previous code, where
//! `KernelModule::shutdown` had no call sites anywhere in the
//! workspace and `disable` only paused a module.
//!
//! The kernel's own test binary links only `superx-kernel`, so the
//! compiled-in inventory is `kernel` + `capture`. Both have no-op
//! `startup`/`shutdown`, which is exactly right for pinning the
//! kernel's bookkeeping: what is under test is who gets started,
//! stopped and cycled, not what a particular module does about it.

mod common;

use std::time::Duration;

use superx_kernel::supervise::{reconcile_once, request_restart, shutdown_all, RESTART_PARAM};
use superx_kernel::{ModuleStatus, NodeKind};

use common::fresh_kernel;

const MODULE: &str = "capture";

type R = Result<(), Box<dyn std::error::Error>>;

#[tokio::test]
async fn boot_records_what_it_started() -> R {
    let kernel = fresh_kernel().await?;
    superx_kernel::boot(&kernel).await?;
    assert!(
        kernel.module_is_running(MODULE),
        "boot must record its startups, or the first reconcile would start everything twice"
    );
    assert!(kernel.module_started_at(MODULE).is_some());
    assert!(kernel.running_modules().contains(&MODULE.to_string()));
    Ok(())
}

#[tokio::test]
async fn disable_stops_the_module_and_cancels_its_token() -> R {
    let kernel = fresh_kernel().await?;
    superx_kernel::boot(&kernel).await?;
    // What a module's own tasks hold — a server's graceful-shutdown
    // future, a loop's poll check.
    let token = kernel.module_token(MODULE);
    assert!(!token.is_cancelled(), "a running module's token is live");

    kernel
        .set_module_status(NodeKind::KernelModule, MODULE, ModuleStatus::Disabled)
        .await?;
    reconcile_once(&kernel).await;

    assert!(
        !kernel.module_is_running(MODULE),
        "disable STOPS the module — the defect M0 fixes"
    );
    assert!(
        token.is_cancelled(),
        "the module's tasks are told, so a server can release its port"
    );
    // And the await resolves rather than hanging forever.
    tokio::time::timeout(Duration::from_secs(2), token.cancelled())
        .await
        .expect("cancelled() resolves once the module is stopped");
    Ok(())
}

#[tokio::test]
async fn enable_starts_it_again_with_a_live_token() -> R {
    let kernel = fresh_kernel().await?;
    superx_kernel::boot(&kernel).await?;
    kernel
        .set_module_status(NodeKind::KernelModule, MODULE, ModuleStatus::Disabled)
        .await?;
    reconcile_once(&kernel).await;
    assert!(!kernel.module_is_running(MODULE));

    kernel
        .set_module_status(NodeKind::KernelModule, MODULE, ModuleStatus::Enabled)
        .await?;
    reconcile_once(&kernel).await;

    assert!(kernel.module_is_running(MODULE), "enable restarts it, no OS restart");
    assert!(
        !kernel.module_token(MODULE).is_cancelled(),
        "the new life gets a FRESH token — inheriting the cancelled one \
         would close the new server the instant it opened"
    );
    Ok(())
}

#[tokio::test]
async fn a_restart_request_cycles_the_module_exactly_once() -> R {
    let kernel = fresh_kernel().await?;
    superx_kernel::boot(&kernel).await?;
    let before = kernel.module_started_at(MODULE).expect("running after boot");
    let token = kernel.module_token(MODULE);

    request_restart(&kernel, NodeKind::KernelModule, MODULE).await?;
    // The request is a substrate fact — written by a different
    // process, which is the whole reason a reconciler exists.
    let entity = kernel
        .find_module_by_name(NodeKind::KernelModule, MODULE)
        .await?
        .expect("registered");
    assert!(
        kernel.get_parameter(entity, RESTART_PARAM).await?.is_some(),
        "restart lives in the substrate, not in daemon memory"
    );

    reconcile_once(&kernel).await;
    let after = kernel.module_started_at(MODULE).expect("running after restart");
    assert!(after > before, "the module was cycled: {after} > {before}");
    assert!(token.is_cancelled(), "the old life's tasks were stopped");

    reconcile_once(&kernel).await;
    let third = kernel.module_started_at(MODULE).expect("still running");
    assert_eq!(after, third, "a spent request does not re-fire every tick");
    Ok(())
}

#[tokio::test]
async fn shutdown_all_stops_everything_running() -> R {
    let kernel = fresh_kernel().await?;
    superx_kernel::boot(&kernel).await?;
    assert!(!kernel.running_modules().is_empty(), "boot started modules");
    let token = kernel.module_token(MODULE);

    shutdown_all(&kernel).await;

    assert!(
        kernel.running_modules().is_empty(),
        "OS shutdown stops modules instead of tearing them down mid-flight"
    );
    assert!(token.is_cancelled(), "their tasks get to close cleanly");
    Ok(())
}

#[tokio::test]
async fn a_reconcile_converges_and_then_holds_still() -> R {
    let kernel = fresh_kernel().await?;
    superx_kernel::boot(&kernel).await?;
    kernel
        .set_module_status(NodeKind::KernelModule, MODULE, ModuleStatus::Disabled)
        .await?;
    reconcile_once(&kernel).await;
    assert!(!kernel.module_is_running(MODULE), "converged to stopped");

    // Idempotence is the property that makes a tick loop safe: the
    // second pass has nothing to do and must do nothing.
    let started_before = kernel.module_started_at(MODULE);
    reconcile_once(&kernel).await;
    assert!(!kernel.module_is_running(MODULE), "and stays stopped");
    assert_eq!(started_before, kernel.module_started_at(MODULE));
    Ok(())
}

#[tokio::test]
async fn an_unknown_module_cannot_be_restarted() -> R {
    let kernel = fresh_kernel().await?;
    superx_kernel::boot(&kernel).await?;
    let err = request_restart(&kernel, NodeKind::KernelModule, "no-such-module")
        .await
        .expect_err("an unknown module is refused, never silently accepted");
    assert!(err.to_string().contains("no-such-module"), "the error names it: {err}");
    Ok(())
}
