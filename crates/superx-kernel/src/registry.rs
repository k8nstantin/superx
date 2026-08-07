//! Module registry — the mechanism that lets every kernel module and
//! agent adapter announce itself to the kernel at link time.
//!
//! Each module/adapter uses `linkme::distributed_slice` to register a
//! static descriptor; the kernel iterates the slice — no central
//! enumeration. Adding a module = one new file with one registration.
//! Zero edits to existing files (BLUEPRINT.md §2.5).

use async_trait::async_trait;
use linkme::distributed_slice;
use serde::{Deserialize, Serialize};
use surrealdb::types::{Array, Object, RecordId, SurrealValue, Value};

use crate::error::Result;
use crate::metamodel::MetamodelType;
use crate::substrate::Kernel;

/// What KIND of thing this is — a kernel module or an agent adapter.
/// The kind drives which `node_*` entity type backs it in the
/// substrate registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    /// A kernel module — long-running or startup-time kernel
    /// capability. Backed by `node_kernel_module` entities.
    KernelModule,
    /// A per-agent capture adapter (BLUEPRINT.md §2, "Agent
    /// adapters"). Backed by `node_adapter` entities.
    Adapter,
}

impl NodeKind {
    /// The `type_definition.uid` that backs entities of this kind.
    #[must_use]
    pub fn type_uid(self) -> &'static str {
        match self {
            Self::KernelModule => "node_kernel_module",
            Self::Adapter => "node_adapter",
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::KernelModule => "kernel_module",
            Self::Adapter => "adapter",
        }
    }
}

/// Static descriptor every kernel module / adapter exports. Lives in
/// the linkme inventory; read at boot time to compose the boot DAG.
#[derive(Debug, Clone)]
pub struct KernelModuleDescriptor {
    /// Stable identifier — unique across the binary. Conventions:
    /// kernel modules: `"kernel"`, `"capture"`; adapters:
    /// `"adapter_claude_code"`, `"adapter_gemini_cli"`.
    pub name: &'static str,
    /// Semver version of the providing crate.
    pub version: &'static str,
    /// What KIND this is.
    pub kind: NodeKind,
    /// Names of other registered modules this depends on. Boot
    /// topo-sorts by this. Empty slice = no dependencies.
    pub depends_on: &'static [&'static str],
    /// Type_definition uids this module introduces — seeded via
    /// `ensure_type_definition` at boot, composing with the kernel's
    /// own `REQUIRED_METAMODEL_TYPES`.
    pub required_metamodel: &'static [MetamodelType],
}

/// Trait every kernel module / adapter implements.
///
/// Implementations are typically zero-sized types (`struct Foo;`) —
/// state lives in the substrate. Register with
/// `#[distributed_slice(KERNEL_MODULES)]` on a `static`.
#[async_trait]
pub trait KernelModule: Send + Sync + 'static {
    /// Return the compile-time descriptor. Pure, no I/O.
    fn descriptor(&self) -> KernelModuleDescriptor;

    /// Called by boot after the kernel has signed in, the metamodel is
    /// seeded, and this module's dependencies are Active. Must be
    /// idempotent (re-runnable without duplicating state).
    ///
    /// # Errors
    ///
    /// Any error marks this module `Failed`, emits `module_failed`
    /// telemetry, and SKIPs every dependent. Boot continues with
    /// independent modules.
    async fn startup(&self, kernel: &Kernel) -> Result<()>;

    /// Called on graceful shutdown. Default: nothing — substrate
    /// state persists across boots.
    async fn shutdown(&self, _kernel: &Kernel) -> Result<()> {
        Ok(())
    }
}

/// Compile-time inventory of every registered kernel module / adapter
/// in the binary. Filled at link time via `#[distributed_slice]`.
///
/// ## How to register
///
/// ```ignore
/// use linkme::distributed_slice;
/// use superx_kernel::{Kernel, KernelModule, KernelModuleDescriptor,
///                     NodeKind, Result, KERNEL_MODULES};
///
/// pub struct MyModule;
///
/// #[async_trait::async_trait]
/// impl KernelModule for MyModule {
///     fn descriptor(&self) -> KernelModuleDescriptor {
///         KernelModuleDescriptor {
///             name: "my_module",
///             version: env!("CARGO_PKG_VERSION"),
///             kind: NodeKind::KernelModule,
///             depends_on: &[],
///             required_metamodel: &[],
///         }
///     }
///     async fn startup(&self, _kernel: &Kernel) -> Result<()> { Ok(()) }
/// }
///
/// #[distributed_slice(KERNEL_MODULES)]
/// static MY_REGISTRATION: &'static (dyn KernelModule + Sync) = &MyModule;
/// ```
#[distributed_slice]
pub static KERNEL_MODULES: [&'static (dyn KernelModule + Sync)];

/// Status of one registered module as exposed by
/// [`Kernel::list_with_status`] / [`Kernel::detailed_status`].
#[derive(Debug, Clone, Serialize)]
pub struct RegistryStatus {
    /// `descriptor.name` — stable identifier across boots.
    pub name: String,
    /// `descriptor.version`.
    pub version: String,
    /// `descriptor.kind`.
    pub kind: NodeKind,
    /// The module's substrate identity (its registry entity id).
    pub entity_id: RecordId,
    /// Current lifecycle state, from the latest `attr_lifecycle_state`
    /// row.
    pub lifecycle: crate::lifecycle::LifecycleState,
}

/// Enabled / disabled flag persisted on `attr_module_status`. Distinct
/// from lifecycle — status is the operator's intent, lifecycle is the
/// runtime reality.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleStatus {
    Enabled,
    Disabled,
}

impl ModuleStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// Registry verbs on Kernel
// ─────────────────────────────────────────────────────────────────────

impl Kernel {
    /// Idempotently register one module/adapter descriptor. If an
    /// entry with the same `(kind, name)` exists, returns its entity
    /// id and supersedes the descriptor (fields may change across
    /// binary versions); otherwise creates a fresh entity + initial
    /// status/lifecycle rows.
    pub async fn register_module(
        &self,
        descriptor: &KernelModuleDescriptor,
    ) -> Result<RecordId> {
        let existing = self
            .find_module_by_name(descriptor.kind, descriptor.name)
            .await?;
        let entity_id = match existing {
            Some(id) => id,
            None => {
                let id = self.create_entity(descriptor.kind.type_uid()).await?;
                self.write_status(id.clone(), ModuleStatus::Enabled).await?;
                self.write_lifecycle(id.clone(), &crate::lifecycle::LifecycleState::Enabled)
                    .await?;
                id
            }
        };
        self.write_descriptor(entity_id.clone(), descriptor).await?;
        Ok(entity_id)
    }

    /// List every registered module/adapter of the given kind with
    /// its current status.
    pub async fn list_with_status(&self, kind: NodeKind) -> Result<Vec<RegistryStatus>> {
        let type_id = self.find_type(kind.type_uid()).await?;
        #[derive(SurrealValue)]
        struct EntityRow {
            id: RecordId,
        }
        let entities: Vec<EntityRow> = self
            .db()
            .query("SELECT id FROM entity WHERE type = $type")
            .bind(("type", type_id))
            .await?
            .take(0)?;

        let mut out = Vec::with_capacity(entities.len());
        for e in entities {
            if let Some(status) = self.read_registry_status(e.id, kind).await? {
                out.push(status);
            }
        }
        Ok(out)
    }

    /// Detailed status of one registered module by name; `None` if
    /// not registered.
    pub async fn detailed_status(
        &self,
        kind: NodeKind,
        name: &str,
    ) -> Result<Option<RegistryStatus>> {
        let Some(entity_id) = self.find_module_by_name(kind, name).await? else {
            return Ok(None);
        };
        self.read_registry_status(entity_id, kind).await
    }

    /// Read the operator's enable/disable intent for a registered
    /// module. `None` when unregistered or never written — callers
    /// treat both as enabled (installed = enabled).
    ///
    /// # Errors
    ///
    /// [`crate::KernelError::Corrupt`] when a status row exists but
    /// doesn't carry `{ value: "enabled" | "disabled" }`.
    pub async fn module_status(
        &self,
        kind: NodeKind,
        name: &str,
    ) -> Result<Option<ModuleStatus>> {
        let Some(entity_id) = self.find_module_by_name(kind, name).await? else {
            return Ok(None);
        };
        let Some(value) = self.current_state(entity_id, "attr_module_status").await? else {
            return Ok(None);
        };
        let Value::Object(obj) = value else {
            return Err(crate::error::KernelError::Corrupt(
                "attr_module_status payload is not an object".to_string(),
            ));
        };
        match obj.get("value") {
            Some(Value::String(s)) if s == "enabled" => Ok(Some(ModuleStatus::Enabled)),
            Some(Value::String(s)) if s == "disabled" => Ok(Some(ModuleStatus::Disabled)),
            other => Err(crate::error::KernelError::Corrupt(format!(
                "attr_module_status value is not 'enabled' / 'disabled': {other:?}"
            ))),
        }
    }

    // ─────────────────────────────────────────────────────────────────
    // Internal helpers
    // ─────────────────────────────────────────────────────────────────

    /// Find the entity id of a registered module by (kind, name);
    /// `None` if not yet registered.
    pub(crate) async fn find_module_by_name(
        &self,
        kind: NodeKind,
        name: &str,
    ) -> Result<Option<RecordId>> {
        self.find_entity_by_name(kind.type_uid(), "attr_module_descriptor", name)
            .await
    }

    /// Write (supersede) the descriptor payload on a registry entity.
    pub(crate) async fn write_descriptor(
        &self,
        entity_id: RecordId,
        descriptor: &KernelModuleDescriptor,
    ) -> Result<RecordId> {
        let mut payload = Object::new();
        payload.insert(
            "name".to_string(),
            Value::String(descriptor.name.to_string()),
        );
        payload.insert(
            "version".to_string(),
            Value::String(descriptor.version.to_string()),
        );
        payload.insert(
            "kind".to_string(),
            Value::String(descriptor.kind.as_str().to_string()),
        );
        let deps: Vec<Value> = descriptor
            .depends_on
            .iter()
            .map(|s| Value::String((*s).to_string()))
            .collect();
        payload.insert("depends_on".to_string(), Value::Array(Array::from(deps)));
        self.supersede_state(entity_id, "attr_module_descriptor", Value::Object(payload))
            .await
    }

    /// Write the module's enabled/disabled status row.
    pub(crate) async fn write_status(
        &self,
        entity_id: RecordId,
        status: ModuleStatus,
    ) -> Result<RecordId> {
        let mut payload = Object::new();
        payload.insert(
            "value".to_string(),
            Value::String(status.as_str().to_string()),
        );
        self.supersede_state(entity_id, "attr_module_status", Value::Object(payload))
            .await
    }

    /// Compose a [`RegistryStatus`] from substrate state; `None` if
    /// the descriptor row hasn't been written yet.
    async fn read_registry_status(
        &self,
        entity_id: RecordId,
        kind: NodeKind,
    ) -> Result<Option<RegistryStatus>> {
        let desc = self
            .current_state(entity_id.clone(), "attr_module_descriptor")
            .await?;
        let Some(Value::Object(desc_obj)) = desc else {
            return Ok(None);
        };
        let name = match desc_obj.get("name") {
            Some(Value::String(s)) => s.clone(),
            _ => return Ok(None),
        };
        let version = match desc_obj.get("version") {
            Some(Value::String(s)) => s.clone(),
            _ => String::new(),
        };
        let lifecycle = crate::lifecycle::read_lifecycle(self, entity_id.clone())
            .await?
            .unwrap_or(crate::lifecycle::LifecycleState::Enabled);

        Ok(Some(RegistryStatus {
            name,
            version,
            kind,
            entity_id,
            lifecycle,
        }))
    }
}
