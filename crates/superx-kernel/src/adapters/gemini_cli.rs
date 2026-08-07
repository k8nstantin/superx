//! Gemini CLI adapter — discovers per-project session dirs under the
//! Gemini root and captures chat JSONL into `message` +
//! `telemetry_stream` rows.
//!
//! Format knowledge (mapped from live data on this machine,
//! 2026-08-07):
//!
//! - Root: `~/.gemini/tmp/<project-slug>/`; conversations live in
//!   `chats/session-*.jsonl` (top level; subagent transcripts in
//!   subdirectories are post-FVP).
//! - Line 1 of each file is a session header (`sessionId`, `kind`).
//! - `{"$set": …}` lines are header mutations — folded, not messages.
//! - `type:"user"` — `content` is an ARRAY of parts (`{text}` /
//!   `inlineData`); `type:"gemini"` — `content` is a plain string,
//!   plus `thoughts`, `tokens`, `model`, optional `toolCalls`.
//! - Records are RE-EMITTED as they stream (same `id`, progressively
//!   fuller): dedupe within a batch by id (last wins); across batches
//!   a same-id record is re-emitted only if it grew.
//! - Files reach hundreds of MB with base64 `inlineData` and huge
//!   `resultDisplay` render junk: backfill is byte-capped and raw
//!   payloads are sanitized (resultDisplay stripped, inlineData
//!   truncated).

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
pub const ADAPTER_NAME: &str = "adapter_gemini_cli";

/// The agent this adapter captures.
pub const AGENT_NAME: &str = "gemini_cli";

/// Parameter overriding the Gemini root (`~/.gemini` by default).
pub const ROOT_PARAM: &str = "attr_gemini_cli_root";

/// Parameter capping how many trailing bytes of a large chat file are
/// backfilled on first contact.
pub const BACKFILL_MAX_PARAM: &str = "attr_gemini_cli_backfill_max_bytes";

/// Cursor type uid for chat-file read positions.
pub const CURSOR_TYPE: &str = "gemini_cli_chats";

/// Default backfill cap; Gemini chat files reach 677 MB on this
/// machine and ingesting them whole would swamp the substrate.
const DEFAULT_BACKFILL_MAX_BYTES: u64 = 8_388_608; // skill-allow: §9-const — 8 MiB backfill cap, param-overridable

/// Truncation bound for base64 `inlineData` blobs inside raw payloads.
const INLINE_DATA_MAX: usize = 256; // skill-allow: §9-const — raw-payload sanitization bound

pub struct GeminiCliAdapter;

#[async_trait]
impl KernelModule for GeminiCliAdapter {
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
static MODULE_REGISTRATION: &'static (dyn KernelModule + Sync) = &GeminiCliAdapter;

#[distributed_slice(ADAPTERS)]
static ADAPTER_REGISTRATION: &'static (dyn AgentAdapter + Sync) = &GeminiCliAdapter;

#[async_trait]
impl AgentAdapter for GeminiCliAdapter {
    fn name(&self) -> &'static str {
        ADAPTER_NAME
    }

    fn agent_name(&self) -> &'static str {
        AGENT_NAME
    }

    async fn discover(&self, kernel: &Kernel) -> Result<Vec<DiscoveredSource>> {
        let Some(root) = self.root(kernel).await? else {
            return Ok(vec![]); // agent not present on this machine
        };
        let tmp = root.join("tmp");
        let Ok(entries) = std::fs::read_dir(&tmp) else {
            return Ok(vec![]);
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if path.is_dir() && name != "background-processes" {
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
                "Gemini CLI chat-file read position (per-file byte offsets)",
            )
            .await?;

        let chats = PathBuf::from(&source.locator).join("chats");
        let files = chat_files(&chats);

        let prior = kernel
            .latest_cursor(source.entity_id.clone(), CURSOR_TYPE)
            .await?;
        let mut offsets: BTreeMap<String, u64> = prior
            .as_ref()
            .and_then(|c| c.metadata.as_ref())
            .map(read_offsets)
            .unwrap_or_default();
        let backfill_max = self.backfill_max(kernel).await?;

        let mut events: u64 = 0;
        let mut changed = false;
        let mut sessions: BTreeMap<String, RecordId> = BTreeMap::new();
        // Session key discovered from each file's header line; keyed
        // by file so non-header lines resolve their session.
        let mut file_sessions: BTreeMap<String, String> = BTreeMap::new();

        for file in &files {
            let key = file.to_string_lossy().to_string();
            let len = file.metadata().map(|m| m.len()).unwrap_or(0);
            let known = offsets.contains_key(&key);
            let mut offset = offsets.get(&key).copied().unwrap_or(0);
            if offset > len {
                offset = 0; // rotated / replaced
            }
            if !known && len > backfill_max {
                // First contact with an oversized file: cap the
                // backfill to the trailing window, aligned to the
                // next full line.
                offset = align_to_next_line(file, len - backfill_max)?;
                let mut payload = Object::new();
                payload.insert("file".to_string(), Value::String(key.clone()));
                payload.insert(
                    "skipped_bytes".to_string(),
                    Value::Number((offset as i64).into()),
                );
                kernel
                    .log_telemetry_for_agent(
                        "backfill_capped",
                        Value::Object(payload),
                        Some(source.entity_id.clone()),
                        Some(source.agent_id.clone()),
                    )
                    .await?;
            }
            if len == offset {
                continue;
            }
            let (lines, consumed) = read_complete_lines(file, offset)?;

            // Batch-level dedupe of re-emitted records: same id →
            // last occurrence wins.
            let parsed = dedupe_batch(&lines);
            for json in parsed {
                events += self
                    .capture_record(kernel, source, &mut sessions, &mut file_sessions, file, &json)
                    .await?;
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

impl GeminiCliAdapter {
    async fn root(&self, kernel: &Kernel) -> Result<Option<PathBuf>> {
        if let Some(entity) = kernel
            .find_module_by_name(NodeKind::Adapter, ADAPTER_NAME)
            .await?
        {
            if let Some(Value::String(p)) = kernel.get_parameter(entity, ROOT_PARAM).await? {
                return Ok(Some(PathBuf::from(p)));
            }
        }
        // skill-allow: §9-default — agent-format default; operator overrides via the parameter
        let home = std::env::var("HOME")
            .map_err(|_| KernelError::Config("HOME is not set; cannot locate ~/.gemini".into()))?;
        let root = Path::new(&home).join(".gemini");
        Ok(root.is_dir().then_some(root))
    }

    async fn backfill_max(&self, kernel: &Kernel) -> Result<u64> {
        if let Some(entity) = kernel
            .find_module_by_name(NodeKind::Adapter, ADAPTER_NAME)
            .await?
        {
            if let Some(Value::Number(n)) =
                kernel.get_parameter(entity, BACKFILL_MAX_PARAM).await?
            {
                if let Some(i) = n.to_int() {
                    return Ok(i.max(0) as u64);
                }
            }
        }
        Ok(DEFAULT_BACKFILL_MAX_BYTES)
    }

    /// Capture one deduped record. Returns events emitted (0 for
    /// folded `$set` lines).
    #[allow(clippy::too_many_arguments)] // skill-allow: allow — internal seam, grouped params
    async fn capture_record(
        &self,
        kernel: &Kernel,
        source: &SourceRef,
        sessions: &mut BTreeMap<String, RecordId>,
        file_sessions: &mut BTreeMap<String, String>,
        file: &Path,
        json: &serde_json::Value,
    ) -> Result<u64> {
        let file_key = file.to_string_lossy().to_string();

        // Header line: establishes the session for this file.
        if let Some(session_id) = json.get("sessionId").and_then(|v| v.as_str()) {
            if json.get("type").is_none() && json.get("$set").is_none() {
                file_sessions.insert(file_key.clone(), session_id.to_string());
                return Ok(0);
            }
        }
        // Header mutations are state churn, not conversation.
        if json.get("$set").is_some() {
            return Ok(0);
        }

        let session_key = file_sessions
            .get(&file_key)
            .cloned()
            .or_else(|| {
                json.get("sessionId")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| {
                file.file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "unknown-session".to_string())
            });

        let line_type = json.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match line_type {
            "user" | "gemini" => {
                let session_id = match sessions.get(&session_key) {
                    Some(id) => id.clone(),
                    None => {
                        let id = crate::capture::ensure_session(
                            kernel,
                            &source.agent_id,
                            AGENT_NAME,
                            &session_key,
                            &file_key,
                        )
                        .await?;
                        sessions.insert(session_key.clone(), id.clone());
                        id
                    }
                };
                let role = if line_type == "user" { "user" } else { "assistant" };
                let content = extract_text(json);
                let emitted_at = json
                    .get("timestamp")
                    .and_then(|v| v.as_str())
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.with_timezone(&Utc));
                let sanitized = sanitize(json);

                kernel
                    .log_message(NewMessage {
                        session: session_id.clone(),
                        agent: source.agent_id.clone(),
                        role: role.to_string(),
                        content: content.clone(),
                        raw: Some(json_to_object(&sanitized)),
                        seq: None,
                        emitted_at,
                    })
                    .await?;

                let mut payload = Object::new();
                payload.insert("role".to_string(), Value::String(role.to_string()));
                payload.insert("session".to_string(), Value::String(session_key.clone()));
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

                // Tool calls are actions — one event per call.
                let mut emitted = 1;
                if let Some(calls) = json.get("toolCalls").and_then(|v| v.as_array()) {
                    for call in calls {
                        let mut payload = Object::new();
                        if let Some(name) = call.get("name").and_then(|v| v.as_str()) {
                            payload.insert("tool".to_string(), Value::String(name.to_string()));
                        }
                        if let Some(status) = call.get("status").and_then(|v| v.as_str()) {
                            payload
                                .insert("status".to_string(), Value::String(status.to_string()));
                        }
                        payload
                            .insert("session".to_string(), Value::String(session_key.clone()));
                        kernel
                            .log_telemetry_for_agent(
                                "tool_call",
                                Value::Object(payload),
                                Some(source.entity_id.clone()),
                                Some(source.agent_id.clone()),
                            )
                            .await?;
                        emitted += 1;
                    }
                }
                Ok(emitted)
            }
            "info" | "error" | "warning" => {
                let mut payload = Object::new();
                payload.insert("kind".to_string(), Value::String(line_type.to_string()));
                payload.insert("session".to_string(), Value::String(session_key));
                if let Some(c) = json.get("content").and_then(|v| v.as_str()) {
                    payload.insert("content".to_string(), Value::String(c.to_string()));
                }
                kernel
                    .log_telemetry_for_agent(
                        "transcript_event",
                        Value::Object(payload),
                        Some(source.entity_id.clone()),
                        Some(source.agent_id.clone()),
                    )
                    .await?;
                Ok(1)
            }
            _ => {
                // Unknown shape — captured raw, never dropped.
                let mut payload = Object::new();
                payload.insert("file".to_string(), Value::String(file_key));
                let snippet: String = json.to_string().chars().take(500).collect();
                payload.insert("snippet".to_string(), Value::String(snippet));
                kernel
                    .log_telemetry_for_agent(
                        "transcript_raw",
                        Value::Object(payload),
                        Some(source.entity_id.clone()),
                        Some(source.agent_id.clone()),
                    )
                    .await?;
                Ok(1)
            }
        }
    }
}

/// Parse lines and dedupe re-emitted records: same `id` → the LAST
/// occurrence in the batch wins (records grow as they stream).
/// Unparseable lines survive as raw strings wrapped in a marker so
/// the caller still captures them.
fn dedupe_batch(lines: &[String]) -> Vec<serde_json::Value> {
    let mut order: Vec<(Option<String>, serde_json::Value)> = Vec::new();
    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let json = serde_json::from_str::<serde_json::Value>(trimmed).unwrap_or_else(|_| {
            serde_json::json!({ "type": "unparseable", "line": trimmed })
        });
        let id = json
            .get("id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            // Only conversation records dedupe; headers/$set have no id.
            .filter(|_| json.get("type").is_some());
        if let Some(ref id_val) = id {
            if let Some(pos) = order
                .iter()
                .position(|(existing, _)| existing.as_deref() == Some(id_val))
            {
                order.remove(pos);
            }
        }
        order.push((id, json));
    }
    order.into_iter().map(|(_, j)| j).collect()
}

/// Extract readable text: user parts arrays join their `text` fields;
/// gemini `content` is already a plain string.
fn extract_text(json: &serde_json::Value) -> String {
    match json.get("content") {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(parts)) => {
            let mut out: Vec<String> = Vec::new();
            for p in parts {
                if let Some(t) = p.get("text").and_then(|v| v.as_str()) {
                    out.push(t.to_string());
                }
            }
            out.join("\n")
        }
        _ => String::new(),
    }
}

/// Sanitize a record for raw storage: strip `resultDisplay` render
/// junk wholesale; truncate base64 `inlineData.data` blobs.
fn sanitize(json: &serde_json::Value) -> serde_json::Value {
    let mut v = json.clone();
    if let Some(calls) = v.get_mut("toolCalls").and_then(|c| c.as_array_mut()) {
        for call in calls {
            if let Some(obj) = call.as_object_mut() {
                obj.remove("resultDisplay");
            }
        }
    }
    if let Some(parts) = v.get_mut("content").and_then(|c| c.as_array_mut()) {
        for part in parts {
            if let Some(data) = part
                .pointer_mut("/inlineData/data")
                .and_then(|d| d.as_str().map(str::to_string))
            {
                if data.len() > INLINE_DATA_MAX {
                    let truncated: String = data.chars().take(INLINE_DATA_MAX).collect();
                    if let Some(slot) = part.pointer_mut("/inlineData/data") {
                        *slot = serde_json::Value::String(format!(
                            "{truncated}…[truncated {} bytes]",
                            data.len()
                        ));
                    }
                }
            }
        }
    }
    v
}

/// `chats/*.jsonl` at the top level, sorted. Subagent transcripts in
/// subdirectories are post-FVP.
fn chat_files(dir: &Path) -> Vec<PathBuf> {
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

/// Find the first line boundary at or after `from`, so a capped
/// backfill never starts mid-line.
fn align_to_next_line(file: &Path, from: u64) -> Result<u64> {
    let mut f = std::fs::File::open(file)
        .map_err(|e| KernelError::Module(format!("open {}: {e}", file.display())))?;
    f.seek(SeekFrom::Start(from))
        .map_err(|e| KernelError::Module(format!("seek {}: {e}", file.display())))?;
    let mut buf = [0u8; 65536];
    let mut pos = from;
    loop {
        let n = f
            .read(&mut buf)
            .map_err(|e| KernelError::Module(format!("read {}: {e}", file.display())))?;
        if n == 0 {
            return Ok(pos); // no newline until EOF
        }
        if let Some(idx) = buf[..n].iter().position(|&b| b == b'\n') {
            return Ok(pos + idx as u64 + 1);
        }
        pos += n as u64;
    }
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
