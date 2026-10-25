//! Claude Code adapter — discovers projects under the Claude Code
//! projects root and captures transcript JSONL into first-class
//! `message` rows + `telemetry_stream` events.
//!
//! Format knowledge (mapped from live transcripts on this machine,
//! 2026-08-07):
//!
//! - Projects root: `~/.claude/projects/<mangled-cwd>/`; each
//!   `<sessionId>.jsonl` at the top level is one session transcript.
//!   A subagent's own transcript is `<sessionId>/subagents/agent-*.jsonl`,
//!   a workflow agent's `<sessionId>/subagents/workflows/<run>/agent-*.jsonl`
//!   beside the run's `journal.jsonl`: their lines carry the parent
//!   session's `sessionId` (and `isSidechain`), and are captured into it.
//!   A session started under `CLAUDE_CONFIG_DIR=<dir>` writes to
//!   `<dir>/projects/` instead, so the root can be several folders.
//! - Conversation lines carry `type` (`user` / `assistant`), a
//!   `message` envelope (string content for typed prompts; block
//!   arrays for assistant text / tool_use / tool_result), `sessionId`,
//!   ISO-8601 `timestamp`, and `origin.kind == "human"` on genuine
//!   human prompts.
//! - Every other line (`attachment`, `system`, `api-request*`,
//!   `ai-title`, `mode`, `file-history-*`, …) is not conversation and is
//!   captured as telemetry, the whole line under `line` (#471).
//! - Unparseable lines are captured raw (`transcript_raw`), the whole
//!   text under `text`, never dropped, never a crash.
//! - Beside the transcripts Claude Code writes other files: saved tool
//!   outputs (`<sessionId>/tool-results/`), subagent descriptions
//!   (`subagents/**/*.meta.json`), workflows (`<sessionId>/workflows/`) and
//!   the project's `memory/`. Each is captured whole as a
//!   `transcript_event` of kind `file`, whenever its size or modification
//!   time changed (#520).
//!
//! History is backfilled on first contact (operator directive:
//! conversations readable historical AND live), then the cursor
//! resumes incrementally.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use linkme::distributed_slice;
use surrealdb::types::{Object, RecordId, Value};

use crate::capture::{AgentAdapter, DiscoveredSource, SourceRef, ADAPTERS};
use crate::error::{KernelError, Result};
use crate::message::{line_to_object, NewMessage};
use crate::registry::{KernelModule, KernelModuleDescriptor, NodeKind, KERNEL_MODULES};
use crate::substrate::Kernel;

/// Adapter + registry-module name.
pub const ADAPTER_NAME: &str = "adapter_claude_code";

/// The agent this adapter captures.
pub const AGENT_NAME: &str = "claude_code";

/// Parameter (on this adapter's registry entity) overriding the
/// projects root: one path, or a list of paths when Claude Code also
/// runs under another `CLAUDE_CONFIG_DIR`. Tests inject fixture dirs
/// through this — no env hacks.
pub const PROJECTS_ROOT_PARAM: &str = "attr_claude_code_projects_root";

/// Cursor type uid for transcript read positions.
pub const CURSOR_TYPE: &str = "claude_code_transcript";

/// Cursor type uid for the files beside the transcripts: each file's size
/// and modification time when it was last captured.
pub const FILES_CURSOR_TYPE: &str = "claude_code_files";

/// The folder under a session's own folder that holds its subagents'
/// transcripts.
// skill-allow: §9-const — agent-format knowledge: where Claude Code writes a subagent's transcript
const SUBAGENTS_DIR: &str = "subagents";

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
        let mut out = Vec::new();
        for (i, root) in self.projects_roots(kernel).await?.iter().enumerate() {
            if !root.is_dir() {
                continue; // that config dir is not on this machine
            }
            let entries = std::fs::read_dir(root).map_err(|e| {
                KernelError::Module(format!("cannot read projects root {}: {e}", root.display()))
            })?;
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let locator = path.to_string_lossy().to_string();
                    // Two config dirs hold the same project folder name
                    // (both have `-Users-…-gryphon-data-lake`), and a
                    // source is known by its name. The first root keeps
                    // the names its sources already carry; a later
                    // root's are named by their full path, so each gets
                    // its own source and read position.
                    let name = if i == 0 {
                        entry.file_name().to_string_lossy().to_string()
                    } else {
                        locator.clone()
                    };
                    out.push(DiscoveredSource { name, locator });
                }
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
        events += self.capture_files(kernel, source, &dir, &files).await?;
        Ok(events)
    }
}

impl ClaudeCodeAdapter {
    /// Resolve the projects roots: the substrate parameter if set (one
    /// path, or a list), else `$HOME/.claude/projects`. A root that does
    /// not exist is skipped by discovery (agent absent there).
    async fn projects_roots(&self, kernel: &Kernel) -> Result<Vec<PathBuf>> {
        if let Some(entity) = kernel
            .find_module_by_name(NodeKind::Adapter, ADAPTER_NAME)
            .await?
        {
            match kernel.get_parameter(entity, PROJECTS_ROOT_PARAM).await? {
                Some(Value::String(p)) => return Ok(vec![PathBuf::from(p)]),
                Some(Value::Array(paths)) => {
                    return paths
                        .iter()
                        .map(|v| match v {
                            Value::String(p) => Ok(PathBuf::from(p)),
                            other => Err(KernelError::Config(format!(
                                "{PROJECTS_ROOT_PARAM}: every entry must be a path, got {other:?}"
                            ))),
                        })
                        .collect();
                }
                _ => {}
            }
        }
        // Default location is format knowledge this adapter owns.
        // skill-allow: §9-default — agent-format default; operator overrides via the parameter
        let home = std::env::var("HOME").map_err(|_| {
            KernelError::Config("HOME is not set; cannot locate ~/.claude/projects".into())
        })?;
        Ok(vec![Path::new(&home).join(".claude").join("projects")])
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
        // The transcript FILE is `<sessionId>.jsonl`, or sits under
        // `<sessionId>/subagents/`: its path attributes lines that don't
        // carry a sessionId themselves (sidecars, a workflow's journal,
        // unparseable lines). Nothing is ever "unknown" (issues #186/#204).
        let file_session = session_of_file(file);
        let Ok(json) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            let mut payload = Object::new();
            payload.insert(
                "file".to_string(),
                Value::String(file.to_string_lossy().to_string()),
            );
            payload.insert("session".to_string(), Value::String(file_session));
            let snippet: String = trimmed.chars().take(RAW_SNIPPET_MAX).collect();
            payload.insert("snippet".to_string(), Value::String(snippet));
            // The whole line, whatever its length: nothing is dropped (#471).
            payload.insert("text".to_string(), Value::String(trimmed.to_string()));
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
                        raw: Some(line_to_object(&json)),
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
                // The whole line (#471): the system prompt and the context
                // sent with each request, attachments, turn durations, away
                // summaries. These were kept as their kind alone.
                payload.insert("line".to_string(), Value::Object(line_to_object(&json)));
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

    /// Capture every file under the project folder that is not a
    /// transcript, whole, whenever its size or modification time changed
    /// since it was last captured (#520): a saved tool output, a subagent's
    /// description, a workflow, a memory file. Each is a `transcript_event`
    /// of kind `file`, so the session view and the export show it in place.
    async fn capture_files(
        &self,
        kernel: &Kernel,
        source: &SourceRef,
        dir: &Path,
        transcripts: &[PathBuf],
    ) -> Result<u64> {
        kernel
            .ensure_cursor_type(
                FILES_CURSOR_TYPE,
                "telemetry",
                "Claude Code files beside the transcripts (size and modification time of each, when captured)",
            )
            .await?;
        let prior = kernel
            .latest_cursor(source.entity_id.clone(), FILES_CURSOR_TYPE)
            .await?;
        let mut stamps = prior
            .as_ref()
            .and_then(|c| c.metadata.as_ref())
            .map(read_stamps)
            .unwrap_or_default();
        let mut listed = BTreeSet::new();
        let mut events: u64 = 0;
        for file in side_files(dir, transcripts) {
            let key = file.to_string_lossy().to_string();
            listed.insert(key.clone());
            let Ok(meta) = std::fs::metadata(&file) else {
                continue; // gone since it was listed
            };
            let stamp = stamp_of(&meta);
            if stamps.get(&key) == Some(&stamp) {
                continue;
            }
            let bytes = match std::fs::read(&file) {
                Ok(bytes) => bytes,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(KernelError::Module(format!("read {}: {e}", file.display()))),
            };
            let mut line = Object::new();
            line.insert("type".to_string(), Value::String("file".to_string()));
            line.insert(
                "path".to_string(),
                Value::String(file.strip_prefix(dir).unwrap_or(&file).to_string_lossy().to_string()),
            );
            line.insert(
                "bytes".to_string(),
                Value::Number(i64::try_from(bytes.len()).unwrap_or(i64::MAX).into()),
            );
            if let Ok(modified) = meta.modified() {
                line.insert("modified".to_string(), Value::Datetime(DateTime::<Utc>::from(modified).into()));
            }
            let content = match String::from_utf8(bytes) {
                Ok(text) => Value::String(text),
                Err(e) => Value::Bytes(e.into_bytes().into()),
            };
            line.insert("content".to_string(), content);
            let mut payload = Object::new();
            payload.insert("kind".to_string(), Value::String("file".to_string()));
            if let Some(session) = session_of_side_file(dir, &file) {
                payload.insert("session".to_string(), Value::String(session));
            }
            payload.insert("file".to_string(), Value::String(key.clone()));
            payload.insert("line".to_string(), Value::Object(line));
            kernel
                .log_telemetry_for_agent(
                    "transcript_event",
                    Value::Object(payload),
                    Some(source.entity_id.clone()),
                    Some(source.agent_id.clone()),
                )
                .await?;
            stamps.insert(key, stamp);
            events += 1;
        }
        if events > 0 {
            stamps.retain(|path, _| listed.contains(path));
            let mut files = Object::new();
            for (path, (bytes, modified)) in &stamps {
                let stamp = vec![
                    Value::Number(i64::try_from(*bytes).unwrap_or(i64::MAX).into()),
                    Value::Number((*modified).into()),
                ];
                files.insert(path.clone(), Value::Array(surrealdb::types::Array::from(stamp)));
            }
            let mut metadata = Object::new();
            metadata.insert("files".to_string(), Value::Object(files));
            kernel
                .write_cursor(
                    source.entity_id.clone(),
                    FILES_CURSOR_TYPE,
                    Some(events.to_string()),
                    Some(metadata),
                )
                .await?;
        }
        Ok(events)
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

/// The transcripts of a project directory, sorted: each `*.jsonl` at its
/// top level, and each `*.jsonl` under a session's `subagents/` folder at
/// any depth (a subagent's own transcript, a workflow agent's, a workflow's
/// journal). A vanished directory yields an empty list — discovery owns
/// forgetting sources.
fn transcript_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut files = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            jsonl_under(&path.join(SUBAGENTS_DIR), &mut files);
        } else if is_jsonl(&path) {
            files.push(path);
        }
    }
    files.sort();
    files
}

/// Every `*.jsonl` under `dir`, at any depth. A folder linked from
/// elsewhere is not followed.
fn jsonl_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            jsonl_under(&path, out);
        } else if is_jsonl(&path) {
            out.push(path);
        }
    }
}

/// Every regular file under the project folder that is not one of its
/// transcripts, sorted. A folder or file linked from elsewhere is not
/// followed.
fn side_files(dir: &Path, transcripts: &[PathBuf]) -> Vec<PathBuf> {
    let transcripts: BTreeSet<&PathBuf> = transcripts.iter().collect();
    let mut out = Vec::new();
    files_under(dir, &mut out);
    out.retain(|file| !transcripts.contains(file));
    out.sort();
    out
}

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(t) if t.is_dir() => files_under(&path, out),
            Ok(t) if t.is_file() => out.push(path),
            _ => {}
        }
    }
}

/// The session a file beside the transcripts belongs to: the one whose
/// folder holds it, when `<sessionId>.jsonl` sits beside that folder. A
/// file anywhere else, such as `memory/`, is the project's.
fn session_of_side_file(dir: &Path, file: &Path) -> Option<String> {
    let mut parts = file.strip_prefix(dir).ok()?.components();
    let folder = parts.next()?.as_os_str().to_string_lossy().to_string();
    parts.next()?;
    dir.join(format!("{folder}.jsonl")).is_file().then_some(folder)
}

/// A file's size and modification time, in nanoseconds since the epoch.
fn stamp_of(meta: &std::fs::Metadata) -> (u64, i64) {
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX));
    (meta.len(), modified)
}

fn read_stamps(metadata: &Object) -> BTreeMap<String, (u64, i64)> {
    let mut out = BTreeMap::new();
    if let Some(Value::Object(files)) = metadata.get("files") {
        for (path, stamp) in files.iter() {
            if let Value::Array(a) = stamp {
                if let [Value::Number(bytes), Value::Number(modified)] = a.as_slice() {
                    if let (Some(bytes), Some(modified)) = (bytes.to_int(), modified.to_int()) {
                        out.insert(path.clone(), (u64::try_from(bytes).unwrap_or(0), modified));
                    }
                }
            }
        }
    }
    out
}

fn is_jsonl(path: &Path) -> bool {
    path.is_file() && path.extension().is_some_and(|e| e == "jsonl")
}

/// The session a transcript belongs to: the session folder that holds the
/// `subagents/` folder it sits under, else the file's own name.
fn session_of_file(file: &Path) -> String {
    let parts: Vec<&std::ffi::OsStr> = file.iter().collect();
    match parts.iter().rposition(|p| *p == SUBAGENTS_DIR) {
        Some(i) if i > 0 => parts[i - 1].to_string_lossy().to_string(),
        _ => file
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown-session".to_string()),
    }
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
