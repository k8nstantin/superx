//! Kernel module registry — the mechanism that lets every kernel module,
//! driver, and app announce itself to the kernel at link time.
//!
//! Pattern E2 (locked in `project_superx_architecture.md`): each
//! kernel-module / driver / app crate uses
//! `linkme::distributed_slice` to register a static descriptor at link
//! time. The kernel iterates the slice — no central enumeration. Adding
//! a new contribution = new crate + Cargo dep + workspace member. Zero
//! edits to existing files.

use async_trait::async_trait;
use linkme::distributed_slice;
use serde::{Deserialize, Serialize};
use surrealdb::types::{RecordId, SurrealValue};

use crate::error::Result;
use crate::metamodel::MetamodelType;
use crate::substrate::Kernel;

/// What KIND of thing this is — kernel module, driver, app, or a
/// future open-ended category. The kind drives which `node_*` entity
/// type backs it and where it lands in the dependency DAG.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    /// A kernel module — extends the kernel. Shares the kernel schema
    /// and the `superx_kernel` service account. Backed by
    /// `node_kernel_module` entities.
    KernelModule,

    /// A contribution — driver, app, or any future category. Brings
    /// its own schema + service account. Backed by `node_contribution`
    /// entities. The `category` field of the descriptor distinguishes
    /// drivers from apps from future things.
    Contribution,
}

/// Static descriptor every kernel module / driver / app exports. Lives
/// in the linkme inventory; read at boot time by
/// `superx-kernel-bootstrap` to compose the boot DAG.
#[derive(Debug, Clone)]
pub struct KernelModuleDescriptor {
    /// Stable identifier — must be unique across the binary. Used as
    /// the entity's primary identity in the registry. Conventions:
    ///   • kernel modules: short name like "parameters", "discovery"
    ///   • drivers: "driver_claude_code", "driver_kafka_sink"
    ///   • apps: "app_gmaster", "app_mcp"
    pub name: &'static str,

    /// Semver version of this contribution's crate.
    pub version: &'static str,

    /// What KIND this is. Drives which substrate entity type backs it.
    pub kind: NodeKind,

    /// Open-ended category string. For `NodeKind::Contribution`, this
    /// distinguishes drivers from apps from future categories like
    /// "model_integration" or "quantum_module". For
    /// `NodeKind::KernelModule`, this is conventionally the empty
    /// string (kernel modules don't have a sub-category).
    pub category: &'static str,

    /// Names of other registered modules this depends on. Bootstrap
    /// topo-sorts by this. Empty slice means "no dependencies."
    pub depends_on: &'static [&'static str],

    /// Type_definition uids this contribution introduces — every name
    /// here lands in the substrate via `ensure_type_definition` when
    /// the contribution registers. Composes with the kernel's own
    /// `REQUIRED_METAMODEL_TYPES` and every other contribution's slice.
    pub required_metamodel: &'static [MetamodelType],
}

/// Trait every kernel module / driver / app implements.
///
/// Implementations are typically zero-sized types (`struct Foo;`)
/// because the trait holds no state — state lives in the substrate.
/// Each implementor uses `#[distributed_slice(KERNEL_MODULES)]` on a
/// `static` reference of itself to register at link time.
#[async_trait]
pub trait KernelModule: Send + Sync + 'static {
    /// Return the compile-time descriptor. Pure, no I/O. Cached at
    /// boot time by the orchestrator.
    fn descriptor(&self) -> KernelModuleDescriptor;

    /// Called by `superx-kernel-bootstrap` after the kernel has signed
    /// in, the metamodel is seeded, and this module's dependencies are
    /// Active. The startup should be idempotent (re-runnable without
    /// duplicating state).
    ///
    /// # Errors
    ///
    /// Any error returned causes bootstrap to mark this module
    /// `Failed`, emit a `module_failed` telemetry event, and SKIP
    /// every dependent module. Bootstrap continues with independent
    /// modules.
    async fn startup(&self, kernel: &Kernel) -> Result<()>;

    /// Called on graceful shutdown. Default impl does nothing. Most
    /// modules don't need teardown — substrate state persists across
    /// boots.
    async fn shutdown(&self, _kernel: &Kernel) -> Result<()> {
        Ok(())
    }
}

/// Compile-time inventory of every registered kernel module / driver /
/// app in the binary. Filled at link time via `#[distributed_slice]`
/// attributes on `static`s in each contribution's crate.
///
/// ## How to register a new module / driver / app
///
/// In your crate's `lib.rs`:
///
/// ```ignore
/// use linkme::distributed_slice;
/// use async_trait::async_trait;
/// use superx_kernel::{Kernel, KernelModule, KernelModuleDescriptor, NodeKind,
///                     Result, REQUIRED_METAMODEL_TYPES, MetamodelType,
///                     KERNEL_MODULES};
///
/// pub struct MyKernelModule;
///
/// #[async_trait]
/// impl KernelModule for MyKernelModule {
///     fn descriptor(&self) -> KernelModuleDescriptor {
///         KernelModuleDescriptor {
///             name: "my_module",
///             version: env!("CARGO_PKG_VERSION"),
///             kind: NodeKind::KernelModule,
///             category: "",
///             depends_on: &[],
///             required_metamodel: &[
///                 MetamodelType {
///                     uid: "attr_my_module_setting",
///                     category: "attribute",
///                     memory_tier: "working",
///                 },
///             ],
///         }
///     }
///
///     async fn startup(&self, _kernel: &Kernel) -> Result<()> {
///         Ok(())
///     }
/// }
///
/// #[distributed_slice(KERNEL_MODULES)]
/// static MY_REGISTRATION: &'static dyn KernelModule = &MyKernelModule;
/// ```
///
/// That's the whole registration. No central enumeration; the slice
/// fills itself at link time.
#[distributed_slice]
pub static KERNEL_MODULES: [&'static (dyn KernelModule + Sync)];

/// Status of one registered module as exposed by
/// [`Kernel::list_with_status`] / [`Kernel::detailed_status`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryStatus {
    /// `descriptor.name` — stable identifier across boots.
    pub name: String,
    /// `descriptor.version` — semver of the contribution.
    pub version: String,
    /// `descriptor.kind` — KernelModule or Contribution.
    pub kind: NodeKind,
    /// `descriptor.category` — open-ended category string for
    /// Contributions (driver / app / model_integration / …).
    pub category: String,
    /// Module's substrate identity — its `node_kernel_module` or
    /// `node_contribution` entity's RecordId.
    pub entity_id: surrealdb::types::RecordId,
    /// Current lifecycle state, derived from the latest
    /// `attr_lifecycle_state` state_ledger row.
    pub lifecycle: crate::lifecycle::LifecycleState,
}

impl NodeKind {
    /// The `type_definition.uid` that backs entities of this kind.
    pub fn type_uid(self) -> &'static str {
        match self {
            Self::KernelModule => "node_kernel_module",
            Self::Contribution => "node_contribution",
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// Registry verbs on Kernel
// ─────────────────────────────────────────────────────────────────────

impl Kernel {
    /// Idempotently register one kernel module / driver / app's
    /// descriptor. If an entry with the same `descriptor.name` already
    /// exists for the same kind, returns that entity's RecordId; the
    /// descriptor is superseded (updated to the latest values). If not
    /// found, creates a fresh entity + initial state_ledger rows
    /// (descriptor + status=enabled + lifecycle=Enabled).
    ///
    /// Called by each kernel module / driver / app at startup; the
    /// resulting RecordId is what subsequent lifecycle calls
    /// ([`Kernel::mark_starting`], etc.) target.
    pub async fn register_module(
        &self,
        descriptor: &KernelModuleDescriptor,
    ) -> Result<RecordId> {
        let kind = descriptor.kind;
        let existing = self.find_module_by_name(kind, descriptor.name).await?;

        let entity_id = match existing {
            Some(id) => id,
            None => {
                let id = self
                    .create_entity(kind.type_uid(), "admin")
                    .await?;
                // Initial status + lifecycle on first registration.
                self.write_status(id.clone(), ModuleStatus::Enabled).await?;
                self.write_lifecycle(id.clone(), &crate::lifecycle::LifecycleState::Enabled)
                    .await?;
                id
            }
        };

        // Always supersede the descriptor — fields may have changed
        // across binary versions.
        self.write_descriptor(entity_id.clone(), descriptor).await?;
        Ok(entity_id)
    }

    /// List every registered module / driver / app of the given kind
    /// with its current status.
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

    /// Detailed status of one registered module by name. Returns
    /// `None` if no module of that kind + name has been registered.
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

    // ─────────────────────────────────────────────────────────────────
    // Internal helpers (registry-related)
    // ─────────────────────────────────────────────────────────────────

    /// Find the entity_id of a registered module by (kind, name).
    /// Returns `None` if not yet registered.
    pub(crate) async fn find_module_by_name(
        &self,
        kind: NodeKind,
        name: &str,
    ) -> Result<Option<RecordId>> {
        self.find_entity_by_name(kind.type_uid(), "attr_module_descriptor", name)
            .await
    }

    /// Write (supersede) the descriptor payload on a registered
    /// entity.
    pub(crate) async fn write_descriptor(
        &self,
        entity_id: RecordId,
        descriptor: &KernelModuleDescriptor,
    ) -> Result<RecordId> {
        let mut payload = surrealdb::types::Object::new();
        payload.insert(
            "name".to_string(),
            surrealdb::types::Value::String(descriptor.name.to_string()),
        );
        payload.insert(
            "version".to_string(),
            surrealdb::types::Value::String(descriptor.version.to_string()),
        );
        payload.insert(
            "kind".to_string(),
            surrealdb::types::Value::String(match descriptor.kind {
                NodeKind::KernelModule => "kernel_module".to_string(),
                NodeKind::Contribution => "contribution".to_string(),
            }),
        );
        payload.insert(
            "category".to_string(),
            surrealdb::types::Value::String(descriptor.category.to_string()),
        );
        let depends_on_arr: Vec<surrealdb::types::Value> = descriptor
            .depends_on
            .iter()
            .map(|s| surrealdb::types::Value::String(s.to_string()))
            .collect();
        payload.insert(
            "depends_on".to_string(),
            surrealdb::types::Value::Array(surrealdb::types::Array::from(depends_on_arr)),
        );

        self.supersede_state(
            entity_id,
            "attr_module_descriptor",
            surrealdb::types::Value::Object(payload),
        )
        .await
    }

    /// Write the module's enabled/disabled status as a state_ledger
    /// row.
    pub(crate) async fn write_status(
        &self,
        entity_id: RecordId,
        status: ModuleStatus,
    ) -> Result<RecordId> {
        let mut payload = surrealdb::types::Object::new();
        payload.insert(
            "value".to_string(),
            surrealdb::types::Value::String(status.as_str().to_string()),
        );
        self.supersede_state(
            entity_id,
            "attr_module_status",
            surrealdb::types::Value::Object(payload),
        )
        .await
    }

    /// Compose a [`RegistryStatus`] from substrate state for one
    /// entity. Returns `None` if its descriptor row hasn't been
    /// written yet.
    async fn read_registry_status(
        &self,
        entity_id: RecordId,
        kind: NodeKind,
    ) -> Result<Option<RegistryStatus>> {
        let desc = self
            .current_state(entity_id.clone(), "attr_module_descriptor")
            .await?;
        let Some(surrealdb::types::Value::Object(desc_obj)) = desc else {
            return Ok(None);
        };

        let name = match desc_obj.get("name") {
            Some(surrealdb::types::Value::String(s)) => s.clone(),
            _ => return Ok(None),
        };
        let version = match desc_obj.get("version") {
            Some(surrealdb::types::Value::String(s)) => s.clone(),
            _ => String::new(),
        };
        let category = match desc_obj.get("category") {
            Some(surrealdb::types::Value::String(s)) => s.clone(),
            _ => String::new(),
        };

        let lifecycle = crate::lifecycle::read_lifecycle(self, entity_id.clone())
            .await?
            .unwrap_or(crate::lifecycle::LifecycleState::Enabled);

        Ok(Some(RegistryStatus {
            name,
            version,
            kind,
            category,
            entity_id,
            lifecycle,
        }))
    }
}

/// Enabled / disabled flag persisted on `attr_module_status`. Distinct
/// from the lifecycle state (`Active` / `Failed` / etc.) — status is
/// the operator's intent, lifecycle is the runtime reality.
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

impl Kernel {
    /// Read the operator's enable/disable intent for a registered
    /// module. Returns `None` if the module isn't registered or no
    /// status row has been written yet (callers treat both as
    /// enabled — installed = enabled).
    ///
    /// # Errors
    ///
    /// [`crate::error::KernelError::Corrupt`] when a status row
    /// exists but doesn't carry `{ value: "enabled" | "disabled" }`.
    pub async fn module_status(
        &self,
        kind: NodeKind,
        name: &str,
    ) -> Result<Option<ModuleStatus>> {
        let Some(entity_id) = self.find_module_by_name(kind, name).await? else {
            return Ok(None);
        };
        let Some(value) = self
            .current_state(entity_id, "attr_module_status")
            .await?
        else {
            return Ok(None);
        };
        let surrealdb::types::Value::Object(obj) = value else {
            return Err(crate::error::KernelError::Corrupt(
                "attr_module_status payload is not an object".to_string(),
            ));
        };
        match obj.get("value") {
            Some(surrealdb::types::Value::String(s)) if s == "enabled" => {
                Ok(Some(ModuleStatus::Enabled))
            }
            Some(surrealdb::types::Value::String(s)) if s == "disabled" => {
                Ok(Some(ModuleStatus::Disabled))
            }
            other => Err(crate::error::KernelError::Corrupt(format!(
                "attr_module_status value is not 'enabled' / 'disabled': {other:?}"
            ))),
        }
    }
}
