# SuperX — System Architecture & Data Flow

## 1. System Overview

SuperX is a documentation-driven, everything-is-a-parameter Agentic OS. It transforms the traditionally messy world of agents into a **Semantic Compiler** and a **Telemetry/Execution Engine**. 

The OS operates on a **5-Table Hardened Atomic Kernel** (Metamodel, Entity, Relation, State Ledger, and CRDT Sync). Every rule, permission, and instruction is a queryable graph node, enabling mathematical auditability and infinite runtime extensibility.

## 2. The Functional Cycle

### A. Design Phase (The Semantic Architect)
Architects and Proposers write iterative, SCD-2 versioned descriptions into the `state_ledger`. These are pure semantic intent, linked to external documentation (Google Docs/Confluence) via graph relations.

### B. Ingestion Phase (The Extractor Swarm)
* **Graphify (Files):** Watches the filesystem and parses code into the Universal Graph.
* **Infra Extractors (CDC):** Watches external databases (MySQL, Kafka, Iceberg) and continuously updates the OS's understanding of external schemas and data states.

### C. Compilation Phase (Aura Plan)
Before execution, the OS kernel walks the DAG to perform a **"Aura Plan" (Test Compile)**. It merges semantic intent, tool definitions, and live infrastructure schemas into a **Compiled XML Prompt**. It also compiles a dynamic **WASI Capability Manifest** based on the relationship graph.

### D. The Intelligence Layer (System Agents)
The OS Kernel natively embeds open-weights models (e.g., **Gemma 2 2B/9B**) via Rust ML frameworks (like `candle`). These are not external tools; they are **System Agents**.
* **Zero-Latency Orchestration:** They perform high-speed, local tasks like classifying user intent, summarizing massive communication threads, and validating compiled XML prompts *before* they are sent to expensive cloud models.
* **Parameterized Routing:** The choice of which local model to use (and its specific system prompt) is fully defined by the `state_ledger`. The OS can hot-swap models based on the hardware tier (T1-T5) without changing kernel code.

### E. Execution Phase (The Golden Path)
Agents execute in a sub-10ms Wasm sandbox. They interact with the system via the **MCP Bridge**. 
* **HITL Integration:** Humans can intersect execution via Live Queries, injecting signals ("Hey, consider this") directly into the agent's context window.

### F. Observation Phase (The Telemetry Engine)
Every thought, tool call, and token is logged to the `telemetry_stream`. Machine physics (CPU/RAM) are captured by an external OTel Collector and correlated via `trace_id`.

## 3. Data Invariants

1. **Immutable Transition Log:** Data is never deleted, only transitioned via SCD-2.
2. **Referential Integrity:** The database engine (SurrealDB) enforces `SCHEMAFULL` constraints and hard foreign keys.
3. **Physical Isolation:** Active state (Hot Path), historical ledger (Cold Path), and execution events (Analytics) are physically separated to ensure zero-latency execution.
