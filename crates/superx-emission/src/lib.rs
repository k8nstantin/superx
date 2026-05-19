//! # superx-emission — substrate-to-outside telemetry routing
//!
//! Implements the **fine-grained telemetry collection pillar**
//! (`ARCHITECTURE.md` §0a Pillar 1) on the *egress* side. The `telemetry_stream`
//! table captures every kernel mutation; this crate forwards those rows to
//! downstream sinks (Kafka, HTTP, future OTLP) so an operator can see the
//! firehose from outside the `SuperX` process.
//!
//! ## Entry points
//!
//! - [`TelemetryRow`] — the on-wire shape every sink receives. Mirrors the
//!   `telemetry_stream` schema field-for-field.
//! - [`KafkaSink`] — `rdkafka` `FutureProducer` wrapper.
//! - [`ApiSink`] — `reqwest` HTTP POST with optional bearer auth. Non-2xx is
//!   surfaced as `Err` so silent failures don't defeat the audit-trail
//!   invariant.
//! - [`TelemetrySubscriber::run_loop`] — long-lived background task that
//!   bridges `SurrealDB` `LIVE SELECT` to the configured sinks.
//!
//! ## Design notes
//!
//! - **Sinks are independent.** Configure 0, 1, or both — the subscriber
//!   fan-outs to whichever are present. Future `OtelSink` slots in next to
//!   them per Roadmap #7.
//! - **The subscriber runs in root context.** It cannot use
//!   `Kernel::get_parameter` (which needs a session); instead it reads
//!   `attr_config` directly with the tenant id bound as a `SurrealQL`
//!   parameter. This is the documented exception to the "all writes go
//!   through kernel verbs" rule (Roadmap #13).
//! - **Sink failures log but don't kill the loop.** An emit error on one
//!   sink logs at ERROR and the loop continues — losing a row is better
//!   than losing the firehose.
//!
//! Copyright (c) 2026 Constantin Alexander <constantin@dedomena.io>.
//! Licensed under the Apache License, Version 2.0.

#![deny(warnings)]
#![deny(clippy::pedantic)]

use superx_kernel::{Kernel, KernelError};
use futures_util::StreamExt;
use rdkafka::config::ClientConfig;
use rdkafka::producer::{FutureProducer, FutureRecord};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use surrealdb::Notification;
use reqwest::Client;

/// One row of the unified telemetry firehose, as it travels from
/// `telemetry_stream` to a downstream sink (`KafkaSink` / `ApiSink`).
///
/// Field semantics mirror the `telemetry_stream` schema defined in
/// `superx-kernel::apply_substrate_schema`:
/// - `id` — the row's `Thing` record id (native `Id::Uuid`); preserves
///   the temporal ordering of when the event was logged.
/// - `timestamp` — ISO-8601 wall-clock time the row was written.
/// - `tenant_id` — the tenant that produced the event; sinks may filter on this.
/// - `lifecycle_event` — typed event name (`state_supersede`, `edge_create`,
///   `agent_discovered`, `system_pulse`, …).
/// - `run_id` — optional correlation id linking the event to a workflow run.
/// - `payload` — typed structured payload specific to `lifecycle_event`.
#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct TelemetryRow {
    pub id: surrealdb::sql::Thing,
    pub timestamp: String,
    pub tenant_id: String,
    pub lifecycle_event: String,
    pub run_id: Option<String>,
    pub payload: serde_json::Value,
}

/// Kafka egress sink. Wraps an `rdkafka` `FutureProducer` configured for a
/// single bootstrap-broker list and a single topic. Use one `KafkaSink` per
/// destination topic; sinks are cheap (a tokio-friendly producer handle).
pub struct KafkaSink {
    producer: FutureProducer,
    topic: String,
}

impl KafkaSink {
    /// Creates a new `KafkaSink`.
    ///
    /// # Errors
    /// Returns an error string if the Kafka producer cannot be created.
    pub fn new(brokers: &str, topic: &str) -> Result<Self, String> {
        let producer: FutureProducer = ClientConfig::new()
            .set("bootstrap.servers", brokers)
            .set("message.timeout.ms", "5000")
            .create()
            .map_err(|e| e.to_string())?;
        Ok(Self { producer, topic: topic.to_string() })
    }

    /// `emit`: Serializes and sends a telemetry event to Kafka.
    ///
    /// # Errors
    /// Returns an error string if serialization or transmission fails.
    pub async fn emit(&self, event: &TelemetryRow) -> Result<(), String> {
        let payload_json = serde_json::to_string(&event).map_err(|e| e.to_string())?;
        let key = event.id.to_string();

        let record = FutureRecord::to(&self.topic)
            .payload(&payload_json)
            .key(&key);

        self.producer.send(record, Duration::from_secs(0))
            .await
            .map_err(|(e, _)| e.to_string())?;

        Ok(())
    }
}

/// HTTP egress sink — POSTs each `TelemetryRow` as JSON to a configured URL.
/// Bearer-token authentication is optional. Failure modes:
/// - Non-2xx response → `Err` returned (silent failure would defeat the audit
///   trail invariant).
/// - Network error → `Err` returned.
///
/// One `ApiSink` corresponds to one downstream endpoint; create multiple sinks
/// for fan-out.
pub struct ApiSink {
    url: String,
    auth_token: Option<String>,
    client: Client,
}

impl ApiSink {
    /// Creates a new `ApiSink`.
    #[must_use]
    pub fn new(url: &str, auth_token: Option<String>) -> Self {
        Self { 
            url: url.to_string(), 
            auth_token,
            client: Client::new(),
        }
    }

    /// `emit`: Sends a telemetry event to a remote API via POST.
    ///
    /// # Errors
    /// Returns an error string if the request fails.
    pub async fn emit(&self, event: &TelemetryRow) -> Result<(), String> {
        let mut builder = self.client.post(&self.url);
        if let Some(ref token) = self.auth_token {
            builder = builder.bearer_auth(token);
        }

        let res = builder.json(event).send().await.map_err(|e| e.to_string())?;
        if !res.status().is_success() {
            return Err(format!("API Egress failed with status: {}", res.status()));
        }

        Ok(())
    }
}

/// Long-lived background task that bridges the substrate's `telemetry_stream`
/// (via `SurrealDB` `LIVE SELECT`) to one or more downstream sinks. Spawned
/// by `superx-cli` for the lifetime of a command and by `superx-mcp` for the
/// lifetime of the server process. Runs in the kernel's root context so it
/// can observe cross-tenant telemetry for an instance; per-event tenant
/// filtering is up to the sinks.
pub struct TelemetrySubscriber<'a> {
    kernel: &'a Kernel,
}

impl<'a> TelemetrySubscriber<'a> {
    /// Creates a new `TelemetrySubscriber`.
    #[must_use]
    pub fn new(kernel: &'a Kernel) -> Self {
        Self { kernel }
    }

    /// `run_loop`: Subscribes to the telemetry stream and emits to the provided sinks.
    ///
    /// # Errors
    /// Returns `KernelError` if the subscription or emission fails.
    pub async fn run_loop(&self, kafka_sink: Option<&KafkaSink>, api_sink: Option<&ApiSink>, tenant_id: &str) -> Result<(), KernelError> {
        #[derive(Deserialize)] struct ValueRow { value_json: serde_json::Value }

        tracing::info!("Starting telemetry subscription for tenant: {tenant_id}");

        // Internal loop runs in root context. We bypass get_parameter and use direct queries
        // because we don't have a $auth context in the background thread.
        let query = "SELECT * FROM state_ledger WHERE target.type = type_definition:node_substrate AND tenant_id = $tnt AND `type` = type_definition:attr_config AND is_current = true LIMIT 1";
        let mut config_res = self.kernel.db.query(query).bind(("tnt", tenant_id.to_string())).await?;
        
        let config_row: Option<ValueRow> = config_res.take::<Vec<ValueRow>>(0)?.pop();
        
        let (enabled, scope) = if let Some(row) = config_row {
            (
                row.value_json.get("emission_enabled").and_then(serde_json::Value::as_bool).unwrap_or(true),
                row.value_json.get("data_scope").and_then(|v| v.as_str()).unwrap_or("all").to_string()
            )
        } else {
            (true, "all".to_string())
        };

        if !enabled {
            tracing::info!("Emission disabled for tenant: {tenant_id}");
            return Ok(());
        }

        // 2. LIVE SELECT as root (local mode bypasses PERMISSIONS for root)
        let live_query = "LIVE SELECT * FROM telemetry_stream WHERE tenant_id = $tnt";
        let mut response = self.kernel.db.query(live_query)
            .bind(("tnt", tenant_id.to_string())).await?;

        let mut stream = response.stream::<Notification<TelemetryRow>>(0)?;

        while let Some(res) = stream.next().await {
            match res {
                Ok(notification) => {
                    let row = notification.data;
                    
                    // Parameterized Filtering
                    if scope != "all" && row.lifecycle_event != scope {
                        continue;
                    }

                    if let Some(ks) = kafka_sink {
                        if let Err(e) = ks.emit(&row).await {
                            tracing::error!("Kafka emission error: {e}");
                        }
                    }
                    if let Some(asink) = api_sink {
                        if let Err(e) = asink.emit(&row).await {
                            tracing::error!("API emission error: {e}");
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("Subscription stream error: {e}");
                }
            }
        }

        Ok(())
    }
}
