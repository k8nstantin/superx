//! The agents' memory (#566, step 13): the folders the agents keep under the
//! projects roots the Claude Code adapter captures from, found from each
//! session's transcript and listed as files, read-only.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::Serialize;
use superx_kernel::adapters::claude_code::{ADAPTER_NAME, PROJECTS_ROOT_PARAM};
use superx_kernel::types::Value;
use superx_kernel::{Kernel, NodeKind, Result};

/// The folder a project keeps its memory in.
const MEMORY: &str = "memory"; // skill-allow: §9-const — agent-format knowledge: where Claude Code keeps a project's memory

/// One file of a memory folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemoryFile {
    pub name: String,
    pub size: u64,
    pub modified: Option<DateTime<Utc>>,
    /// The `description:` of the frontmatter that opens the file.
    pub description: Option<String>,
}

/// A session's memory folder and its files, or why it has none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemoryView {
    pub folder: Option<String>,
    pub files: Vec<MemoryFile>,
    pub why: Option<String>,
}

/// The projects roots the adapter captures from: its parameter, one path or
/// a list, read with the kernel's own verbs; when it is not set, the folder
/// `.claude/projects` under `home`, the adapter's own fallback.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] when the kernel's database does not answer.
pub async fn roots(kernel: &Kernel, home: Option<&Path>) -> Result<Vec<PathBuf>> {
    if let Some(entity) = kernel.find_module_by_name(NodeKind::Adapter, ADAPTER_NAME).await? {
        match kernel.get_parameter(entity, PROJECTS_ROOT_PARAM).await? {
            Some(Value::String(p)) => return Ok(vec![PathBuf::from(p)]),
            Some(Value::Array(paths)) => {
                return Ok(paths
                    .iter()
                    .filter_map(|v| match v {
                        Value::String(p) => Some(PathBuf::from(p)),
                        _ => None,
                    })
                    .collect())
            }
            _ => {}
        }
    }
    // The adapter's own fallback, as `projects_roots` has it.
    // skill-allow: §9-default — agent-format default, the adapter's; the operator overrides it via the parameter
    Ok(home.map(|h| h.join(".claude").join("projects")).into_iter().collect())
}

/// A transcript's project folder: the root's child folder on its path,
/// under the projects root that holds it.
#[must_use]
pub fn project(transcript: &Path, roots: &[PathBuf]) -> Option<PathBuf> {
    roots.iter().find_map(|root| {
        let first = transcript.strip_prefix(root).ok()?.components().next()?;
        Some(root.join(first))
    })
}

/// A transcript's memory folder, the folder `memory` in its project folder.
#[must_use]
pub fn folder(transcript: &Path, roots: &[PathBuf]) -> Option<PathBuf> {
    project(transcript, roots).map(|p| p.join(MEMORY))
}

/// Whether `path` lies in the folder `memory` of a projects root's child.
#[must_use]
pub fn holds(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| {
        let Ok(rel) = path.strip_prefix(root) else { return false };
        let mut parts = rel.components();
        parts.next().is_some() && parts.next().is_some_and(|c| c.as_os_str() == MEMORY) && parts.next().is_some()
    })
}

/// A memory folder's files, in the order of their names.
///
/// # Errors
///
/// Why the folder cannot be listed, in the system's words.
pub fn list(folder: &Path) -> std::result::Result<Vec<MemoryFile>, String> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(folder).map_err(|e| format!("{}: {e}", folder.display()))? {
        let entry = entry.map_err(|e| format!("{}: {e}", folder.display()))?;
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(entry.path()).unwrap_or_default();
        files.push(MemoryFile {
            name: entry.file_name().to_string_lossy().into_owned(),
            size: meta.len(),
            modified: meta.modified().ok().map(DateTime::<Utc>::from),
            description: description(&text),
        });
    }
    files.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(files)
}

/// The value of a `description:` line in the frontmatter that opens `text`
/// between two lines of `---`.
#[must_use]
pub fn description(text: &str) -> Option<String> {
    let mut lines = text.lines();
    if lines.next()?.trim_end() != "---" {
        return None;
    }
    for line in lines {
        if line.trim_end() == "---" {
            return None;
        }
        if let Some(value) = line.strip_prefix("description:") {
            let value = value.trim();
            let value = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or(value);
            return Some(value.to_string());
        }
    }
    None
}
