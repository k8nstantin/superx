use thiserror::Error;

/// Errors surfaced by kernel operations.
///
/// SurrealDB errors propagate verbatim via [`KernelError::Db`]. Engine
/// refusals (`ASSERT` failures, `SCHEMAFULL` field-type mismatches,
/// `PERMISSIONS` rejections) reach the caller through this variant —
/// per [SKILL.md §12], the caller's response is to fix the code that
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
}

pub type Result<T> = std::result::Result<T, KernelError>;
