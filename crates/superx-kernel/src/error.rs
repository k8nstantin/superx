use thiserror::Error;

/// Errors surfaced by kernel operations.
///
/// SurrealDB errors propagate verbatim via [`KernelError::Db`]. Engine
/// refusals (`ASSERT` failures, `SCHEMAFULL` field-type mismatches,
/// `PERMISSIONS` rejections) reach the caller through this variant —
/// per SKILL.md §12, the caller's response is to fix the code that
/// issued the rejected operation, never to authenticate as root to
/// bypass the refusal.
#[derive(Debug, Error)]
pub enum KernelError {
    /// Connection / signin / wire-protocol error from the SurrealDB SDK.
    #[error("surrealdb error: {0}")]
    Db(#[from] surrealdb::Error),

    /// JSON serialization error when shaping a payload for a kernel verb.
    #[error("json serialization error: {0}")]
    Json(#[from] serde_json::Error),

    /// A SELECT lookup expected at least one row and got none — e.g.
    /// [`crate::Kernel::find_type`] was asked for a `type_definition`
    /// row by uid and no row with that uid exists in the substrate.
    #[error("not found: {0}")]
    NotFound(String),

    /// A substrate payload was read back but is semantically invalid —
    /// e.g. an `attr_lifecycle_state` row carries an unknown state tag,
    /// or a typed payload is missing its required shape. This indicates
    /// a corrupt or foreign write; the offending row must be inspected,
    /// never silently coerced into a default value.
    #[error("corrupt substrate state: {0}")]
    Corrupt(String),
}

pub type Result<T> = std::result::Result<T, KernelError>;
