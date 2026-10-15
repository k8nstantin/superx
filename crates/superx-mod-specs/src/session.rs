//! A build session, as the kernel captured it: what it read and what it
//! wrote, in the order it did them.
//!
//! The kernel keeps each transcript line whole in `message.raw`
//! (adapter_claude_code). What this module reads out of it, mapped from
//! live transcripts on 2026-10-13:
//!
//! - a Read's result is a `tool` line whose `toolUseResult` is
//!   `{type: "text", file: {filePath, content, startLine, numLines,
//!   totalLines}}` — the file's own bytes, with no line numbers, and
//!   enough to tell a whole read from a part;
//! - its call id is the line's `message.content[0].tool_use_id`;
//! - a write is an assistant `tool_use` block named `Edit`, `MultiEdit`,
//!   `Write` or `NotebookEdit`, its path in `input.file_path` (a
//!   notebook's in `input.notebook_path`);
//! - every line carries `gitBranch`, and an assistant line its `model`.

use chrono::{DateTime, Utc};
use superx_kernel::types::Value;

/// One file read through the Read tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Read {
    pub path: String,
    /// From the first line to the last.
    pub whole: bool,
    /// The bytes read.
    pub content: Option<String>,
    pub tool_use_id: Option<String>,
    pub at: DateTime<Utc>,
}

/// One file written through a writing tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Write {
    pub path: String,
    pub tool_use_id: Option<String>,
}

/// What a session did, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Read(Read),
    Write(Write),
}

/// One captured line, reduced to what the checks use. The projection
/// that produces it is [`crate::store::TIMELINE`].
#[derive(Debug, Clone, Default)]
pub struct Row {
    pub role: String,
    pub at: Option<DateTime<Utc>>,
    pub branch: Option<String>,
    pub model: Option<String>,
    pub result_type: Option<String>,
    pub file: Option<Value>,
    pub tool_use_id: Option<String>,
    pub uses: Vec<Value>,
    /// What a shell call printed, on its result line.
    pub stdout: Option<String>,
}

/// The tools that write a file.
const WRITING_TOOLS: [&str; 4] = ["Edit", "MultiEdit", "Write", "NotebookEdit"];

/// The reads and writes of a session, in captured order.
#[must_use]
pub fn events(rows: &[Row]) -> Vec<Event> {
    let mut out = Vec::new();
    for row in rows {
        let Some(at) = row.at else { continue };
        if row.role == "tool" && row.result_type.as_deref() == Some("text") {
            if let Some(file) = &row.file {
                let Some(path) = get_str(file, "filePath") else { continue };
                let start = get_int(file, "startLine");
                let lines = get_int(file, "numLines");
                let total = get_int(file, "totalLines");
                let whole = start == Some(1) && lines.is_some() && lines == total;
                out.push(Event::Read(Read {
                    path,
                    whole,
                    content: get_str(file, "content"),
                    tool_use_id: row.tool_use_id.clone(),
                    at,
                }));
            }
            continue;
        }
        if row.role == "assistant" {
            for block in &row.uses {
                let Some(name) = get_str(block, "name") else { continue };
                if !WRITING_TOOLS.contains(&name.as_str()) {
                    continue;
                }
                let input = match block {
                    Value::Object(o) => o.get("input").cloned(),
                    _ => None,
                };
                let path = input
                    .as_ref()
                    .and_then(|i| get_str(i, "file_path").or_else(|| get_str(i, "notebook_path")));
                if let Some(path) = path {
                    out.push(Event::Write(Write { path, tool_use_id: get_str(block, "id") }));
                }
            }
        }
    }
    out
}

/// What a session wrote or printed as documents, in order: every Write
/// tool's content, every here-document a shell call wrote to a file, and
/// what each shell call printed — a build's PR body among them, however
/// it was made.
#[must_use]
pub fn documents(rows: &[Row]) -> Vec<String> {
    let mut out = Vec::new();
    for row in rows {
        if row.role == "tool" {
            out.extend(row.stdout.clone());
            continue;
        }
        for block in row.uses.iter() {
            let input = match block {
                Value::Object(o) => o.get("input").cloned(),
                _ => None,
            };
            let Some(input) = input else { continue };
            match get_str(block, "name").as_deref() {
                Some("Write") => out.extend(get_str(&input, "content")),
                Some("Bash") => {
                    if let Some(cmd) = get_str(&input, "command") {
                        out.extend(crate::git::written_files(&cmd).into_iter().map(|(_, body)| body));
                    }
                }
                _ => {}
            }
        }
    }
    out
}

/// A session's shell calls, in order.
#[must_use]
pub fn commands(rows: &[Row]) -> Vec<String> {
    rows.iter()
        .filter(|r| r.role == "assistant")
        .flat_map(|r| r.uses.iter())
        .filter(|b| get_str(b, "name").as_deref() == Some("Bash"))
        .filter_map(|b| match b {
            Value::Object(o) => o.get("input").and_then(|i| get_str(i, "command")),
            _ => None,
        })
        .collect()
}

/// What a session's shell calls printed, in order.
#[must_use]
pub fn outputs(rows: &[Row]) -> Vec<String> {
    rows.iter().filter_map(|r| r.stdout.clone()).collect()
}

/// The reads before the first write, and that write.
#[must_use]
pub fn before_first_write(events: &[Event]) -> (Vec<&Read>, Option<&Write>) {
    let mut reads = Vec::new();
    for e in events {
        match e {
            Event::Read(r) => reads.push(r),
            Event::Write(w) => return (reads, Some(w)),
        }
    }
    (reads, None)
}

/// The model that wrote most of the session's replies. Synthetic
/// replies (`<synthetic>`, written by the harness) are no model's.
#[must_use]
pub fn dominant_model(rows: &[Row]) -> Option<String> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for m in rows.iter().filter(|r| r.role == "assistant").filter_map(|r| r.model.as_deref()) {
        if m.starts_with('<') {
            continue;
        }
        match counts.iter_mut().find(|(name, _)| name == m) {
            Some((_, n)) => *n += 1,
            None => counts.push((m.to_string(), 1)),
        }
    }
    counts.into_iter().max_by_key(|(_, n)| *n).map(|(m, _)| m)
}

/// The build's branch and the label after its prefix. The root's method
/// `branch` step names it `<KEY>-<name>-v<file version>-<model>-<effort>`,
/// the name's spaces as hyphens; the label is what follows the version.
#[must_use]
pub fn build_branch(rows: &[Row], key: &str, name: &str, version: i64) -> Option<(String, String)> {
    let prefix = format!("{key}-{}-v{version}-", name.trim().replace(' ', "-"));
    rows.iter()
        .filter_map(|r| r.branch.as_deref())
        .find(|b| b.starts_with(&prefix) && b.len() > prefix.len())
        .map(|b| (b.to_string(), b[prefix.len()..].to_string()))
}

/// A string field of an object, or nothing.
#[must_use]
pub fn get_str(v: &Value, key: &str) -> Option<String> {
    match v {
        Value::Object(o) => match o.get(key) {
            Some(Value::String(s)) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// An integer field of an object, or nothing.
#[must_use]
pub fn get_int(v: &Value, key: &str) -> Option<i64> {
    match v {
        Value::Object(o) => match o.get(key) {
            Some(Value::Number(n)) => n.to_int(),
            _ => None,
        },
        _ => None,
    }
}
