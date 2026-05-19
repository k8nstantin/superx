# AuraOS — Emission Engineering

AuraOS is designed for enterprise environments where every action must be auditable, analyzable, and secure.

## 1. The Outbox Pattern
Directly writing to Kafka from the hot path introduces network latency and reliability risks. AuraOS uses the **Outbox Pattern**.
*   All state changes and agent logs are written to the local SurrealDB `execution_log` table in the same transaction as the SCD-2 state change.
*   A background `tokio` task (The Emitter) independently tails this table.

## 2. Cursor Persistence & Idempotency
*   The Emitter maintains a `last_emitted_id` cursor within SurrealDB.
*   If the system crashes, the Emitter resumes exactly from the cursor.
*   Kafka sinks are configured for idempotency. Each event contains a unique `transaction_id`.

## 3. The Redactor Agent (PII & Secrets)
*   Before an event leaves the kernel, it is passed through the Redactor Wasm agent.
*   The Redactor cross-references the event payload against the KMS `secrets` table and known PII regexes, masking sensitive data (`***`) before Kafka serialization.

## 4. Multi-Sink & Topics
Events are strictly routed by topic:
*   `auraos.telemetry.mutations`: Structural CRDT changes (for data-warehouse replication).
*   `auraos.telemetry.agent_traces`: `<thought>` and `<action>` logs for offline ML training.
*   `auraos.telemetry.security`: Capability violations and access denials.