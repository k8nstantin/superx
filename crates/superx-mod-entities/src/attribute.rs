//! Attributes — everything said about an entity.
//!
//! An attribute is built like the entity it hangs off: a uuid7, a name,
//! and labels. Plus what type it is and what it holds. Any number of
//! them, in the order they were added.
//!
//! THE DATATYPE GATE LIVES HERE, and it is the most important code in
//! this file. `content` is `any` in the schema, so the engine cannot
//! check that a `number` holds a number — the failure every
//! entity-attribute-value model is known for, where validation leaves
//! the database and data quality rots because nothing at the storage
//! layer says no. Every write goes through [`check`]. There is no path
//! around it.
//!
//! `uid` is the attribute's identity across its versions; `id` is the
//! version. Amending appends a row with the same uid, and the current
//! value is the newest row of that chain. Retiring appends
//! `active = false`; restoring appends `true` again. Nothing is
//! overwritten and nothing is removed.

use superx_kernel::types::{Object, RecordId, Value};
use superx_kernel::{Db, KernelError, Result};
use superx_ops::record_uuid;

use crate::author::Author;
use crate::{new_id, newest_by_valid_from, obj_bool, obj_display, obj_get, obj_record, obj_records, obj_str};

/// What a value IS. Five, closed, and the same list the schema asserts —
/// a sixth is a validation rule plus a way to render it, both of which
/// are code, so it is a deliberate change in two places rather than a
/// string somebody invents at a call site.
pub const DATATYPES: [&str; 5] = // skill-allow: §9-const — the module's own data model, not a tunable
    ["text", "number", "boolean", "datetime", "json"];

/// One attribute as it currently reads.
#[derive(Debug, Clone)]
pub struct Attribute {
    /// Identity across versions. This is what a saved screen layout
    /// points at, so a rename never moves a field.
    pub uid: String,
    pub entity: RecordId,
    pub name: String,
    /// What this attribute IS, and how to treat what it holds. Resolved
    /// by whoever reads it; nothing here interprets them.
    pub labels: Vec<RecordId>,
    pub datatype: String,
    /// Absent is a legitimate statement: an attribute carrying only
    /// labels says what the entity IS and holds nothing.
    pub content: Option<Value>,
    /// Per-field overrides — a width, a placeholder. Absent is the
    /// ordinary case.
    pub options: Option<Value>,
    pub active: bool,
    /// `valid_from` of this version.
    pub version: String,
}

/// What to write. A struct rather than eight positional arguments,
/// because a caller that silently swaps two strings is a bug nothing
/// catches.
pub struct Write<'a> {
    pub name: &'a str,
    pub datatype: &'a str,
    pub content: Option<Value>,
    pub labels: &'a [RecordId],
    pub options: Option<Value>,
}

/// Add an attribute to an entity. Returns its `uid` — the handle for
/// every later version of it.
///
/// # Errors
///
/// [`KernelError::Module`] for an empty name, an unknown datatype, or
/// content that is not what the datatype says it is;
/// [`KernelError::Db`] for engine errors, including a label that does
/// not exist.
pub async fn add(
    db: &Db,
    entity: &RecordId,
    w: Write<'_>,
    author: &Author,
) -> Result<String> {
    let uid = record_uuid(&new_id("attr"));
    append(db, entity, &uid, &w, true, author).await?;
    Ok(uid)
}

/// Amend an attribute: a new version on the same chain.
///
/// The WHOLE attribute is stated again — name, datatype, content,
/// labels. A partial write would mean reading the old row to fill the
/// gaps, and two callers doing that at once is how a field silently
/// loses its labels.
///
/// # Errors
///
/// [`KernelError::Module`] when the uid has no chain, or the write does
/// not validate; [`KernelError::Db`] for engine errors.
pub async fn amend(db: &Db, uid: &str, w: Write<'_>, author: &Author) -> Result<()> {
    let now = require(db, uid).await?;
    append(db, &now.entity, uid, &w, now.active, author).await
}

/// Retire an attribute: the entity no longer holds it, and the record
/// still says it once did. Idempotent — returns whether a row was
/// written.
///
/// # Errors
///
/// [`KernelError::Module`] when the uid has no chain; [`KernelError::Db`]
/// for engine errors.
pub async fn retire(db: &Db, uid: &str, author: &Author) -> Result<bool> {
    set_active(db, uid, false, author).await
}

/// Undo a retirement. The same act in the other direction.
///
/// # Errors
///
/// As [`retire`].
pub async fn restore(db: &Db, uid: &str, author: &Author) -> Result<bool> {
    set_active(db, uid, true, author).await
}

async fn set_active(db: &Db, uid: &str, active: bool, author: &Author) -> Result<bool> {
    let now = require(db, uid).await?;
    if now.active == active {
        return Ok(false);
    }
    let w = Write {
        name: &now.name,
        datatype: &now.datatype,
        content: now.content.clone(),
        labels: &now.labels,
        options: now.options.clone(),
    };
    append(db, &now.entity, uid, &w, active, author).await?;
    Ok(true)
}

/// The current version of one attribute.
///
/// # Errors
///
/// [`KernelError::Db`] for engine errors.
pub async fn current(db: &Db, uid: &str) -> Result<Option<Attribute>> {
    let mut resp = db
        .query("SELECT * FROM entity_attribute WHERE uid = $uid")
        .bind(("uid", uid.to_string()))
        .await?;
    let rows: Vec<Value> = resp.take(0)?;
    Ok(newest_by_valid_from(&rows).and_then(parse))
}

/// Everything an entity holds, IN THE ORDER IT WAS ADDED.
///
/// uuid7 is time-ordered, so creation order is uid order and no column
/// has to store it. That is what makes "add a field and it appears below
/// the last one" free.
///
/// # Errors
///
/// [`KernelError::Db`] for engine errors.
pub async fn of(db: &Db, entity: &RecordId, include_retired: bool) -> Result<Vec<Attribute>> {
    let mut resp = db
        .query("SELECT * FROM entity_attribute WHERE entity = $e")
        .bind(("e", entity.clone()))
        .await?;
    let rows: Vec<Value> = resp.take(0)?;

    let mut chains: std::collections::BTreeMap<String, Vec<Value>> =
        std::collections::BTreeMap::new();
    for row in rows {
        let Some(uid) = obj_str(&row, "uid") else { continue };
        chains.entry(uid).or_default().push(row);
    }
    // BTreeMap keys sort lexically, and a uuid7's hex sorts by time —
    // so this is creation order, for free.
    Ok(chains
        .into_values()
        .filter_map(|chain| newest_by_valid_from(&chain).and_then(parse))
        .filter(|a| include_retired || a.active)
        .collect())
}

/// Everything held by a SET of entities, keyed by entity uuid, in ONE
/// request.
///
/// A listing needs this: asking per entity turned one screen into a
/// query per row, and callers wanting a name, a label set and an
/// archived flag each asked separately for the same rows.
///
/// # Errors
///
/// [`KernelError::Db`] for engine errors.
pub async fn of_many(
    db: &Db,
    entities: &[RecordId],
    include_retired: bool,
) -> Result<std::collections::HashMap<String, Vec<Attribute>>> {
    if entities.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let mut resp = db
        .query("SELECT * FROM entity_attribute WHERE entity IN $ids")
        .bind(("ids", entities.to_vec()))
        .await?;
    let rows: Vec<Value> = resp.take(0)?;

    let mut chains: std::collections::BTreeMap<(String, String), Vec<Value>> =
        std::collections::BTreeMap::new();
    for row in rows {
        let (Some(e), Some(uid)) = (obj_record(&row, "entity"), obj_str(&row, "uid")) else {
            continue;
        };
        chains.entry((record_uuid(&e), uid)).or_default().push(row);
    }
    let mut out: std::collections::HashMap<String, Vec<Attribute>> =
        std::collections::HashMap::new();
    for ((entity_uuid, _), chain) in chains {
        let Some(head) = newest_by_valid_from(&chain).and_then(parse) else { continue };
        if head.active || include_retired {
            out.entry(entity_uuid).or_default().push(head);
        }
    }
    Ok(out)
}

/// Every version of one attribute, oldest first.
///
/// # Errors
///
/// [`KernelError::Db`] for engine errors.
pub async fn history(db: &Db, uid: &str) -> Result<Vec<Attribute>> {
    let mut resp = db
        .query(
            "SELECT * FROM entity_attribute WHERE uid = $uid ORDER BY valid_from ASC, id ASC",
        )
        .bind(("uid", uid.to_string()))
        .await?;
    let rows: Vec<Value> = resp.take(0)?;
    Ok(rows.iter().filter_map(parse).collect())
}

/// IS THIS VALUE WHAT ITS DATATYPE SAYS IT IS?
///
/// The engine cannot answer this — `content` is `any` — so this is the
/// only thing standing where a column type normally stands. `json`
/// accepts an object or an array, because both are structures the
/// database can look inside; a bare string stored as `json` would be the
/// exact lie this prevents.
///
/// # Errors
///
/// [`KernelError::Module`] when the value does not match.
pub fn check(datatype: &str, content: &Value) -> Result<()> {
    let ok = match datatype {
        "text" => matches!(content, Value::String(_)),
        "number" => matches!(content, Value::Number(_)),
        "boolean" => matches!(content, Value::Bool(_)),
        "datetime" => matches!(content, Value::Datetime(_)),
        "json" => matches!(content, Value::Object(_) | Value::Array(_)),
        _ => false,
    };
    if ok {
        return Ok(());
    }
    Err(KernelError::Module(format!(
        "this attribute is declared '{datatype}' and its content is not one — \
         a value that is not what it says it is cannot be read back safely"
    )))
}

async fn require(db: &Db, uid: &str) -> Result<Attribute> {
    current(db, uid)
        .await?
        .ok_or_else(|| KernelError::Module(format!("no attribute '{uid}'")))
}

fn parse(row: &Value) -> Option<Attribute> {
    Some(Attribute {
        uid: obj_str(row, "uid")?,
        entity: obj_record(row, "entity")?,
        name: obj_str(row, "name")?,
        labels: obj_records(row, "labels"),
        datatype: obj_str(row, "datatype")?,
        content: obj_get(row, "content"),
        options: obj_get(row, "options"),
        active: obj_bool(row, "active"),
        version: obj_display(row, "valid_from").unwrap_or_default(),
    })
}

/// The one place an attribute row is written, and the one place the
/// datatype gate is applied.
async fn append(
    db: &Db,
    entity: &RecordId,
    uid: &str,
    w: &Write<'_>,
    active: bool,
    author: &Author,
) -> Result<()> {
    let name = w.name.trim();
    if name.is_empty() {
        return Err(KernelError::Module(
            "an attribute needs a name — an unnamed value cannot be read back".to_string(),
        ));
    }
    if !DATATYPES.contains(&w.datatype) {
        return Err(KernelError::Module(format!(
            "'{}' is not a datatype this module knows: {}",
            w.datatype,
            DATATYPES.join(", ")
        )));
    }
    if let Some(c) = &w.content {
        check(w.datatype, c)?;
    }

    let mut row = Object::new();
    row.insert("uid".to_string(), Value::String(uid.to_string()));
    row.insert("entity".to_string(), Value::RecordId(entity.clone()));
    row.insert("name".to_string(), Value::String(name.to_string()));
    row.insert("datatype".to_string(), Value::String(w.datatype.to_string()));
    row.insert("active".to_string(), Value::Bool(active));
    if let Some(c) = &w.content {
        row.insert("content".to_string(), c.clone());
    }
    if let Some(o) = &w.options {
        row.insert("options".to_string(), o.clone());
    }
    let mut labels: Vec<Value> = Vec::new();
    for l in w.labels {
        let v = Value::RecordId(l.clone());
        if !labels.contains(&v) {
            labels.push(v);
        }
    }
    if !labels.is_empty() {
        row.insert("labels".to_string(), Value::Array(labels.into()));
    }
    row.insert("valid_from".to_string(), Value::Datetime(chrono::Utc::now().into()));
    author.stamp(&mut row);

    db.query("CREATE $id CONTENT $row")
        .bind(("id", new_id("entity_attribute")))
        .bind(("row", Value::Object(row)))
        .await?
        .check()?;
    Ok(())
}
