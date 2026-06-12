//! Telemetry primitive — `log_telemetry` + `recent_telemetry`.
//!
//! Every kernel mutation by every layer emits a typed event through
//! this verb (SKILL.md §8). The substrate's `telemetry_stream` table
//! is the OS's append-only audit log. Subscribers (capture loop,
//! emission, future `superx kernel stats --live`) consume via
//! LIVE SELECT.
//!
//! Reads (`recent_telemetry`) emit NO telemetry. SELECT is not a
//! mutation.

use chrono::{DateTime, Utc};
use surrealdb::engine::any::Any;
use surrealdb::types::{RecordId, SurrealValue, Value};
use surrealdb::Surreal;

use crate::error::Result;
use crate::substrate::Kernel;

/// One row read back from `telemetry_stream`. Returned in newest-first
/// order by [`Kernel::recent_telemetry`].
#[derive(Debug, Clone, SurrealValue)]
pub struct TelemetryRecord {
    pub id: RecordId,
    pub lifecycle_event: String,
    pub payload: Value,
    pub run: Option<RecordId>,
    pub valid_from: DateTime<Utc>,
}

impl Kernel {
    /// Append one row to `telemetry_stream` with an explicit UUIDv7
    /// id (§11). Returns the new row's id.
    ///
    /// `event` is a short lifecycle-event name (e.g. `"system_boot"`,
    /// `"module_starting"`, `"entity_created"`). `payload` is an
    /// open-shape object (the schema's `payload` field is
    /// `FLEXIBLE TYPE object`). `run` is `Some(node_run_id)` for
    /// events with a workflow context, `None` for system-level
    /// events (bootstrap, kernel boot, etc.).
    ///
    /// # Errors
    ///
    /// Surfaces engine refusals verbatim via [`crate::KernelError::Db`].
    pub async fn log_telemetry(
        &self,
        event: &str,
        payload: Value,
        run: Option<RecordId>,
    ) -> Result<RecordId> {
        let id = self.new_record_id("telemetry_stream");
        log_telemetry_inner(self.db(), id.clone(), event, payload, run).await?;
        Ok(id)
    }

    /// Read the most-recent `telemetry_stream` rows, newest first.
    /// Pure SELECT — no mutation, no telemetry emission.
    ///
    /// # Errors
    ///
    /// Surfaces engine errors verbatim via [`crate::KernelError::Db`].
    pub async fn recent_telemetry(&self, limit: u32) -> Result<Vec<TelemetryRecord>> {
        let rows: Vec<TelemetryRecord> = self
            .db()
            .query(
                "SELECT * FROM telemetry_stream \
                 ORDER BY valid_from DESC \
                 LIMIT $limit",
            )
            .bind(("limit", limit))
            .await?
            .take(0)?;
        Ok(rows)
    }

    /// Read `telemetry_stream` rows strictly newer than `after`,
    /// oldest first (the natural rendering order for a live tail),
    /// bounded by `limit`. The newer-than-last-seen primitive behind
    /// `superx kernel stats --live`. Pure SELECT — no mutation, no
    /// telemetry emission.
    ///
    /// # Errors
    ///
    /// Surfaces engine errors verbatim via [`crate::KernelError::Db`].
    pub async fn telemetry_since(
        &self,
        after: DateTime<Utc>,
        limit: u32,
    ) -> Result<Vec<TelemetryRecord>> {
        let rows: Vec<TelemetryRecord> = self
            .db()
            .query(
                "SELECT * FROM telemetry_stream \
                 WHERE valid_from > $after \
                 ORDER BY valid_from ASC \
                 LIMIT $limit",
            )
            .bind(("after", after))
            .bind(("limit", limit))
            .await?
            .take(0)?;
        Ok(rows)
    }
}

/// Shared internal: write one row to telemetry_stream with the
/// given id. Today only `Kernel::log_telemetry` goes through here;
/// kept as the single write-shape chokepoint for any future
/// crate-internal emitter.
pub(crate) async fn log_telemetry_inner(
    db: &Surreal<Any>,
    id: RecordId,
    event: &str,
    payload: Value,
    run: Option<RecordId>,
) -> Result<()> {
    let row = TelemetryRow {
        lifecycle_event: event.to_string(),
        payload,
        run,
        valid_from: Utc::now(),
    };
    let _: Option<TelemetryRow> = db.create(id).content(row).await?;
    Ok(())
}

#[derive(Debug, SurrealValue)]
struct TelemetryRow {
    lifecycle_event: String,
    payload: Value,
    run: Option<RecordId>,
    valid_from: DateTime<Utc>,
}
