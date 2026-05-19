# SuperX — Graphify & Continuous Ingestion

To an Agentic OS, code is not text; code is a highly structured, relational graph. SuperX integrates Graphify's principles (AST extraction, symbol resolution) as a sub-10ms, living system primitive. This is a core differentiator over periodic batch-indexing.

## 1. The Continuous Wasm Extractor Swarm

SuperX does not run batch jobs. The graph breathes with the codebase.

### The `notify` Loop
The Rust kernel runs a background task utilizing the `notify` crate, deeply hooking into macOS `FSEvents` or Linux `inotify`. It monitors the workspace for any file saves.

### Micro-Extractors (Wasm)
When a file (e.g., `src/auth/jwt.rs`) is saved:
1.  The kernel instantly spawns a specialized Wasm Micro-Extractor for the target language.
2.  **`tree-sitter`:** The extractor uses the `tree-sitter` Wasm grammar to parse the file into an Abstract Syntax Tree.
3.  **`stack-graphs`:** It resolves cross-file symbol bindings (determining exactly which struct is being referenced).

### Zero-Copy Graph Patching
Because the Wasm extractor shares memory with the kernel, it directly patches the multi-map Living Graph in RAM.
*   It adds new nodes, tagging them with the correct `tenant_id`.
*   It triggers the Loro CRDT operations.
*   These operations accumulate until the next semantic **Epoch**, which flushes the SCD-2 closure to SurrealDB.

**Latency:** From a developer pressing Cmd+S to the Living Graph reflecting the new AST topology takes **< 10ms**.

## 2. Non-Code Modalities

Graphify principles apply to all context. The kernel maintains extractors for other modalities:
*   **Markdown/Docs:** `pulldown-cmark` extracts headers and links as graph edges.
*   **Media:** Audio dropped into the workspace triggers a Wasm extractor wrapping `whisper.cpp`, converting spoken words into time-stamped text nodes.
*   **External Systems:** Jira/GitHub webhook payloads are routed via MCP into graph nodes, with the Classifier Agent automatically drawing `relates_to` edges.

## 3. Dynamic Algorithms

The graph is enriched continuously in the background by System Agents.

### Community Detection
*   A background task periodically runs clustering algorithms (e.g., Leiden) over the call graph.
*   It detects functional communities (e.g., "Payment Module") and adds semantic edges. On the React spatial canvas (`xyflow`), these symbols visually group together.

### Local Embeddings
*   As nodes are added, a background System Agent uses `fastembed-rs` to generate semantic vectors for the text content.
*   These vectors are stored in SurrealDB's `MTREE` index for instant similarity search across the living graph.