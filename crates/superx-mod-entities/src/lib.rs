//! superx-mod-entities — entities, fields, labels and links (epic #353).
//!
//! EVERYTHING HAS A UUID7, A NAME AND LABELS. A thing, a field on a
//! thing, and a link between two things are built from the same parts; a
//! field is not a lesser kind of row with a key and a value. What a
//! thing IS, is the labels it carries, and a label is itself an entity —
//! one carrying the label `label`, with its meaning written as a field
//! on it.
//!
//! A LINK IS A FIELD THE ENGINE CAN WALK. It carries a name, labels,
//! history and an author like any other field; only its storage differs,
//! because SurrealDB grants traversal to relations and not to columns,
//! and role graphs get deep enough that the difference decides whether
//! the graph is readable at all.
//!
//! This module STORES and DESCRIBES. It enforces two things and no more:
//! a label it is given must exist, and content must be what its datatype
//! says. What a role does, whether a mandate binds, which links are
//! worth following — all of that is the reader's judgement, and this
//! module knows nothing about who the reader is.
//!
//! MODULE CONTRACT (D17). This is a self-contained mini-app, not a
//! library the kernel calls into:
//!
//!   own database   `superx/entities`, with its own service account
//!   own schema     `schema/entities.surql`, applied at provision
//!   own directory  `modules/entities/`, for anything it puts on disk
//!   own log        `target: "entities"`
//!   own UI         its own server on its own port, publishing its URL
//!                  to the substrate so the core dashboard finds it
//!                  there and never through an import (D-UI2). The port
//!                  parameter and the server arrive WITH the UI — code
//!                  for a surface that does not exist yet is the same
//!                  debt as code for one that no longer does.
//!
//! AND NO CLI, DELIBERATELY. The database is the interface: another
//! module reads these tables, and a person uses the UI. A command
//! namespace would be a third surface saying the same things in a third
//! shape, and the last one drifted from the other two — it rendered
//! prose that a reader then scraped for uuids. The trait's default
//! answers `superx entities …` with "this module has no CLI", which is
//! the truth.
//!
//! The schema is designed and locked first, code after (§11). This is
//! the ground cleared and the foundation laid; the verbs land on top of
//! it.

pub mod api;
pub mod attribute;
pub mod author;
pub mod edge;
pub mod entity;
mod server;

/// Start the module's service against a database the caller already
/// has. The dev harness (`examples/demo.rs`) uses it to drive the
/// dashboard without a provisioned instance; startup uses it with the
/// module's own db.
///
/// # Errors
///
/// [`KernelError::Module`](superx_kernel::KernelError::Module) when the
/// port cannot be bound.
pub async fn serve(kernel: &Kernel, db: superx_kernel::Db, port: u16) -> Result<()> {
    server::spawn(kernel, db, port).await
}

use async_trait::async_trait;
use superx_kernel::types::Value;
use superx_kernel::{
    Kernel, KernelModule, KernelModuleDescriptor, NodeKind, Result, KERNEL_MODULES,
};

/// The module's name — db name, dir name, CLI namespace, log target.
pub const MODULE_NAME: &str = "entities";

/// Port parameter for this module's OWN service, on its own registry
/// entity.
pub const UI_PORT_PARAM: &str = "attr_entities_ui_port";

/// Where it listens when the parameter is unset.
pub const DEFAULT_UI_PORT: u16 = 5151; // skill-allow: §9-const — bootstrap fallback, param-overridable

/// The convention every module with a UI follows (D-UI2): publish the
/// URL as a parameter on its own registry entity, so the core dashboard
/// finds it in the substrate and never through code coupling.
pub const MODULE_UI_URL_PARAM: &str = "attr_module_ui_url";

/// How many hops the graph opens at.
pub const DEFAULT_GRAPH_DEPTH: usize = 2; // skill-allow: §9-const — bootstrap fallback, param-overridable

/// The ceiling a caller may ask for, so one request cannot walk the
/// whole graph by accident.
pub const MAX_GRAPH_DEPTH: usize = 12; // skill-allow: §9-const — a bound, not a tunable

/// Resolve this module's port: the parameter on its registry entity,
/// else the fallback.
pub async fn resolved_ui_port(kernel: &Kernel) -> u16 {
    let Ok(Some(entity)) = kernel
        .find_module_by_name(NodeKind::KernelModule, MODULE_NAME)
        .await
    else {
        return DEFAULT_UI_PORT;
    };
    match kernel.get_parameter(entity, UI_PORT_PARAM).await {
        Ok(Some(Value::Number(n))) => n
            .to_int()
            .and_then(|i| u16::try_from(i).ok())
            .filter(|&p| p > 0)
            .unwrap_or(DEFAULT_UI_PORT),
        _ => DEFAULT_UI_PORT,
    }
}

/// The module's own schema, applied by `superx modules provision
/// entities` into `superx/entities` (kernel schema untouched — §7).
pub const SCHEMA_DDL: &str = include_str!("../schema/entities.surql");

/// Mint a uuid7 record id in the module's own db — time-ordered ids are
/// the historical log (the kernel's convention; its own minter is
/// crate-private).
#[must_use]
pub fn new_id(table: &'static str) -> superx_kernel::types::RecordId {
    superx_kernel::types::RecordId::new(
        table,
        superx_kernel::types::Uuid::from(uuid::Uuid::now_v7()),
    )
}

/// The entities module. A ZST — all state lives in the substrate, per
/// the module contract.
pub struct EntitiesModule;

#[async_trait]
impl KernelModule for EntitiesModule {
    fn descriptor(&self) -> KernelModuleDescriptor {
        KernelModuleDescriptor {
            name: MODULE_NAME,
            version: env!("CARGO_PKG_VERSION"),
            kind: NodeKind::KernelModule,
            depends_on: &[],
            required_metamodel: &[],
        }
    }

    async fn startup(&self, kernel: &Kernel) -> Result<()> {
        // Provisioning is an operator step. An unprovisioned module must
        // not fail the boot walk: the OS runs, and the module waits for
        // `superx modules provision entities`.
        // Resolve the db ONCE. Unprovisioned is a real state, not a
        // bug — the OS runs and this module waits for
        // `superx modules provision entities` — so the service simply
        // does not come up rather than answering every request with a
        // 503 it could have known about at boot.
        let db = match kernel.module_db(MODULE_NAME).await {
            Ok(db) => {
                tracing::info!(target: "entities", "substrate ready");
                db
            }
            Err(e) => {
                tracing::warn!(
                    target: "entities",
                    "own db unavailable — module idle until provisioned: {e}"
                );
                return Ok(());
            }
        };
        // The module's OWN service on its OWN port, and its URL
        // published to the substrate so the core dashboard discovers it
        // there rather than by importing anything (D-UI2).
        let port = resolved_ui_port(kernel).await;
        server::spawn(kernel, db, port).await?;
        let url = format!("http://127.0.0.1:{port}");
        if let Ok(Some(entity)) =
            kernel.find_module_by_name(NodeKind::KernelModule, MODULE_NAME).await
        {
            kernel
                .set_parameter(entity, MODULE_UI_URL_PARAM, Value::String(url.clone()))
                .await?;
        }
        tracing::info!(target: "entities", %url, "service listening");
        Ok(())
    }

    fn schema_ddl(&self) -> Option<&'static str> {
        Some(SCHEMA_DDL)
    }

    fn needs_dir(&self) -> bool {
        true
    }
}

#[linkme::distributed_slice(KERNEL_MODULES)]
static ENTITIES_REGISTRATION: &'static (dyn KernelModule + Sync) = &EntitiesModule;

/// A column that may be absent, with NONE and NULL both read as absent.
pub(crate) fn obj_get(row: &Value, key: &str) -> Option<Value> {
    match row {
        Value::Object(o) => o
            .get(key)
            .filter(|v| !matches!(v, Value::None | Value::Null))
            .cloned(),
        _ => None,
    }
}

pub(crate) fn obj_str(row: &Value, key: &str) -> Option<String> {
    match obj_get(row, key) {
        Some(Value::String(s)) => Some(s),
        _ => None,
    }
}

pub(crate) fn obj_record(row: &Value, key: &str) -> Option<superx_kernel::types::RecordId> {
    match obj_get(row, key) {
        Some(Value::RecordId(r)) => Some(r),
        _ => None,
    }
}

pub(crate) fn obj_records(row: &Value, key: &str) -> Vec<superx_kernel::types::RecordId> {
    match obj_get(row, key) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|i| match i {
                Value::RecordId(r) => Some(r.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// A `bool` column that may be absent. Absent is `false`.
pub(crate) fn obj_bool(row: &Value, key: &str) -> bool {
    matches!(obj_get(row, key), Some(Value::Bool(true)))
}

pub(crate) fn obj_display(row: &Value, key: &str) -> Option<String> {
    obj_get(row, key).map(|v| match v {
        Value::Datetime(d) => d.to_string(),
        other => format!("{other:?}"),
    })
}

/// The newest row of a small set, compared as PARSED datetimes. Lexical
/// RFC3339 comparison is a trap: the engine's rendering is not
/// guaranteed to sort the way the instants do.
pub(crate) fn newest_by_valid_from(rows: &[Value]) -> Option<&Value> {
    rows.iter().max_by_key(|r| {
        obj_display(r, "valid_from")
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
            .map(|t| t.with_timezone(&chrono::Utc))
            .unwrap_or(chrono::DateTime::<chrono::Utc>::MIN_UTC)
    })
}
