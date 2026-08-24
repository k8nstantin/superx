//! Wave planning (epic #189 S3/D25, phase R2 per spec #192 as
//! amended): parse the entities module's graph JSON, layer the task
//! nodes over `depends_on` edges, refuse cycles. Pure computation —
//! zero writes, zero telemetry.

use std::collections::{HashMap, HashSet};

use superx_kernel::{Kernel, KernelError, Result};


/// Subgraph resolution depth for planning (epic S2).
pub const PLAN_DEPTH_PARAM: &str = "attr_runner_plan_depth";
const DEFAULT_PLAN_DEPTH: usize = 20; // skill-allow: §9-const — bootstrap fallback, param-overridable (attr_runner_plan_depth)

/// One node of the entities graph JSON contract (#192 amendment).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct GraphNode {
    pub uid: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    pub content: Option<String>,
    pub attributes: Option<serde_json::Value>,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub depth: usize,
}

/// One edge of the graph JSON contract.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct GraphEdge {
    pub edge_uid: String,
    pub from: String,
    pub to: String,
    pub rel: String,
}

/// The parsed subgraph.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Graph {
    pub root: String,
    #[serde(default)]
    pub truncated_at_depth: bool,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

/// A task scheduled into a wave.
#[derive(Debug, Clone)]
pub struct TaskRef {
    pub uid: String,
    pub name: String,
}

/// The execution plan: waves in order; tasks inside a wave are
/// independent and dispatch in parallel (D25).
#[derive(Debug)]
pub struct Plan {
    pub waves: Vec<Vec<TaskRef>>,
    pub warnings: Vec<String>,
}

/// Resolve the target's subgraph through the entities module (D24)
/// and parse the JSON contract.
///
/// # Errors
///
/// [`KernelError::Module`] when the entities module refuses (its
/// error verbatim) or the JSON does not parse.
pub async fn fetch_graph(kernel: &Kernel, fragment: &str, depth: usize) -> Result<Graph> {
    let raw = superx_ops::run_module_cli(
        kernel,
        &[
            "entities".to_string(),
            "graph".to_string(),
            fragment.to_string(),
            "--json".to_string(),
            "--depth".to_string(),
            depth.to_string(),
        ],
    )
    .await
    .map_err(KernelError::Module)?;
    serde_json::from_str(&raw)
        .map_err(|e| KernelError::Module(format!("unparseable entities graph JSON: {e}")))
}

/// The planning depth: the module's own setting (#284), falling back to
/// a kernel parameter set before the module owned its settings, and
/// adopting it so the next read finds it here.
pub async fn resolved_plan_depth(kernel: &Kernel) -> usize {
    if let Some(own) = crate::params::load(kernel).plan_depth {
        return own.max(1);
    }
    match crate::daemon::kernel_u64(kernel, PLAN_DEPTH_PARAM).await {
        Some(set) => {
            let adopted = usize::try_from(set).unwrap_or(DEFAULT_PLAN_DEPTH).max(1);
            crate::params::adopt(kernel, |s| s.plan_depth = Some(adopted));
            adopted
        }
        None => DEFAULT_PLAN_DEPTH,
    }
}

/// Layer the graph's task nodes into execution waves (Kahn over
/// `depends_on`). Only `task` nodes execute; everything else is
/// context. A `depends_on` whose target is not a task in the
/// subgraph is a WARNING and counts as satisfied (epic S3).
///
/// # Errors
///
/// [`KernelError::Module`] naming the cycle path when `depends_on`
/// edges cycle among tasks.
pub fn compute_waves(graph: &Graph) -> Result<Plan> {
    let tasks: HashMap<&str, &GraphNode> = graph
        .nodes
        .iter()
        .filter(|n| n.kind == "task")
        .map(|n| (n.uid.as_str(), n))
        .collect();

    let mut warnings = Vec::new();
    // deps[task] = the in-subgraph task uids it waits on.
    let mut deps: HashMap<&str, HashSet<&str>> = tasks.keys().map(|&k| (k, HashSet::new())).collect();
    for edge in graph.edges.iter().filter(|e| e.rel == "depends_on") {
        let Some(source) = tasks.get(edge.from.as_str()) else { continue };
        if tasks.contains_key(edge.to.as_str()) {
            deps.get_mut(edge.from.as_str())
                .expect("source is a task")
                .insert(edge.to.as_str());
        } else {
            warnings.push(format!(
                "task '{}' depends_on {} — not a task in this subgraph; treated satisfied",
                source.name, edge.to
            ));
        }
    }

    let mut waves: Vec<Vec<TaskRef>> = Vec::new();
    let mut placed: HashSet<&str> = HashSet::new();
    while placed.len() < tasks.len() {
        let mut wave: Vec<&str> = deps
            .iter()
            .filter(|(uid, waiting)| {
                !placed.contains(**uid) && waiting.iter().all(|d| placed.contains(d))
            })
            .map(|(uid, _)| *uid)
            .collect();
        if wave.is_empty() {
            let remaining: Vec<&str> = tasks.keys().filter(|u| !placed.contains(**u)).copied().collect();
            return Err(KernelError::Module(format!(
                "depends_on cycle among tasks: {}",
                cycle_path(&deps, &remaining, &tasks)
            )));
        }
        wave.sort_unstable(); // uuid7 order = creation order, stable output
        waves.push(
            wave.iter()
                .map(|uid| TaskRef {
                    uid: (*uid).to_string(),
                    name: tasks[uid].name.clone(),
                })
                .collect(),
        );
        placed.extend(wave);
    }

    Ok(Plan { waves, warnings })
}

/// Walk the remaining tasks' dep edges until a repeat names the cycle.
fn cycle_path(
    deps: &HashMap<&str, HashSet<&str>>,
    remaining: &[&str],
    tasks: &HashMap<&str, &GraphNode>,
) -> String {
    let Some(start) = remaining.first() else { return "(unlocatable)".to_string() };
    let mut path: Vec<&str> = vec![start];
    let mut seen: HashSet<&str> = HashSet::from([*start]);
    let mut here = *start;
    while let Some(next) = deps.get(here).and_then(|d| d.iter().next()) {
        path.push(next);
        if !seen.insert(next) {
            break;
        }
        here = next;
    }
    path.iter()
        .map(|uid| tasks.get(uid).map_or(*uid, |n| n.name.as_str()))
        .collect::<Vec<_>>()
        .join(" → ")
}

/// Render a plan for the terminal (`superx runner plan`).
#[must_use]
pub fn render_plan(plan: &Plan) -> String {
    let mut out = String::new();
    for warning in &plan.warnings {
        out.push_str(&format!("WARNING: {warning}\n"));
    }
    let total: usize = plan.waves.iter().map(Vec::len).sum();
    for (i, wave) in plan.waves.iter().enumerate() {
        let parallel = if wave.len() > 1 { "  (parallel)" } else { "" };
        out.push_str(&format!("wave {}:{parallel}\n", i + 1));
        for task in wave {
            out.push_str(&format!("  {}  {}\n", &task.uid[..task.uid.len().min(13)], task.name));
        }
    }
    out.push_str(&format!("{total} tasks in {} waves\n", plan.waves.len()));
    out
}
