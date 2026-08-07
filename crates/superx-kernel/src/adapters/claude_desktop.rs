//! Claude Desktop adapter — captures the app's plain-text logs as
//! telemetry.
//!
//! Honest capture surface (mapped from live data on this machine,
//! 2026-08-07): Claude Desktop stores **no conversation data
//! locally** — conversations are cloud-side. What IS capturable is
//! app lifecycle, MCP connector registration/connection events,
//! Claude Code bridge init, login state, and auto-update outcomes,
//! from electron-log files under `~/Library/Logs/Claude/`. This
//! adapter therefore emits telemetry only, no `message` rows.
//!
//! - Line format: `YYYY-MM-DD HH:MM:SS [level] message`, local time;
//!   records may span multiple lines (continuations don't start with
//!   a date).
//! - ~99% of lines are polling noise (health checks, feature
//!   refreshes); only signal-bearing lines become events.
//! - Rotation: `<name>.log` → `<name>1.log`, one generation kept —
//!   shrinkage resets the offset. The duplicated
//!   `unknown-window*.log` renderers are skipped (byte-identical to
//!   `claude.ai-web*.log`).

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use linkme::distributed_slice;
use surrealdb::types::{Object, Value};

use crate::capture::{AgentAdapter, DiscoveredSource, SourceRef, ADAPTERS};
use crate::error::{KernelError, Result};
use crate::registry::{KernelModule, KernelModuleDescriptor, NodeKind, KERNEL_MODULES};
use crate::substrate::Kernel;

/// Adapter + registry-module name.
pub const ADAPTER_NAME: &str = "adapter_claude_desktop";

/// The agent this adapter captures.
pub const AGENT_NAME: &str = "claude_desktop";

/// Parameter overriding the logs root
/// (`~/Library/Logs/Claude` by default).
pub const LOGS_ROOT_PARAM: &str = "attr_claude_desktop_logs_root";

/// Cursor type uid for log read positions.
pub const CURSOR_TYPE: &str = "claude_desktop_logs";

/// Signal markers: a line containing any of these becomes a
/// `desktop_event`; everything else is polling noise and is skipped.
/// Format knowledge owned by this adapter (measured ~99% noise).
const SIGNAL_MARKERS: &[&str] = &[
    "[MCP]",
    "[Chrome Extension MCP]",
    "[Claude in Chrome]",
    "[CCD]",
    "MCP Server connection",
    "Starting app",
    "beforeQuit",
    "willQuit",
    "onQuitCleanup",
    "account active and logged in",
    "No persisted sessions",
    "[error]",
];

pub struct ClaudeDesktopAdapter;

#[async_trait]
impl KernelModule for ClaudeDesktopAdapter {
    fn descriptor(&self) -> KernelModuleDescriptor {
        KernelModuleDescriptor {
            name: ADAPTER_NAME,
            version: env!("CARGO_PKG_VERSION"),
            kind: NodeKind::Adapter,
            depends_on: &[],
            required_metamodel: &[],
        }
    }

    async fn startup(&self, _kernel: &Kernel) -> Result<()> {
        Ok(())
    }
}

#[distributed_slice(KERNEL_MODULES)]
static MODULE_REGISTRATION: &'static (dyn KernelModule + Sync) = &ClaudeDesktopAdapter;

#[distributed_slice(ADAPTERS)]
static ADAPTER_REGISTRATION: &'static (dyn AgentAdapter + Sync) = &ClaudeDesktopAdapter;

#[async_trait]
impl AgentAdapter for ClaudeDesktopAdapter {
    fn name(&self) -> &'static str {
        ADAPTER_NAME
    }

    fn agent_name(&self) -> &'static str {
        AGENT_NAME
    }

    async fn discover(&self, kernel: &Kernel) -> Result<Vec<DiscoveredSource>> {
        let Some(root) = self.logs_root(kernel).await? else {
            return Ok(vec![]); // app not present on this machine
        };
        let Ok(entries) = std::fs::read_dir(&root) else {
            return Ok(vec![]);
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            // Active-generation logs only; skip rotated `<name>1.log`
            // and the byte-identical unknown-window duplicates.
            if path.is_file()
                && name.ends_with(".log")
                && !name.starts_with("unknown-window")
                && !name.trim_end_matches(".log").ends_with('1')
            {
                out.push(DiscoveredSource {
                    name,
                    locator: path.to_string_lossy().to_string(),
                });
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    async fn poll(&self, kernel: &Kernel, source: &SourceRef) -> Result<u64> {
        kernel
            .ensure_cursor_type(
                CURSOR_TYPE,
                "telemetry",
                "Claude Desktop log read position (byte offset)",
            )
            .await?;

        let file = PathBuf::from(&source.locator);
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);

        let prior = kernel
            .latest_cursor(source.entity_id.clone(), CURSOR_TYPE)
            .await?;
        let mut offset = prior
            .as_ref()
            .and_then(|c| c.metadata.as_ref())
            .and_then(|m| match m.get("offset") {
                Some(Value::Number(n)) => n.to_int().map(|i| i.max(0) as u64),
                _ => None,
            })
            .unwrap_or(0);
        if offset > len {
            offset = 0; // rotated — one generation kept, recapture
        }
        if len == offset {
            return Ok(0);
        }

        let (lines, consumed) = read_complete_lines(&file, offset)?;
        let records = fold_multiline(&lines);
        let mut events = 0u64;
        for record in records {
            if !SIGNAL_MARKERS.iter().any(|m| record.contains(m)) {
                continue; // measured ~99% polling noise
            }
            let mut payload = Object::new();
            payload.insert("line".to_string(), Value::String(record.clone()));
            payload.insert(
                "file".to_string(),
                Value::String(source.name.clone()),
            );
            kernel
                .log_telemetry_for_agent(
                    "desktop_event",
                    Value::Object(payload),
                    Some(source.entity_id.clone()),
                    Some(source.agent_id.clone()),
                )
                .await?;
            events += 1;
        }

        if consumed > 0 {
            let mut metadata = Object::new();
            metadata.insert(
                "offset".to_string(),
                Value::Number(((offset + consumed) as i64).into()),
            );
            kernel
                .write_cursor(
                    source.entity_id.clone(),
                    CURSOR_TYPE,
                    Some(events.to_string()),
                    Some(metadata),
                )
                .await?;
        }
        Ok(events)
    }
}

impl ClaudeDesktopAdapter {
    async fn logs_root(&self, kernel: &Kernel) -> Result<Option<PathBuf>> {
        if let Some(entity) = kernel
            .find_module_by_name(NodeKind::Adapter, ADAPTER_NAME)
            .await?
        {
            if let Some(Value::String(p)) = kernel.get_parameter(entity, LOGS_ROOT_PARAM).await?
            {
                return Ok(Some(PathBuf::from(p)));
            }
        }
        // skill-allow: §9-default — agent-format default; operator overrides via the parameter
        let home = std::env::var("HOME").map_err(|_| {
            KernelError::Config("HOME is not set; cannot locate ~/Library/Logs/Claude".into())
        })?;
        let root = Path::new(&home).join("Library").join("Logs").join("Claude");
        Ok(root.is_dir().then_some(root))
    }
}

/// Fold continuation lines into their parent record: a new record
/// starts with a `YYYY-MM-DD ` date prefix; anything else belongs to
/// the previous line (Node error dumps span multiple lines).
fn fold_multiline(lines: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in lines {
        if starts_with_date(line) || out.is_empty() {
            out.push(line.clone());
        } else if let Some(last) = out.last_mut() {
            last.push('\n');
            last.push_str(line);
        }
    }
    out
}

fn starts_with_date(line: &str) -> bool {
    let b = line.as_bytes();
    b.len() > 10
        && b[0].is_ascii_digit()
        && b[1].is_ascii_digit()
        && b[2].is_ascii_digit()
        && b[3].is_ascii_digit()
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b' '
}

fn read_complete_lines(file: &Path, offset: u64) -> Result<(Vec<String>, u64)> {
    let mut f = std::fs::File::open(file)
        .map_err(|e| KernelError::Module(format!("open {}: {e}", file.display())))?;
    f.seek(SeekFrom::Start(offset))
        .map_err(|e| KernelError::Module(format!("seek {}: {e}", file.display())))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)
        .map_err(|e| KernelError::Module(format!("read {}: {e}", file.display())))?;
    let Some(last_newline) = buf.iter().rposition(|&b| b == b'\n') else {
        return Ok((vec![], 0));
    };
    let complete = &buf[..=last_newline];
    let lines = String::from_utf8_lossy(complete)
        .lines()
        .map(str::to_string)
        .collect();
    Ok((lines, (last_newline + 1) as u64))
}
