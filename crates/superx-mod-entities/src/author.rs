//! Who wrote this.
//!
//! Every versioned row in this module carries an author, because the
//! interesting writes are not the operator's. An agent names a thing, a
//! job sets a field, a role links two entities — and "who did this" is
//! the authorization record, not decoration on it.
//!
//! A plain string uid, never a `record<>`: an author may live in another
//! database entirely, and a typed link across databases is a promise
//! this module cannot keep.

use superx_kernel::types::{Object, Value};
use superx_kernel::{KernelError, Result};

/// Who a write may claim to be. Closed, because a value nothing
/// recognises is a value nothing can act on — and this one is the
/// subject of authorization.
pub const KINDS: [&str; 4] = ["operator", "role", "agent", "system"];

/// The author of one write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Author {
    /// One of [`KINDS`].
    pub kind: String,
    /// The author's own uuid, where there is one.
    pub uid: Option<String>,
    /// The ROLE context: "agent X, acting as role Y".
    pub via: Option<String>,
}

impl Author {
    /// The operator, at the console.
    #[must_use]
    pub fn operator() -> Self {
        Self { kind: "operator".to_string(), uid: None, via: None }
    }

    /// An author a caller claims to be.
    ///
    /// # Errors
    ///
    /// [`KernelError::Module`] for a kind outside [`KINDS`]. A write
    /// recorded as something nobody can interpret is worse than one
    /// recorded as unknown.
    pub fn claimed(kind: &str, uid: Option<&str>, via: Option<&str>) -> Result<Self> {
        if !KINDS.contains(&kind) {
            return Err(KernelError::Module(format!(
                "author kind '{kind}' is not one of: {}",
                KINDS.join(", ")
            )));
        }
        Ok(Self {
            kind: kind.to_string(),
            uid: uid.map(str::to_string),
            via: via.map(str::to_string),
        })
    }

    /// Stamp the three author columns onto a row about to be written.
    /// One place, so a table that forgets one is a compile-time omission
    /// rather than a silent gap in the record.
    pub fn stamp(&self, row: &mut Object) {
        row.insert("author_kind".to_string(), Value::String(self.kind.clone()));
        if let Some(uid) = &self.uid {
            row.insert("author_uid".to_string(), Value::String(uid.clone()));
        }
        if let Some(via) = &self.via {
            row.insert("via_uid".to_string(), Value::String(via.clone()));
        }
    }
}
