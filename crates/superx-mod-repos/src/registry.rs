//! The module's entry in the OS's registry, kept in the order the kernel's
//! boot keeps a linked module's (`superx-kernel/src/boot.rs`): registered
//! and starting, then active with its startup time or failed with why, and
//! disabled once it stops. `superx status`, `superx param` and the core
//! UI's Status page read it there, and the core UI links to the module
//! through the URL it publishes (D-UI2).

use superx_kernel::types::{RecordId, Value};
use superx_kernel::{Kernel, NodeKind, Result};

use crate::{descriptor, MODULE_NAME};

/// Where a module that serves its own page publishes its URL, on its
/// registry entity (D-UI2); the core UI reads it there.
pub const MODULE_UI_URL_PARAM: &str = "attr_module_ui_url";

/// Register (again, when it ran before) and say it is starting.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the registry cannot be written.
pub async fn starting(kernel: &Kernel) -> Result<RecordId> {
    let entity = kernel.register_module(&descriptor()).await?;
    kernel.mark_starting(NodeKind::KernelModule, MODULE_NAME).await?;
    Ok(entity)
}

/// Running: its URL published, whether its own database is provisioned
/// recorded in the module ledger when that changed, and active with its
/// startup time.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the URL or the lifecycle cannot be
/// written. The ledger is observability and never stops a start (#158).
pub async fn active(kernel: &Kernel, entity: RecordId, url: &str, startup_ms: u64) -> Result<()> {
    let provisioned = kernel.module_db(MODULE_NAME).await.is_ok();
    match kernel.latest_module_record(MODULE_NAME).await {
        Ok(Some(record)) if record.provisioned == provisioned => {}
        Ok(_) => {
            if let Err(e) = kernel.append_module_record(&descriptor(), entity.clone(), provisioned).await {
                tracing::warn!(target: "repos", error = %e, "module ledger append skipped");
            }
        }
        Err(e) => tracing::warn!(target: "repos", error = %e, "module ledger unreadable"),
    }
    kernel.set_parameter(entity, MODULE_UI_URL_PARAM, Value::String(url.to_string())).await?;
    kernel.mark_active(NodeKind::KernelModule, MODULE_NAME, startup_ms).await
}

/// It could not start, and why.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the registry cannot be written.
pub async fn failed(kernel: &Kernel, why: &str) -> Result<()> {
    kernel.mark_failed(NodeKind::KernelModule, MODULE_NAME, why, "startup").await
}

/// It stopped.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the registry cannot be written.
pub async fn stopped(kernel: &Kernel) -> Result<()> {
    kernel.mark_disabled(NodeKind::KernelModule, MODULE_NAME).await
}
