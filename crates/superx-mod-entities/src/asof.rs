//! Reading the past (§14).
//!
//! Per-chain history answers *"how did this text change"*. A
//! whole-entity as-of read answers **"what did the agent see when it
//! did that"** — the question after a bad run, and one that cannot be
//! assembled from separate pickers because each moves independently.
//!
//! The rule is the same everywhere: `valid_from <= as_of`, latest wins
//! per chain, edges as they were active then. Every chain reduction in
//! this module already happens in Rust over an ascending scan, so an
//! instant is a FILTER APPLIED BEFORE THE REDUCTION rather than a
//! second query shape — which is why the same read serves both and
//! cannot drift from it.

use superx_kernel::{KernelError, Result};

/// The instant a read is taken at. `None` is now.
pub type AsOf = Option<chrono::DateTime<chrono::Utc>>;

/// Parse an operator-supplied instant.
///
/// # Errors
///
/// [`KernelError::Module`] when it is not RFC3339.
pub fn parse(raw: Option<&str>) -> Result<AsOf> {
    let Some(raw) = raw else { return Ok(None) };
    chrono::DateTime::parse_from_rfc3339(raw)
        .map(|t| Some(t.with_timezone(&chrono::Utc)))
        .map_err(|e| {
            KernelError::Module(format!(
                "'{raw}' is not an instant this can read at ({e}) — \
                 use RFC3339, e.g. 2026-08-24T17:00:00Z"
            ))
        })
}

/// Was this version written at or before the instant?
///
/// Timestamps are compared as PARSED datetimes, never lexically: a
/// fractionless `…06Z` against `…06.5Z` inverts under string compare
/// (`Z` > `.`), and this decides which version a reader is handed.
#[must_use]
pub fn visible(valid_from: &str, as_of: AsOf) -> bool {
    let Some(instant) = as_of else { return true };
    match chrono::DateTime::parse_from_rfc3339(valid_from) {
        Ok(t) => t.with_timezone(&chrono::Utc) <= instant,
        // Unparseable is treated as NOT visible in a historical read.
        // Guessing it is old enough would silently put a row into a
        // reconstruction that may not have existed then, and the whole
        // point of the read is that it is exact.
        Err(_) => false,
    }
}

/// The same question for a chain that already holds a parsed instant —
/// `note` and `attachment` do. No reparse, and no round-trip through a
/// string that could reformat.
///
/// A row with NO `valid_from` is not visible in a historical read, for
/// the same reason an unparseable one is not: the read is exact or it
/// is worthless.
#[must_use]
pub fn visible_at(valid_from: Option<chrono::DateTime<chrono::Utc>>, as_of: AsOf) -> bool {
    let Some(instant) = as_of else { return true };
    valid_from.is_some_and(|t| t <= instant)
}
