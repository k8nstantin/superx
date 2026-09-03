//! `superx-mod-ui` — the UI module (epic #141, P4 skeleton).
//!
//! A web dashboard served BY the running OS: the module's `startup()`
//! spawns an axum server on `127.0.0.1:<attr_ui_port>` (substrate
//! parameter on this module's registry entity). P4 ships the seam —
//! JSON status, whitelisted command execution, a placeholder page;
//! the read API + SSE arrive in P5, the React dashboard in P6.
//!
//! Facilities exercised: own CLI (`superx ui url`), own parameters,
//! own dir (declared for P5's needs), the kernel as the only hub —
//! everything rendered here is read through kernel verbs or
//! `superx-ops` runners.

use async_trait::async_trait;
use linkme::distributed_slice;
use superx_kernel::types::Value;
use superx_kernel::{
    Kernel, KernelError, KernelModule, KernelModuleDescriptor, NodeKind, Result,
    KERNEL_MODULES,
};

pub mod activity;
pub mod api;
pub mod insights;
mod server;
pub mod stats;

pub const MODULE_NAME: &str = "ui";

/// Port parameter on the ui module's registry entity.
pub const PORT_PARAM: &str = "attr_ui_port";

/// Default port when the parameter is unset.
pub const DEFAULT_PORT: u16 = 5150; // skill-allow: §9-const — bootstrap fallback, param-overridable

/// The UI module's own schema (command history, preferences) —
/// provisioned into `superx/ui` by `superx modules provision ui`.
pub const SCHEMA_DDL: &str = include_str!("../schema/ui.surql");

pub struct UiModule;

/// Resolve the UI port: parameter on the module entity, else default.
pub async fn resolved_port(kernel: &Kernel) -> u16 {
    let Ok(Some(entity)) = kernel
        .find_module_by_name(NodeKind::KernelModule, MODULE_NAME)
        .await
    else {
        return DEFAULT_PORT;
    };
    match kernel.get_parameter(entity, PORT_PARAM).await {
        Ok(Some(Value::Number(n))) => n
            .to_int()
            .and_then(|i| u16::try_from(i).ok())
            .filter(|&p| p > 0)
            .unwrap_or(DEFAULT_PORT),
        _ => DEFAULT_PORT,
    }
}

/// The UI's URL for this instance.
pub async fn resolved_url(kernel: &Kernel) -> String {
    format!("http://127.0.0.1:{}", resolved_port(kernel).await)
}

/// Context-window size parameter (tokens) on the ui module's registry
/// entity — the denominator of the Sessions list's context bar
/// (issue #202).
pub const CONTEXT_WINDOW_PARAM: &str = "attr_ui_context_window_tokens";

/// Default context window when the parameter is unset.
pub const DEFAULT_CONTEXT_WINDOW: i64 = 1_000_000; // skill-allow: §9-const — bootstrap fallback, param-overridable

/// Raw-message window for the Status page's stats walk (issue #228).
pub const STATS_WINDOW_PARAM: &str = "attr_ui_stats_window_messages";

/// Default stats window when the parameter is unset.
pub const DEFAULT_STATS_WINDOW: u32 = 500; // skill-allow: §9-const — bootstrap fallback, param-overridable

/// Resolve the stats window: parameter on the module entity, else
/// default.
pub async fn resolved_stats_window(kernel: &Kernel) -> u32 {
    let Ok(Some(entity)) = kernel
        .find_module_by_name(NodeKind::KernelModule, MODULE_NAME)
        .await
    else {
        return DEFAULT_STATS_WINDOW;
    };
    match kernel.get_parameter(entity, STATS_WINDOW_PARAM).await {
        Ok(Some(Value::Number(n))) => n
            .to_int()
            .and_then(|v| u32::try_from(v).ok())
            .filter(|&v| v > 0)
            .unwrap_or(DEFAULT_STATS_WINDOW),
        _ => DEFAULT_STATS_WINDOW,
    }
}

/// The range the cockpit lands on (#367) — a parameter on the ui
/// module's registry entity. The newest-500 window is about two hours
/// at a working instance's rate and lands every quality instrument on
/// zero, so the landing view is a decision, and decisions live in the
/// substrate.
pub const DEFAULT_RANGE_PARAM: &str = "attr_ui_default_range";

/// Fallback when the parameter is unset or names no known range.
pub const DEFAULT_RANGE: &str = "24h";

/// Every range the stats API accepts. `window` is the newest-N read;
/// the rest are rolling time bounds; `all` is unbounded and row-capped.
pub const RANGES: [&str; 7] = ["window", "1h", "6h", "24h", "7d", "30d", "all"];

/// Resolve the landing range: parameter on the module entity when it
/// names a known range, else the fallback.
pub async fn resolved_default_range(kernel: &Kernel) -> String {
    let Ok(Some(entity)) = kernel
        .find_module_by_name(NodeKind::KernelModule, MODULE_NAME)
        .await
    else {
        return DEFAULT_RANGE.to_string();
    };
    match kernel.get_parameter(entity, DEFAULT_RANGE_PARAM).await {
        Ok(Some(Value::String(r))) if RANGES.contains(&r.as_str()) => r,
        _ => DEFAULT_RANGE.to_string(),
    }
}

/// Resolve the context-window size: parameter on the module entity,
/// else default.
pub async fn resolved_context_window(kernel: &Kernel) -> i64 {
    let Ok(Some(entity)) = kernel
        .find_module_by_name(NodeKind::KernelModule, MODULE_NAME)
        .await
    else {
        return DEFAULT_CONTEXT_WINDOW;
    };
    match kernel.get_parameter(entity, CONTEXT_WINDOW_PARAM).await {
        Ok(Some(Value::Number(n))) => n
            .to_int()
            .filter(|&v| v > 0)
            .unwrap_or(DEFAULT_CONTEXT_WINDOW),
        _ => DEFAULT_CONTEXT_WINDOW,
    }
}

#[async_trait]
impl KernelModule for UiModule {
    fn descriptor(&self) -> KernelModuleDescriptor {
        KernelModuleDescriptor {
            name: MODULE_NAME,
            version: env!("CARGO_PKG_VERSION"),
            kind: NodeKind::KernelModule,
            depends_on: &[],
            required_metamodel: &[],
        }
    }

    async fn startup(&self, kernel: &Kernel) -> Result<()> {
        let port = resolved_port(kernel).await;
        server::spawn(kernel.clone(), port).await?;
        let url = format!("http://127.0.0.1:{port}");
        tracing::info!(target: "ui", %url, "ui module serving");
        let mut payload = superx_kernel::types::Object::new();
        payload.insert("url".to_string(), Value::String(url));
        kernel
            .log_telemetry("ui_started", Value::Object(payload), None)
            .await?;
        Ok(())
    }

    fn schema_ddl(&self) -> Option<&'static str> {
        Some(SCHEMA_DDL)
    }

    fn needs_dir(&self) -> bool {
        true // exports/downloads land here
    }

    /// `superx ui url` — where the dashboard lives.
    async fn cli(&self, kernel: &Kernel, args: &[String]) -> Result<String> {
        match args.first().map(String::as_str) {
            Some("url") | None => Ok(format!("{}\n", resolved_url(kernel).await)),
            _ => Err(KernelError::Module("usage: superx ui url".into())),
        }
    }
}

#[distributed_slice(KERNEL_MODULES)]
static UI_REGISTRATION: &'static (dyn KernelModule + Sync) = &UiModule;
