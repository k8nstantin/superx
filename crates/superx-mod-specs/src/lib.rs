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
//!   own UI         the ledger as a page, on `attr_specs_ui_port`, its URL
//!                  published as `attr_module_ui_url`
//!   own CLI        `superx specs ingest`, `superx specs crawl`, `superx specs runs`,
//!                  `superx specs ledger [KEY]`
//!
//! Nothing else in the workspace names this crate except the binary's
//! link line: removing it removes the module and changes nothing else.

pub mod crawl;
pub mod gates;
pub mod git;
pub mod ingest;
pub mod ledger;
pub mod ownership;
pub mod read_order;
pub mod repo;
pub mod server;
pub mod session;
pub mod spec;
pub mod store;

use std::fmt::Write as _;

use async_trait::async_trait;
use superx_kernel::types::Value;
use superx_kernel::{Kernel, KernelError, KernelModule, KernelModuleDescriptor, NodeKind, Result, KERNEL_MODULES};

/// Port parameter for this module's OWN service, on its own registry
/// entity.
pub const UI_PORT_PARAM: &str = "attr_specs_ui_port";

/// Where it listens when the parameter is unset.
pub const DEFAULT_UI_PORT: u16 = 5152; // skill-allow: §9-const — bootstrap fallback, param-overridable

/// The convention every module with a UI follows (D-UI2): publish the URL
/// on its own registry entity, so the core dashboard finds it in the
/// substrate and never through code.
pub const MODULE_UI_URL_PARAM: &str = "attr_module_ui_url";

/// The core dashboard's registry name and port parameter: SUBSTRATE
/// COORDINATES, not a dependency — this crate imports no other module.
const CORE_UI_MODULE: &str = "ui";
const CORE_UI_PORT_PARAM: &str = "attr_ui_port";

/// Where the core dashboard listens when its port parameter is unset.
const CORE_UI_DEFAULT_PORT: u16 = 5150; // skill-allow: §9-const — bootstrap fallback, param-overridable

fn port_of(v: Option<Value>) -> Option<u16> {
    match v {
        Some(Value::Number(n)) => n.to_int().and_then(|i| u16::try_from(i).ok()).filter(|&p| p > 0),
        _ => None,
    }
}

/// This module's port: the parameter on its registry entity, else the
/// fallback.
pub async fn resolved_ui_port(kernel: &Kernel) -> u16 {
    let Ok(Some(entity)) = kernel.find_module_by_name(NodeKind::KernelModule, MODULE_NAME).await else {
        return DEFAULT_UI_PORT;
    };
    port_of(kernel.get_parameter(entity, UI_PORT_PARAM).await.ok().flatten()).unwrap_or(DEFAULT_UI_PORT)
}

/// Where the core dashboard lives, for the page's way back: the URL it
/// publishes, else its port.
pub async fn core_dashboard_url(kernel: &Kernel) -> Option<String> {
    let entity = kernel.find_module_by_name(NodeKind::KernelModule, CORE_UI_MODULE).await.ok().flatten()?;
    if let Ok(Some(Value::String(url))) = kernel.get_parameter(entity.clone(), MODULE_UI_URL_PARAM).await {
        return Some(url);
    }
    let port = port_of(kernel.get_parameter(entity, CORE_UI_PORT_PARAM).await.ok().flatten()).unwrap_or(CORE_UI_DEFAULT_PORT);
    Some(format!("http://127.0.0.1:{port}"))
}

/// The module's name — db name, CLI namespace, log target.
pub const MODULE_NAME: &str = "specs";

/// The module's own schema, applied by `superx modules provision specs`
/// into `superx/specs` (kernel schema untouched — §7).
pub const SCHEMA_DDL: &str = include_str!("../schema/specs.surql");

const USAGE: &str = "usage: superx specs ingest | superx specs crawl | superx specs runs | superx specs ledger [KEY]";

/// How often the module refreshes itself — an ingest, then a crawl —
/// while the OS runs, in seconds; 0 is never.
pub const REFRESH_SECS_PARAM: &str = "attr_specs_refresh_secs";

/// Where it stands when the parameter is unset.
pub const DEFAULT_REFRESH_SECS: u64 = 900; // skill-allow: §9-const — bootstrap fallback, param-overridable

/// The refresh interval: the parameter on this module's registry entity,
/// else the fallback.
pub async fn resolved_refresh_secs(kernel: &Kernel) -> u64 {
    let Ok(Some(entity)) = kernel.find_module_by_name(NodeKind::KernelModule, MODULE_NAME).await else {
        return DEFAULT_REFRESH_SECS;
    };
    match kernel.get_parameter(entity, REFRESH_SECS_PARAM).await.ok().flatten() {
        Some(Value::Number(n)) => n.to_int().and_then(|i| u64::try_from(i).ok()).unwrap_or(DEFAULT_REFRESH_SECS),
        _ => DEFAULT_REFRESH_SECS,
    }
}

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
        // not fail the boot walk. No ingest runs at boot: one is asked for.
        let db = match kernel.module_db(MODULE_NAME).await {
            Ok(db) => db,
            Err(e) => {
                tracing::warn!(target: "specs", "own db unavailable — module idle until provisioned: {e}");
                return Ok(());
            }
        };
        // The ledger as a page, on this module's own port, its URL
        // published where the core dashboard looks for module UIs (D-UI2).
        let port = resolved_ui_port(kernel).await;
        let db_for_refresh = db.clone();
        server::spawn(kernel, db, port).await?;
        let url = format!("http://127.0.0.1:{port}");
        if let Ok(Some(entity)) = kernel.find_module_by_name(NodeKind::KernelModule, MODULE_NAME).await {
            kernel.set_parameter(entity, MODULE_UI_URL_PARAM, Value::String(url.clone())).await?;
        }
        tracing::info!(target: "specs", %url, "ledger page listening");

        // Keep the ledger current while the OS runs: an ingest, then a
        // read-only crawl of the builds' repositories, every so often. The
        // kernel's stop token ends it with the module.
        let every = resolved_refresh_secs(kernel).await;
        if every > 0 {
            let (kernel, db, stop) = (kernel.clone(), db_for_refresh, kernel.module_token(MODULE_NAME));
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        () = stop.clone().cancelled() => break,
                        () = tokio::time::sleep(std::time::Duration::from_secs(every)) => {}
                    }
                    if let Err(e) = ingest::ingest(&kernel, &db).await {
                        tracing::warn!(target: "specs", error = %e, "refresh: ingest failed");
                        continue;
                    }
                    if let Err(e) = crawl::crawl(&kernel, &db).await {
                        tracing::warn!(target: "specs", error = %e, "refresh: crawl failed");
                    }
                }
            });
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
                    ("reverts_unresolved", r.reverts_unresolved),
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
                    [
                        (r.cut_short, "judged on their first rows only"),
                        (r.reverts_unresolved, "reverts whose merged branch no session printed"),
                    ]
                    .iter()
                    .filter(|(n, _)| *n > 0)
                    .map(|(n, what)| format!(", {n} {what}"))
                    .collect::<String>()
                ))
            }
            Some("crawl") => {
                let db = kernel.module_db(MODULE_NAME).await?;
                let r = crawl::crawl(kernel, &db).await?;
                Ok(format!(
                    "{} repositories read (read-only, as last fetched): {} build branches, {} changed, {} unchanged{}\n",
                    r.repos,
                    r.branches,
                    r.written,
                    r.unchanged,
                    if r.unreadable > 0 { format!(", {} builds whose directory is no repository", r.unreadable) } else { String::new() }
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
