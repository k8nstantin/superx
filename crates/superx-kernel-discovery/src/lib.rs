//! SuperX agent discovery — the probe framework.
//!
//! The L1 kernel module that finds every agent on the system and
//! registers what it finds in the substrate. Drivers (L2 crates like
//! `superx-driver-claude-code`) implement [`DiscoveryProbe`] and
//! register into [`DISCOVERY_PROBES`] via linkme — discovery iterates
//! the slice; it knows nothing about specific drivers (E2 pattern,
//! same as [`KERNEL_MODULES`]).
//!
//! What a probe reports becomes substrate state:
//!
//! - each [`DiscoveredAgent`] → a `node_agent` entity carrying an
//!   `attr_agent_descriptor` (`{name, probe}`),
//! - each [`DiscoveredSource`] → a `node_source` entity carrying an
//!   `attr_source_descriptor` (`{name, locator, agent, probe}`) plus
//!   an `edge_agent_has_source` relation from its agent,
//! - re-runs are idempotent — found-by-name via
//!   [`Kernel::find_entity_by_name`], nothing duplicated.
//!
//! Telemetry is the point (capture is THE function): every creation
//! emits `agent_discovered` / `source_discovered`, every probe error
//! emits `probe_failed`, and every run closes with a
//! `discovery_census` summary — even when nothing was found, the
//! census proves the scan ran.
//!
//! Per-probe failure isolation: one failing probe never blocks the
//! others. All probes run; if any failed, `startup()` returns a
//! `Module` error afterwards so bootstrap records the module as
//! `Failed` while the successful probes' agents remain registered
//! and observable.

use async_trait::async_trait;
use linkme::distributed_slice;
use superx_kernel::types::{Object, RecordId, Value};
use superx_kernel::{
    Kernel, KernelError, KernelModule, KernelModuleDescriptor, MetamodelType, NodeKind,
    Result, KERNEL_MODULES,
};

fn s(v: &str) -> Value {
    Value::String(v.to_string())
}

/// One source of agent activity a probe found — something the capture
/// loop (F10) can watch: a transcript directory, a log file, an OTLP
/// endpoint.
#[derive(Debug, Clone)]
pub struct DiscoveredSource {
    /// Stable identity for the source. Conventionally the locator
    /// itself (a path is its own best name).
    pub name: String,
    /// Where the source lives — filesystem path, URL, …
    pub locator: String,
}

/// One agent a probe found, with the activity sources it exposes.
#[derive(Debug, Clone)]
pub struct DiscoveredAgent {
    /// Stable agent identity (e.g. `"claude_code"`).
    pub name: String,
    /// Activity sources belonging to this agent.
    pub sources: Vec<DiscoveredSource>,
}

/// The probe contract drivers implement. A probe inspects the host
/// system (config files, well-known directories, …) and reports what
/// agents it can identify. Probes read configuration through
/// [`Kernel::get_parameter`] — no hardcoded paths.
#[async_trait]
pub trait DiscoveryProbe: Send + Sync + 'static {
    /// Stable probe identifier, recorded on everything it discovers.
    fn name(&self) -> &'static str;

    /// Inspect the host; report agents. Absence of agents is
    /// `Ok(vec![])`, not an error — errors are for broken probes.
    async fn probe(&self, kernel: &Kernel) -> Result<Vec<DiscoveredAgent>>;
}

/// Compile-time inventory of discovery probes, filled by driver
/// crates via `#[distributed_slice(DISCOVERY_PROBES)]`.
#[distributed_slice]
pub static DISCOVERY_PROBES: [&'static (dyn DiscoveryProbe + Sync)];

/// The discovery kernel module. Bootstrap starts it; its `startup()`
/// runs every registered probe once. (Continuous re-probing is a
/// post-FVP refinement; the F10 capture loop watches the sources
/// discovered here.)
pub struct DiscoveryModule;

#[async_trait]
impl KernelModule for DiscoveryModule {
    fn descriptor(&self) -> KernelModuleDescriptor {
        KernelModuleDescriptor {
            name: "discovery",
            version: env!("CARGO_PKG_VERSION"),
            kind: NodeKind::KernelModule,
            category: "",
            depends_on: &[],
            required_metamodel: &[
                MetamodelType {
                    uid: "attr_agent_descriptor",
                    category: "attribute",
                    memory_tier: "working",
                },
                MetamodelType {
                    uid: "attr_source_descriptor",
                    category: "attribute",
                    memory_tier: "working",
                },
                MetamodelType {
                    uid: "edge_agent_has_source",
                    category: "edge",
                    memory_tier: "core",
                },
            ],
        }
    }

    async fn startup(&self, kernel: &Kernel) -> Result<()> {
        run_probes(kernel).await
    }
}

#[distributed_slice(KERNEL_MODULES)]
static DISCOVERY_REGISTRATION: &'static (dyn KernelModule + Sync) = &DiscoveryModule;

/// Run every registered probe once: register what they find, emit
/// telemetry per discovery, close with a census event.
///
/// # Errors
///
/// [`KernelError::Module`] listing every probe that failed — returned
/// only after all probes have run and all successful results are
/// registered. Substrate-level errors during registration propagate
/// directly.
pub async fn run_probes(kernel: &Kernel) -> Result<()> {
    let mut agents_discovered: u64 = 0;
    let mut agents_known: u64 = 0;
    let mut sources_discovered: u64 = 0;
    let mut probe_failures: Vec<String> = Vec::new();

    for probe in DISCOVERY_PROBES {
        match probe.probe(kernel).await {
            Ok(agents) => {
                for agent in agents {
                    let (created, agent_id) =
                        ensure_agent(kernel, probe.name(), &agent).await?;
                    if created {
                        agents_discovered += 1;
                    } else {
                        agents_known += 1;
                    }
                    for source in &agent.sources {
                        if ensure_source(kernel, probe.name(), &agent.name, agent_id.clone(), source)
                            .await?
                        {
                            sources_discovered += 1;
                        }
                    }
                }
            }
            Err(e) => {
                let mut payload = Object::new();
                payload.insert("probe".to_string(), s(probe.name()));
                payload.insert("error".to_string(), s(&e.to_string()));
                kernel
                    .log_telemetry(
                        "probe_failed",
                        Value::Object(payload),
                        None,
                    )
                    .await?;
                probe_failures.push(format!("{}: {e}", probe.name()));
            }
        }
    }

    // The census proves the scan ran even when nothing was found.
    let mut census = Object::new();
    census.insert("probes".to_string(), Value::Number((DISCOVERY_PROBES.len() as i64).into()));
    census.insert("agents_discovered".to_string(), Value::Number((agents_discovered as i64).into()));
    census.insert("agents_known".to_string(), Value::Number((agents_known as i64).into()));
    census.insert("sources_discovered".to_string(), Value::Number((sources_discovered as i64).into()));
    census.insert("probe_failures".to_string(), Value::Number((probe_failures.len() as i64).into()));
    kernel
        .log_telemetry("discovery_census", Value::Object(census), None)
        .await?;

    if probe_failures.is_empty() {
        Ok(())
    } else {
        Err(KernelError::Module(format!(
            "{} probe(s) failed: {}",
            probe_failures.len(),
            probe_failures.join("; ")
        )))
    }
}

/// Find-or-create the `node_agent` for a discovered agent. Returns
/// `(created, entity_id)`; emits `agent_discovered` on creation.
async fn ensure_agent(
    kernel: &Kernel,
    probe_name: &str,
    agent: &DiscoveredAgent,
) -> Result<(bool, RecordId)> {
    if let Some(existing) = kernel
        .find_entity_by_name("node_agent", "attr_agent_descriptor", &agent.name)
        .await?
    {
        return Ok((false, existing));
    }
    let entity_id = kernel.create_entity("node_agent", "user").await?;
    let mut desc = Object::new();
    desc.insert("name".to_string(), s(&agent.name));
    desc.insert("probe".to_string(), s(probe_name));
    kernel
        .supersede_state(entity_id.clone(), "attr_agent_descriptor", Value::Object(desc))
        .await?;

    let mut payload = Object::new();
    payload.insert("agent".to_string(), s(&agent.name));
    payload.insert("probe".to_string(), s(probe_name));
    kernel
        .log_telemetry("agent_discovered", Value::Object(payload), None)
        .await?;
    Ok((true, entity_id))
}

/// Find-or-create the `node_source` for a discovered source and its
/// edge from the agent. Returns whether it was created; emits
/// `source_discovered` on creation.
async fn ensure_source(
    kernel: &Kernel,
    probe_name: &str,
    agent_name: &str,
    agent_id: RecordId,
    source: &DiscoveredSource,
) -> Result<bool> {
    if kernel
        .find_entity_by_name("node_source", "attr_source_descriptor", &source.name)
        .await?
        .is_some()
    {
        return Ok(false);
    }
    let source_id = kernel.create_entity("node_source", "user").await?;
    let mut desc = Object::new();
    desc.insert("name".to_string(), s(&source.name));
    desc.insert("locator".to_string(), s(&source.locator));
    desc.insert("agent".to_string(), s(agent_name));
    desc.insert("probe".to_string(), s(probe_name));
    kernel
        .supersede_state(source_id.clone(), "attr_source_descriptor", Value::Object(desc))
        .await?;
    kernel
        .create_relation(agent_id, source_id, "edge_agent_has_source", true)
        .await?;

    let mut payload = Object::new();
    payload.insert("agent".to_string(), s(agent_name));
    payload.insert("source".to_string(), s(&source.name));
    payload.insert("locator".to_string(), s(&source.locator));
    payload.insert("probe".to_string(), s(probe_name));
    kernel
        .log_telemetry("source_discovered", Value::Object(payload), None)
        .await?;
    Ok(true)
}
