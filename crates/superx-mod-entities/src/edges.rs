//! Edge verbs — native graph links with append-only history (epic
//! #166, phase E3; D19). Direction convention: owner → target
//! (product —contains→ component, entity —describes→ its text), so
//! forward traversal from a top-level entity reveals the whole
//! product. Link = RELATE with a fresh uuid7 edge_uid; unlink = a NEW
//! row on the same edge_uid with active = false; current edge state =
//! latest row per edge_uid.

use std::collections::HashMap;

use superx_kernel::types::{RecordId, Value};
use superx_kernel::{Db, KernelError, Result};
use superx_ops::record_uuid;

use crate::nodes::require_type;
use crate::registry::new_id;

/// The current state of one edge chain, as used by traversal.
#[derive(Clone)]
pub struct EdgeRow {
    pub edge_uid: String,
    pub from: RecordId,
    pub to: RecordId,
    pub rel_type: String,
    pub active: bool,
    pub valid_from: String,
}

/// Link two anchors. Refuses self-links and duplicate ACTIVE links on
/// the same (from, to, rel_type). Returns the new edge_uid.
///
/// # Errors
///
/// [`KernelError::Module`] for validation refusals;
/// [`KernelError::Db`] for engine errors (incl. ENFORCED refusals).
pub async fn link(db: &Db, from: &RecordId, to: &RecordId, rel_type: &str) -> Result<String> {
    require_type(db, rel_type, "relation").await?;
    if record_uuid(from) == record_uuid(to) {
        return Err(KernelError::Module("an entity cannot link to itself".to_string()));
    }
    let existing = current_edges_between(db, from, to).await?;
    if existing.iter().any(|e| e.rel_type == rel_type && e.active) {
        return Err(KernelError::Module(format!(
            "{} -[{rel_type}]-> {} is already linked (unlink first)",
            record_uuid(from),
            record_uuid(to)
        )));
    }
    let edge_uid = uuid::Uuid::now_v7().to_string();
    relate(db, from, to, &edge_uid, rel_type, true).await?;
    Ok(edge_uid)
}

/// Retract the active link on (from, to, rel_type) by appending an
/// inactive row on the same edge_uid — history stays whole.
///
/// # Errors
///
/// [`KernelError::Module`] when no active link exists;
/// [`KernelError::Db`] for engine errors.
pub async fn unlink(db: &Db, from: &RecordId, to: &RecordId, rel_type: &str) -> Result<String> {
    let existing = current_edges_between(db, from, to).await?;
    let Some(active) = existing.iter().find(|e| e.rel_type == rel_type && e.active) else {
        return Err(KernelError::Module(format!(
            "no active {rel_type} link from {} to {}",
            record_uuid(from),
            record_uuid(to)
        )));
    };
    relate(db, from, to, &active.edge_uid, rel_type, false).await?;
    Ok(active.edge_uid.clone())
}

async fn relate(
    db: &Db,
    from: &RecordId,
    to: &RecordId,
    edge_uid: &str,
    rel_type: &str,
    active: bool,
) -> Result<()> {
    // D19: RELATE is creation-only — a new edge record per event.
    db.query(
        "RELATE $from->edge->$to SET id = $id, edge_uid = $edge_uid, \
         rel_type = $rel_type, active = $active, valid_from = time::now()",
    )
    .bind(("from", from.clone()))
    .bind(("to", to.clone()))
    .bind(("id", new_id("edge")))
    .bind(("edge_uid", edge_uid.to_string()))
    .bind(("rel_type", rel_type.to_string()))
    .bind(("active", active))
    .await?
    .check()?;
    Ok(())
}

/// Current edge state per chain between two anchors (both
/// directions' rows are NOT merged — from → to only).
async fn current_edges_between(db: &Db, from: &RecordId, to: &RecordId) -> Result<Vec<EdgeRow>> {
    let mut resp = db
        .query(
            "SELECT id, in, out, edge_uid, rel_type, active, valid_from FROM edge \
             WHERE in = $from AND out = $to ORDER BY valid_from DESC",
        )
        .bind(("from", from.clone()))
        .bind(("to", to.clone()))
        .await?;
    let rows: Vec<Value> = resp.take(0)?;
    Ok(latest_per_chain(rows.iter().filter_map(parse_edge)))
}

/// Expand a frontier by record pointers: every edge touching the
/// given anchors in the requested direction, reduced to current
/// state per edge chain. Two id-addressed queries — the edge table
/// is never scanned (D19's scale contract).
///
/// # Errors
///
/// [`KernelError::Db`] for engine errors.
pub async fn expand(db: &Db, frontier: &[RecordId], reverse: bool) -> Result<Vec<EdgeRow>> {
    if frontier.is_empty() {
        return Ok(Vec::new());
    }
    let step = if reverse { "<-edge" } else { "->edge" };
    let mut resp = db
        .query(format!("SELECT id, {step} AS hits FROM $frontier"))
        .bind(("frontier", frontier.to_vec()))
        .await?;
    let rows: Vec<Value> = resp.take(0)?;
    let mut edge_ids: Vec<RecordId> = Vec::new();
    for row in &rows {
        if let Value::Object(o) = row {
            if let Some(Value::Array(hits)) = o.get("hits") {
                for hit in hits.iter() {
                    if let Value::RecordId(r) = hit {
                        edge_ids.push(r.clone());
                    }
                }
            }
        }
    }
    if edge_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut resp = db
        .query("SELECT id, in, out, edge_uid, rel_type, active, valid_from FROM $ids")
        .bind(("ids", edge_ids))
        .await?;
    let rows: Vec<Value> = resp.take(0)?;
    Ok(latest_per_chain(rows.iter().filter_map(parse_edge)))
}

/// Latest row per edge_uid wins (rows arrive in any order; compare
/// valid_from lexically — RFC3339 sorts chronologically).
fn latest_per_chain(rows: impl Iterator<Item = EdgeRow>) -> Vec<EdgeRow> {
    let mut latest: HashMap<String, EdgeRow> = HashMap::new();
    for row in rows {
        match latest.get(&row.edge_uid) {
            Some(seen) if seen.valid_from >= row.valid_from => {}
            _ => {
                latest.insert(row.edge_uid.clone(), row);
            }
        }
    }
    let mut out: Vec<EdgeRow> = latest.into_values().collect();
    out.sort_by(|a, b| a.valid_from.cmp(&b.valid_from));
    out
}

fn parse_edge(row: &Value) -> Option<EdgeRow> {
    let Value::Object(o) = row else { return None };
    let get_record = |key: &str| match o.get(key) {
        Some(Value::RecordId(r)) => Some(r.clone()),
        _ => None,
    };
    let get_str = |key: &str| match o.get(key) {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    };
    Some(EdgeRow {
        edge_uid: get_str("edge_uid")?,
        from: get_record("in")?,
        to: get_record("out")?,
        rel_type: get_str("rel_type")?,
        active: matches!(o.get("active"), Some(Value::Bool(true))),
        valid_from: match o.get("valid_from") {
            Some(Value::Datetime(d)) => d.to_string(),
            _ => String::new(),
        },
    })
}
