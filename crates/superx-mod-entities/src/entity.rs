//! Entities — the identity, and how to find one.
//!
//! An entity is a uuid7 and the instant it was issued. Nothing else.
//! Like a social security number: you can change your name, and you are
//! still you. That is why every edge and every label in the database
//! anchors to it — a rename cannot ripple through the graph, because the
//! name was never in the graph.
//!
//! Everything you would expect to find here — the name, what it is, what
//! it holds — is an [`crate::attribute`], and that is not a workaround.
//! It is what makes an entity extensible without this file changing.

use superx_kernel::types::{Object, RecordId, Value};
use superx_kernel::{Db, KernelError, Result};
use superx_ops::record_uuid;

use crate::attribute::{self, Write};
use crate::author::Author;
use crate::{new_id, obj_record};

/// The attribute every entity carries. A convention, not a schema rule:
/// the store does not require it, and an entity without one still reads
/// — it simply has nothing to show in a list.
pub const NAME: &str = "name"; // skill-allow: §9-const — the module's own vocabulary, not a tunable

/// The attribute that says an entity has been put away.
///
/// Archiving is NOT a column on the anchor: the anchor is identity and
/// identity does not change. It is a boolean attribute, which means
/// every archive and every restore is a versioned event with an author
/// and a time — you can see who put a thing away and when, and undo it.
pub const ARCHIVED: &str = "archived"; // skill-allow: §9-const — the module's own vocabulary, not a tunable

/// The attribute that says what an entity IS. Its labels are the
/// ENTITY's labels; every other attribute's labels are that FIELD's own
/// (operator, 2026-09-03: "entities have labels, fields have labels").
///
/// Reading every content-less labelled attribute as a declaration was
/// wrong the moment a field could be born with a label and no value: add
/// `notes` labelled `mandate` to an entity and, until something was
/// typed into it, the ENTITY read as a mandate. A third name the module
/// knows, and like the other two a storage concern — which row to read
/// for what — not a meaning.
pub const IS: &str = "is"; // skill-allow: §9-const — the module's own vocabulary, not a tunable

/// Mint an entity and give it a name.
///
/// Two rows, one act: the identity, then the name that identity is known
/// by. Naming is not built into the anchor precisely so it can change
/// without the anchor changing.
///
/// # Errors
///
/// [`KernelError::Module`] for an empty name; [`KernelError::Db`] for
/// engine errors.
pub async fn create(db: &Db, name: &str, author: &Author) -> Result<RecordId> {
    let name = name.trim();
    if name.is_empty() {
        return Err(KernelError::Module(
            "an entity needs a name — something has to appear in a list".to_string(),
        ));
    }
    let anchor = new_id("entity");
    let mut row = Object::new();
    row.insert("created_at".to_string(), Value::Datetime(chrono::Utc::now().into()));
    db.query("CREATE $id CONTENT $row")
        .bind(("id", anchor.clone()))
        .bind(("row", Value::Object(row)))
        .await?
        .check()?;

    attribute::add(
        db,
        &anchor,
        Write {
            name: NAME,
            datatype: "text",
            content: Some(Value::String(name.to_string())),
            labels: &[],
            options: None,
        },
        author,
    )
    .await?;
    Ok(anchor)
}

/// What this entity is currently called, if it has been named.
///
/// # Errors
///
/// [`KernelError::Db`] for engine errors.
pub async fn name_of(db: &Db, anchor: &RecordId) -> Result<Option<String>> {
    Ok(name_in(&attribute::of(db, anchor, false).await?))
}

/// The same answer from attributes a caller ALREADY has.
///
/// Every read of a name used to cost its own query, and a listing asked
/// four separate times for the same entity's attributes — its name, its
/// labels, whether it was archived, and its fields. One read, four
/// answers.
#[must_use]
pub fn name_in(attributes: &[attribute::Attribute]) -> Option<String> {
    attributes.iter().find(|a| a.name == NAME).and_then(|a| match &a.content {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    })
}

/// What a thing IS, from attributes already in hand: the labels on its
/// [`IS`] rows, and nothing a field carries for itself.
#[must_use]
pub fn labels_in(attributes: &[attribute::Attribute]) -> Vec<RecordId> {
    let mut out: Vec<RecordId> = Vec::new();
    for a in attributes.iter().filter(|a| a.name == IS) {
        for l in &a.labels {
            if !out.contains(l) {
                out.push(l.clone());
            }
        }
    }
    out
}

/// Whether it is put away, from attributes already in hand.
#[must_use]
pub fn archived_in(attributes: &[attribute::Attribute]) -> bool {
    attributes
        .iter()
        .find(|a| a.name == ARCHIVED)
        .is_some_and(|a| matches!(a.content, Some(Value::Bool(true))))
}

/// Rename: a new version of the name attribute. The identity does not
/// move, so nothing pointing at this entity notices.
///
/// # Errors
///
/// [`KernelError::Module`] for an empty name or an entity that was never
/// named; [`KernelError::Db`] for engine errors.
pub async fn rename(db: &Db, anchor: &RecordId, name: &str, author: &Author) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err(KernelError::Module(
            "an entity needs a name — something has to appear in a list".to_string(),
        ));
    }
    let existing = attribute::of(db, anchor, true)
        .await?
        .into_iter()
        .find(|a| a.name == NAME)
        .ok_or_else(|| {
            KernelError::Module(format!(
                "entity {} has no name to change",
                record_uuid(anchor)
            ))
        })?;
    attribute::amend(
        db,
        &existing.uid,
        Write {
            name: NAME,
            datatype: "text",
            content: Some(Value::String(name.to_string())),
            labels: &existing.labels,
            options: existing.options.clone(),
        },
        author,
    )
    .await
}

/// WHAT THIS ENTITY IS: the labels carried by its attributes that hold
/// nothing.
///
/// An attribute with labels and no content is an assertion about the
/// ENTITY — "this is a role". One with content is a value, and its
/// labels describe the VALUE — "this text is a mandate". Keeping the two
/// apart is the difference between "the DBA is a role" and "the DBA has
/// a rule that binds".
///
/// # Errors
///
/// [`KernelError::Db`] for engine errors.
pub async fn labels_of(db: &Db, anchor: &RecordId) -> Result<Vec<RecordId>> {
    Ok(labels_in(&attribute::of(db, anchor, false).await?))
}

/// Say what an entity IS: one attribute, labels, no content. Only a row
/// named [`IS`] is read back as the entity's labels; `name` is a
/// parameter so a caller can still write a labelled, content-less row
/// under any other name and have it mean nothing to the entity.
///
/// # Errors
///
/// [`KernelError::Db`] for engine errors, including a label that does
/// not exist.
pub async fn declare(
    db: &Db,
    anchor: &RecordId,
    name: &str,
    labels: &[RecordId],
    author: &Author,
) -> Result<String> {
    attribute::add(
        db,
        anchor,
        Write { name, datatype: "text", content: None, labels, options: None },
        author,
    )
    .await
}

/// Put an entity away. Idempotent — returns whether anything was
/// written.
///
/// Nothing is deleted, here or anywhere: the entity keeps its identity,
/// its attributes and its edges, and simply stops appearing in the
/// lists. [`unarchive`] is the same act in reverse.
///
/// # Errors
///
/// [`KernelError::Db`] for engine errors.
pub async fn archive(db: &Db, anchor: &RecordId, author: &Author) -> Result<bool> {
    set_archived(db, anchor, true, author).await
}

/// Take an entity back out of the archive.
///
/// # Errors
///
/// As [`archive`].
pub async fn unarchive(db: &Db, anchor: &RecordId, author: &Author) -> Result<bool> {
    set_archived(db, anchor, false, author).await
}

/// Is this entity archived? Absent means no — a thing nobody has put
/// away is not away.
///
/// # Errors
///
/// [`KernelError::Db`] for engine errors.
pub async fn is_archived(db: &Db, anchor: &RecordId) -> Result<bool> {
    Ok(archived_in(&attribute::of(db, anchor, false).await?))
}

async fn set_archived(
    db: &Db,
    anchor: &RecordId,
    archived: bool,
    author: &Author,
) -> Result<bool> {
    let existing = attribute::of(db, anchor, true)
        .await?
        .into_iter()
        .find(|a| a.name == ARCHIVED);
    let write = Write {
        name: ARCHIVED,
        datatype: "boolean",
        content: Some(Value::Bool(archived)),
        labels: &[],
        options: None,
    };
    match existing {
        Some(a) => {
            if matches!(a.content, Some(Value::Bool(b)) if b == archived) && a.active {
                return Ok(false);
            }
            attribute::amend(db, &a.uid, write, author).await?;
            if !a.active {
                attribute::restore(db, &a.uid, author).await?;
            }
        }
        None => {
            if !archived {
                // Never archived, and asked to un-archive: nothing was
                // decided, so nothing is written.
                return Ok(false);
            }
            attribute::add(db, anchor, write, author).await?;
        }
    }
    Ok(true)
}

/// Does this entity exist? Asked before pointing something at it.
///
/// # Errors
///
/// [`KernelError::Db`] for engine errors.
pub async fn exists(db: &Db, anchor: &RecordId) -> Result<bool> {
    let mut resp = db
        .query("SELECT id FROM $id")
        .bind(("id", anchor.clone()))
        .await?;
    let rows: Vec<Value> = resp.take(0)?;
    Ok(!rows.is_empty())
}

/// Every entity, newest first — uuid7 ids are time-ordered, so the id
/// order IS creation order and nothing has to sort by a column.
///
/// Archived entities are left out unless asked for. They are not gone:
/// they are put away, and every one of them can be named, read and
/// restored.
///
/// # Errors
///
/// [`KernelError::Db`] for engine errors.
pub async fn list(db: &Db, include_archived: bool) -> Result<Vec<RecordId>> {
    let mut resp = db.query("SELECT id FROM entity ORDER BY id DESC").await?;
    let rows: Vec<Value> = resp.take(0)?;
    let all: Vec<RecordId> = rows.iter().filter_map(|r| obj_record(r, "id")).collect();
    if include_archived {
        return Ok(all);
    }
    // ONE read for the whole set, not one per entity. Asking
    // `is_archived` in a loop turned a listing into a query per row.
    let held = attribute::of_many(db, &all, false).await?;
    Ok(all
        .into_iter()
        .filter(|id| !archived_in(held.get(&record_uuid(id)).map_or(&[][..], Vec::as_slice)))
        .collect())
}

/// Find one entity by any unique fragment of its uuid.
///
/// # Errors
///
/// [`KernelError::Module`] when nothing matches, or when the fragment is
/// ambiguous — guessing between two entities is worse than refusing.
pub async fn resolve(db: &Db, fragment: &str) -> Result<RecordId> {
    let fragment = fragment.trim();
    if fragment.is_empty() {
        return Err(KernelError::Module("no entity given".to_string()));
    }
    // A WHOLE UUID IS NOT A SEARCH. The UI only ever sends complete
    // ones, and every write resolves the entity plus one per label — so
    // scanning the table for something we can address directly turned a
    // single save into several full reads.
    if let Ok(uuid) = fragment.parse::<uuid::Uuid>() {
        let anchor = RecordId::new("entity", superx_kernel::types::Uuid::from(uuid));
        if exists(db, &anchor).await? {
            return Ok(anchor);
        }
        return Err(KernelError::Module(format!("no entity matches '{fragment}'")));
    }
    let hits: Vec<RecordId> = list(db, true)
        .await?
        .into_iter()
        .filter(|id| record_uuid(id).contains(fragment))
        .collect();
    match hits.len() {
        0 => Err(KernelError::Module(format!("no entity matches '{fragment}'"))),
        1 => Ok(hits.into_iter().next().expect("one hit")),
        n => Err(KernelError::Module(format!(
            "'{fragment}' matches {n} entities — say more of the uuid"
        ))),
    }
}
