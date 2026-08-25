//! Does this graph make sense? (§5.5)
//!
//! > Once cardinality, endpoints and acyclicity are DATA, "does this
//! > graph make sense?" derives from the dictionary alone … exactly the
//! > check to run **before dispatching agents at a graph one of them
//! > designed** — and free, because the definitions were needed anyway.
//!
//! Every rule here is read from the dictionary; none is written in this
//! file. A new label with new endpoints is checked without a code
//! change, which is the same property that lets the UI render a type it
//! has never seen.
//!
//! WHY IT EXISTS SEPARATELY FROM THE WRITE CHECKS. `edges::link` refuses
//! a bad edge at creation, but a graph can be wrong without any single
//! write being wrong: a type gains a required slot after its entities
//! were made, a label is narrowed after the edges exist, a cycle is
//! closed by data written before the rule. The write path guards the
//! future; this reads the present.

use std::collections::HashSet;

use superx_kernel::types::RecordId;
use superx_kernel::{Db, Result};
use superx_ops::record_uuid;

use crate::{dictionary, edges, graph, nodes, notes};

/// One thing wrong with the graph, in the operator's terms.
pub struct Finding {
    /// The entity or edge it is about.
    pub subject: String,
    pub detail: String,
}

/// Walk the subgraph and report everything that does not fit the
/// dictionary.
///
/// Returns findings rather than the first error: an operator about to
/// dispatch wants the whole list, not one problem at a time.
///
/// # Errors
///
/// [`KernelError::Db`](superx_kernel::KernelError::Db) for engine
/// errors.
pub async fn subgraph(db: &Db, root: &RecordId, depth: usize) -> Result<Vec<Finding>> {
    let mut findings = Vec::new();
    let sub = graph::subgraph(db, root, depth, false).await?;

    // --- edges: endpoints, and cycles in an acyclic label ------------
    let mut seen_labels: HashSet<String> = HashSet::new();
    for edge in &sub.edges {
        // BY NAME, for the same reason the write check is: a label under
        // the merged kind is still the label this edge carries.
        let Some(label) = dictionary::find(db, &edge.rel_type).await? else {
            findings.push(Finding {
                subject: format!("{} -[{}]-> {}", &edge.from[..8], edge.rel_type, &edge.to[..8]),
                detail: format!(
                    "'{}' is not a link label — an edge nobody has defined cannot be \
                     interpreted, and an agent meeting it has to guess",
                    edge.rel_type
                ),
            });
            continue;
        };
        let from = nodes::resolve_entity(db, &edge.from).await?;
        let to = nodes::resolve_entity(db, &edge.to).await?;
        let (from_type, _) = nodes::anchor_info(db, &from).await?;
        let (to_type, _) = nodes::anchor_info(db, &to).await?;
        if !label.source_types.is_empty() && !label.source_types.contains(&from_type) {
            findings.push(Finding {
                subject: format!("{} -[{}]-> {}", &edge.from[..8], edge.rel_type, &edge.to[..8]),
                detail: format!(
                    "'{}' starts at {} — this one starts at a {from_type}",
                    edge.rel_type,
                    label.source_types.join(" or ")
                ),
            });
        }
        if !label.target_types.is_empty() && !label.target_types.contains(&to_type) {
            findings.push(Finding {
                subject: format!("{} -[{}]-> {}", &edge.from[..8], edge.rel_type, &edge.to[..8]),
                detail: format!(
                    "'{}' points at {} — this one points at a {to_type}",
                    edge.rel_type,
                    label.target_types.join(" or ")
                ),
            });
        }
        if label.acyclic && seen_labels.insert(edge.rel_type.clone()) {
            // Once per label, not once per edge: the walk below covers
            // every chain the label makes.
            if let Some(loop_at) = a_cycle_in(db, &sub, &edge.rel_type).await? {
                findings.push(Finding {
                    subject: loop_at.clone(),
                    detail: format!(
                        "'{}' is acyclic and this closes a loop — the runner's wave pass \
                         drops every task in a cycle, so the work never runs and nothing \
                         says why",
                        edge.rel_type
                    ),
                });
            }
        }
    }

    // --- entities: what their type says they must carry --------------
    for node in &sub.nodes {
        let uuid = record_uuid(&node.id);
        let slots = dictionary::slots_for(db, &node.entity_type, false).await?;
        let held: HashSet<String> =
            notes::for_entity(db, &node.id, false).await?.into_iter().map(|n| n.label).collect();
        for slot in slots.iter().filter(|s| s.required) {
            let in_attrs = matches!(
                &node.attributes,
                Some(superx_kernel::types::Value::Object(o)) if o.contains_key(&slot.label)
            );
            if !held.contains(&slot.label) && !in_attrs {
                findings.push(Finding {
                    subject: format!("{uuid} ({})", node.entity_type),
                    detail: format!(
                        "carries no '{}', and its type requires one — an agent handed this \
                         is missing something it was promised",
                        slot.label
                    ),
                });
            }
        }
        // A `one` label holding several notes is two answers to a
        // question that has one.
        for label in &held {
            let singular = dictionary::find(db, label)
                .await?
                .and_then(|d| d.cardinality)
                .as_deref()
                == Some("one");
            if !singular {
                continue;
            }
            let count = notes::for_entity(db, &node.id, false)
                .await?
                .into_iter()
                .filter(|n| &n.label == label)
                .count();
            if count > 1 {
                findings.push(Finding {
                    subject: format!("{uuid} ({})", node.entity_type),
                    detail: format!(
                        "holds {count} '{label}' notes but the label is `one` — nothing says \
                         which of them counts"
                    ),
                });
            }
        }
    }

    Ok(findings)
}

/// Is there a loop among the edges carrying this label? Returns the
/// node it closes at.
async fn a_cycle_in(db: &Db, sub: &graph::Subgraph, label: &str) -> Result<Option<String>> {
    let _ = db;
    let mut out: std::collections::HashMap<&str, Vec<&str>> = std::collections::HashMap::new();
    for e in sub.edges.iter().filter(|e| e.rel_type == label) {
        out.entry(e.from.as_str()).or_default().push(e.to.as_str());
    }
    // Depth-first with a path set — a cycle is an edge back onto the
    // path being walked, which is exactly what "would close a loop"
    // means.
    let mut done: HashSet<&str> = HashSet::new();
    for start in out.keys().copied().collect::<Vec<_>>() {
        let mut stack = vec![(start, false)];
        let mut path: Vec<&str> = Vec::new();
        while let Some((node, backtrack)) = stack.pop() {
            if backtrack {
                path.pop();
                continue;
            }
            if path.contains(&node) {
                return Ok(Some(node.to_string()));
            }
            if !done.insert(node) {
                continue;
            }
            path.push(node);
            stack.push((node, true));
            for next in out.get(node).into_iter().flatten() {
                stack.push((next, false));
            }
        }
    }
    Ok(None)
}

/// Walk from every root the operator can see, for the whole-instance
/// check.
///
/// # Errors
///
/// Verb errors pass through.
pub async fn everything(db: &Db, depth: usize) -> Result<Vec<Finding>> {
    let mut findings = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for row in nodes::list_entities(db, None).await? {
        if row.archived {
            continue;
        }
        // Only roots: an entity nothing points at. Walking from every
        // node would report the same finding once per path to it.
        let inbound = edges::expand(db, std::slice::from_ref(&row.id), true).await?;
        if inbound.iter().any(|e| e.active) {
            continue;
        }
        for f in subgraph(db, &row.id, depth).await? {
            if seen.insert(format!("{}|{}", f.subject, f.detail)) {
                findings.push(f);
            }
        }
    }
    Ok(findings)
}
