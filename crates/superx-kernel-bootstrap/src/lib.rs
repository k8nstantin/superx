//! SuperX boot orchestrator — the L1 kernel module that turns the
//! compile-time inventory into a running system.
//!
//! OS-style boot semantics (locked canon §11):
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
//! Boot sequence ([`bootstrap`]):
//!
//! 1. Seed the kernel's [`REQUIRED_METAMODEL_TYPES`].
//! 2. Snapshot [`KERNEL_MODULES`]; refuse duplicate names
//!    (report-level failure, first registration wins).
//! 3. Seed each descriptor's `required_metamodel`; register every
//!    module in the substrate registry.
//! 4. Read each module's `attr_module_status`; disabled modules are
//!    not attempted and their dependents are skipped.
//! 5. Topo-sort by `depends_on` (Kahn). Cycle members are marked
//!    `Failed { failed_during: "topo_sort" }`; everything stuck
//!    behind them is skipped.
//! 6. Walk the order: `mark_starting` → `startup()` → `mark_active`
//!    (with measured duration) or `mark_failed`. Panics in
//!    `startup()` are caught (the future runs in its own task) and
//!    recorded as failures.
//!
//! Every transition is a `state_ledger` row + a `telemetry_stream`
//! event via the kernel lifecycle verbs — the full boot is
//! reconstructable from the substrate alone. The returned
//! [`BootReport`] is a convenience snapshot for CLI rendering, not
//! the source of truth.
//!
//! Graceful-shutdown walking (reverse topo `shutdown()`) ships with
//! the first long-running module (the F10 capture loop), where there
//! is something real to tear down.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Instant;

use linkme::distributed_slice;
use superx_kernel::{
    Kernel, KernelModule, KernelModuleDescriptor, LifecycleState, ModuleStatus, NodeKind,
    Result, KERNEL_MODULES, REQUIRED_METAMODEL_TYPES,
};

/// Outcome of one boot pass over the inventory.
#[derive(Debug, Clone)]
pub struct BootReport {
    /// One entry per inventory descriptor, in boot order (modules
    /// that never reached the startup walk — duplicates, cycle
    /// members, disabled — appear after the walked ones).
    pub entries: Vec<BootEntry>,
}

impl BootReport {
    /// Convenience: count of entries whose outcome is `Active`.
    #[must_use]
    pub fn active_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| matches!(e.outcome, LifecycleState::Active { .. }))
            .count()
    }
}

/// Boot outcome for one registered module / driver / app.
#[derive(Debug, Clone)]
pub struct BootEntry {
    /// `descriptor.name`.
    pub name: String,
    /// `descriptor.kind`.
    pub kind: NodeKind,
    /// `descriptor.category`.
    pub category: String,
    /// `descriptor.version`.
    pub version: String,
    /// Where this module ended up — reuses the substrate lifecycle
    /// vocabulary ([`LifecycleState`]) so the report and
    /// `superx kernel modules list` always speak the same language.
    pub outcome: LifecycleState,
}

/// The bootstrap orchestrator registers itself in the inventory like
/// every other kernel module — it appears in
/// `superx kernel modules list` and carries its own lifecycle rows.
/// Its `startup()` is a no-op: the orchestration work happens in
/// [`bootstrap`], which is what *invokes* startups.
pub struct BootstrapModule;

#[async_trait::async_trait]
impl KernelModule for BootstrapModule {
    fn descriptor(&self) -> KernelModuleDescriptor {
        KernelModuleDescriptor {
            name: "bootstrap",
            version: env!("CARGO_PKG_VERSION"),
            kind: NodeKind::KernelModule,
            category: "",
            depends_on: &[],
            required_metamodel: &[],
        }
    }

    async fn startup(&self, _kernel: &Kernel) -> Result<()> {
        Ok(())
    }
}

#[distributed_slice(KERNEL_MODULES)]
static BOOTSTRAP_REGISTRATION: &'static (dyn KernelModule + Sync) = &BootstrapModule;

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
/// machinery itself (seeding the kernel metamodel, lifecycle writes).
/// Per-module problems — registration failure, startup error or
/// panic, dependency cycles, unknown dependencies — never abort the
/// boot; they are isolated into that module's [`BootEntry`] outcome.
pub async fn bootstrap(kernel: &Kernel) -> Result<BootReport> {
    // 1. Kernel metamodel.
    for t in REQUIRED_METAMODEL_TYPES {
        kernel
            .ensure_type_definition(t.uid, t.category, t.memory_tier)
            .await?;
    }

    // 2. Inventory snapshot + duplicate-name guard.
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
                category: desc.category.to_string(),
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

    // 3. Per-module metamodel + registration. Registration failures
    //    are isolated (report-only — there is no entity to mark).
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
                    category: desc.category.to_string(),
                    version: desc.version.to_string(),
                    outcome: outcomes[desc.name].clone(),
                });
            }
        }
    }

    // 4. Operator enable/disable from the substrate.
    let mut disabled: BTreeSet<&'static str> = BTreeSet::new();
    for (_, desc) in &registered {
        if kernel.module_status(desc.kind, desc.name).await? == Some(ModuleStatus::Disabled) {
            disabled.insert(desc.name);
            outcomes.insert(desc.name, LifecycleState::Disabled);
        }
    }

    // 5. Kahn topo-sort over the registered set. Dependencies may
    //    point at any registered name; unknown names surface as a
    //    skip reason during the walk.
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

    // Anything Kahn never emitted is on a dependency cycle or stuck
    // behind one. Distinguish members from victims: a member can
    // reach itself following depends_on edges within the stuck set.
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
            LifecycleState::Failed {
                error: "dependency cycle".to_string(),
                failed_during: "topo_sort".to_string(),
            }
        } else {
            LifecycleState::Skipped {
                reason: "stuck behind a dependency cycle".to_string(),
            }
        };
        if on_cycle {
            kernel
                .mark_failed(
                    by_name[name].kind,
                    name,
                    "dependency cycle",
                    "topo_sort",
                )
                .await?;
        } else {
            kernel
                .mark_skipped(by_name[name].kind, name, "stuck behind a dependency cycle")
                .await?;
        }
        outcomes.insert(name, state);
    }

    // 6. The startup walk.
    let module_by_name: BTreeMap<&'static str, &'static (dyn KernelModule + Sync)> =
        registered.iter().map(|(m, d)| (d.name, *m)).collect();
    for name in order {
        let desc = by_name[name];
        if disabled.contains(name) {
            // Operator said no. Lifecycle untouched; dependents skip.
            continue;
        }
        // Every dependency must be Active.
        let blocking = desc.depends_on.iter().find(|dep| {
            !matches!(
                outcomes.get(**dep),
                Some(LifecycleState::Active { .. })
            )
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
                kernel.mark_active(desc.kind, name, elapsed_ms).await?;
                LifecycleState::Active {
                    startup_duration_ms: elapsed_ms,
                }
            }
            Err(error) => {
                kernel
                    .mark_failed(desc.kind, name, &error, "startup")
                    .await?;
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
            category: desc.category.to_string(),
            version: desc.version.to_string(),
            outcome: outcomes
                .get(desc.name)
                .cloned()
                .unwrap_or(LifecycleState::Enabled),
        });
    }
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
/// `depends_on` edges? If yes it is on a cycle; if no it is merely
/// stuck behind one.
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
