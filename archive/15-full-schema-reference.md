# SuperX — Master Schema Reference (Revision 24: Validated Vaults)

This is the final, production-hardened blueprint for the SuperX Atomic Kernel. It adopts the "Everything is a Parameter" philosophy while ensuring physical isolation between declarative state, encrypted secrets, causal sync, and analytics.

## The Physical Isolation Invariant
To prevent system-wide I/O lockup and secure credentials, SuperX physically isolates four distinct data flows:
1. **Declarative State**: (Graph & State Ledger) The rules and current reality of the OS.
2. **Encrypted Vaults**: (Vault Ledger) KMS-backed, proactively tested credentials.
3. **Causal Sync**: (CRDT Ledger) High-speed binary deltas for recovery.
4. **Analytics**: (Telemetry) High-volume event traces for OTel and audit.

---

## 1. Declarative Graph & Metamodel (The "System Rules")

### `type_definition` (The Metamodel)
The recursive blueprint for every node, edge, and attribute.
* `type_uid` (string): Unique identifier.
* `category` (string): `node | edge | attribute`.
* `validation` (object): JSON Schema for `state_ledger.value`.
* `eviction_policy` (object): Rules for the Compiler (e.g. `summarize_at_1M_tokens`).

### `entity` (The Anchors)
Stable identity anchors. No content.
* `entity_uid` (string): Immutable ID.
* `type` (record<type_definition>): **REFERENCES** metamodel.

### `relation` (The Graph Edges)
Physical connections connecting any two entities.
* `in` / `out` (record<entity>): Source and Destination.
* `type` (record<type_definition>): **REFERENCES** metamodel.
* **Security Note**: This table stores **WASI Capability Grants** and **User RBAC** (e.g., `User_A -[can_write]-> Product_B`).

### `state_ledger` (The Content)
Polymorphic, SCD-2 versioned repository for all declarative data.
* `target` (record): Points to an `entity` or a `relation`.
* `type` (record<type_definition>): **REFERENCES** metamodel.
* `value` (any): Character-perfect data (validated against `type.validation`).
* `is_current` (bool): Optimized lookup flag.

---

## 2. Security & Credentials

### `vault_ledger` (NEW)
Isolated, KMS-backed storage for API keys, SSH keys, and connection strings.
* `target` (record<entity>): The external data source or model entity.
* `type` (record<type_definition>): Blueprint (e.g., `api_key`, `aws_iam_role`).
* **KMS Integration:**
    * `ciphertext` (option<string>): The encrypted key.
    * `kms_provider` (string): `aws | gcp | vault | local`.
    * `kms_key_id` (string): External KMS reference.
* **Proactive Validation:**
    * `connection_tested` (bool): Must be `true` for the Semantic Compiler to succeed.
    * `last_tested_at` (option<datetime>): Timestamp of the last successful ping.
    * `test_error_msg` (option<string>): Reason for failure.
* **SCD-2 Audit:** Uses standard `is_current`, `valid_to`, `created_by` fields.

---

## 3. Recovery & Reliability (The "Heartbeat")

### `crdt_sync_ledger`
Dedicated, high-speed storage for Loro OpLogs. Physically isolated from Telemetry.
* `target` (record<entity>): The node being mutated.
* `op_log_binary` (bytes): The raw Loro delta.
* `vector_clock` (object): Causal ordering metadata.
* **Purpose**: Used strictly for microsecond in-memory recovery after a kernel crash.

---

## 4. Telemetry & Observation (The "Brain Audit")

### `telemetry_stream`
Append-only OpenTelemetry compliant event log.
* `trace_id` / `span_id`: For external correlation (Prometheus/Zipkin).
* `actor` / `target` (record<entity>): The 'Who' and 'What'.
* `raw_prompt` (record<state_ledger>): Points to the exact compiled prompt version.
* `metrics_ref` (string): Trace ID used to pull CPU/RAM from external OTel Collector.
* **Architecture Mandate**: Machine metrics (CPU/RAM) are **NOT** stored here to prevent disk thrashing.

---

## 5. Multi-Modal & Assets

### `content_blob` (CAS)
Content-Addressable Storage for heavy assets (Video, Audio, Images).
* `blob_uid` (string): Unique ID.
* `checksum` (string): SHA-256 integrity.
* `storage_uri` (string): `local://` or `s3://`.

---

## Usage Mandate for Agents & Engineers
1. **Validation**: All writes to `state_ledger` MUST be validated against the `type_definition.validation` JSON Schema in the Rust Hot Path.
2. **Security**: WASI Manifests are compiled dynamically from `relation` edges. They are never hardcoded.
3. **Purity**: Never store execution intermediate thoughts in `state_ledger`. Thoughts go to `telemetry_stream`. Only "Committed Design" goes to the ledger.
