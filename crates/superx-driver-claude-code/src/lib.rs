//! SuperX Claude Code driver — discovers Claude Code activity on the
//! host.
//!
//! Claude Code writes one JSONL transcript per session under
//! `<projects-root>/<project-slug>/<session-uuid>.jsonl` (the root is
//! `~/.claude/projects` by default). This driver's probe reports one
//! `claude_code` agent with one capture source per project directory;
//! the capture loop (F10) tails the `*.jsonl` files inside each.
//!
//! Configuration is a substrate parameter, never a hardcoded path:
//! the probe reads `attr_claude_code_projects_root` off the driver's
//! own registry entity. When unset, the probe derives
//! `$HOME/.claude/projects` once and **writes it back as the
//! parameter** — the default becomes visible, auditable,
//! operator-supersedable substrate state. Tests point the parameter
//! at fixture directories via `set_parameter`; no env hacks.
//!
//! The capture half lives in [`watcher`]: a [`TranscriptWatcher`]
//! registered into the capture loop's `CAPTURE_SOURCES` slice tails
//! the discovered directories' `*.jsonl` transcripts.
//!
//! [`TranscriptWatcher`]: watcher::TranscriptWatcher

pub mod watcher;

use std::path::PathBuf;

use async_trait::async_trait;
use linkme::distributed_slice;
use superx_kernel::types::Value;
use superx_kernel::{
    Kernel, KernelError, KernelModule, KernelModuleDescriptor, MetamodelType, ModuleStatus,
    NodeKind, Result, KERNEL_MODULES,
};
use superx_kernel_discovery::{
    DiscoveredAgent, DiscoveredSource, DiscoveryProbe, DISCOVERY_PROBES,
};

/// The driver's stable identity in the registry and on everything it
/// discovers.
pub const DRIVER_NAME: &str = "driver_claude_code";

/// The agent identity this driver reports.
pub const AGENT_NAME: &str = "claude_code";

/// The substrate parameter holding the projects root.
pub const PROJECTS_ROOT_PARAM: &str = "attr_claude_code_projects_root";

/// Registry presence: an L2 driver, dependent on the discovery
/// framework it plugs into. Its `startup()` is a no-op — the work
/// happens in the probe, which discovery invokes.
pub struct ClaudeCodeDriver;

#[async_trait]
impl KernelModule for ClaudeCodeDriver {
    fn descriptor(&self) -> KernelModuleDescriptor {
        KernelModuleDescriptor {
            name: DRIVER_NAME,
            version: env!("CARGO_PKG_VERSION"),
            kind: NodeKind::Contribution,
            category: "driver",
            depends_on: &["discovery"],
            required_metamodel: &[MetamodelType {
                uid: PROJECTS_ROOT_PARAM,
                category: "attribute",
                memory_tier: "working",
            }],
        }
    }

    async fn startup(&self, _kernel: &Kernel) -> Result<()> {
        Ok(())
    }
}

#[distributed_slice(KERNEL_MODULES)]
static DRIVER_REGISTRATION: &'static (dyn KernelModule + Sync) = &ClaudeCodeDriver;

/// The discovery probe. Registered via linkme; runs whenever the
/// discovery module probes the host.
pub struct ClaudeCodeProbe;

#[async_trait]
impl DiscoveryProbe for ClaudeCodeProbe {
    fn name(&self) -> &'static str {
        DRIVER_NAME
    }

    async fn probe(&self, kernel: &Kernel) -> Result<Vec<DiscoveredAgent>> {
        // Probes are linkme statics, so they run even when the
        // operator disabled the driver module — honor the disable
        // here.
        if kernel.module_status(NodeKind::Contribution, DRIVER_NAME).await?
            == Some(ModuleStatus::Disabled)
        {
            return Ok(vec![]);
        }

        let root = projects_root(kernel).await?;
        if !root.is_dir() {
            // Claude Code not installed / never run — an empty find,
            // not an error.
            return Ok(vec![]);
        }

        let mut sources = Vec::new();
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&root)
            .map_err(|e| {
                KernelError::Module(format!(
                    "cannot read projects root {}: {e}",
                    root.display()
                ))
            })?
            .filter_map(std::result::Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        entries.sort();
        for project_dir in entries {
            let locator = project_dir.display().to_string();
            sources.push(DiscoveredSource {
                name: locator.clone(),
                locator,
            });
        }

        Ok(vec![DiscoveredAgent {
            name: AGENT_NAME.to_string(),
            sources,
        }])
    }
}

#[distributed_slice(DISCOVERY_PROBES)]
static PROBE_REGISTRATION: &'static (dyn DiscoveryProbe + Sync) = &ClaudeCodeProbe;

/// Resolve the projects root: the substrate parameter on the driver's
/// registry entity, seeding `$HOME/.claude/projects` on first use so
/// the effective default is always recorded substrate state.
///
/// # Errors
///
/// [`KernelError::Module`] when the driver isn't registered yet
/// (probe invoked outside a bootstrap) or no default can be derived
/// (no `HOME`).
async fn projects_root(kernel: &Kernel) -> Result<PathBuf> {
    let Some(status) = kernel
        .detailed_status(NodeKind::Contribution, DRIVER_NAME)
        .await?
    else {
        return Err(KernelError::Module(format!(
            "{DRIVER_NAME} is not registered — probe requires a bootstrapped substrate"
        )));
    };
    let entity_id = status.entity_id;

    if let Some(value) = kernel.get_parameter(entity_id.clone(), PROJECTS_ROOT_PARAM).await? {
        let Value::String(path) = value else {
            return Err(KernelError::Corrupt(format!(
                "{PROJECTS_ROOT_PARAM} is not a string"
            )));
        };
        return Ok(PathBuf::from(path));
    }

    // First use: derive the conventional default and record it as the
    // parameter so it is visible and operator-supersedable.
    let home = std::env::var("HOME").map_err(|_| {
        KernelError::Module(format!(
            "{PROJECTS_ROOT_PARAM} unset and no HOME to derive a default from"
        ))
    })?;
    let default = PathBuf::from(home).join(".claude").join("projects");
    kernel
        .set_parameter(
            entity_id,
            PROJECTS_ROOT_PARAM,
            Value::String(default.display().to_string()),
        )
        .await?;
    Ok(default)
}
