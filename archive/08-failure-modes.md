# SuperX — Failure Mode Matrix

SuperX guarantees deterministic recovery across its subsystems.

| Subsystem | Failure Event | Detection Mechanism | Recovery Behavior |
| :--- | :--- | :--- | :--- |
| **SurrealDB (Cold)** | Database file corruption / IOErr | Boot-time consistency check | Refuses to boot. Requires manual snapshot restore or Kafka replay. |
| **Loro (Hot)** | OOM / Buffer Overflow | Rust memory pressure monitor | Triggers emergency Epoch flush (`Trigger.MemoryPressure`). Re-allocates RAM. |
| **Wasm Runtime** | Agent Panic / Infinite Loop | `wasmtime` fuel limits / Trap catch | Instance killed. Evaluator logs failure to SCD-2. System routes failure to Meta-Harness Proposer. |
| **Extractor Swarm** | `notify` dropped file events | `need_rescan()` OS kernel flag | Suspends swarm. Parallel `walkdir` reconciles File IDs (inodes) against CRDT state. Resumes. |
| **MCP Bridge** | Socket death / client disconnect | EOF on Unix Domain Socket | Drops un-flushed MCP edits for that session. Cleans up temporary WASI handles. |
| **Kafka Outbox** | Network outage to broker | `rdkafka` queue backpressure | Redactor pauses emission; relies on SurrealDB `execution_log` as durable cursor until network restores. |