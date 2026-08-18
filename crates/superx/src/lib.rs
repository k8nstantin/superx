//! `superx` — operator CLI internals (BLUEPRINT.md G6).
//!
//! The binary in `main.rs` is a thin dispatcher; everything testable
//! lives here: command runners, session resolution, and the
//! human-readable renderers.

use std::io::Write as _;

use chrono::{DateTime, Utc};
use superx_kernel::types::Value;
use superx_kernel::{Kernel, MessageRecord, NodeKind, TelemetryRecord};

pub mod initialize;

pub use clap::Parser;

/// Poll cadence for the `--live` tails, on the `kernel` registry
/// entity.
pub const LIVE_POLL_PARAM: &str = "attr_cli_live_poll_secs";
const DEFAULT_LIVE_POLL_SECS: u64 = 1; // skill-allow: §9-const — bootstrap fallback, param-overridable

/// Page size for historical reads before a live tail begins.
const READ_BACKLOG_LIMIT: u32 = 10_000; // skill-allow: §9-const — render page bound

#[derive(Debug, Parser)]
#[command(
    name = "superx",
    version,
    about = "SuperX — the agentic OS. Boots, discovers coding agents, captures their telemetry and conversations."
)]
pub struct Cli {
    /// Provision this instance end-to-end (prompt for the root
    /// password, create the database + schema, initialize everything)
    /// and start gathering data. Idempotent: an initialized instance
    /// boots straight away.
    #[arg(long, global = true)]
    pub initialize: bool,
    /// Where the instance's datastore lives.
    #[arg(long, global = true, env = "SUPERX_DATA_DIR", default_value = "./db/superx-v2.db")] // skill-allow: §9-default — env-overridable
    pub data_dir: std::path::PathBuf,
    #[command(flatten)]
    pub conn: ConnectionArgs,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, clap::Args)]
pub struct ConnectionArgs {
    /// SurrealDB connection URL.
    #[arg(long, global = true, env = "SUPERX_ENDPOINT", default_value = "ws://127.0.0.1:8000")] // skill-allow: §9-default — env-overridable
    pub endpoint: String,
    /// SurrealDB namespace.
    #[arg(long, global = true, env = "SUPERX_NAMESPACE", default_value = "superx")] // skill-allow: §9-default — env-overridable
    pub namespace: String,
    /// SurrealDB database.
    #[arg(long, global = true, env = "SUPERX_DATABASE", default_value = "kernel")] // skill-allow: §9-default — env-overridable
    pub database: String,
}

#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// Boot the OS in the foreground (debugging): register modules,
    /// discover agents, run capture until ctrl-c. Normal operation
    /// uses `--initialize`, which runs the OS in the background.
    Boot {
        /// Internal: this process IS the background OS (pidfile owned
        /// by the parent). Skips the duplicate-capture guard.
        #[arg(long, hide = true)]
        daemonized: bool,
    },
    /// Stop the background OS started by --initialize.
    Stop,
    /// Module + adapter lifecycle status.
    Status,
    /// Discovered agents with their session and source counts.
    Agents,
    /// The action stream (telemetry), newest first.
    Actions {
        /// How many recent events to show.
        #[arg(short = 'n', long, default_value_t = 20)]
        // skill-allow: §9-default — render page size, flag-overridable
        limit: u32,
        /// Only events concerning this agent (name, e.g. claude_code).
        #[arg(long)]
        agent: Option<String>,
        /// Keep tailing new events as they arrive.
        #[arg(long)]
        live: bool,
    },
    /// List captured conversations.
    Sessions {
        /// Only sessions of this agent (name, e.g. gemini_cli).
        #[arg(long)]
        agent: Option<String>,
    },
    /// Render one conversation, oldest first.
    Read {
        /// Session name (`claude_code/<id>`) or any unique fragment
        /// of one (id prefix works).
        session: String,
        /// Keep following the conversation as it continues.
        #[arg(long)]
        live: bool,
    },
}

/// Connect + signin with an actionable hint on auth refusal. The
/// password comes from `SUPERX_KERNEL_PASSWORD` or the instance
/// credentials file written by `--initialize`.
pub async fn connect(conn: &ConnectionArgs, data_dir: &std::path::Path) -> Result<Kernel, String> {
    let Some(password) = initialize::resolve_password(data_dir) else {
        return Err(format!(
            "no credentials: export {} or run `superx --initialize` first",
            initialize::PASSWORD_ENV
        ));
    };
    superx_kernel::Kernel::connect_service_with_password(
        &conn.endpoint,
        &conn.namespace,
        &conn.database,
        &password,
    )
        .await
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("authentication") || msg.contains("credentials") {
                format!(
                    "{msg}\nhint: the password must match the one used at initialize/deploy \
                     time — re-run `superx --initialize`, or export SUPERX_KERNEL_PASSWORD, \
                     and confirm the server at {} is this instance's",
                    conn.endpoint
                )
            } else {
                msg
            }
        })
}

/// `superx boot` body: boot, render the report, start the capture
/// loop, block until ctrl-c (or SIGINT from `superx stop` when
/// daemonized).
pub async fn run_boot(kernel: &Kernel) -> Result<(), String> {
    let report = superx_kernel::boot(kernel).await.map_err(|e| e.to_string())?;
    emit(&render_boot_report(&report));

    let (tx, rx) = tokio::sync::watch::channel(false);
    let loop_kernel = kernel.clone();
    let capture = tokio::spawn(async move {
        superx_kernel::capture::run_loop(&loop_kernel, rx).await
    });
    emit("capture loop running — ctrl-c to shut down\n");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| format!("signal handler: {e}"))?;
    tracing::info!("ctrl-c received; shutting down");
    if tx.send(true).is_err() {
        tracing::warn!("capture loop already gone at shutdown signal");
    }
    match capture.await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.to_string()),
        Err(e) => Err(format!("capture task: {e}")),
    }
}

/// `superx stop` body: SIGINT the background OS, wait for exit.
pub async fn run_stop(data_dir: &std::path::Path) -> Result<String, String> {
    let Some(pid) = initialize::read_live_pid(data_dir) else {
        return Ok("OS is not running\n".to_string());
    };
    let ok = std::process::Command::new("kill")
        .arg("-INT")
        .arg(pid.to_string())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        return Err(format!("could not signal pid {pid}"));
    }
    // Graceful shutdown: the loop stops between source polls, so this
    // normally lands in seconds even mid-backfill.
    for i in 0..300 {
        if !initialize::pid_alive(pid) {
            let _removed = std::fs::remove_file(initialize::pid_path(data_dir));
            return Ok(format!("OS stopped (pid {pid})\n"));
        }
        if i == 50 {
            emit("waiting for the current source poll to finish…\n");
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await; // skill-allow: §9-duration — stop-poll cadence, bounded loop
    }
    Err(format!(
        "OS (pid {pid}) is still shutting down — check `superx status` shortly"
    ))
}

/// `superx status` body.
pub async fn run_status(kernel: &Kernel, data_dir: &std::path::Path) -> Result<String, String> {
    let mut out = String::new();
    match initialize::read_live_pid(data_dir) {
        Some(pid) => out.push_str(&format!("OS: running in background (pid {pid})\n")),
        None => out.push_str("OS: not running (`superx --initialize` starts it)\n"),
    }
    for (kind, title) in [
        (NodeKind::KernelModule, "kernel modules"),
        (NodeKind::Adapter, "adapters"),
    ] {
        let list = kernel
            .list_with_status(kind)
            .await
            .map_err(|e| e.to_string())?;
        out.push_str(&format!("{title}:\n"));
        if list.is_empty() {
            out.push_str("  (none registered — has the OS booted?)\n");
        }
        for s in list {
            out.push_str(&format!(
                "  {:<24} {:<9} v{}\n",
                s.name,
                s.lifecycle.short_tag(),
                s.version
            ));
        }
    }
    Ok(out)
}

/// `superx agents` body.
pub async fn run_agents(kernel: &Kernel) -> Result<String, String> {
    let agents = kernel
        .list_named_entities("node_agent", "attr_agent_descriptor")
        .await
        .map_err(|e| e.to_string())?;
    if agents.is_empty() {
        return Ok("no agents discovered yet — run `superx boot` first".to_string());
    }
    let sessions = kernel
        .list_named_entities("node_session", "attr_session_descriptor")
        .await
        .map_err(|e| e.to_string())?;
    let sources = kernel
        .list_named_entities("node_source", "attr_source_descriptor")
        .await
        .map_err(|e| e.to_string())?;

    let mut out = String::new();
    for a in agents {
        let name = payload_str(&a.payload, "name").unwrap_or_else(|| "?".into());
        let prefix = format!("{name}/");
        let n_sessions = sessions
            .iter()
            .filter(|s| payload_str(&s.payload, "name").is_some_and(|n| n.starts_with(&prefix)))
            .count();
        let n_sources = sources
            .iter()
            .filter(|s| payload_str(&s.payload, "name").is_some_and(|n| n.starts_with(&prefix)))
            .count();
        out.push_str(&format!(
            "{name:<16} agent_id={}  sources={n_sources}  sessions={n_sessions}\n",
            short_record_id(&a.entity_id)
        ));
    }
    Ok(out)
}

/// `superx sessions` body.
pub async fn run_sessions(kernel: &Kernel, agent: Option<&str>) -> Result<String, String> {
    let sessions = kernel
        .list_named_entities("node_session", "attr_session_descriptor")
        .await
        .map_err(|e| e.to_string())?;
    let mut out = String::new();
    let mut shown = 0;
    for s in sessions {
        let Some(name) = payload_str(&s.payload, "name") else {
            continue;
        };
        if let Some(agent) = agent {
            if !name.starts_with(&format!("{agent}/")) {
                continue;
            }
        }
        let count = kernel
            .session_message_count(s.entity_id.clone())
            .await
            .map_err(|e| e.to_string())?;
        let agent_prefix = name.split('/').next().unwrap_or("?");
        let source_id = name.split_once('/').map_or("", |(_, rest)| rest);
        out.push_str(&format!(
            "{agent_prefix}/{:<40} src={source_id:<40} {count:>6} messages\n",
            record_uuid(&s.entity_id)
        ));
        shown += 1;
    }
    if shown == 0 {
        out.push_str("no sessions captured yet\n");
    }
    Ok(out)
}

/// Resolve a session by exact name, or by any UNIQUE fragment of one
/// (prefix, suffix, or middle — users paste whatever part of the id
/// they have; issue #122).
pub async fn resolve_session(
    kernel: &Kernel,
    query: &str,
) -> Result<superx_kernel::types::RecordId, String> {
    let sessions = kernel
        .list_named_entities("node_session", "attr_session_descriptor")
        .await
        .map_err(|e| e.to_string())?;
    let matches: Vec<_> = sessions
        .iter()
        .filter(|s| {
            let Some(name) = payload_str(&s.payload, "name") else {
                return false;
            };
            let agent_prefix = name.split('/').next().unwrap_or("?");
            let identity = format!("{agent_prefix}/{}", record_uuid(&s.entity_id));
            name == query
                || identity == query
                || name.contains(query)
                || identity.contains(query)
        })
        .collect();
    match matches.len() {
        0 => Err(format!("no session matches '{query}' — try `superx sessions`")),
        1 => Ok(matches[0].entity_id.clone()),
        n => Err(format!(
            "'{query}' is ambiguous ({n} sessions match) — use the full name from `superx sessions`"
        )),
    }
}

/// `superx read` body (historical part). Returns the rendered
/// backlog and the timestamp high-water mark for a live tail.
pub async fn run_read(
    kernel: &Kernel,
    session: superx_kernel::types::RecordId,
) -> Result<(String, Option<DateTime<Utc>>), String> {
    let messages = kernel
        .session_messages(session, READ_BACKLOG_LIMIT)
        .await
        .map_err(|e| e.to_string())?;
    let high_water = messages.last().map(|m| m.valid_from);
    let mut out = String::new();
    for m in &messages {
        out.push_str(&render_message(m));
    }
    if messages.is_empty() {
        out.push_str("(no messages captured for this session yet)\n");
    }
    Ok((out, high_water))
}

/// One live-tail pass for `read --live`. Returns rendered new
/// messages + the advanced high-water mark.
pub async fn read_tail_tick(
    kernel: &Kernel,
    session: superx_kernel::types::RecordId,
    after: DateTime<Utc>,
) -> Result<(String, DateTime<Utc>), String> {
    let messages = kernel
        .session_messages_since(session, after, READ_BACKLOG_LIMIT)
        .await
        .map_err(|e| e.to_string())?;
    let high = messages.last().map(|m| m.valid_from).unwrap_or(after);
    let mut out = String::new();
    for m in &messages {
        out.push_str(&render_message(m));
    }
    Ok((out, high))
}

/// `superx actions` body (historical part): newest-first fetch,
/// rendered oldest-first for reading. Returns text + high-water mark.
pub async fn run_actions(
    kernel: &Kernel,
    limit: u32,
    agent: Option<&str>,
) -> Result<(String, Option<DateTime<Utc>>), String> {
    let mut events = match agent {
        Some(name) => {
            let id = kernel
                .find_entity_by_name("node_agent", "attr_agent_descriptor", name)
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("no agent named '{name}' — try `superx agents`"))?;
            kernel
                .agent_telemetry(id, limit)
                .await
                .map_err(|e| e.to_string())?
        }
        None => kernel
            .recent_telemetry(limit)
            .await
            .map_err(|e| e.to_string())?,
    };
    events.reverse(); // oldest first for reading
    let high_water = events.last().map(|e| e.valid_from);
    let mut out = String::new();
    for e in &events {
        out.push_str(&render_event(e));
    }
    if events.is_empty() {
        out.push_str("(no actions captured yet)\n");
    }
    Ok((out, high_water))
}

/// One live-tail pass for `actions --live`.
pub async fn actions_tail_tick(
    kernel: &Kernel,
    after: DateTime<Utc>,
    agent_filter: Option<&superx_kernel::types::RecordId>,
    limit: u32,
) -> Result<(String, DateTime<Utc>), String> {
    let events = kernel
        .telemetry_since(after, limit)
        .await
        .map_err(|e| e.to_string())?;
    let high = events.last().map(|e| e.valid_from).unwrap_or(after);
    let mut out = String::new();
    for e in &events {
        if let Some(want) = agent_filter {
            if e.agent.as_ref() != Some(want) {
                continue;
            }
        }
        out.push_str(&render_event(e));
    }
    Ok((out, high))
}

/// Live-tail poll cadence from the kernel parameter.
pub async fn live_poll_secs(kernel: &Kernel) -> u64 {
    let Ok(Some(entity)) = kernel
        .find_module_by_name(NodeKind::KernelModule, "kernel")
        .await
    else {
        return DEFAULT_LIVE_POLL_SECS;
    };
    match kernel.get_parameter(entity, LIVE_POLL_PARAM).await {
        Ok(Some(Value::Number(n))) => n
            .to_int()
            .and_then(|i| u64::try_from(i).ok())
            .filter(|&s| s > 0)
            .unwrap_or(DEFAULT_LIVE_POLL_SECS),
        _ => DEFAULT_LIVE_POLL_SECS,
    }
}

// ─────────────────────────────────────────────────────────────────────
// Renderers — human-readable, one line (or block) per record
// ─────────────────────────────────────────────────────────────────────

/// `[hh:mm:ss] role: content` — multiline content indented under the
/// role line; empty content shown as the raw kind marker.
#[must_use]
pub fn render_message(m: &MessageRecord) -> String {
    let ts = m
        .emitted_at
        .unwrap_or(m.valid_from)
        .format("%Y-%m-%d %H:%M:%S");
    let content = if m.content.is_empty() {
        "(no text — see raw)".to_string()
    } else {
        m.content.replace('\n', "\n    ")
    };
    format!("[{ts}] {:>9}: {content}\n", m.role)
}

/// `[hh:mm:ss] event  key=val…` with a compact payload summary.
#[must_use]
pub fn render_event(e: &TelemetryRecord) -> String {
    let ts = e.valid_from.format("%H:%M:%S");
    format!(
        "[{ts}] {:<20} {}\n",
        e.lifecycle_event,
        compact_value(&e.payload)
    )
}

#[must_use]
pub fn render_boot_report(report: &superx_kernel::BootReport) -> String {
    let mut out = format!(
        "boot complete: {}/{} active\n",
        report.active_count(),
        report.entries.len()
    );
    for e in &report.entries {
        out.push_str(&format!(
            "  {:<24} {}\n",
            e.name,
            e.outcome.short_tag()
        ));
    }
    out
}

/// Compact single-line payload rendering: `k=v` pairs for objects,
/// bare value otherwise, long strings elided.
fn compact_value(v: &Value) -> String {
    const ELIDE: usize = 80; // skill-allow: §9-const — render truncation bound
    match v {
        Value::Object(o) => o
            .iter()
            .map(|(k, v)| format!("{k}={}", elide(&plain(v), ELIDE)))
            .collect::<Vec<_>>()
            .join(" "),
        other => elide(&plain(other), ELIDE),
    }
}

fn plain(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::None | Value::Null => String::new(),
        other => format!("{other:?}"),
    }
}

fn elide(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}

/// The bare UUIDv7 of a record id (RecordId has no Display; its
/// Debug nests wrappers).
#[must_use]
pub fn record_uuid(id: &superx_kernel::types::RecordId) -> String {
    let dbg = format!("{:?}", id.key);
    dbg.rfind('(')
        .and_then(|start| dbg[start + 1..].find(')').map(|end| dbg[start + 1..start + 1 + end].to_string()))
        .unwrap_or(dbg)
}

/// Compact `table:uuid` rendering for display.
#[must_use]
pub fn short_record_id(id: &superx_kernel::types::RecordId) -> String {
    let table = format!("{:?}", id.table).replace("Table(\"", "").replace("\")", "");
    format!("{table}:{}", record_uuid(id))
}

fn payload_str(payload: &Value, key: &str) -> Option<String> {
    match payload {
        Value::Object(o) => match o.get(key) {
            Some(Value::String(s)) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// Print + flush (live tails must not sit in stdout buffers).
pub fn emit(text: &str) {
    print!("{text}");
    if let Err(e) = std::io::stdout().flush() {
        tracing::warn!(error = %e, "stdout flush failed");
    }
}
