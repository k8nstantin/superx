//! The type registry — types-as-data (epic #166). Node kinds and edge
//! kinds are rows in the module's own db, extensible at runtime with
//! no DDL; the seeded contract types below are data, not an enum.

use superx_kernel::types::{RecordId, Value};
use superx_kernel::{Db, KernelError, Result};

/// The categories a type can carry: graph node kinds vs edge kinds.
pub const CATEGORIES: [&str; 2] = ["entity", "relation"];

/// The contract types seeded idempotently at every startup
/// (name, category, description). D22 text-as-entity lives here:
/// `text` nodes + the role edges `describes`/`comments`/`instructs`.
pub const SEEDED_TYPES: &[(&str, &str, &str)] = &[
    ("product", "entity", "a buildable thing; contains components"),
    ("task", "entity", "a unit of work an agent can execute"),
    ("rag", "entity", "a retrieval source agents consult"),
    ("model", "entity", "an integrated model"),
    ("document", "entity", "a file-backed content node"),
    ("text", "entity", "an inline content node: description, comment, instructions"),
    ("contains", "relation", "parent contains child (product -> component)"),
    ("linked", "relation", "generic association"),
    ("attached", "relation", "document attached to an entity"),
    ("consults", "relation", "task consults a rag or model"),
    ("describes", "relation", "text describes an entity"),
    ("comments", "relation", "text comments on an entity"),
    ("instructs", "relation", "text instructs a task"),
];

/// One row of the registry, as read back for rendering.
pub struct EntityTypeRow {
    pub name: String,
    pub category: String,
    pub description: Option<String>,
}

/// Mint a uuid7 record id in the module's own db — time-ordered ids
/// are the historical log (the kernel's convention, applied here; the
/// kernel's own minter is crate-private).
#[must_use]
pub fn new_id(table: &'static str) -> RecordId {
    RecordId::new(table, superx_kernel::types::Uuid::from(uuid::Uuid::now_v7()))
}

/// Seed the contract types; returns how many were newly created.
/// Idempotent: existing names are skipped; a UNIQUE-index race counts
/// as skipped.
///
/// # Errors
///
/// [`KernelError::Db`] for engine errors other than the benign
/// duplicate.
pub async fn seed_types(db: &Db) -> Result<usize> {
    let mut created = 0;
    for (name, category, description) in SEEDED_TYPES {
        if insert_type(db, name, category, Some(description)).await? {
            created += 1;
        }
    }
    Ok(created)
}

/// All registered types, ordered by category then name.
///
/// # Errors
///
/// [`KernelError::Db`] for engine errors.
pub async fn list_types(db: &Db) -> Result<Vec<EntityTypeRow>> {
    let mut resp = db
        .query("SELECT name, category, description FROM entity_type ORDER BY category, name")
        .await?;
    let rows: Vec<Value> = resp.take(0)?;
    Ok(rows
        .iter()
        .filter_map(|row| {
            let name = obj_str(row, "name")?;
            let category = obj_str(row, "category")?;
            Some(EntityTypeRow {
                name,
                category,
                description: obj_str(row, "description"),
            })
        })
        .collect())
}

/// Register a new type at runtime (`superx entities types add`).
///
/// # Errors
///
/// [`KernelError::Module`] for an invalid name/category or a
/// duplicate; [`KernelError::Db`] for engine errors.
pub async fn add_type(
    db: &Db,
    name: &str,
    category: &str,
    description: Option<&str>,
) -> Result<()> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(KernelError::Module(format!(
            "type name '{name}' must be lowercase [a-z0-9_]"
        )));
    }
    if !CATEGORIES.contains(&category) {
        return Err(KernelError::Module(format!(
            "category '{category}' must be one of: {}",
            CATEGORIES.join(" | ")
        )));
    }
    if insert_type(db, name, category, description).await? {
        Ok(())
    } else {
        Err(KernelError::Module(format!("type '{name}' already exists")))
    }
}

/// Check-then-create against the UNIQUE name index. Returns whether a
/// row was created; a lost race reads as "already there".
async fn insert_type(
    db: &Db,
    name: &str,
    category: &str,
    description: Option<&str>,
) -> Result<bool> {
    let mut resp = db
        .query("SELECT name FROM entity_type WHERE name = $name LIMIT 1")
        .bind(("name", name.to_string()))
        .await?;
    let existing: Vec<Value> = resp.take(0)?;
    if !existing.is_empty() {
        return Ok(false);
    }

    let id = new_id("entity_type");
    let outcome = if let Some(d) = description {
        db.query(
            "CREATE $id SET name = $name, category = $category, \
             description = $description, valid_from = time::now()",
        )
        .bind(("id", id))
        .bind(("name", name.to_string()))
        .bind(("category", category.to_string()))
        .bind(("description", d.to_string()))
        .await?
        .check()
    } else {
        db.query(
            "CREATE $id SET name = $name, category = $category, \
             valid_from = time::now()",
        )
        .bind(("id", id))
        .bind(("name", name.to_string()))
        .bind(("category", category.to_string()))
        .await?
        .check()
    };
    match outcome {
        Ok(_) => Ok(true),
        Err(e) if e.to_string().contains("already") => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// Pull a string field out of a dynamic row object.
fn obj_str(row: &Value, key: &str) -> Option<String> {
    match row {
        Value::Object(o) => match o.get(key) {
            Some(Value::String(s)) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}
