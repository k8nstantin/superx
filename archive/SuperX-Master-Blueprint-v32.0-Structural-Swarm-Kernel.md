# SuperX Master Blueprint v32.0: The Structural Swarm Kernel

**Classification:** Confirmed Final Architectural Design  
**Paradigm:** Documentation-Driven / Structural DAG as Source of Truth / Decentralized  
**Core Engine:** Semantic Compiler + Telemetry/Execution Host + Federation Router  

## The Airtight Schema Mandate (Zero Bypasses)
SuperX enforces data integrity exclusively at the database layer using native SurrealDB constraints.
1. **No Triggers:** Business logic is never hidden in database triggers. 
2. **No Application Bypasses:** The Rust kernel MUST NOT contain code that attempts to disable, suppress, or work around `SCHEMAFULL`, `REFERENCES`, or `NOT NULL` constraints.
3. **Database as Enforcer:** If an agent attempts to write data that violates the metamodel, the database rejects the transaction.
4. **Structural Supremacy:** The Structural DAG is the absolute source of truth for product definition and execution boundaries.

*See also: [SuperX Physical Execution Roadmap](./SuperX-Physical-Execution-Roadmap-The-Iron-Foundation.md)*

---

## 1. Executive Philosophy: "The Database is the OS"

SuperX discards the traditional "application" model. Instead, the OS treats **Language as the Source Code**, the **Graph as the Compiler**, and **Agents as the CPU**.

*   **Zero Hardcoding:** No concept exists as a table. All domain knowledge is stored as data in a universal metamodel.
*   **Documentation-Driven:** Execution is a byproduct of high-fidelity semantic descriptions. Agents execute by traversing a Directed Acyclic Graph (DAG) of versioned text.
*   **Recursive Inference Swarm:** Intelligence is not a single point. Models can "Consult" other Models or RAGs. A Coder Model might have a `semantic_relation` to a Security Reviewer Model. The OS traverses these "Consultation Edges" to compile multi-model workflows.
*   **Parameterized OS Brain:** The OS’s own internal reasoning (classification, orchestration) is performed by a selectable **System Agent**. Which model is used (e.g., Gemma 2B, Llama 3) is a parameter in the `state_ledger`.

---

## 2. P2P Federation Protocols (The Swarm)

SuperX instances are designed to discover each other, establish trust, and share execution graphs (Product DAGs). This is governed entirely by the database layer.

### A. Autonomous Node Discovery (mDNS & Gossip)
SuperX instances do not rely on central servers to find each other. Discovery is handled at the network edge.
* **Local Swarm (mDNS / ZeroConf):** On a local network, the Rust kernel runs an mDNS responder. It broadcasts its `tenant_uid` and active gRPC port.
* **Global Swarm (Gossip Protocol):** For distributed VMs, you provide a "Seed Node." The kernel uses a Gossip protocol to exchange known peer lists. 
* **The Result:** Discovered nodes are written to the local `entity` table (Type: `node_peer`). 

### B. The Initialization Handshake (mTLS & Ed25519)
SuperX operates on a strict Zero-Trust model. 
1. **Identity Generation:** On first boot, the OS generates an Ed25519 keypair. The public key is the node's true identity.
2. **The Connection Request:** Node A attempts to open a raw TCP connection to Node B.
3. **mTLS Negotiation:** They perform a Mutual TLS (mTLS) handshake.
4. **The Trust Exchange:** Node A sends its Public Key. If approved, Node B stores Node A's public key in its **`vault_ledger`**.
5. **The Graph Edge:** Node B creates a `semantic_relation` edge (`edge_trusts_peer`) pointing to Node A's entity.

### C. The Federation Inbox (Safe Ingestion)
When a trusted peer sends a shared Product Design (a serialized JSON export), it is **never** written directly into the local graph.
1. **Quarantine:** The payload lands in the `federation_inbox` table (`status: 'pending'`).
2. **Cryptographic Proof:** The Rust kernel verifies the payload's Ed25519 signature against the `vault_ledger`.
3. **The Merge:** If verified, the OS parses the JSON and appends new historical records to the local `state_ledger` (SCD-2), merging the branches.

---

## 3. Swarm Collaboration Mechanics (P2P Workflows)

SuperX enables three primary collaborative workflows:

### A. Real-Time Co-Authoring (Causal Sync)
*   **Mechanic:** The local OS monitors its **`crdt_sync_ledger`**. When a local edit creates a Loro delta, the OS kernel signs it and broadcasts it to trusted peers.
*   **Convergence:** Remote peers merge the delta into their local Loro state. Loro’s mathematical properties ensure perfect convergence.

### B. Distributed Task Delegation (Remote Spans)
A local "Product Run" can leverage the compute power of a remote peer.
*   **The OS Bridge:** Node A sends the **`compiled_xml_prompt`** and task parameters to Node B. Node B executes the Wasm tool locally and streams the `telemetry_stream` events back to Node A in real-time.

### C. Proxied Knowledge Access (Shared RAGs)
One node can act as the "Source of Truth" for a specific data source without sharing the raw credentials.
*   **The Proxy:** A request is routed over the mTLS bridge. The host kernel executes the query locally, redacts sensitive data, and returns the result.

---

## 4. Context Portability (The Portable Design Bundle)

SuperX enables the **"Design Locally, Execute Globally"** workflow.

### A. Modular Sub-Graph Export (The Deep Bundle)
The OS kernel can generate a **Deep Graph Bundle** starting from **ANY arbitrary entity** (e.g., a top-level Product or a single Component). 
*   **Structural Traversal:** The kernel treats the selected entity as the "Root." It walks the **`structural_relation`** DAG strictly downwards and packages every linked `entity` and `type_definition`. 
*   **State Snapshot:** It pulls the latest versioned strings from the `state_ledger`.
*   **Credential Portability (P2P Re-Encryption):** It decrypts local secrets, re-encrypts them specifically for the target node's Public Key, and embeds them in the bundle.

### B. Remote Instantiation (The Sync)
*   The signed Deep Bundle is pushed to a trusted peer, lands in the `federation_inbox`, and is instantiated instantly for execution.

---

## 5. The 9-Table Atomic Kernel (The Dual-Topology Substrate)

All OS operations are collapsed into these mathematically pure tables.

### I. `type_definition` (The Metamodel)
The recursive blueprint for the entire universe.
*   **Purpose:** Defines rules for every node, edge, and attribute.
*   **Topology Flag:** Edge types define whether they belong to the Structural DAG or the Semantic Web.

### II. `entity` (The Nodes)
Stable identity anchors.
*   **Mandate:** Contains NO data. It is a pure pointer to a `type_definition`.

### III. `structural_relation` (The DAG Backbone)
The unbreakable structural assembly (e.g., `Product -> [owns] -> Component`).
*   **The Invariant:** This is the **Absolute Source of Truth** for the OS architecture. 
*   **Cycle Prevention:** The Rust kernel strictly monitors this table. Any transaction that creates a loop is physically blocked.
*   **Performance:** Highly optimized for the Semantic Compiler to walk the tree rapidly.

### IV. `semantic_relation` (The Knowledge Web)
Flexible associations (e.g., `Component -> [consults_rag] -> KnowledgeBase`).
*   **The Invariant:** Loose links with no cycle constraints. Cross-referencing is permitted here without slowing down the structural DAG.

### V. `state_ledger` (The Content / SCD-2)
The polymorphic repository for all versioned data.
*   **Purpose:** Stores descriptions, prompts, compiled artifacts, and logic.

### VI. `vault_ledger` (The Secrets)
Isolated, KMS-backed storage for credentials and P2P keys.

### VII. `execution_state` (Ephemeral Progress)
SCD-2 tracking of a Run's progress against specific nodes in the Structural DAG (`idle/active/done`).

### VIII. `telemetry_stream` (The Events / OTel)
The high-volume, immutable audit log of the OS. Captures exact payloads and latency for every command.

### IX. `content_blob` (The Assets)
CAS references for massive out-of-band files (Video/Audio).

---

## 3. The Functional Lifecycle (The 6-Step Loop)

### Step 1: Semantic Intent (Human/Architect)
The Architect writes a **Product Description**. This is not code; it is a versioned semantic intent stored in the `state_ledger`. External links (Google Docs/Confluence) are captured as `relation` edges to `external_url` entities.

### Step 2: DAG Construction (Topology)
The Product is broken into nested **Components**. RAGs, Models, and Skills are linked to these components via the `relation` table. This builds the physical DAG the agent will traverse.

### Step 3: Source Refinery (Infrastructure Extraction)
Pluggable Wasm Extractors (Sensors) watch the external world (MySQL, Iceberg, Kafka, Repos). They process raw infrastructure into **Compiled Data Sources**—versioned artifacts in the `state_ledger` that tell the agent exactly what the schema and usage rules are.

### Step 4: The Semantic Compiler (Aura Plan)
Before execution, the OS kernel performs a **Compilation Run**. It walks the DAG, hydrates it with the latest semantic strings and compiled data sources, and generates a **Compiled XML Prompt** and a **WASI Capability Manifest**. 
*   *Note:* The "Plan" phase validates context window budgets and link integrity before spending tokens.

### Step 4: System Agent Validation (Local Inference)
The OS Kernel natively embeds open-weights models (e.g., **Gemma 2**, Llama-3-8B) via an isolated Rust ML micro-host. These **System Agents** act as the OS's internal immune system and orchestrator. 
*   **The Mechanic:** They read the output of the compilation phase and perform zero-latency, local tasks: classifying user intent, pruning old chat messages, and validating the structural integrity of the XML context.
*   **Parameterization:** The specific model type (e.g., `gemma-2-2b-it`), its quantization level, and its task instructions are defined entirely in the `state_ledger`. The OS automatically routes internal tasks to different local models based on available hardware and DAG parameters.

### Step 5: Autonomous Execution (Cloud / Heavy Agents)
The Agent (Claude/Gemini/etc.) connects via the **MCP Bridge**. It receives the `compiled_xml_prompt` and executes within a strict, deterministic Wasm sandbox. 

### Step 6: Telemetry & Feedback
The Kernel logs every thought and tool call to the `telemetry_stream`. If a failure occurs, the OS uses **Time-Travel Diagnosis** (`fn::current_at`) to rewind the `state_ledger` and find the specific version of the description that caused the confusion.

---

## 4. Collaborative Core: Human-in-the-Loop (HITL)

SuperX is a social environment for humans and agents.

*   **Non-Blocking Interception:** Users can type "Hey, consider this" while an agent is running. This signal is injected into the `telemetry_stream` and pushed to the agent's live context.
*   **Communication Threads:** Chat history is stored in the `state_ledger` (Type: `communication_thread`). It is a first-class citizen of the graph, meaning the Semantic Compiler can "bake" previous chat decisions into the next compiled prompt.
*   **CRDT Sync:** Loro CRDTs handle the "Hot Path" in memory, ensuring that if you and an agent edit the same description, the state converges mathematically with zero conflicts.

---

## 5. Security & Governance Invariants

1.  **Metamodel Sovereignty:** No data can be written that isn't first defined in `type_definition`.
2.  **Hard Referential Integrity:** All links are physically enforced `record` references.
3.  **Immutable Transitions:** The OS has no "Delete" verb. Entities only transition to an `is_deleted = true` terminal state via SCD-2.
4.  **Deterministic Permissions:** WASI manifests are never hardcoded; they are compiled on-the-fly from graph edges (`Agent -[can_execute]-> Tool`).

---

**End of Document**  
*SuperX v21.0: Design Finalized.*
