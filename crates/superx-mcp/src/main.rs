//! # superx-mcp — binary entry point
//!
//! Thin process wrapper that opens the substrate, spawns the two long-lived
//! background tasks (telemetry subscriber + minute-cadence heartbeat pulse),
//! then hands the [`McpServer`] to `rmcp` over stdio.
//!
//! All MCP tool-dispatch policy lives in [`superx_mcp::dispatch_tool`] (see
//! `src/lib.rs`) so the policy is unit-testable without standing up a real
//! rmcp transport. This file is *only* runtime concerns:
//!
//! - environment-variable configuration (`SUPERX_DB_PATH`, `SUPERX_NS`,
//!   `SUPERX_DB_NAME`, `SUPERX_TENANT`, `SUPERX_KAFKA_*`, `SUPERX_EMISSION_API`,
//!   `SUPERX_RUNNER_INTERVAL_MS`, `SUPERX_RUNNER_AGENT_ID`)
//! - tracing init with `RUST_LOG` honored via `EnvFilter`
//! - background-task lifecycle with error logging (no silent failures)
//!
//! Copyright (c) 2026 Constantin Alexander <constantin@dedomena.io>.
//! Licensed under the Apache License, Version 2.0.

#![deny(warnings)]
#![deny(clippy::pedantic)]

use rmcp::serve_server;
use std::path::PathBuf;
use std::sync::Arc;
use superx_emission::{ApiSink, KafkaSink, TelemetrySubscriber};
use superx_kernel::Kernel;
use superx_mcp::McpServer;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Honour RUST_LOG so operators can set verbosity at runtime
    // (e.g. `RUST_LOG=info,superx_inference=debug` to see full prompts).
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let db_path = PathBuf::from(
        std::env::var("SUPERX_DB_PATH").unwrap_or_else(|_| "./db/superx.db".to_string()),
    );
    let ns = std::env::var("SUPERX_NS").unwrap_or_else(|_| "superx".to_string());
    let db_name = std::env::var("SUPERX_DB_NAME").unwrap_or_else(|_| "main".to_string());
    let kernel = Arc::new(Kernel::init(&db_path, &ns, &db_name).await?);
    let tenant = std::env::var("SUPERX_TENANT").unwrap_or_else(|_| superx_kernel::DEFAULT_TENANT.to_string());

    // 1. Observability pipe (LIVE SELECT → Kafka/API sinks if configured).
    //    Errors are logged so a downstream failure (broker down, endpoint 5xx,
    //    permission denied) doesn't silently kill the firehose.
    let k_sub = kernel.clone();
    let kafka_brokers = std::env::var("SUPERX_KAFKA_BROKERS").ok();
    let kafka_topic = std::env::var("SUPERX_KAFKA_TOPIC").ok();
    let api_url = std::env::var("SUPERX_EMISSION_API").ok();
    let sub_tenant = tenant.clone();
    tokio::spawn(async move {
        let sub = TelemetrySubscriber::new(&k_sub);
        let k_sink = match (kafka_brokers, kafka_topic) {
            (Some(b), Some(t)) => KafkaSink::new(&b, &t).ok(),
            _ => None,
        };
        let a_sink = api_url.map(|u| ApiSink::new(&u, None));
        if let Err(e) = sub.run_loop(k_sink.as_ref(), a_sink.as_ref(), &sub_tenant).await {
            tracing::error!("telemetry subscriber terminated: {e}");
        }
    });

    // 2. Heartbeat pulse — one telemetry row every minute so freshness is observable.
    //    Pulse errors are logged but don't tear down the loop (next tick retries).
    let k_pulse = kernel.clone();
    let p_tenant = tenant.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_mins(1));
        loop {
            interval.tick().await;
            if let Err(e) = k_pulse.set_session_auth(&p_tenant, "admin").await {
                tracing::error!("pulse set_session_auth failed: {e}");
                continue;
            }
            if let Err(e) = k_pulse.pulse().await {
                tracing::error!("pulse emission failed: {e}");
            }
        }
    });

    // 3. RunnerBlade — consume due `schedule` rows on a configurable cadence.
    //    The session is re-bound to the tenant's substrate uuid (not the human
    //    tenant name) on every tick so interleaved `dispatch_tool` / pulse
    //    session changes can't clobber the runner's view of `$session_tenant`
    //    across tick boundaries. Tick errors log but don't tear down the loop.
    //
    //    Dispatcher selection:
    //      - SUPERX_RUNNER_AGENT_ID set → KernelDispatcher (capability-checked,
    //        real tool-blade dispatch — `compile` / `promote` / `ingest` so far).
    //      - unset → NoopDispatcher (loop-only smoke mode).
    let k_run = kernel.clone();
    let r_tenant = tenant.clone();
    let runner_interval_ms: u64 = std::env::var("SUPERX_RUNNER_INTERVAL_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1000);
    let runner_agent_id = std::env::var("SUPERX_RUNNER_AGENT_ID").ok();
    tokio::spawn(async move {
        let ns_uuid = uuid::Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8")
            .expect("DNS namespace UUID is well-formed");
        let substrate_uuid = uuid::Uuid::new_v5(&ns_uuid, r_tenant.as_bytes()).to_string();
        let dispatcher: std::sync::Arc<dyn superx_runner::Dispatcher> = if let Some(aid) = runner_agent_id {
            tracing::info!("runner: dispatching via KernelDispatcher (agent_id={aid})");
            std::sync::Arc::new(KernelDispatcher {
                kernel: k_run.clone(),
                agent_id: aid,
            })
        } else {
            tracing::info!("runner: dispatching via NoopDispatcher (SUPERX_RUNNER_AGENT_ID unset)");
            std::sync::Arc::new(superx_runner::NoopDispatcher)
        };
        let runner = superx_runner::RunnerBlade::with_dispatcher(&k_run, dispatcher);
        let mut interval =
            tokio::time::interval(std::time::Duration::from_millis(runner_interval_ms));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            if let Err(e) = k_run.set_session_auth(&substrate_uuid, "user").await {
                tracing::error!("runner session bind failed: {e}");
                continue;
            }
            match runner.tick().await {
                Ok(0) => { /* nothing due — stay quiet */ }
                Ok(n) => tracing::info!("runner: processed {n} schedule chain(s)"),
                Err(e) => tracing::error!("runner tick error: {e}"),
            }
        }
    });

    println!("SuperX MCP Server starting on stdio (Tenant: {tenant})...");
    serve_server(McpServer::new(kernel), rmcp::transport::stdio()).await?;
    Ok(())
}

/// MCP-side concrete dispatcher — symmetric to `superx-cli`'s
/// `KernelDispatcher`. Routes schedule-row `kind` values through
/// `CapabilityGovernor::check_capability` then to the appropriate tool
/// blade.
///
/// Currently routed (kept in lockstep with `superx-cli`'s dispatcher;
/// follow-up PRs add `propose` + `evaluate`):
///   - `compile` → `CompilerBlade::compile(target_id, run_id, None)`
///   - `promote` → `MetaHarness::promote(target_id, attr_config.promote_threshold)`
///   - `ingest`  → `UniversalIngestor::ingest(FileSource{path=attr_desc.text}, run_id)`
///
/// Unknown kinds return `KernelError::Validation`; the runner records a
/// clean `failed` transition that surfaces in telemetry.
///
/// **Note:** this is currently a copy of the CLI's `KernelDispatcher`.
/// A follow-up PR extracts the implementation to a shared crate
/// (`superx-dispatcher`?) so the two binaries import the same code.
struct KernelDispatcher {
    kernel: std::sync::Arc<Kernel>,
    agent_id: String,
}

#[async_trait::async_trait]
impl superx_runner::Dispatcher for KernelDispatcher {
    async fn dispatch(
        &self,
        kind: &str,
        target: &surrealdb::sql::Thing,
        run: &surrealdb::sql::Thing,
    ) -> Result<(), superx_kernel::KernelError> {
        let tool_uid = format!("tool_{kind}");
        let gov = superx_agent::CapabilityGovernor::new(&self.kernel);
        gov.check_capability(&self.agent_id, &tool_uid).await?;

        let target_id = thing_to_record_id_literal(target);
        let run_id = thing_to_record_id_literal(run);

        match kind {
            "compile" => {
                let compiler = superx_compiler::CompilerBlade::new(&self.kernel, None);
                compiler.compile(&target_id, &run_id, None).await.map(|_xml| ())
            }
            "promote" => {
                let threshold: f64 = self
                    .kernel
                    .get_parameter("promote_threshold", 0.8_f64)
                    .await;
                let harness = superx_harness::MetaHarness::new(&self.kernel);
                harness.promote(&target_id, threshold).await.map(|_| ())
            }
            "ingest" => {
                #[derive(serde::Deserialize)]
                #[allow(dead_code)]
                struct DescRow {
                    value_json: serde_json::Value,
                    valid_from: chrono::DateTime<chrono::Utc>,
                }
                let attr_desc = self.kernel.type_thing("attr_desc")?;
                let mut res = self
                    .kernel
                    .db
                    .query(
                        "SELECT value_json, valid_from FROM state_ledger \
                         WHERE target = $t AND `type` = $ty \
                         ORDER BY valid_from DESC LIMIT 1",
                    )
                    .bind(("t", target.clone()))
                    .bind(("ty", attr_desc))
                    .await?;
                let row = res.take::<Vec<DescRow>>(0)?.pop().ok_or_else(|| {
                    superx_kernel::KernelError::Validation(format!(
                        "ingest dispatcher: target {target_id} has no \
                         attr_desc — the file path must live there per \
                         ARCHITECTURE.md §8"
                    ))
                })?;
                let path = row
                    .value_json
                    .get("text")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        superx_kernel::KernelError::Validation(format!(
                            "ingest dispatcher: target {target_id} \
                             attr_desc.text is missing or not a string"
                        ))
                    })?
                    .to_string();
                let ingestor = superx_ingest::UniversalIngestor::new(&self.kernel);
                let source = Box::new(superx_ingest::FileSource { path });
                ingestor.ingest(source, &run_id).await.map(|_root_id| ())
            }
            other => Err(superx_kernel::KernelError::Validation(format!(
                "runner dispatcher: kind `{other}` not yet implemented — \
                 needs target-entity attr-resolution (ARCHITECTURE.md §8); \
                 see follow-up PRs"
            ))),
        }
    }
}

/// Render a `Thing` as its canonical `<table>:<uuid>` record-id literal
/// without going through `Display` (which adds backtick escaping for
/// hyphen-bearing UUIDs and breaks `Kernel::parse_id` round-trip).
fn thing_to_record_id_literal(t: &surrealdb::sql::Thing) -> String {
    let id = match &t.id {
        surrealdb::sql::Id::Uuid(u) => u.to_raw(),
        surrealdb::sql::Id::String(s) => s.clone(),
        other => format!("{other:?}"),
    };
    format!("{}:{}", t.tb, id)
}
