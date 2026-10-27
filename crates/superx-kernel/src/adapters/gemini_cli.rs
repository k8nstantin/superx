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
//! - `{"$set": …}` lines are header mutations, not messages: the header
//!   and every `$set` are kept whole as `transcript_event`s, as their text (#471, #541).
//! - `type:"user"` — `content` is an ARRAY of parts (`{text}` /
//!   `inlineData`); `type:"gemini"` — `content` is a plain string,
//!   plus `thoughts`, `tokens`, `model`, optional `toolCalls`.
//! - Records are RE-EMITTED as they stream (same `id`, progressively
//!   fuller). Every line is captured whole (#518): within one read, a
//!   record's last write is its `message` row and each earlier write a
//!   `transcript_event` of kind `superseded`; a later read's write is a
//!   row of its own.
//! - Only the header names the session: every line of a file is captured
//!   into it, whichever read takes the line.
//! - Nothing is cut: a new file is read whole from its start, and a
//!   record keeps its `resultDisplay` and its `inlineData` as written.

use std::collections::BTreeMap;
use std::io::{BufRead, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use linkme::distributed_slice;
use surrealdb::types::{Object, RecordId, Value};

use crate::capture::{AgentAdapter, DiscoveredSource, SourceRef, ADAPTERS};
use crate::error::{KernelError, Result};
use crate::message::NewMessage;
use crate::registry::{KernelModule, KernelModuleDescriptor, NodeKind, KERNEL_MODULES};
use crate::substrate::Kernel;

/// Adapter + registry-module name.
pub const ADAPTER_NAME: &str = "adapter_gemini_cli";

/// The agent this adapter captures.
pub const AGENT_NAME: &str = "gemini_cli";

/// Parameter overriding the Gemini root (`~/.gemini` by default).
pub const ROOT_PARAM: &str = "attr_gemini_cli_root";

/// Cursor type uid for chat-file read positions.
pub const CURSOR_TYPE: &str = "gemini_cli_chats";

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

        let mut events: u64 = 0;
        let mut changed = false;
        let mut sessions: BTreeMap<String, RecordId> = BTreeMap::new();
        // Session key discovered from each file's header line; keyed
        // by file so non-header lines resolve their session.
        let mut file_sessions: BTreeMap<String, String> = BTreeMap::new();

        for file in &files {
            let key = file.to_string_lossy().to_string();
            let len = file.metadata().map(|m| m.len()).unwrap_or(0);
            let mut offset = offsets.get(&key).copied().unwrap_or(0);
            if offset > len {
                offset = 0; // rotated / replaced
            }
            if len == offset {
                continue;
            }
            let (lines, consumed) = read_complete_lines(file, offset)?;
            if offset > 0 && !file_sessions.contains_key(&key) {
                // This read starts past the header, the only line that
                // names the session.
                if let Some(session) = header_session(file)? {
                    file_sessions.insert(key.clone(), session);
                }
            }

            for (json, text, superseded) in parse_read(&lines) {
                events += if superseded {
                    self.capture_superseded(kernel, source, &file_sessions, file, &json, text).await?
                } else {
                    self.capture_record(kernel, source, &mut sessions, &mut file_sessions, file, &json, text)
                        .await?
                };
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
        text: &str,
    ) -> Result<u64> {
        let file_key = file.to_string_lossy().to_string();

        // Header line: establishes the session for this file.
        let header_session = json
            .get("sessionId")
            .and_then(|v| v.as_str())
            .filter(|_| json.get("type").is_none() && json.get("$set").is_none());
        if let Some(session_id) = header_session {
            file_sessions.insert(file_key.clone(), session_id.to_string());
        }
        let header_kind = if header_session.is_some() {
            Some("header")
        } else if json.get("$set").is_some() {
            Some("$set")
        } else {
            None
        };

        let session_key = session_of(file_sessions, file, json);

        // A header, and a header mutation (`$set`), are not conversation:
        // they are kept whole as telemetry (#471). They were folded away.
        if let Some(kind) = header_kind {
            let mut payload = Object::new();
            payload.insert("kind".to_string(), Value::String(kind.to_string()));
            payload.insert("session".to_string(), Value::String(session_key));
            payload.insert("file".to_string(), Value::String(file_key));
            payload.insert("raw_text".to_string(), Value::String(text.to_string()));
            kernel
                .log_telemetry_for_agent(
                    "transcript_event",
                    Value::Object(payload),
                    Some(source.entity_id.clone()),
                    Some(source.agent_id.clone()),
                )
                .await?;
            return Ok(1);
        }

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
                kernel
                    .log_message_line(NewMessage {
                        session: session_id.clone(),
                        agent: source.agent_id.clone(),
                        role: role.to_string(),
                        content: content.clone(),
                        // The line's text alone (`raw_text`, below): a parsed copy
                        // made capture depend on how deep a line nests (#541).
                        raw: None,
                        seq: None,
                        emitted_at,
                    }, Some(text.to_string()))
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
                payload.insert("raw_text".to_string(), Value::String(text.to_string()));
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
                // Unknown shape — captured raw, never dropped, and
                // still attributed to its session (issue #186).
                let mut payload = Object::new();
                payload.insert("file".to_string(), Value::String(file_key));
                payload.insert("session".to_string(), Value::String(session_key));
                let snippet: String = json.to_string().chars().take(500).collect();
                payload.insert("snippet".to_string(), Value::String(snippet));
                payload.insert("raw_text".to_string(), Value::String(text.to_string()));
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

    /// Capture a record's earlier write, which a later line of the same
    /// read writes again: whole, as a `transcript_event` of kind
    /// `superseded`, so no line is dropped (#518).
    async fn capture_superseded(
        &self,
        kernel: &Kernel,
        source: &SourceRef,
        file_sessions: &BTreeMap<String, String>,
        file: &Path,
        json: &serde_json::Value,
        text: &str,
    ) -> Result<u64> {
        let mut payload = Object::new();
        payload.insert("kind".to_string(), Value::String("superseded".to_string()));
        payload.insert("session".to_string(), Value::String(session_of(file_sessions, file, json)));
        payload.insert("file".to_string(), Value::String(file.to_string_lossy().to_string()));
        payload.insert("raw_text".to_string(), Value::String(text.to_string()));
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
}

/// A read's lines, in order, each parsed, with its exact text (#528), and
/// with whether a later line of the same read writes the same record (same
/// `id`): Gemini re-emits a record as it streams, each time fuller. Headers
/// and `$set` lines carry no `id`.
/// Unparseable lines survive as raw strings wrapped in a marker so the
/// caller still captures them.
fn parse_read(lines: &[String]) -> Vec<(serde_json::Value, &str, bool)> {
    let read: Vec<&str> = lines.iter().map(String::as_str).filter(|line| !line.trim().is_empty()).collect();
    let parsed: Vec<serde_json::Value> = read
        .iter()
        .map(|line| {
            let trimmed = line.trim();
            serde_json::from_str::<serde_json::Value>(trimmed)
                .unwrap_or_else(|_| serde_json::json!({ "type": "unparseable", "line": trimmed }))
        })
        .collect();
    let mut last: BTreeMap<&str, usize> = BTreeMap::new();
    for (i, json) in parsed.iter().enumerate() {
        if let Some(id) = record_id(json) {
            last.insert(id, i);
        }
    }
    let superseded: Vec<bool> =
        parsed.iter().enumerate().map(|(i, json)| record_id(json).is_some_and(|id| last.get(id) != Some(&i))).collect();
    parsed.into_iter().zip(read).zip(superseded).map(|((json, text), superseded)| (json, text, superseded)).collect()
}

/// A conversation record's `id`; headers and `$set` lines have none.
fn record_id(json: &serde_json::Value) -> Option<&str> {
    json.get("id").and_then(|v| v.as_str()).filter(|_| json.get("type").is_some())
}

/// The session a line is captured into: the one its file's header names,
/// else the line's own `sessionId`, else the file's name.
fn session_of(file_sessions: &BTreeMap<String, String>, file: &Path, json: &serde_json::Value) -> String {
    file_sessions
        .get(&*file.to_string_lossy())
        .cloned()
        .or_else(|| json.get("sessionId").and_then(|v| v.as_str()).map(str::to_string))
        .unwrap_or_else(|| {
            file.file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "unknown-session".to_string())
        })
}

/// The session a chat file's header names: its first line, when that line
/// is a header (a `sessionId` with no `type` and no `$set`).
fn header_session(file: &Path) -> Result<Option<String>> {
    let f = std::fs::File::open(file)
        .map_err(|e| KernelError::Module(format!("open {}: {e}", file.display())))?;
    let mut first = String::new();
    std::io::BufReader::new(f)
        .read_line(&mut first)
        .map_err(|e| KernelError::Module(format!("read {}: {e}", file.display())))?;
    let Ok(json) = serde_json::from_str::<serde_json::Value>(first.trim()) else {
        return Ok(None);
    };
    Ok(json
        .get("sessionId")
        .and_then(|v| v.as_str())
        .filter(|_| json.get("type").is_none() && json.get("$set").is_none())
        .map(str::to_string))
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
