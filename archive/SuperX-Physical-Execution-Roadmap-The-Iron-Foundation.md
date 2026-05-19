# SuperX Physical Execution Roadmap: The Orchestrated Swarm v3.0

This document defines the final engineering sequence for implementing **SuperX v40.1**.

*See also: [SuperX Master Blueprint v40.1](./SuperX-Master-Blueprint-v40.1-Self-Configuring-Swarm.md)*

---

## 1. Technological Lock-in (Revision 40.1)

*   **Substrate:** SurrealDB v2.1+ (Embedded RocksDB).
*   **System Bus:** Native Rust Traits (Modular Blades).
*   **Safety:** NASA JPL "Power of 10" (Iterative, Bounded, No Unused code).
*   **Identity:** Absolute UUIDv7 (Temporal Ordering).
*   **Interface:** MCP (rmcp) + A2A (Agent Protocol).
*   **Intelligence:** Llama-cpp-2 (Local GGUF models).

---

## 2. Definitive Engineering Sequence

### Phase 1: The Intelligent Substrate [COMPLETED]
**Goal:** Durable, safety-critical 10-table foundation.
*   [x] **Persistent Boot:** Initialize RocksDB-backed SurrealDB with `SCHEMAFULL` rules.
*   [x] **Metamodel Seed:** Bootstrap the Four Pillars (Code, Data, Knowledge, Compute).
*   [x] **NASA Safety:** Implement iterative DFS cycle detection and bounded traversals.
*   [x] **Temporal Identity:** Enforce UUIDv7 generation for all records.

### Phase 2: Universal Ingestion (Graphify) [COMPLETED]
**Goal:** Map the world into the DAG.
*   [x] **CodeBlade:** Tree-sitter AST deconstruction into `structural_relation`.
*   [x] **ConnectionTester:** Proactive SQL and API heartbeat verification.
*   [x] **CatalogBlade:** AWS Glue and Polaris/Iceberg REST deconstruction.

### Phase 3: The Orchestrated Swarm (Current) [NEXT]
**Goal:** Build the "System 2" reasoning and execution layer.
1.  **Knowledge Distillation:** Update the Semantic Compiler to use local Gemma for pre-compressing DAG state into Task Artifacts.
2.  **MCP/A2A Bridge:** Build the `superx-mcp` server. Expose `node_agent_card` for peer discovery.
3.  **Cyclic Reasoning:** Implement `thread_id` and recursive looping in the `execution_state` table.
4.  **Meta-Harness:** Integrate `wasmtime` to execute autonomous test nodes and feed results back to agents.

### Phase 4: Control Plane & Emission
**Goal:** Visual management and central dashboard telemetry.
1.  **React Canvas:** Implement the `xyflow` DAG visualizer with HITL Interrupt support.
2.  **Emission Diode:** Implement the `policy_emission` Changefeed that pushes telemetry to port 9967.
3.  **Site Plan UI:** Build the bootstrap UI for environment-led self-configuration.

### Phase 5: Swarm Federation
**Goal:** Decentralized P2P sub-graph sharing.
1.  **P2P Secret Re-Encryption:** Ed25519-signed Deep Graph Bundle exports.
2.  **Federation Inbox:** Conflict Resolver Agent to manage split-brain merges.

---

## 3. High-Discipline Constraints

1.  **Consult Before Build:** Before every module implementation, the developer MUST read the v40.1 Blueprint.
2.  **Database as Source of Truth:** No in-memory state can exist without a corresponding persistent record in the 10-table substrate.
3.  **Airtight Security:** All agent actions MUST use the restricted `exec_db` role.

---

## 4. Immediate Tasks

1.  **Finalize `superx-mcp` implementation with `rmcp 1.7` stable types.**
2.  **Implement 'Knowledge Distillation' logic in the Semantic Compiler.**
3.  **Initialize the `superx-harness` Wasm sandbox.**
