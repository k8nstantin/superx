//! Documents — file-backed content nodes (epic #166, phase E4).
//! A document is an ordinary entity (type 'document'): it lists,
//! links, traverses, and visualizes like everything else. The file
//! bytes live under the module's own dir; the node's attributes
//! carry the metadata.



/// Register an attached document: a `document` node carrying file
/// metadata, linked from its owner (`owner —attached→ document`).
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
use superx_kernel::types::Value;

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
