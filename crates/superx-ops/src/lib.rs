//! `superx-ops` — shared SuperX operations (epic #141 P3).
//!
//! The command runners, renderers, and instance utilities that both
//! the CLI (`superx`) and modules (first consumer: the UI) build on.
//! Kernel-only dependencies: this crate never links modules and never
//! touches clap/config — pure operations over a connected [`Kernel`].
//! Extracted from the CLI as pure movement; the CLI re-exports
//! everything, so all previous paths remain valid.

use chrono::{DateTime, Utc};
use superx_kernel::types::Value;
use superx_kernel::{Kernel, MessageRecord, NodeKind, TelemetryRecord};

use std::path::{Path, PathBuf};

/// Poll cadence for the `--live` tails, on the `kernel` registry
/// entity.
pub const LIVE_POLL_PARAM: &str = "attr_cli_live_poll_secs";
const DEFAULT_LIVE_POLL_SECS: u64 = 1; // skill-allow: §9-const — bootstrap fallback, param-overridable

/// Page size for historical reads before a live tail begins.
const READ_BACKLOG_LIMIT: u32 = 10_000; // skill-allow: §9-const — render page bound


/// The background OS pidfile: sibling of the datastore.
#[must_use]
pub fn pid_path(data_dir: &Path) -> PathBuf {
    data_dir
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("superx.pid")
}

/// Is this pid alive? (`kill -0` probe.)
#[must_use]
pub fn pid_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// The running background OS's pid, if the pidfile exists AND the
/// process is alive. A stale pidfile (dead process) is removed.
#[must_use]
pub fn read_live_pid(data_dir: &Path) -> Option<u32> {
    let path = pid_path(data_dir);
    let pid: u32 = std::fs::read_to_string(&path).ok()?.trim().parse().ok()?;
    if pid_alive(pid) {
        Some(pid)
    } else {
        let _removed = std::fs::remove_file(&path); // stale — clean up
        None
    }
}

pub fn write_pidfile(data_dir: &Path, pid: u32) -> Result<(), String> {
    let path = pid_path(data_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create {parent:?}: {e}"))?;
    }
    std::fs::write(&path, format!("{pid}\n")).map_err(|e| format!("write {path:?}: {e}"))
}


/// `superx modules list` body: the compiled-in inventory joined with
/// substrate state (operator intent + lifecycle).
pub async fn run_modules_list(kernel: &Kernel) -> Result<String, String> {
    let mut out = String::new();
    out.push_str(&format!(
        "{:<26} {:<38} {:<14} {:<9} {:<9} {:<5} {}\n",
        "MODULE", "MODULE_ID", "KIND", "INTENT", "LIFECYCLE", "PROV", "VERSION"
    ));
    for module in superx_kernel::KERNEL_MODULES {
        let desc = module.descriptor();
        let (module_id, intent, lifecycle) =
            match kernel.detailed_status(desc.kind, desc.name).await {
                Ok(Some(status)) => {
                    let intent = match kernel
                        .module_status(desc.kind, desc.name)
                        .await
                        .map_err(|e| e.to_string())?
                    {
                        Some(superx_kernel::ModuleStatus::Disabled) => "disabled",
                        _ => "enabled",
                    };
                    (
                        record_uuid(&status.entity_id),
                        intent,
                        status.lifecycle.short_tag().to_string(),
                    )
                }
                _ => ("-".to_string(), "enabled", "never-booted".to_string()),
            };
        let provisioned = kernel
            .latest_module_record(desc.name)
            .await
            .ok()
            .flatten()
            .map(|r| if r.provisioned { "yes" } else { "no" })
            .unwrap_or("-");
        out.push_str(&format!(
            "{:<26} {:<38} {:<14} {:<9} {:<9} {:<5} v{}\n",
            desc.name,
            module_id,
            desc.kind.type_uid().trim_start_matches("node_"),
            intent,
            lifecycle,
            provisioned,
            desc.version
        ));
    }
    Ok(out)
}

/// Resolve a module by NAME or by a fragment of its UUIDv7 module id
/// (operator directive: each module is referenced by its own uuid7).
pub async fn resolve_module(
    kernel: &Kernel,
    query: &str,
) -> Result<superx_kernel::KernelModuleDescriptor, String> {
    for module in superx_kernel::KERNEL_MODULES {
        let desc = module.descriptor();
        if desc.name == query {
            return Ok(desc);
        }
        if let Ok(Some(status)) = kernel.detailed_status(desc.kind, desc.name).await {
            if record_uuid(&status.entity_id).contains(query) {
                return Ok(desc);
            }
        }
    }
    Err(format!(
        "no compiled-in module matches '{query}' (name or uuid fragment) — see `superx modules list`"
    ))
}

/// `superx modules enable|disable` body. Resolves the module in the
/// compiled-in inventory by name or uuid7 fragment, writes intent,
/// reports when it takes effect.
pub async fn run_modules_set(
    kernel: &Kernel,
    name: &str,
    enable: bool,
) -> Result<String, String> {
    let desc = resolve_module(kernel, name).await?;
    let name = desc.name;
    let status = if enable {
        superx_kernel::ModuleStatus::Enabled
    } else {
        superx_kernel::ModuleStatus::Disabled
    };
    kernel
        .set_module_status(desc.kind, name, status)
        .await
        .map_err(|e| e.to_string())?;
    Ok(format!(
        "{name} {} — takes effect on the running OS within one capture tick\n",
        if enable { "enabled" } else { "disabled" }
    ))
}

/// A module's own CLI: route `superx <module-name> [args…]` to the
/// module's `cli()` hook.
pub async fn run_module_cli(
    kernel: &Kernel,
    argv: &[String],
) -> Result<String, String> {
    let Some(name) = argv.first() else {
        return Err("empty module command".to_string());
    };
    let Some(module) = superx_kernel::KERNEL_MODULES
        .iter()
        .find(|m| m.descriptor().name == *name)
    else {
        return Err(format!(
            "unknown command or module '{name}' — `superx --help` / `superx modules list`"
        ));
    };
    module
        .cli(kernel, &argv[1..])
        .await
        .map_err(|e| e.to_string())
}

/// `superx status` body.
pub async fn run_status(kernel: &Kernel, data_dir: &Path) -> Result<String, String> {
    let mut out = String::new();
    // Boot honesty (issue #158): a running pidfile must never mask a
    // dead OS — if nothing is active, say so at the top, loudly.
    let mut any = 0usize;
    let mut active = 0usize;
    for kind in [NodeKind::KernelModule, NodeKind::Adapter] {
        if let Ok(list) = kernel.list_with_status(kind).await {
            for s in &list {
                any += 1;
                if s.lifecycle.short_tag() == "active" {
                    active += 1;
                }
            }
        }
    }
    if any > 0 && active == 0 {
        out.push_str(
            "⚠ BOOT UNHEALTHY: 0 modules active — the OS is up but doing nothing.\n  Check `superx logs -n 40` and run `superx upgrade`, then `superx restart`.\n",
        );
    }
    match read_live_pid(data_dir) {
        Some(pid) => out.push_str(&format!("OS: running in background (pid {pid})\n")),
        None => out.push_str("OS: not running (`superx --initialize` starts it)\n"),
    }
    // The UI module's URL, when it is compiled in and registered
    // (generic lookup — ops never depends on module crates).
    if let Ok(Some(entity)) = kernel
        .find_module_by_name(NodeKind::KernelModule, "ui")
        .await
    {
        let port = match kernel.get_parameter(entity, "attr_ui_port").await {
            Ok(Some(Value::Number(n))) => n
                .to_int()
                .and_then(|i| u16::try_from(i).ok())
                .filter(|&p| p > 0)
                .unwrap_or(5150), // skill-allow: §9-or — mirrors the ui module's param-overridable default
            _ => 5150, // skill-allow: §9-const — mirrors the ui module's param-overridable default
        };
        out.push_str(&format!("UI: http://127.0.0.1:{port}\n"));
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
/// role line. Empty content renders a compact tool-call/thinking
/// summary from the preserved raw event (issue #135); the placeholder
/// remains only for truly opaque rows.
#[must_use]
pub fn render_message(m: &MessageRecord) -> String {
    let ts = m
        .emitted_at
        .unwrap_or(m.valid_from)
        .format("%Y-%m-%d %H:%M:%S");
    let content = if m.content.is_empty() {
        m.raw
            .as_ref()
            .and_then(summarize_raw)
            .unwrap_or_else(|| "(no text — see raw)".to_string())
    } else {
        m.content.replace('\n', "\n    ")
    };
    format!("[{ts}] {:>9}: {content}\n", m.role)
}

/// Compact summary of a no-text event from its raw payload: tool
/// calls render as `⚙ name — hint`, thinking as `… thinking`.
/// Understands both captured shapes: Anthropic-style
/// `message.content[]` blocks (Claude Code) and top-level
/// `toolCalls[]` (Gemini CLI). Returns `None` when nothing
/// summarizable is present.
#[must_use]
pub fn summarize_raw(raw: &superx_kernel::types::Object) -> Option<String> {
    const HINT_MAX: usize = 60; // skill-allow: §9-const — render truncation bound
    const LINE_MAX: usize = 160; // skill-allow: §9-const — render truncation bound
    let mut parts: Vec<String> = Vec::new();

    let obj_str = |o: &superx_kernel::types::Object, key: &str| -> Option<String> {
        match o.get(key) {
            Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
            _ => None,
        }
    };

    // Anthropic-style blocks: raw.message.content[]
    if let Some(Value::Object(msg)) = raw.get("message") {
        if let Some(Value::Array(blocks)) = msg.get("content") {
            for b in blocks.iter() {
                let Value::Object(block) = b else { continue };
                match block.get("type") {
                    Some(Value::String(k)) if k == "tool_use" => {
                        let name = obj_str(block, "name").unwrap_or_else(|| "tool".into());
                        let hint = match block.get("input") {
                            Some(Value::Object(input)) => [
                                "description",
                                "command",
                                "file_path",
                                "prompt",
                                "pattern",
                                "query",
                            ]
                            .iter()
                            .find_map(|k| obj_str(input, k)),
                            _ => None,
                        };
                        parts.push(match hint {
                            Some(h) => format!("⚙ {name} — {}", elide(&h, HINT_MAX)),
                            None => format!("⚙ {name}"),
                        });
                    }
                    Some(Value::String(k)) if k == "thinking" => {
                        parts.push("… thinking".to_string());
                    }
                    Some(Value::String(k)) if k == "tool_result" => {
                        parts.push("⚙ result".to_string());
                    }
                    _ => {}
                }
            }
        }
    }

    // Gemini-style: raw.toolCalls[]
    if let Some(Value::Array(calls)) = raw.get("toolCalls") {
        for c in calls.iter() {
            let Value::Object(call) = c else { continue };
            let name = obj_str(call, "name").unwrap_or_else(|| "tool".into());
            match obj_str(call, "status") {
                Some(s) => parts.push(format!("⚙ {name} [{s}]")),
                None => parts.push(format!("⚙ {name}")),
            }
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(elide(&parts.join("; "), LINE_MAX))
    }
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
        Value::Number(n) => n
            .to_int()
            .map(|i| i.to_string())
            .unwrap_or_else(|| format!("{n:?}")),
        Value::Bool(b) => b.to_string(),
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

