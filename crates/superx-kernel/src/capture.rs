//! The capture engine — kernel-core per operator decision D2.
//!
//! The engine is generic: it knows how to discover-and-poll through
//! the [`AgentAdapter`] seam, checkpoint via cursors, and turn errors
//! into telemetry. Knowing how a *specific* agent emits telemetry is
//! adapter knowledge (BLUEPRINT.md §2, "Agent adapters") — the engine
//! never names an agent.
//!
//! - [`run_discovery`] — walk [`ADAPTERS`], let each one discover its
//!   agent + sources, create the identity entities idempotently, emit
//!   `agent_discovered` / `source_discovered` telemetry.
//! - [`capture_tick`] — one poll pass over every (adapter, source)
//!   pair. Poll errors become `capture_error` telemetry, never
//!   panics; one failing source never blocks the rest.
//! - [`run_loop`] — [`capture_tick`] on an interval (parameter
//!   `attr_capture_interval_secs`, default 2 s) until the shutdown
//!   signal flips.

use std::time::Duration;

use async_trait::async_trait;
use linkme::distributed_slice;
use surrealdb::types::{Object, RecordId, Value};

use crate::error::Result;
use crate::registry::{KernelModule, KernelModuleDescriptor, NodeKind, KERNEL_MODULES};
use crate::substrate::Kernel;

/// Capture-loop interval parameter (seconds), on the capture module's
/// registry entity.
pub const INTERVAL_PARAM: &str = "attr_capture_interval_secs";

/// Default poll interval when the parameter is unset; the operator
/// overrides via `attr_capture_interval_secs`.
const DEFAULT_INTERVAL_SECS: u64 = 2; // skill-allow: §9-const — bootstrap fallback, param-overridable

/// The capture module's registry name.
pub const MODULE_NAME: &str = "capture";

/// One capturable stream, resolved to its substrate identities.
#[derive(Debug, Clone)]
pub struct SourceRef {
    /// The `node_source` entity.
    pub entity_id: RecordId,
    /// The `node_agent` entity this source belongs to.
    pub agent_id: RecordId,
    /// Human-stable source name (from its descriptor).
    pub name: String,
    /// Where the stream lives (path, URL, …) — adapter-interpreted.
    pub locator: String,
}

/// A source freshly discovered by an adapter, before it has substrate
/// identity.
#[derive(Debug, Clone)]
pub struct DiscoveredSource {
    /// Stable source name (unique within the adapter's agent).
    pub name: String,
    /// Where the stream lives — adapter-interpreted at poll time.
    pub locator: String,
}

/// The one plugin interface the kernel defines (BLUEPRINT.md §2).
/// Each adapter owns everything about ONE agent's on-disk/off-disk
/// format: where it lives, how to discover its sources, how to parse
/// what they emit.
#[async_trait]
pub trait AgentAdapter: Send + Sync + 'static {
    /// Stable adapter name (`"adapter_claude_code"`, …) — also its
    /// registry-module name.
    fn name(&self) -> &'static str;

    /// The human-stable agent name this adapter captures
    /// (`"claude_code"`, `"gemini_cli"`, …).
    fn agent_name(&self) -> &'static str;

    /// Enumerate this agent's capturable sources as found on the
    /// machine right now. Called at boot (and re-callable any time —
    /// must be side-effect free).
    async fn discover(&self, kernel: &Kernel) -> Result<Vec<DiscoveredSource>>;

    /// Poll one source: read whatever is new since the cursor, write
    /// `message` / `telemetry_stream` rows, advance the cursor.
    /// Returns the number of events captured this pass.
    async fn poll(&self, kernel: &Kernel, source: &SourceRef) -> Result<u64>;
}

/// Compile-time inventory of agent adapters.
#[distributed_slice]
pub static ADAPTERS: [&'static (dyn AgentAdapter + Sync)];

/// Report for one capture pass.
#[derive(Debug, Clone, Default)]
pub struct TickReport {
    /// Events captured per (adapter, source-name) pair.
    pub captured: Vec<(String, String, u64)>,
    /// Sources whose poll errored this pass (error already emitted as
    /// `capture_error` telemetry).
    pub errors: Vec<(String, String, String)>,
}

impl TickReport {
    /// Total events captured this pass.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.captured.iter().map(|(_, _, n)| n).sum()
    }
}

/// The capture engine's registry entry. Startup validates nothing —
/// discovery and the loop are driven explicitly (`run_discovery` at
/// boot by the adapters themselves, `run_loop` by the CLI after boot).
pub struct CaptureModule;

#[async_trait]
impl KernelModule for CaptureModule {
    fn descriptor(&self) -> KernelModuleDescriptor {
        KernelModuleDescriptor {
            name: MODULE_NAME,
            version: env!("CARGO_PKG_VERSION"),
            kind: NodeKind::KernelModule,
            depends_on: &[],
            required_metamodel: &[],
        }
    }

    async fn startup(&self, _kernel: &Kernel) -> Result<()> {
        Ok(())
    }
}

#[distributed_slice(KERNEL_MODULES)]
static CAPTURE_REGISTRATION: &'static (dyn KernelModule + Sync) = &CaptureModule;

/// Ensure a named entity exists with a descriptor, returning its id.
/// Find-by-current-name first; create + descriptor write when absent.
async fn ensure_named_entity(
    kernel: &Kernel,
    entity_type_uid: &str,
    attr_type_uid: &str,
    name: &str,
    extra: &[(&str, &str)],
) -> Result<(RecordId, bool)> {
    if let Some(existing) = kernel
        .find_entity_by_name(entity_type_uid, attr_type_uid, name)
        .await?
    {
        return Ok((existing, false));
    }
    let id = kernel.create_entity(entity_type_uid).await?;
    let mut payload = Object::new();
    payload.insert("name".to_string(), Value::String(name.to_string()));
    for (k, v) in extra {
        payload.insert((*k).to_string(), Value::String((*v).to_string()));
    }
    kernel
        .supersede_state(id.clone(), attr_type_uid, Value::Object(payload))
        .await?;
    Ok((id, true))
}

/// Ensure the `node_agent` entity for an adapter's agent; the entity
/// row id IS the agent_id (decision D9).
pub async fn ensure_agent(kernel: &Kernel, adapter: &dyn AgentAdapter) -> Result<RecordId> {
    let (id, created) = ensure_named_entity(
        kernel,
        "node_agent",
        "attr_agent_descriptor",
        adapter.agent_name(),
        &[("adapter", adapter.name())],
    )
    .await?;
    if created {
        let mut payload = Object::new();
        payload.insert(
            "agent".to_string(),
            Value::String(adapter.agent_name().to_string()),
        );
        kernel
            .log_telemetry_for_agent(
                "agent_discovered",
                Value::Object(payload),
                Some(id.clone()),
                Some(id.clone()),
            )
            .await?;
    }
    Ok(id)
}

/// Ensure the `node_session` entity for a source-scoped session id.
/// Session names are `<agent>/<session-id>` so they're globally
/// unique; the descriptor records both halves plus the locator.
pub async fn ensure_session(
    kernel: &Kernel,
    agent_id: &RecordId,
    agent_name: &str,
    session_key: &str,
    locator: &str,
) -> Result<RecordId> {
    let name = format!("{agent_name}/{session_key}");
    let (id, created) = ensure_named_entity(
        kernel,
        "node_session",
        "attr_session_descriptor",
        &name,
        &[("session", session_key), ("locator", locator)],
    )
    .await?;
    if created {
        let mut payload = Object::new();
        payload.insert("session".to_string(), Value::String(name));
        kernel
            .log_telemetry_for_agent(
                "session_discovered",
                Value::Object(payload),
                Some(id.clone()),
                Some(agent_id.clone()),
            )
            .await?;
    }
    Ok(id)
}

/// Run discovery for every adapter in [`ADAPTERS`]: ensure the agent
/// entity, ensure one `node_source` entity per discovered source,
/// emit discovery telemetry. Idempotent — re-running finds instead of
/// creating. Per-adapter failures are isolated into telemetry.
pub async fn run_discovery(kernel: &Kernel) -> Result<Vec<SourceRef>> {
    let mut out = Vec::new();
    for adapter in ADAPTERS {
        match discover_one(kernel, *adapter).await {
            Ok(mut sources) => out.append(&mut sources),
            Err(e) => {
                tracing::warn!(adapter = adapter.name(), error = %e, "discovery failed");
                let mut payload = Object::new();
                payload.insert(
                    "adapter".to_string(),
                    Value::String(adapter.name().to_string()),
                );
                payload.insert("error".to_string(), Value::String(e.to_string()));
                kernel
                    .log_telemetry("discovery_error", Value::Object(payload), None)
                    .await?;
            }
        }
    }
    Ok(out)
}

async fn discover_one(
    kernel: &Kernel,
    adapter: &'static (dyn AgentAdapter + Sync),
) -> Result<Vec<SourceRef>> {
    let agent_id = ensure_agent(kernel, adapter).await?;
    let discovered = adapter.discover(kernel).await?;
    let mut out = Vec::with_capacity(discovered.len());
    for src in discovered {
        let scoped_name = format!("{}/{}", adapter.agent_name(), src.name);
        let (source_id, created) = ensure_named_entity(
            kernel,
            "node_source",
            "attr_source_descriptor",
            &scoped_name,
            &[
                ("locator", &src.locator),
                ("adapter", adapter.name()),
            ],
        )
        .await?;
        if created {
            let mut payload = Object::new();
            payload.insert("source".to_string(), Value::String(scoped_name.clone()));
            payload.insert("locator".to_string(), Value::String(src.locator.clone()));
            kernel
                .log_telemetry_for_agent(
                    "source_discovered",
                    Value::Object(payload),
                    Some(source_id.clone()),
                    Some(agent_id.clone()),
                )
                .await?;
        }
        out.push(SourceRef {
            entity_id: source_id,
            agent_id: agent_id.clone(),
            name: scoped_name,
            locator: src.locator,
        });
    }
    tracing::info!(adapter = adapter.name(), sources = out.len(), "discovery complete");
    Ok(out)
}

/// One capture pass over every (adapter, source) pair. Errors become
/// `capture_error` telemetry and the pass continues.
pub async fn capture_tick(kernel: &Kernel, sources: &[(usize, SourceRef)]) -> Result<TickReport> {
    let mut report = TickReport::default();
    for (adapter_idx, source) in sources {
        let adapter = ADAPTERS[*adapter_idx];
        match adapter.poll(kernel, source).await {
            Ok(n) => {
                if n > 0 {
                    tracing::debug!(source = %source.name, events = n, "captured");
                }
                report
                    .captured
                    .push((adapter.name().to_string(), source.name.clone(), n));
            }
            Err(e) => {
                tracing::warn!(source = %source.name, error = %e, "capture error");
                let mut payload = Object::new();
                payload.insert("source".to_string(), Value::String(source.name.clone()));
                payload.insert("error".to_string(), Value::String(e.to_string()));
                kernel
                    .log_telemetry_for_agent(
                        "capture_error",
                        Value::Object(payload),
                        Some(source.entity_id.clone()),
                        Some(source.agent_id.clone()),
                    )
                    .await?;
                report.errors.push((
                    adapter.name().to_string(),
                    source.name.clone(),
                    e.to_string(),
                ));
            }
        }
    }
    Ok(report)
}

/// Pair every discovered source with its adapter's index in
/// [`ADAPTERS`] (sources are matched to adapters by descriptor
/// `adapter` name at discovery time; this keeps the pairing explicit
/// and testable).
pub async fn discover_paired(kernel: &Kernel) -> Result<Vec<(usize, SourceRef)>> {
    let mut out = Vec::new();
    for (idx, adapter) in ADAPTERS.iter().enumerate() {
        if let Ok(sources) = discover_one(kernel, *adapter).await {
            for s in sources {
                out.push((idx, s));
            }
        }
    }
    Ok(out)
}

/// The capture loop: tick every interval until `shutdown` flips true.
/// Re-runs discovery every tick, so agents and sources that appear
/// after boot are captured without a restart (fixes v1's one-shot
/// discovery, the top item of its quality backlog).
pub async fn run_loop(
    kernel: &Kernel,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    let interval = poll_interval_secs(kernel).await?;
    tracing::info!(interval_secs = interval, "capture loop starting");
    loop {
        let sources = discover_paired(kernel).await?;
        let report = capture_tick(kernel, &sources).await?;
        if report.total() > 0 {
            tracing::info!(events = report.total(), "capture tick");
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(interval)) => {}
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    tracing::info!("capture loop shutting down");
                    return Ok(());
                }
            }
        }
    }
}

/// Resolve the poll interval from the capture module's parameter,
/// falling back to the default.
pub async fn poll_interval_secs(kernel: &Kernel) -> Result<u64> {
    let Some(entity) = kernel
        .find_module_by_name(NodeKind::KernelModule, MODULE_NAME)
        .await?
    else {
        return Ok(DEFAULT_INTERVAL_SECS);
    };
    match kernel.get_parameter(entity, INTERVAL_PARAM).await? {
        Some(Value::Number(n)) => Ok(n
            .to_int()
            .and_then(|i| u64::try_from(i).ok())
            .unwrap_or(DEFAULT_INTERVAL_SECS)),
        _ => Ok(DEFAULT_INTERVAL_SECS),
    }
}
