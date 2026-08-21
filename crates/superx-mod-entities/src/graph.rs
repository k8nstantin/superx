//! Graph traversal — BFS over pointer expansion (epic #166, phase
//! E3). Wall cost per level is one pointer query for the whole
//! frontier plus one id-addressed state fetch; the edge table's total
//! size (history included) is never scanned.

use std::collections::{HashMap, HashSet};

use superx_kernel::types::RecordId;
use superx_kernel::{Db, Result};
use superx_ops::record_uuid;

use crate::edges::{expand, EdgeRow};
use crate::nodes::current_meta;

/// A traversed node, in visit order.
pub struct GraphNode {
    pub id: RecordId,
    pub entity_type: String,
    pub name: String,
    pub content: Option<String>,
    /// FLEXIBLE attributes of the current state (prompt context).
    pub attributes: Option<superx_kernel::types::Value>,
    /// Current state's valid_from — the version the reader saw.
    pub version: String,
    pub depth: usize,
}

/// An active edge inside the traversed subgraph.
pub struct GraphEdge {
    pub edge_uid: String,
    pub from: String,
    pub to: String,
    pub rel_type: String,
}

/// The subgraph reachable from `root` within `max_depth` hops.
pub struct Subgraph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub truncated_at_depth: bool,
}

/// BFS from a root. Cycles are legal in the graph; the visited set
/// terminates them. Only ACTIVE edges are followed; inactive chains
/// stay history.
///
/// # Errors
///
/// [`KernelError::Db`](superx_kernel::KernelError::Db) for engine
/// errors.
pub async fn subgraph(
    db: &Db,
    root: &RecordId,
    max_depth: usize,
    reverse: bool,
) -> Result<Subgraph> {
    let mut nodes: Vec<GraphNode> = Vec::new();
    let mut edges: Vec<GraphEdge> = Vec::new();
    let mut visited: HashSet<String> = HashSet::new();
    let mut truncated = false;

    let mut frontier: Vec<RecordId> = vec![root.clone()];
    visited.insert(record_uuid(root));
    // Labels are batch-resolved PER LEVEL for just the visited
    // frontier (issue #179): a walk costs the reached nodes, never
    // the table — the same contract the edge expansion keeps.
    push_level(db, &mut nodes, std::slice::from_ref(root), 0).await?;

    let mut depth = 0;
    while !frontier.is_empty() {
        if depth >= max_depth {
            truncated = true;
            break;
        }
        let level: Vec<EdgeRow> = expand(db, &frontier, reverse)
            .await?
            .into_iter()
            .filter(|e| e.active)
            .collect();
        let mut next: Vec<RecordId> = Vec::new();
        for edge in level {
            edges.push(GraphEdge {
                edge_uid: edge.edge_uid.clone(),
                from: record_uuid(&edge.from),
                to: record_uuid(&edge.to),
                rel_type: edge.rel_type.clone(),
            });
            let target = if reverse { edge.from } else { edge.to };
            if visited.insert(record_uuid(&target)) {
                next.push(target);
            }
        }
        push_level(db, &mut nodes, &next, depth + 1).await?;
        frontier = next;
        depth += 1;
    }

    Ok(Subgraph { nodes, edges, truncated_at_depth: truncated })
}

/// Resolve one BFS level's metadata in a single batched read and
/// append the level's nodes in stable (uuid7 = creation) order.
async fn push_level(
    db: &Db,
    nodes: &mut Vec<GraphNode>,
    level: &[RecordId],
    depth: usize,
) -> Result<()> {
    if level.is_empty() {
        return Ok(());
    }
    let meta = current_meta(db, level).await?;
    for id in level {
        let uuid = record_uuid(id);
        let (entity_type, name, content, attributes, version) = match meta.get(&uuid) {
            Some(m) => (
                m.entity_type.clone(),
                m.name.clone(),
                m.content.clone(),
                m.attributes.clone(),
                m.version.clone(),
            ),
            None => (String::new(), String::new(), None, None, String::new()),
        };
        // Content rides along only for content-bearing nodes; other
        // kinds keep exports lean.
        let content = if entity_type == "text" || entity_type == "document" {
            content
        } else {
            None
        };
        nodes.push(GraphNode { id: id.clone(), entity_type, name, content, attributes, version, depth });
    }
    Ok(())
}

/// Render a subgraph as a terminal tree (children grouped under
/// their parent, rel-type labelled).
#[must_use]
pub fn render_tree(graph: &Subgraph, root: &RecordId) -> String {
    let by_uuid: HashMap<String, &GraphNode> =
        graph.nodes.iter().map(|n| (record_uuid(&n.id), n)).collect();
    let mut children: HashMap<String, Vec<&GraphEdge>> = HashMap::new();
    for edge in &graph.edges {
        children.entry(edge.from.clone()).or_default().push(edge);
    }

    let root_uuid = record_uuid(root);
    let mut out = String::new();
    if let Some(node) = by_uuid.get(&root_uuid) {
        out.push_str(&format!("{} {} {}\n", root_uuid, node.entity_type, node.name));
    }
    let mut drawn: HashSet<String> = HashSet::new();
    drawn.insert(root_uuid.clone());
    render_children(&root_uuid, "", &by_uuid, &children, &mut drawn, &mut out);
    if graph.truncated_at_depth {
        out.push_str("(deeper levels exist — raise --depth)\n");
    }
    out
}

fn render_children(
    parent: &str,
    prefix: &str,
    by_uuid: &HashMap<String, &GraphNode>,
    children: &HashMap<String, Vec<&GraphEdge>>,
    drawn: &mut HashSet<String>,
    out: &mut String,
) {
    let Some(edges) = children.get(parent) else { return };
    for (i, edge) in edges.iter().enumerate() {
        let last = i == edges.len() - 1;
        let branch = if last { "└─" } else { "├─" };
        let label = match by_uuid.get(&edge.to) {
            Some(n) if n.entity_type == "text" => {
                format!("text \"{}\"", first_line(n.content.as_deref().unwrap_or(&n.name)))
            }
            Some(n) => format!("{} {}", n.entity_type, n.name),
            None => "?".to_string(),
        };
        out.push_str(&format!("{prefix}{branch} {} → {}  {}\n", edge.rel_type, &edge.to[..13], label));
        if drawn.insert(edge.to.clone()) {
            let next_prefix = format!("{prefix}{}", if last { "   " } else { "│  " });
            render_children(&edge.to, &next_prefix, by_uuid, children, drawn, out);
        } else if children.contains_key(&edge.to) {
            let next_prefix = format!("{prefix}{}", if last { "   " } else { "│  " });
            out.push_str(&format!("{next_prefix}(already shown above — cycle)\n"));
        }
    }
}

fn first_line(s: &str) -> String {
    let line = s.lines().next().unwrap_or("");
    if line.chars().count() > 60 {
        let cut: String = line.chars().take(60).collect();
        format!("{cut}…")
    } else {
        line.to_string()
    }
}

/// The subgraph as the CLI's JSON contract — ONE producer so the
/// `graph --json` command and every consumer (the runner, tests, the
/// future UI page) speak the identical shape (#192 amendment).
#[must_use]
pub fn to_json(graph: &Subgraph, root: &RecordId) -> serde_json::Value {
    serde_json::json!({
        "root": record_uuid(root),
        "truncated_at_depth": graph.truncated_at_depth,
        "nodes": graph.nodes.iter().map(|n| serde_json::json!({
            "uid": record_uuid(&n.id),
            "type": n.entity_type,
            "name": n.name,
            "content": n.content,
            "attributes": n.attributes.as_ref().map(crate::nodes::value_to_json),
            "version": n.version,
            "depth": n.depth,
        })).collect::<Vec<_>>(),
        "edges": graph.edges.iter().map(|e| serde_json::json!({
            "edge_uid": e.edge_uid,
            "from": e.from,
            "to": e.to,
            "rel": e.rel_type,
        })).collect::<Vec<_>>(),
    })
}

/// Parent-edge priority when an entity has several incoming edges
/// (issue #252): structural containment first, then attachment, then
/// the text roles, then anything else. The first match wins, so a
/// component under a product reads as its child even when other
/// entities also point at it.
const PARENT_PRIORITY: [&str; 6] = [
    "contains",
    "attached",
    "describes",
    "instructs",
    "comments",
    "produced",
];

/// One step of an ancestor path.
pub struct Ancestor {
    pub id: RecordId,
    pub entity_type: String,
    pub name: String,
    /// The edge that links this ancestor to the step below it.
    pub rel_type: String,
}

/// The ancestor path of an entity, ROOT FIRST, excluding the entity
/// itself: walk active incoming edges upward, choosing one parent per
/// level by [`PARENT_PRIORITY`]. Cycle-safe (a visited set stops the
/// walk) and depth-capped, so a cyclic graph terminates instead of
/// climbing forever.
///
/// # Errors
///
/// [`KernelError::Db`](superx_kernel::KernelError::Db) for engine
/// errors.
pub async fn ancestors(db: &Db, anchor: &RecordId, max_depth: usize) -> Result<Vec<Ancestor>> {
    let mut path: Vec<Ancestor> = Vec::new();
    let mut visited: HashSet<String> = HashSet::from([record_uuid(anchor)]);
    let mut here = anchor.clone();

    for _ in 0..max_depth {
        let incoming: Vec<EdgeRow> = expand(db, std::slice::from_ref(&here), true)
            .await?
            .into_iter()
            .filter(|e| e.active && !visited.contains(&record_uuid(&e.from)))
            .collect();
        if incoming.is_empty() {
            break;
        }
        // One parent per level: priority first, then creation order
        // (uuid7 sorts chronologically) so the choice is stable.
        let pick = PARENT_PRIORITY
            .iter()
            .find_map(|rel| {
                let mut matches: Vec<&EdgeRow> =
                    incoming.iter().filter(|e| e.rel_type == *rel).collect();
                matches.sort_by_key(|e| record_uuid(&e.from));
                matches.first().copied()
            })
            .or_else(|| {
                let mut rest: Vec<&EdgeRow> = incoming.iter().collect();
                rest.sort_by_key(|e| record_uuid(&e.from));
                rest.first().copied()
            });
        let Some(edge) = pick else { break };

        let parent = edge.from.clone();
        let uuid = record_uuid(&parent);
        visited.insert(uuid.clone());
        let meta = current_meta(db, std::slice::from_ref(&parent)).await?;
        let (entity_type, name) = match meta.get(&uuid) {
            Some(m) => (m.entity_type.clone(), m.name.clone()),
            None => (String::new(), String::new()),
        };
        path.push(Ancestor {
            id: parent.clone(),
            entity_type,
            name,
            rel_type: edge.rel_type.clone(),
        });
        here = parent;
    }

    path.reverse(); // root first
    Ok(path)
}
