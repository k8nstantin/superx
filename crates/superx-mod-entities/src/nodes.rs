//! Node verbs — immutable anchors + SCD-2 state chains (epic #166,
//! phase E2). Every write is an INSERT; "current" is computed as the
//! latest state row per anchor.

use superx_kernel::types::{RecordId, Value};
use superx_kernel::{Db, KernelError, Result};
use superx_ops::record_uuid;

use crate::registry::new_id;

/// One version row of an entity's state chain.
pub struct StateRow {
    pub name: String,
    pub content: Option<String>,
    pub attributes: Option<Value>,
    pub valid_from: String,
}

/// An entity as listed: anchor identity + current label.
pub struct EntityRow {
    pub id: RecordId,
    pub entity_type: String,
    pub name: String,
}

/// Validate a type name against the registry for a category.
///
/// # Errors
///
/// [`KernelError::Module`] for an unknown name or a category
/// mismatch; [`KernelError::Db`] for engine errors.
pub async fn require_type(db: &Db, name: &str, category: &str) -> Result<()> {
    let mut resp = db
        .query("SELECT category FROM entity_type WHERE name = $name LIMIT 1")
        .bind(("name", name.to_string()))
        .await?;
    let rows: Vec<Value> = resp.take(0)?;
    match rows.first().and_then(|r| obj_str(r, "category")) {
        Some(c) if c == category => Ok(()),
        Some(c) => Err(KernelError::Module(format!(
            "type '{name}' is a {c} type, not {category} — see `superx entities types`"
        ))),
        None => Err(KernelError::Module(format!(
            "unknown type '{name}' — see `superx entities types` (add with `types add`)"
        ))),
    }
}

/// Create an entity: one immutable anchor + its first state row.
/// Returns the anchor id (the entity's uuid7 identity).
///
/// # Errors
///
/// Type validation per [`require_type`]; [`KernelError::Db`] for
/// engine errors.
pub async fn create_entity(
    db: &Db,
    entity_type: &str,
    name: &str,
    content: Option<String>,
    attributes: Option<Value>,
) -> Result<RecordId> {
    require_type(db, entity_type, "entity").await?;
    let anchor = new_id("entity");
    db.query("CREATE $id SET entity_type = $entity_type, created_at = time::now()")
        .bind(("id", anchor.clone()))
        .bind(("entity_type", entity_type.to_string()))
        .await?
        .check()?;
    append_state(db, &anchor, name, content, attributes).await?;
    Ok(anchor)
}

/// Append a new state version. Unset fields carry forward from the
/// current version (an update never loses what it didn't mention).
///
/// # Errors
///
/// [`KernelError::Module`] when the anchor has no state chain;
/// [`KernelError::Db`] for engine errors.
pub async fn update_entity(
    db: &Db,
    anchor: &RecordId,
    name: Option<String>,
    content: Option<String>,
    attributes: Option<Value>,
) -> Result<()> {
    let current = current_state(db, anchor).await?.ok_or_else(|| {
        KernelError::Module(format!(
            "entity {} has no state chain — corrupt substrate?",
            record_uuid(anchor)
        ))
    })?;
    append_state(
        db,
        anchor,
        &name.unwrap_or(current.name),
        content.or(current.content),
        attributes.or(current.attributes),
    )
    .await
}

async fn append_state(
    db: &Db,
    anchor: &RecordId,
    name: &str,
    content: Option<String>,
    attributes: Option<Value>,
) -> Result<()> {
    let mut statement = String::from(
        "CREATE $id SET entity = $entity, name = $name, valid_from = time::now()",
    );
    if content.is_some() {
        statement.push_str(", content = $content");
    }
    if attributes.is_some() {
        statement.push_str(", attributes = $attributes");
    }
    let mut query = db
        .query(statement)
        .bind(("id", new_id("entity_state")))
        .bind(("entity", anchor.clone()))
        .bind(("name", name.to_string()));
    if let Some(c) = content {
        query = query.bind(("content", c));
    }
    if let Some(a) = attributes {
        query = query.bind(("attributes", a));
    }
    query.await?.check()?;
    Ok(())
}

/// The current state of an anchor (latest version row), if any.
///
/// # Errors
///
/// [`KernelError::Db`] for engine errors.
pub async fn current_state(db: &Db, anchor: &RecordId) -> Result<Option<StateRow>> {
    Ok(state_rows(db, anchor, "DESC", Some(1)).await?.into_iter().next())
}

/// The full version history of an anchor, oldest first.
///
/// # Errors
///
/// [`KernelError::Db`] for engine errors.
pub async fn state_history(db: &Db, anchor: &RecordId) -> Result<Vec<StateRow>> {
    state_rows(db, anchor, "ASC", None).await
}

async fn state_rows(
    db: &Db,
    anchor: &RecordId,
    order: &str,
    limit: Option<usize>,
) -> Result<Vec<StateRow>> {
    // `valid_from` is projected because every ORDER BY idiom must
    // appear in the selection. Order/limit are code-controlled.
    let statement = format!(
        "SELECT name, content, attributes, valid_from FROM entity_state \
         WHERE entity = $entity ORDER BY valid_from {order}{}",
        limit.map(|n| format!(" LIMIT {n}")).unwrap_or_default()
    );
    let mut resp = db.query(statement).bind(("entity", anchor.clone())).await?;
    let rows: Vec<Value> = resp.take(0)?;
    Ok(rows.iter().filter_map(parse_state).collect())
}

/// Anchors + current labels, optionally filtered by type, in
/// creation order (uuid7 ids are time-ordered).
///
/// # Errors
///
/// [`KernelError::Db`] for engine errors.
pub async fn list_entities(db: &Db, type_filter: Option<&str>) -> Result<Vec<EntityRow>> {
    let mut resp = if let Some(t) = type_filter {
        db.query("SELECT id, entity_type FROM entity WHERE entity_type = $t")
            .bind(("t", t.to_string()))
            .await?
    } else {
        db.query("SELECT id, entity_type FROM entity").await?
    };
    let rows: Vec<Value> = resp.take(0)?;
    let mut anchors: Vec<(RecordId, String)> = rows
        .iter()
        .filter_map(|r| Some((obj_record(r, "id")?, obj_str(r, "entity_type")?)))
        .collect();
    anchors.sort_by_key(|(id, _)| record_uuid(id));

    // One batched read resolves every current label: newest state
    // first, first-seen per anchor wins.
    let ids: Vec<RecordId> = anchors.iter().map(|(id, _)| id.clone()).collect();
    let mut resp = db
        .query(
            "SELECT entity, name, valid_from FROM entity_state \
             WHERE entity INSIDE $ids ORDER BY valid_from DESC",
        )
        .bind(("ids", ids))
        .await?;
    let state_rows: Vec<Value> = resp.take(0)?;
    let mut names: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for row in &state_rows {
        if let (Some(entity), Some(name)) = (obj_record(row, "entity"), obj_str(row, "name")) {
            names.entry(record_uuid(&entity)).or_insert(name);
        }
    }

    Ok(anchors
        .into_iter()
        .map(|(id, entity_type)| {
            let name = names.get(&record_uuid(&id)).cloned().unwrap_or_default();
            EntityRow { id, entity_type, name }
        })
        .collect())
}

/// Resolve a uuid7 fragment to exactly one anchor (the session/module
/// resolution convention).
///
/// # Errors
///
/// [`KernelError::Module`] for zero or multiple matches;
/// [`KernelError::Db`] for engine errors.
pub async fn resolve_entity(db: &Db, fragment: &str) -> Result<RecordId> {
    let mut resp = db.query("SELECT id FROM entity").await?;
    let rows: Vec<Value> = resp.take(0)?;
    let matches: Vec<RecordId> = rows
        .iter()
        .filter_map(|r| obj_record(r, "id"))
        .filter(|id| record_uuid(id).contains(fragment))
        .collect();
    match matches.len() {
        1 => Ok(matches.into_iter().next().expect("len checked")),
        0 => Err(KernelError::Module(format!(
            "no entity matches '{fragment}' — see `superx entities list`"
        ))),
        n => Err(KernelError::Module(format!(
            "'{fragment}' is ambiguous ({n} entities match) — give more of the uuid"
        ))),
    }
}

/// The anchor's own fields (type + creation instant).
///
/// # Errors
///
/// [`KernelError::Module`] when the anchor doesn't exist;
/// [`KernelError::Db`] for engine errors.
pub async fn anchor_info(db: &Db, anchor: &RecordId) -> Result<(String, String)> {
    let mut resp = db
        .query("SELECT entity_type, created_at FROM $anchor")
        .bind(("anchor", anchor.clone()))
        .await?;
    let rows: Vec<Value> = resp.take(0)?;
    rows.first()
        .and_then(|r| {
            Some((
                obj_str(r, "entity_type")?,
                obj_display(r, "created_at").unwrap_or_default(),
            ))
        })
        .ok_or_else(|| {
            KernelError::Module(format!("entity {} not found", record_uuid(anchor)))
        })
}

fn parse_state(row: &Value) -> Option<StateRow> {
    Some(StateRow {
        name: obj_str(row, "name")?,
        content: obj_str(row, "content"),
        attributes: obj_get(row, "attributes"),
        valid_from: obj_display(row, "valid_from").unwrap_or_default(),
    })
}

fn obj_get(row: &Value, key: &str) -> Option<Value> {
    match row {
        Value::Object(o) => o.get(key).filter(|v| !matches!(v, Value::None | Value::Null)).cloned(),
        _ => None,
    }
}

fn obj_str(row: &Value, key: &str) -> Option<String> {
    match obj_get(row, key) {
        Some(Value::String(s)) => Some(s),
        _ => None,
    }
}

fn obj_record(row: &Value, key: &str) -> Option<RecordId> {
    match obj_get(row, key) {
        Some(Value::RecordId(r)) => Some(r),
        _ => None,
    }
}

fn obj_display(row: &Value, key: &str) -> Option<String> {
    obj_get(row, key).map(|v| match v {
        Value::Datetime(d) => d.to_string(),
        other => format!("{other:?}"),
    })
}

/// Render a dynamic Value as compact JSON (the mirror of the
/// kernel's `value_from_json`, for CLI display).
#[must_use]
pub fn value_to_json(v: &Value) -> serde_json::Value {
    match v {
        Value::Null | Value::None => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Number(n) => n
            .to_int()
            .map(|i| serde_json::json!(i))
            .or_else(|| n.into_float().ok().map(|f| serde_json::json!(f)))
            .unwrap_or(serde_json::Value::Null),
        Value::String(s) => serde_json::Value::String(s.clone()),
        Value::Array(a) => serde_json::Value::Array(a.iter().map(value_to_json).collect()),
        Value::Object(o) => serde_json::Value::Object(
            o.iter().map(|(k, val)| (k.clone(), value_to_json(val))).collect(),
        ),
        other => serde_json::Value::String(format!("{other:?}")),
    }
}
