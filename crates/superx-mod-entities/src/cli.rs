//! `superx entities …` — the module's own CLI, dispatched by the
//! kernel's external_subcommand router. E1: type registry. E2: node
//! verbs. Graph edges/traversal land in E3–E4.

use superx_kernel::types::Value;
use superx_kernel::{Kernel, KernelError, NodeKind, Result};
use superx_ops::record_uuid;

use crate::{
    attachments, documents, edges, fields, graph, migrate, nodes, notes, registry, texts,
    MODULE_NAME,
};

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
  comment <uuid-fragment> [--author-kind operator|role|agent|system]\n\
          [--author-uid <id>] [--via <role>] <text…>\n\
  attach <uuid-fragment> <file-path>   copy a file in; document node + attached edge\n\
  tree <uuid-fragment> [--depth <n>] [--reverse]\n\
  graph <uuid-fragment> [--json] [--depth <n>]   export the reachable subgraph\n\
  types                                list the type registry\n\
  types add <name> --category entity|relation [--description <text>]\n\
  labels [--all] [--for <type>]        the dictionary: what the terminology means\n\
  fields <uuid-fragment>               the declared values of an entity\n\
  set <uuid-fragment> <key> <value>    set one declared value, checked against its kind\n\
  labels define <key> --kind slot|link --semantics <s> [--cardinality one|many]\n\
          [--source-types a,b] [--target-types a,b] [--acyclic] [--inverse <text>]\n\
                       [--value-kind <k>] [--display <d>] [--description <text>]\n\
  labels history <key> --kind slot|link      every version of one label, oldest first\n\
  labels archive <key> --kind slot|link [--restore]\n\
  migrate-prose [--dry-run]            move text carriers into the note store, history and all\n\
  notes <uuid-fragment> [--all]        the prose attached to an entity, by label\n\
  note --on entity|type|label <id> --label <l> <text…>   prose on a type or a label too\n\
  files --on entity|type|label <id>    what is attached, and what each file MEANS\n\
  file --on entity|type|label <id> --label <l> <path>    attach a file AS that label\n\
  file retract <attachment-uid>\n\
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
        Some("note") => note_on_cmd(kernel, &args[1..]).await,
        Some("files") => files_cmd(kernel, &args[1..]).await,
        Some("file") => file_cmd(kernel, &args[1..]).await,
        Some("migrate-prose") => migrate_cmd(kernel, &args[1..]).await,
        Some("fields") => fields_cmd(kernel, &args[1..]).await,
        Some("set") => set_cmd(kernel, &args[1..]).await,
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
        let cardinality = flag(args, "--cardinality");
        let value_kind = flag(args, "--value-kind");
        let inverse = flag(args, "--inverse");
        let source_types = flag(args, "--source-types").map(|v| csv(&v));
        let target_types = flag(args, "--target-types").map(|v| csv(&v));
        let acyclic = args.iter().any(|a| a == "--acyclic").then_some(true);
        crate::dictionary::define(
            &db,
            crate::dictionary::Definition {
                key,
                kind: &kind,
                display: &display,
                semantics: &semantics,
                description: description.as_deref(),
                cardinality: cardinality.as_deref(),
                value_kind: value_kind.as_deref(),
                source_types: source_types.as_deref(),
                target_types: target_types.as_deref(),
                inverse: inverse.as_deref(),
                acyclic,
            },
        )
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
            // The TYPE's override wins (§5.2): `description` is context on
            // a product and directive on a task. Printing the label's own
            // semantics here showed the wrong one — and semantics is the
            // thing an agent acts on.
            let semantics = slot
                .semantics_override
                .clone()
                .or_else(|| defined.as_ref().map(|d| d.semantics.clone()))
                .unwrap_or_else(|| "?".to_string());
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

/// `superx entities fields` — the declared values of an entity (#274).
///
/// A field is a slot label whose kind is a value kind. Reads never fail:
/// a key the type no longer declares still shows, marked, because a
/// removed declaration must not look like deleted data.
async fn fields_cmd(kernel: &Kernel, args: &[String]) -> Result<String> {
    let db = kernel.module_db(MODULE_NAME).await?;
    let fragment = args.first().ok_or_else(usage)?;
    let anchor = nodes::resolve_entity(&db, fragment).await?;
    let all = fields::of(&db, &anchor).await?;
    if all.is_empty() {
        return Ok("this type declares no values — only prose\n".to_string());
    }
    let mut out = format!("{} field(s)\n\n", all.len());
    for f in &all {
        let value = f.value.as_deref().unwrap_or("—");
        let note = if f.undeclared {
            "  [no longer declared]"
        } else if f.required && f.value.is_none() {
            "  [required, unset]"
        } else {
            ""
        };
        out.push_str(&format!(
            "  {:<14} {:<11} {}{}\n",
            f.key,
            if f.value_kind.is_empty() { "?" } else { &f.value_kind },
            value,
            note
        ));
    }
    Ok(out)
}

/// `superx entities set` — write one declared value.
async fn set_cmd(kernel: &Kernel, args: &[String]) -> Result<String> {
    let db = kernel.module_db(MODULE_NAME).await?;
    let fragment = args.first().ok_or_else(usage)?;
    let key = args.get(1).ok_or_else(usage)?;
    let value = rest(args, 2)?;
    let anchor = nodes::resolve_entity(&db, fragment).await?;
    fields::set(&db, &anchor, key, &value).await?;
    emit(kernel, "entity_updated", &record_uuid(&anchor), "", key).await;
    Ok(format!("{key} set\n"))
}

/// `superx entities migrate-prose` — move the old text carriers into
/// the note store (#276).
///
/// Nothing is deleted: the text nodes and their edges stay exactly where
/// they are. Every VERSION moves, carrying its original timestamp, so
/// the evolution reads as it always did rather than collapsing into the
/// instant the migration ran.
async fn migrate_cmd(kernel: &Kernel, args: &[String]) -> Result<String> {
    let db = kernel.module_db(MODULE_NAME).await?;
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let r = migrate::prose(&db, dry_run).await?;

    let mut out = if dry_run {
        String::from("dry run — nothing written\n\n")
    } else {
        String::new()
    };
    out.push_str(&format!(
        "  {:<28} {}\n  {:<28} {}\n  {:<28} {}\n",
        "text carriers", r.carriers,
        if dry_run { "versions to move" } else { "versions moved" }, r.versions,
        "already moved", r.already,
    ));
    if r.dual_written > 0 {
        out.push_str(&format!(
            "  {:<28} {}\n",
            "already a note (dual-write)", r.dual_written
        ));
    }
    if !r.other_roles.is_empty() {
        out.push_str(&format!(
            "\n  {} text node(s) reached by a NON-prose edge (the runner's \
             `produced`) — out of scope here, left alone:\n",
            r.other_roles.len()
        ));
        for id in r.other_roles.iter().take(10) {
            out.push_str(&format!("    {id}\n"));
        }
    }
    if !r.orphans.is_empty() {
        out.push_str(&format!(
            "\n  {} text node(s) nothing active points at — nothing claims them, \
             so nothing is guessed:\n",
            r.orphans.len()
        ));
        for id in r.orphans.iter().take(10) {
            out.push_str(&format!("    {id}\n"));
        }
    }
    out.push_str("\nthe text nodes and their edges are untouched — this adds, it does not move\n");
    Ok(out)
}

/// `--on <kind> <id>`: what the content belongs to (#296).
async fn target_from(db: &superx_kernel::Db, args: &[String]) -> Result<crate::target::Target> {
    let kind = flag(args, "--on").ok_or_else(usage)?;
    // The id is the argument after the kind, so `--on type product` reads
    // as one phrase rather than two flags that must agree.
    let at = args.iter().position(|a| a == "--on").ok_or_else(usage)?;
    let id = args.get(at + 2).ok_or_else(usage)?;
    crate::target::Target::resolve(db, &kind, id).await
}

/// The prose belonging to a target, whichever of the three it is.
fn render_notes(all: &[notes::Note]) -> String {
    let mut out = format!("{} note(s)\n", all.len());
    for n in all {
        let who = n.author_kind.as_deref().unwrap_or("?");
        let when = n
            .valid_from
            .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "-".to_string());
        out.push_str(&format!("\n  {}  {when}  {who}\n    {}\n    uid {}\n", n.label, n.body, n.uid));
    }
    out
}

/// `superx entities note --on type product --label description <text>`
///
/// A type is exactly the thing people argue about (§3), and until now it
/// had nowhere to hold the argument.
async fn note_on_cmd(kernel: &Kernel, args: &[String]) -> Result<String> {
    let db = kernel.module_db(MODULE_NAME).await?;
    let target = target_from(&db, args).await?;
    let label = flag(args, "--label").ok_or_else(usage)?;
    let text = rest_after_flags(args, &["--on", "--label"])?;
    if text.is_empty() {
        // Nothing to write means this is a read.
        let all = notes::for_target(&db, &target, false).await?;
        return Ok(render_notes(&all));
    }
    let (uid, is_new) =
        notes::write_to_target(&db, &target, &label, &text, &notes::Author::operator()).await?;
    Ok(format!(
        "{} on {} '{}' — note {uid}\n",
        if is_new { "wrote" } else { "amended" },
        target.kind(),
        target.uid()
    ))
}

/// What is attached to a target, and what each file MEANS.
async fn files_cmd(kernel: &Kernel, args: &[String]) -> Result<String> {
    let db = kernel.module_db(MODULE_NAME).await?;
    let target = target_from(&db, args).await?;
    let all = attachments::for_target(&db, &target, args.iter().any(|a| a == "--all")).await?;
    if all.is_empty() {
        return Ok("nothing attached\n".to_string());
    }
    let mut out = format!("{} file(s)\n", all.len());
    for a in &all {
        let retracted = if a.active { "" } else { "  [retracted]" };
        out.push_str(&format!(
            "\n  [{}] {}  {}  {} bytes{}\n    uid {}\n",
            a.label, a.filename, a.mime, a.size, retracted, a.uid
        ));
    }
    out.push_str("\nthe label is what the file MEANS — a spec sheet as a PDF is still a spec\n");
    Ok(out)
}

/// Attach a file AS a label — a mandate can be typed inline on one role
/// and uploaded as a PDF on another (§5.4).
async fn file_cmd(kernel: &Kernel, args: &[String]) -> Result<String> {
    let db = kernel.module_db(MODULE_NAME).await?;

    if args.first().map(String::as_str) == Some("retract") {
        let uid = args.get(1).ok_or_else(usage)?;
        attachments::retract(&db, uid, &notes::Author::operator()).await?;
        return Ok(format!(
            "retracted {uid} — the bytes stay on disk and every version is still readable\n"
        ));
    }

    let target = target_from(&db, args).await?;
    let label = flag(args, "--label").ok_or_else(usage)?;
    let path = rest_after_flags(args, &["--on", "--label"])?;
    if path.is_empty() {
        return Err(usage());
    }
    let dir = kernel.module_dir(MODULE_NAME)?;
    let uid = attachments::attach(
        &db,
        &dir,
        &target,
        &label,
        std::path::Path::new(&path),
        &notes::Author::operator(),
    )
    .await?;
    Ok(format!(
        "attached as '{label}' on {} '{}' — {uid}\n",
        target.kind(),
        target.uid()
    ))
}

/// Everything that is not a flag or a flag's value, joined.
///
/// `--on` takes TWO words (kind and id), which is why this cannot be the
/// generic leading-flag parser used elsewhere.
fn rest_after_flags(args: &[String], flags: &[&str]) -> Result<String> {
    let mut parts: Vec<&str> = Vec::new();
    let mut skip = 0usize;
    for (i, arg) in args.iter().enumerate() {
        if skip > 0 {
            skip -= 1;
            continue;
        }
        if flags.contains(&arg.as_str()) {
            skip = if arg == "--on" { 2 } else { 1 };
            continue;
        }
        let _ = i;
        parts.push(arg);
    }
    Ok(parts.join(" "))
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

    // The third door (#294). `--attrs` used to write whatever JSON it was
    // handed, so every rule fields::set enforces could be walked around by
    // sending the same value one command over.
    let attributes = match attributes {
        Some(superx_kernel::types::Value::Object(bag)) => Some(
            superx_kernel::types::Value::Object(fields::validate_bag(&db, &anchor, &bag).await?),
        ),
        other => other,
    };

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
    // Prose from the note store (#278), by label rather than by the edge
    // it used to hang off.
    let attached = notes::for_entity(&db, &anchor, false).await?;
    if !attached.is_empty() {
        out.push_str("prose:\n");
        for note in attached {
            let who = note.author_kind.as_deref().unwrap_or("?");
            out.push_str(&format!(
                "  [{}] ({}) {}  {}\n",
                note.label, note.uid, who, note.body
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
    // Flags come FIRST and the prose is everything after them, which is
    // this CLI's existing shape (`create --type <t> … <name…>`).
    //
    // Scanning the whole line for flags instead would eat a comment that
    // happens to say "route --via the proxy" — the token AND the word
    // after it — and the author would never know two words went missing.
    let (author, text) = leading_author(&args[1..])?;
    if text.is_empty() {
        return Err(usage());
    }
    let target = nodes::resolve_entity(&db, fragment).await?;
    let node = texts::add_comment(&db, &target, &text, &author).await?;
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

/// Read the author flags off the FRONT of a comment, and return them
/// with the prose that follows.
///
/// Stops at the first argument that is not one of these flags, so
/// everything from there on is the comment verbatim — including a word
/// that happens to look like a flag.
fn leading_author(args: &[String]) -> Result<(notes::Author, String)> {
    let mut kind: Option<&str> = None;
    let mut uid: Option<&str> = None;
    let mut via: Option<&str> = None;
    let mut at = 0;

    while at < args.len() {
        let slot = match args[at].as_str() {
            "--author-kind" => &mut kind,
            "--author-uid" => &mut uid,
            "--via" => &mut via,
            _ => break,
        };
        let value = args.get(at + 1).ok_or_else(usage)?;
        *slot = Some(value.as_str());
        at += 2;
    }

    let author = match kind {
        Some(k) => notes::Author::claimed(k, uid, via)?,
        // Absent means the operator, which is what a person typing this
        // is. Naming a uid without a kind is a mistake, not a default.
        None if uid.is_some() || via.is_some() => {
            return Err(KernelError::Module(
                "--author-uid and --via need --author-kind to say what is writing".to_string(),
            ))
        }
        None => notes::Author::operator(),
    };
    Ok((author, args[at..].join(" ")))
}

/// A comma-separated list, as a flag carries one.
fn csv(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
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
