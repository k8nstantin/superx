//! Telemetry primitive — `log_telemetry` + `recent_telemetry` +
//! `telemetry_since`.
//!
//! Every kernel mutation by every layer emits a typed event through
//! this verb. The substrate's `telemetry_stream` table is the OS's
//! append-only audit log; event rows are never superseded and
//! `valid_from` is the event timestamp (SUPERX_SCHEMA.md §3.7).
//!
//! Reads emit NO telemetry. SELECT is not a mutation.

use chrono::{DateTime, Utc};
use surrealdb::types::{RecordId, SurrealValue, Value};

use crate::error::Result;
use crate::substrate::Kernel;

/// One row read back from `telemetry_stream`.
#[derive(Debug, Clone, SurrealValue)]
pub struct TelemetryRecord {
    pub id: RecordId,
    pub lifecycle_event: String,
    pub payload: Value,
    /// Who emitted this event — a module, agent, or source entity;
    /// `None` for system-level events with no attributable emitter.
    pub subject: Option<RecordId>,
    pub valid_from: DateTime<Utc>,
}

impl Kernel {
    /// Append one row to `telemetry_stream` with an explicit UUIDv7
    /// id. Returns the new row's id.
    ///
    /// `event` is a short lifecycle-event name (e.g. `"system_boot"`,
    /// `"agent_discovered"`, `"transcript_event"`). `payload` is an
    /// open-shape value (the schema's `payload` field is `any` —
    /// tolerant by design). `subject` is the emitting entity, or
    /// `None` for system-level events.
    ///
    /// # Errors
    ///
    /// Surfaces engine refusals verbatim via [`crate::KernelError::Db`].
    pub async fn log_telemetry(
        &self,
        event: &str,
        payload: Value,
        subject: Option<RecordId>,
    ) -> Result<RecordId> {
        let id = self.new_record_id("telemetry_stream");
        let row = TelemetryRow {
            lifecycle_event: event.to_string(),
            payload,
            subject,
            valid_from: Utc::now(),
        };
        let _: Option<TelemetryRow> = self.db().create(id.clone()).content(row).await?;
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
    /// the future `superx stats --live` (G6). Pure SELECT — no
    /// mutation, no telemetry emission.
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

#[derive(Debug, SurrealValue)]
struct TelemetryRow {
    lifecycle_event: String,
    payload: Value,
    subject: Option<RecordId>,
    valid_from: DateTime<Utc>,
}
