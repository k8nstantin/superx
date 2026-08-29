//! Edges — what an entity points at.
//!
//! An edge is built like an attribute: a uid, a name, labels, and a
//! history. The difference is where it is stored. A native relation lets
//! the ENGINE walk the graph — both directions, many levels, one query —
//! and a link kept in a column cannot be walked at all: following it
//! means looking up the next entity's rows, which is a round trip per
//! level. Roles and products get deep enough that this decides whether
//! the graph is readable.
//!
//! MANY EDGES MAY JOIN THE SAME PAIR. A role can both contain a task and
//! consult it, and those are two edges with two names and two histories,
//! not one edge with two meanings.
//!
//! Linking appends a row with a fresh uid. Unlinking appends another on
//! the same uid with `active = false`, and relinking appends a third —
//! so a connection that was cut is still on the record, with who cut it
//! and when.

use std::collections::HashSet;

use superx_kernel::types::{Object, RecordId, Value};
use superx_kernel::{Db, KernelError, Result};
use superx_ops::record_uuid;

use crate::author::Author;
use crate::{new_id, newest_by_valid_from, obj_bool, obj_display, obj_record, obj_records, obj_str};

/// One edge as it currently reads.
#[derive(Debug, Clone)]
pub struct Edge {
    /// Identity across versions — what an unlink and a relink share.
    pub uid: String,
    pub from: RecordId,
    pub to: RecordId,
    pub name: String,
    /// What the connection MEANS. Resolved by the reader.
    pub labels: Vec<RecordId>,
    pub active: bool,
    pub version: String,
}

/// Link two entities. Returns the edge's `uid`.
///
/// # Errors
///
/// [`KernelError::Module`] for an empty name or a self-link;
/// [`KernelError::Db`] for engine errors — including an endpoint that
/// does not exist, which the schema refuses, and a label that does not
/// exist.
pub async fn link(
    db: &Db,
    from: &RecordId,
    to: &RecordId,
    name: &str,
    labels: &[RecordId],
    author: &Author,
) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(KernelError::Module(
            "a connection needs a name — an unnamed edge cannot be read".to_string(),
        ));
    }
    if record_uuid(from) == record_uuid(to) {
        return Err(KernelError::Module(
            "an entity cannot point at itself — that says nothing".to_string(),
        ));
    }
    let uid = record_uuid(&new_id("edge"));
    relate(db, Row { from, to, uid: &uid, name, labels, active: true }, author).await?;
    Ok(uid)
}

/// Cut a link: the same uid, `active = false`. Idempotent — returns
/// whether a row was written.
///
/// # Errors
///
/// [`KernelError::Module`] when the uid has no chain; [`KernelError::Db`]
/// for engine errors.
pub async fn unlink(db: &Db, uid: &str, author: &Author) -> Result<bool> {
    set_active(db, uid, false, author).await
}

/// Put a cut link back.
///
/// # Errors
///
/// As [`unlink`].
pub async fn relink(db: &Db, uid: &str, author: &Author) -> Result<bool> {
    set_active(db, uid, true, author).await
}

async fn set_active(db: &Db, uid: &str, active: bool, author: &Author) -> Result<bool> {
    let now = current(db, uid)
        .await?
        .ok_or_else(|| KernelError::Module(format!("no edge '{uid}'")))?;
    if now.active == active {
        return Ok(false);
    }
    relate(
        db,
        Row {
            from: &now.from,
            to: &now.to,
            uid,
            name: &now.name,
            labels: &now.labels,
            active,
        },
        author,
    )
    .await?;
    Ok(true)
}

/// The current version of one edge.
///
/// # Errors
///
/// [`KernelError::Db`] for engine errors.
pub async fn current(db: &Db, uid: &str) -> Result<Option<Edge>> {
    let mut resp = db
        .query("SELECT *, in, out FROM entity_edge WHERE uid = $uid")
        .bind(("uid", uid.to_string()))
        .await?;
    let rows: Vec<Value> = resp.take(0)?;
    Ok(newest_by_valid_from(&rows).and_then(parse))
}

/// The live edges of one entity: outward, inward, or both.
///
/// This is the read the tree makes on every expand — "what hangs off
/// this one" — so it is one query bounded by the entity's degree, never
/// the table.
///
/// # Errors
///
/// [`KernelError::Db`] for engine errors.
pub async fn of(db: &Db, anchor: &RecordId, dir: Direction) -> Result<Vec<Edge>> {
    let clause = match dir {
        Direction::Out => "in = $e",
        Direction::In => "out = $e",
        Direction::Both => "in = $e OR out = $e",
    };
    let mut resp = db
        .query(format!("SELECT *, in, out FROM entity_edge WHERE {clause}"))
        .bind(("e", anchor.clone()))
        .await?;
    let rows: Vec<Value> = resp.take(0)?;

    Ok(live(rows))
}

/// The current edges of a set of rows: newest row per uid, active only.
///
/// ONE PLACE. This was written out three times, and the copy inside the
/// traversal drifted — it filtered `active = true` at the ROW level,
/// which is wrong in an append-only store because unlinking appends a
/// new row and leaves the old one exactly as it was. The stale
/// `active = true` row kept matching, so a walk went on reaching the
/// other end of a connection that had been cut.
fn live(rows: Vec<Value>) -> Vec<Edge> {
    let mut chains: std::collections::BTreeMap<String, Vec<Value>> =
        std::collections::BTreeMap::new();
    for row in rows {
        let Some(uid) = obj_str(&row, "uid") else { continue };
        chains.entry(uid).or_default().push(row);
    }
    chains
        .into_values()
        .filter_map(|c| newest_by_valid_from(&c).and_then(parse))
        .filter(|e| e.active)
        .collect()
}

/// Which way to look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// What this entity points at.
    Out,
    /// What points at this entity — free, because the engine holds the
    /// pointers both ways.
    In,
    Both,
}

/// Which entities have at least one live edge leaving them AND landing
/// somewhere `visible`, in ONE read.
///
/// A listing needs exactly this and nothing more: whether a row opens.
/// Asking `of` per row turned one screen into a scan of the edge table
/// per entity.
///
/// THE FAR END HAS TO BE VISIBLE. The menu hides archived entities, so
/// an edge into one is not a path anyone can walk there — counting it
/// left rows wearing an expander that opened on nothing.
///
/// # Errors
///
/// [`KernelError::Db`] for engine errors.
pub async fn sources(
    db: &Db,
    visible: &std::collections::HashSet<String>,
) -> Result<std::collections::HashSet<String>> {
    let mut resp = db.query("SELECT *, in, out FROM entity_edge").await?;
    Ok(live(resp.take(0)?)
        .into_iter()
        .filter(|e| visible.contains(&record_uuid(&e.to)))
        .map(|e| record_uuid(&e.from))
        .collect())
}

/// Which entities have at least one live edge pointing AT them, in one
/// read.
///
/// The menu needs it to know what a root is: a tree whose top level is
/// every entity is not a tree, it is a list — and a child then appears
/// twice, once at the root and once under its parent.
///
/// # Errors
///
/// [`KernelError::Db`] for engine errors.
pub async fn targets(
    db: &Db,
    visible: &std::collections::HashSet<String>,
) -> Result<std::collections::HashSet<String>> {
    let mut resp = db.query("SELECT *, in, out FROM entity_edge").await?;
    Ok(live(resp.take(0)?)
        .into_iter()
        .filter(|e| visible.contains(&record_uuid(&e.from)))
        .map(|e| record_uuid(&e.to))
        .collect())
}

/// Every version of one edge, oldest first — including every time it was
/// cut and put back.
///
/// # Errors
///
/// [`KernelError::Db`] for engine errors.
pub async fn history(db: &Db, uid: &str) -> Result<Vec<Edge>> {
    let mut resp = db
        .query(
            "SELECT *, in, out FROM entity_edge WHERE uid = $uid \
             ORDER BY valid_from ASC, id ASC",
        )
        .bind(("uid", uid.to_string()))
        .await?;
    let rows: Vec<Value> = resp.take(0)?;
    Ok(rows.iter().filter_map(parse).collect())
}

fn parse(row: &Value) -> Option<Edge> {
    Some(Edge {
        uid: obj_str(row, "uid")?,
        from: obj_record(row, "in")?,
        to: obj_record(row, "out")?,
        name: obj_str(row, "name")?,
        labels: obj_records(row, "labels"),
        active: obj_bool(row, "active"),
        version: obj_display(row, "valid_from").unwrap_or_default(),
    })
}

/// One edge row, about to be written. A struct rather than a row of
/// positional arguments, because two `&RecordId` and two `&str` next to
/// each other is a swap nothing catches.
struct Row<'a> {
    from: &'a RecordId,
    to: &'a RecordId,
    uid: &'a str,
    name: &'a str,
    labels: &'a [RecordId],
    active: bool,
}

/// The one place an edge row is written.
async fn relate(db: &Db, r: Row<'_>, author: &Author) -> Result<()> {
    let (from, to, uid, name, labels, active) =
        (r.from, r.to, r.uid, r.name, r.labels, r.active);
    let mut row = Object::new();
    row.insert("uid".to_string(), Value::String(uid.to_string()));
    row.insert("name".to_string(), Value::String(name.to_string()));
    row.insert("active".to_string(), Value::Bool(active));
    let mut wanted: Vec<Value> = Vec::new();
    for l in labels {
        let v = Value::RecordId(l.clone());
        if !wanted.contains(&v) {
            wanted.push(v);
        }
    }
    if !wanted.is_empty() {
        row.insert("labels".to_string(), Value::Array(wanted.into()));
    }
    row.insert("valid_from".to_string(), Value::Datetime(chrono::Utc::now().into()));
    author.stamp(&mut row);

    db.query("RELATE $from->entity_edge->$to CONTENT $row")
        .bind(("from", from.clone()))
        .bind(("to", to.clone()))
        .bind(("row", Value::Object(row)))
        .await?
        .check()?;
    Ok(())
}

/// One entity reached by a walk, and how far out it was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reached {
    pub entity: RecordId,
    /// 0 is the root.
    pub depth: usize,
}

/// The subgraph around one entity: what was reached, and the live edges
/// among them.
#[derive(Debug, Clone)]
pub struct Subgraph {
    pub nodes: Vec<Reached>,
    pub edges: Vec<Edge>,
}

/// WALK THE GRAPH FROM ONE ENTITY, to a depth, following only the edges
/// you care about.
///
/// This is the read the graph tab makes, and the reason edges are a
/// native relation rather than a column: the engine follows record
/// pointers, so the whole subtree comes back from ONE traversal instead
/// of a round trip per level. A four-deep role graph is one query, and
/// so is a ten-deep one.
///
/// One request per level, each bounded by the frontier — so the cost
/// follows what is reachable, not the size of the edge history.
///
/// `label` narrows it to one kind of connection, which is what makes a
/// tree out of a graph: "follow `contains` from here" is a hierarchy,
/// while following everything is a web.
///
/// # Errors
///
/// [`KernelError::Db`] for engine errors.
pub async fn walk(
    db: &Db,
    root: &RecordId,
    label: Option<&RecordId>,
    depth: usize,
) -> Result<Subgraph> {
    // ONE READ PER LEVEL, bounded by the frontier.
    //
    // The traversal cannot be left to the engine: SurrealQL filters
    // ROWS, and "is this edge still connected" is a question about a
    // CHAIN — unlinking appends `active = false` and leaves the earlier
    // row untouched, so a row-level filter followed connections that had
    // been cut. But reading the WHOLE table to resolve chains in Rust
    // was worse: cost grew with every version ever written, whatever the
    // depth asked for. Asking per level keeps it proportional to what is
    // actually reachable.
    let mut nodes = vec![Reached { entity: root.clone(), depth: 0 }];
    let mut seen: HashSet<String> = HashSet::new();
    seen.insert(record_uuid(root));
    let mut frontier = vec![root.clone()];
    let mut edges: Vec<Edge> = Vec::new();

    // BREADTH FIRST, so the depth recorded is the SHORTEST way there. A
    // depth-first walk records whichever branch happened to arrive
    // first, which paints a direct neighbour as though it were far away.
    for d in 1..=depth {
        if frontier.is_empty() {
            break;
        }
        // BOTH DIRECTIONS. A neighbourhood is what points at you as
        // much as what you point at — the engine holds the pointers
        // both ways, so the inbound half is free. Following only `in`
        // meant every leaf drew as a lone dot with no connections,
        // while the entity page listed its inbound links directly
        // underneath: the same graph, described two different ways on
        // one screen.
        let mut resp = db
            .query("SELECT *, in, out FROM entity_edge WHERE in IN $ids OR out IN $ids")
            .bind(("ids", frontier.clone()))
            .await?;
        let level: Vec<Edge> = live(resp.take(0)?)
            .into_iter()
            .filter(|e| label.is_none_or(|l| e.labels.contains(l)))
            .collect();

        // The far end is whichever end is not the one we came from.
        let here: HashSet<String> = frontier.iter().map(record_uuid).collect();
        let mut next: Vec<RecordId> = Vec::new();
        for e in &level {
            let far = if here.contains(&record_uuid(&e.from)) { &e.to } else { &e.from };
            if seen.insert(record_uuid(far)) {
                nodes.push(Reached { entity: far.clone(), depth: d });
                next.push(far.clone());
            }
        }
        edges.extend(level);
        frontier = next;
    }

    // Only the connections BETWEEN things actually reached — a level
    // read brings back edges pointing past the depth budget too.
    let reached: HashSet<String> = nodes.iter().map(|n| record_uuid(&n.entity)).collect();
    edges.retain(|e| {
        reached.contains(&record_uuid(&e.from)) && reached.contains(&record_uuid(&e.to))
    });
    edges.sort_by(|a, b| a.uid.cmp(&b.uid));
    edges.dedup_by(|a, b| a.uid == b.uid);

    Ok(Subgraph { nodes, edges })
}
