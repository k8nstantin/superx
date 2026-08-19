//! Text-as-entity sugar (epic #166, D22): descriptions, comments,
//! and instructions are `text` NODES linked by role edges. Describe/
//! instruct update the existing text node (its own SCD-2 history);
//! comment always mints a new node — many comments coexist, and a
//! comment on a comment is a thread.

use superx_kernel::types::RecordId;
use superx_kernel::{Db, Result};

use crate::edges::{expand, link};
use crate::nodes::{create_entity, current_state, update_entity};

/// The role edges whose targets are inline text nodes.
pub const TEXT_ROLES: [&str; 3] = ["describes", "comments", "instructs"];

/// One annotation on an entity, resolved for display.
pub struct Annotation {
    pub rel_type: String,
    pub text_id: RecordId,
    pub content: String,
}

/// Set the role-text of an entity (describe/instruct semantics): if
/// an active edge of this role already points at a text node, a new
/// VERSION is appended to that node (one evolving text with its own
/// history); otherwise a text node is created and linked. Returns
/// (text node, created_new).
///
/// # Errors
///
/// Verb errors pass through ([`superx_kernel::KernelError`]).
pub async fn set_role_text(
    db: &Db,
    target: &RecordId,
    role: &str,
    text: &str,
) -> Result<(RecordId, bool)> {
    let existing = expand(db, std::slice::from_ref(target), false)
        .await?
        .into_iter()
        .find(|e| e.active && e.rel_type == role);
    if let Some(edge) = existing {
        update_entity(db, &edge.to, Some(label_for(text)), Some(text.to_string()), None).await?;
        return Ok((edge.to, false));
    }
    let node = create_entity(db, "text", &label_for(text), Some(text.to_string()), None).await?;
    link(db, target, &node, role).await?;
    Ok((node, true))
}

/// Add a comment: always a fresh text node + `comments` edge.
///
/// # Errors
///
/// Verb errors pass through.
pub async fn add_comment(db: &Db, target: &RecordId, text: &str) -> Result<RecordId> {
    let node = create_entity(db, "text", &label_for(text), Some(text.to_string()), None).await?;
    link(db, target, &node, "comments").await?;
    Ok(node)
}

/// The current text annotations of an entity (active role edges →
/// current content of each text node), for `show`.
///
/// # Errors
///
/// Verb errors pass through.
pub async fn annotations(db: &Db, target: &RecordId) -> Result<Vec<Annotation>> {
    let mut out = Vec::new();
    for edge in expand(db, std::slice::from_ref(target), false).await? {
        if !edge.active || !TEXT_ROLES.contains(&edge.rel_type.as_str()) {
            continue;
        }
        let content = current_state(db, &edge.to)
            .await?
            .and_then(|s| s.content.or(Some(s.name)))
            .unwrap_or_default();
        out.push(Annotation { rel_type: edge.rel_type, text_id: edge.to, content });
    }
    Ok(out)
}

/// A one-line label derived from the text (name column contract).
fn label_for(text: &str) -> String {
    let line = text.lines().next().unwrap_or("");
    if line.chars().count() > 40 {
        let cut: String = line.chars().take(40).collect();
        format!("{cut}…")
    } else {
        line.to_string()
    }
}
