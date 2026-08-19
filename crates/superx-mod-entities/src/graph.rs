//! Graph traversal — BFS over pointer expansion (epic #166, phase
//! E3). Wall cost per level is one pointer query for the whole
//! frontier plus one id-addressed state fetch; the edge table's total
//! size (history included) is never scanned.

use std::collections::{HashMap, HashSet};

use superx_kernel::types::RecordId;
use superx_kernel::{Db, Result};
use superx_ops::record_uuid;

use crate::edges::{expand, EdgeRow};
use crate::nodes::{current_state, list_entities};

/// A traversed node, in visit order.
pub struct GraphNode {
    pub id: RecordId,
    pub entity_type: String,
    pub name: String,
    pub content: Option<String>,
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
    // Anchor metadata for labels — one pass over anchors + one
    // batched label read (list_entities), reused across all levels.
    let labels: HashMap<String, (String, String)> = list_entities(db, None)
        .await?
        .into_iter()
        .map(|row| (record_uuid(&row.id), (row.entity_type, row.name)))
        .collect();

    let mut nodes: Vec<GraphNode> = Vec::new();
    let mut edges: Vec<GraphEdge> = Vec::new();
    let mut visited: HashSet<String> = HashSet::new();
    let mut truncated = false;

    let mut frontier: Vec<RecordId> = vec![root.clone()];
    visited.insert(record_uuid(root));
    push_node(db, &mut nodes, root, &labels, 0).await?;

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
                push_node(db, &mut nodes, &target, &labels, depth + 1).await?;
                next.push(target);
            }
        }
        frontier = next;
        depth += 1;
    }

    Ok(Subgraph { nodes, edges, truncated_at_depth: truncated })
}

async fn push_node(
    db: &Db,
    nodes: &mut Vec<GraphNode>,
    id: &RecordId,
    labels: &HashMap<String, (String, String)>,
    depth: usize,
) -> Result<()> {
    let uuid = record_uuid(id);
    let (entity_type, name) = labels.get(&uuid).cloned().unwrap_or_default();
    // Content travels with content-bearing nodes (texts, documents)
    // so exports and trees can show it without a second pass.
    let content = if entity_type == "text" || entity_type == "document" {
        current_state(db, id).await?.and_then(|s| s.content)
    } else {
        None
    };
    nodes.push(GraphNode { id: id.clone(), entity_type, name, content, depth });
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
