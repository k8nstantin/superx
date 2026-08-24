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
use crate::nodes::anchor_info;
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

/// Where a discussion goes. A reply to a slot that may only hold one
/// thing has to land somewhere that may hold many, or answering
/// something would replace it.
pub const DISCUSSION: &str = "comments"; // skill-allow: §9-const — the module's own vocabulary, not a tunable

/// Who a write may claim to be. Closed, because a column that says
/// something nothing recognises is a column nothing can act on — and
/// this one is the subject of authorization, not decoration.
pub const AUTHOR_KINDS: [&str; 4] = ["operator", "role", "agent", "system"];

impl Author {
    /// The operator, at the console. What a CLI write is.
    #[must_use]
    pub fn operator() -> Self {
        Self { kind: "operator".to_string(), uid: None, via: None }
    }

    /// An author claimed by a caller.
    ///
    /// # Errors
    ///
    /// [`KernelError::Module`] for a kind outside the closed set. A write
    /// recorded as something nobody can interpret is worse than one
    /// recorded as unknown.
    pub fn claimed(kind: &str, uid: Option<&str>, via: Option<&str>) -> Result<Self> {
        if !AUTHOR_KINDS.contains(&kind) {
            return Err(KernelError::Module(format!(
                "author kind '{kind}' is not one of: {}",
                AUTHOR_KINDS.join(", ")
            )));
        }
        Ok(Self {
            kind: kind.to_string(),
            uid: uid.map(str::to_string),
            via: via.map(str::to_string),
        })
    }
}

/// One note, at one version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    /// Stable across versions — the chain key.
    pub uid: String,
    /// The typed link, where the target is an entity.
    pub entity: Option<RecordId>,
    /// `entity` | `type` | `label`. A note on a type has no entity link,
    /// so this is the only thing that says where it belongs.
    pub target_kind: Option<String>,
    pub target_uid: Option<String>,
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
    // `record<entity>` type-checks the SHAPE of the value; it does not
    // verify the row exists — probed on a real engine, not assumed. All
    // writes go through these verbs, which §10 already makes the
    // enforcement layer, so the check lives here. Without it a note
    // attaches to nothing and is unreachable from either direction: no
    // entity lists it, and nothing points back.
    anchor_info(db, entity).await?;

    let defined = require_label(db, label).await?;

    // "one" means one: amend the chain that exists rather than leaving
    // two live descriptions and no rule for which one counts.
    if defined.cardinality.as_deref() == Some("one") {
        if let Some(existing) = current_for_label(db, entity, label).await? {
            append(
                db,
                Version {
                    target: &crate::target::Target::Entity(entity.clone()),
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
        Version {
            target: &crate::target::Target::Entity(entity.clone()),
            uid: &uid,
            label,
            body,
            parent_uid: None,
            active: true,
            author,
        },
    )
    .await?;
    Ok((uid, true))
}

/// Reply to a note — a comment on a comment. Threading needs a parent,
/// and an edge has no fields to put one in, which is why this could not
/// exist while prose was an entity joined by an edge.
///
/// The entity comes from the PARENT, never from the caller: a thread
/// spanning two entities is not a thing anyone should be able to write
/// by passing the wrong argument.
///
/// **A reply to a SINGULAR slot is a comment on it, not a second one.**
/// Inheriting the label outright looked tidy and broke the invariant
/// `write` exists to keep: answering a description produced a second
/// live `description`, and the runner reads a task's description as its
/// orders and takes the first match — so a question about the work
/// could become the work. Threading survives; the answer is a comment
/// pointing at what it answers.
///
/// # Errors
///
/// [`KernelError::Module`] if there is no such parent; [`KernelError::Db`]
/// for engine errors.
pub async fn reply(db: &Db, parent_uid: &str, body: &str, author: &Author) -> Result<String> {
    let parent = current(db, parent_uid)
        .await?
        .ok_or_else(|| KernelError::Module(format!("no note '{parent_uid}' to reply to")))?;
    let target = attached_to(db, &parent).await?;

    let singular = dictionary::current(db, &parent.label, SLOT)
        .await?
        .and_then(|d| d.cardinality)
        .as_deref()
        == Some("one");
    let label = if singular { DISCUSSION } else { parent.label.as_str() };

    let uid = uuid::Uuid::now_v7().to_string();
    append(
        db,
        Version {
            target: &target,
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
/// Everything but `active` comes from the note itself. Taking the entity
/// as an argument meant a caller passing the wrong one would append a
/// row that MOVED the note to a different entity, silently.
///
/// # Errors
///
/// [`KernelError::Module`] if there is no such note; [`KernelError::Db`]
/// for engine errors.
pub async fn retract(db: &Db, uid: &str, author: &Author) -> Result<()> {
    let Some(note) = current(db, uid).await? else {
        return Err(KernelError::Module(format!("no note '{uid}'")));
    };
    let target = attached_to(db, &note).await?;
    append(
        db,
        Version {
            target: &target,
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

/// What a note hangs off — an entity, a type or a label.
///
/// This used to return the typed `record<entity>` link alone, so a note
/// on a TYPE could be neither retracted nor replied to: both said
/// "attached to nothing" about something plainly attached to a type. A
/// thread you cannot answer is not a thread, which is the one thing §3
/// says a type needs.
async fn attached_to(db: &Db, note: &Note) -> Result<crate::target::Target> {
    if let (Some(kind), Some(uid)) = (note.target_kind.as_deref(), note.target_uid.as_deref()) {
        return crate::target::Target::resolve(db, kind, uid).await;
    }
    // Rows written before the polymorphic columns carry only the link.
    note.entity
        .clone()
        .map(crate::target::Target::Entity)
        .ok_or_else(|| {
            KernelError::Module(format!("note '{}' is attached to nothing", note.uid))
        })
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

/// Every current note on any target — an entity, a type or a label
/// (#296). A type is exactly the thing people argue about, and this is
/// where the argument lives.
///
/// # Errors
///
/// [`KernelError::Db`] for engine errors.
pub async fn for_target(
    db: &Db,
    target: &crate::target::Target,
    include_retracted: bool,
) -> Result<Vec<Note>> {
    // An entity's notes may predate the polymorphic columns, so they are
    // still found by the typed link; a type's or a label's can only be
    // found by the pair.
    if let Some(entity) = target.entity() {
        return for_entity(db, &entity, include_retracted).await;
    }
    let mut resp = db
        .query(
            "SELECT * FROM note WHERE target_uid = $uid AND target_kind = $kind \
             ORDER BY valid_from ASC, id ASC",
        )
        .bind(("uid", target.uid()))
        .bind(("kind", target.kind().to_string()))
        .await?;
    let rows: Vec<Value> = resp.take(0)?;
    Ok(heads_of(&rows, include_retracted))
}

/// Attach prose to a type or a label.
///
/// Entities go through [`write`], which also keeps the typed link. This
/// is the same act for the two things that are not nodes.
///
/// # Errors
///
/// [`KernelError::Module`] for an undefined label; [`KernelError::Db`]
/// for engine errors.
pub async fn write_to_target(
    db: &Db,
    target: &crate::target::Target,
    label: &str,
    body: &str,
    author: &Author,
) -> Result<(String, bool)> {
    if let Some(entity) = target.entity() {
        return write(db, &entity, label, body, author).await;
    }
    let defined = require_label(db, label).await?;

    if defined.cardinality.as_deref() == Some("one") {
        if let Some(existing) = for_target(db, target, false)
            .await?
            .into_iter()
            .find(|n| n.label == label)
        {
            append(
                db,
                Version {
                    target,
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
        Version { target, uid: &uid, label, body, parent_uid: None, active: true, author },
    )
    .await?;
    Ok((uid, true))
}

/// Latest row per uid, filtered.
fn heads_of(rows: &[Value], include_retracted: bool) -> Vec<Note> {
    let mut heads: std::collections::BTreeMap<String, Note> = std::collections::BTreeMap::new();
    for row in rows {
        if let Some(note) = parse(row) {
            heads.insert(note.uid.clone(), note);
        }
    }
    heads
        .into_values()
        .filter(|n| include_retracted || n.active)
        .collect()
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
    /// An entity, a type or a label. The typed `record<entity>` link is
    /// written only when there IS one, so a note on a type is the same
    /// row shape without pretending to point at a node.
    target: &'a crate::target::Target,
    uid: &'a str,
    label: &'a str,
    body: &'a str,
    parent_uid: Option<String>,
    active: bool,
    author: &'a Author,
}

async fn append(db: &Db, v: Version<'_>) -> Result<()> {
    let Version { target, uid, label, body, parent_uid, active, author } = v;
    let mut row = Object::new();
    row.insert("uid".to_string(), Value::String(uid.to_string()));
    // Both forms (#296): the typed link where there IS one, so those rows
    // keep the engine's integrity check and readers that only know about
    // entities keep working — and the polymorphic pair always, so a note
    // on a type or a label is the same shape as a note on an entity.
    if let Some(entity) = target.entity() {
        row.insert("entity".to_string(), Value::RecordId(entity));
    }
    row.insert("target_kind".to_string(), Value::String(target.kind().to_string()));
    row.insert("target_uid".to_string(), Value::String(target.uid()));
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
        target_kind: str_field(o, "target_kind"),
        target_uid: str_field(o, "target_uid"),
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

/// The dictionary entry for a slot label, or a refusal.
///
/// Two different situations look identical from a missing lookup, and
/// conflating them is what makes this subtle:
///
/// * the dictionary was **never seeded** — the schema exists but startup
///   has not run. `superx modules provision <m>` leaves exactly that
///   window open, and module CLI verbs reach the database inside it, so
///   this is a real state and not only a test artifact.
/// * the label **genuinely does not exist**.
///
/// Seeding an EMPTY dictionary settles which one it is. An initialized
/// dictionary is never touched, so writing prose can never move the
/// revision that readers cache against — and because archiving appends
/// rather than deletes, an archived label leaves the table non-empty and
/// can never be resurrected by this path.
async fn require_label(db: &Db, label: &str) -> Result<dictionary::LabelRow> {
    if let Some(defined) = dictionary::current(db, label, SLOT).await? {
        return Ok(defined);
    }
    if dictionary::revision(db).await? == 0 {
        let seeded = dictionary::seed(db).await?;
        tracing::info!(
            target: "entities",
            seeded,
            label,
            "dictionary was never seeded — shipped vocabulary applied before the first prose write"
        );
        if let Some(defined) = dictionary::current(db, label, SLOT).await? {
            return Ok(defined);
        }
    }
    Err(KernelError::Module(format!(
        "the dictionary defines no slot label '{label}' — \
         define it first (superx entities labels define), because a slot \
         nobody defined is a slot nobody can interpret"
    )))
}

/// The current notes of MANY entities in one read, keyed by entity uuid.
///
/// A graph walk resolves each level in one query (#179) so that it costs
/// the nodes it reaches and never the table. Asking per node would put
/// that back: fifty nodes, fifty round trips, and the cost growing with
/// the graph instead of with the level.
///
/// # Errors
///
/// [`KernelError::Db`] for engine errors.
pub async fn for_entities(
    db: &Db,
    entities: &[RecordId],
    include_retracted: bool,
) -> Result<std::collections::HashMap<String, Vec<Note>>> {
    if entities.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let mut resp = db
        .query("SELECT * FROM note WHERE entity IN $entities ORDER BY valid_from ASC, id ASC")
        .bind(("entities", entities.to_vec()))
        .await?;
    let rows: Vec<Value> = resp.take(0)?;

    // Latest row per uid wins, exactly as the single-entity read does;
    // one ascending scan settles every chain at once.
    let mut heads: std::collections::BTreeMap<String, Note> = std::collections::BTreeMap::new();
    for row in &rows {
        if let Some(note) = parse(row) {
            heads.insert(note.uid.clone(), note);
        }
    }

    let mut out: std::collections::HashMap<String, Vec<Note>> =
        std::collections::HashMap::new();
    for note in heads.into_values() {
        if !include_retracted && !note.active {
            continue;
        }
        let Some(entity) = note.entity.clone() else { continue };
        out.entry(superx_ops::record_uuid(&entity)).or_default().push(note);
    }
    Ok(out)
}
