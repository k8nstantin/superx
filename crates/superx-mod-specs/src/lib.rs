//! superx-mod-specs — how far each build drifted from the XML spec it
//! was built from (#446).
//!
//! A design session writes a spec; a build session is pointed at it and
//! builds. When a build is thrown away, either the spec was wrong or the
//! build did not follow it. This module records, for every build, what
//! the spec told it and what it did, so the answer is a reading of
//! facts, not a guess. This first part records the spec files, the
//! builds, and one check: the read order (level 1).
//!
//! READ-ONLY, ABSOLUTELY. Everything comes from the sessions the kernel
//! already captured, read with SELECT through kernel verbs and the
//! kernel's own connection. This module opens no repository, runs no
//! git, starts no process, and writes nothing outside its own database.
//!
//! MODULE CONTRACT (D17):
//!
//!   own database   `superx/specs`, its own service account
//!   own schema     `schema/specs.surql`, applied at provision
//!   own log        `target: "specs"`
//!   own CLI        `superx specs ingest`, `superx specs runs`,
//!                  `superx specs ledger [KEY]`
//!
//! Nothing else in the workspace names this crate except the binary's
//! link line: removing it removes the module and changes nothing else.

pub mod gates;
pub mod git;
pub mod ingest;
pub mod ledger;
pub mod ownership;
pub mod read_order;
pub mod session;
pub mod spec;
pub mod store;

use std::fmt::Write as _;

use async_trait::async_trait;
use superx_kernel::types::Value;
use superx_kernel::{Kernel, KernelError, KernelModule, KernelModuleDescriptor, NodeKind, Result, KERNEL_MODULES};

/// The module's name — db name, CLI namespace, log target.
pub const MODULE_NAME: &str = "specs";

/// The module's own schema, applied by `superx modules provision specs`
/// into `superx/specs` (kernel schema untouched — §7).
pub const SCHEMA_DDL: &str = include_str!("../schema/specs.surql");

const USAGE: &str = "usage: superx specs ingest | superx specs runs | superx specs ledger [KEY]";

/// The specs module. A ZST — all state lives in the substrate.
pub struct SpecsModule;

#[async_trait]
impl KernelModule for SpecsModule {
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
        // Provisioning is an operator step; an unprovisioned module must
        // not fail the boot walk. Nothing runs at boot: an ingest is asked
        // for.
        match kernel.module_db(MODULE_NAME).await {
            Ok(_) => tracing::info!(target: "specs", "substrate ready"),
            Err(e) => tracing::warn!(target: "specs", "own db unavailable — module idle until provisioned: {e}"),
        }
        Ok(())
    }

    fn schema_ddl(&self) -> Option<&'static str> {
        Some(SCHEMA_DDL)
    }

    async fn cli(&self, kernel: &Kernel, args: &[String]) -> Result<String> {
        match args.first().map(String::as_str) {
            Some("ingest") => {
                let db = kernel.module_db(MODULE_NAME).await?;
                let r = ingest::ingest(kernel, &db).await?;
                let mut payload = superx_kernel::types::Object::new();
                for (k, v) in [
                    ("sessions", r.sessions),
                    ("builds", r.builds),
                    ("runs_written", r.runs_written),
                    ("runs_unchanged", r.runs_unchanged),
                    ("rejudged", r.rejudged),
                    ("docs_written", r.docs_written),
                    ("revisions_written", r.revisions_written),
                    ("branch_events_written", r.branch_events_written),
                    ("cut_short", r.cut_short),
                ] {
                    payload.insert(k.to_string(), Value::Number(i64::try_from(v).unwrap_or(i64::MAX).into()));
                }
                kernel.log_telemetry("specs_ingested", Value::Object(payload), None).await?;
                Ok(format!(
                    "{} sessions read, {} builds found: {} runs written, {} unchanged ({} re-judged), {} new spec \
                     files, {} new spec revisions, {} new branch events{}\n",
                    r.sessions,
                    r.builds,
                    r.runs_written,
                    r.runs_unchanged,
                    r.rejudged,
                    r.docs_written,
                    r.revisions_written,
                    r.branch_events_written,
                    if r.cut_short > 0 { format!(", {} judged on their first rows only", r.cut_short) } else { String::new() }
                ))
            }
            Some("ledger") => {
                let db = kernel.module_db(MODULE_NAME).await?;
                ledger::render(&db, args.get(1).map(String::as_str)).await
            }
            Some("runs") => {
                let db = kernel.module_db(MODULE_NAME).await?;
                Ok(render_runs(&store::current_runs(&db).await?))
            }
            _ => Err(KernelError::Module(USAGE.into())),
        }
    }
}

#[linkme::distributed_slice(KERNEL_MODULES)]
static SPECS_REGISTRATION: &'static (dyn KernelModule + Sync) = &SpecsModule;

/// One line per build, oldest first: when, which task and version, who
/// built it, and what the read-order check found.
#[must_use]
pub fn render_runs(runs: &[(Value, Vec<Value>)]) -> String {
    if runs.is_empty() {
        return "no builds recorded — run `superx specs ingest`\n".to_string();
    }
    let mut out = String::new();
    for (run, findings) in runs {
        let at = store::get_datetime(run, "started_at").map_or_else(|| "?".to_string(), |d| d.format("%Y-%m-%d %H:%M").to_string());
        let key = session::get_str(run, "key").unwrap_or_else(|| "?".to_string());
        let version = session::get_int(run, "version").map_or_else(|| "?".to_string(), |v| v.to_string());
        let who = session::get_str(run, "label")
            .or_else(|| session::get_str(run, "model"))
            .unwrap_or_else(|| "?".to_string());
        let sess: String = session::get_str(run, "session_key").unwrap_or_default().chars().take(8).collect();
        let _ = writeln!(out, "{at}  {key} v{version}  {who}  session {sess}");
        for f in findings {
            let _ = writeln!(
                out,
                "    {}: {} — {}",
                session::get_str(f, "check").unwrap_or_default(),
                session::get_str(f, "result").unwrap_or_default(),
                session::get_str(f, "note").unwrap_or_default()
            );
        }
    }
    out
}
