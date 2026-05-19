# AuraOS — Full Library Inventory

AuraOS leverages specific, state-of-the-art libraries to avoid reinventing the wheel while maintaining extreme performance.

## Rust Stack (The Kernel)
*   **`wasmtime` / `wasmtime-wasi`:** The core execution engine. Unmatched security and speed.
*   **`loro`:** High-performance CRDTs. Chosen over Automerge for its shallow-snapshot capabilities and movable tree structures.
*   **`surrealdb` (`surrealkv`):** The embedded cold-path database. Provides native SCD-2 versioning.
*   **`tree-sitter` / `stack-graphs`:** Extractor swarm foundation for AST parsing and symbol resolution.
*   **`notify-debouncer-full`:** Filesystem monitoring with robust inode-tracking for rescan reconciliation.
*   **`rig-core`:** Standardized LLM provider abstractions (OpenAI, Anthropic, local).
*   **`candle-core`:** Local, bare-metal ML inference for T1/T2 system agents.
*   **`fastembed-rs`:** High-speed vector generation for local text embedding.
*   **`tantivy`:** Full-text search index (paired with SurrealDB for complex lexical queries).
*   **`rmcp`:** The Model Context Protocol Rust implementation.
*   **`rdkafka`:** Enterprise outbox telemetry emission.

## React Stack (The Visual UI)
*   **`vite`:** Build tooling and dev server.
*   **`xyflow` (React Flow):** The spatial canvas. Purpose-built for node-edge editing, infinitely superior to basic chart libraries like ECharts for topological exploration.
*   **`tailwind` + `shadcn/ui`:** Component design system for deterministic, clean UI.
*   **`tanstack-query`:** Async state management for interacting with the AuraOS MCP bridge over WebSockets.