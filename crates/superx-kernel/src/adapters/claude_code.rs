//! Claude Code adapter — discovers projects under the Claude Code
//! projects root and captures transcript JSONL into first-class
//! `message` rows + `telemetry_stream` events.
//!
//! Format knowledge (mapped from live transcripts on this machine,
//! 2026-08-07):
//!
//! - Projects root: `~/.claude/projects/<mangled-cwd>/`; each
//!   `<sessionId>.jsonl` at the top level is one session transcript.
//! - Conversation lines carry `type` (`user` / `assistant`), a
//!   `message` envelope (string content for typed prompts; block
//!   arrays for assistant text / tool_use / tool_result), `sessionId`,
//!   ISO-8601 `timestamp`, and `origin.kind == "human"` on genuine
//!   human prompts.
//! - Sidecar lines (`ai-title`, `mode`, `file-history-*`, …) are
//!   state, not conversation — captured as telemetry.
//! - Unknown shapes are captured raw (`transcript_raw`), never
//!   dropped, never a crash.
//!
//! History is backfilled on first contact (operator directive:
//! conversations readable historical AND live), then the cursor
//! resumes incrementally.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use linkme::distributed_slice;
use surrealdb::types::{Object, RecordId, Value};

use crate::capture::{AgentAdapter, DiscoveredSource, SourceRef, ADAPTERS};
use crate::error::{KernelError, Result};
use crate::message::{json_to_object, NewMessage};
use crate::registry::{KernelModule, KernelModuleDescriptor, NodeKind, KERNEL_MODULES};
use crate::substrate::Kernel;

/// Adapter + registry-module name.
pub const ADAPTER_NAME: &str = "adapter_claude_code";

/// The agent this adapter captures.
pub const AGENT_NAME: &str = "claude_code";

/// Parameter (on this adapter's registry entity) overriding the
/// projects root. Tests inject fixture dirs through this — no env
/// hacks.
pub const PROJECTS_ROOT_PARAM: &str = "attr_claude_code_projects_root";

/// Cursor type uid for transcript read positions.
pub const CURSOR_TYPE: &str = "claude_code_transcript";

/// How much of an unparseable line the `transcript_raw` payload
/// carries.
// skill-allow: §9-const — safety truncation bound for corrupt lines
const RAW_SNIPPET_MAX: usize = 500;

pub struct ClaudeCodeAdapter;

#[async_trait]
impl KernelModule for ClaudeCodeAdapter {
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
static MODULE_REGISTRATION: &'static (dyn KernelModule + Sync) = &ClaudeCodeAdapter;

#[distributed_slice(ADAPTERS)]
static ADAPTER_REGISTRATION: &'static (dyn AgentAdapter + Sync) = &ClaudeCodeAdapter;

#[async_trait]
impl AgentAdapter for ClaudeCodeAdapter {
    fn name(&self) -> &'static str {
        ADAPTER_NAME
    }

    fn agent_name(&self) -> &'static str {
        AGENT_NAME
    }

    async fn discover(&self, kernel: &Kernel) -> Result<Vec<DiscoveredSource>> {
        let root = self.projects_root(kernel).await?;
        let Some(root) = root else {
            return Ok(vec![]); // agent not present on this machine
        };
        let mut out = Vec::new();
        let entries = std::fs::read_dir(&root).map_err(|e| {
            KernelError::Module(format!("cannot read projects root {}: {e}", root.display()))
        })?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = entry.file_name().to_string_lossy().to_string();
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
                "Claude Code transcript read position (per-file byte offsets)",
            )
            .await?;

        let dir = PathBuf::from(&source.locator);
        let files = transcript_files(&dir);

        let prior = kernel
            .latest_cursor(source.entity_id.clone(), CURSOR_TYPE)
            .await?;
        let mut offsets: BTreeMap<String, u64> = prior
            .as_ref()
            .and_then(|c| c.metadata.as_ref())
            .map(read_offsets)
            .unwrap_or_default();

        let mut events: u64 = 0;
        let mut changed = false;
        let mut sessions: BTreeMap<String, RecordId> = BTreeMap::new();

        for file in &files {
            let key = file.to_string_lossy().to_string();
            let len = file.metadata().map(|m| m.len()).unwrap_or(0);
            let mut offset = offsets.get(&key).copied().unwrap_or(0);
            if offset > len {
                offset = 0; // rotated / replaced — re-capture
            }
            if len == offset {
                continue;
            }
            let (lines, consumed) = read_complete_lines(file, offset)?;
            for line in lines {
                self.capture_line(kernel, source, &mut sessions, file, &line)
                    .await?;
                events += 1;
            }
            if consumed > 0 {
                offsets.insert(key, offset + consumed);
                changed = true;
            }
        }

        if changed {
            write_checkpoint(kernel, source, &offsets, events).await?;
        }
        Ok(events)
    }
}

impl ClaudeCodeAdapter {
    /// Resolve the projects root: the substrate parameter if set,
    /// else `$HOME/.claude/projects` when it exists, else `None`
    /// (agent absent).
    async fn projects_root(&self, kernel: &Kernel) -> Result<Option<PathBuf>> {
        if let Some(entity) = kernel
            .find_module_by_name(NodeKind::Adapter, ADAPTER_NAME)
            .await?
        {
            if let Some(Value::String(p)) =
                kernel.get_parameter(entity, PROJECTS_ROOT_PARAM).await?
            {
                return Ok(Some(PathBuf::from(p)));
            }
        }
        // Default location is format knowledge this adapter owns.
        // skill-allow: §9-default — agent-format default; operator overrides via the parameter
        let home = std::env::var("HOME").map_err(|_| {
            KernelError::Config("HOME is not set; cannot locate ~/.claude/projects".into())
        })?;
        let root = Path::new(&home).join(".claude").join("projects");
        Ok(root.is_dir().then_some(root))
    }

    /// Capture one transcript line: conversation lines become
    /// `message` rows (+ a light `message_captured` action event);
    /// sidecars become `transcript_event` telemetry; unparseable
    /// lines become `transcript_raw` telemetry. Never a crash.
    async fn capture_line(
        &self,
        kernel: &Kernel,
        source: &SourceRef,
        sessions: &mut BTreeMap<String, RecordId>,
        file: &Path,
        line: &str,
    ) -> Result<()> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Ok(());
        }
        // The transcript FILE is `<sessionId>.jsonl` — the filename
        // attributes lines that don't carry a sessionId themselves
        // (sidecars, unparseable lines). Nothing is ever "unknown"
        // (issues #186/#204).
        let file_session = file
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown-session".to_string());
        let Ok(json) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            let mut payload = Object::new();
            payload.insert(
                "file".to_string(),
                Value::String(file.to_string_lossy().to_string()),
            );
            payload.insert("session".to_string(), Value::String(file_session));
            let snippet: String = trimmed.chars().take(RAW_SNIPPET_MAX).collect();
            payload.insert("snippet".to_string(), Value::String(snippet));
            kernel
                .log_telemetry_for_agent(
                    "transcript_raw",
                    Value::Object(payload),
                    Some(source.entity_id.clone()),
                    Some(source.agent_id.clone()),
                )
                .await?;
            return Ok(());
        };

        let line_type = json.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let session_key = json
            .get("sessionId")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or(file_session);

        match line_type {
            "user" | "assistant" => {
                let session_id = match sessions.get(&session_key) {
                    Some(id) => id.clone(),
                    None => {
                        let id = crate::capture::ensure_session(
                            kernel,
                            &source.agent_id,
                            AGENT_NAME,
                            &session_key,
                            &file.to_string_lossy(),
                        )
                        .await?;
                        sessions.insert(session_key.clone(), id.clone());
                        id
                    }
                };
                let role = classify_role(&json, line_type);
                let content = extract_text(&json);
                let emitted_at = json
                    .get("timestamp")
                    .and_then(|v| v.as_str())
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.with_timezone(&Utc));

                kernel
                    .log_message(NewMessage {
                        session: session_id.clone(),
                        agent: source.agent_id.clone(),
                        role: role.to_string(),
                        content: content.clone(),
                        raw: Some(json_to_object(&json)),
                        seq: None,
                        emitted_at,
                    })
                    .await?;

                // Light action event so the live actions stream pulses
                // per message without duplicating the raw payload.
                let mut payload = Object::new();
                payload.insert("role".to_string(), Value::String(role.to_string()));
                payload.insert("session".to_string(), Value::String(session_key));
                payload.insert(
                    "chars".to_string(),
                    Value::Number((content.chars().count() as i64).into()),
                );
                kernel
                    .log_telemetry_for_agent(
                        "message_captured",
                        Value::Object(payload),
                        Some(session_id),
                        Some(source.agent_id.clone()),
                    )
                    .await?;
            }
            other => {
                let mut payload = Object::new();
                payload.insert("kind".to_string(), Value::String(other.to_string()));
                payload.insert("session".to_string(), Value::String(session_key));
                payload.insert(
                    "file".to_string(),
                    Value::String(file.to_string_lossy().to_string()),
                );
                kernel
                    .log_telemetry_for_agent(
                        "transcript_event",
                        Value::Object(payload),
                        Some(source.entity_id.clone()),
                        Some(source.agent_id.clone()),
                    )
                    .await?;
            }
        }
        Ok(())
    }
}

/// Role classification per the mapped format: genuine human prompts
/// (`origin.kind == "human"`) are `user`; tool results (block arrays
/// containing `tool_result`) are `tool`; other injected `user` lines
/// are `system`; `assistant` stays `assistant`.
fn classify_role(json: &serde_json::Value, line_type: &str) -> &'static str {
    if line_type == "assistant" {
        return "assistant";
    }
    let human = json
        .pointer("/origin/kind")
        .and_then(|v| v.as_str())
        .map(|k| k == "human")
        .unwrap_or(false);
    if human {
        return "user";
    }
    let has_tool_result = json
        .pointer("/message/content")
        .and_then(|c| c.as_array())
        .map(|blocks| {
            blocks
                .iter()
                .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result"))
        })
        .unwrap_or(false);
    if has_tool_result {
        "tool"
    } else {
        "system"
    }
}

/// Extract readable text: string content verbatim; block arrays yield
/// the concatenated `text` blocks plus string-form `tool_result`
/// contents. Absent/empty content yields `""` — `raw` always has the
/// full event.
fn extract_text(json: &serde_json::Value) -> String {
    let Some(content) = json.pointer("/message/content") else {
        return String::new();
    };
    match content {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(blocks) => {
            let mut parts: Vec<String> = Vec::new();
            for b in blocks {
                match b.get("type").and_then(|t| t.as_str()) {
                    Some("text") => {
                        if let Some(t) = b.get("text").and_then(|t| t.as_str()) {
                            parts.push(t.to_string());
                        }
                    }
                    Some("tool_result") => {
                        if let Some(t) = b.get("content").and_then(|t| t.as_str()) {
                            parts.push(t.to_string());
                        }
                    }
                    _ => {}
                }
            }
            parts.join("\n")
        }
        _ => String::new(),
    }
}

/// `*.jsonl` files at the top level of the project directory, sorted.
/// A vanished directory yields an empty list — discovery owns
/// forgetting sources.
fn transcript_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    files.sort();
    files
}

/// Read complete lines from `offset`, returning the lines and the
/// byte count consumed (up to and including the last newline — a
/// partial trailing line is left for the next poll).
fn read_complete_lines(file: &Path, offset: u64) -> Result<(Vec<String>, u64)> {
    let mut f = std::fs::File::open(file)
        .map_err(|e| KernelError::Module(format!("open {}: {e}", file.display())))?;
    f.seek(SeekFrom::Start(offset))
        .map_err(|e| KernelError::Module(format!("seek {}: {e}", file.display())))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)
        .map_err(|e| KernelError::Module(format!("read {}: {e}", file.display())))?;

    let Some(last_newline) = buf.iter().rposition(|&b| b == b'\n') else {
        return Ok((vec![], 0)); // no complete line yet
    };
    let complete = &buf[..=last_newline];
    let lines = String::from_utf8_lossy(complete)
        .lines()
        .map(str::to_string)
        .collect();
    Ok((lines, (last_newline + 1) as u64))
}

fn read_offsets(metadata: &Object) -> BTreeMap<String, u64> {
    let mut out = BTreeMap::new();
    if let Some(Value::Object(files)) = metadata.get("files") {
        for (k, v) in files.iter() {
            if let Value::Number(n) = v {
                if let Some(i) = n.to_int() {
                    out.insert(k.clone(), i.max(0) as u64);
                }
            }
        }
    }
    out
}

async fn write_checkpoint(
    kernel: &Kernel,
    source: &SourceRef,
    offsets: &BTreeMap<String, u64>,
    events: u64,
) -> Result<()> {
    let mut files = Object::new();
    for (k, v) in offsets {
        files.insert(k.clone(), Value::Number((*v as i64).into()));
    }
    let mut metadata = Object::new();
    metadata.insert("files".to_string(), Value::Object(files));
    kernel
        .write_cursor(
            source.entity_id.clone(),
            CURSOR_TYPE,
            Some(events.to_string()),
            Some(metadata),
        )
        .await?;
    Ok(())
}
