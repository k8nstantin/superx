//! `superx-mod-hello` — the REFERENCE module (epic #141).
//!
//! Deliberately tiny; its job is to exercise every facility of the
//! module contract in the shipped binary and to be the template
//! contributors copy:
//!
//! - **Own schema + data objects** — [`KernelModule::schema_ddl`]
//!   ships `schema/hello.surql`, provisioned into the module's OWN
//!   database (`superx/hello`) with its own service account.
//! - **Own dir** — `<home>/modules/hello/` ([`KernelModule::needs_dir`]).
//! - **Own log** — everything here logs with `target = "hello"`,
//!   routed to `<home>/modules/hello/logs/hello.log.<date>`.
//! - **Own CLI** — `superx hello greet <text…>` writes a greeting row
//!   into the module's own db and reads the total back.
//!
//! The kernel is the core: this module depends on `superx-kernel`
//! and nothing else in the workspace.

use async_trait::async_trait;
use linkme::distributed_slice;
use superx_kernel::{
    Kernel, KernelError, KernelModule, KernelModuleDescriptor, NodeKind, Result,
    KERNEL_MODULES,
};

pub const MODULE_NAME: &str = "hello";

/// The module's own schema, shipped inside the crate.
pub const SCHEMA_DDL: &str = include_str!("../schema/hello.surql");

pub struct HelloModule;

#[async_trait]
impl KernelModule for HelloModule {
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
        // Own dir (facility proof; ignore test handles without a home).
        if let Ok(dir) = kernel.module_dir(MODULE_NAME) {
            tracing::info!(target: "hello", dir = %dir.display(), "hello module dir ready");
        }
        Ok(())
    }

    fn schema_ddl(&self) -> Option<&'static str> {
        Some(SCHEMA_DDL)
    }

    fn needs_dir(&self) -> bool {
        true
    }

    /// `superx hello greet <text…>` — write a greeting into the
    /// module's OWN db, report the running total.
    async fn cli(&self, kernel: &Kernel, args: &[String]) -> Result<String> {
        match args.first().map(String::as_str) {
            Some("greet") => {
                use superx_kernel::types::Value;
                let text = if args.len() > 1 {
                    args[1..].join(" ")
                } else {
                    "hello from the hello module".to_string()
                };
                let db = kernel.module_db(MODULE_NAME).await?;
                // Everything below uses only the kernel's re-exported
                // types — module crates need no direct surrealdb dep.
                db.query(
                    "CREATE greeting SET text = $text, valid_from = time::now()",
                )
                .bind(("text", text.clone()))
                .await?
                .check()?;
                let mut resp = db
                    .query("SELECT count() AS c FROM greeting GROUP ALL")
                    .await?;
                let rows: Vec<Value> = resp.take(0)?;
                let total = rows
                    .first()
                    .and_then(|v| match v {
                        Value::Object(o) => match o.get("c") {
                            Some(Value::Number(n)) => n.to_int(),
                            _ => None,
                        },
                        _ => None,
                    })
                    .unwrap_or(0);
                tracing::info!(target: "hello", %text, total, "greeting stored");
                Ok(format!(
                    "stored greeting #{total} in the hello module's own db: {text}\n"
                ))
            }
            _ => Err(KernelError::Module(
                "usage: superx hello greet [text…]".into(),
            )),
        }
    }
}

#[distributed_slice(KERNEL_MODULES)]
static HELLO_REGISTRATION: &'static (dyn KernelModule + Sync) = &HelloModule;
