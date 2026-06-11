//! SuperX operator CLI — the `superx` binary's argument surface and
//! command handlers.
//!
//! CLI pattern (locked canon §3): `superx <module> <action>`. Core
//! kernel operations live under `superx kernel <action>` — they ARE
//! the kernel's actions, not flat top-level commands. Future modules
//! and apps add their own subcommands (`superx gmaster infer …`).
//!
//! Handlers take `&Kernel` and return rendered text, so tests drive
//! them against an in-memory substrate without a wire connection;
//! `main.rs` is a thin shell that connects and prints.
//!
//! The CLI is itself an L3 app: it registers in [`KERNEL_MODULES`]
//! like every other contribution and shows up in
//! `superx kernel modules list`.

use clap::{Args, Parser, Subcommand};
use linkme::distributed_slice;
use superx_kernel::{
    Kernel, KernelModule, KernelModuleDescriptor, LifecycleState, NodeKind, Result,
    KERNEL_MODULES,
};
use superx_kernel_bootstrap::{bootstrap, BootReport};

// Link the FVP contribution set into every binary built on this
// library (the `superx` bin and the test binaries): their linkme
// registrations fill KERNEL_MODULES / DISCOVERY_PROBES /
// CAPTURE_SOURCES at link time. No other coupling.
use superx_driver_claude_code as _;
use superx_kernel_capture as _;
use superx_kernel_discovery as _;

// ─────────────────────────────────────────────────────────────────────
// App registration
// ─────────────────────────────────────────────────────────────────────

/// The CLI's registry presence. Its `startup()` is a no-op — the
/// binary's work happens in the command handlers, not at boot.
pub struct CliApp;

#[async_trait::async_trait]
impl KernelModule for CliApp {
    fn descriptor(&self) -> KernelModuleDescriptor {
        KernelModuleDescriptor {
            name: "app_cli",
            version: env!("CARGO_PKG_VERSION"),
            kind: NodeKind::Contribution,
            category: "app",
            depends_on: &[],
            required_metamodel: &[],
        }
    }

    async fn startup(&self, _kernel: &Kernel) -> Result<()> {
        Ok(())
    }
}

#[distributed_slice(KERNEL_MODULES)]
static CLI_REGISTRATION: &'static (dyn KernelModule + Sync) = &CliApp;

// ─────────────────────────────────────────────────────────────────────
// Argument surface
// ─────────────────────────────────────────────────────────────────────

/// `superx` — the SuperX operator CLI.
#[derive(Debug, Parser)]
#[command(name = "superx", version, about = "SuperX agentic OS — operator CLI")]
pub struct Cli {
    #[command(flatten)]
    pub connection: ConnectionArgs,

    #[command(subcommand)]
    pub command: Command,
}

/// Where the substrate lives. Every value can come from a flag or an
/// env var; defaults match the operator quickstart in README.md.
///
/// These three carry `skill-allow: §9-default` markers: connection
/// bootstrap parameters are the one legitimate hardcoded-default
/// exception — they locate the substrate and therefore cannot be
/// read *from* it. Everything downstream of the connection is an
/// `attr_config` parameter per §9.
#[derive(Debug, Args)]
pub struct ConnectionArgs {
    /// SurrealDB connection URL.
    // skill-allow: §9-default — locates the substrate; cannot be read from it
    #[arg(long, env = "SUPERX_ENDPOINT", default_value = "ws://127.0.0.1:8000")]
    pub endpoint: String,

    /// SurrealDB namespace.
    // skill-allow: §9-default — locates the substrate; cannot be read from it
    #[arg(long, env = "SUPERX_NAMESPACE", default_value = "superx")]
    pub namespace: String,

    /// SurrealDB database.
    // skill-allow: §9-default — locates the substrate; cannot be read from it
    #[arg(long, env = "SUPERX_DATABASE", default_value = "kernel")]
    pub database: String,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Core kernel operations.
    #[command(subcommand)]
    Kernel(KernelCommand),
}

#[derive(Debug, Subcommand)]
pub enum KernelCommand {
    /// Provision + seed + register + start every enabled module.
    Bootstrap,

    /// Registry inspection.
    #[command(subcommand)]
    Modules(ModulesCommand),

    /// Show recent telemetry (newest first).
    Stats {
        /// How many events to show.
        #[arg(short = 'n', long, default_value_t = 25)]
        limit: u32,
    },
}

#[derive(Debug, Subcommand)]
pub enum ModulesCommand {
    /// One-line summary of every registered module / driver / app.
    List,
}

// ─────────────────────────────────────────────────────────────────────
// Command handlers — take &Kernel, return rendered text
// ─────────────────────────────────────────────────────────────────────

/// `superx kernel bootstrap` — run the boot orchestrator. Returns the
/// report (so the binary can decide whether to stay foreground for
/// capture) together with its rendering.
///
/// # Errors
///
/// Substrate-level failures of the boot machinery itself; per-module
/// problems are isolated into the rendered report.
pub async fn run_bootstrap(kernel: &Kernel) -> Result<(BootReport, String)> {
    let report = bootstrap(kernel).await?;
    let rendered = render_boot_report(&report);
    Ok((report, rendered))
}

/// True when the capture module booted Active — the binary stays
/// foreground so the capture loop keeps streaming (D6).
#[must_use]
pub fn capture_is_active(report: &BootReport) -> bool {
    report.entries.iter().any(|e| {
        e.name == superx_kernel_capture::MODULE_NAME
            && matches!(e.outcome, LifecycleState::Active { .. })
    })
}

/// `superx kernel modules list` — render the registry.
///
/// # Errors
///
/// Substrate errors. An unseeded metamodel (fresh substrate, never
/// bootstrapped) renders a hint instead of erroring.
pub async fn run_modules_list(kernel: &Kernel) -> Result<String> {
    if kernel.find_type_opt("node_kernel_module").await?.is_none() {
        return Ok(
            "substrate has no registry yet — run `superx kernel bootstrap` first\n".to_string(),
        );
    }
    let mut rows = kernel.list_with_status(NodeKind::KernelModule).await?;
    rows.extend(kernel.list_with_status(NodeKind::Contribution).await?);

    let mut out = String::new();
    out.push_str(&format!(
        "{:<24} {:<14} {:<10} {:<9} {}\n",
        "NAME", "KIND", "CATEGORY", "VERSION", "STATE"
    ));
    for row in &rows {
        out.push_str(&format!(
            "{:<24} {:<14} {:<10} {:<9} {}\n",
            row.name,
            kind_label(row.kind),
            if row.category.is_empty() { "-" } else { &row.category },
            row.version,
            lifecycle_label(&row.lifecycle),
        ));
    }
    out.push_str(&format!("\n{} registered\n", rows.len()));
    Ok(out)
}

/// `superx kernel stats -n <limit>` — render recent telemetry,
/// newest first.
///
/// # Errors
///
/// Substrate errors.
pub async fn run_stats(kernel: &Kernel, limit: u32) -> Result<String> {
    let events = kernel.recent_telemetry(limit).await?;
    let mut out = String::new();
    for e in &events {
        out.push_str(&format!(
            "{}  {:<24} {}\n",
            e.valid_from.format("%Y-%m-%d %H:%M:%S%.3f"),
            e.lifecycle_event,
            serde_json::to_string(&e.payload)
                .unwrap_or_else(|_| format!("{:?}", e.payload)),
        ));
    }
    out.push_str(&format!("\n{} event(s)\n", events.len()));
    Ok(out)
}

// ─────────────────────────────────────────────────────────────────────
// Rendering
// ─────────────────────────────────────────────────────────────────────

/// Render a [`BootReport`] as the operator-facing boot table.
#[must_use]
pub fn render_boot_report(report: &BootReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{:<24} {:<14} {:<9} {}\n",
        "NAME", "KIND", "VERSION", "OUTCOME"
    ));
    for entry in &report.entries {
        out.push_str(&format!(
            "{:<24} {:<14} {:<9} {}\n",
            entry.name,
            kind_label(entry.kind),
            entry.version,
            lifecycle_label(&entry.outcome),
        ));
    }
    out.push_str(&format!(
        "\n{} active / {} total\n",
        report.active_count(),
        report.entries.len(),
    ));
    out
}

fn kind_label(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::KernelModule => "kernel-module",
        NodeKind::Contribution => "contribution",
    }
}

fn lifecycle_label(state: &LifecycleState) -> String {
    match state {
        LifecycleState::Active { startup_duration_ms } => {
            format!("active ({startup_duration_ms} ms)")
        }
        LifecycleState::Failed { error, failed_during } => {
            format!("FAILED during {failed_during}: {error}")
        }
        LifecycleState::Skipped { reason } => format!("skipped: {reason}"),
        other => other.short_tag().to_string(),
    }
}
