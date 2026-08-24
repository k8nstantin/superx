//! Files, with the same discipline as prose (issue #296, spec §3).
//!
//! `note` and `attachment` are one idea in two shapes: **content that
//! belongs to something and is never a node.** One is inline text, the
//! other a file — chained, versioned, authored, retractable, and
//! carrying a LABEL.
//!
//! The label is the point. §5.4: *a mandate can be typed inline on one
//! role and uploaded as a PDF on another; the label carries the meaning,
//! the source carries the bytes.* A consumer asks for "the mandate of
//! this role" and gets content, never caring which it was.
//!
//! What this replaces: `entities attach` made a `document` ENTITY and
//! linked it, so a file became a node in the product graph an agent
//! walks — the same pollution prose was moved out of the graph to end.

use std::path::{Path, PathBuf};

use superx_kernel::types::{Object, Value};
use superx_kernel::{Db, KernelError, Result};

use crate::registry::new_id;
use crate::target::Target;

/// Where attachment bytes live inside the module's own directory.
const SUBDIR: &str = "attachments"; // skill-allow: §9-const — the module's own storage layout, not a tunable

/// One attached file, at one version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub uid: String,
    pub target_kind: String,
    pub target_uid: String,
    /// What this file MEANS — `spec`, `mandate`, `description`.
    pub label: String,
    pub filename: String,
    pub mime: String,
    pub size: i64,
    /// Relative to the module's directory, never absolute: an instance
    /// that moves must not carry a dead path with it.
    pub path: String,
    pub active: bool,
    pub valid_from: Option<chrono::DateTime<chrono::Utc>>,
    pub author_kind: Option<String>,
    pub author_uid: Option<String>,
    pub via_uid: Option<String>,
}

/// Store bytes and record what they are.
///
/// The label must be one the dictionary defines, for the same reason a
/// note's must: a file nobody can interpret is a file nobody reads.
///
/// # Errors
///
/// [`KernelError::Module`] for an undefined label or an unwritable
/// directory; [`KernelError::Db`] for engine errors.
pub async fn attach(
    db: &Db,
    module_dir: &Path,
    target: &Target,
    label: &str,
    source: &Path,
    author: &crate::notes::Author,
) -> Result<String> {
    let filename = source
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .ok_or_else(|| KernelError::Module(format!("{source:?} has no file name")))?;
    let bytes = std::fs::read(source)
        .map_err(|e| KernelError::Module(format!("read {source:?}: {e}")))?;
    attach_bytes(
        db,
        module_dir,
        Upload { target, label, filename: &filename, bytes: &bytes, author },
    )
    .await
}

/// One upload, as a value — the browser has bytes in hand rather than a
/// path, and the two paths must not drift apart.
pub struct Upload<'a> {
    pub target: &'a Target,
    /// What the file MEANS. A PDF labelled `mandate` IS the mandate.
    pub label: &'a str,
    pub filename: &'a str,
    pub bytes: &'a [u8],
    pub author: &'a crate::notes::Author,
}

/// Store bytes already in hand and record what they are.
///
/// # Errors
///
/// [`KernelError::Module`] for an undefined label or an unwritable
/// directory; [`KernelError::Db`] for engine errors.
pub async fn attach_bytes(db: &Db, module_dir: &Path, up: Upload<'_>) -> Result<String> {
    let Upload { target, label, filename, bytes, author } = up;

    if crate::dictionary::current(db, label, crate::dictionary::SLOT).await?.is_none() {
        return Err(KernelError::Module(format!(
            "the dictionary defines no slot label '{label}' — a file that means \
             nothing is a file nobody reads"
        )));
    }

    // The name is untrusted wherever it came from: keep only the final
    // component, so it can never climb out of the module's directory.
    let filename = Path::new(filename)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty() && *n != "." && *n != "..")
        .unwrap_or("attachment")
        .to_string();

    // One directory per attachment uid: two files of the same name on the
    // same target must not overwrite each other, and a version chain
    // keeps its own bytes.
    let uid = uuid::Uuid::now_v7().to_string();
    let relative = PathBuf::from(SUBDIR).join(&uid).join(&filename);
    let absolute = module_dir.join(&relative);
    if let Some(parent) = absolute.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| KernelError::Module(format!("create {parent:?}: {e}")))?;
    }
    std::fs::write(&absolute, bytes)
        .map_err(|e| KernelError::Module(format!("write {absolute:?}: {e}")))?;

    append(
        db,
        Version {
            uid: &uid,
            target,
            label,
            filename: &filename,
            mime: crate::documents::mime_for(&filename),
            size: i64::try_from(bytes.len()).unwrap_or(i64::MAX),
            path: &relative.to_string_lossy(),
            active: true,
            author,
        },
    )
    .await?;
    Ok(uid)
}

/// Everything attached to one target, oldest first.
///
/// # Errors
///
/// [`KernelError::Db`] for engine errors.
pub async fn for_target(
    db: &Db,
    target: &Target,
    include_retracted: bool,
) -> Result<Vec<Attachment>> {
    let mut resp = db
        .query(
            "SELECT * FROM attachment WHERE target_uid = $uid AND target_kind = $kind \
             ORDER BY valid_from ASC, id ASC",
        )
        .bind(("uid", target.uid()))
        .bind(("kind", target.kind().to_string()))
        .await?;
    let rows: Vec<Value> = resp.take(0)?;

    // Latest row per uid wins — the chain read, as everywhere else.
    let mut heads: std::collections::BTreeMap<String, Attachment> =
        std::collections::BTreeMap::new();
    for row in &rows {
        if let Some(a) = parse(row) {
            heads.insert(a.uid.clone(), a);
        }
    }
    Ok(heads
        .into_values()
        .filter(|a| include_retracted || a.active)
        .collect())
}

/// The current version of one attachment.
///
/// # Errors
///
/// [`KernelError::Db`] for engine errors.
pub async fn current(db: &Db, uid: &str) -> Result<Option<Attachment>> {
    let mut resp = db
        .query(
            "SELECT * FROM attachment WHERE uid = $uid ORDER BY valid_from DESC, id DESC LIMIT 1",
        )
        .bind(("uid", uid.to_string()))
        .await?;
    let rows: Vec<Value> = resp.take(0)?;
    Ok(rows.first().and_then(parse))
}

/// Withdraw an attachment. The bytes stay on disk and the row stays in
/// the chain: a retraction says *this no longer stands*, never *this was
/// never here*.
///
/// # Errors
///
/// [`KernelError::Module`] if there is no such attachment;
/// [`KernelError::Db`] for engine errors.
pub async fn retract(db: &Db, uid: &str, author: &crate::notes::Author) -> Result<()> {
    let Some(a) = current(db, uid).await? else {
        return Err(KernelError::Module(format!("no attachment '{uid}'")));
    };
    let target = Target::resolve(db, &a.target_kind, &a.target_uid).await?;
    append(
        db,
        Version {
            uid,
            target: &target,
            label: &a.label,
            filename: &a.filename,
            mime: &a.mime,
            size: a.size,
            path: &a.path,
            active: false,
            author,
        },
    )
    .await
}

/// Where an attachment's bytes actually are.
#[must_use]
pub fn absolute_path(module_dir: &Path, a: &Attachment) -> PathBuf {
    module_dir.join(&a.path)
}

struct Version<'a> {
    uid: &'a str,
    target: &'a Target,
    label: &'a str,
    filename: &'a str,
    mime: &'a str,
    size: i64,
    path: &'a str,
    active: bool,
    author: &'a crate::notes::Author,
}

async fn append(db: &Db, v: Version<'_>) -> Result<()> {
    let Version { uid, target, label, filename, mime, size, path, active, author } = v;
    let mut row = Object::new();
    row.insert("uid".to_string(), Value::String(uid.to_string()));
    row.insert("target_kind".to_string(), Value::String(target.kind().to_string()));
    row.insert("target_uid".to_string(), Value::String(target.uid()));
    row.insert("label".to_string(), Value::String(label.to_string()));
    row.insert("filename".to_string(), Value::String(filename.to_string()));
    row.insert("mime".to_string(), Value::String(mime.to_string()));
    row.insert("size".to_string(), Value::Number(size.into()));
    row.insert("path".to_string(), Value::String(path.to_string()));
    row.insert("active".to_string(), Value::Bool(active));
    row.insert("valid_from".to_string(), Value::Datetime(chrono::Utc::now().into()));
    row.insert("author_kind".to_string(), Value::String(author.kind.clone()));
    if let Some(who) = &author.uid {
        row.insert("author_uid".to_string(), Value::String(who.clone()));
    }
    if let Some(via) = &author.via {
        row.insert("via_uid".to_string(), Value::String(via.clone()));
    }
    db.query("CREATE $id CONTENT $row")
        .bind(("id", new_id("attachment")))
        .bind(("row", Value::Object(row)))
        .await?
        .check()?;
    Ok(())
}

fn parse(row: &Value) -> Option<Attachment> {
    let Value::Object(o) = row else { return None };
    Some(Attachment {
        uid: str_field(o, "uid")?,
        target_kind: str_field(o, "target_kind")?,
        target_uid: str_field(o, "target_uid")?,
        label: str_field(o, "label")?,
        filename: str_field(o, "filename").unwrap_or_default(),
        mime: str_field(o, "mime").unwrap_or_default(),
        size: match o.get("size") {
            Some(Value::Number(n)) => n.to_int().unwrap_or(0),
            _ => 0,
        },
        path: str_field(o, "path").unwrap_or_default(),
        active: !matches!(o.get("active"), Some(Value::Bool(false))),
        valid_from: match o.get("valid_from") {
            Some(Value::Datetime(d)) => Some(**d),
            _ => None,
        },
        author_kind: str_field(o, "author_kind"),
        author_uid: str_field(o, "author_uid"),
        via_uid: str_field(o, "via_uid"),
    })
}

fn str_field(o: &Object, key: &str) -> Option<String> {
    match o.get(key) {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}
