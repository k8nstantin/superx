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

pub mod api;
mod server;

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
