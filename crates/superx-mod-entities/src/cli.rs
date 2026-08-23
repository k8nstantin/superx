//! `superx entities …` — the module's own CLI, dispatched by the
//! kernel's external_subcommand router. E1: type registry. E2: node
//! verbs. Graph edges/traversal land in E3–E4.

use superx_kernel::types::Value;
use superx_kernel::{Kernel, KernelError, NodeKind, Result};
use superx_ops::record_uuid;

use crate::{documents, edges, graph, nodes, notes, registry, texts, MODULE_NAME};

/// Traversal depth ceiling parameter on the module's registry entity.
pub const MAX_DEPTH_PARAM: &str = "attr_entities_max_depth";
const DEFAULT_MAX_DEPTH: usize = 5; // skill-allow: §9-const — bootstrap fallback, param-overridable (attr_entities_max_depth)
/// Depth ceiling for the breadcrumb walk (#253).
const ANCESTOR_MAX_DEPTH: usize = 12; // skill-allow: §9-const — render-layer bound, not a policy tunable

const USAGE: &str = "usage: superx entities <command>\n\
  create --type <type> [--describe <text>] [--content <text>] [--attrs <json>] <name…>\n\
  update <uuid-fragment> [--name <name>] [--content <text>] [--attrs <json>]\n\
         (--attrs REPLACES the whole attributes object; omit to keep it)\n\
  show <uuid-fragment> [--history]\n\
  list [--type <type>]\n\
  link <from-fragment> <to-fragment> --rel <relation-type>\n\
  unlink <from-fragment> <to-fragment> --rel <relation-type>\n\
  describe <uuid-fragment> <text…>     set/evolve the describing text node\n\
  instruct <uuid-fragment> <text…>     set/evolve the instructing text node\n\
  comment <uuid-fragment> <text…>      add a comment text node (threads: comment a comment)\n\
  attach <uuid-fragment> <file-path>   copy a file in; document node + attached edge\n\
  tree <uuid-fragment> [--depth <n>] [--reverse]\n\
  graph <uuid-fragment> [--json] [--depth <n>]   export the reachable subgraph\n\
  types                                list the type registry\n\
  types add <name> --category entity|relation [--description <text>]\n\
  labels [--all] [--for <type>]        the dictionary: what the terminology means\n\
  labels define <key> --kind slot|link --semantics <s> [--display <d>] [--description <text>]\n\
  labels history <key> --kind slot|link      every version of one label, oldest first\n\
  labels archive <key> --kind slot|link [--restore]\n\
  notes <uuid-fragment> [--all]        the prose attached to an entity, by label\n\
  notes history <note-uid>             every version of one note\n\
  notes reply <note-uid> <text…>       a comment on a comment\n\
  notes retract <note-uid>             it no longer stands; it stays on the record\n\
  url                                  where this module's own UI lives\n\
each write emits telemetry into the kernel firehose";

/// Route a `superx entities …` invocation.
///
/// # Errors
///
/// [`KernelError::Module`] with usage on unknown commands; verb
/// errors pass through.
pub async fn dispatch(kernel: &Kernel, args: &[String]) -> Result<String> {
    match args.first().map(String::as_str) {
        Some("types") => types_cmd(kernel, &args[1..]).await,
        Some("labels") => labels_cmd(kernel, &args[1..]).await,
        Some("notes") => notes_cmd(kernel, &args[1..]).await,
        Some("create") => create_cmd(kernel, &args[1..]).await,
        Some("update") => update_cmd(kernel, &args[1..]).await,
        Some("show") => show_cmd(kernel, &args[1..]).await,
        Some("list") => list_cmd(kernel, &args[1..]).await,
        Some("link") => link_cmd(kernel, &args[1..], true).await,
        Some("unlink") => link_cmd(kernel, &args[1..], false).await,
        Some("describe") => role_text_cmd(kernel, &args[1..], "describes").await,
        Some("instruct") => role_text_cmd(kernel, &args[1..], "instructs").await,
        Some("comment") => comment_cmd(kernel, &args[1..]).await,
        Some("attach") => attach_cmd(kernel, &args[1..]).await,
        Some("tree") => tree_cmd(kernel, &args[1..]).await,
        Some("graph") => graph_cmd(kernel, &args[1..]).await,
        Some("url") => Ok(format!("{}\n", crate::resolved_ui_url(kernel).await)),
        _ => Err(KernelError::Module(USAGE.to_string())),
    }
}

/// `superx entities labels` — the dictionary (#266).
///
/// Reading it is a precondition for interpreting anything else: types
/// say what a thing is, labels say what the terminology means. A
/// product carrying both a `description` and a `spec` is the case that
/// makes it matter — same kind, same storage, and the label is the
/// entire difference.
async fn labels_cmd(kernel: &Kernel, args: &[String]) -> Result<String> {
    let db = kernel.module_db(MODULE_NAME).await?;
    if args.first().map(String::as_str) == Some("define") {
        let key = args.get(1).ok_or_else(usage)?;
        let kind = flag(args, "--kind").ok_or_else(usage)?;
        let semantics = flag(args, "--semantics").ok_or_else(usage)?;
        let display = flag(args, "--display").unwrap_or_else(|| key.clone());
        let description = flag(args, "--description");
        crate::dictionary::define(&db, key, &kind, &display, &semantics, description.as_deref())
            .await?;
        let revision = crate::dictionary::revision(&db).await?;
        return Ok(format!(
            "defined {kind} label '{key}' ({semantics}) — dictionary revision {revision}\n"
        ));
    }

    if args.first().map(String::as_str) == Some("history") {
        let key = args.get(1).ok_or_else(usage)?;
        let kind = flag(args, "--kind").ok_or_else(usage)?;
        let versions = crate::dictionary::history(&db, key, &kind).await?;
        if versions.is_empty() {
            return Err(KernelError::Module(format!(
                "the dictionary has no {kind} label '{key}'"
            )));
        }
        let mut out = format!("{kind} label '{key}' — {} version(s)\n", versions.len());
        for (n, v) in versions.iter().enumerate() {
            let when = v
                .valid_from
                .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_else(|| "-".to_string());
            let archived = if v.archived { "  [archived]" } else { "" };
            out.push_str(&format!(
                "\n  v{}  {}  {}{}\n      {}\n",
                n + 1,
                when,
                v.semantics,
                archived,
                v.description.as_deref().unwrap_or("")
            ));
        }
        out.push_str("\nthe oldest version is what the entities written under it meant\n");
        return Ok(out);
    }

    if args.first().map(String::as_str) == Some("archive") {
        let key = args.get(1).ok_or_else(usage)?;
        let kind = flag(args, "--kind").ok_or_else(usage)?;
        let restore = args.iter().any(|a| a == "--restore");
        crate::dictionary::archive(&db, key, &kind, !restore).await?;
        let revision = crate::dictionary::revision(&db).await?;
        let verb = if restore { "restored" } else { "archived" };
        return Ok(format!(
            "{verb} {kind} label '{key}' — dictionary revision {revision}\n\
             nothing was deleted; every version is still readable via labels history\n"
        ));
    }

    if let Some(entity_type) = flag(args, "--for") {
        let slots = crate::dictionary::slots_for(&db, &entity_type, false).await?;
        if slots.is_empty() {
            return Ok(format!(
                "type '{entity_type}' declares no slots — nothing can be attached to \
                 one of its entities, so nothing can act on it\n"
            ));
        }
        let mut out = format!("type '{entity_type}' carries {} slot(s)\n\n", slots.len());
        for slot in &slots {
            let defined = crate::dictionary::current(&db, &slot.label, crate::dictionary::SLOT)
                .await?;
            let semantics = defined
                .as_ref()
                .map_or("?".to_string(), |d| d.semantics.clone());
            let card = defined
                .as_ref()
                .and_then(|d| d.cardinality.clone())
                .unwrap_or_else(|| "-".to_string());
            out.push_str(&format!(
                "  {:<14} {:<11} {:<5} {}\n",
                slot.label,
                semantics,
                card,
                if slot.required { "required" } else { "" }
            ));
        }
        return Ok(out);
    }

    let include_archived = args.iter().any(|a| a == "--all");
    let labels = crate::dictionary::list(&db, include_archived).await?;
    let revision = crate::dictionary::revision(&db).await?;
    let mut out = format!("dictionary — revision {revision}\n");
    for kind in [crate::dictionary::SLOT, crate::dictionary::LINK] {
        let of_kind: Vec<_> = labels.iter().filter(|l| l.label_kind == kind).collect();
        if of_kind.is_empty() {
            continue;
        }
        out.push_str(&format!("\n{kind} labels\n"));
        for l in of_kind {
            let card = l.cardinality.as_deref().unwrap_or("-");
            let archived = if l.archived { "  [archived]" } else { "" };
            out.push_str(&format!(
                "  {:<14} {:<11} {:<5} {}{}\n",
                l.key,
                l.semantics,
                card,
                l.description.as_deref().unwrap_or(""),
                archived
            ));
        }
    }
    out.push_str("\nsemantics decide how an agent must TREAT a label, not just its shape\n");
    Ok(out)
}

/// `superx entities notes` — the prose store (#268).
///
/// A description, a spec, a mandate and a comment are the same
/// mechanism with a different LABEL. This reads the note store
/// directly; the detail page still reads the legacy carrier until the
/// readers are switched over in their own PR.
async fn notes_cmd(kernel: &Kernel, args: &[String]) -> Result<String> {
    let db = kernel.module_db(MODULE_NAME).await?;
    match args.first().map(String::as_str) {
        Some("history") => {
            let uid = args.get(1).ok_or_else(usage)?;
            let versions = notes::history(&db, uid).await?;
            if versions.is_empty() {
                return Err(KernelError::Module(format!("no note '{uid}'")));
            }
            let mut out = format!("note {uid} — {} version(s)\n", versions.len());
            for (n, v) in versions.iter().enumerate() {
                out.push_str(&format!(
                    "\n  v{}  {}  {}{}\n      {}\n",
                    n + 1,
                    stamp(v),
                    v.label,
                    if v.active { "" } else { "  [retracted]" },
                    v.body
                ));
            }
            Ok(out)
        }
        Some("reply") => {
            let parent = args.get(1).ok_or_else(usage)?;
            let text = rest(args, 2)?;
            let uid = notes::reply(&db, parent, &text, &notes::Author::operator()).await?;
            Ok(format!("replied to {parent} — note {uid}\n"))
        }
        Some("retract") => {
            let uid = args.get(1).ok_or_else(usage)?;
            notes::retract(&db, uid, &notes::Author::operator()).await?;
            Ok(format!(
                "retracted note {uid} — it no longer stands, and every version is \
                 still readable via notes history\n"
            ))
        }
        _ => {
            let fragment = args.first().ok_or_else(usage)?;
            let include_retracted = args.iter().any(|a| a == "--all");
            let anchor = nodes::resolve_entity(&db, fragment).await?;
            let all = notes::for_entity(&db, &anchor, include_retracted).await?;
            if all.is_empty() {
                return Ok("no notes on this entity yet\n".to_string());
            }
            let mut out = format!("{} note(s)\n", all.len());
            // Threads render under their parent; a reply whose parent is
            // filtered out still shows, because losing a reply because
            // someone retracted the thing it answered would lose the
            // answer too.
            let shown: std::collections::HashSet<&str> =
                all.iter().map(|n| n.uid.as_str()).collect();
            for note in all.iter().filter(|n| {
                n.parent_uid.as_deref().is_none_or(|p| !shown.contains(p))
            }) {
                render_note(&mut out, note, &all, 0);
            }
            Ok(out)
        }
    }
}

/// One note and its replies, indented by depth.
fn render_note(out: &mut String, note: &notes::Note, all: &[notes::Note], depth: usize) {
    let pad = "  ".repeat(depth + 1);
    let who = note.author_kind.as_deref().unwrap_or("?");
    let via = note
        .via_uid
        .as_deref()
        .map(|r| format!(" as {r}"))
        .unwrap_or_default();
    out.push_str(&format!(
        "\n{pad}{}  {}  {who}{via}{}\n{pad}  {}\n{pad}  uid {}\n",
        note.label,
        stamp(note),
        if note.active { "" } else { "  [retracted]" },
        note.body,
        note.uid
    ));
    for reply in all.iter().filter(|n| n.parent_uid.as_deref() == Some(note.uid.as_str())) {
        render_note(out, reply, all, depth + 1);
    }
}

fn stamp(note: &notes::Note) -> String {
    note.valid_from
        .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| "-".to_string())
}

/// The remaining args joined as one text payload.
fn rest(args: &[String], from: usize) -> Result<String> {
    let text = args[from.min(args.len())..].join(" ");
    if text.trim().is_empty() {
        return Err(usage());
    }
    Ok(text)
}

async fn create_cmd(kernel: &Kernel, args: &[String]) -> Result<String> {
    let db = kernel.module_db(MODULE_NAME).await?;
    let mut entity_type = None;
    let mut describe = None;
    let mut content = None;
    let mut attrs = None;
    let mut name_words: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--type" => {
                entity_type = Some(next_value(args, i)?);
                i += 2;
            }
            "--describe" => {
                describe = Some(next_value(args, i)?);
                i += 2;
            }
            "--content" => {
                content = Some(next_value(args, i)?);
                i += 2;
            }
            "--attrs" => {
                attrs = Some(next_value(args, i)?);
                i += 2;
            }
            word => {
                name_words.push(word.to_string());
                i += 1;
            }
        }
    }
    let entity_type = entity_type.ok_or_else(usage)?;
    if name_words.is_empty() {
        return Err(usage());
    }
    let name = name_words.join(" ");
    let attributes = attrs.as_deref().map(parse_attrs).transpose()?;

    let anchor = nodes::create_entity(&db, &entity_type, &name, content, attributes).await?;
    let uuid = record_uuid(&anchor);
    emit(kernel, "entity_created", &uuid, &entity_type, &name).await;
    if let Some(text) = describe {
        let (text_node, _) = texts::set_role_text(&db, &anchor, "describes", &text).await?;
        emit(kernel, "entity_created", &record_uuid(&text_node), "text", "describes-text").await;
        emit_link(kernel, &uuid, &record_uuid(&text_node), "describes", "entities_linked").await;
    }
    Ok(format!("{uuid}\n"))
}

async fn update_cmd(kernel: &Kernel, args: &[String]) -> Result<String> {
    let db = kernel.module_db(MODULE_NAME).await?;
    let fragment = args.first().ok_or_else(usage)?.clone();
    let mut name = None;
    let mut content = None;
    let mut attrs = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--name" => {
                name = Some(next_value(args, i)?);
                i += 2;
            }
            "--content" => {
                content = Some(next_value(args, i)?);
                i += 2;
            }
            "--attrs" => {
                attrs = Some(next_value(args, i)?);
                i += 2;
            }
            _ => return Err(usage()),
        }
    }
    if name.is_none() && content.is_none() && attrs.is_none() {
        return Err(KernelError::Module(
            "nothing to update — give --name, --content, or --attrs".to_string(),
        ));
    }
    let attributes = attrs.as_deref().map(parse_attrs).transpose()?;

    let anchor = nodes::resolve_entity(&db, &fragment).await?;
    nodes::update_entity(&db, &anchor, name, content, attributes).await?;
    let (entity_type, _) = nodes::anchor_info(&db, &anchor).await?;
    let current = nodes::current_state(&db, &anchor).await?;
    let label = current.map(|s| s.name).unwrap_or_default();
    let uuid = record_uuid(&anchor);
    emit(kernel, "entity_updated", &uuid, &entity_type, &label).await;
    Ok(format!("{uuid} updated (new version appended)\n"))
}

async fn show_cmd(kernel: &Kernel, args: &[String]) -> Result<String> {
    let db = kernel.module_db(MODULE_NAME).await?;
    let fragment = args.first().ok_or_else(usage)?;
    let history = args.iter().any(|a| a == "--history");

    let anchor = nodes::resolve_entity(&db, fragment).await?;
    let (entity_type, created_at) = nodes::anchor_info(&db, &anchor).await?;
    let mut out = format!(
        "entity {} · type {entity_type} · created {created_at}\n",
        record_uuid(&anchor)
    );
    // The breadcrumb trail, CLI edition (#253) — same walk the
    // dashboards render.
    let trail = graph::ancestors(&db, &anchor, ANCESTOR_MAX_DEPTH).await?;
    if !trail.is_empty() {
        let names: Vec<String> = trail
            .iter()
            .map(|a| format!("{} ({})", a.name, a.entity_type))
            .collect();
        out.push_str(&format!("path: {} › this\n", names.join(" › ")));
    }
    if history {
        let versions = nodes::state_history(&db, &anchor).await?;
        out.push_str(&format!("history ({} versions, oldest first):\n", versions.len()));
        for (n, v) in versions.iter().enumerate() {
            out.push_str(&format!("  v{} · {}\n", n + 1, v.valid_from));
            out.push_str(&render_state(v, "    "));
        }
    } else if let Some(current) = nodes::current_state(&db, &anchor).await? {
        out.push_str(&render_state(&current, "  "));
    }
    let notes = texts::annotations(&db, &anchor).await?;
    if !notes.is_empty() {
        out.push_str("texts:\n");
        for note in notes {
            out.push_str(&format!(
                "  [{}] ({}) {}\n",
                note.rel_type,
                record_uuid(&note.text_id),
                note.content
            ));
        }
    }
    Ok(out)
}

async fn list_cmd(kernel: &Kernel, args: &[String]) -> Result<String> {
    let db = kernel.module_db(MODULE_NAME).await?;
    let type_filter = match args.first().map(String::as_str) {
        Some("--type") => Some(args.get(1).ok_or_else(usage)?.clone()),
        Some(_) => return Err(usage()),
        None => None,
    };
    let rows = nodes::list_entities(&db, type_filter.as_deref()).await?;
    if rows.is_empty() {
        return Ok("no entities yet — create one with `superx entities create`\n".to_string());
    }
    let mut out = format!("entities ({}):\n", rows.len());
    for row in rows {
        out.push_str(&format!(
            "  {}  {:<10} {}\n",
            record_uuid(&row.id),
            row.entity_type,
            row.name
        ));
    }
    Ok(out)
}

async fn link_cmd(kernel: &Kernel, args: &[String], create: bool) -> Result<String> {
    let db = kernel.module_db(MODULE_NAME).await?;
    let (from_frag, to_frag) = match (args.first(), args.get(1)) {
        (Some(f), Some(t)) => (f.clone(), t.clone()),
        _ => return Err(usage()),
    };
    let rel = match (args.get(2).map(String::as_str), args.get(3)) {
        (Some("--rel"), Some(r)) => r.clone(),
        _ => return Err(usage()),
    };
    let from = nodes::resolve_entity(&db, &from_frag).await?;
    let to = nodes::resolve_entity(&db, &to_frag).await?;
    let (from_uuid, to_uuid) = (record_uuid(&from), record_uuid(&to));
    if create {
        edges::link(&db, &from, &to, &rel).await?;
        emit_link(kernel, &from_uuid, &to_uuid, &rel, "entities_linked").await;
        Ok(format!("{from_uuid} -[{rel}]-> {to_uuid}\n"))
    } else {
        edges::unlink(&db, &from, &to, &rel).await?;
        emit_link(kernel, &from_uuid, &to_uuid, &rel, "entities_unlinked").await;
        Ok(format!("{from_uuid} -[{rel}]-x {to_uuid} (retracted; history kept)\n"))
    }
}

async fn role_text_cmd(kernel: &Kernel, args: &[String], role: &str) -> Result<String> {
    let db = kernel.module_db(MODULE_NAME).await?;
    let fragment = args.first().ok_or_else(usage)?;
    let text = args[1..].join(" ");
    if text.is_empty() {
        return Err(usage());
    }
    let target = nodes::resolve_entity(&db, fragment).await?;
    let (node, created) = texts::set_role_text(&db, &target, role, &text).await?;
    let node_uuid = record_uuid(&node);
    if created {
        emit(kernel, "entity_created", &node_uuid, "text", "").await;
        emit_link(kernel, &record_uuid(&target), &node_uuid, role, "entities_linked").await;
        Ok(format!("{role} text created: {node_uuid}\n"))
    } else {
        emit(kernel, "entity_updated", &node_uuid, "text", "").await;
        Ok(format!("{role} text evolved (new version): {node_uuid}\n"))
    }
}

async fn comment_cmd(kernel: &Kernel, args: &[String]) -> Result<String> {
    let db = kernel.module_db(MODULE_NAME).await?;
    let fragment = args.first().ok_or_else(usage)?;
    let text = args[1..].join(" ");
    if text.is_empty() {
        return Err(usage());
    }
    let target = nodes::resolve_entity(&db, fragment).await?;
    let node = texts::add_comment(&db, &target, &text).await?;
    let node_uuid = record_uuid(&node);
    emit(kernel, "entity_created", &node_uuid, "text", "comment").await;
    emit_link(kernel, &record_uuid(&target), &node_uuid, "comments", "entities_linked").await;
    Ok(format!("comment added: {node_uuid}\n"))
}

async fn attach_cmd(kernel: &Kernel, args: &[String]) -> Result<String> {
    let db = kernel.module_db(MODULE_NAME).await?;
    let (fragment, path) = match (args.first(), args.get(1)) {
        (Some(f), Some(p)) => (f, std::path::PathBuf::from(p)),
        _ => return Err(usage()),
    };
    let owner = nodes::resolve_entity(&db, fragment).await?;
    let size = std::fs::metadata(&path)
        .map_err(|e| KernelError::Module(format!("cannot read {}: {e}", path.display())))?
        .len();
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("attachment")
        .to_string();

    // The file lands in the module's own dir, keyed by a fresh uuid7
    // (the historical-log convention extends to stored blobs).
    let files_dir = kernel.module_dir(MODULE_NAME)?.join("files");
    std::fs::create_dir_all(&files_dir)
        .map_err(|e| KernelError::Module(format!("cannot create files dir: {e}")))?;
    let stored_name = format!("{}-{file_name}", uuid::Uuid::now_v7());
    let stored = files_dir.join(&stored_name);
    // Streamed copy — attachments never transit memory whole (#179).
    std::fs::copy(&path, &stored)
        .map_err(|e| KernelError::Module(format!("cannot store file: {e}")))?;

    let mime = documents::mime_for(&file_name);
    let node = documents::attach_document(
        &db,
        &owner,
        &file_name,
        &stored.to_string_lossy(),
        mime,
        size,
    )
    .await?;
    let node_uuid = record_uuid(&node);
    emit(kernel, "document_attached", &node_uuid, "document", &file_name).await;
    emit_link(kernel, &record_uuid(&owner), &node_uuid, "attached", "entities_linked").await;
    Ok(format!(
        "document {node_uuid} attached ({mime}, {size} bytes) — stored at {}\n",
        stored.display()
    ))
}

async fn tree_cmd(kernel: &Kernel, args: &[String]) -> Result<String> {
    let db = kernel.module_db(MODULE_NAME).await?;
    let fragment = args.first().ok_or_else(usage)?;
    let reverse = args.iter().any(|a| a == "--reverse");
    let depth = match args.iter().position(|a| a == "--depth") {
        Some(i) => args
            .get(i + 1)
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|&d| d > 0)
            .ok_or_else(usage)?,
        None => resolved_max_depth(kernel).await,
    };
    let root = nodes::resolve_entity(&db, fragment).await?;
    let sub = graph::subgraph(&db, &root, depth, reverse).await?;
    Ok(graph::render_tree(&sub, &root))
}

async fn graph_cmd(kernel: &Kernel, args: &[String]) -> Result<String> {
    let db = kernel.module_db(MODULE_NAME).await?;
    let fragment = args.first().ok_or_else(usage)?;
    let root = nodes::resolve_entity(&db, fragment).await?;
    let depth = match args.iter().position(|a| a == "--depth") {
        Some(i) => args
            .get(i + 1)
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|&d| d > 0)
            .ok_or_else(usage)?,
        None => resolved_max_depth(kernel).await,
    };
    let sub = graph::subgraph(&db, &root, depth, false).await?;
    if args.iter().any(|a| a == "--json") {
        Ok(format!("{:#}\n", graph::to_json(&sub, &root)))
    } else {
        Ok(format!(
            "subgraph of {}: {} nodes, {} active edges{}\n(use --json for the full export)\n",
            record_uuid(&root),
            sub.nodes.len(),
            sub.edges.len(),
            if sub.truncated_at_depth { " (depth-truncated)" } else { "" }
        ))
    }
}

/// Resolve the traversal ceiling: substrate parameter, else the
/// marked bootstrap fallback (ui-module resolved_port pattern).
pub(crate) async fn resolved_max_depth(kernel: &Kernel) -> usize {
    let Ok(Some(entity)) = kernel
        .find_module_by_name(NodeKind::KernelModule, MODULE_NAME)
        .await
    else {
        return DEFAULT_MAX_DEPTH;
    };
    match kernel.get_parameter(entity, MAX_DEPTH_PARAM).await {
        Ok(Some(Value::Number(n))) => n
            .to_int()
            .and_then(|i| usize::try_from(i).ok())
            .filter(|&d| d > 0)
            .unwrap_or(DEFAULT_MAX_DEPTH),
        _ => DEFAULT_MAX_DEPTH,
    }
}

async fn emit_link(kernel: &Kernel, from: &str, to: &str, rel: &str, event: &str) {
    let payload = superx_kernel::message::value_from_json(&serde_json::json!({
        "from": from,
        "to": to,
        "rel": rel,
    }));
    let subject = kernel
        .find_module_by_name(NodeKind::KernelModule, MODULE_NAME)
        .await
        .ok()
        .flatten();
    if let Err(e) = kernel.log_telemetry(event, payload, subject).await {
        tracing::warn!(target: "entities", "telemetry write failed: {e}");
    }
}

async fn types_cmd(kernel: &Kernel, args: &[String]) -> Result<String> {
    let db = kernel.module_db(MODULE_NAME).await?;
    match args.first().map(String::as_str) {
        None => {
            let rows = registry::list_types(&db).await?;
            let mut out = format!("type registry ({}):\n", rows.len());
            for category in registry::CATEGORIES {
                out.push_str(&format!("  {category} types:\n"));
                for row in rows.iter().filter(|r| r.category == category) {
                    out.push_str(&format!(
                        "    {:<12} {}\n",
                        row.name,
                        row.description.as_deref().unwrap_or("")
                    ));
                }
            }
            Ok(out)
        }
        Some("add") => {
            let (name, category, description) = parse_add(&args[1..])?;
            registry::add_type(&db, &name, &category, description.as_deref()).await?;
            emit(kernel, "entity_type_added", &name, &category, "").await;
            Ok(format!("type '{name}' registered ({category})\n"))
        }
        _ => Err(usage()),
    }
}

/// Attribute a write to this module in the one firehose; telemetry
/// failure never fails the verb that already committed.
async fn emit(kernel: &Kernel, event: &str, uid: &str, kind: &str, name: &str) {
    let payload = superx_kernel::message::value_from_json(&serde_json::json!({
        "uid": uid,
        "type": kind,
        "name": name,
    }));
    let subject = kernel
        .find_module_by_name(NodeKind::KernelModule, MODULE_NAME)
        .await
        .ok()
        .flatten();
    if let Err(e) = kernel.log_telemetry(event, payload, subject).await {
        tracing::warn!(target: "entities", "telemetry write failed: {e}");
    }
}

fn parse_attrs(raw: &str) -> Result<Value> {
    let json: serde_json::Value = serde_json::from_str(raw)
        .map_err(|e| KernelError::Module(format!("--attrs is not valid JSON: {e}")))?;
    if !json.is_object() {
        return Err(KernelError::Module(
            "--attrs must be a JSON object, e.g. '{\"sku\": \"W-1\"}'".to_string(),
        ));
    }
    Ok(superx_kernel::message::value_from_json(&json))
}

fn render_state(state: &nodes::StateRow, indent: &str) -> String {
    let mut out = format!("{indent}name: {}\n", state.name);
    if let Some(content) = &state.content {
        out.push_str(&format!("{indent}content: {content}\n"));
    }
    if let Some(attributes) = &state.attributes {
        out.push_str(&format!(
            "{indent}attributes: {}\n",
            nodes::value_to_json(attributes)
        ));
    }
    out
}

fn next_value(args: &[String], i: usize) -> Result<String> {
    args.get(i + 1).cloned().ok_or_else(usage)
}

fn usage() -> KernelError {
    KernelError::Module(USAGE.to_string())
}

/// The value after a `--flag`, if it is present and followed by one.
fn flag(args: &[String], name: &str) -> Option<String> {
    let at = args.iter().position(|a| a == name)?;
    args.get(at + 1).cloned()
}

/// Parse `add <name> --category <c> [--description <text…>]`.
fn parse_add(args: &[String]) -> Result<(String, String, Option<String>)> {
    let name = args.first().ok_or_else(usage)?.clone();
    let mut category = None;
    let mut description: Option<String> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--category" => {
                category = Some(next_value(args, i)?);
                i += 2;
            }
            "--description" => {
                description = Some(args[i + 1..].join(" "));
                break;
            }
            _ => return Err(usage()),
        }
    }
    Ok((name, category.ok_or_else(usage)?, description))
}
