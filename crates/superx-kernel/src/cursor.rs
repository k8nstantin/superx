//! Cursor verbs — durable progress checkpoints over the `cursor` +
//! `cursor_type` tables.
//!
//! A cursor records "how far has X been processed": the capture loop
//! (roadmap F10) checkpoints each telemetry source's read position
//! after every poll tick; future workload runners checkpoint
//! resume points the same way. Append-only like everything else —
//! every checkpoint is a fresh row, the current position is the
//! latest row in the `(subject, cursor_type)` chain, and the full
//! processing history of any source is queryable forever.
//!
//! `cursor_type` categorizes cursor kinds (engine-enforced:
//! `'workload'` or `'telemetry'`) and is self-describing via `uid` +
//! `description`, mirroring how `type_definition` works for the
//! entity graph.

use chrono::{DateTime, Utc};
use surrealdb::types::{Object, RecordId, SurrealValue};

use crate::error::{KernelError, Result};
use crate::substrate::Kernel;

/// One cursor row read back from the substrate — the latest known
/// progress checkpoint for a `(subject, cursor_type)` chain.
#[derive(Debug, Clone, SurrealValue)]
pub struct Cursor {
    /// UUIDv7 row id of this checkpoint.
    pub id: RecordId,
    /// The entity whose processing this cursor tracks (e.g. a
    /// `node_source` for telemetry capture, a `node_run` for
    /// workloads).
    pub subject: RecordId,
    /// FK to the `cursor_type` row categorizing this cursor.
    pub cursor_type: RecordId,
    /// Opaque progress marker owned by the writer (file offset, last
    /// event uuid, …). `None` on a freshly-initialized cursor.
    pub last_processed: Option<String>,
    /// Optional writer-owned envelope (e.g. `{file, line}`).
    pub metadata: Option<Object>,
    /// When this checkpoint was written.
    pub valid_from: DateTime<Utc>,
}

/// Insert shape for `cursor_type` rows.
#[derive(Debug, Clone, SurrealValue)]
struct CursorTypeRow {
    uid: String,
    category: String,
    description: String,
    sch_json: Option<String>,
    valid_from: DateTime<Utc>,
}

/// Insert shape for `cursor` rows.
#[derive(Debug, Clone, SurrealValue)]
struct CursorWriteRow {
    subject: RecordId,
    cursor_type: RecordId,
    last_processed: Option<String>,
    metadata: Option<Object>,
    valid_from: DateTime<Utc>,
}

impl Kernel {
    /// Idempotently CREATE one `cursor_type` row, returning the
    /// resolved [`RecordId`]. If a row with this `uid` already exists,
    /// returns its id without creating a new row. Mirrors
    /// [`Kernel::ensure_type_definition`].
    ///
    /// # Errors
    ///
    /// [`KernelError::Db`] for engine errors — including the engine's
    /// `ASSERT` refusal when `category` is outside
    /// `['workload', 'telemetry']`.
    pub async fn ensure_cursor_type(
        &self,
        uid: &str,
        category: &str,
        description: &str,
    ) -> Result<RecordId> {
        if let Some(existing) = self.find_cursor_type_opt(uid).await? {
            return Ok(existing);
        }
        let id = self.new_record_id("cursor_type");
        let row = CursorTypeRow {
            uid: uid.to_string(),
            category: category.to_string(),
            description: description.to_string(),
            sch_json: None,
            valid_from: Utc::now(),
        };
        let _: Option<CursorTypeRow> = self.db().create(id.clone()).content(row).await?;
        Ok(id)
    }

    /// CREATE one progress checkpoint with an explicit UUIDv7 id
    /// (§11). Append-only: prior checkpoints are never mutated; the
    /// current position is recovered by [`Kernel::latest_cursor`].
    ///
    /// # Errors
    ///
    /// [`KernelError::NotFound`] if `cursor_type_uid` doesn't resolve;
    /// [`KernelError::Db`] for engine errors.
    pub async fn write_cursor(
        &self,
        subject: RecordId,
        cursor_type_uid: &str,
        last_processed: Option<String>,
        metadata: Option<Object>,
    ) -> Result<RecordId> {
        let cursor_type = self.find_cursor_type(cursor_type_uid).await?;
        let id = self.new_record_id("cursor");
        let row = CursorWriteRow {
            subject,
            cursor_type,
            last_processed,
            metadata,
            valid_from: Utc::now(),
        };
        let _: Option<CursorWriteRow> = self.db().create(id.clone()).content(row).await?;
        Ok(id)
    }

    /// Recover the latest checkpoint for a `(subject, cursor_type)`
    /// chain. Returns `None` when no checkpoint has been written —
    /// whether the cursor type is entirely unseeded or seeded but
    /// never written for this subject.
    ///
    /// # Errors
    ///
    /// [`KernelError::Db`] for engine errors.
    pub async fn latest_cursor(
        &self,
        subject: RecordId,
        cursor_type_uid: &str,
    ) -> Result<Option<Cursor>> {
        // An unseeded cursor type means "no checkpoint yet" — same
        // contract as get_parameter, not find_type's NotFound.
        let Some(cursor_type) = self.find_cursor_type_opt(cursor_type_uid).await? else {
            return Ok(None);
        };
        let rows: Vec<Cursor> = self
            .db()
            .query(
                "SELECT id, subject, cursor_type, last_processed, metadata, valid_from \
                 FROM cursor \
                 WHERE subject = $subject AND cursor_type = $cursor_type \
                 ORDER BY valid_from DESC LIMIT 1",
            )
            .bind(("subject", subject))
            .bind(("cursor_type", cursor_type))
            .await?
            .take(0)?;
        Ok(rows.into_iter().next())
    }

    // ─────────────────────────────────────────────────────────────────
    // Internal helpers (cursor-related)
    // ─────────────────────────────────────────────────────────────────

    /// Look up a `cursor_type` row id by uid; error if absent.
    async fn find_cursor_type(&self, uid: &str) -> Result<RecordId> {
        self.find_cursor_type_opt(uid)
            .await?
            .ok_or_else(|| KernelError::NotFound(format!("cursor_type with uid '{uid}'")))
    }

    /// Look up a `cursor_type` row id by uid; `Ok(None)` if absent.
    async fn find_cursor_type_opt(&self, uid: &str) -> Result<Option<RecordId>> {
        #[derive(SurrealValue)]
        struct IdOnly {
            id: RecordId,
        }
        let rows: Vec<IdOnly> = self
            .db()
            .query("SELECT id FROM cursor_type WHERE uid = $uid LIMIT 1")
            .bind(("uid", uid.to_string()))
            .await?
            .take(0)?;
        Ok(rows.into_iter().next().map(|r| r.id))
    }
}
