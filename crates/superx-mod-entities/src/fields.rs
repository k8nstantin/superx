//! Declared, typed values (issue #274).
//!
//! `entity_state.attributes` is a FLEXIBLE bag, and until now nothing
//! declared what belongs in it: a typo was a new field, `"true"` and
//! `true` were equally welcome, and a UI had nothing to render from but
//! a blob of JSON.
//!
//! **A field is a slot label whose kind is a value kind.** The
//! dictionary already carries `value_kind` and already decides storage —
//! prose kinds become note chains (#268), value kinds live in the
//! attributes bag — and `type_label` already declares which slots a type
//! carries. So this needs no new table and no second vocabulary: a new
//! field is a dictionary entry, exactly as a new kind of prose is.
//!
//! Two rules hold without exception:
//!
//! * **a value that names another entity is an edge, never a field** —
//!   otherwise composition hides inside JSON where the graph cannot
//!   traverse it and an agent cannot see it
//! * **a secret field holds a reference, never a value** — the secret
//!   lives in the tool; the graph holds a pointer to it
//!
//! And declarations must be able to evolve: validation applies the
//! CURRENT declarations on WRITE only, reads never fail, and a type that
//! declares nothing accepts anything. Without that, the first refinement
//! of a type would break every entity written before it.

use superx_kernel::types::{Object, RecordId, Value};
use superx_kernel::{Db, KernelError, Result};

use crate::dictionary::{self, SLOT};
use crate::nodes::{current_state, update_entity};

/// Kinds whose content is prose: a versioned note chain, read by
/// people and by models, edited over time.
pub const PROSE_KINDS: [&str; 2] = ["markdown", "text"];

/// Kinds whose content is a value you compute with, stored in the
/// entity's attributes bag.
///
/// `string` and the prose kinds differ by STORAGE and versioning rather
/// than by length: a branch name is a value that gets overwritten, a
/// description is prose whose every wording stays readable.
pub const VALUE_KINDS: [&str; 7] =
    ["string", "number", "integer", "boolean", "datetime", "enum", "url"];

/// A pointer to a secret held elsewhere. The value never enters the
/// graph, so what is stored is the means of finding it.
pub const SECRET_KIND: &str = "secret_ref"; // skill-allow: §9-const — the module's own data model, not a tunable

/// How a secret reference may be written. A closed set, because "it
/// looks like a reference" is not something that can be checked, while
/// "it names one of the places we resolve from" is.
pub const SECRET_SOURCES: [&str; 3] = ["env", "keychain", "vault"];

/// One declared field of an entity, with its value if it has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub key: String,
    pub value_kind: String,
    pub required: bool,
    /// `None` when the entity has nothing in this slot yet.
    pub value: Option<String>,
    /// True when the entity holds a key the type no longer declares.
    /// Reads never fail, so this surfaces rather than disappears.
    pub undeclared: bool,
}

/// Is this kind stored as a value rather than as prose?
#[must_use]
pub fn is_value_kind(kind: &str) -> bool {
    VALUE_KINDS.contains(&kind) || kind == SECRET_KIND
}

/// Every kind a slot label may declare.
#[must_use]
pub fn all_kinds() -> Vec<&'static str> {
    let mut kinds: Vec<&'static str> = PROSE_KINDS.to_vec();
    kinds.extend(VALUE_KINDS);
    kinds.push(SECRET_KIND);
    kinds
}

/// Set one declared field on an entity.
///
/// The dictionary says what kind the slot is and the value is checked
/// against it, so `"yes"` never becomes a boolean and a typo never
/// becomes a new field.
///
/// # Errors
///
/// [`KernelError::Module`] when the label is undeclared, is prose rather
/// than a value, the value does not match its kind, names an entity, or
/// is a secret written as anything but a reference;
/// [`KernelError::Db`] for engine errors.
pub async fn set(db: &Db, entity: &RecordId, key: &str, value: &str) -> Result<()> {
    let (entity_type, _) = crate::nodes::anchor_info(db, entity).await?;
    let declared = dictionary::current(db, key, SLOT).await?.ok_or_else(|| {
        KernelError::Module(format!(
            "the dictionary declares no slot '{key}' — define it first \
             (superx entities labels define), because a key nobody declared is \
             a key nobody reads"
        ))
    })?;
    let kind = declared.value_kind.clone().unwrap_or_default();

    if PROSE_KINDS.contains(&kind.as_str()) {
        return Err(KernelError::Module(format!(
            "'{key}' is {kind} — prose lives in its own store, not the attributes \
             bag; write it with describe or comment"
        )));
    }
    if !is_value_kind(&kind) {
        return Err(KernelError::Module(format!(
            "'{key}' declares no value kind, so nothing knows how to read it — \
             give it one of: {}",
            VALUE_KINDS.join(", ")
        )));
    }

    // A type that declares nothing accepts anything: a role must be able
    // to invent a type and refine it later. But once a type HAS
    // declarations, a key outside them is a mistake, not an extension.
    let slots = dictionary::slots_for(db, &entity_type, false).await?;
    if !slots.is_empty() && !slots.iter().any(|s| s.label == key) {
        return Err(KernelError::Module(format!(
            "type '{entity_type}' does not carry '{key}' — it carries: {}",
            slots.iter().map(|s| s.label.clone()).collect::<Vec<_>>().join(", ")
        )));
    }

    let checked = check(&declared, key, &kind, value)?;
    refuse_if_it_names_an_entity(db, key, value).await?;

    // The bag is replaced wholesale by `update_entity`, so read, set one
    // key, write back. Everything else in it carries forward untouched.
    let state = current_state(db, entity).await?.ok_or_else(|| {
        KernelError::Module(format!(
            "entity {} has no state chain",
            superx_ops::record_uuid(entity)
        ))
    })?;
    let mut bag = match state.attributes {
        Some(Value::Object(o)) => o,
        _ => Object::new(),
    };
    bag.insert(key.to_string(), checked);
    update_entity(db, entity, None, None, Some(Value::Object(bag))).await
}

/// Every field of an entity: what its type declares, what it holds, and
/// anything it holds that is no longer declared.
///
/// Reads NEVER fail. An entity written under an older set of
/// declarations still reads in full — the undeclared leftovers are
/// flagged, not hidden and not fatal.
///
/// # Errors
///
/// [`KernelError::Db`] for engine errors.
pub async fn of(db: &Db, entity: &RecordId) -> Result<Vec<Field>> {
    let (entity_type, _) = crate::nodes::anchor_info(db, entity).await?;
    let state = current_state(db, entity).await?;
    let bag = match state.and_then(|s| s.attributes) {
        Some(Value::Object(o)) => o,
        _ => Object::new(),
    };

    let mut out = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for slot in dictionary::slots_for(db, &entity_type, false).await? {
        let Some(declared) = dictionary::current(db, &slot.label, SLOT).await? else {
            continue;
        };
        let kind = declared.value_kind.unwrap_or_default();
        if !is_value_kind(&kind) {
            continue;
        }
        seen.push(slot.label.clone());
        out.push(Field {
            value: bag.get(&slot.label).map(render),
            key: slot.label,
            value_kind: kind,
            required: slot.required,
            undeclared: false,
        });
    }

    // What the entity holds that nothing declares. Silently dropping
    // these would make a removed declaration look like deleted data.
    for (key, value) in bag.iter() {
        if seen.iter().any(|k| k == key) {
            continue;
        }
        out.push(Field {
            key: key.clone(),
            value_kind: String::new(),
            required: false,
            value: Some(render(value)),
            undeclared: true,
        });
    }
    Ok(out)
}

/// Check a written value against the kind its label declares, returning
/// the typed value to store.
fn check(declared: &dictionary::LabelRow, key: &str, kind: &str, value: &str) -> Result<Value> {
    let bad = |why: &str| {
        KernelError::Module(format!("'{key}' is {kind}: {why}"))
    };

    match kind {
        "integer" => value
            .parse::<i64>()
            .map(|n| Value::Number(n.into()))
            .map_err(|_| bad("not a whole number")),
        "number" => match value.parse::<f64>() {
            // `"NaN"`, `"inf"` and `1e400` all parse. None of them survives a
            // JSON round trip, every comparison against them is false, and
            // arithmetic on one poisons whatever it touches — so a value the
            // engine cannot meaningfully hold is refused at the door.
            Ok(n) if n.is_finite() => Ok(Value::Number(n.into())),
            Ok(_) => Err(bad("not a finite number")),
            Err(_) => Err(bad("not a number")),
        },
        "boolean" => match value {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => Err(bad("write true or false — 'yes' is a word, not a boolean")),
        },
        "datetime" => chrono::DateTime::parse_from_rfc3339(value)
            .map(|t| Value::Datetime(t.with_timezone(&chrono::Utc).into()))
            .map_err(|_| bad("not an RFC 3339 timestamp (2026-08-23T20:00:00Z)")),
        "string" => {
            if value.trim().is_empty() {
                Err(bad("empty — clear the field rather than storing a blank"))
            } else {
                Ok(Value::String(value.to_string()))
            }
        }
        "url" => {
            if value.starts_with("http://") || value.starts_with("https://") {
                Ok(Value::String(value.to_string()))
            } else {
                Err(bad("not an http(s) url"))
            }
        }
        "enum" => {
            let allowed = enum_options(declared);
            if allowed.is_empty() {
                return Err(bad("declares no options, so nothing is valid"));
            }
            if allowed.iter().any(|o| o == value) {
                Ok(Value::String(value.to_string()))
            } else {
                Err(bad(&format!("not one of: {}", allowed.join(", "))))
            }
        }
        k if k == SECRET_KIND => {
            let source = value.split_once(':');
            match source {
                Some((s, rest)) if SECRET_SOURCES.contains(&s) && !rest.is_empty() => {
                    Ok(Value::String(value.to_string()))
                }
                _ => Err(KernelError::Module(format!(
                    "'{key}' holds a REFERENCE to a secret, never the secret — \
                     write {}:<name>. The value itself belongs in the tool that \
                     uses it, so that it never enters the graph and never leaves \
                     in an export.",
                    SECRET_SOURCES.join(":<name> | ")
                ))),
            }
        }
        other => Err(KernelError::Module(format!(
            "'{key}' declares unknown kind '{other}'"
        ))),
    }
}

/// The options an `enum` label allows, from its attributes bag.
fn enum_options(declared: &dictionary::LabelRow) -> Vec<String> {
    match declared.attributes.as_ref().and_then(|a| a.get("options")) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| match v {
                Value::String(s) => Some(s.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// A stored value as one line of text, for display.
fn render(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        // SurrealDB marks a float with a trailing `f`. That is the
        // engine's notation, not the operator's value: a max_notional of
        // 50000 should read as 50000, not `50000f`.
        Value::Number(n) => n.to_string().trim_end_matches('f').to_string(),
        Value::Datetime(d) => d.to_string(),
        // A nested value reaches here only from an UNDECLARED key — a
        // field's own kinds are all scalar. Rendering it as Rust debug
        // would show the operator `Array(Array([...]))` where their data
        // should be, so it goes back out as the JSON it came in as.
        other => crate::nodes::value_to_json(other).to_string(),
    }
}

/// Composition must never hide inside JSON: the graph cannot traverse a
/// field, so a link written as one is invisible to anything walking the
/// graph — which is every agent.
///
/// Matching the `entity:` prefix alone was not the rule it claimed to
/// be, because the obvious way to write the same mistake is to paste the
/// bare uuid. What settles it is whether the value IDENTIFIES SOMETHING
/// THAT EXISTS: if it does, it is a link, wherever it was typed.
async fn refuse_if_it_names_an_entity(db: &Db, key: &str, value: &str) -> Result<()> {
    let candidate = value.strip_prefix("entity:").unwrap_or(value);
    let Ok(uuid) = candidate.parse::<uuid::Uuid>() else {
        return Ok(());
    };
    let target = RecordId::new("entity", superx_kernel::types::Uuid::from(uuid));
    if crate::nodes::anchor_info(db, &target).await.is_ok() {
        return Err(KernelError::Module(format!(
            "'{key}' names an entity that exists — that is an EDGE, not a field; \
             link it so the graph can traverse it and an agent can follow it"
        )));
    }
    // A uuid that resolves to nothing is just a string. Refusing it would
    // stop a field holding another system's id, which is a real thing to
    // want.
    Ok(())
}
