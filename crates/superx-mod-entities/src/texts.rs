//! Text-as-entity sugar (epic #166, D22): descriptions, comments,
//! and instructions are `text` NODES linked by role edges. Describe/
//! instruct update the existing text node (its own SCD-2 history);
//! comment always mints a new node — many comments coexist, and a
//! comment on a comment is a thread.

use superx_kernel::types::RecordId;
use superx_kernel::{Db, Result};

use crate::edges::expand;
use crate::nodes::current_state;
use crate::notes::{self, Author};

/// The role edges whose targets are inline text nodes.
pub const TEXT_ROLES: [&str; 3] = ["describes", "comments", "instructs"];

/// The dictionary slot label each legacy role carries (#268).
///
/// The roles are edge names — verbs, from when prose was an entity on
/// the far end of an edge. The labels are what the prose IS, which is
/// what the dictionary defines and what the note store records.
///
/// # Errors
///
/// [`KernelError::Module`] for a role with no label, which would mean
/// prose written under a term nothing can interpret.
pub fn label_for_role(role: &str) -> Result<&'static str> {
    match role {
        "describes" => Ok("description"),
        "instructs" => Ok("instructions"),
        "comments" => Ok("comments"),
        other => Err(superx_kernel::KernelError::Module(format!(
            "'{other}' is not a prose role — expected one of {}",
            TEXT_ROLES.join(", ")
        ))),
    }
}

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
) -> Result<(String, bool)> {
    // THE LAST STEP OF THE SEAM (#302). This wrote the note AND a
    // legacy `text` entity on a role edge, so readers could be moved
    // across one at a time. They all have been — `api::detail` reads
    // `notes::for_entity`, the runner reads `node.note(...)`, and
    // `texts::annotations` has no caller left outside its own tests.
    //
    // So the carrier stops being written. Not deleted: the ~41 that
    // exist stay on the record, append-only, and `migrate-prose` still
    // reads their role edges to move history that predates the note
    // store.
    let (note_uid, created) =
        notes::write(db, target, label_for_role(role)?, text, &Author::operator()).await?;
    Ok((note_uid, created))
}

/// Add a comment: always a fresh text node + `comments` edge.
///
/// # Errors
///
/// Verb errors pass through.
pub async fn add_comment(
    db: &Db,
    target: &RecordId,
    text: &str,
    author: &Author,
) -> Result<String> {
    let (note_uid, _) = notes::write(db, target, label_for_role("comments")?, text, author).await?;
    Ok(note_uid)
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
