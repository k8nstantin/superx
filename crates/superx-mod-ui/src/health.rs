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
//! A change newer than the newest record has not been taken in yet. That
//! is normal for the seconds capture takes; it is a stall when it lasts.
//! Those two facts alone cannot say how long it has lasted — a quiet hour
//! and one new line look the same as a dead capture under busy work — so
//! the server remembers, across its polls, since when a change has been
//! waiting (`waiting_since`), and calls it stalled once that passes
//! `attr_ui_capture_stall_secs`. It clears the moment capture catches up.
//! (2026-10-18: a 6-minute quiet spell, then a line taken in 81 s late
//! behind a hung connection, read as a 5-minute stall under the first
//! rule, which compared the two times directly.)

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

/// The two facts, as one read finds them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    /// When the OS last recorded anything.
    pub last_written: Option<DateTime<Utc>>,
    /// The newest transcript change, and whose.
    pub newest: Option<(DateTime<Utc>, String)>,
}

/// Judge the facts at `now`, given since when a change was already known
/// to be waiting. Returns the view and the new `waiting_since` to keep.
#[must_use]
pub fn judge(facts: &Facts, waiting_since: Option<DateTime<Utc>>, now: DateTime<Utc>, stall_secs: u64) -> (CaptureView, Option<DateTime<Utc>>) {
    let limit = chrono::Duration::seconds(i64::try_from(stall_secs).unwrap_or(i64::MAX));
    // A change newer than the newest record is waiting to be taken in.
    let waiting = match (&facts.newest, facts.last_written) {
        (Some((changed, _)), Some(written)) => (*changed > written).then_some(*changed),
        (Some((changed, _)), None) => Some(*changed),
        (None, _) => None,
    };
    // It has waited since the earliest such change seen and not yet taken.
    let since = waiting.map(|changed| waiting_since.map_or(changed, |s| s.min(changed)));
    let view = CaptureView {
        last_written: facts.last_written.map(|t| t.to_rfc3339()),
        transcripts_changed: facts.newest.as_ref().map(|(t, _)| t.to_rfc3339()),
        transcript: facts.newest.as_ref().map(|(_, name)| name.clone()),
        waiting_since: since.map(|t| t.to_rfc3339()),
        stall_secs: i64::try_from(stall_secs).unwrap_or(i64::MAX),
        stalled: since.is_some_and(|s| now - s > limit),
    };
    (view, since)
}

/// Read both facts from the running OS.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the substrate does not answer.
pub async fn read(kernel: &Kernel) -> Result<Facts> {
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
    Ok(Facts { last_written, newest: newest_change(&files) })
}
