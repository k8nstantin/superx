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
use std::collections::HashSet;

use superx_kernel::types::{RecordId, Value};
use superx_kernel::{Db, KernelError, Result};
use superx_ops::record_uuid;
use ts_rs::TS;

use crate::{
    attachments, dictionary, documents, edges, fields, graph, nodes, notes, registry, target,
    texts,
};

/// Depth ceiling for the breadcrumb walk (#253): deep enough for any
/// real product hierarchy, shallow enough that a pathological graph
/// cannot stall a detail render.
const ANCESTOR_MAX_DEPTH: usize = 12; // skill-allow: §9-const — render-layer bound, not a policy tunable

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
    /// Hidden from the default list, still on the record.
    pub archived: bool,
}

#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct AnnotationView {
    /// The dictionary slot this prose sits in — `description`, `spec`,
    /// `comments`. Not an edge's rel_type: the carrier is the LABEL now.
    pub label: String,
    /// The note's chain id, stable across every version of it.
    pub note_uid: String,
    pub content: String,
    /// The note this one answers, for a threaded comment.
    pub parent_uid: Option<String>,
    /// `operator` | `role` | `agent` | `system`, and the role it acted as.
    pub author_kind: Option<String>,
    pub via_uid: Option<String>,
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

/// One step of the ancestor path (issue #253) — the breadcrumb the
/// dashboards render above an entity. Root first.
#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct AncestorView {
    pub id: String,
    pub name: String,
    pub entity_type: String,
    /// The edge linking this step to the one below it.
    pub rel_type: String,
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
    /// valid_from of the current state row — the version stamp. Send
    /// it back as `based_on` to make a write compare-and-append (§6).
    pub version: String,
    /// Hidden from the lists, still on the record (§14).
    pub archived: bool,
    pub annotations: Vec<AnnotationView>,
    /// Active NON-TEXT edges, both directions (text-role edges show
    /// as annotations instead).
    pub edges: Vec<EdgeView>,
    /// Files attached to this entity (EU4) — the `attached` edges,
    /// resolved to their document nodes' metadata.
    pub attachments: Vec<AttachmentView>,
    /// The ancestor path, ROOT FIRST, excluding this entity (#253).
    pub ancestors: Vec<AncestorView>,
}

/// One attached file. `size` is bytes as recorded at attach time; the
/// bytes themselves live under the module's own dir and are served
/// only through the download route.
#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct AttachmentView {
    pub id: String,
    pub name: String,
    pub mime: String,
    pub size: i64,
}

/// One node of a per-entity subgraph (EU5).
#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct GraphNodeView {
    pub id: String,
    pub entity_type: String,
    pub name: String,
    /// Hops from the root — 0 is the entity the graph is rooted at.
    pub depth: i64,
}

#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct GraphEdgeView {
    pub from: String,
    pub to: String,
    pub rel_type: String,
}

/// The subgraph reachable from one entity — the graph is PER ENTITY
/// (operator model): rooted where you opened it, never a global map.
#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct GraphView {
    pub root: String,
    pub nodes: Vec<GraphNodeView>,
    pub edges: Vec<GraphEdgeView>,
    /// The walk stopped at the depth limit — there is more out there.
    pub truncated: bool,
    pub depth: i64,
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
    /// The version this edit was based on (§6). Send it and a write
    /// that would silently clobber somebody else's is refused, with
    /// their version named so you can re-read and merge. Omit it and
    /// the old latest-wins behaviour applies — the guarantee is
    /// offered, never imposed.
    pub based_on: Option<String>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct TextReq {
    /// Who is writing. Absent means the operator, which is what a person
    /// typing into the page is.
    pub author_kind: Option<String>,
    pub author_uid: Option<String>,
    pub via_uid: Option<String>,
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
            // Nothing is a "system" type any more: prose and files
            // stopped being entities, so no kind has to be hidden from
            // the create form to keep the operator out of trouble.
            system: false,
            name: t.name,
            description: t.description,
        })
        .collect())
}

/// One term in the dictionary, as the design surface shows it.
#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct LabelView {
    pub key: String,
    /// `slot` (what an entity carries) or `link` (how entities connect).
    pub label_kind: String,
    pub display: String,
    /// How a reader must TREAT it — the closed vocabulary.
    pub semantics: String,
    pub description: Option<String>,
    /// Decides storage: prose kinds become note chains, value kinds live
    /// in the attributes bag.
    pub value_kind: Option<String>,
    pub cardinality: Option<String>,
    pub archived: bool,
    /// Link labels: what this edge will accept at each end, how it reads
    /// the other way, and whether it may close a loop.
    pub source_types: Vec<String>,
    pub target_types: Vec<String>,
    pub inverse: Option<String>,
    pub acyclic: bool,
}

/// A slot a type carries, resolved against the label it names.
#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct SlotView {
    pub label: String,
    pub required: bool,
    pub display_order: i64,
    pub active: bool,
    /// The label's own semantics, or this type's override of them.
    pub semantics: String,
    pub semantics_override: Option<String>,
    pub value_kind: Option<String>,
    pub cardinality: Option<String>,
}

/// What the design surface needs to choose from: every closed vocabulary
/// in one read, so the UI never hardcodes a list the substrate owns.
#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct VocabularyView {
    pub slot_semantics: Vec<String>,
    pub link_semantics: Vec<String>,
    pub value_kinds: Vec<String>,
    pub prose_kinds: Vec<String>,
    pub cardinalities: Vec<String>,
    pub revision: i64,
}

/// Define or redefine a label.
#[derive(Debug, Deserialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct LabelReq {
    pub key: String,
    pub label_kind: String,
    pub display: Option<String>,
    pub semantics: String,
    pub description: Option<String>,
    pub cardinality: Option<String>,
    pub value_kind: Option<String>,
    /// Link labels only: which types may sit at each end. Absent leaves
    /// what is there; an empty list clears it back to permissive.
    pub source_types: Option<Vec<String>>,
    pub target_types: Option<Vec<String>>,
    pub inverse: Option<String>,
    pub acyclic: Option<bool>,
}

/// Give a type a slot, or change the one it has.
#[derive(Debug, Deserialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct SlotReq {
    pub label: String,
    pub required: Option<bool>,
    pub semantics_override: Option<String>,
    /// Absent leaves the slot where it is; present moves it.
    pub display_order: Option<i64>,
    /// Absent leaves it as it is; `false` retires it.
    pub active: Option<bool>,
    /// Say so explicitly to remove a semantics override. Absent means
    /// "leave it alone" — omitting a field must never be how something
    /// gets cleared, because a form that mentions one thing would erase
    /// the others.
    pub clear_semantics_override: Option<bool>,
}

/// A declared field of an entity, with what it holds.
#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct FieldOffer {
    pub key: String,
    pub value_kind: String,
    pub semantics: String,
    pub description: String,
    /// Already on the type — adding it here just fills it in. False
    /// means adding it is an AD HOC field on this entity alone.
    pub on_the_type: bool,
}

#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct FieldView {
    pub key: String,
    /// Empty when the entity holds a key its type no longer declares.
    pub value_kind: String,
    pub required: bool,
    pub value: Option<String>,
    /// True when nothing declares this key any more. Reads never fail,
    /// so it surfaces rather than disappearing.
    pub undeclared: bool,
    /// The dictionary defines this label, but this entity's TYPE does
    /// not carry it — a field added ad hoc to this one thing (§6).
    /// Distinct from `undeclared`, which means nothing defines it at
    /// all: one is a deliberate exception, the other is a leftover.
    pub ad_hoc: bool,
    /// How this type treats it — the override where there is one.
    pub semantics: String,
    /// What `enum` allows, empty for every other kind.
    pub options: Vec<String>,
}

/// Every field of an entity: declared, typed, and what it holds.
///
/// # Errors
///
/// Verb errors pass through.
pub async fn entity_fields(db: &Db, fragment: &str) -> Result<Vec<FieldView>> {
    let id = nodes::resolve_entity(db, fragment).await?;
    let (entity_type, _) = nodes::anchor_info(db, &id).await?;
    let slots = dictionary::slots_for(db, &entity_type, false).await?;

    let mut out = Vec::new();
    for f in fields::of(db, &id).await? {
        let slot = slots.iter().find(|s| s.label == f.key);
        let defined = dictionary::current(db, &f.key, dictionary::SLOT).await?;
        out.push(FieldView {
            semantics: slot
                .and_then(|s| s.semantics_override.clone())
                .or_else(|| defined.as_ref().map(|d| d.semantics.clone()))
                .unwrap_or_default(),
            options: defined
                .as_ref()
                .and_then(|d| d.attributes.as_ref())
                .and_then(|a| a.get("options"))
                .map(|v| match v {
                    superx_kernel::types::Value::Array(items) => items
                        .iter()
                        .filter_map(|i| match i {
                            superx_kernel::types::Value::String(s) => Some(s.clone()),
                            _ => None,
                        })
                        .collect(),
                    _ => Vec::new(),
                })
                .unwrap_or_default(),
            ad_hoc: f.ad_hoc,
            key: f.key,
            value_kind: f.value_kind,
            required: f.required,
            value: f.value,
            undeclared: f.undeclared,
        });
    }
    Ok(out)
}

/// The slot labels this entity could take that it does not already
/// hold — the dictionary's offer, so a field is PICKED, never invented
/// inline (§6).
///
/// Prose kinds are excluded: they are notes, not values in the bag, and
/// offering them here would put a description into the attributes.
///
/// # Errors
///
/// Verb errors pass through.
pub async fn addable_fields(db: &Db, fragment: &str) -> Result<Vec<FieldOffer>> {
    let id = nodes::resolve_entity(db, fragment).await?;
    let held: std::collections::HashSet<String> =
        fields::of(db, &id).await?.into_iter().map(|f| f.key).collect();
    let (entity_type, _) = nodes::anchor_info(db, &id).await?;
    let slots = dictionary::slots_for(db, &entity_type, false).await?;

    Ok(dictionary::list(db, false)
        .await?
        .into_iter()
        .filter(|l| l.label_kind == dictionary::SLOT)
        .filter(|l| l.value_kind.as_deref().is_some_and(fields::is_value_kind))
        .filter(|l| !held.contains(&l.key))
        .map(|l| FieldOffer {
            on_the_type: slots.iter().any(|s| s.label == l.key),
            value_kind: l.value_kind.unwrap_or_default(),
            description: l.description.unwrap_or_default(),
            semantics: l.semantics,
            key: l.key,
        })
        .collect())
}

/// Promote an ad-hoc field to the type: every entity of that type
/// carries the slot from now on (§6).
///
/// Not required by default — promoting says "this belongs on the type",
/// not "every existing one is now wrong". §7: making a field required
/// does not retroactively invalidate what exists.
///
/// # Errors
///
/// Verb errors pass through.
pub async fn promote_field(db: &Db, fragment: &str, key: &str) -> Result<()> {
    let id = nodes::resolve_entity(db, fragment).await?;
    let (entity_type, _) = nodes::anchor_info(db, &id).await?;
    dictionary::bind_slot(db, &entity_type, key, false, None, &notes::Author::operator()).await
}

/// Set one declared field, checked against what its label declares.
///
/// # Errors
///
/// Verb errors pass through.
pub async fn set_field(db: &Db, fragment: &str, key: &str, value: &str) -> Result<()> {
    let id = nodes::resolve_entity(db, fragment).await?;
    fields::set(db, &id, key, value).await
}

/// One typed value, from the entity page.
#[derive(Debug, Deserialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct FieldReq {
    pub key: String,
    pub value: String,
}

/// A file attached to something, as the page shows it.
#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct AttachedView {
    pub uid: String,
    /// What the file MEANS — a PDF labelled `mandate` IS the mandate.
    pub label: String,
    pub filename: String,
    pub mime: String,
    pub size: i64,
    pub active: bool,
    pub author_kind: Option<String>,
    pub valid_from: Option<String>,
}

/// Everything that belongs to one target — an entity, a type or a label.
#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct ContentView {
    pub target_kind: String,
    pub target_uid: String,
    pub notes: Vec<AnnotationView>,
    pub files: Vec<AttachedView>,
}

/// Prose written on a target.
#[derive(Debug, Deserialize, TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct ContentNoteReq {
    pub label: String,
    pub body: String,
}

/// The content of an entity, a type or a label (#296).
///
/// A type is exactly the thing people argue about, and this is where the
/// argument lives.
///
/// # Errors
///
/// Verb errors pass through.
pub async fn content(db: &Db, kind: &str, uid: &str) -> Result<ContentView> {
    let target = target::Target::resolve(db, kind, uid).await?;
    Ok(ContentView {
        notes: notes::for_target(db, &target, false)
            .await?
            .into_iter()
            .map(|n| AnnotationView {
                label: n.label,
                note_uid: n.uid,
                content: n.body,
                parent_uid: n.parent_uid,
                author_kind: n.author_kind,
                via_uid: n.via_uid,
            })
            .collect(),
        files: attachments::for_target(db, &target, false)
            .await?
            .into_iter()
            .map(|a| AttachedView {
                uid: a.uid,
                label: a.label,
                filename: a.filename,
                mime: a.mime,
                size: a.size,
                active: a.active,
                author_kind: a.author_kind,
                valid_from: a.valid_from.map(|t| t.to_rfc3339()),
            })
            .collect(),
        target_kind: target.kind().to_string(),
        target_uid: target.uid(),
    })
}

/// Write prose on any target.
///
/// # Errors
///
/// Verb errors pass through.
pub async fn write_content_note(
    db: &Db,
    kind: &str,
    uid: &str,
    req: &ContentNoteReq,
) -> Result<String> {
    let target = target::Target::resolve(db, kind, uid).await?;
    let (note_uid, _) =
        notes::write_to_target(db, &target, &req.label, &req.body, &notes::Author::operator())
            .await?;
    Ok(note_uid)
}

/// The whole dictionary, for the design surface.
///
/// # Errors
///
/// Verb errors pass through.
/// Archive or restore an entity (§14). Returns whether anything
/// changed — archiving what is already archived is a no-op, not an
/// error.
///
/// # Errors
///
/// Verb errors pass through.
pub async fn set_archived(db: &Db, fragment: &str, archived: bool) -> Result<bool> {
    let id = nodes::resolve_entity(db, fragment).await?;
    nodes::set_archived(db, &id, archived).await
}

/// Archive or restore a label (§14): a term superseded by a better one
/// stops being offered without the dictionary losing what it meant.
///
/// # Errors
///
/// Verb errors pass through.
pub async fn set_label_archived(
    db: &Db,
    key: &str,
    kind: &str,
    archived: bool,
) -> Result<()> {
    dictionary::archive(db, key, kind, archived).await
}

pub async fn labels(db: &Db, include_archived: bool) -> Result<Vec<LabelView>> {
    Ok(dictionary::list(db, include_archived)
        .await?
        .into_iter()
        .map(|l| LabelView {
            key: l.key,
            label_kind: l.label_kind,
            display: l.display,
            semantics: l.semantics,
            description: l.description,
            value_kind: l.value_kind,
            cardinality: l.cardinality,
            archived: l.archived,
            source_types: l.source_types,
            target_types: l.target_types,
            inverse: l.inverse,
            acyclic: l.acyclic,
        })
        .collect())
}

/// Every closed vocabulary the design surface offers.
///
/// # Errors
///
/// Verb errors pass through.
pub async fn vocabulary(db: &Db) -> Result<VocabularyView> {
    Ok(VocabularyView {
        slot_semantics: dictionary::SLOT_SEMANTICS.iter().map(ToString::to_string).collect(),
        link_semantics: dictionary::LINK_SEMANTICS.iter().map(ToString::to_string).collect(),
        value_kinds: fields::VALUE_KINDS
            .iter()
            .chain(std::iter::once(&fields::SECRET_KIND))
            .map(ToString::to_string)
            .collect(),
        prose_kinds: fields::PROSE_KINDS.iter().map(ToString::to_string).collect(),
        cardinalities: vec!["one".to_string(), "many".to_string()],
        revision: dictionary::revision(db).await?,
    })
}

/// Define or redefine a label.
///
/// # Errors
///
/// Verb errors pass through.
pub async fn define_label(db: &Db, req: &LabelReq) -> Result<()> {
    dictionary::define(
        db,
        dictionary::Definition {
            key: &req.key,
            kind: &req.label_kind,
            display: req.display.as_deref().unwrap_or(&req.key),
            semantics: &req.semantics,
            description: req.description.as_deref(),
            cardinality: req.cardinality.as_deref(),
            value_kind: req.value_kind.as_deref(),
            source_types: req.source_types.as_deref(),
            target_types: req.target_types.as_deref(),
            inverse: req.inverse.as_deref(),
            acyclic: req.acyclic,
        },
    )
    .await
}

/// The slots a type carries, resolved against the dictionary.
///
/// # Errors
///
/// Verb errors pass through.
pub async fn slots(db: &Db, entity_type: &str, include_retired: bool) -> Result<Vec<SlotView>> {
    let mut out = Vec::new();
    for slot in dictionary::slots_for(db, entity_type, include_retired).await? {
        let defined = dictionary::current(db, &slot.label, dictionary::SLOT).await?;
        out.push(SlotView {
            semantics: slot
                .semantics_override
                .clone()
                .or_else(|| defined.as_ref().map(|d| d.semantics.clone()))
                .unwrap_or_default(),
            value_kind: defined.as_ref().and_then(|d| d.value_kind.clone()),
            cardinality: defined.and_then(|d| d.cardinality),
            label: slot.label,
            required: slot.required,
            display_order: slot.display_order,
            active: slot.active,
            semantics_override: slot.semantics_override,
        });
    }
    Ok(out)
}

/// Give a type a slot, move it, or retire it.
///
/// # Errors
///
/// Verb errors pass through.
pub async fn bind_slot(db: &Db, entity_type: &str, req: &SlotReq) -> Result<()> {
    let author = notes::Author::operator();
    let existing = dictionary::slots_for(db, entity_type, true)
        .await?
        .into_iter()
        .find(|s| s.label == req.label);

    match existing {
        // Already there: this is an edit, and each part is optional so a
        // form that only changes the order does not also reset the rest.
        Some(prior) => {
            // ABSENT MEANS UNCHANGED. Treating a missing field as "clear
            // it" meant a request that only set `required` also wiped the
            // §5.2 semantics override — the value that decides how an
            // agent treats the slot — with nothing in the request saying
            // so and nothing in the response admitting it.
            let clearing = req.clear_semantics_override.unwrap_or(false);
            let semantics_override = if clearing {
                None
            } else {
                req.semantics_override.clone().or_else(|| prior.semantics_override.clone())
            };
            let required = req.required.unwrap_or(prior.required);

            if required != prior.required || semantics_override != prior.semantics_override {
                dictionary::bind_slot(
                    db,
                    entity_type,
                    &req.label,
                    required,
                    semantics_override.as_deref(),
                    &author,
                )
                .await?;
            }
            if let Some(order) = req.display_order {
                dictionary::order_slot(db, entity_type, &req.label, order, &author).await?;
            }
            if let Some(active) = req.active {
                dictionary::retire_slot(db, entity_type, &req.label, active, &author).await?;
            }
            Ok(())
        }
        None => {
            dictionary::bind_slot(
                db,
                entity_type,
                &req.label,
                req.required.unwrap_or(false),
                req.semantics_override.as_deref(),
                &author,
            )
            .await
        }
    }
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
pub async fn list(
    db: &Db,
    type_filter: Option<&str>,
    include_archived: bool,
) -> Result<Vec<EntityListItem>> {
    Ok(nodes::list_entities(db, type_filter)
        .await?
        .into_iter()
        // No type filter. The carriers that used to need hiding are
        // ARCHIVED by B4, and archived is the one question the list
        // asks — "should this still be shown by default?" (§14).
        .filter(|e| include_archived || !e.archived)
        .map(|e| EntityListItem {
            id: record_uuid(&e.id),
            entity_type: e.entity_type,
            name: e.name,
            archived: e.archived,
        })
        .collect())
}

pub async fn detail(db: &Db, fragment: &str) -> Result<EntityDetail> {
    detail_at(db, fragment, None).await
}

/// THE WHOLE ENTITY AS IT STOOD AT AN INSTANT (§14).
///
/// A field-by-field picker answers "how did this text change". This
/// answers **"what did the agent see when it did that"** — the question
/// after a bad run — and it cannot be assembled from separate pickers,
/// because each one moves independently.
///
/// One instant reaches every chain: state, notes, attachments and
/// edges, all resolved at the same moment. `valid_from <= as_of`,
/// latest wins per chain, edges as they were ACTIVE then.
///
/// # Errors
///
/// [`KernelError::Module`] when the instant is unreadable or the entity
/// did not exist yet; verb errors pass through.
pub async fn detail_at(
    db: &Db,
    fragment: &str,
    as_of: crate::asof::AsOf,
) -> Result<EntityDetail> {
    let id = nodes::resolve_entity(db, fragment).await?;
    let (entity_type, _created) = nodes::anchor_info(db, &id).await?;
    let state = nodes::state_at(db, &id, as_of).await?.ok_or_else(|| {
        // Distinguishing the two is the difference between a broken
        // substrate and a question about a time before this existed.
        if as_of.is_some() {
            KernelError::Module(format!(
                "entity {} had no version at that instant — it was created later",
                record_uuid(&id)
            ))
        } else {
            KernelError::Module(format!("entity {} has no state chain", record_uuid(&id)))
        }
    })?;
    // Prose comes from the note store (#278). The text carriers still
    // exist and are still written, but nothing reads them for display any
    // more — which is what had to become true before they can go.
    let annotations = notes::for_entity_at(db, &id, false, as_of)
        .await?
        .into_iter()
        .map(|n| AnnotationView {
            label: n.label,
            note_uid: n.uid,
            content: n.body,
            parent_uid: n.parent_uid,
            author_kind: n.author_kind,
            via_uid: n.via_uid,
        })
        .collect();

    // Active NON-TEXT edges, both directions, with the far side named.
    let mut views: Vec<(RecordId, EdgeView)> = Vec::new();
    for (rows, outbound) in [
        (edges::expand_at(db, std::slice::from_ref(&id), false, as_of).await?, true),
        (edges::expand_at(db, std::slice::from_ref(&id), true, as_of).await?, false),
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
    let edges: Vec<EdgeView> = views
        .into_iter()
        .map(|(other, mut v)| {
            if let Some(m) = meta.get(&record_uuid(&other)) {
                v.other_name = m.name.clone();
                v.other_type = m.entity_type.clone();
            }
            v
        })
        .collect();

    let attachments = attachments_of(db, &id, as_of).await?;
    let ancestors = graph::ancestors(db, &id, ANCESTOR_MAX_DEPTH)
        .await?
        .into_iter()
        .map(|a| AncestorView {
            id: record_uuid(&a.id),
            name: a.name,
            entity_type: a.entity_type,
            rel_type: a.rel_type,
        })
        .collect();

    Ok(EntityDetail {
        id: record_uuid(&id),
        entity_type,
        name: state.name,
        content: state.content,
        attributes_json: attrs_to_json(&state.attributes),
        archived: state.archived,
        version: state.valid_from,
        annotations,
        edges,
        attachments,
        ancestors,
    })
}

/// The files on an entity, read from the `attachment` rows.
///
/// It used to walk the `attached` edge to a `document` node and read
/// its attributes. B4 retracted those edges and B6 stopped creating the
/// nodes, so that walk now finds nothing — the rows are the one place
/// files live (§3, §6).
async fn attachments_of(
    db: &Db,
    id: &RecordId,
    as_of: crate::asof::AsOf,
) -> Result<Vec<AttachmentView>> {
    Ok(attachments::for_target_at(db, &target::Target::Entity(id.clone()), false, as_of)
        .await?
        .into_iter()
        .map(|a| AttachmentView { id: a.uid, name: a.filename, mime: a.mime, size: a.size })
        .collect())
}

/// A string out of an attributes bag, for the legacy document rows the
/// download route still resolves.
fn attr_str(attrs: Option<&Value>, key: &str) -> Option<String> {
    match attrs {
        Some(Value::Object(o)) => match o.get(key) {
            Some(Value::String(v)) => Some(v.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// The subgraph rooted at one entity (EU5). `both` unions the forward
/// and reverse walks: an entity's neighbourhood is what it points at
/// AND what points at it — a root-level entity should show its whole
/// world, which is the operator's model for this view.
pub async fn graph_view(
    db: &Db,
    fragment: &str,
    depth: usize,
    direction: &str,
) -> Result<GraphView> {
    let id = nodes::resolve_entity(db, fragment).await?;
    let root_uuid = record_uuid(&id);
    let walks: &[bool] = match direction {
        "out" => &[false],
        "in" => &[true],
        _ => &[false, true],
    };
    let mut nodes_out: Vec<GraphNodeView> = Vec::new();
    let mut edges_out: Vec<GraphEdgeView> = Vec::new();
    let mut truncated = false;
    let mut seen_nodes: HashSet<String> = HashSet::new();
    let mut seen_edges: HashSet<String> = HashSet::new();
    for &reverse in walks {
        let sub = graph::subgraph(db, &id, depth, reverse).await?;
        truncated |= sub.truncated_at_depth;
        for n in sub.nodes {
            // The type check that used to live here is gone. Prose is
            // not a member of the product graph (operator, issue #246)
            // — and the walk now knows that from the dictionary, so it
            // never arrives to be filtered. The rule held HERE and
            // nowhere in the CLI path is exactly why the operator's
            // graph showed 1 node where the runner's showed 4 (#300).
            let uuid = record_uuid(&n.id);
            // A node reached by both walks keeps its SHALLOWEST depth.
            if let Some(prev) = nodes_out.iter_mut().find(|p| p.id == uuid) {
                prev.depth = prev.depth.min(n.depth as i64);
                continue;
            }
            if !seen_nodes.insert(uuid.clone()) {
                continue;
            }
            nodes_out.push(GraphNodeView {
                id: uuid,
                entity_type: n.entity_type,
                name: n.name,
                depth: n.depth as i64,
            });
        }
        for e in sub.edges {
            if !seen_edges.insert(e.edge_uid.clone()) {
                continue;
            }
            edges_out.push(GraphEdgeView {
                from: e.from,
                to: e.to,
                rel_type: e.rel_type,
            });
        }
    }
    // An edge whose far end was never reached would render as a stub.
    edges_out.retain(|e| seen_nodes.contains(&e.from) && seen_nodes.contains(&e.to));
    Ok(GraphView {
        root: root_uuid,
        nodes: nodes_out,
        edges: edges_out,
        truncated,
        depth: depth as i64,
    })
}

/// Where an attachment's bytes live, and what to call them on the way
/// out. Path resolution stays in the module: the stored path is a
/// substrate fact written by the attach path, never client input.
pub async fn attachment_file(db: &Db, uid: &str) -> Result<(String, String, String)> {
    // An attachment ROW, by uid. It used to resolve a `document` entity
    // and read its attributes; B4 migrated those into rows and B6
    // stopped creating them, so the row is where the path is.
    if let Some(a) = attachments::current(db, uid).await? {
        if !a.active {
            return Err(KernelError::Module(
                "that attachment was retracted — it is on the record, not on the way out".into(),
            ));
        }
        return Ok((a.path, a.filename, a.mime));
    }

    // A LEGACY document node, for an instance that has not migrated yet.
    // Reading it keeps every existing download link working across the
    // upgrade instead of breaking until the operator runs the migration.
    let id = nodes::resolve_entity(db, uid).await?;
    let state = nodes::current_state(db, &id)
        .await?
        .ok_or_else(|| KernelError::Module("attachment has no state".into()))?;
    let attrs = state.attributes.as_ref();
    let path = attrs
        .and_then(documents::stored_path)
        .ok_or_else(|| KernelError::Module("not an attachment: no stored file".into()))?;
    Ok((
        path,
        attr_str(attrs, "original_name").unwrap_or(state.name),
        attr_str(attrs, "mime").unwrap_or_else(|| "application/octet-stream".into()),
    ))
}

/// Version history, for an entity OR for a note.
///
/// #262 shipped version viewing and the UI reaches it with the id from
/// the detail payload. That id is now a note uid, so an endpoint that
/// only resolved entities would 404 on every description in the system.
/// Notes are tried first: a note uid is a full uuid and cannot collide
/// with the fragment-matching an entity lookup does.
pub async fn history(db: &Db, fragment: &str) -> Result<Vec<VersionView>> {
    let versions = notes::history(db, fragment).await?;
    if !versions.is_empty() {
        return Ok(versions
            .into_iter()
            .map(|n| VersionView {
                name: n.label,
                content: Some(n.body),
                attributes_json: None,
                valid_from: n
                    .valid_from
                    .map(|t| t.to_rfc3339())
                    .unwrap_or_default(),
            })
            .collect());
    }

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
    // Before anything is validated or written: did the thing move under
    // the writer's feet? Refusing here means no half-applied edit.
    nodes::check_fresh(db, &id, req.based_on.as_deref()).await?;
    let attrs = attrs_from_json(&req.attributes_json)?;

    // Every declared key goes through the same check whichever door it
    // came in by (#294). Without this the attributes box was a way round
    // every rule `fields::set` enforces — including the one that keeps a
    // raw credential out of the graph.
    let attrs = match attrs {
        Some(superx_kernel::types::Value::Object(bag)) => Some(
            superx_kernel::types::Value::Object(fields::validate_bag(db, &id, &bag).await?),
        ),
        other => other,
    };

    nodes::update_entity(db, &id, req.name.clone(), req.content.clone(), attrs).await?;
    Ok(record_uuid(&id))
}

pub async fn describe(db: &Db, fragment: &str, text: &str) -> Result<String> {
    let id = nodes::resolve_entity(db, fragment).await?;
    // The note's uid, not a carrier's. Prose stopped being an entity
    // in #302; returning a node id here would name something that is
    // no longer created.
    let (note_uid, _new) = texts::set_role_text(db, &id, "describes", text).await?;
    Ok(note_uid)
}

/// Add a comment, optionally as someone other than the operator.
///
/// The default stays the operator, so every caller that does not say who
/// it is keeps behaving as it did. What changes is that a caller which
/// KNOWS who it is can now say so — an agent's output recorded as the
/// operator's would be a lie in the column that exists to be trusted.
pub async fn comment(
    db: &Db,
    fragment: &str,
    text: &str,
    author: &notes::Author,
) -> Result<String> {
    let id = nodes::resolve_entity(db, fragment).await?;
    let note_uid = texts::add_comment(db, &id, text, author).await?;
    Ok(note_uid)
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
