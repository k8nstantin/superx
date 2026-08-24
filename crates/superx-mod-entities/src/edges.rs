//! Edge verbs — native graph links with append-only history (epic
//! #166, phase E3; D19). Direction convention: owner → target
//! (product —contains→ component, entity —describes→ its text), so
//! forward traversal from a top-level entity reveals the whole
//! product. Link = RELATE with a fresh uuid7 edge_uid; unlink = a NEW
//! row on the same edge_uid with active = false; current edge state =
//! latest row per edge_uid.

use std::collections::{HashMap, HashSet};

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
    // What the LABEL will accept (#298, spec §5.5). A mislabelled field
    // is a confusing entity; a mislabelled edge is a wrong graph, and the
    // graph is what agents execute.
    enforce_link_label(db, from, to, rel_type).await?;

    let edge_uid = uuid::Uuid::now_v7().to_string();
    relate(db, from, to, &edge_uid, rel_type, true).await?;
    Ok(edge_uid)
}

/// Refuse a link the dictionary says cannot exist.
///
/// A label with NO declared endpoints stays permissive: one somebody has
/// not finished describing must not block work, the same rule as a type
/// that declares no slots accepting anything (§7). Enforcement arrives
/// with the declaration, never before it.
async fn enforce_link_label(
    db: &Db,
    from: &RecordId,
    to: &RecordId,
    rel_type: &str,
) -> Result<()> {
    let Some(label) = crate::dictionary::current(db, rel_type, crate::dictionary::LINK).await?
    else {
        // Not in the dictionary at all: the registry already vouched for
        // the relation type, and refusing here would break every edge
        // written before link labels were seeded.
        return Ok(());
    };

    if !label.source_types.is_empty() {
        let (kind, _) = crate::nodes::anchor_info(db, from).await?;
        if !label.source_types.contains(&kind) {
            return Err(KernelError::Module(format!(
                "'{rel_type}' does not start at a {kind} — it starts at: {}",
                label.source_types.join(", ")
            )));
        }
    }
    if !label.target_types.is_empty() {
        let (kind, _) = crate::nodes::anchor_info(db, to).await?;
        if !label.target_types.contains(&kind) {
            return Err(KernelError::Module(format!(
                "'{rel_type}' does not point at a {kind} — it points at: {}",
                label.target_types.join(", ")
            )));
        }
    }

    if label.acyclic && reaches(db, to, from, rel_type).await? {
        return Err(KernelError::Module(format!(
            "'{rel_type}' is acyclic and this would close a loop — {} already reaches {} \
             that way. A cycle here is not a link that reads oddly: the runner's wave \
             pass drops every task in it, so the work never runs and nothing says why",
            record_uuid(to),
            record_uuid(from)
        )));
    }
    Ok(())
}

/// Can `start` reach `goal` by following active edges of one label?
///
/// Bounded by the nodes it visits, not by the table: an acyclic check
/// that walked everything would cost the graph on every link.
async fn reaches(db: &Db, start: &RecordId, goal: &RecordId, rel_type: &str) -> Result<bool> {
    let goal_uuid = record_uuid(goal);
    let mut seen: HashSet<String> = HashSet::new();
    let mut frontier = vec![start.clone()];
    seen.insert(record_uuid(start));

    while !frontier.is_empty() {
        if seen.contains(&goal_uuid) {
            return Ok(true);
        }
        let mut next = Vec::new();
        for edge in expand(db, &frontier, false).await? {
            if !edge.active || edge.rel_type != rel_type {
                continue;
            }
            let uuid = record_uuid(&edge.to);
            if uuid == goal_uuid {
                return Ok(true);
            }
            if seen.insert(uuid) {
                next.push(edge.to);
            }
        }
        frontier = next;
    }
    Ok(false)
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
    expand_at(db, frontier, reverse, None).await
}

/// The same expansion, as it stood at an instant (§14): edges as they
/// were active then, not as they are now.
///
/// # Errors
///
/// [`KernelError::Db`](superx_kernel::KernelError::Db) for engine
/// errors.
pub async fn expand_at(
    db: &Db,
    frontier: &[RecordId],
    reverse: bool,
    as_of: crate::asof::AsOf,
) -> Result<Vec<EdgeRow>> {
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
    Ok(latest_per_chain(
        rows.iter()
            .filter_map(parse_edge)
            .filter(|e| crate::asof::visible(&e.valid_from, as_of)),
    ))
}

/// Latest row per edge_uid wins. Timestamps are compared as PARSED
/// datetimes (issue #179): lexical RFC3339 comparison inverts when a
/// fractionless '…06Z' meets '…06.5Z' ('Z' > '.'), and this reduction
/// decides which link states are real.
fn latest_per_chain(rows: impl Iterator<Item = EdgeRow>) -> Vec<EdgeRow> {
    let mut latest: HashMap<String, EdgeRow> = HashMap::new();
    for row in rows {
        match latest.get(&row.edge_uid) {
            Some(seen) if !newer(&row.valid_from, &seen.valid_from) => {}
            _ => {
                latest.insert(row.edge_uid.clone(), row);
            }
        }
    }
    let mut out: Vec<EdgeRow> = latest.into_values().collect();
    out.sort_by(|a, b| match (parse_ts(&a.valid_from), parse_ts(&b.valid_from)) {
        (Some(x), Some(y)) => x.cmp(&y),
        _ => a.valid_from.cmp(&b.valid_from),
    });
    out
}

fn parse_ts(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&chrono::Utc))
}

/// Is `a` strictly newer than `b`? Parsed comparison with a lexical
/// fallback for unparseable strings.
fn newer(a: &str, b: &str) -> bool {
    match (parse_ts(a), parse_ts(b)) {
        (Some(x), Some(y)) => x > y,
        _ => a > b,
    }
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

#[cfg(test)]
mod tests {
    use super::newer;

    #[test]
    fn fractionless_timestamps_compare_chronologically() {
        // The lexical trap: 'Z' (0x5A) > '.' (0x2E) would call the
        // WHOLE second newer than the later fractional instant.
        assert!(newer("2026-08-19T13:00:06.5Z", "2026-08-19T13:00:06Z"));
        assert!(!newer("2026-08-19T13:00:06Z", "2026-08-19T13:00:06.5Z"));
        assert!(newer("2026-08-19T13:00:06.55Z", "2026-08-19T13:00:06.5Z"));
        assert!(!newer("same", "same"));
    }
}
