# AuraOS — Resource Quotas & Tenancy Budgets

To prevent "runaway agents" from bankrupting organizations or DOSing the kernel, AuraOS implements strict, multi-tenant resource accounting.

## 1. The `resource_account`
Every `tenant_id` in SurrealDB maps to a `resource_account`. This account holds budgets for:
*   **LLM Tokens:** (Input/Output limits).
*   **Wasm Fuel:** Computational cycles allowed per task.
*   **Database IOPS:** Limits on how many nodes an agent can query per second.

## 2. Enforcement at the Tool-Call Boundary
Quotas are not checked "after the fact."
*   **Wasmtime Fuel:** AuraOS utilizes `wasmtime`'s native fuel metering. If an agent enters an infinite loop, it runs out of fuel and traps instantly.
*   **MCP Interception:** Every MCP request (`rmcp`) is intercepted by a Quota Middleware. If the tenant's token budget is exhausted, the RPC call returns a `429 Too Many Requests` equivalent, forcing the agent to `<yield>`.

## 3. Dynamic Adjustments
The Meta-Harness Proposer can request quota increases for specific complex tasks, which require explicit Human-in-the-Loop approval via the UI before execution.