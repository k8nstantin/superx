//! Is capture keeping up? (#452)
//!
//! Capture stops on its first error and does not come back (#368), and
//! the daemon's connection can stop answering for hours (#452). Either
//! way the Sessions page used to show every session idle, which looks the
//! same as nobody working. This says which it is.
//!
//! Two facts, both generic over agents:
//!
//! - when the OS last RECORDED anything: the newest `telemetry_stream`
//!   row (one indexed read — `idx_telemetry_valid_from`). Capture writes
//!   one for every transcript line it takes, sidecar lines included;
//! - when a session's transcript last CHANGED: the newest modification
//!   time among the sessions' transcript files (their `locator`), read
//!   from the filesystem's metadata only.
//!
//! When transcripts kept changing for longer than
//! `attr_ui_capture_stall_secs` after the OS last recorded anything,
//! capture is stalled.

use std::path::Path;

use chrono::{DateTime, Utc};
use superx_kernel::types::Value;
use superx_kernel::{Kernel, Result};

use crate::api::CaptureView;

/// How long transcripts may change past the newest record before the
/// page says capture is stalled, in seconds.
pub const STALL_SECS_PARAM: &str = "attr_ui_capture_stall_secs";

/// Where it stands when the parameter is unset.
pub const DEFAULT_STALL_SECS: u64 = 300; // skill-allow: §9-const — bootstrap fallback, param-overridable

/// The newest change among transcript files: when, and whose. A path
/// that is gone, or not a regular file, says nothing.
#[must_use]
pub fn newest_change(files: &[(String, std::path::PathBuf)]) -> Option<(DateTime<Utc>, String)> {
    files
        .iter()
        .filter_map(|(name, path)| {
            let meta = std::fs::metadata(path).ok()?;
            if !meta.is_file() {
                return None;
            }
            Some((DateTime::<Utc>::from(meta.modified().ok()?), name.clone()))
        })
        .max_by_key(|(at, _)| *at)
}

/// Judge the two facts.
#[must_use]
pub fn judge(last_written: Option<DateTime<Utc>>, newest: Option<(DateTime<Utc>, String)>, stall_secs: u64) -> CaptureView {
    let limit = chrono::Duration::seconds(i64::try_from(stall_secs).unwrap_or(i64::MAX));
    let stalled = match (&newest, last_written) {
        (Some((changed, _)), Some(written)) => *changed - written > limit,
        // Transcripts, and nothing ever recorded: nothing is being taken.
        (Some(_), None) => true,
        (None, _) => false,
    };
    CaptureView {
        last_written: last_written.map(|t| t.to_rfc3339()),
        transcripts_changed: newest.as_ref().map(|(t, _)| t.to_rfc3339()),
        transcript: newest.map(|(_, name)| name),
        stall_secs: i64::try_from(stall_secs).unwrap_or(i64::MAX),
        stalled,
    }
}

/// Read both facts from the running OS.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the substrate does not answer.
pub async fn read(kernel: &Kernel, stall_secs: u64) -> Result<CaptureView> {
    let rows: Vec<Value> = crate::answered("newest record", || async {
        Ok(kernel
            .db()
            .query("SELECT valid_from FROM telemetry_stream ORDER BY valid_from DESC LIMIT 1")
            .await?
            .take(0)?)
    })
    .await?;
    let last_written = rows.first().and_then(|r| match r {
        Value::Object(o) => match o.get("valid_from") {
            Some(Value::Datetime(d)) => Some(DateTime::<Utc>::from(*d)),
            _ => None,
        },
        _ => None,
    });
    let sessions =
        crate::answered("sessions", || kernel.list_named_entities("node_session", "attr_session_descriptor")).await?;
    let files: Vec<(String, std::path::PathBuf)> = sessions
        .iter()
        .filter_map(|s| match &s.payload {
            Value::Object(o) => match (o.get("name"), o.get("locator")) {
                (Some(Value::String(name)), Some(Value::String(locator))) => {
                    Some((name.clone(), Path::new(locator).to_path_buf()))
                }
                _ => None,
            },
            _ => None,
        })
        .collect();
    Ok(judge(last_written, newest_change(&files), stall_secs))
}
