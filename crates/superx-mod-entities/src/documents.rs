//! Documents — file-backed content nodes (epic #166, phase E4).
//! A document is an ordinary entity (type 'document'): it lists,
//! links, traverses, and visualizes like everything else. The file
//! bytes live under the module's own dir; the node's attributes
//! carry the metadata.

use superx_kernel::types::{RecordId, Value};
use superx_kernel::{Db, Result};

use crate::edges::link;
use crate::nodes::create_entity;

/// Register an attached document: a `document` node carrying file
/// metadata, linked from its owner (`owner —attached→ document`).
/// File placement is the caller's job (the CLI copies into the
/// module dir); this verb records the substrate facts.
///
/// # Errors
///
/// Verb errors pass through ([`superx_kernel::KernelError`]).
pub async fn attach_document(
    db: &Db,
    owner: &RecordId,
    file_name: &str,
    stored_path: &str,
    mime: &str,
    size: u64,
) -> Result<RecordId> {
    let attributes = superx_kernel::message::value_from_json(&serde_json::json!({
        "file": stored_path,
        "original_name": file_name,
        "mime": mime,
        "size": size,
    }));
    let node = create_entity(db, "document", file_name, None, Some(attributes)).await?;
    link(db, owner, &node, "attached").await?;
    Ok(node)
}

/// A minimal extension → MIME mapping for attach metadata (contract
/// data, not a tunable; unknown extensions record octet-stream).
#[must_use]
pub fn mime_for(file_name: &str) -> &'static str {
    match file_name.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str() {
        "md" | "markdown" => "text/markdown",
        "txt" | "log" => "text/plain",
        "json" => "application/json",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "html" | "htm" => "text/html",
        "csv" => "text/csv",
        "rs" => "text/x-rust",
        "surql" | "sql" => "application/sql",
        _ => "application/octet-stream",
    }
}

/// Extract the stored-file path from a document node's attributes.
#[must_use]
pub fn stored_path(attributes: &Value) -> Option<String> {
    match attributes {
        Value::Object(o) => match o.get("file") {
            Some(Value::String(s)) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}
