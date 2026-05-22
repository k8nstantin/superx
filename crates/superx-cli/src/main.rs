//! # superx-cli — the operator-facing CLI
//!
//! Implements **MVP capability C4** (`ARCHITECTURE.md` §0d): CLI parity across
//! every substrate verb. Drives the same kernel APIs as the MCP server but
//! through human-facing subcommands.
//!
//! ## Subcommands (v1.0)
//!
//! | Verb | Purpose |
//! | --- | --- |
//! | `bootstrap` | First-run provisioning + agent discovery |
//! | `graphify` | Walk a directory, ingest into the substrate as a DAG |
//! | `compile` | Distill context for an entity (tier-aware, optionally LLM-distilled) |
//! | `propose` | Run the proposer blade to suggest a structural edge |
//! | `evaluate` | Score a proposal via the Meta-Harness wasm sandbox |
//! | `promote` | Link a passing proposal to the tenant's substrate |
//! | `identify` | Agent handshake; returns a session uid |
//! | `list-agents` | Enumerate registered agents for the tenant |
//! | `list-tools` | Enumerate registered tools for the tenant |
//! | `demo` | One-shot bootstrap → ingest → propose → promote (direct API) |
//! | `runner-demo` | Same end-to-end flow but driven via schedule queue + runner |
//! | `stats` | Stream recent telemetry events |
//! | `runner` | Long-running background daemon: consume due `schedule` rows |
//! | `enqueue` | Add a `schedule` row for the runner to pick up |
//! | `schedule-list` | Show every schedule chain's current state |
//!
//! ## Telemetry-pipe lifetime
//!
//! Every command spawns a [`TelemetrySubscriber`] task at startup so any
//! telemetry the command produces flows to the configured Kafka / HTTP sinks
//! for the duration of the run. The MCP server (`superx-mcp` binary) does
//! the same — the difference is lifetime (command vs. server process).
//!
//! ## Environment variables
//!
//! - `SUPERX_DB_PATH` — substrate file path (default `./db/superx.db`)
//! - `SUPERX_NS` — `SurrealDB` namespace (default `superx`)
//! - `SUPERX_DB_NAME` — `SurrealDB` database name (default `main`)
//! - `SUPERX_KAFKA_BROKERS`, `SUPERX_KAFKA_TOPIC` — Kafka egress
//! - `SUPERX_EMISSION_API` — HTTP egress URL
//! - `RUST_LOG` — tracing filter (e.g. `info,superx_inference=debug`)
//!
//! Copyright (c) 2026 Constantin Alexander <constantin@dedomena.io>.
//! Licensed under the Apache License, Version 2.0.

#![deny(warnings)]
#![deny(clippy::pedantic)]

use superx_ingest::{UniversalIngestor, FileSource};
use superx_compiler::CompilerBlade;
use superx_harness::MetaHarness;
use superx_proposer::ProposerBlade;
use superx_bootstrap::BootstrapBlade;
use superx_kernel::{Kernel, DEFAULT_TENANT};
use superx_emission::{TelemetrySubscriber, ApiSink, KafkaSink};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
    /// Optional Kafka brokers for emission
    #[arg(long, env = "SUPERX_KAFKA_BROKERS")]
    kafka_brokers: Option<String>,
    /// Optional Kafka topic for emission
    #[arg(long, env = "SUPERX_KAFKA_TOPIC")]
    kafka_topic: Option<String>,
    /// Optional API URL for emission
    #[arg(long, env = "SUPERX_EMISSION_API")]
    emission_api: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Ingest a source into the `SuperX` substrate
    Graphify {
        #[arg(short, long)]
        path: PathBuf,
        #[arg(short, long, default_value = DEFAULT_TENANT)]
        tenant: String,
        #[arg(short, long)]
        run_id: Option<String>,
    },
    /// Compile context for a root entity
    Compile {
        #[arg(short, long)]
        root: String,
        #[arg(short, long, default_value = DEFAULT_TENANT)]
        tenant: String,
        #[arg(short, long)]
        model: Option<PathBuf>,
        #[arg(long)]
        tokenizer: Option<PathBuf>,
        #[arg(long)]
        tiers: Option<Vec<String>>,
    },
    /// Evaluate a proposal node using a Wasm harness
    Evaluate {
        #[arg(short, long)]
        proposal: String,
        #[arg(short, long)]
        harness: PathBuf,
        #[arg(short, long, default_value = DEFAULT_TENANT)]
        tenant: String,
    },
    /// Promote a proposal node if it passes threshold
    Promote {
        #[arg(short, long)]
        proposal: String,
        #[arg(short, long, default_value = "0.8")]
        threshold: f64,
        #[arg(short, long, default_value = DEFAULT_TENANT)]
        tenant: String,
    },
    /// Propose a relation between two nodes using an agent
    Propose {
        #[arg(short, long)]
        from: String,
        #[arg(short, long)]
        to: String,
        #[arg(short, long, default_value = DEFAULT_TENANT)]
        tenant: String,
        #[arg(short, long)]
        model: PathBuf,
        #[arg(short, long)]
        tokenizer: PathBuf,
    },
    /// Bootstrap the substrate for a tenant
    Bootstrap {
        #[arg(short, long, default_value = DEFAULT_TENANT)]
        tenant: String,
    },
    /// View the latest telemetry stats
    Stats {
        #[arg(short, long, default_value = DEFAULT_TENANT)]
        tenant: String,
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },
    /// Identify an agent and start a durable session (handshake)
    Identify {
        #[arg(short, long)]
        agent_uid: String,
        #[arg(short, long, default_value = DEFAULT_TENANT)]
        tenant: String,
    },
    /// List all agents registered in the substrate for the tenant
    ListAgents {
        #[arg(short, long, default_value = DEFAULT_TENANT)]
        tenant: String,
    },
    /// List all tools registered in the substrate (capability-gated targets)
    ListTools {
        #[arg(short, long, default_value = DEFAULT_TENANT)]
        tenant: String,
    },
    /// One-shot end-to-end demo: bootstrap -> ingest fixture -> propose -> promote
    Demo {
        #[arg(short, long, default_value = "demo")]
        tenant: String,
    },
    /// End-to-end demo that drives the pipeline through the SCHEDULE
    /// QUEUE + RUNNER (not the direct-API path). Bootstraps a tenant,
    /// creates a source-typed entity pointing at a temp fixture,
    /// enqueues a `kind=ingest` schedule row, ticks the runner with
    /// the real `KernelDispatcher` until the chain completes (or a
    /// safety bound trips), then prints the final chain state. Proves
    /// the full operator-facing pipeline works end-to-end.
    RunnerDemo {
        #[arg(short, long, default_value = "runner_demo")]
        tenant: String,
        /// Max ticks before giving up — bounded loop per NASA Rule 2.
        #[arg(long, default_value = "10")]
        max_ticks: u32,
    },
    /// Enqueue a `schedule` row for the runner to pick up. Outputs the
    /// resulting schedule chain's record-id literal on stdout. The target
    /// entity must already exist and (for kinds requiring per-target
    /// attrs like `ingest`) be populated with the relevant attribute
    /// rows before enqueueing.
    Enqueue {
        #[arg(short, long, default_value = DEFAULT_TENANT)]
        tenant: String,
        /// Schedule kind. Routed by `KernelDispatcher` — currently
        /// `compile`, `promote`, `ingest`.
        #[arg(short, long)]
        kind: String,
        /// Target entity (`entity:<uuid>` record-id literal).
        #[arg(long)]
        target: String,
        /// Run entity (`entity:<uuid>` record-id literal). If omitted,
        /// a fresh `node_run` is created under the tenant.
        #[arg(long)]
        run: Option<String>,
        /// JSON metadata blob to attach to the schedule row's `metadata`
        /// field. Defaults to `{}`.
        #[arg(long, default_value = "{}")]
        metadata: String,
    },
    /// List the current state of every schedule chain in the tenant —
    /// one line per chain showing its latest status row.
    ScheduleList {
        #[arg(short, long, default_value = DEFAULT_TENANT)]
        tenant: String,
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },
    /// Long-running background daemon: consume due `schedule` rows and walk
    /// them through the SCD-2 lifecycle. Pure mechanical executor — no
    /// scheduling decisions (those live in the future `SchedulerBlade`).
    /// Stop with Ctrl-C.
    Runner {
        #[arg(short, long, default_value = DEFAULT_TENANT)]
        tenant: String,
        /// Interval between ticks in milliseconds.
        #[arg(long, default_value = "1000")]
        interval_ms: u64,
        /// Agent record-id literal (`entity:<uuid>`) the runner dispatches as.
        /// When set, the runner uses `CapabilityGovernor::check_capability`
        /// then the appropriate tool blade for each dispatch. When unset,
        /// the runner uses `NoopDispatcher` and only walks schedule rows
        /// through the SCD-2 lifecycle (useful for smoke-testing the loop).
        #[arg(long)]
        agent_id: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Honour RUST_LOG (e.g. `RUST_LOG=info,superx_inference=debug` to see prompts).
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let cli = Cli::parse();

    let db_path = PathBuf::from(
        std::env::var("SUPERX_DB_PATH").unwrap_or_else(|_| "./db/superx.db".to_string()),
    );
    let ns = std::env::var("SUPERX_NS").unwrap_or_else(|_| "superx".to_string());
    let db_name = std::env::var("SUPERX_DB_NAME").unwrap_or_else(|_| "main".to_string());
    let kernel = Arc::new(Kernel::init(&db_path, &ns, &db_name).await?);

    // Activate Observability Pipe (Background - Runs as Root to avoid session clash)
    let k_sub = kernel.clone();
    let kafka_brokers = cli.kafka_brokers.clone();
    let kafka_topic = cli.kafka_topic.clone();
    let api_url = cli.emission_api.clone();
    
    let sub_tenant = get_command_tenant(&cli.command);

    tokio::spawn(async move {
        let sub = TelemetrySubscriber::new(&k_sub);
        let k_sink = if let (Some(b), Some(t)) = (kafka_brokers, kafka_topic) {
            KafkaSink::new(&b, &t).ok()
        } else { None };
        let a_sink = api_url.map(|u| ApiSink::new(&u, None));
        
        // Internal loop runs in root context to capture cross-tenant telemetry for this instance
        let _ = sub.run_loop(k_sink.as_ref(), a_sink.as_ref(), &sub_tenant).await;
    });

    handle_command(cli.command, kernel).await?;

    Ok(())
}

fn get_command_tenant(cmd: &Commands) -> String {
    match cmd {
        Commands::Graphify { tenant, .. } |
        Commands::Compile { tenant, .. } |
        Commands::Evaluate { tenant, .. } |
        Commands::Promote { tenant, .. } |
        Commands::Propose { tenant, .. } |
        Commands::Bootstrap { tenant } |
        Commands::Stats { tenant, .. } |
        Commands::Identify { tenant, .. } |
        Commands::ListAgents { tenant } |
        Commands::ListTools { tenant } |
        Commands::Demo { tenant } |
        Commands::Runner { tenant, .. } |
        Commands::Enqueue { tenant, .. } |
        Commands::ScheduleList { tenant, .. } |
        Commands::RunnerDemo { tenant, .. } => tenant.clone(),
    }
}

async fn handle_command(cmd: Commands, kernel: Arc<Kernel>) -> Result<(), Box<dyn std::error::Error>> {
    match cmd {
        Commands::Graphify { path, tenant, run_id } => {
            kernel.set_session_auth(&tenant, "user").await?;
            let abs_path = std::fs::canonicalize(path)?;
            let actual_run_id = run_id.unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
            println!("Ingesting from: {} (Run ID: {actual_run_id})", abs_path.display());
            
            let ingestor = UniversalIngestor::new(&kernel);
            let source = Box::new(FileSource { path: abs_path.to_string_lossy().to_string() });
            let root_id = ingestor.ingest(source, &actual_run_id).await?;
            println!("Ingestion complete. Root Entity: {root_id}");
        }
        Commands::Compile { root, tenant, model, tokenizer, tiers } => {
            kernel.set_session_auth(&tenant, "user").await?;
            println!("Compiling context for: {root}");
            
            let inference = if let (Some(m), Some(t)) = (model, tokenizer) {
                println!("Loading inference engine: {} with tokenizer {}", m.display(), t.display());
                let engine = superx_inference::InferenceEngine::new(&m, &t)?;
                Some(std::sync::Arc::new(tokio::sync::Mutex::new(engine)))
            } else {
                None
            };

            let compiler = CompilerBlade::new(&kernel, inference);
            let run_id = uuid::Uuid::now_v7().to_string();
            let xml = compiler.compile(&root, &run_id, tiers).await?;
            println!("--- COMPILED XML (Run ID: {run_id}) ---\n{xml}\n--------------------");
        }
        Commands::Evaluate { proposal, harness, tenant } => {
            kernel.set_session_auth(&tenant, "user").await?;
            println!("Evaluating proposal {proposal} with harness {}", harness.display());
            let wasm = std::fs::read(harness)?;
            let meta = MetaHarness::new(&kernel);
            let score = meta.evaluate(&proposal, &wasm).await?;
            println!("Evaluation complete. Score: {score:.2}");
        }
        Commands::Promote { proposal, threshold, tenant } => {
            kernel.set_session_auth(&tenant, "user").await?;
            println!("Attempting to promote proposal {proposal} (Threshold: {threshold:.2})");
            let meta = MetaHarness::new(&kernel);
            let promoted = meta.promote(&proposal, threshold).await?;
            if promoted {
                println!("SUCCESS: Proposal promoted to substrate.");
            } else {
                println!("FAILED: Proposal did not meet threshold.");
            }
        }
        Commands::Propose { from, to, tenant, model, tokenizer } => {
            kernel.set_session_auth(&tenant, "user").await?;
            println!("Agent analyzing relation between {from} and {to}...");
            let engine = superx_inference::InferenceEngine::new(&model, &tokenizer)?;
            let engine_arc = std::sync::Arc::new(tokio::sync::Mutex::new(engine));
            let proposer = ProposerBlade::new(&kernel, engine_arc);
            let run_id = uuid::Uuid::now_v7().to_string();
            let proposal_id = proposer.propose_relation(&from, &to, &run_id).await?;
            println!("Relation proposed. Proposal Entity: {proposal_id}");
        }
        Commands::Bootstrap { tenant } => {
            println!("Bootstrapping substrate for tenant: {tenant}...");
            let boot = BootstrapBlade::new(&kernel);
            let substrate_id = boot.run(&tenant).await?;
            println!("Bootstrap complete. Substrate ID: {substrate_id}");
        }
        Commands::Identify { agent_uid, tenant } => run_identify(&kernel, &agent_uid, &tenant).await?,
        Commands::ListAgents { tenant } => run_list_agents(&kernel, &tenant).await?,
        Commands::ListTools { tenant } => run_list_tools(&kernel, &tenant).await?,
        Commands::Demo { tenant } => run_demo(&kernel, &tenant).await?,
        Commands::RunnerDemo { tenant, max_ticks } => {
            run_runner_demo(kernel.clone(), &tenant, max_ticks).await?;
        }
        Commands::Enqueue { tenant, kind, target, run, metadata } => {
            run_enqueue(&kernel, &tenant, &kind, &target, run.as_deref(), &metadata).await?;
        }
        Commands::ScheduleList { tenant, limit } => {
            run_schedule_list(&kernel, &tenant, limit).await?;
        }
        Commands::Runner { tenant, interval_ms, agent_id } => {
            run_runner(kernel.clone(), &tenant, interval_ms, agent_id).await?;
        }
        Commands::Stats { tenant, limit } => {
            kernel.set_session_auth(&tenant, "user").await?;
            println!("Fetching latest {limit} telemetry events for {tenant}...");
            let query = "SELECT *, <string>id as id, <string>timestamp as timestamp \
                 FROM telemetry_stream WHERE tenant_id = $session_tenant ORDER BY timestamp DESC LIMIT $limit";
            let mut res = kernel.db.query(query).bind(("limit", limit)).await?;
            
            println!("--- TELEMETRY EVENTS ---");
            if let Ok(rows) = res.take::<Vec<serde_json::Value>>(0) {
                for row in rows {
                    println!("Event: {}", row.get("lifecycle_event").and_then(|v| v.as_str()).unwrap_or("unknown"));
                    println!("Payload: {}", serde_json::to_string_pretty(row.get("payload").unwrap_or(&serde_json::Value::Null)).unwrap_or_else(|_| "{}".to_string()));
                    println!("Timestamp: {}", row.get("timestamp").and_then(|v| v.as_str()).unwrap_or("none"));
                    println!("---");
                }
            }
            println!("------------------------");
        }
    }
    Ok(())
}

/// `runner-demo`: full end-to-end pipeline driven through the schedule
/// queue + runner. The most operator-facing demonstration of the v0.1
/// runtime: every step happens via the same kernel verbs and dispatcher
/// the daemon process uses in production.
///
/// Lives as one function — clippy's 100-line cap doesn't fit a demo
/// that wires bootstrap + entity provisioning + enqueue + tick loop in
/// the right order. Splitting it would just scatter the demo's
/// narrative across helpers operators have to follow back and forth.
#[allow(clippy::too_many_lines)]
async fn run_runner_demo(
    kernel: std::sync::Arc<Kernel>,
    tenant: &str,
    max_ticks: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::Arc;

    #[derive(serde::Deserialize)]
    struct IdRow { id: surrealdb::sql::Thing }
    #[derive(serde::Deserialize)]
    #[allow(dead_code)]
    struct StatusRow {
        status: String,
        valid_from: chrono::DateTime<chrono::Utc>,
    }

    println!("=== SuperX Runner Demo (tenant=`{tenant}`) ===");

    // 1. Bootstrap — provisions substrate, seeds admin agents + 5 tools,
    //    grants edge_has_capability so the dispatch path is admissible.
    println!("[1/5] Bootstrapping tenant…");
    let _substrate_id = superx_bootstrap::BootstrapBlade::new(&kernel).run(tenant).await?;

    // 2. Locate one bootstrap-seeded admin agent — the runner will
    //    dispatch as this identity. system_controller + gemini_cli are
    //    both seeded with full tool capability.
    let node_agent = kernel.type_thing("node_agent")?;
    let mut res = kernel.db
        .query("SELECT id FROM entity WHERE type = $ty AND role = 'admin' \
                AND tenant = $session_tenant LIMIT 1")
        .bind(("ty", node_agent))
        .await?;
    let admin_thing = res.take::<Vec<IdRow>>(0)?
        .pop()
        .ok_or("no admin agent after bootstrap")?
        .id;
    let admin_id = format!(
        "entity:{}",
        match admin_thing.id {
            surrealdb::sql::Id::Uuid(u) => u.to_raw(),
            surrealdb::sql::Id::String(s) => s,
            other => format!("{other:?}"),
        }
    );
    println!("      admin agent: {admin_id}");

    // 3. Create a source-typed entity with attr_desc pointing at a temp
    //    fixture file. This is the §8 pattern: the schedule row carries
    //    only the entity id; the agent reads the path from the entity's
    //    attr ledger at dispatch time.
    println!("[2/5] Provisioning source entity + fixture…");
    let fixture = tempfile::tempdir()?;
    let fixture_path = fixture.path().join("payload.txt");
    std::fs::write(&fixture_path, "runner demo payload — exercises the ingest dispatch path")?;
    let source_uuid = uuid::Uuid::now_v7();
    let source_thing = surrealdb::sql::Thing::from((
        "entity",
        surrealdb::sql::Id::Uuid(surrealdb::sql::Uuid::from(source_uuid)),
    ));
    let source_id = format!("entity:{source_uuid}");
    let node_source = kernel.type_thing("node_source_external")?;
    let tenant_thing = kernel.session_tenant_thing().await?;
    kernel.db
        .query("CREATE entity CONTENT { \
            id: $id, type: $type, tenant: $tenant, role: 'user' \
        }")
        .bind(("id", source_thing.clone()))
        .bind(("type", node_source))
        .bind(("tenant", tenant_thing))
        .await?.check()?;
    kernel.supersede_state(
        &source_id,
        "attr_desc",
        serde_json::json!({"text": fixture.path().to_string_lossy().to_string()}),
        None,
    ).await?;
    println!("      source: {source_id}");
    println!("      fixture: {}", fixture.path().display());

    // 4. Enqueue an ingest schedule row + provision a node_run.
    println!("[3/5] Enqueueing ingest schedule item…");
    let run_uuid = uuid::Uuid::now_v7();
    let run_thing = surrealdb::sql::Thing::from((
        "entity",
        surrealdb::sql::Id::Uuid(surrealdb::sql::Uuid::from(run_uuid)),
    ));
    let node_run = kernel.type_thing("node_run")?;
    let tenant_thing = kernel.session_tenant_thing().await?;
    kernel.db
        .query("CREATE entity CONTENT { \
            id: $id, type: $type, tenant: $tenant, role: 'user' \
        }")
        .bind(("id", run_thing.clone()))
        .bind(("type", node_run))
        .bind(("tenant", tenant_thing))
        .await?.check()?;
    let _sid = kernel.enqueue_schedule_item(
        run_thing.clone(),
        "ingest",
        source_thing,
        chrono::Utc::now(),
        vec![],
        serde_json::json!({}),
    ).await?;
    println!("      run: entity:{run_uuid}");

    // 5. Build the real KernelDispatcher and tick until the chain
    //    completes (or max_ticks trips per NASA Rule 2 bounded loop).
    println!("[4/5] Ticking RunnerBlade with KernelDispatcher…");
    let dispatcher: Arc<dyn superx_runner::Dispatcher> = Arc::new(
        superx_dispatcher::KernelDispatcher::new(kernel.clone(), admin_id.clone())
    );
    let runner = superx_runner::RunnerBlade::with_dispatcher(&kernel, dispatcher);
    let mut tick_count = 0_u32;
    let final_status = loop {
        if tick_count >= max_ticks {
            break "<max_ticks tripped>".to_string();
        }
        let processed = runner.tick().await?;
        tick_count += 1;
        println!("      tick {tick_count}: processed {processed} chain(s)");

        // Peek the chain's current status.
        let mut res = kernel.db
            .query("SELECT status, valid_from FROM schedule \
                    WHERE run = $run AND tenant = $session_tenant \
                    ORDER BY valid_from DESC LIMIT 1")
            .bind(("run", run_thing.clone()))
            .await?;
        let latest: Option<StatusRow> = res.take::<Vec<StatusRow>>(0)?.pop();
        let status = latest.map_or_else(|| "<no row>".to_string(), |r| r.status);
        if matches!(status.as_str(), "completed" | "failed") {
            break status;
        }
    };

    println!("[5/5] Final chain status: {final_status}");
    println!("=== Done ===");
    Ok(())
}

/// Enqueue a `schedule` row. The operator hands the runner a typed unit
/// of work to dispatch. The handler binds the session to the tenant's
/// substrate, optionally provisions a fresh `node_run` entity, then
/// calls `Kernel::enqueue_schedule_item` with the typed Things the
/// kernel verb requires.
async fn run_enqueue(
    kernel: &Kernel,
    tenant: &str,
    kind: &str,
    target: &str,
    run: Option<&str>,
    metadata_json: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let ns_uuid = uuid::Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8")
        .expect("DNS namespace UUID is well-formed");
    let substrate_uuid = uuid::Uuid::new_v5(&ns_uuid, tenant.as_bytes());
    kernel
        .set_session_auth(&substrate_uuid.to_string(), "admin")
        .await?;

    let target_thing = Kernel::parse_id(target)?;

    // Resolve or create the run entity. Operators that already have a
    // run can pass `--run entity:<uuid>`; otherwise a fresh `node_run`
    // is provisioned so the schedule row's `run` FK has a valid target.
    let run_thing = if let Some(run_id) = run {
        Kernel::parse_id(run_id)?
    } else {
        let run_uuid = uuid::Uuid::now_v7();
        let run_thing = surrealdb::sql::Thing::from((
            "entity",
            surrealdb::sql::Id::Uuid(surrealdb::sql::Uuid::from(run_uuid)),
        ));
        let node_run = kernel.type_thing("node_run")?;
        let tenant_thing = kernel.session_tenant_thing().await?;
        kernel
            .db
            .query(
                "CREATE entity CONTENT { \
                    id: $id, type: $type, tenant: $tenant, role: 'user' \
                }",
            )
            .bind(("id", run_thing.clone()))
            .bind(("type", node_run))
            .bind(("tenant", tenant_thing))
            .await?
            .check()?;
        println!("Provisioned fresh node_run: entity:{run_uuid}");
        run_thing
    };

    let metadata: serde_json::Value = serde_json::from_str(metadata_json)
        .map_err(|e| format!("--metadata is not valid JSON: {e}"))?;

    let sid = kernel
        .enqueue_schedule_item(
            run_thing,
            kind,
            target_thing,
            chrono::Utc::now(),
            vec![],
            metadata,
        )
        .await?;

    // Render the schedule id as the canonical literal so the operator
    // can pass it into follow-up CLI calls.
    let sid_literal = match &sid.id {
        surrealdb::sql::Id::Uuid(u) => format!("schedule:{}", u.to_raw()),
        surrealdb::sql::Id::String(s) => format!("schedule:{s}"),
        other => format!("schedule:{other:?}"),
    };
    println!("Enqueued: {sid_literal} (kind={kind}, target={target})");
    Ok(())
}

/// Print one line per schedule chain in the tenant showing the chain's
/// current state (most-recent row by `valid_from`).
async fn run_schedule_list(
    kernel: &Kernel,
    tenant: &str,
    limit: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    #[derive(serde::Deserialize)]
    #[allow(dead_code)]
    struct Row {
        run: surrealdb::sql::Thing,
        kind: String,
        target: surrealdb::sql::Thing,
        status: String,
        attempt: i64,
        valid_from: chrono::DateTime<chrono::Utc>,
    }

    let ns_uuid = uuid::Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8")
        .expect("DNS namespace UUID is well-formed");
    let substrate_uuid = uuid::Uuid::new_v5(&ns_uuid, tenant.as_bytes());
    kernel
        .set_session_auth(&substrate_uuid.to_string(), "admin")
        .await?;

    // Per-run dedupe in SQL would need a self-join; simpler to scan
    // rows ordered by (run, valid_from DESC) and dedupe by run in Rust.
    let mut res = kernel
        .db
        .query(
            "SELECT run, kind, target, status, attempt, valid_from \
             FROM schedule WHERE tenant = $session_tenant \
             ORDER BY run ASC, valid_from DESC",
        )
        .await?;
    let rows: Vec<Row> = res.take(0)?;

    let mut seen_runs = std::collections::HashSet::new();
    let mut printed = 0_usize;
    println!(
        "{:<30} {:<12} {:<14} {:>3} {:<32} VALID_FROM",
        "RUN", "KIND", "STATUS", "ATT", "TARGET"
    );
    for r in rows {
        let key = r.run.to_string();
        if !seen_runs.insert(key) {
            continue;
        }
        if printed >= limit {
            break;
        }
        let run_id = match &r.run.id {
            surrealdb::sql::Id::Uuid(u) => u.to_raw(),
            surrealdb::sql::Id::String(s) => s.clone(),
            other => format!("{other:?}"),
        };
        let target_id = match &r.target.id {
            surrealdb::sql::Id::Uuid(u) => u.to_raw(),
            surrealdb::sql::Id::String(s) => s.clone(),
            other => format!("{other:?}"),
        };
        println!(
            "{:<30} {:<12} {:<14} {:>3} {:<32} {}",
            &run_id[..run_id.len().min(28)],
            r.kind,
            r.status,
            r.attempt,
            &target_id[..target_id.len().min(30)],
            r.valid_from.format("%Y-%m-%dT%H:%M:%SZ"),
        );
        printed += 1;
    }
    if printed == 0 {
        println!("(no schedule chains in tenant `{tenant}`)");
    }
    Ok(())
}

/// Long-running daemon — consume due `schedule` rows in `tenant` and walk
/// each chain through the SCD-2 lifecycle until Ctrl-C. The runner uses
/// the same UUIDv5-substrate-id derivation `BootstrapBlade` uses, so the
/// session binds to the substrate row that holds the schedule chains
/// rather than to the bare tenant-name string.
///
/// When `agent_id` is supplied, dispatches through `KernelDispatcher`
/// (capability-gated, real tool blades). Otherwise falls back to
/// `NoopDispatcher` — the SCD-2 transition loop is fully exercised
/// either way.
async fn run_runner(
    kernel: Arc<Kernel>,
    tenant: &str,
    interval_ms: u64,
    agent_id: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Derive the substrate Thing the same way bootstrap does, then bind
    // the session to that substrate uuid — the schedule PERMISSIONS use
    // `tenant = $session_tenant` (typed Thing compare).
    let ns_uuid = uuid::Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8")
        .expect("DNS namespace UUID is well-formed");
    let substrate_uuid = uuid::Uuid::new_v5(&ns_uuid, tenant.as_bytes());
    kernel.set_session_auth(&substrate_uuid.to_string(), "user").await?;

    // Build the runner with whichever dispatcher matches the CLI flags.
    let dispatcher: std::sync::Arc<dyn superx_runner::Dispatcher> = if let Some(aid) = agent_id {
        println!("Dispatching via KernelDispatcher (agent_id={aid}).");
        std::sync::Arc::new(superx_dispatcher::KernelDispatcher::new(kernel.clone(), aid))
    } else {
        println!("Dispatching via NoopDispatcher (no --agent-id supplied).");
        std::sync::Arc::new(superx_runner::NoopDispatcher)
    };
    let runner = superx_runner::RunnerBlade::with_dispatcher(&kernel, dispatcher);

    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(interval_ms));
    // The first tick fires immediately; from then on every `interval_ms`.
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    println!(
        "RunnerBlade started for tenant `{tenant}` (substrate {substrate_uuid}). \
         Tick interval = {interval_ms}ms. Ctrl-C to stop."
    );

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                match runner.tick().await {
                    Ok(0) => { /* nothing due — stay quiet */ }
                    Ok(n) => println!("tick: processed {n} schedule chain(s)"),
                    Err(e) => eprintln!("tick error: {e}"),
                }
            }
            _ = tokio::signal::ctrl_c() => {
                println!("Ctrl-C received; runner stopping.");
                break;
            }
        }
    }
    Ok(())
}

async fn run_identify(kernel: &Kernel, agent_uid: &str, tenant: &str) -> Result<(), Box<dyn std::error::Error>> {
    kernel.set_session_auth(tenant, "user").await?;
    println!("Identifying agent {agent_uid} in tenant {tenant}...");
    let gov = superx_agent::CapabilityGovernor::new(kernel);
    let session_uid = gov.handshake(agent_uid).await?;
    println!("Session established. session_uid: {session_uid}");
    Ok(())
}

async fn run_list_agents(kernel: &Kernel, tenant: &str) -> Result<(), Box<dyn std::error::Error>> {
    kernel.set_session_auth(tenant, "user").await?;
    println!("Agents registered for tenant {tenant}:");
    let mut res = kernel.db.query(
        "SELECT <string>id AS id, role FROM entity \
         WHERE type = type_definition:node_agent AND tenant_id = $session_tenant"
    ).await?;
    let rows: Vec<serde_json::Value> = res.take(0).unwrap_or_default();
    if rows.is_empty() {
        println!("  (none — run `superx-cli bootstrap --tenant {tenant}` first)");
    }
    for row in rows {
        let raw_id = row.get("id").and_then(|v| v.as_str()).unwrap_or("?");
        let role = row.get("role").and_then(|v| v.as_str()).unwrap_or("?");
        // SurrealDB returns hyphen-bearing UUIDs as `entity:`<uuid>`` (with backticks
        // per the EscapeRidKey rule). Strip them so operators can copy-paste the
        // local uid into `superx-cli identify --agent-uid <uid>`.
        let clean = raw_id.replace('`', "");
        let (_, uid) = clean.split_once(':').unwrap_or(("", ""));
        println!("  uid={uid}  role={role}");
    }
    Ok(())
}

async fn run_list_tools(kernel: &Kernel, tenant: &str) -> Result<(), Box<dyn std::error::Error>> {
    kernel.set_session_auth(tenant, "user").await?;
    println!("Tools registered for tenant {tenant}:");
    let mut res = kernel.db.query(
        "SELECT <string>id AS id FROM entity \
         WHERE type = type_definition:node_tool AND tenant_id = $session_tenant"
    ).await?;
    let rows: Vec<serde_json::Value> = res.take(0).unwrap_or_default();
    if rows.is_empty() {
        println!("  (none — run `superx-cli bootstrap --tenant {tenant}` first)");
    }
    for row in rows {
        let raw_id = row.get("id").and_then(|v| v.as_str()).unwrap_or("?");
        let clean = raw_id.replace('`', "");
        let (_, uid) = clean.split_once(':').unwrap_or(("", ""));
        println!("  uid={uid}");
    }
    Ok(())
}

/// `run_demo`: drives the full cognitive loop in one command —
/// bootstrap -> graphify (against a tiny in-process fixture) -> proposal+score
/// (without inference) -> promote -> read-back. The intent is to give an
/// operator a single one-shot to confirm every blade is wired correctly.
async fn run_demo(kernel: &Kernel, tenant: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("== STAGE 1/5 — bootstrap (tenant={tenant})");
    let substrate_id = BootstrapBlade::new(kernel).run(tenant).await?;
    println!("   substrate: {substrate_id}");

    println!("== STAGE 2/5 — graphify (in-process fixture)");
    let fixture = tempfile::tempdir()?;
    std::fs::write(fixture.path().join("alpha.txt"), "alpha content")?;
    std::fs::write(fixture.path().join("beta.txt"), "beta content")?;
    let ingestor = UniversalIngestor::new(kernel);
    let run_id = uuid::Uuid::now_v7().to_string();
    let source = Box::new(FileSource { path: fixture.path().to_string_lossy().to_string() });
    let root_id = ingestor.ingest(source, &run_id).await?;
    println!("   root: {root_id}");

    println!("== STAGE 3/5 — proposal entity + simulated evaluation score");
    // Entities in SuperX follow the Id::String(uuid_text) convention so they
    // round-trip cleanly through `Kernel::parse_id` (which splits on `:`).
    // Only state_ledger / relation / telemetry rows use native `Id::Uuid`.
    let raw_uuid = uuid::Uuid::now_v7().to_string();
    let proposal_thing = surrealdb::sql::Thing::from(("entity".to_string(), raw_uuid.clone()));
    let proposal_id = format!("entity:{raw_uuid}");
    kernel.db
        .query("CREATE $id CONTENT { tenant_id: $session_tenant, type: type_definition:node_proposal }")
        .bind(("id", proposal_thing))
        .await?
        .check()?;
    kernel.supersede_state(&proposal_id, "attr_score", serde_json::json!({"score": 0.92}), Some(run_id.clone())).await?;
    println!("   proposal: {proposal_id}  (score=0.92)");

    println!("== STAGE 4/5 — promote (threshold=0.8)");
    let promoted = MetaHarness::new(kernel).promote(&proposal_id, 0.8).await?;
    println!("   promoted: {promoted}");

    println!("== STAGE 5/5 — telemetry summary (most recent 5)");
    let mut res = kernel
        .db
        .query("SELECT lifecycle_event, <string>timestamp AS timestamp FROM telemetry_stream WHERE tenant_id = $session_tenant ORDER BY timestamp DESC LIMIT 5")
        .await?;
    if let Ok(rows) = res.take::<Vec<serde_json::Value>>(0) {
        for row in rows {
            let event = row.get("lifecycle_event").and_then(|v| v.as_str()).unwrap_or("?");
            let ts = row.get("timestamp").and_then(|v| v.as_str()).unwrap_or("?");
            println!("   {ts}  {event}");
        }
    }

    println!("== DONE — cognitive loop completed end-to-end.");
    Ok(())
}
