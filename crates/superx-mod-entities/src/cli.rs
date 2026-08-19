//! `superx entities …` — the module's own CLI, dispatched by the
//! kernel's external_subcommand router. E1: type registry. E2: node
//! verbs. Graph edges/traversal land in E3–E4.

use superx_kernel::types::Value;
use superx_kernel::{Kernel, KernelError, NodeKind, Result};
use superx_ops::record_uuid;

use crate::{nodes, registry, MODULE_NAME};

const USAGE: &str = "usage: superx entities <command>\n\
  create --type <type> [--content <text>] [--attrs <json>] <name…>\n\
  update <uuid-fragment> [--name <name>] [--content <text>] [--attrs <json>]\n\
  show <uuid-fragment> [--history]\n\
  list [--type <type>]\n\
  types                                          list the type registry\n\
  types add <name> --category entity|relation [--description <text>]\n\
(edges + traversal — link/tree/graph/… — land in epic #166 phases E3–E4)";

/// Route a `superx entities …` invocation.
///
/// # Errors
///
/// [`KernelError::Module`] with usage on unknown commands; verb
/// errors pass through.
pub async fn dispatch(kernel: &Kernel, args: &[String]) -> Result<String> {
    match args.first().map(String::as_str) {
        Some("types") => types_cmd(kernel, &args[1..]).await,
        Some("create") => create_cmd(kernel, &args[1..]).await,
        Some("update") => update_cmd(kernel, &args[1..]).await,
        Some("show") => show_cmd(kernel, &args[1..]).await,
        Some("list") => list_cmd(kernel, &args[1..]).await,
        _ => Err(KernelError::Module(USAGE.to_string())),
    }
}

async fn create_cmd(kernel: &Kernel, args: &[String]) -> Result<String> {
    let db = kernel.module_db(MODULE_NAME).await?;
    let mut entity_type = None;
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
