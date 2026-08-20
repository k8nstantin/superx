//! The entities UI's typed API layer (issue #231) — thin wiring over
//! the module's existing verbs, with every struct exported to
//! `ui/src/generated/` via ts-rs so the frontend type-checks against
//! the module. All functions here are pure translations: validation,
//! versioning, and telemetry semantics live in the verbs they call.
//!
//! Operator model (approved design): TYPES ARE ENTITY TYPES — the
//! registry's relation kinds are never surfaced as "types"; they
//! exist only for the link dialog's relation picker.

use serde::{Deserialize, Serialize};
use superx_kernel::message::value_from_json;
use superx_kernel::types::RecordId;
use superx_kernel::{Db, KernelError, Result};
use superx_ops::record_uuid;
use ts_rs::TS;

use crate::{edges, graph, nodes, registry, texts};

#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct TypeView {
    pub name: String,
    pub description: Option<String>,
    /// The module creates these itself (the `text` carrier) — the
    /// create form must not offer them.
    pub system: bool,
}

#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct EntityListItem {
    pub id: String,
    pub entity_type: String,
    pub name: String,
}

#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct AnnotationView {
    pub rel_type: String,
    pub text_id: String,
    pub content: String,
}

#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct EdgeView {
    pub edge_uid: String,
    pub rel_type: String,
    /// The entity on the far side of the edge.
    pub other_id: String,
    pub other_name: String,
    pub other_type: String,
    /// true = this entity → other; false = other → this entity.
    pub outbound: bool,
}

#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct EntityDetail {
    pub id: String,
    pub entity_type: String,
    pub name: String,
    /// Markdown (BlockNote round-trip, D-UI5) — structured for
    /// agents, readable everywhere.
    pub content: Option<String>,
    /// Pretty-printed attributes JSON, absent when empty.
    pub attributes_json: Option<String>,
    /// valid_from of the current state row — the version stamp.
    pub version: String,
    pub annotations: Vec<AnnotationView>,
    /// Active NON-TEXT edges, both directions (text-role edges show
    /// as annotations instead).
    pub edges: Vec<EdgeView>,
}

#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct VersionView {
    pub name: String,
    pub content: Option<String>,
    pub attributes_json: Option<String>,
    pub valid_from: String,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct CreateReq {
    pub entity_type: String,
    pub name: String,
    /// Optional description — becomes a text node linked describes→.
    pub description: Option<String>,
    pub content: Option<String>,
    pub attributes_json: Option<String>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct UpdateReq {
    pub name: Option<String>,
    pub content: Option<String>,
    /// REPLACES the whole attributes object when present (the
    /// module's update semantics); omit to keep it.
    pub attributes_json: Option<String>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct TextReq {
    /// Markdown from the standard editor.
    pub text: String,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct LinkReq {
    /// Target entity: uuid or unique fragment.
    pub to: String,
    pub rel: String,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct TypeReq {
    pub name: String,
    pub description: Option<String>,
}

fn attrs_from_json(s: &Option<String>) -> Result<Option<superx_kernel::types::Value>> {
    match s {
        None => Ok(None),
        Some(raw) if raw.trim().is_empty() => Ok(None),
        Some(raw) => {
            let v: serde_json::Value = serde_json::from_str(raw)
                .map_err(|e| KernelError::Module(format!("attributes is not valid JSON: {e}")))?;
            Ok(Some(value_from_json(&v)))
        }
    }
}

fn attrs_to_json(v: &Option<superx_kernel::types::Value>) -> Option<String> {
    let v = v.as_ref()?;
    let json = nodes::value_to_json(v);
    if json.as_object().is_some_and(|o| o.is_empty()) {
        return None;
    }
    serde_json::to_string_pretty(&json).ok()
}

/// The module's own annotation carrier: descriptions, comments and
/// instructions are `text` entities hung off their target by a role
/// edge (`texts::set_role_text` / `add_comment`). It is a real
/// registered type, but nothing hand-creates one — writing a
/// description IS creating it.
const TEXT_TYPE: &str = "text"; // skill-allow: §9-const — the module's own data model (texts.rs, graph.rs), not a tunable

/// ENTITY types only — relation kinds are not "types" (operator
/// model); they surface solely through [`rel_types`]. The carrier type
/// is flagged, not hidden: the registry stays honest and the create
/// form drops it.
pub async fn types_list(db: &Db) -> Result<Vec<TypeView>> {
    Ok(registry::list_types(db)
        .await?
        .into_iter()
        .filter(|t| t.category == "entity")
        .map(|t| TypeView {
            system: t.name == TEXT_TYPE,
            name: t.name,
            description: t.description,
        })
        .collect())
}

/// Create a new ENTITY type (the UI never creates relation kinds).
pub async fn types_add(db: &Db, req: &TypeReq) -> Result<()> {
    registry::add_type(db, &req.name, "entity", req.description.as_deref()).await
}

/// Relation kinds, for the link dialog's picker only.
pub async fn rel_types(db: &Db) -> Result<Vec<String>> {
    Ok(registry::list_types(db)
        .await?
        .into_iter()
        .filter(|t| t.category == "relation")
        .map(|t| t.name)
        .collect())
}

/// The entity list. Text nodes are annotations of the entity they hang
/// off — every comment and description would otherwise land here as a
/// row of its own — so the unfiltered list omits them; asking for the
/// carrier type by name still returns them.
pub async fn list(db: &Db, type_filter: Option<&str>) -> Result<Vec<EntityListItem>> {
    Ok(nodes::list_entities(db, type_filter)
        .await?
        .into_iter()
        .filter(|e| type_filter.is_some() || e.entity_type != TEXT_TYPE)
        .map(|e| EntityListItem {
            id: record_uuid(&e.id),
            entity_type: e.entity_type,
            name: e.name,
        })
        .collect())
}

pub async fn detail(db: &Db, fragment: &str) -> Result<EntityDetail> {
    let id = nodes::resolve_entity(db, fragment).await?;
    let (entity_type, _created) = nodes::anchor_info(db, &id).await?;
    let state = nodes::current_state(db, &id).await?.ok_or_else(|| {
        KernelError::Module(format!("entity {} has no state chain", record_uuid(&id)))
    })?;
    let annotations = texts::annotations(db, &id)
        .await?
        .into_iter()
        .map(|a| AnnotationView {
            rel_type: a.rel_type,
            text_id: record_uuid(&a.text_id),
            content: a.content,
        })
        .collect();

    // Active NON-TEXT edges, both directions, with the far side named.
    let mut views: Vec<(RecordId, EdgeView)> = Vec::new();
    for (rows, outbound) in [
        (edges::expand(db, std::slice::from_ref(&id), false).await?, true),
        (edges::expand(db, std::slice::from_ref(&id), true).await?, false),
    ] {
        for e in rows {
            if !e.active || texts::TEXT_ROLES.contains(&e.rel_type.as_str()) {
                continue;
            }
            let other = if outbound { e.to.clone() } else { e.from.clone() };
            views.push((
                other.clone(),
                EdgeView {
                    edge_uid: e.edge_uid,
                    rel_type: e.rel_type,
                    other_id: record_uuid(&other),
                    other_name: String::new(),
                    other_type: String::new(),
                    outbound,
                },
            ));
        }
    }
    let others: Vec<RecordId> = views.iter().map(|(o, _)| o.clone()).collect();
    let meta = nodes::current_meta(db, &others).await?;
    let edges = views
        .into_iter()
        .map(|(other, mut v)| {
            if let Some(m) = meta.get(&record_uuid(&other)) {
                v.other_name = m.name.clone();
                v.other_type = m.entity_type.clone();
            }
            v
        })
        .collect();

    Ok(EntityDetail {
        id: record_uuid(&id),
        entity_type,
        name: state.name,
        content: state.content,
        attributes_json: attrs_to_json(&state.attributes),
        version: state.valid_from,
        annotations,
        edges,
    })
}

pub async fn history(db: &Db, fragment: &str) -> Result<Vec<VersionView>> {
    let id = nodes::resolve_entity(db, fragment).await?;
    Ok(nodes::state_history(db, &id)
        .await?
        .into_iter()
        .map(|s| VersionView {
            name: s.name,
            content: s.content,
            attributes_json: attrs_to_json(&s.attributes),
            valid_from: s.valid_from,
        })
        .collect())
}

pub async fn create(db: &Db, req: &CreateReq) -> Result<String> {
    let attrs = attrs_from_json(&req.attributes_json)?;
    let id = nodes::create_entity(db, &req.entity_type, &req.name, req.content.clone(), attrs)
        .await?;
    if let Some(desc) = req.description.as_deref() {
        if !desc.trim().is_empty() {
            texts::set_role_text(db, &id, "describes", desc).await?;
        }
    }
    Ok(record_uuid(&id))
}

pub async fn update(db: &Db, fragment: &str, req: &UpdateReq) -> Result<String> {
    let id = nodes::resolve_entity(db, fragment).await?;
    let attrs = attrs_from_json(&req.attributes_json)?;
    nodes::update_entity(db, &id, req.name.clone(), req.content.clone(), attrs).await?;
    Ok(record_uuid(&id))
}

pub async fn describe(db: &Db, fragment: &str, text: &str) -> Result<String> {
    let id = nodes::resolve_entity(db, fragment).await?;
    let (text_id, _new) = texts::set_role_text(db, &id, "describes", text).await?;
    Ok(record_uuid(&text_id))
}

pub async fn comment(db: &Db, fragment: &str, text: &str) -> Result<String> {
    let id = nodes::resolve_entity(db, fragment).await?;
    let text_id = texts::add_comment(db, &id, text).await?;
    Ok(record_uuid(&text_id))
}

pub async fn link(db: &Db, fragment: &str, req: &LinkReq) -> Result<String> {
    let from = nodes::resolve_entity(db, fragment).await?;
    let to = nodes::resolve_entity(db, &req.to).await?;
    edges::link(db, &from, &to, &req.rel).await
}

pub async fn unlink(db: &Db, fragment: &str, req: &LinkReq) -> Result<String> {
    let from = nodes::resolve_entity(db, fragment).await?;
    let to = nodes::resolve_entity(db, &req.to).await?;
    edges::unlink(db, &from, &to, &req.rel).await
}

/// Keep the graph module linked for EU5 (the Graph button's phase).
#[allow(dead_code)] // skill-allow: allow — EU5 lands the graph endpoint over this
fn _eu5_uses_graph() {
    let _ = graph::render_tree;
}
