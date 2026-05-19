# SuperX — The Meta-Harness Evolution Loop

An Agentic OS must improve itself. SuperX treats the "Harness" (the configuration, capability manifest, prompts, and tools of an agent) as a versioned program. The OS runs a continuous Propose -> Evaluate -> Promote loop.

Because SuperX mandates strict SCD-2 persistence, the Meta-Harness has access to a mathematically perfect, time-traveled audit log of every failure and success.

## 1. The Anatomy of a Harness

A Harness is a strongly-typed `entity` row in SurrealDB. It contains:
*   **System Instructions:** The base prompt (using strict XML/tagged prompt discipline).
*   **WASI Capability Manifest:** The deterministic list of endpoints and directory access granted to the agent.
*   **Tool List:** Pointers to the Wasm modules the agent can execute.
*   **Completion Check:** A deterministic predicate evaluating if the task is "Done."

## 2. The Evolutionary Cycle

The cycle is driven by System Agents and gated by Human-in-the-Loop review.

### Step 1: The Proposer (Diagnosis)
When an agent fails a task:
1.  **Time-Travel Diagnosis:** The Proposer agent uses `fn::current_at()` to pull the exact state of the Living Graph and the `execution_log` at the moment of failure.
2.  **Counterfactual Reasoning:** The Proposer asks: *"Why did it fail? Was a tool missing?"*
3.  **Proposal Generation:** The Proposer generates a new Harness configuration. It may write a new Wasm tool and attach it to the proposed Harness.
4.  **SCD-2 Storage:** The new Harness is saved as a *proposal row* in the database. 

### Step 2: The Evaluator (Testing)
1.  The Evaluator agent receives the proposed Harness.
2.  It pulls a historical "Evaluation Set" from SurrealDB.
3.  It spawns isolated, temporary Wasm instances with the *proposed* Harness and runs them against the evaluation set, recording scores.

### Step 3: The Human-in-the-Loop Promotion Gate
SuperX strictly avoids the "supply-chain attack surface" of automatically deploying agent-written code to production.

1.  If the Evaluator determines the proposal shows a statistically significant improvement, it flags the Harness as `Ready for Review`.
2.  **Human Approval:** A human developer is notified via the React `xyflow` UI or an MCP notification. The developer reviews the proposed Wasm tool source code and the evaluation traces.
3.  **Promotion:** Only upon explicit human approval is the `supersede()` transaction fired, closing the old Harness and making the new one active in the production environment. 

## 3. Why Strict SCD-2 is Non-Negotiable

Without strict SCD-2, the Meta-Harness is blind.
*   If a file was overwritten during a failed run, the Proposer couldn't see the exact code that confused the agent.
*   Because SuperX saves *everything* securely to SurrealDB (and emits cleansed data to Kafka), the Proposer has a perfect, time-aligned "filesystem" of history to learn from.