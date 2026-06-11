//! The Claude Code transcript watcher — the driver's capture half.
//!
//! Each discovered source is one project directory holding
//! `<session-uuid>.jsonl` transcripts. The watcher tails every
//! transcript in the directory:
//!
//! - **Capture-from-now:** first contact records each file's current
//!   length in the cursor and emits nothing (like `tail -f`).
//! - Subsequent polls read each file from its recorded offset to the
//!   last complete newline, emit one `transcript_event` per parseable
//!   JSONL line (`{agent, source, file, event_type, uuid?, session?}`)
//!   and one `transcript_raw` per unparseable line — drift in Claude
//!   Code's format degrades to raw capture, never to a crash.
//! - New files appearing mid-stream are captured from offset 0 (they
//!   are new activity). A file shorter than its recorded offset was
//!   rotated/replaced — re-captured from 0.
//! - The cursor (`cursor_type = "claude_code_transcript"`, subject =
//!   the source entity) stores per-file byte offsets in `metadata`
//!   and the event count in `last_processed`. Append-only like
//!   everything else; restart resumes exactly.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use linkme::distributed_slice;
use superx_kernel::types::{Object, Value};
use superx_kernel::{Kernel, KernelError, Result};
use superx_kernel_capture::{CaptureSource, SourceRef, CAPTURE_SOURCES};

use crate::DRIVER_NAME;

/// Cursor type uid for transcript read positions.
pub const CURSOR_TYPE: &str = "claude_code_transcript";

/// How much of an unparseable line the `transcript_raw` payload
/// carries (enough to debug format drift without flooding the
/// substrate). Parameterization (`attr_capture_raw_snippet_max`) is
/// flagged as post-FVP work in docs/ROADMAP.md.
// skill-allow: §9-const — safety truncation bound; parameterize post-FVP
const RAW_SNIPPET_MAX: usize = 500;

pub struct TranscriptWatcher;

#[async_trait]
impl CaptureSource for TranscriptWatcher {
    fn probe_name(&self) -> &'static str {
        DRIVER_NAME
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
        let files = transcript_files(&dir)?;

        let prior = kernel
            .latest_cursor(source.entity_id.clone(), CURSOR_TYPE)
            .await?;

        // First contact: checkpoint current ends, capture nothing.
        let Some(prior) = prior else {
            let mut offsets = BTreeMap::new();
            for f in &files {
                offsets.insert(file_key(f), f.metadata().map(|m| m.len()).unwrap_or(0));
            }
            write_checkpoint(kernel, source, &offsets, 0).await?;
            return Ok(0);
        };

        let mut offsets = read_offsets(prior.metadata.as_ref());
        let mut events: u64 = 0;
        let mut changed = false;

        for file in &files {
            let key = file_key(file);
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
                emit_line(kernel, source, &key, &line).await?;
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

#[distributed_slice(CAPTURE_SOURCES)]
static WATCHER_REGISTRATION: &'static (dyn CaptureSource + Sync) = &TranscriptWatcher;

/// `*.jsonl` files in the project directory, sorted for determinism.
fn transcript_files(dir: &Path) -> Result<Vec<PathBuf>> {
    if !dir.is_dir() {
        // Source directory vanished — nothing to capture this tick;
        // discovery owns forgetting sources.
        return Ok(vec![]);
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| KernelError::Module(format!("cannot read {}: {e}", dir.display())))?
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "jsonl"))
        .collect();
    files.sort();
    Ok(files)
}

fn file_key(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Read from `offset` up to the last complete newline. Returns the
/// complete lines and how many bytes they consumed (a trailing
/// partial line stays unread for the next tick).
fn read_complete_lines(path: &Path, offset: u64) -> Result<(Vec<String>, u64)> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| KernelError::Module(format!("cannot open {}: {e}", path.display())))?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| KernelError::Module(format!("cannot seek {}: {e}", path.display())))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .map_err(|e| KernelError::Module(format!("cannot read {}: {e}", path.display())))?;

    let Some(last_newline) = buf.iter().rposition(|&b| b == b'\n') else {
        return Ok((vec![], 0));
    };
    let consumed = (last_newline + 1) as u64;
    let lines = String::from_utf8_lossy(&buf[..last_newline])
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect();
    Ok((lines, consumed))
}

/// One transcript line → one telemetry event. Parseable JSON becomes
/// a structured `transcript_event`; anything else degrades to
/// `transcript_raw` — format drift is captured, never fatal.
async fn emit_line(kernel: &Kernel, source: &SourceRef, file: &str, line: &str) -> Result<()> {
    let mut payload = Object::new();
    payload.insert("agent".to_string(), Value::String(source.agent.clone()));
    payload.insert("source".to_string(), Value::String(source.locator.clone()));
    payload.insert("file".to_string(), Value::String(file.to_string()));

    match serde_json::from_str::<serde_json::Value>(line) {
        Ok(v) => {
            let event_type = v
                .get("type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            payload.insert("event_type".to_string(), Value::String(event_type.to_string()));
            if let Some(uuid) = v.get("uuid").and_then(serde_json::Value::as_str) {
                payload.insert("uuid".to_string(), Value::String(uuid.to_string()));
            }
            if let Some(session) = v.get("sessionId").and_then(serde_json::Value::as_str) {
                payload.insert("session".to_string(), Value::String(session.to_string()));
            }
            kernel
                .log_telemetry("transcript_event", Value::Object(payload), None)
                .await?;
        }
        Err(_) => {
            let snippet: String = line.chars().take(RAW_SNIPPET_MAX).collect();
            payload.insert("line".to_string(), Value::String(snippet));
            kernel
                .log_telemetry("transcript_raw", Value::Object(payload), None)
                .await?;
        }
    }
    Ok(())
}

fn read_offsets(metadata: Option<&Object>) -> BTreeMap<String, u64> {
    let mut out = BTreeMap::new();
    let Some(meta) = metadata else { return out };
    let Some(Value::Object(files)) = meta.get("files") else {
        return out;
    };
    for (k, v) in files.iter() {
        if let Value::Number(n) = v {
            out.insert(
                k.clone(),
                u64::try_from(n.to_int().unwrap_or(0)).unwrap_or(0),
            );
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
        files.insert(k.clone(), Value::Number(i64::try_from(*v).unwrap_or(i64::MAX).into()));
    }
    let mut meta = Object::new();
    meta.insert("files".to_string(), Value::Object(files));
    kernel
        .write_cursor(
            source.entity_id.clone(),
            CURSOR_TYPE,
            Some(format!("events:{events}")),
            Some(meta),
        )
        .await?;
    Ok(())
}
