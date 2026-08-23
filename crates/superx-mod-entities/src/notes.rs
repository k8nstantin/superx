//! Prose, as its own store (issue #268).
//!
//! A description, a spec, a mandate, a playbook and a comment are the
//! **same mechanism with a different label**. One chain shape, one
//! versioning path, one authorship story — and a new kind of attached
//! prose becomes a dictionary entry rather than a migration, which is
//! types-as-data one level down.
//!
//! Prose is not a field on `entity_state`: SCD-2 copies the whole row,
//! so a long description would be rewritten on every unrelated
//! attribute tweak. And it is not an entity, which is what it is
//! today — a product's description is currently a node inside its own
//! product graph, and every reader has to know that some nodes are not
//! real nodes.
//!
//! **The dictionary decides the write.** A label whose cardinality is
//! `one` amends its existing chain; `many` mints a new note. A label
//! the dictionary does not define is refused — a slot silently
//! accepted is a constraint that silently vanished.

use superx_kernel::types::{Object, RecordId, Value};
use superx_kernel::{Db, KernelError, Result};

use crate::dictionary::{self, SLOT};
use crate::registry::new_id;

/// Who wrote a note. Authorship is not only provenance: it is the
/// subject of authorization — "a role may not edit another role's
/// mandate" is enforceable precisely because every write says who is
/// writing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Author {
    /// `operator` | `role` | `agent` | `system`.
    pub kind: String,
    /// The author's own uuid, where there is one. A plain string, not a
    /// `record<>` — an author may live in another database entirely.
    pub uid: Option<String>,
    /// The ROLE context: "agent X, acting as role Y".
    pub via: Option<String>,
}

impl Author {
    /// The operator, at the console. What a CLI write is.
    #[must_use]
    pub fn operator() -> Self {
        Self { kind: "operator".to_string(), uid: None, via: None }
    }
}

/// One note, at one version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    /// Stable across versions — the chain key.
    pub uid: String,
    /// What the note is attached to.
    pub entity: Option<RecordId>,
    pub label: String,
    pub body: String,
    pub parent_uid: Option<String>,
    /// `false` means retracted: no longer stands, still on the record.
    pub active: bool,
    pub valid_from: Option<chrono::DateTime<chrono::Utc>>,
    pub author_kind: Option<String>,
    pub author_uid: Option<String>,
    pub via_uid: Option<String>,
}

/// Attach prose to an entity under a dictionary label.
///
/// Cardinality comes from the dictionary, not from the call site: a
/// `one` label amends the note that is already there, a `many` label
/// adds another. Returns the note's `uid` and whether it is new.
///
/// # Errors
///
/// [`KernelError::Module`] if the dictionary does not define the label
/// as a slot label; [`KernelError::Db`] for engine errors.
pub async fn write(
    db: &Db,
    entity: &RecordId,
    label: &str,
    body: &str,
    author: &Author,
) -> Result<(String, bool)> {
    let defined = dictionary::current(db, label, SLOT).await?.ok_or_else(|| {
        KernelError::Module(format!(
            "the dictionary defines no slot label '{label}' — \
             define it first (superx entities labels define), because a slot \
             nobody defined is a slot nobody can interpret"
        ))
    })?;

    // "one" means one: amend the chain that exists rather than leaving
    // two live descriptions and no rule for which one counts.
    if defined.cardinality.as_deref() == Some("one") {
        if let Some(existing) = current_for_label(db, entity, label).await? {
            append(
                db,
                Version {
                    entity,
                    uid: &existing.uid,
                    label,
                    body,
                    parent_uid: existing.parent_uid,
                    active: true,
                    author,
                },
            )
            .await?;
            return Ok((existing.uid, false));
        }
    }

    let uid = uuid::Uuid::now_v7().to_string();
    append(
        db,
        Version { entity, uid: &uid, label, body, parent_uid: None, active: true, author },
    )
    .await?;
    Ok((uid, true))
}

/// Reply to a note — a comment on a comment. Threading needs a parent,
/// and an edge has no fields to put one in, which is why this could
/// not exist while prose was an entity joined by an edge.
///
/// # Errors
///
/// [`KernelError::Module`] for an unknown label or a missing parent;
/// [`KernelError::Db`] for engine errors.
pub async fn reply(
    db: &Db,
    entity: &RecordId,
    parent_uid: &str,
    label: &str,
    body: &str,
    author: &Author,
) -> Result<String> {
    if current(db, parent_uid).await?.is_none() {
        return Err(KernelError::Module(format!(
            "no note '{parent_uid}' to reply to"
        )));
    }
    let uid = uuid::Uuid::now_v7().to_string();
    append(
        db,
        Version {
            entity,
            uid: &uid,
            label,
            body,
            parent_uid: Some(parent_uid.to_string()),
            active: true,
            author,
        },
    )
    .await?;
    Ok(uid)
}

/// Retract a note: it no longer stands, and it is still on the record.
/// Appends like every other change — a retraction that erased the thing
/// retracted would destroy the reason anyone said it.
///
/// # Errors
///
/// [`KernelError::Module`] if there is no such note; [`KernelError::Db`]
/// for engine errors.
pub async fn retract(db: &Db, entity: &RecordId, uid: &str, author: &Author) -> Result<()> {
    let Some(note) = current(db, uid).await? else {
        return Err(KernelError::Module(format!("no note '{uid}'")));
    };
    append(
        db,
        Version {
            entity,
            uid,
            label: &note.label,
            body: &note.body,
            parent_uid: note.parent_uid,
            active: false,
            author,
        },
    )
    .await
}

/// Every current note on an entity, oldest first. Retracted notes are
/// omitted unless asked for; nothing is ever deleted, so "omitted" is
/// the strongest form of gone there is.
///
/// # Errors
///
/// [`KernelError::Db`] for engine errors.
pub async fn for_entity(db: &Db, entity: &RecordId, include_retracted: bool) -> Result<Vec<Note>> {
    let mut resp = db
        .query(
            "SELECT * FROM note WHERE entity = $entity ORDER BY valid_from ASC, id ASC",
        )
        .bind(("entity", entity.clone()))
        .await?;
    let rows: Vec<Value> = resp.take(0)?;
    // Latest row per uid wins — the chain read, done once over an
    // ascending scan rather than a query per note. A BTreeMap keyed by
    // uid keeps the result in uuid7 order, which is creation order.
    let mut heads: std::collections::BTreeMap<String, Note> = std::collections::BTreeMap::new();
    for row in &rows {
        if let Some(note) = parse(row) {
            heads.insert(note.uid.clone(), note);
        }
    }
    Ok(heads
        .into_values()
        .filter(|n| include_retracted || n.active)
        .collect())
}

/// The current version of one note.
///
/// # Errors
///
/// [`KernelError::Db`] for engine errors.
pub async fn current(db: &Db, uid: &str) -> Result<Option<Note>> {
    let mut resp = db
        .query(
            "SELECT * FROM note WHERE uid = $uid ORDER BY valid_from DESC, id DESC LIMIT 1",
        )
        .bind(("uid", uid.to_string()))
        .await?;
    let rows: Vec<Value> = resp.take(0)?;
    Ok(rows.first().and_then(parse))
}

/// Every version of one note, oldest first. A description you cannot
/// read the previous wording of is a description you cannot review.
///
/// # Errors
///
/// [`KernelError::Db`] for engine errors.
pub async fn history(db: &Db, uid: &str) -> Result<Vec<Note>> {
    let mut resp = db
        .query("SELECT * FROM note WHERE uid = $uid ORDER BY valid_from ASC, id ASC")
        .bind(("uid", uid.to_string()))
        .await?;
    let rows: Vec<Value> = resp.take(0)?;
    Ok(rows.iter().filter_map(parse).collect())
}

/// The current note carrying one label on one entity, if any.
async fn current_for_label(db: &Db, entity: &RecordId, label: &str) -> Result<Option<Note>> {
    Ok(for_entity(db, entity, false)
        .await?
        .into_iter()
        .find(|n| n.label == label))
}

/// One appended version, as a value. Seven positional arguments in a
/// row is how a caller ends up silently swapping `label` and `body`.
struct Version<'a> {
    entity: &'a RecordId,
    uid: &'a str,
    label: &'a str,
    body: &'a str,
    parent_uid: Option<String>,
    active: bool,
    author: &'a Author,
}

async fn append(db: &Db, v: Version<'_>) -> Result<()> {
    let Version { entity, uid, label, body, parent_uid, active, author } = v;
    let mut row = Object::new();
    row.insert("uid".to_string(), Value::String(uid.to_string()));
    row.insert("entity".to_string(), Value::RecordId(entity.clone()));
    row.insert("label".to_string(), Value::String(label.to_string()));
    row.insert("body".to_string(), Value::String(body.to_string()));
    if let Some(parent) = parent_uid {
        row.insert("parent_uid".to_string(), Value::String(parent));
    }
    row.insert("active".to_string(), Value::Bool(active));
    row.insert("valid_from".to_string(), Value::Datetime(chrono::Utc::now().into()));
    row.insert("author_kind".to_string(), Value::String(author.kind.clone()));
    if let Some(uid) = &author.uid {
        row.insert("author_uid".to_string(), Value::String(uid.clone()));
    }
    if let Some(via) = &author.via {
        row.insert("via_uid".to_string(), Value::String(via.clone()));
    }
    db.query("CREATE $id CONTENT $row")
        .bind(("id", new_id("note")))
        .bind(("row", Value::Object(row)))
        .await?
        .check()?;
    Ok(())
}

fn parse(row: &Value) -> Option<Note> {
    let Value::Object(o) = row else { return None };
    Some(Note {
        uid: str_field(o, "uid")?,
        entity: match o.get("entity") {
            Some(Value::RecordId(r)) => Some(r.clone()),
            _ => None,
        },
        label: str_field(o, "label")?,
        body: str_field(o, "body").unwrap_or_default(),
        parent_uid: str_field(o, "parent_uid"),
        active: !matches!(o.get("active"), Some(Value::Bool(false))),
        valid_from: match o.get("valid_from") {
            Some(Value::Datetime(d)) => Some(**d),
            _ => None,
        },
        author_kind: str_field(o, "author_kind"),
        author_uid: str_field(o, "author_uid"),
        via_uid: str_field(o, "via_uid"),
    })
}

fn str_field(o: &Object, key: &str) -> Option<String> {
    match o.get(key) {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}
