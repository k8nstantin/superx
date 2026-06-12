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

    /// Show recent telemetry (newest first), or follow it live.
    Stats {
        /// How many events to show (initial backlog in --live mode).
        // skill-allow: §9-default — render-size knob for a one-shot read; the
        // live-poll interval is the substrate parameter
        #[arg(short = 'n', long, default_value_t = 25)]
        limit: u32,

        /// Follow the firehose: render new events as they land
        /// (rolling poll; interval = attr_stats_poll_interval_secs).
        #[arg(long)]
        live: bool,

        /// Only show events whose payload mentions this module /
        /// agent name.
        #[arg(long)]
        module: Option<String>,

        /// Emit one plain JSON object per event (for piping into jq
        /// etc.) instead of the human-readable key=value rendering.
        #[arg(long)]
        json: bool,
    },

    /// List discovered agents and their capture sources.
    Agents,
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
/// newest first, optionally filtered by `--module`.
///
/// # Errors
///
/// Substrate errors.
pub async fn run_stats(
    kernel: &Kernel,
    limit: u32,
    module: Option<&str>,
    format: RenderFormat,
) -> Result<String> {
    let events = kernel.recent_telemetry(limit).await?;
    let mut out = String::new();
    let mut shown = 0usize;
    for e in &events {
        let Some(line) = render_event(e, module, format) else { continue };
        out.push_str(&line);
        shown += 1;
    }
    out.push_str(&format!("\n{shown} event(s)\n"));
    Ok(out)
}

/// Rolling state for the `--live` tail.
///
/// `valid_from` timestamps are assigned client-side by each writer
/// *before* its insert commits, so under concurrent writers (the
/// capture loop + lifecycle emitters — exactly the FVP topology) a
/// strictly-greater watermark could permanently skip an event whose
/// commit landed after a later-stamped event was already rendered.
/// The tail therefore re-reads an **overlap window** behind the
/// watermark each tick and dedupes by UUIDv7 row id; the seen-set is
/// pruned to the window so memory stays bounded.
#[derive(Debug)]
pub struct LiveTail {
    high_water: chrono::DateTime<chrono::Utc>,
    overlap: chrono::Duration,
    format: RenderFormat,
    /// Row ids already rendered, with their valid_from for pruning.
    seen: std::collections::HashMap<String, chrono::DateTime<chrono::Utc>>,
}

impl LiveTail {
    /// Start a tail. `after` is the newest already-rendered event
    /// timestamp (or `MIN_UTC` to replay everything); `overlap` is
    /// how far behind the watermark each poll re-reads —
    /// conventionally 2× the poll interval.
    #[must_use]
    pub fn new(
        after: chrono::DateTime<chrono::Utc>,
        overlap: chrono::Duration,
        format: RenderFormat,
    ) -> Self {
        Self {
            high_water: after,
            overlap,
            format,
            seen: std::collections::HashMap::new(),
        }
    }

    /// One poll: render every not-yet-seen event since
    /// `high_water − overlap`, oldest first, and advance the
    /// watermark. Pulled out of the timer loop so tests drive ticks
    /// directly.
    ///
    /// # Errors
    ///
    /// Substrate errors.
    pub async fn tick(&mut self, kernel: &Kernel, module: Option<&str>) -> Result<String> {
        let from = self
            .high_water
            .checked_sub_signed(self.overlap)
            .unwrap_or(self.high_water);
        let events = kernel.telemetry_since(from, LIVE_PAGE_SIZE).await?;
        let mut out = String::new();
        for e in &events {
            // UUIDv7 row id, rendered via Debug (RecordId has no
            // Display impl) — uniqueness is what matters for dedup.
            let key = format!("{:?}", e.id);
            if self.seen.contains_key(&key) {
                continue;
            }
            self.seen.insert(key, e.valid_from);
            if e.valid_from > self.high_water {
                self.high_water = e.valid_from;
            }
            if let Some(line) = render_event(e, module, self.format) {
                out.push_str(&line);
            }
        }
        // Prune the seen-set to the overlap window — anything older
        // can never be returned by a future poll.
        let horizon = self
            .high_water
            .checked_sub_signed(self.overlap)
            .unwrap_or(self.high_water);
        self.seen.retain(|_, ts| *ts >= horizon);
        Ok(out)
    }

    /// The newest rendered event timestamp.
    #[must_use]
    pub fn high_water(&self) -> chrono::DateTime<chrono::Utc> {
        self.high_water
    }
}

/// Render the `--live` backlog (newest-first input, oldest-first
/// output) and return the rendering plus the high-water timestamp the
/// follow loop should start from. The interactive print loop itself
/// lives in `main.rs` (library code stays print-free; binaries own
/// the terminal).
///
/// # Errors
///
/// Substrate errors.
pub async fn live_backlog(
    kernel: &Kernel,
    limit: u32,
    module: Option<&str>,
    format: RenderFormat,
) -> Result<(String, chrono::DateTime<chrono::Utc>)> {
    let mut backlog = kernel.recent_telemetry(limit).await?;
    backlog.reverse();
    let mut out = String::new();
    let mut high_water = chrono::DateTime::<chrono::Utc>::MIN_UTC;
    for e in &backlog {
        if e.valid_from > high_water {
            high_water = e.valid_from;
        }
        if let Some(line) = render_event(e, module, format) {
            out.push_str(&line);
        }
    }
    Ok((out, high_water))
}

/// True when the substrate has never been bootstrapped — callers
/// render the bootstrap hint instead of erroring.
///
/// # Errors
///
/// Substrate errors.
pub async fn substrate_is_bare(kernel: &Kernel) -> Result<bool> {
    Ok(kernel.find_type_opt("node_kernel_module").await?.is_none())
}

/// `superx kernel agents` — discovered agents with their capture
/// sources.
///
/// # Errors
///
/// Substrate errors. An unseeded metamodel renders the bootstrap
/// hint.
pub async fn run_agents(kernel: &Kernel) -> Result<String> {
    if kernel.find_type_opt("node_agent").await?.is_none()
        || kernel.find_type_opt("attr_agent_descriptor").await?.is_none()
    {
        return Ok(
            "substrate has no registry yet — run `superx kernel bootstrap` first\n".to_string(),
        );
    }
    let agents = kernel
        .list_named_entities("node_agent", "attr_agent_descriptor")
        .await?;
    let sources = kernel
        .list_named_entities("node_source", "attr_source_descriptor")
        .await?;

    let mut out = String::new();
    for agent in &agents {
        let Some(name) = payload_str(&agent.payload, "name") else { continue };
        let probe = payload_str(&agent.payload, "probe").unwrap_or_else(|| "-".to_string());
        let agent_sources: Vec<String> = sources
            .iter()
            .filter(|s| payload_str(&s.payload, "agent").as_deref() == Some(&name))
            .filter_map(|s| payload_str(&s.payload, "locator"))
            .collect();
        out.push_str(&format!(
            "{name}  (probe: {probe}, {} source(s))\n",
            agent_sources.len(),
        ));
        for locator in agent_sources {
            out.push_str(&format!("    {locator}\n"));
        }
    }
    out.push_str(&format!("\n{} agent(s)\n", agents.len()));
    Ok(out)
}

/// Events per `--live` tick; anything beyond arrives next tick.
// skill-allow: §9-const — render page bound, not behavior policy
const LIVE_PAGE_SIZE: u32 = 500;

/// Poll-interval parameter for `--live`, on the CLI app's registry
/// entity.
pub const STATS_POLL_PARAM: &str = "attr_stats_poll_interval_secs";

/// Interval seed when the parameter is unset — written to the
/// substrate on first use so the effective value is recorded state.
// skill-allow: §9-const — seed value; recorded as a substrate parameter on first use
const DEFAULT_STATS_POLL_SECS: i64 = 1;

/// Resolve the `--live` poll interval (seconds), seeding the default
/// into the substrate on first use. Public for the binary's follow
/// loop.
///
/// # Errors
///
/// Substrate errors.
pub async fn live_poll_interval_secs(kernel: &Kernel) -> Result<u64> {
    use superx_kernel::types::Value;
    let Some(status) = kernel.detailed_status(NodeKind::Contribution, "app_cli").await? else {
        return Ok(u64::try_from(DEFAULT_STATS_POLL_SECS).unwrap_or(1));
    };
    if let Some(Value::Number(n)) = kernel
        .get_parameter(status.entity_id.clone(), STATS_POLL_PARAM)
        .await?
    {
        let secs = n.to_int().unwrap_or(DEFAULT_STATS_POLL_SECS);
        return Ok(u64::try_from(secs).unwrap_or(1).max(1));
    }
    kernel
        .set_parameter(
            status.entity_id,
            STATS_POLL_PARAM,
            Value::Number(DEFAULT_STATS_POLL_SECS.into()),
        )
        .await?;
    Ok(u64::try_from(DEFAULT_STATS_POLL_SECS).unwrap_or(1))
}

/// How `stats` renders each event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RenderFormat {
    /// `TS  EVENT  key=value key=value` — the operator-facing default.
    #[default]
    Human,
    /// One plain (untagged) JSON object per line — for jq pipelines.
    Json,
}

/// Convert a substrate [`Value`](superx_kernel::types::Value) into
/// plain (untagged) `serde_json::Value` — `{"agent": "claude_code"}`
/// instead of serde's externally-tagged
/// `{"Object":{"agent":{"String":"claude_code"}}}`.
fn value_to_plain(v: &superx_kernel::types::Value) -> serde_json::Value {
    use superx_kernel::types::Value as V;
    match v {
        V::None | V::Null => serde_json::Value::Null,
        V::Bool(b) => serde_json::Value::Bool(*b),
        V::Number(n) => n
            .to_int()
            .map(serde_json::Value::from)
            .unwrap_or_else(|| serde_json::Value::String(format!("{n:?}"))),
        V::String(s) => serde_json::Value::String(s.clone()),
        V::Array(a) => serde_json::Value::Array(a.iter().map(value_to_plain).collect()),
        V::Object(o) => serde_json::Value::Object(
            o.iter().map(|(k, v)| (k.clone(), value_to_plain(v))).collect(),
        ),
        other => serde_json::Value::String(format!("{other:?}")),
    }
}

/// Render one event line in the requested format, applying the
/// `--module` substring filter against the event name and the plain
/// payload text. `None` = filtered out.
fn render_event(
    e: &superx_kernel::TelemetryRecord,
    module: Option<&str>,
    format: RenderFormat,
) -> Option<String> {
    let plain = value_to_plain(&e.payload);
    let plain_text = plain.to_string();
    if let Some(needle) = module {
        if !e.lifecycle_event.contains(needle) && !plain_text.contains(needle) {
            return None;
        }
    }
    match format {
        RenderFormat::Json => {
            let line = serde_json::json!({
                "ts": e.valid_from.to_rfc3339(),
                "event": e.lifecycle_event,
                "payload": plain,
            });
            Some(format!("{line}\n"))
        }
        RenderFormat::Human => {
            let fields = match &plain {
                serde_json::Value::Object(map) => map
                    .iter()
                    .map(|(k, v)| match v {
                        serde_json::Value::String(s) => format!("{k}={s}"),
                        other => format!("{k}={other}"),
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
                other => other.to_string(),
            };
            Some(format!(
                "{}  {:<24} {fields}\n",
                e.valid_from.format("%Y-%m-%d %H:%M:%S%.3f"),
                e.lifecycle_event,
            ))
        }
    }
}

fn payload_str(payload: &superx_kernel::types::Value, key: &str) -> Option<String> {
    match payload {
        superx_kernel::types::Value::Object(o) => match o.get(key) {
            Some(superx_kernel::types::Value::String(s)) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
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
