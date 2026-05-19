# SuperX — Interactive Activity Capture

SuperX captures human intent just as rigorously as agent intent. Developer actions in the IDE or React Canvas are logged as first-class semantic events, creating training data for future agents.

## 1. The `interactive.*` Event Taxonomy
Every UI action in `xyflow` or the MCP IDE plugin emits a structured event to the Living Graph:
*   `interactive.canvas.pan`: User focuses on a specific community of nodes.
*   `interactive.node.inspect`: User reads documentation for a function.
*   `interactive.edge.create`: User manually links a Jira ticket to a code node.
*   `interactive.harness.approve`: Human approves a Meta-Harness promotion.

## 2. Intent Mining
These events are not just analytics; they update the CRDT state. If a developer spends 10 minutes inspecting nodes related to "Authentication" before asking an agent a question, the agent's context window is automatically primed with the "Authentication" subgraph.

## 3. Seamless Hand-off
Because human and agent mutations exist in the same Loro CRDT space, a human can start a task, pause, and say to the OS: "Finish what I started based on my last 5 `interactive.node.edit` actions."