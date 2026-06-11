//! SuperX telemetry capture — THE core FVP function.
//!
//! The L1 kernel module that turns discovered sources into a live
//! telemetry firehose. Discovery (F8/F9) records *where* agent
//! activity lives (`node_source` entities); capture polls each source
//! through the driver watcher that understands it, emits one typed
//! `telemetry_stream` event per observed activity, and checkpoints
//! its read position via cursor verbs so a restart never re-captures
//! and never loses its place.
//!
//! Structure mirrors discovery's E2 pattern: drivers implement
//! [`CaptureSource`] and register into [`CAPTURE_SOURCES`] via
//! linkme. The loop matches sources to watchers by the `probe` field
//! discovery stamped on each `attr_source_descriptor` — the watcher
//! that captures a source is the driver whose probe discovered it.
//!
//! **Capture-from-now semantics:** on first contact with a source the
//! watcher initializes its cursor at the current end and emits
//! nothing — like `tail -f`. The FVP captures live activity;
//! history backfill is a post-FVP parameter.
//!
//! **Nothing crashes the loop:** a watcher error becomes a
//! `capture_error` telemetry event and the tick moves on. Every tick
//! reads its interval from the `attr_capture_interval_secs` parameter
//! (seeded at startup, operator-supersedable at runtime — the loop
//! picks up changes on the next tick).

use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use linkme::distributed_slice;
use superx_kernel::types::{Object, RecordId, Value};
use superx_kernel::{
    Kernel, KernelModule, KernelModuleDescriptor, MetamodelType, NodeKind, Result,
    KERNEL_MODULES,
};

/// The capture module's name in the registry.
pub const MODULE_NAME: &str = "capture";

/// Poll-interval parameter (seconds), on the capture module's
/// registry entity.
pub const INTERVAL_PARAM: &str = "attr_capture_interval_secs";

/// Interval used to seed the parameter when unset. This constant is
/// the *seed*, not the runtime value — startup writes it into the
/// substrate as `attr_capture_interval_secs`, and every tick reads
/// the parameter back, so the operator-visible substrate state always
/// governs.
// skill-allow: §9-const — seed value; recorded as a substrate parameter at startup
const DEFAULT_INTERVAL_SECS: i64 = 2;

/// One source as handed to a watcher: the substrate identity plus the
/// descriptor fields discovery recorded.
#[derive(Debug, Clone)]
pub struct SourceRef {
    /// The `node_source` entity.
    pub entity_id: RecordId,
    /// Where the source lives (filesystem path, URL, …).
    pub locator: String,
    /// The agent this source belongs to.
    pub agent: String,
}

/// The watcher contract drivers implement: poll one source, emit
/// telemetry through kernel verbs, manage your own cursor.
#[async_trait]
pub trait CaptureSource: Send + Sync + 'static {
    /// The discovery probe whose sources this watcher understands —
    /// matched against `attr_source_descriptor.probe`.
    fn probe_name(&self) -> &'static str;

    /// Poll one source once. Returns the number of events captured.
    /// First contact should initialize the cursor at the current end
    /// and return 0 (capture-from-now).
    async fn poll(&self, kernel: &Kernel, source: &SourceRef) -> Result<u64>;
}

/// Compile-time inventory of capture watchers, filled by driver
/// crates via `#[distributed_slice(CAPTURE_SOURCES)]`.
#[distributed_slice]
pub static CAPTURE_SOURCES: [&'static (dyn CaptureSource + Sync)];

/// What one pass over all sources did.
#[derive(Debug, Clone, Default)]
pub struct TickReport {
    /// Sources that had a matching watcher and were polled.
    pub sources_polled: u64,
    /// Total events captured across all sources.
    pub events_captured: u64,
    /// Watcher errors (each also emitted as `capture_error`).
    pub errors: u64,
}

/// Stop flag for the background loop. `shutdown()` sets it; the loop
/// exits on the next tick boundary.
static STOP: AtomicBool = AtomicBool::new(false);

/// The capture kernel module. `startup()` seeds the interval
/// parameter and spawns the background loop.
pub struct CaptureModule;

#[async_trait]
impl KernelModule for CaptureModule {
    fn descriptor(&self) -> KernelModuleDescriptor {
        KernelModuleDescriptor {
            name: MODULE_NAME,
            version: env!("CARGO_PKG_VERSION"),
            kind: NodeKind::KernelModule,
            category: "",
            depends_on: &["discovery"],
            required_metamodel: &[MetamodelType {
                uid: INTERVAL_PARAM,
                category: "attribute",
                memory_tier: "working",
            }],
        }
    }

    async fn startup(&self, kernel: &Kernel) -> Result<()> {
        // Seed the interval parameter once so the effective default
        // is substrate state, then run detached.
        let _ = interval_secs(kernel).await?;
        STOP.store(false, Ordering::SeqCst);
        let looped = kernel.clone();
        tokio::spawn(async move {
            run_loop(&looped).await;
        });
        Ok(())
    }

    async fn shutdown(&self, _kernel: &Kernel) -> Result<()> {
        STOP.store(true, Ordering::SeqCst);
        Ok(())
    }
}

#[distributed_slice(KERNEL_MODULES)]
static CAPTURE_REGISTRATION: &'static (dyn KernelModule + Sync) = &CaptureModule;

/// The background loop: tick, sleep the configured interval, repeat
/// until [`CaptureModule::shutdown`] flips the stop flag. Tick
/// failures are recorded as telemetry by `capture_tick` itself;
/// substrate-level failures here end the loop with a final
/// `capture_loop_stopped` event attempt.
pub async fn run_loop(kernel: &Kernel) {
    while !STOP.load(Ordering::SeqCst) {
        if let Err(e) = capture_tick(kernel).await {
            // Loop-level failure (substrate unreachable, …) — try to
            // leave a trace, then stop rather than spin.
            let mut payload = Object::new();
            payload.insert("error".to_string(), Value::String(e.to_string()));
            let _ = kernel
                .log_telemetry("capture_loop_stopped", Value::Object(payload), None)
                .await;
            return;
        }
        let secs = interval_secs(kernel).await.unwrap_or(DEFAULT_INTERVAL_SECS);
        let secs = u64::try_from(secs).unwrap_or(DEFAULT_INTERVAL_SECS as u64).max(1);
        tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
    }
}

/// One pass over every discovered source: dispatch each to the
/// watcher whose `probe_name` matches the source's recorded probe.
/// Public so tests (and future CLI verbs) can drive single ticks
/// without timers.
///
/// # Errors
///
/// Substrate-level failures only (listing sources, emitting
/// telemetry). Watcher errors are isolated into `capture_error`
/// events and the report's error count.
pub async fn capture_tick(kernel: &Kernel) -> Result<TickReport> {
    let mut report = TickReport::default();

    let sources = kernel
        .list_named_entities("node_source", "attr_source_descriptor")
        .await?;
    for named in sources {
        let Value::Object(desc) = &named.payload else { continue };
        let probe = string_field(desc, "probe");
        let locator = string_field(desc, "locator");
        let agent = string_field(desc, "agent");
        let (Some(probe), Some(locator), Some(agent)) = (probe, locator, agent) else {
            continue;
        };
        let Some(watcher) = CAPTURE_SOURCES.iter().find(|w| w.probe_name() == probe)
        else {
            continue; // no driver in this binary understands it
        };

        let source = SourceRef {
            entity_id: named.entity_id,
            locator,
            agent,
        };
        report.sources_polled += 1;
        match watcher.poll(kernel, &source).await {
            Ok(events) => report.events_captured += events,
            Err(e) => {
                report.errors += 1;
                let mut payload = Object::new();
                payload.insert("source".to_string(), Value::String(source.locator.clone()));
                payload.insert("agent".to_string(), Value::String(source.agent.clone()));
                payload.insert("error".to_string(), Value::String(e.to_string()));
                kernel
                    .log_telemetry("capture_error", Value::Object(payload), None)
                    .await?;
            }
        }
    }
    Ok(report)
}

/// Read the poll interval, seeding the default into the substrate on
/// first use (the F9 pattern: effective defaults are recorded state).
async fn interval_secs(kernel: &Kernel) -> Result<i64> {
    let Some(status) = kernel
        .detailed_status(NodeKind::KernelModule, MODULE_NAME)
        .await?
    else {
        // Not registered (loop driven outside bootstrap, e.g. tests
        // that only tick): run with the built-in default, seed later.
        return Ok(DEFAULT_INTERVAL_SECS);
    };
    if let Some(Value::Number(n)) = kernel
        .get_parameter(status.entity_id.clone(), INTERVAL_PARAM)
        .await?
    {
        return Ok(n.to_int().unwrap_or(DEFAULT_INTERVAL_SECS));
    }
    kernel
        .set_parameter(
            status.entity_id,
            INTERVAL_PARAM,
            Value::Number(DEFAULT_INTERVAL_SECS.into()),
        )
        .await?;
    Ok(DEFAULT_INTERVAL_SECS)
}

fn string_field(obj: &Object, key: &str) -> Option<String> {
    match obj.get(key) {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}
