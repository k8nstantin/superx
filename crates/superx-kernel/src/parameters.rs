//! Parameter verbs — the substrate plumbing for the "everything is a
//! parameter" mandate.
//!
//! Pattern B (locked in `project_superx_architecture.md` §13): each
//! parameter is its own `type_definition` row with
//! `category = 'attribute'` (e.g. `attr_capture_interval_secs`,
//! `attr_claude_code_projects_root`). Setting a parameter seeds the
//! type idempotently and appends a `state_ledger` row; reading takes
//! the latest row in the chain. Both operations are SELECT + CREATE
//! only — changing a parameter never mutates history, and the full
//! SCD-2 audit trail of every value a parameter ever held is one
//! `SELECT … ORDER BY valid_from ASC` away.
//!
//! ## Payload convention
//!
//! `state_ledger.payload` is an object at the engine level, so scalar
//! parameter values are wrapped as `{ "value": <v> }` — the same
//! convention `attr_module_status` already uses. The verbs wrap and
//! unwrap transparently; callers hand in and get back the bare
//! [`Value`].
//!
//! ## Where parameters live
//!
//! Parameters are attributes **of an entity**. Module-, driver-, and
//! app-scoped parameters target that contribution's own registry
//! entity (the [`RecordId`] returned by
//! [`Kernel::register_module`](crate::Kernel::register_module)).
//! Global kernel parameters conventionally live on the kernel's own
//! registry entity — `superx-kernel-bootstrap` (roadmap F6) registers
//! the kernel itself under the name `"kernel"` and global knobs hang
//! off that entity. The kernel does not special-case a singleton; the
//! convention is bootstrap's to uphold.

use surrealdb::types::{Object, RecordId, Value};

use crate::error::{KernelError, Result};
use crate::substrate::Kernel;

impl Kernel {
    /// Set (supersede) a parameter on a target entity.
    ///
    /// Idempotently seeds the parameter's `type_definition`
    /// (`category = 'attribute'`, `memory_tier = 'working'`), then
    /// appends a `state_ledger` row carrying `{ "value": <value> }`.
    /// Prior values are never mutated — they remain as queryable
    /// history.
    ///
    /// Returns the `state_ledger` row id of the new current value.
    ///
    /// # Errors
    ///
    /// [`KernelError::Db`] for engine errors (including the engine's
    /// `ASSERT` refusal if `param_uid` resolves to a non-attribute
    /// type).
    pub async fn set_parameter(
        &self,
        target: RecordId,
        param_uid: &str,
        value: Value,
    ) -> Result<RecordId> {
        self.ensure_type_definition(param_uid, "attribute", "working")
            .await?;
        let mut payload = Object::new();
        payload.insert("value".to_string(), value);
        self.supersede_state(target, param_uid, Value::Object(payload))
            .await
    }

    /// Read the current value of a parameter on a target entity.
    ///
    /// Returns `None` if the parameter has never been set — whether
    /// the parameter type is entirely unseeded or seeded but never
    /// written for this target.
    ///
    /// # Errors
    ///
    /// [`KernelError::Corrupt`] if a row exists for this
    /// `(target, param_uid)` chain but its payload does not carry the
    /// `{ "value": … }` shape — a foreign or corrupt write that must
    /// surface, never be coerced into `None`.
    pub async fn get_parameter(
        &self,
        target: RecordId,
        param_uid: &str,
    ) -> Result<Option<Value>> {
        // An unseeded parameter type means "never set anywhere" — not
        // an error, unlike find_type's NotFound contract.
        if self.find_type_opt(param_uid).await?.is_none() {
            return Ok(None);
        }
        let Some(current) = self.current_state(target, param_uid).await? else {
            return Ok(None);
        };
        let Value::Object(obj) = current else {
            return Err(KernelError::Corrupt(format!(
                "parameter '{param_uid}' payload is not an object"
            )));
        };
        match obj.get("value") {
            Some(v) => Ok(Some(v.clone())),
            None => Err(KernelError::Corrupt(format!(
                "parameter '{param_uid}' payload is missing the 'value' key"
            ))),
        }
    }
}
