//! `superx entities …` — the module's own CLI, dispatched by the
//! kernel's external_subcommand router. Phase E1 surface: the type
//! registry. Graph verbs land in the epic's later phases.

use superx_kernel::types::Value;
use superx_kernel::{Kernel, KernelError, NodeKind, Result};

use crate::{registry, MODULE_NAME};

const USAGE: &str = "usage: superx entities <command>\n\
  types                                          list the type registry\n\
  types add <name> --category entity|relation [--description <text>]\n\
(graph verbs — create/link/tree/… — land in epic #166 phases E2–E4)";

/// Route a `superx entities …` invocation.
///
/// # Errors
///
/// [`KernelError::Module`] with usage on unknown commands; verb
/// errors pass through.
pub async fn dispatch(kernel: &Kernel, args: &[String]) -> Result<String> {
    match args.first().map(String::as_str) {
        Some("types") => types_cmd(kernel, &args[1..]).await,
        _ => Err(KernelError::Module(USAGE.to_string())),
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
            // Attribute the change to this module in the one firehose.
            if let Ok(Some(me)) = kernel
                .find_module_by_name(NodeKind::KernelModule, MODULE_NAME)
                .await
            {
                if let Err(e) = kernel
                    .log_telemetry("entity_type_added", Value::String(name.clone()), Some(me))
                    .await
                {
                    tracing::warn!(target: "entities", "telemetry write failed: {e}");
                }
            }
            Ok(format!("type '{name}' registered ({category})\n"))
        }
        _ => Err(KernelError::Module(USAGE.to_string())),
    }
}

/// Parse `add <name> --category <c> [--description <text…>]`.
fn parse_add(args: &[String]) -> Result<(String, String, Option<String>)> {
    let usage = || KernelError::Module(USAGE.to_string());
    let name = args.first().ok_or_else(usage)?.clone();
    let mut category = None;
    let mut description: Option<String> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--category" => {
                category = Some(args.get(i + 1).ok_or_else(usage)?.clone());
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
