# SuperX — Cargo Workspace Topology

The SuperX source is organized as a strict Cargo virtual workspace to enforce decoupling between the kernel, the bridge, and the execution environments.

```text
/superx
├── Cargo.toml                  # Workspace root
├── crates/
│   ├── superx-kernel/          # Core OS loop, Loro CRDT management, SurrealDB sync
│   ├── superx-mcp/             # rmcp server, Unix Domain Socket handling
│   ├── superx-wasm/            # wasmtime host, WASI capability manifesting
│   ├── superx-docker/          # OCI container management, UDS bind-mounting
│   ├── superx-extractors/      # Wasm swarm, notify-debouncer, tree-sitter wrappers
│   ├── superx-system-agents/   # Proposer, Evaluator, Classifier (candle inference)
│   └── superx-telemetry/       # Outbox, rdkafka publisher, Redactor logic
└── tools/
    └── meta-harness-cli/       # CLI for human-in-the-loop promotion approvals
```