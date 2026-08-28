//! The HTTP surface, as typed views.
//!
//! Everything here is a thin adapter over [`crate::entity`],
//! [`crate::attribute`] and [`crate::edge`]: it renames nothing,
//! interprets nothing, and decides nothing. Its whole job is to turn
//! record ids into strings a browser can hold and back again.
//!
//! The view types are exported to TypeScript by ts-rs, so the UI reads
//! the contract from the module rather than from a hand-written copy
//! that drifts the first time either side changes.

use serde::{Deserialize, Serialize};
use superx_kernel::types::{Number, RecordId, Value};
use superx_kernel::{Db, KernelError, Result};
use superx_ops::record_uuid;
use ts_rs::TS;

use crate::attribute::{self, Write};
use crate::author::Author;
use crate::edge::{self, Direction};
use crate::entity;

/// An attribute, as the browser sees it.
#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct AttributeView {
    /// Identity across versions — what a saved layout points at.
    pub uid: String,
    pub name: String,
    pub datatype: String,
    /// The value, as JSON. `null` when the attribute carries only
    /// labels, which is how it says what the entity IS.
    #[ts(type = "unknown")]
    pub content: serde_json::Value,
    /// Label uuids. What they MEAN is on the label entity itself.
    pub labels: Vec<String>,
    /// Per-field overrides — a width, a placeholder.
    #[ts(type = "unknown")]
    pub options: serde_json::Value,
    pub active: bool,
    pub version: String,
}

/// An entity opened on its own page.
#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct EntityView {
    pub uuid: String,
    pub name: String,
    /// What it IS: the labels on attributes that hold nothing.
    pub labels: Vec<LabelView>,
    pub archived: bool,
    /// In the order they were added — uuid7 is time-ordered, so this is
    /// creation order and nothing stored it.
    pub attributes: Vec<AttributeView>,
    pub links: Vec<LinkView>,
}

/// A label, named. Enough to render a chip without a second request.
#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct LabelView {
    pub uuid: String,
    pub name: String,
}

/// One connection, from the point of view of the entity you are on.
#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct LinkView {
    pub uid: String,
    pub name: String,
    /// Where it points, whichever end that is.
    pub other: String,
    pub other_name: String,
    /// True when this entity is the one doing the pointing.
    pub outbound: bool,
    pub labels: Vec<LabelView>,
}

/// A node of the graph view.
#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct GraphNodeView {
    pub uuid: String,
    pub name: String,
    pub depth: usize,
    pub labels: Vec<LabelView>,
}

/// An edge of the graph view.
#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct GraphEdgeView {
    pub uid: String,
    pub name: String,
    pub from: String,
    pub to: String,
    pub labels: Vec<LabelView>,
}

/// A subgraph, rooted where it was opened.
#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct GraphView {
    pub nodes: Vec<GraphNodeView>,
    pub edges: Vec<GraphEdgeView>,
}

/// One row of the menu tree. `has_children` is what lets the tree load
/// a level at a time instead of the whole graph.
#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct TreeNodeView {
    pub uuid: String,
    pub name: String,
    pub labels: Vec<LabelView>,
    pub has_children: bool,
    /// The edge that got us here, so the tree can show what the
    /// relationship is rather than just the child.
    pub via: Option<String>,
}

/// What the browser sends to create an entity.
#[derive(Debug, Deserialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct CreateReq {
    pub name: String,
}

/// What the browser sends to add or amend an attribute.
#[derive(Debug, Deserialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct AttributeReq {
    /// Absent to add; present to amend that chain.
    pub uid: Option<String>,
    pub name: String,
    pub datatype: String,
    #[ts(type = "unknown | null")]
    pub content: Option<serde_json::Value>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[ts(type = "unknown | null")]
    pub options: Option<serde_json::Value>,
}

/// What the browser sends to link two entities.
#[derive(Debug, Deserialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct LinkReq {
    pub to: String,
    pub name: String,
    #[serde(default)]
    pub labels: Vec<String>,
}

/// Everything about one entity, for its page.
///
/// # Errors
///
/// Verb errors pass through.
pub async fn detail(db: &Db, fragment: &str) -> Result<EntityView> {
    let id = entity::resolve(db, fragment).await?;
    // ONE read, four answers: the fields, the name, what it IS, and
    // whether it is archived. This used to ask the same question of the
    // same entity four times.
    let attributes = attribute::of(db, &id, false).await?;
    let mut names = NameCache::default();
    let labels = names.views(db, &entity::labels_in(&attributes)).await?;
    let mut links = Vec::new();
    for e in edge::of(db, &id, Direction::Both).await? {
        let outbound = record_uuid(&e.from) == record_uuid(&id);
        let other = if outbound { e.to.clone() } else { e.from.clone() };
        links.push(LinkView {
            uid: e.uid,
            name: e.name,
            other_name: entity::name_of(db, &other).await?.unwrap_or_default(),
            other: record_uuid(&other),
            outbound,
            labels: names.views(db, &e.labels).await?,
        });
    }
    let name = entity::name_in(&attributes).unwrap_or_default();
    let archived = entity::archived_in(&attributes);
    let views: Vec<AttributeView> = attributes.into_iter().map(attribute_view).collect();
    Ok(EntityView {
        uuid: record_uuid(&id),
        name,
        labels,
        archived,
        attributes: views,
        links,
    })
}

/// The children of one entity in the menu tree: one level, on expand.
///
/// `label` narrows which connections are followed, which is what makes a
/// tree out of a graph — "show me what this contains" is a hierarchy,
/// "show me everything" is a web.
///
/// # Errors
///
/// Verb errors pass through.
pub async fn children(
    db: &Db,
    fragment: &str,
    label: Option<&str>,
) -> Result<Vec<TreeNodeView>> {
    let id = entity::resolve(db, fragment).await?;
    let filter = match label {
        Some(l) => Some(entity::resolve(db, l).await?),
        None => None,
    };
    let mut out = Vec::new();
    for e in edge::of(db, &id, Direction::Out).await? {
        if let Some(f) = &filter {
            if !e.labels.contains(f) {
                continue;
            }
        }
        let child = e.to.clone();
        // One more hop only to answer "is there an arrow to draw" — the
        // tree needs it to decide whether the row expands at all.
        let deeper = edge::of(db, &child, Direction::Out).await?;
        out.push(TreeNodeView {
            name: entity::name_of(db, &child).await?.unwrap_or_default(),
            labels: label_views(db, &entity::labels_of(db, &child).await?).await?,
            has_children: deeper
                .iter()
                .any(|d| filter.as_ref().is_none_or(|f| d.labels.contains(f))),
            via: Some(e.name),
            uuid: record_uuid(&child),
        });
    }
    Ok(out)
}

/// Every entity, for the top of the menu.
///
/// # Errors
///
/// Verb errors pass through.
pub async fn roots(db: &Db, include_archived: bool) -> Result<Vec<TreeNodeView>> {
    // ONE read of every entity's attributes, and one name lookup per
    // DISTINCT label. Asking per row turned the first screen into
    // thousands of round trips.
    let ids = entity::list(db, include_archived).await?;
    let held = attribute::of_many(db, &ids, false).await?;
    let mut names = NameCache::default();
    let mut out = Vec::new();
    for id in &ids {
        let mine = held.get(&record_uuid(id)).map_or(&[][..], Vec::as_slice);
        let labels = entity::labels_in(mine);
        out.push(TreeNodeView {
            name: entity::name_in(mine).unwrap_or_default(),
            labels: names.views(db, &labels).await?,
            has_children: !edge::of(db, id, Direction::Out).await?.is_empty(),
            via: None,
            uuid: record_uuid(id),
        });
    }
    Ok(out)
}

/// One name lookup per distinct entity, however often it is referenced.
/// A label appears on nearly every row, and asking for its name each
/// time is the difference between one query and hundreds.
#[derive(Default)]
struct NameCache(std::collections::HashMap<String, String>);

impl NameCache {
    async fn views(&mut self, db: &Db, ids: &[RecordId]) -> Result<Vec<LabelView>> {
        let mut out = Vec::new();
        for id in ids {
            let key = record_uuid(id);
            let name = match self.0.get(&key) {
                Some(n) => n.clone(),
                None => {
                    let n = entity::name_of(db, id).await?.unwrap_or_default();
                    self.0.insert(key.clone(), n.clone());
                    n
                }
            };
            out.push(LabelView { uuid: key, name });
        }
        Ok(out)
    }
}

/// The subgraph around one entity, for the graph tab.
///
/// # Errors
///
/// Verb errors pass through.
pub async fn graph(
    db: &Db,
    fragment: &str,
    label: Option<&str>,
    depth: usize,
) -> Result<GraphView> {
    let id = entity::resolve(db, fragment).await?;
    let filter = match label {
        Some(l) => Some(entity::resolve(db, l).await?),
        None => None,
    };
    let sub = edge::walk(db, &id, filter.as_ref(), depth).await?;
    let mut nodes = Vec::new();
    for n in &sub.nodes {
        nodes.push(GraphNodeView {
            uuid: record_uuid(&n.entity),
            name: entity::name_of(db, &n.entity).await?.unwrap_or_default(),
            depth: n.depth,
            labels: label_views(db, &entity::labels_of(db, &n.entity).await?).await?,
        });
    }
    let mut edges = Vec::new();
    for e in &sub.edges {
        edges.push(GraphEdgeView {
            uid: e.uid.clone(),
            name: e.name.clone(),
            from: record_uuid(&e.from),
            to: record_uuid(&e.to),
            labels: label_views(db, &e.labels).await?,
        });
    }
    Ok(GraphView { nodes, edges })
}

/// Create an entity.
///
/// # Errors
///
/// Verb errors pass through.
pub async fn create(db: &Db, req: &CreateReq, author: &Author) -> Result<String> {
    Ok(record_uuid(&entity::create(db, &req.name, author).await?))
}

/// Add or amend an attribute.
///
/// # Errors
///
/// Verb errors pass through, including the datatype gate.
pub async fn put_attribute(
    db: &Db,
    fragment: &str,
    req: &AttributeReq,
    author: &Author,
) -> Result<String> {
    let id = entity::resolve(db, fragment).await?;
    let mut labels = Vec::new();
    for l in &req.labels {
        labels.push(entity::resolve(db, l).await?);
    }
    let content = req
        .content
        .as_ref()
        .filter(|c| !c.is_null())
        .map(|c| from_json(&req.datatype, c))
        .transpose()?;
    let options = req
        .options
        .as_ref()
        .filter(|o| !o.is_null())
        .map(superx_kernel::message::value_from_json);
    let w = Write {
        name: &req.name,
        datatype: &req.datatype,
        content,
        labels: &labels,
        options,
    };
    match &req.uid {
        Some(uid) => {
            // THE URL SAYS WHOSE ATTRIBUTE THIS IS. Without this check a
            // request addressed to one entity could amend a chain
            // belonging to another — renaming it, or turning it into
            // `archived`/`boolean` to hide something.
            let existing = attribute::current(db, uid).await?.ok_or_else(|| {
                KernelError::Module(format!("no attribute '{uid}'"))
            })?;
            if record_uuid(&existing.entity) != record_uuid(&id) {
                return Err(KernelError::Module(format!(
                    "attribute '{uid}' does not belong to this entity"
                )));
            }
            attribute::amend(db, uid, w, author).await?;
            Ok(uid.clone())
        }
        None => attribute::add(db, &id, w, author).await,
    }
}

/// Link two entities.
///
/// # Errors
///
/// Verb errors pass through.
pub async fn link(
    db: &Db,
    fragment: &str,
    req: &LinkReq,
    author: &Author,
) -> Result<String> {
    let from = entity::resolve(db, fragment).await?;
    let to = entity::resolve(db, &req.to).await?;
    let mut labels = Vec::new();
    for l in &req.labels {
        labels.push(entity::resolve(db, l).await?);
    }
    edge::link(db, &from, &to, &req.name, &labels, author).await
}

/// JSON in, a typed value out.
///
/// `value_from_json` has no datetime — JSON has no such literal — so an
/// instant arrives as a string and the datatype gate refused it, which
/// made `datetime` unwritable through the only write path there is. The
/// conversion belongs here, at the boundary where the declared datatype
/// is known.
fn from_json(datatype: &str, c: &serde_json::Value) -> Result<Value> {
    if datatype != "datetime" {
        return Ok(superx_kernel::message::value_from_json(c));
    }
    let s = c.as_str().ok_or_else(|| {
        KernelError::Module(
            "a datetime is sent as an RFC 3339 string, e.g. 2026-08-28T12:34:00Z".to_string(),
        )
    })?;
    let parsed = chrono::DateTime::parse_from_rfc3339(s).map_err(|e| {
        KernelError::Module(format!("'{s}' is not an RFC 3339 instant: {e}"))
    })?;
    Ok(Value::Datetime(parsed.with_timezone(&chrono::Utc).into()))
}

async fn label_view(db: &Db, id: &RecordId) -> Result<LabelView> {
    Ok(LabelView {
        uuid: record_uuid(id),
        name: entity::name_of(db, id).await?.unwrap_or_default(),
    })
}

async fn label_views(db: &Db, ids: &[RecordId]) -> Result<Vec<LabelView>> {
    let mut out = Vec::new();
    for id in ids {
        out.push(label_view(db, id).await?);
    }
    Ok(out)
}

fn attribute_view(a: attribute::Attribute) -> AttributeView {
    AttributeView {
        uid: a.uid,
        name: a.name,
        datatype: a.datatype,
        content: a.content.as_ref().map_or(serde_json::Value::Null, to_json),
        labels: a.labels.iter().map(record_uuid).collect(),
        options: a.options.as_ref().map_or(serde_json::Value::Null, to_json),
        active: a.active,
        version: a.version,
    }
}

/// Render a dynamic value as JSON for the browser.
fn to_json(v: &Value) -> serde_json::Value {
    match v {
        Value::String(s) => serde_json::Value::String(s.clone()),
        Value::Bool(b) => serde_json::Value::Bool(*b),
        // MATCH THE VARIANT, do not ask `to_int`. It answers `Some` for a
        // Float too — `Some(v as i64)` — so 19.99 came back as 19, the
        // form rendered 19, and the next blur wrote 19 over the stored
        // value. A price or a ratio was destroyed by being looked at.
        Value::Number(Number::Int(i)) => serde_json::Value::Number((*i).into()),
        Value::Number(Number::Float(f)) => serde_json::Number::from_f64(*f)
            .map_or(serde_json::Value::Null, serde_json::Value::Number),
        Value::Number(Number::Decimal(d)) => d
            .to_string()
            .parse::<serde_json::Number>()
            .map_or_else(|_| serde_json::Value::String(d.to_string()), serde_json::Value::Number),
        Value::Datetime(d) => serde_json::Value::String(d.to_string()),
        Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(to_json).collect())
        }
        Value::Object(o) => serde_json::Value::Object(
            o.iter().map(|(k, val)| (k.clone(), to_json(val))).collect(),
        ),
        _ => serde_json::Value::Null,
    }
}
