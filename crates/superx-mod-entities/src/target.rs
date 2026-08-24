//! What content belongs to (issue #296, spec §3).
//!
//! Notes and attachments are one idea in two shapes — content that
//! belongs to something and is never a node — and the something is an
//! **entity, a type, or a label**. A type is exactly the thing people
//! argue about, and until now it had nowhere to hold the argument.
//!
//! The cost, stated rather than discovered: a polymorphic target cannot
//! be a typed `record<>` link, so the engine's referential-integrity
//! check does not cover it and these verbs carry it instead. That is
//! why [`Target::resolve`] exists and why every write goes through it.

use superx_kernel::types::RecordId;
use superx_kernel::{Db, KernelError, Result};

/// The three things content can belong to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A fact in the graph. Keeps its typed `record<entity>` link too, so
    /// rows written before this and readers that only know about entities
    /// both keep working.
    Entity(RecordId),
    /// A registry row — argued about far more than any single entity.
    Type(String),
    /// A term in the dictionary. Redefining one changes every entity that
    /// used it, which is exactly the kind of thing worth a thread.
    Label(String),
}

/// The three kinds, as the column stores them.
pub const KINDS: [&str; 3] = ["entity", "type", "label"]; // skill-allow: §9-const — the module's own data model, not a tunable

impl Target {
    /// `entity` | `type` | `label`.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Entity(_) => "entity",
            Self::Type(_) => "type",
            Self::Label(_) => "label",
        }
    }

    /// What identifies it: a uuid for an entity, the key for a type or a
    /// label. A type's key IS its identity — types are rows, not nodes.
    #[must_use]
    pub fn uid(&self) -> String {
        match self {
            Self::Entity(id) => superx_ops::record_uuid(id),
            Self::Type(key) | Self::Label(key) => key.clone(),
        }
    }

    /// The typed link, where there is one.
    #[must_use]
    pub fn entity(&self) -> Option<RecordId> {
        match self {
            Self::Entity(id) => Some(id.clone()),
            _ => None,
        }
    }

    /// Find what a kind and a uid name, and refuse if it is not there.
    ///
    /// The engine cannot check a polymorphic target, so this is the only
    /// thing standing between a note and a target that does not exist —
    /// which would be unreachable from both directions: nothing lists it,
    /// and nothing points back.
    ///
    /// # Errors
    ///
    /// [`KernelError::Module`] for an unknown kind, or a target nothing
    /// defines; [`KernelError::Db`] for engine errors.
    pub async fn resolve(db: &Db, kind: &str, uid: &str) -> Result<Self> {
        match kind {
            "entity" => {
                let id = crate::nodes::resolve_entity(db, uid).await?;
                Ok(Self::Entity(id))
            }
            "type" => {
                if crate::registry::list_types(db).await?.iter().any(|t| t.name == uid) {
                    Ok(Self::Type(uid.to_string()))
                } else {
                    Err(KernelError::Module(format!("no type '{uid}'")))
                }
            }
            "label" => {
                // Either kind of label can be argued about, so both count.
                for label_kind in [crate::dictionary::SLOT, crate::dictionary::LINK] {
                    if crate::dictionary::current(db, uid, label_kind).await?.is_some() {
                        return Ok(Self::Label(uid.to_string()));
                    }
                }
                Err(KernelError::Module(format!("the dictionary defines no label '{uid}'")))
            }
            other => Err(KernelError::Module(format!(
                "'{other}' is not something content can belong to — expected one of: {}",
                KINDS.join(", ")
            ))),
        }
    }
}
