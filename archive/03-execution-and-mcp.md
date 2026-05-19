# SuperX — Execution Layer & The MCP Bridge

SuperX supports the hyper-optimized future (WebAssembly) while providing a robust fallback for the pragmatic present (Docker/Python). The legacy FUSE VFS has been entirely deprecated. We use a strict Protocol-First Bridge built on the Model Context Protocol (`rmcp`).

## 1. Pluggable Execution (`AgentRuntime` Trait)

The Rust kernel abstracts agent execution through a standard trait.

```rust
#[async_trait]
pub trait AgentRuntime: Send + Sync {
    async fn spawn(&self, harness: &Harness) -> Result<Instance>;
    async fn execute(&self, instance: &Instance, invocation: Invocation) -> Result<ExecutionResult>;
    async fn terminate(&self, instance: &Instance) -> Result<()>;
}
```

## 2. The Wasm Runtime & MCP-in-Wasm

For agents compiled to WebAssembly (Rust, Go, JS/TS via Javy), SuperX uses `wasmtime`.

*   **Startup:** Sub-millisecond isolation per task.
*   **Security:** Deny-by-default via dynamic WASI Capability Manifests.
*   **MCP-Server-in-Wasm:** Wasm tools natively expose MCP servers over shared-memory channels. This preserves zero-copy speed (`rkyv`) while remaining fully compatible with the external MCP ecosystem.

## 3. The Docker Runtime (Bind-Mounted UDS)

For agents requiring legacy ML stacks (PyTorch) or heavy headless browsers, SuperX manages OCI containers.

*   **No VFS:** We do not use FUSE or Virtio-FS to map files into the container.
*   **Bridge:** The container interacts with the SuperX Living Graph exclusively via an **`rmcp` Unix Domain Socket (UDS) bind-mounted into the container** at boot (e.g., `/var/run/superx.sock`).
*   **Execution:** Python scripts use standard MCP client libraries to read state, request tools, and commit mutations over this socket.

## 4. External Tooling Integration

The same MCP endpoint serves the external ecosystem seamlessly.

*   **IDEs:** VS Code, Cursor, and IntelliJ query the graph via MCP and project it into their native UI.
*   **Third-Party Agents:** Tools like Claude Code, Aider, or GitHub Copilot connect to the SuperX MCP server to read semantic context and execute native Wasm tools.

## 5. Opt-In FUSE Projection (Exploration Only)

While deprecated as a bridge for agents, a read-only FUSE projection is available as an *opt-in, strictly for human exploration*.

*   **Use Case:** Allows developers to use standard Unix tools (`ls /SuperX/graph/`) to inspect the Living Graph.
*   **Platform Caveats:** 
    *   macOS: Requires `macFUSE` (deprecated by Apple, requires security downgrades).
    *   Windows: FUSE is unsupported natively.
    *   Docker: Mounting this inside a container requires `--privileged` mode (strictly forbidden in production SuperX execution).