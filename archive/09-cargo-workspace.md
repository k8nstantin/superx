# AuraOS — Cargo Workspace Topology

The AuraOS source is organized as a strict Cargo virtual workspace to enforce decoupling between the kernel, the bridge, and the execution environments.

```text
/auraos
├── Cargo.toml                  # Workspace root
├── crates/
│   ├── auraos-kernel/          # Core OS loop, Loro CRDT management, SurrealDB sync
│   ├── auraos-mcp/             # rmcp server, Unix Domain Socket handling
│   ├── auraos-wasm/            # wasmtime host, WASI capability manifesting
│   ├── auraos-docker/          # OCI container management, UDS bind-mounting
│   ├── auraos-extractors/      # Wasm swarm, notify-debouncer, tree-sitter wrappers
│   ├── auraos-system-agents/   # Proposer, Evaluator, Classifier (candle inference)
│   └── auraos-telemetry/       # Outbox, rdkafka publisher, Redactor logic
└── tools/
    └── meta-harness-cli/       # CLI for human-in-the-loop promotion approvals
```