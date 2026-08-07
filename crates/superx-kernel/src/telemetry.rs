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
    /// The agent this event concerns (v2.1) — direct per-agent
    /// queries over the actions firehose.
    pub agent: Option<RecordId>,
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
        self.log_telemetry_for_agent(event, payload, subject, None)
            .await
    }

    /// Append one row to `telemetry_stream` carrying both the emitter
    /// (`subject`) and the agent the event concerns (`agent`, v2.1) —
    /// the capture path's verb, making every action row directly
    /// queryable by agent_id.
    ///
    /// # Errors
    ///
    /// Surfaces engine refusals verbatim via [`crate::KernelError::Db`]
    /// (e.g. an `agent` that is not a `node_agent` entity).
    pub async fn log_telemetry_for_agent(
        &self,
        event: &str,
        payload: Value,
        subject: Option<RecordId>,
        agent: Option<RecordId>,
    ) -> Result<RecordId> {
        let id = self.new_record_id("telemetry_stream");
        let row = TelemetryRow {
            lifecycle_event: event.to_string(),
            payload,
            subject,
            agent,
            valid_from: Utc::now(),
        };
        let _: Option<TelemetryRow> = self.db().create(id.clone()).content(row).await?;
        Ok(id)
    }

    /// Read an agent's telemetry rows, newest first — "query the data
    /// for a specific agent" over the actions firehose.
    ///
    /// # Errors
    ///
    /// Surfaces engine errors verbatim via [`crate::KernelError::Db`].
    pub async fn agent_telemetry(
        &self,
        agent: RecordId,
        limit: u32,
    ) -> Result<Vec<TelemetryRecord>> {
        let rows: Vec<TelemetryRecord> = self
            .db()
            .query(
                "SELECT * FROM telemetry_stream WHERE agent = $agent \
                 ORDER BY valid_from DESC LIMIT $limit",
            )
            .bind(("agent", agent))
            .bind(("limit", limit))
            .await?
            .take(0)?;
        Ok(rows)
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
    agent: Option<RecordId>,
    valid_from: DateTime<Utc>,
}
