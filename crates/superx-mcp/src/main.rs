/*
 * SuperX MCP Server (binary entry point) — Revision 42.14
 *
 * Copyright (c) 2026 Constantin Alexander <constantin@dedomena.io>
 *
 * Thin wrapper: open the kernel, spawn the observability + heartbeat
 * background tasks, hand the `McpServer` to rmcp over stdio. All tool-dispatch
 * policy lives in `superx_mcp::dispatch_tool` (see `src/lib.rs`).
 */

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
    tracing_subscriber::fmt::init();

    let db_path = PathBuf::from("./db/superx.db");
    let kernel = Arc::new(Kernel::init(&db_path, "superx", "prod").await?);
    let tenant = std::env::var("SUPERX_TENANT").unwrap_or_else(|_| "sa_dogfood".to_string());

    // 1. Observability pipe (LIVE SELECT → Kafka/API sinks if configured)
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
        let _ = sub.run_loop(k_sink.as_ref(), a_sink.as_ref(), &sub_tenant).await;
    });

    // 2. Heartbeat pulse — one telemetry row every minute so freshness is observable.
    let k_pulse = kernel.clone();
    let p_tenant = tenant.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_mins(1));
        loop {
            interval.tick().await;
            if k_pulse.set_session_auth(&p_tenant, "admin").await.is_ok() {
                let _ = k_pulse.pulse().await;
            }
        }
    });

    println!("SuperX MCP Server starting on stdio (Tenant: {tenant})...");
    serve_server(McpServer::new(kernel), rmcp::transport::stdio()).await?;
    Ok(())
}
