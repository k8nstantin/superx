//! The boot orchestrator — in-kernel per BLUEPRINT.md §2.2 (operator
//! decision D2): boot is what the kernel *does*, not a module bolted
//! on.
//!
//! OS-style boot semantics:
//!
//! - **Compile-time inventory** ([`KERNEL_MODULES`]) is the set of
//!   things the binary CAN run.
//! - **Substrate-tracked enable/disable** (`attr_module_status`) is
//!   the set of things the binary WILL run this boot. Default on
//!   first encounter: enabled (installed = enabled).
//! - **Topo-sorted startup** by `depends_on`, with failure isolation:
//!   a failed module is captured + visible; its dependents are
//!   SKIPPED (not failed); boot CONTINUES with everything independent.
//!   The system always boots as much as it can.
//!
//! Boot sequence ([`boot`]):
//!
//! 1. Emit the one `system_boot` telemetry event per real boot.
//! 2. Seed the kernel's [`REQUIRED_METAMODEL_TYPES`].
//! 3. Snapshot [`KERNEL_MODULES`]; refuse duplicate names.
//! 4. Seed each descriptor's `required_metamodel`; register every
//!    module in the substrate registry.
//! 5. Read enable/disable intent; disabled modules are not attempted.
//! 6. Topo-sort by `depends_on` (Kahn). Cycle members are
//!    `Failed { failed_during: "topo_sort" }`; modules stuck behind
//!    them are skipped.
//! 7. Walk the order: `mark_starting` → `startup()` → `mark_active`
//!    or `mark_failed`. Panics are caught (each startup runs in its
//!    own task) and recorded as failures.
//!
//! Every transition is a `state_ledger` row + a `telemetry_stream`
//! event — the full boot is reconstructable from the substrate alone.
//! The returned [`BootReport`] is a convenience snapshot for CLI
//! rendering, not the source of truth.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Instant;

use linkme::distributed_slice;

use crate::error::Result;
use crate::lifecycle::LifecycleState;
use crate::metamodel::REQUIRED_METAMODEL_TYPES;
use crate::registry::{
    KernelModule, KernelModuleDescriptor, ModuleStatus, NodeKind, KERNEL_MODULES,
};
use crate::substrate::Kernel;

/// Outcome of one boot pass over the inventory.
#[derive(Debug, Clone)]
pub struct BootReport {
    /// One entry per inventory descriptor, in boot order (modules that
    /// never reached the startup walk — duplicates, cycle members —
    /// appear after the walked ones).
    pub entries: Vec<BootEntry>,
}

impl BootReport {
    /// Count of entries whose outcome is `Active`.
    #[must_use]
    pub fn active_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| matches!(e.outcome, LifecycleState::Active { .. }))
            .count()
    }
}

/// Boot outcome for one registered module / adapter.
#[derive(Debug, Clone)]
pub struct BootEntry {
    pub name: String,
    pub kind: NodeKind,
    pub version: String,
    /// Where this module ended up — the substrate lifecycle vocabulary,
    /// so the report and `superx status` speak the same language.
    pub outcome: LifecycleState,
}

/// The kernel's own registry entry. Global kernel parameters hang off
/// this entity (see `parameters.rs`); its `startup()` is a no-op —
/// the kernel is what invokes startups.
pub struct KernelCore;

#[async_trait::async_trait]
impl KernelModule for KernelCore {
    fn descriptor(&self) -> KernelModuleDescriptor {
        KernelModuleDescriptor {
            name: "kernel",
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
static KERNEL_CORE_REGISTRATION: &'static (dyn KernelModule + Sync) = &KernelCore;

/// Run one full boot pass: seed → register → resolve order → start.
///
/// Idempotent: re-running re-registers (descriptor supersession),
/// re-seeds (idempotent CREATEs), and re-walks startups — modules are
/// required by the [`KernelModule`] contract to have idempotent
/// `startup()`.
///
/// # Errors
///
/// Returns an error only for substrate-level failures of the boot
/// machinery itself (metamodel seeding, lifecycle writes). Per-module
/// problems never abort the boot; they are isolated into that module's
/// [`BootEntry`] outcome.
pub async fn boot(kernel: &Kernel) -> Result<BootReport> {
    tracing::info!("boot starting");

    // 1. One system_boot event per REAL boot — the only place it is
    //    emitted; readers must not mutate the stream they observe.
    {
        let mut payload = surrealdb::types::Object::new();
        payload.insert(
            "service_account".to_string(),
            surrealdb::types::Value::String("superx_kernel".to_string()),
        );
        payload.insert(
            "endpoint".to_string(),
            surrealdb::types::Value::String(
                kernel.endpoint().unwrap_or("(direct handle)").to_string(),
            ),
        );
        kernel
            .log_telemetry("system_boot", surrealdb::types::Value::Object(payload), None)
            .await?;
    }

    // 2. Kernel metamodel.
    for t in REQUIRED_METAMODEL_TYPES {
        kernel
            .ensure_type_definition(t.uid, t.category, t.memory_tier)
            .await?;
    }

    // 3. Inventory snapshot + duplicate-name guard.
    let mut seen = BTreeSet::new();
    let mut modules: Vec<(&'static (dyn KernelModule + Sync), KernelModuleDescriptor)> =
        Vec::new();
    let mut entries: Vec<BootEntry> = Vec::new();
    for module in KERNEL_MODULES {
        let desc = module.descriptor();
        if !seen.insert(desc.name) {
            entries.push(BootEntry {
                name: desc.name.to_string(),
                kind: desc.kind,
                version: desc.version.to_string(),
                outcome: LifecycleState::Failed {
                    error: format!("duplicate module name '{}' in inventory", desc.name),
                    failed_during: "inventory".to_string(),
                },
            });
            continue;
        }
        modules.push((*module, desc));
    }

    // 4. Per-module metamodel + registration; failures isolated.
    let mut outcomes: BTreeMap<&'static str, LifecycleState> = BTreeMap::new();
    let mut registered: Vec<(&'static (dyn KernelModule + Sync), KernelModuleDescriptor)> =
        Vec::new();
    for (module, desc) in modules {
        let mut seed_err = None;
        for t in desc.required_metamodel {
            if let Err(e) = kernel
                .ensure_type_definition(t.uid, t.category, t.memory_tier)
                .await
            {
                seed_err = Some(e);
                break;
            }
        }
        let result = match seed_err {
            Some(e) => Err(e),
            None => kernel.register_module(&desc).await.map(|_| ()),
        };
        match result {
            Ok(()) => registered.push((module, desc)),
            Err(e) => {
                tracing::warn!(module = desc.name, error = %e, "module registration failed");
                outcomes.insert(
                    desc.name,
                    LifecycleState::Failed {
                        error: e.to_string(),
                        failed_during: "registration".to_string(),
                    },
                );
                entries.push(BootEntry {
                    name: desc.name.to_string(),
                    kind: desc.kind,
                    version: desc.version.to_string(),
                    outcome: outcomes[desc.name].clone(),
                });
            }
        }
    }

    // 5. Operator enable/disable intent from the substrate.
    let mut disabled: BTreeSet<&'static str> = BTreeSet::new();
    for (_, desc) in &registered {
        if kernel.module_status(desc.kind, desc.name).await? == Some(ModuleStatus::Disabled) {
            disabled.insert(desc.name);
            outcomes.insert(desc.name, LifecycleState::Disabled);
        }
    }

    // 6. Kahn topo-sort over the registered set.
    let names: BTreeSet<&'static str> = registered.iter().map(|(_, d)| d.name).collect();
    let mut indegree: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut dependents: BTreeMap<&'static str, Vec<&'static str>> = BTreeMap::new();
    for (_, desc) in &registered {
        let known_deps = desc
            .depends_on
            .iter()
            .filter(|d| names.contains(**d))
            .count();
        indegree.insert(desc.name, known_deps);
        for dep in desc.depends_on {
            if names.contains(*dep) {
                dependents.entry(dep).or_default().push(desc.name);
            }
        }
    }
    let mut queue: VecDeque<&'static str> = indegree
        .iter()
        .filter(|(_, deg)| **deg == 0)
        .map(|(n, _)| *n)
        .collect();
    let mut order: Vec<&'static str> = Vec::with_capacity(registered.len());
    while let Some(n) = queue.pop_front() {
        order.push(n);
        for dependent in dependents.get(n).map(Vec::as_slice).unwrap_or(&[]) {
            let deg = indegree
                .get_mut(dependent)
                .expect("dependent is a registered name");
            *deg -= 1;
            if *deg == 0 {
                queue.push_back(dependent);
            }
        }
    }

    // Anything Kahn never emitted is on a cycle or stuck behind one.
    let stuck: BTreeSet<&'static str> = names
        .iter()
        .copied()
        .filter(|n| !order.contains(n))
        .collect();
    let by_name: BTreeMap<&'static str, &KernelModuleDescriptor> =
        registered.iter().map(|(_, d)| (d.name, d)).collect();
    for &name in &stuck {
        let on_cycle = reaches_itself(name, &stuck, &by_name);
        let state = if on_cycle {
            kernel
                .mark_failed(by_name[name].kind, name, "dependency cycle", "topo_sort")
                .await?;
            LifecycleState::Failed {
                error: "dependency cycle".to_string(),
                failed_during: "topo_sort".to_string(),
            }
        } else {
            kernel
                .mark_skipped(by_name[name].kind, name, "stuck behind a dependency cycle")
                .await?;
            LifecycleState::Skipped {
                reason: "stuck behind a dependency cycle".to_string(),
            }
        };
        outcomes.insert(name, state);
    }

    // 7. The startup walk.
    let module_by_name: BTreeMap<&'static str, &'static (dyn KernelModule + Sync)> =
        registered.iter().map(|(m, d)| (d.name, *m)).collect();
    for name in order {
        let desc = by_name[name];
        if disabled.contains(name) {
            continue;
        }
        let blocking = desc.depends_on.iter().find(|dep| {
            !matches!(outcomes.get(**dep), Some(LifecycleState::Active { .. }))
        });
        if let Some(dep) = blocking {
            let reason = if names.contains(*dep) {
                format!(
                    "dependency '{dep}' is {}",
                    outcomes
                        .get(*dep)
                        .map_or("not started", LifecycleState::short_tag)
                )
            } else {
                format!("depends on unknown module '{dep}'")
            };
            kernel.mark_skipped(desc.kind, name, &reason).await?;
            outcomes.insert(name, LifecycleState::Skipped { reason });
            continue;
        }

        kernel.mark_starting(desc.kind, name).await?;
        let started = Instant::now();
        let outcome = run_startup(kernel, module_by_name[name]).await;
        let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let state = match outcome {
            Ok(()) => {
                tracing::info!(module = name, elapsed_ms, "module active");
                kernel.mark_active(desc.kind, name, elapsed_ms).await?;
                LifecycleState::Active {
                    startup_duration_ms: elapsed_ms,
                }
            }
            Err(error) => {
                tracing::warn!(module = name, %error, "module startup failed");
                kernel.mark_failed(desc.kind, name, &error, "startup").await?;
                LifecycleState::Failed {
                    error,
                    failed_during: "startup".to_string(),
                }
            }
        };
        outcomes.insert(name, state);
    }

    // Assemble the report in inventory order for the registered set.
    for (_, desc) in &registered {
        entries.push(BootEntry {
            name: desc.name.to_string(),
            kind: desc.kind,
            version: desc.version.to_string(),
            outcome: outcomes
                .get(desc.name)
                .cloned()
                .unwrap_or(LifecycleState::Enabled),
        });
    }
    tracing::info!(
        active = entries
            .iter()
            .filter(|e| matches!(e.outcome, LifecycleState::Active { .. }))
            .count(),
        total = entries.len(),
        "boot complete"
    );
    Ok(BootReport { entries })
}

/// Run one module's `startup()` in its own task so a panic is caught
/// and isolated instead of tearing down the whole boot.
async fn run_startup(
    kernel: &Kernel,
    module: &'static (dyn KernelModule + Sync),
) -> std::result::Result<(), String> {
    let kernel = kernel.clone();
    let handle = tokio::spawn(async move { module.startup(&kernel).await });
    match handle.await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.to_string()),
        Err(join_err) if join_err.is_panic() => Err(format!("panicked: {join_err}")),
        Err(join_err) => Err(format!("startup task aborted: {join_err}")),
    }
}

/// DFS within the stuck set: can `start` reach itself following
/// `depends_on` edges? Yes → cycle member; no → merely stuck behind
/// one.
fn reaches_itself(
    start: &str,
    stuck: &BTreeSet<&'static str>,
    by_name: &BTreeMap<&'static str, &KernelModuleDescriptor>,
) -> bool {
    let mut visited = BTreeSet::new();
    let mut stack: Vec<&str> = by_name[start]
        .depends_on
        .iter()
        .copied()
        .filter(|d| stuck.contains(d))
        .collect();
    while let Some(n) = stack.pop() {
        if n == start {
            return true;
        }
        if !visited.insert(n) {
            continue;
        }
        if let Some(desc) = by_name.get(n) {
            stack.extend(
                desc.depends_on
                    .iter()
                    .copied()
                    .filter(|d| stuck.contains(d)),
            );
        }
    }
    false
}
