//! Lifecycle state machine for every kernel module / driver / app.
//!
//! State transitions are written to substrate as `attr_lifecycle_state`
//! rows on the corresponding `node_kernel_module` or `node_contribution`
//! entity. Each transition also emits a typed `telemetry_stream` event
//! so the operator can `superx kernel stats --module <name>` and see
//! the full lifecycle history.
//!
//! Failure isolation: failed modules are captured + visible; dependents
//! get marked SKIPPED (not failed); bootstrap continues with the rest.
//! `superx kernel modules retry <name>` re-enters the cycle.

use serde::{Deserialize, Serialize};

/// The lifecycle state of one registered module / driver / app.
///
/// Wire-serialised as a tagged string so substrate writes are
/// human-readable in `superx kernel stats`. Each transition gets its
/// own `state_ledger` row and its own `telemetry_stream` event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LifecycleState {
    /// Compile-time present, substrate-enabled, but startup not yet
    /// attempted. The default state on first encounter.
    Enabled,

    /// `startup()` is currently executing. Transient.
    Starting,

    /// `startup()` returned `Ok`. The module / driver / app is alive
    /// (for those that hold long-running tasks like the capture loop)
    /// or registered and ready (for those that just need to be
    /// reachable).
    Active { startup_duration_ms: u64 },

    /// `startup()` returned `Err`. The error is captured here. The
    /// module is NOT running; dependents will be Skipped.
    Failed { error: String, failed_during: String },

    /// A dependency failed; this module was not attempted. `reason`
    /// records the dependency chain.
    Skipped { reason: String },

    /// Operator-disabled. Bootstrap will not attempt startup until
    /// the operator re-enables.
    Disabled,
}

impl LifecycleState {
    /// Short tag for `superx kernel modules list` output.
    pub fn short_tag(&self) -> &'static str {
        match self {
            Self::Enabled        => "enabled",
            Self::Starting       => "starting",
            Self::Active { .. }  => "active",
            Self::Failed  { .. } => "failed",
            Self::Skipped { .. } => "skipped",
            Self::Disabled       => "disabled",
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// Lifecycle verbs on Kernel
// ─────────────────────────────────────────────────────────────────────

use surrealdb::types::{RecordId, SurrealValue, Value};
use crate::error::Result;
use crate::registry::NodeKind;
use crate::substrate::Kernel;

/// Flat row shape that mirrors the LifecycleState enum in a form
/// SurrealValue can serialize / deserialize natively. The `tag` field
/// distinguishes states; the optional fields carry state-specific
/// extras.
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
                error: None, failed_during: None, reason: None, startup_duration_ms: None,
            },
            LifecycleState::Starting => Self {
                tag: "starting".into(),
                error: None, failed_during: None, reason: None, startup_duration_ms: None,
            },
            LifecycleState::Active { startup_duration_ms } => Self {
                tag: "active".into(),
                startup_duration_ms: Some(*startup_duration_ms as i64),
                error: None, failed_during: None, reason: None,
            },
            LifecycleState::Failed { error, failed_during } => Self {
                tag: "failed".into(),
                error: Some(error.clone()),
                failed_during: Some(failed_during.clone()),
                reason: None, startup_duration_ms: None,
            },
            LifecycleState::Skipped { reason } => Self {
                tag: "skipped".into(),
                reason: Some(reason.clone()),
                error: None, failed_during: None, startup_duration_ms: None,
            },
            LifecycleState::Disabled => Self {
                tag: "disabled".into(),
                error: None, failed_during: None, reason: None, startup_duration_ms: None,
            },
        }
    }

    fn into_state(self) -> Result<LifecycleState> {
        Ok(match self.tag.as_str() {
            "enabled"  => LifecycleState::Enabled,
            "starting" => LifecycleState::Starting,
            "active"   => LifecycleState::Active {
                startup_duration_ms: self.startup_duration_ms.unwrap_or(0).max(0) as u64,
            },
            "failed"   => LifecycleState::Failed {
                error:         self.error.unwrap_or_default(),
                failed_during: self.failed_during.unwrap_or_default(),
            },
            "skipped"  => LifecycleState::Skipped {
                reason: self.reason.unwrap_or_default(),
            },
            "disabled" => LifecycleState::Disabled,
            // An unknown tag is corrupt substrate state (a foreign or
            // bad write). It must surface as an error — defaulting to
            // Enabled would tell a boot orchestrator "healthy, not yet
            // started" about a row it cannot actually interpret.
            other => {
                return Err(crate::error::KernelError::Corrupt(format!(
                    "unknown lifecycle tag '{other}' in attr_lifecycle_state payload"
                )))
            }
        })
    }
}

/// Read the latest LifecycleState for a registered entity.
///
/// Returns `None` if no lifecycle state has been written for this
/// entity yet (i.e., the entity exists but the lifecycle column on
/// `state_ledger` is empty for it).
///
/// # Errors
///
/// Returns [`KernelError::Corrupt`](crate::error::KernelError::Corrupt)
/// if a lifecycle row exists but cannot be interpreted — wrong payload
/// shape or an unknown state tag. Corrupt state is never coerced into
/// a default.
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
    /// Mark a registered module as `Starting`. Bootstrap calls this
    /// just before invoking the module's `startup()` future.
    /// Idempotent — repeat calls supersede the prior state.
    pub async fn mark_starting(&self, kind: NodeKind, name: &str) -> Result<()> {
        let entity_id = self.find_or_error(kind, name).await?;
        self.write_lifecycle(entity_id.clone(), &LifecycleState::Starting).await?;
        self.emit_lifecycle_event(name, kind, "module_starting", None).await?;
        Ok(())
    }

    /// Mark a registered module as `Active`. Bootstrap calls this on
    /// successful return from `startup()`.
    pub async fn mark_active(
        &self,
        kind: NodeKind,
        name: &str,
        startup_duration_ms: u64,
    ) -> Result<()> {
        let entity_id = self.find_or_error(kind, name).await?;
        let state = LifecycleState::Active { startup_duration_ms };
        self.write_lifecycle(entity_id.clone(), &state).await?;
        let mut payload = surrealdb::types::Object::new();
        payload.insert(
            "startup_duration_ms".to_string(),
            Value::Number((startup_duration_ms as i64).into()),
        );
        self.emit_lifecycle_event(name, kind, "module_active", Some(payload)).await?;
        Ok(())
    }

    /// Mark a registered module as `Failed` with an error description.
    /// Bootstrap calls this when `startup()` returns `Err` or panics.
    pub async fn mark_failed(
        &self,
        kind: NodeKind,
        name: &str,
        error: &str,
        failed_during: &str,
    ) -> Result<()> {
        let entity_id = self.find_or_error(kind, name).await?;
        let state = LifecycleState::Failed {
            error: error.to_string(),
            failed_during: failed_during.to_string(),
        };
        self.write_lifecycle(entity_id.clone(), &state).await?;
        let mut payload = surrealdb::types::Object::new();
        payload.insert("error".to_string(), Value::String(error.to_string()));
        payload.insert(
            "failed_during".to_string(),
            Value::String(failed_during.to_string()),
        );
        self.emit_lifecycle_event(name, kind, "module_failed", Some(payload)).await?;
        Ok(())
    }

    /// Mark a registered module as `Skipped` — bootstrap chose not
    /// to call `startup()` because a dependency failed.
    pub async fn mark_skipped(&self, kind: NodeKind, name: &str, reason: &str) -> Result<()> {
        let entity_id = self.find_or_error(kind, name).await?;
        let state = LifecycleState::Skipped { reason: reason.to_string() };
        self.write_lifecycle(entity_id.clone(), &state).await?;
        let mut payload = surrealdb::types::Object::new();
        payload.insert("reason".to_string(), Value::String(reason.to_string()));
        self.emit_lifecycle_event(name, kind, "module_skipped", Some(payload)).await?;
        Ok(())
    }

    // ─────────────────────────────────────────────────────────────────
    // Internal helpers (lifecycle-related)
    // ─────────────────────────────────────────────────────────────────

    /// Write (supersede) the lifecycle state for one registered entity.
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
                "{} module not registered: {name}",
                match kind {
                    NodeKind::KernelModule => "kernel",
                    NodeKind::Contribution => "contribution",
                }
            ))),
        }
    }

    async fn emit_lifecycle_event(
        &self,
        name: &str,
        kind: NodeKind,
        event: &str,
        extra_payload: Option<surrealdb::types::Object>,
    ) -> Result<()> {
        let mut payload = extra_payload.unwrap_or_default();
        payload.insert("name".to_string(), Value::String(name.to_string()));
        payload.insert(
            "kind".to_string(),
            Value::String(
                match kind {
                    NodeKind::KernelModule => "kernel_module",
                    NodeKind::Contribution => "contribution",
                }
                .to_string(),
            ),
        );
        self.log_telemetry(event, Value::Object(payload), None).await?;
        Ok(())
    }
}

