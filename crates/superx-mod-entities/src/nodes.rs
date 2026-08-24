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
    /// Hidden from the lists, still on the record. Absent on every row
    /// written before #304, which reads as not archived — the honest
    /// answer, since append-only forbids inventing one.
    pub archived: bool,
    pub valid_from: String,
}

/// An entity as listed: anchor identity + current label.
pub struct EntityRow {
    pub id: RecordId,
    pub entity_type: String,
    pub name: String,
    pub archived: bool,
}

/// Current metadata of an anchor, batch-resolved (issue #179): one
/// correlated subquery per anchor picks ONLY the newest state row —
/// version chains are never transferred whole.
pub struct NodeMeta {
    pub entity_type: String,
    pub name: String,
    pub content: Option<String>,
    pub attributes: Option<Value>,
    pub archived: bool,
    /// `valid_from` of the current state row — the version stamp.
    pub version: String,
}

/// Batch-resolve current metadata for a set of anchors, keyed by
/// uuid. Cost is bounded by the id set and the chain index, never by
/// table size or history depth.
///
/// # Errors
///
/// [`KernelError::Db`] for engine errors.
pub async fn current_meta(
    db: &Db,
    ids: &[RecordId],
) -> Result<std::collections::HashMap<String, NodeMeta>> {
    if ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    // One request, 2N statements: an anchor read + a plain-equality
    // state-chain read per id, reduced to the newest version in Rust
    // with parsed datetimes. Probed alternatives all intermittently
    // returned [] for existing rows on the mem engine when combined
    // with in-batch reduction ($parent.id correlation, and
    // ORDER BY … LIMIT 1 both with and without the index); the plain
    // equality read is the deterministic form.
    let mut statements = String::new();
    for i in 0..ids.len() {
        statements.push_str(&format!(
            "SELECT id, entity_type FROM $id{i};\
             SELECT name, content, attributes, archived, valid_from \
             FROM entity_state WHERE entity = $id{i};"
        ));
    }
    let mut query = db.query(statements);
    for (i, id) in ids.iter().enumerate() {
        query = query.bind((format!("id{i}"), id.clone()));
    }
    let mut resp = query.await?;
    let mut out = std::collections::HashMap::new();
    for i in 0..ids.len() {
        let anchors: Vec<Value> = resp.take(i * 2)?;
        let states: Vec<Value> = resp.take(i * 2 + 1)?;
        let Some(anchor) = anchors.first() else { continue };
        let Some(id) = obj_record(anchor, "id") else { continue };
        let Some(entity_type) = obj_str(anchor, "entity_type") else { continue };
        let (name, content, attributes, archived, version) = match newest_by_valid_from(&states) {
            Some(s) => (
                obj_str(s, "name").unwrap_or_default(),
                obj_str(s, "content"),
                obj_get(s, "attributes"),
                obj_bool(s, "archived"),
                obj_display(s, "valid_from").unwrap_or_default(),
            ),
            None => (String::new(), None, None, false, String::new()),
        };
        out.insert(
            record_uuid(&id),
            NodeMeta { entity_type, name, content, attributes, archived, version },
        );
    }
    Ok(out)
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
    append_state(db, &anchor, name, content, attributes, false).await?;
    Ok(anchor)
}

/// The write was based on a version that is no longer the head.
///
/// §6: "Every write carries the `valid_from` it was based on. If the
/// chain head has moved, the write is refused and the current version
/// comes back with the refusal." The current version travels WITH the
/// refusal, because a role that is only told "no" has nothing to do,
/// while a role handed the version that beat it can re-read, merge and
/// retry.
#[derive(Debug, Clone)]
pub struct Stale {
    /// What the writer thought it was amending.
    pub based_on: String,
    /// What is actually there now.
    pub current: String,
}

impl std::fmt::Display for Stale {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "this was written against version {} but the current version is {} — \
             somebody else wrote in between. Re-read it, merge what you meant, \
             and write again; overwriting theirs would lose an edit nobody is \
             told about",
            self.based_on, self.current
        )
    }
}

/// Refuse the write if the chain head has moved since `based_on`.
///
/// Latest-wins is the default everywhere in this substrate, and for one
/// human it is nearly always right. With perpetual roles writing
/// continuously it is routine for two writers to hold the same version,
/// and the loser's edit vanishes with nobody told — which in a system
/// whose premise is "the graph is the truth" means an agent builds the
/// wrong thing for a reason nobody can find.
///
/// # Errors
///
/// [`KernelError::Module`] carrying the current version when the head
/// has moved; [`KernelError::Db`] for engine errors.
pub async fn check_fresh(db: &Db, anchor: &RecordId, based_on: Option<&str>) -> Result<()> {
    // A writer that says nothing about what it read is not making the
    // claim, so nothing is checked. The guarantee is offered, never
    // imposed — imposing it would break every caller that has no
    // version to quote, which is most of them today.
    let Some(based_on) = based_on else { return Ok(()) };
    let current = current_state(db, anchor).await?.map(|s| s.valid_from).unwrap_or_default();
    if current == based_on {
        return Ok(());
    }
    Err(KernelError::Module(
        Stale { based_on: based_on.to_string(), current }.to_string(),
    ))
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
        // An edit is not an un-archiving. Renaming an archived thing
        // must not quietly bring it back into every list — two
        // decisions, and only one of them was made here. Same rule the
        // dictionary already applies to a retired slot.
        current.archived,
    )
    .await
}

/// Archive or restore an entity: hidden from the lists, still on the
/// record.
///
/// Append-only, so this is a NEW version carrying the flag rather than
/// a field somebody flipped — the decision is dated and sits in the
/// history beside every other. Restoring is another version, not an
/// erasure of this one.
///
/// Idempotent: archiving what is already archived appends nothing, so a
/// repeated call does not pad the history with rows that say nothing.
///
/// # Errors
///
/// [`KernelError::Module`] if the anchor has no state chain;
/// [`KernelError::Db`] for engine errors.
pub async fn set_archived(db: &Db, anchor: &RecordId, archived: bool) -> Result<bool> {
    let current = current_state(db, anchor).await?.ok_or_else(|| {
        KernelError::Module(format!(
            "entity {} has no state chain — corrupt substrate?",
            record_uuid(anchor)
        ))
    })?;
    if current.archived == archived {
        return Ok(false);
    }
    append_state(db, anchor, &current.name, current.content, current.attributes, archived).await?;
    Ok(true)
}

async fn append_state(
    db: &Db,
    anchor: &RecordId,
    name: &str,
    content: Option<String>,
    attributes: Option<Value>,
    archived: bool,
) -> Result<()> {
    let mut statement = String::from(
        "CREATE $id SET entity = $entity, name = $name, valid_from = time::now()",
    );
    // Written ONLY when true. `archived` is an `option<>` column added
    // in #304, and an instance that has not re-provisioned does not
    // have it yet — writing it unconditionally would refuse every
    // create and update until the operator ran provisioning, which is
    // the opposite of "everything is additive, so everything is
    // revertible" (§15). Absent reads as not archived.
    if archived {
        statement.push_str(", archived = $archived");
    }
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
    if archived {
        query = query.bind(("archived", true));
    }
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
        "SELECT name, content, attributes, archived, valid_from FROM entity_state \
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
    let mut anchors: Vec<RecordId> = rows.iter().filter_map(|r| obj_record(r, "id")).collect();
    // NEWEST FIRST (issue #257): uuid7 is time-ordered, so reversing
    // creation order puts what you just made at the top — an
    // ascending list buries every new entity below the fold.
    anchors.sort_by_key(record_uuid);
    anchors.reverse();

    // Current labels only — never the whole version chains (#179).
    let meta = current_meta(db, &anchors).await?;
    Ok(anchors
        .into_iter()
        .map(|id| {
            let m = meta.get(&record_uuid(&id));
            EntityRow {
                entity_type: m.map(|m| m.entity_type.clone()).unwrap_or_default(),
                name: m.map(|m| m.name.clone()).unwrap_or_default(),
                archived: m.is_some_and(|m| m.archived),
                id,
            }
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
        archived: obj_bool(row, "archived"),
        valid_from: obj_display(row, "valid_from").unwrap_or_default(),
    })
}

/// A `bool` column that may be absent. Absent is `false`, and that is
/// the whole upgrade story for `archived`: rows written before the
/// column existed say nothing, and saying nothing means not archived.
fn obj_bool(row: &Value, key: &str) -> bool {
    matches!(obj_get(row, key), Some(Value::Bool(true)))
}

/// The newest row of a small state set, compared as PARSED datetimes
/// (the #179 rule — lexical RFC3339 comparison is a trap).
fn newest_by_valid_from(rows: &[Value]) -> Option<&Value> {
    rows.iter().max_by_key(|r| {
        obj_display(r, "valid_from")
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
            .map(|t| t.with_timezone(&chrono::Utc))
            .unwrap_or(chrono::DateTime::<chrono::Utc>::MIN_UTC)
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
