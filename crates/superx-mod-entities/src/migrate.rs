//! Moving prose out of the graph (issue #276).
//!
//! Every description and comment written before #268 is a `text`
//! ENTITY hung off its target by a role edge. The note store is where
//! prose lives now, and until the old ones move, every reader has to
//! understand both.
//!
//! **The evolution moves with it.** One description in this instance has
//! eleven versions; migrating only the current wording would keep the
//! text and throw away the history of how it got there, which is the
//! part that says what anyone was thinking. So each version of a text
//! node becomes a version of the note, carrying its ORIGINAL
//! `valid_from` — the chain reads exactly as it always did.
//!
//! **Nothing is deleted.** The text nodes and their edges stay where
//! they are, still readable, still exported. This adds; a later PR stops
//! reading them, and only after that does anything retire.
//!
//! Idempotent by construction: each migrated version records where it
//! came from, so running it twice moves nothing the second time.

use std::collections::HashSet;

use superx_kernel::types::{Object, RecordId, Value};
use superx_kernel::{Db, Result};
use superx_ops::record_uuid;

use crate::registry::new_id;
use crate::{edges, nodes, texts};

/// What a migration did, or would do.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    /// Text nodes carrying prose, reached by a role edge.
    pub carriers: usize,
    /// Versions written (or that would be).
    pub versions: usize,
    /// Versions already migrated by an earlier run.
    pub already: usize,
    /// Text nodes no active prose edge points at — nothing claims them,
    /// so nothing is guessed about where they belong.
    pub orphans: Vec<String>,
    /// Text nodes reached by an edge that is not a prose role, chiefly
    /// the runner's `produced`. Out of scope here and reported rather
    /// than silently skipped.
    pub other_roles: Vec<String>,
    /// Carriers written while both stores were live, whose prose is
    /// already a note. Moving them would duplicate it.
    pub dual_written: usize,
}

/// Move every text carrier into the note store, history and all.
///
/// With `dry_run` the same walk happens and nothing is written, so the
/// report can be read before anything moves.
///
/// # Errors
///
/// [`KernelError::Db`](superx_kernel::KernelError::Db) for engine errors.
pub async fn prose(db: &Db, dry_run: bool) -> Result<Report> {
    let mut report = Report::default();

    for carrier in nodes::list_entities(db, Some("text")).await? {
        // Who points at this text node, and calling it what?
        let inbound = edges::expand(db, std::slice::from_ref(&carrier.id), true).await?;
        let prose_edge = inbound
            .iter()
            .find(|e| e.active && texts::TEXT_ROLES.contains(&e.rel_type.as_str()));

        let Some(edge) = prose_edge else {
            if inbound.iter().any(|e| e.active) {
                report.other_roles.push(record_uuid(&carrier.id));
            } else {
                report.orphans.push(record_uuid(&carrier.id));
            }
            continue;
        };

        let label = texts::label_for_role(&edge.rel_type)?;
        let owner = edge.from.clone();
        let source = record_uuid(&carrier.id);
        let history = nodes::state_history(db, &carrier.id).await?;

        // A carrier CREATED while both stores were live already has all
        // its prose in the note store, and it says so. Moving it again
        // would show the same comment twice, and a plural label has no
        // cardinality rule to collapse the copies back together.
        if names_a_live_note(db, history.last()).await? {
            report.dual_written += history.len();
            continue;
        }

        report.carriers += 1;

        // A SINGULAR slot written since #268 already has a note, and this
        // carrier's older wordings are earlier versions of that same
        // thing — they belong on its chain, not beside it as a second
        // live description with no rule for which one counts.
        //
        // A PLURAL slot is the opposite: every comment is its own note.
        // Joining them by label would collapse every comment an entity
        // has ever had into one chain, so each carrier keeps its own uid.
        let singular = crate::dictionary::current(db, label, crate::dictionary::SLOT)
            .await?
            .and_then(|d| d.cardinality)
            .as_deref()
            == Some("one");
        let uid = match singular {
            true => existing_uid(db, &owner, label).await?,
            false => None,
        }
        .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
        let done = migrated_versions(db, &owner).await?;

        for version in history {
            if done.contains(&(source.clone(), version.valid_from.clone())) {
                report.already += 1;
                continue;
            }
            let body = version.content.clone().unwrap_or_else(|| version.name.clone());
            // The dual-write may have APPENDED to a legacy carrier rather
            // than creating one: then its latest wording is already a
            // note while the earlier wordings are not. Skipping the whole
            // carrier would lose exactly the history this migration
            // exists to keep, so the question is asked per version.
            if recorded_by_the_dual_write(db, &owner, label, &body).await? {
                report.dual_written += 1;
                continue;
            }
            report.versions += 1;
            if dry_run {
                continue;
            }
            write_version(db, &owner, &uid, label, &body, &source, &version.valid_from).await?;
        }
    }
    Ok(report)
}

/// The uid of the note already carrying this label on this entity, if
/// the dual-write has made one.
async fn existing_uid(db: &Db, owner: &RecordId, label: &str) -> Result<Option<String>> {
    Ok(crate::notes::for_entity(db, owner, true)
        .await?
        .into_iter()
        .find(|n| n.label == label)
        .map(|n| n.uid))
}

/// Every (source, version) pair already moved onto this entity, so a
/// second run moves nothing.
async fn migrated_versions(db: &Db, owner: &RecordId) -> Result<HashSet<(String, String)>> {
    let mut resp = db
        .query("SELECT attributes FROM note WHERE entity = $entity")
        .bind(("entity", owner.clone()))
        .await?;
    let rows: Vec<Value> = resp.take(0)?;
    let mut out = HashSet::new();
    for row in &rows {
        let Value::Object(o) = row else { continue };
        let Some(Value::Object(attrs)) = o.get("attributes") else { continue };
        if let (Some(Value::String(from)), Some(Value::String(at))) =
            (attrs.get("migrated_from"), attrs.get("migrated_version"))
        {
            out.insert((from.clone(), at.clone()));
        }
    }
    Ok(out)
}

/// Append one historical version, carrying its ORIGINAL timestamp.
///
/// This does not go through `notes::write`: that stamps `time::now()`,
/// which is right for a new note and wrong for one that was written
/// months ago. Recording the migration time instead of the writing time
/// would flatten eleven versions into one instant and lose the order
/// they happened in.
async fn write_version(
    db: &Db,
    owner: &RecordId,
    uid: &str,
    label: &str,
    body: &str,
    source: &str,
    valid_from: &str,
) -> Result<()> {
    let mut provenance = Object::new();
    provenance.insert("migrated_from".to_string(), Value::String(source.to_string()));
    provenance.insert(
        "migrated_version".to_string(),
        Value::String(valid_from.to_string()),
    );

    let mut row = Object::new();
    row.insert("uid".to_string(), Value::String(uid.to_string()));
    row.insert("entity".to_string(), Value::RecordId(owner.clone()));
    row.insert("label".to_string(), Value::String(label.to_string()));
    row.insert("body".to_string(), Value::String(body.to_string()));
    row.insert("active".to_string(), Value::Bool(true));
    row.insert("attributes".to_string(), Value::Object(provenance));
    // The author is genuinely unknown: text nodes predate authorship.
    // Claiming the operator wrote them would be a fabrication in exactly
    // the column that exists to be trusted.
    row.insert("author_kind".to_string(), Value::String("system".to_string()));

    // Parsed, not passed through: an unreadable timestamp must stop the
    // migration rather than quietly become `now`, which would flatten a
    // chain of eleven versions into one instant.
    let written_at = chrono::DateTime::parse_from_rfc3339(valid_from).map_err(|e| {
        superx_kernel::KernelError::Module(format!(
            "text node {source} has a version timestamped '{valid_from}', which is not \
             a readable time ({e}) — refusing to move it under today's date"
        ))
    })?;
    row.insert(
        "valid_from".to_string(),
        Value::Datetime(written_at.with_timezone(&chrono::Utc).into()),
    );

    db.query("CREATE $id CONTENT $row")
        .bind(("id", new_id("note")))
        .bind(("row", Value::Object(row)))
        .await?
        .check()?;
    Ok(())
}

/// Does this carrier NAME the note it was written alongside?
///
/// Every carrier written from now on does. It is exact: the carrier was
/// created by the dual-write, so all of its prose is already a note and
/// none of it should move.
async fn names_a_live_note(db: &Db, current: Option<&nodes::StateRow>) -> Result<bool> {
    let Some(Value::Object(attrs)) = current.and_then(|s| s.attributes.as_ref()) else {
        return Ok(false);
    };
    let Some(Value::String(uid)) = attrs.get("note_uid") else {
        return Ok(false);
    };
    Ok(crate::notes::current(db, uid).await?.is_some())
}

/// Was this exact wording already recorded as a note by the dual-write?
///
/// For the window between the dual-write shipping and carriers naming
/// their note, the only evidence is the prose itself: an entity holding a
/// note with this label, this body, and no migration provenance was
/// written twice, and moving it again would show it twice.
///
/// A note that DOES carry provenance is not evidence of a dual-write — it
/// is a version this migration already moved, which the per-version
/// source check handles exactly.
async fn recorded_by_the_dual_write(
    db: &Db,
    owner: &RecordId,
    label: &str,
    body: &str,
) -> Result<bool> {
    let mut resp = db
        .query("SELECT body, attributes FROM note WHERE entity = $entity AND label = $label")
        .bind(("entity", owner.clone()))
        .bind(("label", label.to_string()))
        .await?;
    let rows: Vec<Value> = resp.take(0)?;
    for row in &rows {
        let Value::Object(o) = row else { continue };
        let Some(Value::String(existing)) = o.get("body") else { continue };
        if existing != body {
            continue;
        }
        let from_a_migration = matches!(
            o.get("attributes"),
            Some(Value::Object(a)) if a.contains_key("migrated_from")
        );
        if !from_a_migration {
            return Ok(true);
        }
    }
    Ok(false)
}
