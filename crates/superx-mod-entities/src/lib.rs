//! superx-mod-entities — the product graph substrate (epic #166).
//!
//! Typed entities as graph nodes (product, task, rag, model, document,
//! text — extensible at runtime), connected by native SurrealDB edges
//! into a many-to-many graph with full append-only history. Agents
//! will later travel this graph and execute the task nodes; this
//! module owns the substrate: construct, link, traverse, visualize.
//!
//! Module contract (epic #141): own database `superx/entities`, own
//! service account, own dir (`modules/entities/` — document files),
//! own log (`target: "entities"`), own CLI (`superx entities …`).

pub mod cli;
pub mod documents;
pub mod edges;
pub mod graph;
pub mod nodes;
pub mod registry;
mod server;
pub mod texts;

use async_trait::async_trait;
use superx_kernel::types::Value;
use superx_kernel::{
    Kernel, KernelModule, KernelModuleDescriptor, NodeKind, Result, KERNEL_MODULES,
};

/// The module's name — db name, dir name, CLI namespace, log target.
pub const MODULE_NAME: &str = "entities";

/// Port parameter for this module's OWN UI (epic #216, EU1) on its
/// registry entity.
pub const UI_PORT_PARAM: &str = "attr_entities_ui_port";

/// Default UI port when the parameter is unset.
pub const DEFAULT_UI_PORT: u16 = 5151; // skill-allow: §9-const — bootstrap fallback, param-overridable

/// The convention every module with a UI follows (D-UI2): its UI URL
/// as a parameter on its own registry entity — the core dashboard
/// discovers module UIs from the substrate, never via code coupling.
pub const MODULE_UI_URL_PARAM: &str = "attr_module_ui_url";

/// Resolve this module's UI port: parameter on the module entity,
/// else default.
pub async fn resolved_ui_port(kernel: &Kernel) -> u16 {
    let Ok(Some(entity)) = kernel
        .find_module_by_name(NodeKind::KernelModule, MODULE_NAME)
        .await
    else {
        return DEFAULT_UI_PORT;
    };
    match kernel.get_parameter(entity, UI_PORT_PARAM).await {
        Ok(Some(Value::Number(n))) => n
            .to_int()
            .and_then(|i| u16::try_from(i).ok())
            .filter(|&p| p > 0)
            .unwrap_or(DEFAULT_UI_PORT),
        _ => DEFAULT_UI_PORT,
    }
}

/// This module's UI URL.
pub async fn resolved_ui_url(kernel: &Kernel) -> String {
    format!("http://127.0.0.1:{}", resolved_ui_port(kernel).await)
}

/// The module's own schema, applied by `superx modules provision
/// entities` into `superx/entities` (kernel schema untouched — §7).
pub const SCHEMA_DDL: &str = include_str!("../schema/entities.surql");

/// The graph substrate module. A ZST — all state lives in the
/// substrate, per the module contract.
pub struct EntitiesModule;

#[async_trait]
impl KernelModule for EntitiesModule {
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
        // Provisioning is an operator step. An unprovisioned module
        // must not fail the boot walk (the OS runs; the module waits
        // for `superx modules provision entities`). A provisioned-but-
        // broken registry, however, fails loudly and isolates.
        match kernel.module_db(MODULE_NAME).await {
            Ok(db) => {
                let seeded = registry::seed_types(&db).await?;
                if seeded > 0 {
                    tracing::info!(target: "entities", seeded, "type registry seeded");
                } else {
                    tracing::info!(target: "entities", "type registry current");
                }
            }
            Err(e) => {
                tracing::warn!(
                    target: "entities",
                    "own db unavailable — graph verbs disabled until provisioned: {e}"
                );
            }
        }
        // The module's OWN UI (epic #216, EU1): spawn the server and
        // publish the URL on this module's registry entity so the core
        // dashboard discovers it from the substrate (D-UI2).
        let port = resolved_ui_port(kernel).await;
        server::spawn(kernel.clone(), port).await?;
        let url = format!("http://127.0.0.1:{port}");
        if let Ok(Some(entity)) = kernel
            .find_module_by_name(NodeKind::KernelModule, MODULE_NAME)
            .await
        {
            kernel
                .set_parameter(entity.clone(), MODULE_UI_URL_PARAM, Value::String(url.clone()))
                .await?;
            let mut payload = superx_kernel::types::Object::new();
            payload.insert("url".to_string(), Value::String(url.clone()));
            kernel
                .log_telemetry("entities_ui_started", Value::Object(payload), Some(entity))
                .await?;
        }
        tracing::info!(target: "entities", %url, "entities ui serving");
        Ok(())
    }

    fn schema_ddl(&self) -> Option<&'static str> {
        Some(SCHEMA_DDL)
    }

    fn needs_dir(&self) -> bool {
        true
    }

    async fn cli(&self, kernel: &Kernel, args: &[String]) -> Result<String> {
        cli::dispatch(kernel, args).await
    }
}

#[linkme::distributed_slice(KERNEL_MODULES)]
static ENTITIES_REGISTRATION: &'static (dyn KernelModule + Sync) = &EntitiesModule;
