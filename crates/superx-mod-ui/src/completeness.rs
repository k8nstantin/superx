//! Is every transcript line captured, whole? (#489)
//!
//! Capture is what everything else stands on. [`crate::health`] tells from
//! timing whether it keeps up; this checks it, transcript by transcript,
//! for the Claude Code sessions written to lately:
//!
//! - **read**: the bytes a transcript holds against the bytes capture has
//!   read from it, by the adapter's own read position (its cursor), and how
//!   long a complete line has waited unread;
//! - **every line a row**: since this server began watching a transcript,
//!   the lines capture read from it against the rows it wrote for them.
//!   The adapter writes a line's row before it moves its cursor past the
//!   line, so a line read without a row is a line lost;
//! - **whole**: no line kept by its kind alone, as lines were before #471.
//!
//! Read-only: a transcript is read as bytes from where the last count
//! stopped, nothing in it is parsed, and the substrate is read through its
//! indexes for the time since watching began.

use std::collections::{HashMap, HashSet};
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::Path;

use chrono::{DateTime, Utc};
use superx_kernel::adapters::claude_code::{ADAPTER_NAME, CURSOR_TYPE};
use superx_kernel::types::{RecordId, Value};
use superx_kernel::{Kernel, Result};

use crate::api::CaptureFile;

/// How recently a transcript must have changed to be watched, in seconds.
pub const WATCH_SECS_PARAM: &str = "attr_ui_capture_watch_secs";

/// Where it stands when the parameter is unset.
pub const DEFAULT_WATCH_SECS: u64 = 3_600; // skill-allow: §9-const — bootstrap fallback, param-overridable

/// The agent whose transcripts this reads, as the adapter names it.
const AGENT: &str = "claude_code";

/// What this server has counted of each watched transcript, across polls.
#[derive(Debug, Default)]
pub struct Watch {
    files: HashMap<String, Counted>,
}

#[derive(Debug)]
struct Counted {
    /// When capture's cursor last moved before watching began: every row
    /// written after it is for a line past where the count starts.
    since: DateTime<Utc>,
    /// How far the transcript has been counted, and the lines in it that
    /// capture keeps — a blank line it skips.
    counted_to: u64,
    lines: u64,
    /// The rows written for the transcript since then, by id, and the
    /// newest one's time.
    rows: HashSet<String>,
    newest: DateTime<Utc>,
    kind_only: u64,
}

/// A watched transcript: its session, and where it is on disk.
struct Transcript {
    session: RecordId,
    name: String,
    path: String,
}

/// Check every Claude Code transcript changed in the last `window_secs`,
/// counting on from where `watch` stopped.
///
/// # Errors
///
/// The engine's, or a read that got no answer, twice.
pub async fn check(kernel: &Kernel, watch: &mut Watch, now: DateTime<Utc>, window_secs: u64) -> Result<Vec<CaptureFile>> {
    let window = chrono::Duration::seconds(i64::try_from(window_secs).unwrap_or(i64::MAX));
    let live = live_transcripts(kernel, now, window).await?;
    watch.files.retain(|path, _| live.iter().any(|t| &t.path == path));
    if live.is_empty() {
        return Ok(Vec::new());
    }
    let read_to = read_positions(kernel).await?;
    let Some(agent) = crate::answered("capture agent", || {
        kernel.find_entity_by_name("node_agent", "attr_agent_descriptor", AGENT)
    })
    .await?
    else {
        return Ok(Vec::new());
    };

    let mut out = Vec::with_capacity(live.len());
    for t in live {
        let path = Path::new(&t.path);
        let Ok(meta) = std::fs::metadata(path) else { continue };
        let bytes = meta.len();
        let (read, moved) = read_to.get(&t.path).copied().unwrap_or((0, now));
        // A transcript replaced or cut short is counted from the start again.
        let restart = watch.files.get(&t.path).is_some_and(|c| read < c.counted_to);
        if restart {
            watch.files.remove(&t.path);
        }
        let counted = watch.files.entry(t.path.clone()).or_insert_with(|| Counted {
            since: moved,
            counted_to: read,
            lines: 0,
            rows: HashSet::new(),
            newest: moved,
            kind_only: 0,
        });
        counted.lines += kept_lines(path, counted.counted_to, read);
        counted.counted_to = read;
        count_rows(kernel, &t, &agent, counted).await?;

        // A complete line written at the latest when the file last changed.
        let unread_since =
            if unread_line(path, read, bytes) { meta.modified().ok().map(DateTime::<Utc>::from) } else { None };
        out.push(CaptureFile {
            session: t.name,
            since: counted.since.to_rfc3339(),
            bytes: i64::try_from(bytes).unwrap_or(i64::MAX),
            read: i64::try_from(read).unwrap_or(i64::MAX),
            unread_secs: unread_since.map(|at| (now - at).num_seconds().max(0)),
            lines: i64::try_from(counted.lines).unwrap_or(i64::MAX),
            rows: i64::try_from(counted.rows.len()).unwrap_or(i64::MAX),
            kind_only: i64::try_from(counted.kind_only).unwrap_or(i64::MAX),
        });
    }
    out.sort_by(|a, b| a.session.cmp(&b.session));
    Ok(out)
}

/// The Claude Code sessions whose transcript changed in the window.
async fn live_transcripts(kernel: &Kernel, now: DateTime<Utc>, window: chrono::Duration) -> Result<Vec<Transcript>> {
    let sessions =
        crate::answered("sessions", || kernel.list_named_entities("node_session", "attr_session_descriptor")).await?;
    let prefix = format!("{AGENT}/");
    Ok(sessions
        .into_iter()
        .filter_map(|s| {
            let Value::Object(o) = &s.payload else { return None };
            let (Some(Value::String(name)), Some(Value::String(path))) = (o.get("name"), o.get("locator")) else {
                return None;
            };
            let changed = DateTime::<Utc>::from(std::fs::metadata(path).ok()?.modified().ok()?);
            (name.starts_with(&prefix) && now - changed <= window)
                .then(|| Transcript { session: s.entity_id.clone(), name: name.clone(), path: path.clone() })
        })
        .collect())
}

/// Where capture has read each transcript to, and when its cursor last
/// moved: the adapter's checkpoint for every source it reads,
/// `{"files": {path: offset}}`.
async fn read_positions(kernel: &Kernel) -> Result<HashMap<String, (u64, DateTime<Utc>)>> {
    let sources =
        crate::answered("sources", || kernel.list_named_entities("node_source", "attr_source_descriptor")).await?;
    let mut out = HashMap::new();
    for s in sources {
        let Value::Object(o) = &s.payload else { continue };
        if !matches!(o.get("adapter"), Some(Value::String(a)) if a == ADAPTER_NAME) {
            continue;
        }
        let id = s.entity_id.clone();
        let Some(cursor) = crate::answered("read position", || kernel.latest_cursor(id.clone(), CURSOR_TYPE)).await?
        else {
            continue;
        };
        let Some(Value::Object(files)) = cursor.metadata.as_ref().and_then(|m| m.get("files")) else { continue };
        for (path, offset) in files.iter() {
            if let Value::Number(n) = offset {
                if let Some(i) = n.to_int() {
                    out.insert(path.clone(), (u64::try_from(i).unwrap_or(0), cursor.valid_from));
                }
            }
        }
    }
    Ok(out)
}

/// The lines between two offsets that capture keeps: a blank one it
/// skips. `to` ends a line, as capture's read position always does.
fn kept_lines(path: &Path, from: u64, to: u64) -> u64 {
    if to <= from {
        return 0;
    }
    let Ok(mut f) = std::fs::File::open(path) else { return 0 };
    if f.seek(SeekFrom::Start(from)).is_err() {
        return 0;
    }
    let mut buf = Vec::new();
    if f.take(to - from).read_to_end(&mut buf).is_err() {
        return 0;
    }
    buf.split(|&b| b == b'\n').filter(|l| !l.trim_ascii().is_empty()).count() as u64
}

/// Whether a complete line waits past capture's read position: a partial
/// one is still being written, and capture waits for it by design.
fn unread_line(path: &Path, read: u64, bytes: u64) -> bool {
    if bytes <= read {
        return false;
    }
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    if f.seek(SeekFrom::Start(read)).is_err() {
        return false;
    }
    let mut buf = Vec::new();
    f.take(bytes - read).read_to_end(&mut buf).is_ok() && buf.contains(&b'\n')
}

/// The rows written for a transcript since watching began: its session's
/// messages, and its other lines' events. Read from the second the last
/// read reached, whole — the time index keeps the rows within a second in
/// the wrong order (#485) — and counted once each, by id.
async fn count_rows(kernel: &Kernel, t: &Transcript, agent: &RecordId, counted: &mut Counted) -> Result<()> {
    let from = DateTime::<Utc>::from_timestamp(counted.newest.timestamp(), 0).unwrap_or(counted.newest)
        - chrono::Duration::microseconds(1);
    let messages: Vec<Value> = crate::answered("transcript rows", || {
        let session = t.session.clone();
        async move {
            Ok(kernel
                .db()
                .query("SELECT id, valid_from FROM message WHERE session = $sess AND valid_from >= $from")
                .bind(("sess", session))
                .bind(("from", from))
                .await?
                .take(0)?)
        }
    })
    .await?;
    let lines: Vec<Value> = crate::answered("transcript rows", || {
        let (agent, file) = (agent.clone(), t.path.clone());
        async move {
            Ok(kernel
                .db()
                .query(
                    "SELECT id, valid_from, lifecycle_event, payload.line != NONE AS whole FROM telemetry_stream \
                     WHERE agent = $agent AND valid_from >= $from \
                     AND (lifecycle_event = 'transcript_event' OR lifecycle_event = 'transcript_raw') \
                     AND payload.file = $file",
                )
                .bind(("agent", agent))
                .bind(("from", from))
                .bind(("file", file))
                .await?
                .take(0)?)
        }
    })
    .await?;
    for row in messages.iter().chain(lines.iter()) {
        let Value::Object(o) = row else { continue };
        let (Some(Value::RecordId(id)), Some(Value::Datetime(at))) = (o.get("id"), o.get("valid_from")) else {
            continue;
        };
        let at = DateTime::<Utc>::from(*at);
        if at <= counted.since || !counted.rows.insert(superx_ops::record_uuid(id)) {
            continue;
        }
        counted.newest = counted.newest.max(at);
        let kind_only = matches!(o.get("lifecycle_event"), Some(Value::String(e)) if e == "transcript_event")
            && matches!(o.get("whole"), Some(Value::Bool(false)));
        if kind_only {
            counted.kind_only += 1;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(text: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("superx-completeness-{}.jsonl", superx_kernel::types::Uuid::new_v7()));
        std::fs::write(&path, text).expect("write");
        path
    }

    #[test]
    fn a_blank_line_is_not_a_line_capture_keeps() {
        let path = scratch("{\"a\":1}\n\n  \n{\"b\":2}\r\n{\"c\":3}\n");
        let len = std::fs::metadata(&path).expect("meta").len();
        assert_eq!(kept_lines(&path, 0, len), 3);
        assert_eq!(kept_lines(&path, 8, len), 2, "from the second line on");
        assert_eq!(kept_lines(&path, len, len), 0);
        std::fs::remove_file(path).expect("scratch transcript removed");
    }

    #[test]
    fn only_a_complete_line_waits_unread() {
        let path = scratch("{\"a\":1}\n{\"b\":");
        let len = std::fs::metadata(&path).expect("meta").len();
        assert!(!unread_line(&path, 8, len), "a line still being written is not waiting");
        assert!(unread_line(&path, 0, len), "the first line is complete and unread");
        assert!(!unread_line(&path, len, len));
        std::fs::remove_file(path).expect("scratch transcript removed");
    }
}
