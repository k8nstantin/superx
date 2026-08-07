//! Lifecycle state machine for every kernel module / adapter.
//!
//! State transitions are written to the substrate as
//! `attr_lifecycle_state` rows on the corresponding registry entity.
//! Each transition also emits a typed `telemetry_stream` event with
//! the module's entity as `subject`, so the full lifecycle history is
//! queryable per module.
//!
//! Failure isolation: failed modules are captured + visible;
//! dependents get marked SKIPPED (not failed); boot continues with
//! the rest.

use serde::{Deserialize, Serialize};
use surrealdb::types::{Object, RecordId, SurrealValue, Value};

use crate::error::Result;
use crate::registry::NodeKind;
use crate::substrate::Kernel;

/// The lifecycle state of one registered module / adapter.
///
/// Wire-serialised as a tagged string so substrate rows are
/// human-readable. Each transition gets its own `state_ledger` row and
/// its own `telemetry_stream` event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LifecycleState {
    /// Compile-time present, substrate-enabled, startup not yet
    /// attempted. The default state on first encounter.
    Enabled,
    /// `startup()` is currently executing. Transient.
    Starting,
    /// `startup()` returned `Ok`.
    Active { startup_duration_ms: u64 },
    /// `startup()` returned `Err`; the module is NOT running and
    /// dependents will be Skipped.
    Failed { error: String, failed_during: String },
    /// A dependency failed; startup was not attempted.
    Skipped { reason: String },
    /// Operator-disabled; boot will not attempt startup until
    /// re-enabled.
    Disabled,
}

impl LifecycleState {
    /// Short tag for list rendering.
    #[must_use]
    pub fn short_tag(&self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Starting => "starting",
            Self::Active { .. } => "active",
            Self::Failed { .. } => "failed",
            Self::Skipped { .. } => "skipped",
            Self::Disabled => "disabled",
        }
    }
}

/// Flat row shape mirroring the enum in a form SurrealValue can
/// serialize natively. The `tag` field discriminates; optional fields
/// carry state-specific extras.
#[derive(Debug, Clone, SurrealValue)]
struct LifecycleRow {
    tag: String,
    error: Option<String>,
    failed_during: Option<String>,
    reason: Option<String>,
    startup_duration_ms: Option<i64>,
}

impl LifecycleRow {
    fn from_state(s: &LifecycleState) -> Self {
        match s {
            LifecycleState::Enabled => Self {
                tag: "enabled".into(),
                error: None,
                failed_during: None,
                reason: None,
                startup_duration_ms: None,
            },
            LifecycleState::Starting => Self {
                tag: "starting".into(),
                error: None,
                failed_during: None,
                reason: None,
                startup_duration_ms: None,
            },
            LifecycleState::Active {
                startup_duration_ms,
            } => Self {
                tag: "active".into(),
                startup_duration_ms: Some(*startup_duration_ms as i64),
                error: None,
                failed_during: None,
                reason: None,
            },
            LifecycleState::Failed {
                error,
                failed_during,
            } => Self {
                tag: "failed".into(),
                error: Some(error.clone()),
                failed_during: Some(failed_during.clone()),
                reason: None,
                startup_duration_ms: None,
            },
            LifecycleState::Skipped { reason } => Self {
                tag: "skipped".into(),
                reason: Some(reason.clone()),
                error: None,
                failed_during: None,
                startup_duration_ms: None,
            },
            LifecycleState::Disabled => Self {
                tag: "disabled".into(),
                error: None,
                failed_during: None,
                reason: None,
                startup_duration_ms: None,
            },
        }
    }

    fn into_state(self) -> Result<LifecycleState> {
        Ok(match self.tag.as_str() {
            "enabled" => LifecycleState::Enabled,
            "starting" => LifecycleState::Starting,
            "active" => LifecycleState::Active {
                startup_duration_ms: self.startup_duration_ms.unwrap_or(0).max(0) as u64,
            },
            "failed" => LifecycleState::Failed {
                error: self.error.unwrap_or_default(),
                failed_during: self.failed_during.unwrap_or_default(),
            },
            "skipped" => LifecycleState::Skipped {
                reason: self.reason.unwrap_or_default(),
            },
            "disabled" => LifecycleState::Disabled,
            // An unknown tag is corrupt substrate state. It must
            // surface as an error — defaulting to Enabled would tell
            // the boot orchestrator "healthy, not yet started" about a
            // row it cannot interpret (v1 lesson, F2).
            other => {
                return Err(crate::error::KernelError::Corrupt(format!(
                    "unknown lifecycle tag '{other}' in attr_lifecycle_state payload"
                )))
            }
        })
    }
}

/// Read the latest LifecycleState for a registry entity; `None` if no
/// lifecycle row has been written yet.
///
/// # Errors
///
/// [`crate::KernelError::Corrupt`] if a row exists but cannot be
/// interpreted — corrupt state is never coerced into a default.
pub(crate) async fn read_lifecycle(
    kernel: &Kernel,
    entity_id: RecordId,
) -> Result<Option<LifecycleState>> {
    let value = kernel
        .current_state(entity_id, "attr_lifecycle_state")
        .await?;
    let Some(v) = value else { return Ok(None) };
    let row = LifecycleRow::from_value(v).map_err(|e| {
        crate::error::KernelError::Corrupt(format!(
            "attr_lifecycle_state payload not in expected shape: {e}"
        ))
    })?;
    Ok(Some(row.into_state()?))
}

impl Kernel {
    /// Mark a registered module `Starting` — called just before its
    /// `startup()` future runs.
    pub async fn mark_starting(&self, kind: NodeKind, name: &str) -> Result<()> {
        let entity_id = self.find_or_error(kind, name).await?;
        self.write_lifecycle(entity_id.clone(), &LifecycleState::Starting)
            .await?;
        self.emit_lifecycle_event(entity_id, name, "module_starting", None)
            .await
    }

    /// Mark a registered module `Active` after successful startup.
    pub async fn mark_active(
        &self,
        kind: NodeKind,
        name: &str,
        startup_duration_ms: u64,
    ) -> Result<()> {
        let entity_id = self.find_or_error(kind, name).await?;
        self.write_lifecycle(
            entity_id.clone(),
            &LifecycleState::Active {
                startup_duration_ms,
            },
        )
        .await?;
        let mut payload = Object::new();
        payload.insert(
            "startup_duration_ms".to_string(),
            Value::Number((startup_duration_ms as i64).into()),
        );
        self.emit_lifecycle_event(entity_id, name, "module_active", Some(payload))
            .await
    }

    /// Mark a registered module `Failed` with an error description.
    pub async fn mark_failed(
        &self,
        kind: NodeKind,
        name: &str,
        error: &str,
        failed_during: &str,
    ) -> Result<()> {
        let entity_id = self.find_or_error(kind, name).await?;
        self.write_lifecycle(
            entity_id.clone(),
            &LifecycleState::Failed {
                error: error.to_string(),
                failed_during: failed_during.to_string(),
            },
        )
        .await?;
        let mut payload = Object::new();
        payload.insert("error".to_string(), Value::String(error.to_string()));
        payload.insert(
            "failed_during".to_string(),
            Value::String(failed_during.to_string()),
        );
        self.emit_lifecycle_event(entity_id, name, "module_failed", Some(payload))
            .await
    }

    /// Mark a registered module `Skipped` — boot chose not to start it
    /// because a dependency failed.
    pub async fn mark_skipped(&self, kind: NodeKind, name: &str, reason: &str) -> Result<()> {
        let entity_id = self.find_or_error(kind, name).await?;
        self.write_lifecycle(
            entity_id.clone(),
            &LifecycleState::Skipped {
                reason: reason.to_string(),
            },
        )
        .await?;
        let mut payload = Object::new();
        payload.insert("reason".to_string(), Value::String(reason.to_string()));
        self.emit_lifecycle_event(entity_id, name, "module_skipped", Some(payload))
            .await
    }

    // ─────────────────────────────────────────────────────────────────
    // Internal helpers
    // ─────────────────────────────────────────────────────────────────

    /// Write (supersede) the lifecycle state for one registry entity.
    pub(crate) async fn write_lifecycle(
        &self,
        entity_id: RecordId,
        state: &LifecycleState,
    ) -> Result<RecordId> {
        let row = LifecycleRow::from_state(state);
        self.supersede_state(entity_id, "attr_lifecycle_state", row.into_value())
            .await
    }

    async fn find_or_error(&self, kind: NodeKind, name: &str) -> Result<RecordId> {
        match self.find_module_by_name(kind, name).await? {
            Some(id) => Ok(id),
            None => Err(crate::error::KernelError::NotFound(format!(
                "{} not registered: {name}",
                kind.type_uid()
            ))),
        }
    }

    /// Emit one lifecycle telemetry event, attributed to the module's
    /// own registry entity via `subject`.
    async fn emit_lifecycle_event(
        &self,
        entity_id: RecordId,
        name: &str,
        event: &str,
        extra_payload: Option<Object>,
    ) -> Result<()> {
        let mut payload = extra_payload.unwrap_or_default();
        payload.insert("name".to_string(), Value::String(name.to_string()));
        self.log_telemetry(event, Value::Object(payload), Some(entity_id))
            .await?;
        Ok(())
    }
}
