//! The firing engine (epic #189 S3/D26/D27, phase R3): re-plan at
//! every frontier re-evaluation, dispatch runnable tasks in parallel
//! up to the ceiling, spawn the operator-configured agent command,
//! write results back into the graph, record every fact as run rows.

use std::collections::{HashMap, HashSet};

use async_trait::async_trait;
use superx_kernel::{Db, Kernel, KernelError, Result};

use crate::plan::{Graph, GraphNode};
use crate::schedule::ScheduleRow;
use crate::{run, schedule};

/// Cross-module I/O, abstracted so the engine is testable without a
/// provisioned instance. The live implementation speaks to the
/// entities module through its CLI via the kernel dispatch (D24).
#[async_trait]
pub trait Exchange: Send + Sync {
    /// The target's current subgraph (re-read every frontier, D27).
    async fn graph(&self, root: &str, depth: usize) -> Result<Graph>;
    /// Write a task's output into the graph as a `produced` text
    /// node; returns the text node's uuid.
    async fn write_back(&self, task_uid: &str, output: &str) -> Result<String>;
}

/// The live exchange: entities-module CLI through kernel dispatch.
pub struct CliExchange {
    pub kernel: Kernel,
}

#[async_trait]
impl Exchange for CliExchange {
    async fn graph(&self, root: &str, depth: usize) -> Result<Graph> {
        crate::plan::fetch_graph(&self.kernel, root, depth).await
    }

    async fn write_back(&self, task_uid: &str, output: &str) -> Result<String> {
        let created = superx_ops::run_module_cli(
            &self.kernel,
            &[
                "entities".to_string(),
                "create".to_string(),
                "--type".to_string(),
                "text".to_string(),
                "--content".to_string(),
                output.to_string(),
                "result".to_string(),
            ],
        )
        .await
        .map_err(KernelError::Module)?;
        let text_uuid = created.trim().to_string();
        superx_ops::run_module_cli(
            &self.kernel,
            &[
                "entities".to_string(),
                "link".to_string(),
                task_uid.to_string(),
                text_uuid.clone(),
                "--rel".to_string(),
                "produced".to_string(),
            ],
        )
        .await
        .map_err(KernelError::Module)?;
        Ok(text_uuid)
    }
}

/// Everything one firing needs, resolved by the caller (the daemon
/// loop resolves parameters; tests pass values directly).
pub struct FiringConfig {
    /// `attr_runner_agent_cmd` — None means every dispatch fails
    /// loudly with the exact detail string (epic S2: no default).
    pub agent_cmd: Option<String>,
    pub max_parallel: usize,
    pub plan_depth: usize,
}

/// What a firing did (telemetry + tests).
#[derive(Debug)]
pub struct FiringReport {
    pub firing: String,
    pub done: usize,
    pub failed: usize,
    pub cancelled: bool,
}

/// The exact detail string for an unset agent command (spec #193).
pub const UNSET_CMD_DETAIL: &str = "attr_runner_agent_cmd not set";

/// Execute one due schedule end-to-end (S3 firing lifecycle):
/// append `fired`, then re-plan/dispatch until the graph is spent or
/// the chain is cancelled.
///
/// # Errors
///
/// [`superx_kernel::KernelError`] for substrate failures; per-task
/// agent failures are recorded as run rows, never propagated.
pub async fn fire(
    kernel: &Kernel,
    db: &Db,
    exchange: &dyn Exchange,
    config: &FiringConfig,
    due: &ScheduleRow,
) -> Result<FiringReport> {
    // The firing exists on the intent chain first.
    crate::schedule::append_status(db, due, "fired").await?;
    let firing = uuid::Uuid::now_v7().to_string();

    let mut done: HashSet<String> = HashSet::new();
    let mut failed: HashSet<String> = HashSet::new();
    let mut dispatched: HashSet<String> = HashSet::new();
    let mut in_flight: tokio::task::JoinSet<TaskOutcome> = tokio::task::JoinSet::new();
    let mut cancelled = false;
    let mut first_graph_tasks = 0usize;

    emit(kernel, "firing_started", serde_json::json!({
        "schedule": due.uid, "firing": firing, "entity": due.entity,
    })).await;

    loop {
        // D27: the graph is re-read every re-evaluation; cancel is
        // checked before every dispatch round.
        if let Some(current) = schedule::chain_current(db, &due.uid).await? {
            if current.status == "cancelled" {
                cancelled = true;
            }
        }
        if !cancelled {
            let graph = exchange.graph(&due.entity, config.plan_depth).await?;
            let (tasks, deps, _warnings) = task_dependencies(&graph);
            first_graph_tasks = first_graph_tasks.max(tasks.len());
            let runnable: Vec<String> = tasks
                .keys()
                .filter(|uid| {
                    !dispatched.contains(**uid)
                        && deps[**uid].iter().all(|d| done.contains(*d))
                        && !deps[**uid].iter().any(|d| failed.contains(*d))
                })
                .map(|uid| (*uid).to_string())
                .collect();
            let mut runnable = runnable;
            runnable.sort_unstable();

            for task_uid in runnable {
                if in_flight.len() >= config.max_parallel {
                    break;
                }
                dispatched.insert(task_uid.clone());
                let run_uid = uuid::Uuid::now_v7().to_string();
                let (prompt, instruct_version) = build_prompt(&graph, &task_uid);
                run::append_run_row(
                    db, &run_uid, &due.uid, &firing, &task_uid,
                    "dispatched", instruct_version.as_deref(), None, None,
                )
                .await?;
                emit(kernel, "run_dispatched", serde_json::json!({
                    "firing": firing, "task": task_uid,
                    "instruct_version": instruct_version,
                })).await;

                match &config.agent_cmd {
                    None => {
                        run::append_run_row(
                            db, &run_uid, &due.uid, &firing, &task_uid,
                            "failed", instruct_version.as_deref(), None,
                            Some(UNSET_CMD_DETAIL),
                        )
                        .await?;
                        emit(kernel, "run_failed", serde_json::json!({
                            "firing": firing, "task": task_uid, "detail": UNSET_CMD_DETAIL,
                        })).await;
                        failed.insert(task_uid);
                    }
                    Some(cmd) => {
                        let cmd = cmd.clone();
                        let task = task_uid.clone();
                        let iv = instruct_version.clone();
                        in_flight.spawn(async move {
                            (task, run_uid, iv, spawn_agent(&cmd, &prompt).await)
                        });
                    }
                }
            }
        }

        if in_flight.is_empty() {
            break;
        }
        // Await ONE completion, then re-evaluate the frontier (D27).
        if let Some(Ok((task_uid, run_uid, instruct_version, outcome))) = in_flight.join_next().await {
            match outcome {
                Ok(stdout) => match exchange.write_back(&task_uid, &stdout).await {
                    Ok(text_uuid) => {
                        run::append_run_row(
                            db, &run_uid, &due.uid, &firing, &task_uid,
                            "done", instruct_version.as_deref(), Some(&text_uuid), None,
                        )
                        .await?;
                        emit(kernel, "run_done", serde_json::json!({
                            "firing": firing, "task": task_uid, "output_ref": text_uuid,
                        })).await;
                        done.insert(task_uid);
                    }
                    Err(e) => {
                        let detail = format!("write-back failed: {e}");
                        run::append_run_row(
                            db, &run_uid, &due.uid, &firing, &task_uid,
                            "failed", instruct_version.as_deref(), None, Some(&detail),
                        )
                        .await?;
                        emit(kernel, "run_failed", serde_json::json!({
                            "firing": firing, "task": task_uid, "detail": detail,
                        })).await;
                        failed.insert(task_uid);
                    }
                },
                Err(detail) => {
                    run::append_run_row(
                        db, &run_uid, &due.uid, &firing, &task_uid,
                        "failed", instruct_version.as_deref(), None, Some(&detail),
                    )
                    .await?;
                    emit(kernel, "run_failed", serde_json::json!({
                        "firing": firing, "task": task_uid, "detail": detail,
                    })).await;
                    failed.insert(task_uid);
                }
            }
        }
    }

    emit(kernel, "firing_complete", serde_json::json!({
        "schedule": due.uid, "firing": firing,
        "done": done.len(), "failed": failed.len(),
        "tasks_seen": first_graph_tasks, "cancelled": cancelled,
    })).await;

    Ok(FiringReport { firing, done: done.len(), failed: failed.len(), cancelled })
}

/// (task uid, run uid, pinned instruct version, agent outcome).
type TaskOutcome = (String, String, Option<String>, std::result::Result<String, String>);
/// Task nodes by uid + per-task dependency sets + warnings.
type TaskDeps<'g> = (
    HashMap<&'g str, &'g GraphNode>,
    HashMap<&'g str, HashSet<&'g str>>,
    Vec<String>,
);

/// Tasks + their in-subgraph task dependencies (shared with plan).
fn task_dependencies(graph: &Graph) -> TaskDeps<'_> {
    let tasks: HashMap<&str, &GraphNode> = graph
        .nodes
        .iter()
        .filter(|n| n.kind == "task")
        .map(|n| (n.uid.as_str(), n))
        .collect();
    let mut warnings = Vec::new();
    let mut deps: HashMap<&str, HashSet<&str>> = tasks.keys().map(|&k| (k, HashSet::new())).collect();
    for edge in graph.edges.iter().filter(|e| e.rel == "depends_on") {
        if !tasks.contains_key(edge.from.as_str()) {
            continue;
        }
        if tasks.contains_key(edge.to.as_str()) {
            deps.get_mut(edge.from.as_str()).expect("task").insert(edge.to.as_str());
        } else {
            warnings.push(format!("{} depends_on non-task {}", edge.from, edge.to));
        }
    }
    (tasks, deps, warnings)
}

/// Assemble the agent prompt (epic S3, exact section order) and pin
/// the dispatched instructs version (D27).
#[must_use]
pub fn build_prompt(graph: &Graph, task_uid: &str) -> (String, Option<String>) {
    let node_by_uid: HashMap<&str, &GraphNode> =
        graph.nodes.iter().map(|n| (n.uid.as_str(), n)).collect();
    let task = node_by_uid.get(task_uid);
    let mut out = String::new();
    out.push_str(&format!(
        "Task: {}\n",
        task.map_or(task_uid, |t| t.name.as_str())
    ));

    let mut instruct_version = None;
    for edge in graph.edges.iter().filter(|e| e.from == task_uid && e.rel == "instructs") {
        if let Some(text) = node_by_uid.get(edge.to.as_str()) {
            if let Some(content) = &text.content {
                out.push_str(&format!("\nInstructions:\n{content}\n"));
            }
            instruct_version = Some(text.version.clone());
        }
    }

    for edge in graph.edges.iter().filter(|e| e.from == graph.root && e.rel == "describes") {
        if let Some(text) = node_by_uid.get(edge.to.as_str()) {
            if let Some(content) = &text.content {
                out.push_str(&format!("\nAbout the product:\n{content}\n"));
            }
        }
    }

    let mut context = String::new();
    for edge in graph.edges.iter().filter(|e| e.from == task_uid && e.rel != "instructs") {
        if let Some(node) = node_by_uid.get(edge.to.as_str()) {
            context.push_str(&format!(
                "- [{}] {} ({}){}\n",
                edge.rel,
                node.name,
                node.kind,
                node.attributes
                    .as_ref()
                    .map(|a| format!(" attributes: {a}"))
                    .unwrap_or_default()
            ));
        }
    }
    if !context.is_empty() {
        out.push_str(&format!("\nLinked context:\n{context}"));
    }
    out.push_str("\nReport your result as plain text.\n");
    (out, instruct_version)
}

/// Spawn the agent command: whitespace-split template, prompt
/// appended as the final argument. Ok(stdout) on exit 0; Err(detail)
/// otherwise.
async fn spawn_agent(cmd_template: &str, prompt: &str) -> std::result::Result<String, String> {
    let parts: Vec<&str> = cmd_template.split_whitespace().collect();
    let Some(program) = parts.first() else {
        return Err(UNSET_CMD_DETAIL.to_string());
    };
    let output = tokio::process::Command::new(program)
        .args(&parts[1..])
        .arg(prompt)
        .output()
        .await
        .map_err(|e| format!("spawn '{program}' failed: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: String = stderr.chars().rev().take(500).collect::<String>().chars().rev().collect();
        Err(format!("exit {}: {tail}", output.status))
    }
}

/// Telemetry into the one firehose; failures never fail the firing.
async fn emit(kernel: &Kernel, event: &str, payload: serde_json::Value) {
    let subject = kernel
        .find_module_by_name(superx_kernel::NodeKind::KernelModule, crate::MODULE_NAME)
        .await
        .ok()
        .flatten();
    if let Err(e) = kernel
        .log_telemetry(event, superx_kernel::message::value_from_json(&payload), subject)
        .await
    {
        tracing::warn!(target: "runner", "telemetry write failed: {e}");
    }
}
