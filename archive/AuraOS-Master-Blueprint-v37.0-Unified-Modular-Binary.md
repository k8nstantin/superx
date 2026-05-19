# AuraOS Master Blueprint v37.0: The Unified Modular Binary

**Classification:** Final Immutable Architecture (Revision 37.0)  
**Paradigm:** Documentation-Driven / Unified DAG Supremacy / Persistent Execution  
**Distribution:** Unified Modular Binary (UMB) / Static-Linked Providers  
**Core Ecosystem:** Substrate Kernel, Universal Deconstructor, Semantic Compiler, Agent Sandbox, Emission Router

---

## 1. The Airtight Schema Mandate (Revision 37.0)
1. **Zero Triggers:** Integrity is enforced exclusively via declarative `SCHEMAFULL` constraints.
2. **DAG Supremacy:** The `structural_relation` table is the absolute source of truth. The Rust Kernel MUST perform an iterative DFS cycle check before committing structural edges.
3. **Temporal Ordering:** All generated IDs MUST use **UUIDv7**.
4. **Compilation as Transformation:** Compiling a prompt, data source, or catalog creates a **New Entity Node** (`is_compiled: true`), linked via `edge_compiled_from`.
5. **Verified Source Lifecycle:** New data/intelligent sources (DBs, Models, RAGs) must pass a mandatory **Connection Test** and **Compiler Integrity Audit** before being activated in the DAG.
6. **Knowledge Distillation:** The Semantic Compiler MUST perform "Task-Specific Compression" using local inference (Gemma) to distill raw DAG state into high-density XML artifacts.

---

## 2. The 10-Table Atomic Substrate (Powered by `surrealdb`)

All intelligence is stored in a single, un-partitioned graph utilizing the embedded SurrealKV engine (backed by RocksDB).

1. **`type_definition`**: The Metamodel (`is_compiled`, `is_kernel_locked`).
2. **`entity`**: Stable identity anchors for all things (Products, Agents, Messages, Tests).
3. **`structural_relation`**: The Backbone DAG (owns, requires, replies_to). Cycle-blocked.
4. **`semantic_relation`**: The Knowledge Web (mentions, consults). Cycles allowed.
5. **`state_ledger`**: The Content. SCD-2 versioned strings, JSON, and prompts.
6. **`vault_ledger`**: KMS-backed, proactively tested encrypted secrets.
7. **`execution_state`**: SCD-2 tracking of DAG node execution progress (idle/active/validating/done).
8. **`telemetry_stream`**: High-volume OTel-compliant flat trace of agent thoughts and physics.
9. **`content_blob`**: CAS pointers for massive files.
10. **`federation_inbox`**: P2P staging area. Includes a Conflict Resolver Agent for Split-Brain merges.

---

## 3. Distribution Model: The Unified Modular Binary (UMB)

AuraOS is distributed as a single, "Gig Fat Fuck Executable" that encapsulates the entire intelligence stack. While the codebase is modular (Rust crates), the final distribution statically links all providers to ensure zero external dependencies.

*   **Statically Linked Kernel:** Embedded **RocksDB** and **SurrealDB**.
*   **Statically Linked Ingestors:** **Tree-sitter** grammars for all major languages.
*   **Statically Linked Brain:** Native **Llama-cpp** (Metal/CUDA optimized) for local Gemini weights.
*   **Statically Linked Sandbox:** **Wasmtime** runtime for secure tool execution.
*   **Statically Linked APIs:** **Axum/Tokio** for MCP and WebSocket emission.

---

## 4. The Functional Architecture (The 5 Core Modules)

### A. Graphify: The Universal Deconstructor
*   **Purpose:** Deconstructs **ANY** data source—Codebases, Data Lakes (Iceberg), or Data Catalogs (Polaris, Glue)—into the Unified DAG.
*   **Verified Onboarding:** Mandatory heartbeat check using `ConnectionTester`.
*   **Libraries:** `tree-sitter`, `iceberg-rust`, `aws-sdk-glue`, `sea-schema`.

### B. The Context Assembler: The Semantic Compiler
*   **Purpose:** Transforms raw DAGs into executable Context Windows (Knowledge Infrastructure).
*   **Knowledge Distillation:** Shifting reasoning from *inference-time* to *compile-time*. Uses local **Gemma** weights to pre-summarize and structure data into Task Artifacts.
*   **Integrity Audit:** Final connectivity and schema compatibility check before artifact creation.
*   **Libraries:** `surrealdb` (Traversal), `llama-cpp-2` (Distillation), `tiktoken-rs` (Budgeting).

### C. The MCP Execution Bridge: The Agent Sandbox
*   **Purpose:** Exposes the OS to local and cloud LLMs via a standardized protocol.
*   **Mechanism:** Context is paged from the DB into MCP Resources "Just-In-Time".
*   **Libraries:** `rmcp` (Standard Rust SDK).

### D. The Meta-Harness: Autonomous QA
*   **Purpose:** Ensures agentic execution is grounded and verified.
*   **Mechanism:** Executes test nodes in Wasm sandboxes and feeds results back to the Communication DAG.
*   **Libraries:** `wasmtime`.

### E. The Emission Router: Telemetry & Central Control
*   **Purpose:** Streams real-time OS state to central dashboards.
*   **Mechanism:** Governed by `policy_emission` nodes. Uses `CHANGEFEED` to mirror persistent state to external sinks.
*   **Libraries:** `axum` (WebSockets), `tonic` (gRPC).

---

## 5. Persistence-First Swarm Topologies
*   **Durability Over RAM:** AuraOS prioritizes persistent RocksDB writes over volatile memory caches. Intelligence survives reboots natively.
*   **P2P Sharing:** Sub-graph exports with re-encrypted credentials for decentralized swarm collaboration.

---
**End of Document**  
*AuraOS v37.0: Unified Modular Binary Locked. Architecture Finalized.*
