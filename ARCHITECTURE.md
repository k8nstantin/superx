# SuperX Architecture: The Hardened Cognitive Substrate + Autonomous Product Builder (v42.15)

## Status (implemented surface)

- `cargo test --workspace`: **44 / 44 passing**
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: **clean**

SuperX is a safety-critical Agentic Operating System built on a five-table graph substrate plus a durable execution-cursor table. It enforces NASA JPL Power of 10 safety rules, physical read/write multi-tenancy, and character-perfect temporal identity. Modular by design — usable as a pure telemetry collector, a graph/DAG store, an MCP backend, **and (per vision §0 below) an autonomous orchestrator that builds complete software products from cascading DAGs**.

## 0. Vision — SuperX Designs *and* Builds Products

SuperX is not a chat wrapper, an LLM router, or a "substrate other systems drive." Its destination is **an autonomous product designer and builder**:

1. **Design phase.** Given a high-level product intent (e.g. "build a customer-support SaaS that ingests Postgres + Iceberg + RAG over Confluence and exposes a REST API"), SuperX (via the `DesignerBlade`, see §9) walks its catalog of compiled data sources, reusable components, and model entities and **produces a cascading DAG**: `node_product → node_component* → node_task* → node_artifact*`, with explicit `edge_owns`, `edge_implements`, `edge_compiled_from` edges.
2. **Orchestration phase.** A `schedule` table receives the resulting tasks; an `execution_params` table receives the per-run knobs (temperature, branch, turn budget, retry policy, …). A background `RunnerBlade` (see §8) consumes due items in dependency order, reads its `execution_params`, sets the session auth for the assigned agent, dispatches to the right tool via `CapabilityGovernor`, persists artifacts back to the substrate as superseded state, and emits fine-grained telemetry per step. Agents *follow* the DAG SuperX designed; they do not invent it ad-hoc.

Both phases are first-class. The substrate, capability governor, meta-harness, and emission router that exist today are the durable foundations; the *Designer* and *Runner* primitives are the next architectural additions.

### Three deployment modes (modular — same binary, scope selected by config)

SuperX is designed so the same binary can run in any of three modes selected by which background loops + tool sets are wired:

| Mode | What it does | Minimum crates active | Use case |
|---|---|---|---|
| **A — Telemetry collector** | Ingests fine-grained activity from every agent on the system into `telemetry_stream`; forwards via Kafka/HTTP/OTLP sinks. No design, no orchestration. | `superx-kernel` + `superx-emission` + `superx-bootstrap` (discovery only) | A drop-in observability node for an existing fleet of MCP agents. |
| **B — Design / Daggify tool** | Compiles data sources, components, models, and existing code into the substrate. Runs the Designer to produce cascading product DAGs. Does *not* execute them. | + `superx-ingest` + `superx-compiler` + `superx-inference` + (future) `superx-designer` | An intelligent Graphify-style modeller; the operator sees the proposed DAG and chooses what to build. |
| **C — Full agentic OS** | Modes A + B *and* drives the Runner: schedule consumption, dispatch through Governor + Meta-Harness, autonomous product build. | All crates | The full vision — designs, schedules, executes, audits. |

Mode selection is a parameter (`attr_config.deployment_mode` ∈ `{collector, designer, full}`), per principle #7. The CLI exposes mode via `--mode` on `bootstrap` (planned, see §8 item 11).

### Masters of all trades — the excellence bar per pillar

SuperX explicitly rejects the "jack of all trades, master of none" trap. Every pillar must be a **first-class, best-in-field capability**, not a checkbox feature. Each deployment mode below carries an excellence criterion that any code under it must clear before it lands.

| Pillar | Excellence bar — what "masters of" means here |
|---|---|
| **Fine-grained telemetry collection** | Better than OpenTelemetry-out-of-the-box for *agent* workloads. Typed lifecycle events with structured payloads at every kernel mutation; per-tenant isolation enforced at the DB layer (not at the consumer); SCD-2 audit trail; export to Kafka, HTTP, OTel-GenAI semantic conventions simultaneously. The bar: a forensic operator can reconstruct *exactly* what happened on any tenant at any wall-clock instant. |
| **Data ingestion / RAG / "daggify"** | Better than LlamaIndex at structural+semantic fusion. Every data source — SQL, Iceberg, S3, RAG corpus, code repo, web — becomes a `node_data_source` ingested once, queryable forever, no per-run re-learning. Structural traversal (`edge_owns`/`edge_compiled_from`) + semantic similarity (`edge_semantic` via SurrealDB MTREE) inside the same compile pass. The bar: an agent given "find the auth handler" gets both the parent module *and* the semantically related code paths in one `compile_context` call. |
| **Design / DAG construction** | Better than CrewAI/AutoGen at producing **executable** designs. The Designer doesn't just emit a plan — it emits a DAG of typed entities the Runner can immediately schedule. Reusable components are first-class; promoted components are graph-derived, not config-file-derived. The bar: an operator describes a product in English and the Designer produces a cascading DAG that the Runner can execute without human cleanup. |
| **Human-in-the-loop control** | Better than LangGraph at HITL primitives. Pause-and-resume is free from the SCD-2 schedule semantics: a schedule item with `status=awaiting_human` and a Governor-required `edge_can_approve` from an operator agent is a pause point. Approval becomes a state transition with full audit. Operators see the entire history of every approval gate. The bar: zero ad-hoc "wait_for_human" flags scattered across the codebase — HITL is a substrate-level pattern. |
| **Autonomous agentic execution** | Better than AutoGen at safety + auditability. Actor model + topics + subscriptions over the schedule, capability-gated dispatch, wasm-fuel-scored evaluation, full telemetry per step. No silent failures, no untracked retries, no escaping the substrate. The bar: an autonomous run produces a complete, replayable, time-travel-queryable audit trail. |
| **Local model plugability** | Better than Ollama at *substrate integration*. Every model — Gemma 4, Llama 4, Qwen 3, etc. — is a `node_hardened_model` entity with its own `execution_params` row. Pluggable via Rig.rs's `CompletionModel` trait. The Meta-Harness scores model proposals the same way it scores code proposals. New model release → ingest the model card → run distillation benchmarks → promote if it beats the current default. The bar: dropping a new GGUF into the substrate is a single command and the system *automatically* evaluates whether to adopt it. |

### Local-model story — first-class, not fallback

Local inference is a strategic pillar, not a stopgap. When Gemma 4 / Llama 4 / Qwen 3 (and the next family after that) drops, SuperX gains capability by *ingesting the model entity*, not by waiting on a vendor SDK update. Concretely:

- **Model-as-entity.** Each model is a `node_hardened_model` row with `attr_desc` (model card), `attr_config` (default params), `edge_implements` (the `ChatModel` trait), `attr_score` (Meta-Harness evaluation), and `attr_capability_score` (per-task benchmark scores — see §0c below). Adding a new model is a substrate write, not a code change.
- **Execution params owned by the run, not the binary.** Temperature, top_p, top_k, max_tokens, turn budget, retry policy — all live in the `execution_params` table (roadmap #1b) keyed by `run_id`. Two concurrent runs of the same model with different knobs are first-class.
- **Prompts are entities too.** Every prompt template lives in the substrate as a `node_artifact` with `attr_desc` payload (roadmap #6). Blades read their prompts via `Kernel::compile_context` at run start. Operators add custom prompts the same way they add any other entity. The Meta-Harness can supersede prompt entities the same way it promotes code proposals.
- **Provider neutrality via Rig.rs.** Local Candle inference is one `CompletionModel` implementation; remote Anthropic/OpenAI/etc. are siblings. The selected provider for any given task is a row in `execution_params`, not a CLI flag or env var.
- **Meta-Harness evaluates models the same way it evaluates code.** A new model release is treated like a code proposal: run the wasm scoring harness against a canonical benchmark, get a deterministic score, promote-or-reject. The promotion path for `Gemma 4 replaces current default` is identical to the promotion path for `proposed-function replaces old-function`.

This is *why* SuperX is a different kind of system: not because it has local inference (Ollama / mistralrs / LM Studio do), but because **every model is a first-class promotable, evaluable, parameterizable, customizable entity inside the same substrate that holds the products being built**. There is no "local model" code path — it's just another `node_hardened_model`.

### 0c. SuperX is always intelligent — model is integral, not optional

**SuperX is an intelligent agentic OS, not a dumb tool server.** A model is *always* present — packaged with the OS the same way the kernel is. The model is not an optional dependency you bolt on; it is part of what SuperX *is*. This is the directive that distinguishes SuperX from every other Rust agentic library on the market.

#### What "always intelligent" means concretely

| Behavior | Without the model | With the model (default) |
|---|---|---|
| New entity lands in `entity` | Stays raw / unlabeled | Classified by the **ContinuousClassifierBlade** (`node_classification` entity attached) |
| Telemetry burst occurs | Operator scrolls through `stats` | **InsightHarvesterBlade** emits `node_insight` rows summarising the burst |
| `compile_context` called on a node_code | Returns raw `attr_desc` text | Augmented with the on-disk-cached `attr_summary` Gemma generated in the background |
| `Designer` produces a DAG | Operator inspects manually | Gemma critiques the DAG (`node_critique`) before the Runner ever consumes it |
| Repeated task failures | Pile up in the schedule | Gemma proposes new `execution_params` via `node_param_proposal`; Meta-Harness scores and promotes |

The continuous-intelligence blades are first-class background tasks alongside the telemetry subscriber and heartbeat pulse. They subscribe to `telemetry_stream` and react to substrate changes in real time.

#### Model selection — pluggable, capability-routed, never hardcoded

Every model is a `node_hardened_model` entity carrying an `attr_capability_score` payload of the form:

```json
{
  "classify_code":     0.94,
  "summarize_text":    0.91,
  "design_product":    0.62,
  "judge_evaluation":  0.88,
  "detect_anomaly":    0.85,
  "propose_edge":      0.78,
  "tune_params":       0.81,
  "_meta": { "benchmark_run_id": "...", "sample_count": 500 }
}
```

The **ModelRouterBlade** picks the cheapest model whose score clears the per-task threshold (also a parameter on `attr_config`). Concrete routing matrix:

| Task | Cheapest first | Escalation rule |
|---|---|---|
| Classify a new file | Local model (Gemma 4 or equivalent) | Never escalate — high throughput, no API cost permitted |
| Summarize a code file / context window | Local model | Never escalate |
| Detect telemetry anomaly | Local model | Never escalate |
| Tune execution_params | Local model → Meta-Harness scores the proposal | Meta-Harness rejects bad proposals |
| Design a complex product | Local model (first draft) → escalate to Claude/GPT only if local self-confidence < threshold OR DAG-critique fails | One escalation, then HITL pause |
| Judge a proposal subjectively | Local model votes + Meta-Harness scores; disagreement → escalate | Catches local model blind spots |

Escalation rules are parameters on `attr_config`, not code. An operator who wants "always local" sets every escalation threshold to ∞; an operator who wants "always best" sets them to 0.

#### Continuous-intelligence blades (new, MVP-scope per §0d)

These run in the background after bootstrap completes, subscribed to `telemetry_stream` via SurrealDB `CHANGEFEED`:

| Blade | Subscribes to | Writes |
|---|---|---|
| `ContinuousClassifierBlade` | `state_supersede` (new entity-state rows) | `node_classification` linked via `edge_classifies` |
| `InsightHarvesterBlade` | every event, windowed | `node_insight` rows + `edge_observes` to source events |
| `SummarizerBlade` | `state_supersede` on `attr_desc` rows whose payload exceeds a token threshold | `attr_summary` SCD-2 row on the same entity |
| `EdgeProposerBlade` | new `node_code` / `node_artifact` entities | `node_proposed_edge` candidates, Meta-Harness gated |
| `ParamTunerBlade` | `schedule_failed` events repeating beyond a threshold | `node_param_proposal`, Meta-Harness gated |
| `ModelRouterBlade` | every dispatch decision | selects model + writes the choice to `execution_params` |

All six speak the same `ChatModel` trait (Rig.rs). All six can use any model the substrate knows about. None of them care whether the model is Gemma 4 local, Claude remote, or something else.

#### HITL is non-negotiable

Continuous intelligence does **not** replace the human. Every blade above writes *proposals* — `node_proposed_edge`, `node_param_proposal`, `node_critique` — that are visible to the operator and reviewable. The HITL primitive (§0a Pillar 4) gates anything with material consequences:

- **Auto-applied (no HITL):** classifications, summaries, insights, telemetry anomaly notes (read-only observations).
- **HITL-gated by default:** edge promotions, param updates, model swaps, DAG modifications (anything that changes substrate behavior). Schedule items in `awaiting_human` status require an `edge_approves` from an operator agent.

The default thresholds are operator-tunable via `attr_config`. An operator who wants pure autonomy can set HITL gates to "auto-approve high-confidence." An operator who wants oversight on everything can flip every gate to `awaiting_human`.

#### Models directory convention

When models are local (the default expectation — SuperX is "best local model, packaged"), they live under a workspace-relative `./models/` directory. The path is gitignored. The substrate holds metadata; the operator (or `superx-cli install-model <name>`) populates the directory. Path discovery order:

1. `--model-path` CLI flag if provided
2. `SUPERX_MODEL_PATH` env var
3. `node_hardened_model.attr_config.local_path` from the substrate
4. `./models/<model-uid>.gguf` (workspace default)
5. Fall back to remote model if configured, else fail with a clear "no model available" message

Remote-model entities (Claude, GPT-5, Gemini) carry their endpoint URL + auth-token-ref in `attr_config`; no local file required.

### 0d. MVP scope — what "first viable product" means

The MVP is the **collector deployment mode** with the foundations of intelligent classification:

| Capability | MVP requirement |
|---|---|
| **Fine-grained telemetry capture for any agent** | Bootstrap auto-discovers every detectable MCP client on the system (Claude Desktop, Claude Code, Cursor, Continue, Cody — extensible probe set). Every kernel mutation emits typed `telemetry_stream` rows. **Critical: this is THE function — must work flawlessly.** |
| **Bootstrap auto-onboards everything detectable** | `superx-cli bootstrap --tenant <t>` requires no further operator action. Agents are registered, capability edges seeded, telemetry capture begins immediately. |
| **CLI parity for all substrate operations** | `bootstrap, graphify, compile, propose, evaluate, promote, identify, list-agents, list-tools, demo, stats, enqueue` — every kernel verb has an operator-facing CLI surface. |
| **A model is always loaded** | Default = Gemma 4 (smallest reasonable Q4_K_M GGUF, ~16 GB or smaller). Loaded from `./models/` per the discovery order above. If absent, SuperX still boots and warns; the continuous-intelligence blades degrade gracefully. |
| **At minimum, the model serves as RAG** | `CompilerBlade` calls the model for distillation; `ContinuousClassifierBlade` runs in background. Even if no Designer / Runner is configured, the user gets *intelligent context retrieval* from any ingested data source. |
| **Modular, parameterizable, customizable** | Every behavior toggle (HITL thresholds, escalation rules, classifier vocabulary, model selection per task) is an `attr_config` entity. No hardcoded magic. |

What is explicitly *out of scope* for MVP:
- DesignerBlade (roadmap #3)
- RunnerBlade (roadmap #2) — though schedule + execution_params tables ship in MVP so future Runner has somewhere to read from
- Chat UI (roadmap #12)
- OTLP egress (roadmap #7) — Kafka + HTTP sufficient at MVP
- Multi-agent debate / handoff (roadmap #19)

### Core principles (binding for every blade)

1. **Everything is an entity.** Products, components, tasks, data sources, prompts, agents, model versions — all rows of `entity` with a `type` pointer into the metamodel. *No special tables for any of these.*
2. **Types are themselves entities** (rows of `type_definition`). Users — and SuperX itself, via the Meta-Harness — can register new entity types at runtime. As model families evolve (Gemma 3 → Gemma 4 → …), new model entities are added without schema migration.
3. **Products are cascading DAGs.** A `node_product` entity owns `node_component`s via `edge_owns`; components own `node_task`s; tasks own `node_artifact`s. Building the product means walking the DAG, scheduling work at each level, persisting output as superseded state on the parent entity.
4. **Data sources are compiled entities.** MySQL/Postgres/Iceberg/S3 schemas, RAG corpora, local GGUF models, remote API endpoints — each is a `node_data_source` (or specialised subtype) ingested once and referenced thereafter. Agents do not relearn them per run; they pull the compiled context via `CompilerBlade::compile`.
5. **Reusable components.** A `node_component` compiled once into the substrate can be `edge_implements`-linked from multiple products. Promotion via Meta-Harness moves vetted components from "candidate" into "trusted reuse pool."
6. **Schedule + runner drive execution.** A `schedule` table holds pending work items keyed by `run_id`; a background `RunnerBlade` consumes due items, sets session auth, dispatches the appropriate tool/agent, and writes results back through the existing kernel verbs. Agents follow the product DAG; they don't generate it.
7. **No hardcoding.** Every safety bound, every prompt, every threshold, every data-source connector is a parameter — stored on the substrate's `attr_config` ledger row and read via `Kernel::get_parameter`. The Meta-Harness can supersede any parameter the same way it promotes a code proposal.
8. **No database-rule bypass.** SurrealDB `PERMISSIONS` clauses, schema-level `ASSERT`s, and tenant isolation are enforced in the database, not in application code. Application code uses a service account that proves its tenant identity via `set_session_auth`; it does not have super-user credentials at runtime.

### What this means for the current code

- The 5-table substrate is the kernel of (3); already implemented.
- The MetaHarness wasm sandbox is the verification step for (5); already implemented.
- The CapabilityGovernor is the policy layer that gates (6); already implemented.
- **The schedule + runner of (6) is the load-bearing primitive not yet built.** Everything else is in place to receive it. The new work items below in §9 *Roadmap* are scoped against this vision.

### 1. The 5-Table Substrate + Cursor (SurrealDB + RocksDB)

| Table                  | Role                                              | Identity Model |
| :--------------------- | :------------------------------------------------ | :------------- |
| **`type_definition`**  | Metamodel (Core, Working, Archival, Recall tiers) | Unique slug    |
| **`entity`**           | Identity (the persistent *who*)                   | UUIDv7         |
| **`relation`**         | Unified graph (`in → out` pointers)               | UUIDv7         |
| **`state_ledger`**     | SCD-2 content (the versioned *what*)              | UUIDv7         |
| **`telemetry_stream`** | Unified firehose (`CHANGEFEED 1d`)                | UUIDv7         |
| **`execution_cursor`** | Durable workflow checkpoints                      | `run_id` (UUIDv7) |
| **`schedule`** ⏳        | Pending / running / completed work items          | UUIDv7 (planned, §8 #1) |
| **`execution_params`** ⏳ | Per-run agent knobs (temperature, branch, turn budget, retry policy, …) | UUIDv7 (planned, §8 #1b) |

⏳ = planned, not yet in `apply_substrate_schema`. See §8 Roadmap.

All record IDs use native `surrealdb::sql::Id::Uuid` — no string-hashing or MD5 shortcuts. Temporal ordering by id is preserved across every substrate table.

### 2. Core Safety Invariants

1. **Physical Multi-Tenancy.** Every query is gated by SurrealDB `PERMISSIONS` clauses against `$session_tenant` / `$session_role`, set via `db.set(...)` at session start. The kernel additionally hand-filters by `tenant_id = $session_tenant` on every read so isolation holds even if a caller bypasses the access layer. Verified by `test_tenant_isolation_nasa_hardened`, `telemetry_is_tenant_isolated`, `governor_handshake_rejects_cross_tenant_identity`.
2. **Temporal Identity.** Every mutation is stamped with a UUIDv7 across all six tables, ensuring history is naturally ordered and verifiably cross-referenced. Verified by `uuidv7_entity_ids_preserve_temporal_ordering`.
3. **NASA Power of 10.**
   - Hard iteration bounds on every loop: `max_dfs_iterations`, `max_traversal_depth`, `max_context_nodes`, `max_ingestion_entries`. All are parameters (not constants) — overridable via the substrate's `attr_config`. Verified by `test_compile_context_node_limit`, `filesource_enforces_max_ingestion_entries`, `cycle_prevention_across_multi_hop`.
   - Assertion density ≥ 2 per public function.
   - `#![deny(warnings)]` + `#![deny(clippy::pedantic)]` across the workspace.
4. **Real SCD-2.** `supersede_state` runs an atomic `BEGIN…COMMIT` transaction that closes any prior `is_current = true` row (`is_current = false`, `valid_to = time::now()`) before inserting the new current row. Verified by `test_scd2_supersede_nasa_hardened`, `scd2_chain_closes_old_opens_new`.
5. **Anti-Coercion.** `supersede_state` and `create_structural_edge` refuse any write whose target entity belongs to a tenant other than the calling session. `CapabilityGovernor::handshake` likewise rejects cross-tenant identity claims. Verified by `test_validation_nasa_hardened` (tenant-coercion case), `governor_handshake_rejects_cross_tenant_identity`.

### 3. Cognitive Governance

- **Memory Tiers.** `compile_context` distills state by priority — `core` / `working` / `archival` / `recall` — declared on each `type_definition`. Tier filtering is verified by `compile_context_filters_by_memory_tier`.
- **Capability Governor (`superx-agent`).** Agent permissions are graph-derived: a tool dispatch only proceeds if `agent -[edge_has_capability]-> tool` exists for the current tenant. Bootstrap seeds five canonical tools (`tool_ingest`, `tool_compile`, `tool_propose`, `tool_evaluate`, `tool_promote`) and grants both default admin agents capabilities for all of them. Verified by `governor_check_capability_allows_seeded_admin` (allow), `governor_check_capability_denies_when_no_edge_exists` (deny), `governor_handshake_creates_session_and_participates_edge`.
- **Meta-Harness (`superx-harness`).** A fuel-metered (10 000-unit) `wasmtime` sandbox runs proposal-scoring wasm and records the score to the ledger as `attr_score`. Fuel exhaustion traps as `KernelError::SafetyViolation`; in-budget execution promotes proposals that clear a configurable threshold by linking them to the substrate via `edge_promotes`. Verified by `harness_evaluate_traps_on_fuel_exhaustion`, `harness_evaluate_records_score_when_wasm_returns_within_budget`, `harness_promote_below_threshold_returns_false`, `harness_promote_rejects_malformed_proposal_id`.
- **JSON-Schema Validation.** Every typed `state_ledger` write is validated against the `type_definition.sch_json` schema (`attr_desc` requires `{text: string}`, `attr_score` requires `{score: number}`). Verified by `attr_score_rejects_non_numeric_and_missing_score`.

### 4. Telemetry & Observability

- **Built-in instant-zero capture.** Every kernel mutation (`supersede_state`, `create_structural_edge`, `checkpoint_execution`, `compile_context`, `pulse`) emits a typed `telemetry_stream` row tagged with tenant + optional run_id. Verified by `telemetry_bootstrap_emits_system_event`, `telemetry_bootstrap_emits_state_supersede_events`, `telemetry_bootstrap_emits_edge_create_events`, `telemetry_is_tenant_isolated`.
- **Fine-grained agent telemetry from instant-zero.** Bootstrap's discovery routine emits structured per-agent events the moment provisioning completes:
  - `agent_seeded` — one event per canonical SuperX admin (`system_controller`, `gemini_cli`). Payload: `{agent_id, name, role, source}`.
  - `agent_discovered` — one event per `mcpServers` entry found in a Claude Desktop config. Payload: `{agent_id, name, role, source, config_path, command, arg_count}`. Probes macOS / Windows / Linux (XDG fallback) config paths. Tests can override via `SUPERX_CLAUDE_CONFIG`.
  - `agent_activity_observed` — one event per `mcp-server-<name>.log` found in the Claude Desktop logs directory (live-agent signal). Payload: `{name, log_path, mtime_since_epoch}`. Tests can override via `SUPERX_CLAUDE_LOGS`.
  - `bootstrap_census` — closing summary `{agents_seeded, agents_discovered, agents_with_activity}` so consumers can prove the scan ran even when nothing was found.

  Verified by `bootstrap_emits_agent_seeded_events_for_canonical_admins`, `bootstrap_emits_agent_discovered_for_each_claude_desktop_server`, `bootstrap_emits_agent_activity_observed_for_each_mcp_log_file`.
- **Emission Router (`superx-emission`).** A LIVE-SELECT subscriber over `telemetry_stream` forwards events to a `KafkaSink` (`rdkafka`) and/or an `ApiSink` (`reqwest` HTTP POST with optional bearer auth). Spawned automatically by both `superx-cli` (for the duration of a command) and `superx-mcp` (background task for the server's lifetime). Sinks are configured via `--kafka-brokers` / `--kafka-topic` / `--emission-api` flags or `SUPERX_KAFKA_BROKERS` / `SUPERX_KAFKA_TOPIC` / `SUPERX_EMISSION_API` env vars. Verified by `apisink_posts_telemetry_row_to_remote_endpoint`, `apisink_surfaces_non_2xx_as_error`.
- **Heartbeat Pulse.** The MCP server emits one `system_pulse` telemetry row per minute so consumers can prove freshness.

### 5. MCP Surface (`superx-mcp`)

The binary speaks MCP over stdio. Library surface (`superx_mcp::dispatch_tool`) is exposed so tests and embedders can call the dispatcher directly without a full rmcp transport. Tools currently advertised:

| Tool       | Required args                  | Capability gate                 |
| :--------- | :----------------------------- | :------------------------------ |
| `identify` | `agent_uid` (+ optional tenant) | (none — establishes identity)   |
| `graphify` | `agent_id`, `path`              | `agent -> tool_ingest`          |
| `compile`  | `agent_id`, `root_id`, `tiers?` | `agent -> tool_compile`         |

Dispatch denial is verified by `mcp_dispatch_denies_uncapped_agent_on_graphify`; positive path by `mcp_dispatch_allows_seeded_admin_for_graphify`; defensive paths by `mcp_dispatch_rejects_missing_required_args` and `mcp_dispatch_unknown_tool_returns_method_not_found`.

### 6. Workspace Layout

```
crates/
  superx-kernel/      5-table substrate, sessions, SCD-2, schema, cycle detection
  superx-bootstrap/   First-run substrate provisioning, agent + tool seeding
  superx-ingest/      Universal ingestor: FileSource (walk + per-file attr_desc), JsonSource
  superx-compiler/    Tier-aware context distillation, optional LLM-distilled output
  superx-inference/   Local Candle + GGUF runner (zero-cloud inference)
  superx-proposer/    LLM-driven structural-edge proposer
  superx-harness/     wasmtime Meta-Harness (fuel-metered) + promote
  superx-agent/       Capability Governor (handshake, check_capability)
  superx-emission/    Telemetry subscriber + Kafka/HTTP sinks
  superx-cli/         Operator CLI:
                        bootstrap | graphify | compile | propose | evaluate |
                        promote | identify | list-agents | list-tools |
                        demo | stats
  superx-mcp/         MCP stdio server (lib + bin)
```

### 7. Hardening Invariants (binding for every future change)

These principles are *non-negotiable* per the project's Zero-Trust Execution Mode (`.claude/skills/zero-trust-execution/SKILL.md`) and the operator's standing direction:

1. **No hardcoding.** Safety bounds, prompts, model paths, capability sets, scoring thresholds — every constant must be a parameter stored on `attr_config` and read via `Kernel::get_parameter`. Hardcoded values block the Meta-Harness's promotion path.
2. **No database-rule bypass.** Application code must not disable SurrealDB `PERMISSIONS`, drop `ASSERT`s, or use root credentials to "make a query work." A misbehaving query is a bug in the query, not in the schema. The eventual production posture is a per-service-account credential set with database-enforced tenant isolation.
3. **No workarounds.** String coercion of record IDs, hash-of-uuid shortcuts, "fake-async" functions, retry-until-it-works loops — all banned. The Mandate-2 (research before guessing) and Mandate-3 (no unsanctioned architectural change) rules of Zero-Trust Execution apply.
4. **Verification is the only truth.** Code is not landed until `cargo test --workspace && cargo clippy --workspace --all-targets --all-features -- -D warnings` are both green. `ARCHITECTURE.md` is not updated until the gates pass.
5. **Every new table is SCD-2, append-only, time-travel-queryable, with full audit trail.** A row is closed (`is_current=false`, `valid_to=time::now()`) and a new row inserted on every state change; no `UPDATE`-in-place, no `DELETE`. This is binding for `schedule`, `execution_params`, and every future substrate table. Operators must be able to reconstruct the entire history of any work item by `SELECT … ORDER BY valid_from ASC`.
6. **Distributed-collaboration ready.** SuperX consumes `leiden-rs` and other CRDT-aware libraries; substrate writes that may originate concurrently from multiple peers must remain causally consistent. The `state_ledger` SCD-2 model + UUIDv7 temporal ordering already provides the substrate-side primitives; CRDT-merge logic belongs in the blade that produces the write (e.g. `superx-designer` merging concurrent DAG proposals), not in the kernel itself.
7. **Chat / UI is a downstream consumer, never the source of truth.** A future chat UI is a roadmap item; when it lands it MUST read and write through the same kernel verbs as every other blade (no direct `db.query` from a UI layer, no UI-specific schema). The substrate doesn't know or care that a UI exists.

### 8. Roadmap — primitives the vision requires that are not yet built

Each is scoped against the §0 vision. Marked **[NEW]** if the crate/file does not yet exist; **[EXTEND]** if it builds on an existing crate.

| # | Primitive | Crate | Type | Description |
|---|---|---|---|---|
| 1 | `schedule` table | `superx-kernel` | [EXTEND] | A new SCHEMAFULL table next to `execution_cursor`, holding pending work items: `{run_id, tenant_id, kind, target_entity, due_at, status, attempt, depends_on, metadata, is_current, valid_from, valid_to}`. Indexed by `(due_at, is_current)` and `(run_id, is_current)`. **SCD-2 + append-only** per §7 invariant 5: every status transition (`waiting → scheduled → running → completed/failed`) creates a new row; prior row gets `valid_to`. |
| 1b | `execution_params` table | `superx-kernel` | [EXTEND] | A new SCHEMAFULL table holding per-run agent knobs: `{run_id, tenant_id, agent_id, params_json, is_current, valid_from, valid_to}`. `params_json` carries the full set: `{temperature, top_p, top_k, max_tokens, turns, branch, retry_policy, model_ref, …}`. **SCD-2 + append-only**: changing any knob closes the prior row and writes a new one. The Runner reads the current row at dispatch time. |
| 2 | `RunnerBlade` | `superx-runner` | [NEW] | Background loop spawned by `superx-cli` and `superx-mcp`. Polls the `schedule` table for due-and-ready items (deps satisfied), reads `execution_params` for the run, sets session auth, dispatches via `CapabilityGovernor` + the appropriate tool blade, writes results back as superseded state, and transitions the schedule row's status. All transitions log `schedule_*` telemetry events. |
| 3 | `DesignerBlade` | `superx-designer` | [NEW] | Consumes a product-intent payload (text + constraints) and the substrate's catalogue of compiled `node_data_source` / `node_component` / `node_hardened_model` entities. Produces the full cascading DAG (`node_product → component* → task*`) with `edge_owns` and `edge_implements` edges, then enqueues the leaf tasks into the `schedule` table. Uses local Candle inference (or a configured remote model) for the planning step. |
| 4 | `node_data_source` + connector traits | `superx-kernel` + `superx-ingest` | [EXTEND] | New `node_data_source` (and subtypes `node_data_source_sql`, `node_data_source_iceberg`, `node_data_source_rag`, `node_data_source_remote_model`) in the metamodel. `superx-ingest` gains connector blades that probe and compile each source into the substrate (schema fingerprint, table list, vector chunks, model card). |
| 5 | Vector embedding pipeline | `superx-ingest` + `superx-inference` + `superx-kernel` schema | [EXTEND] | Define `attr_embedding` type with a vector field; add SurrealDB `DEFINE INDEX … MTREE DIMENSION …` for cosine search. `UniversalIngestor` chunks text via configurable strategy, calls `superx-inference` for embeddings, stores on the chunk entity. Activates `edge_semantic` via similarity queries inside `CompilerBlade`. |
| 6 | Prompts as substrate entities | `superx-compiler` + `superx-proposer` | [EXTEND] | Move every hardcoded prompt string to a `node_artifact` with `attr_desc` payload. Blades read their prompt template via `Kernel::compile_context` at the start of every run. The Meta-Harness can supersede prompt entities the same way it promotes code proposals. |
| 7 | OTLP emission sink | `superx-emission` | [EXTEND] | Add `OtelSink` next to `ApiSink`/`KafkaSink`, translating `lifecycle_event` rows into OpenTelemetry spans using the `opentelemetry-otlp` crate. Configured via `SUPERX_OTEL_ENDPOINT` env var. Enables Datadog / Honeycomb / Grafana Tempo ingestion with zero schema work on the consumer side. |
| 8 | Real service-account auth | `superx-kernel` | [EXTEND] | Replace the current `db.set("session_tenant", ...)` session-variable assertion with a real `db.signin(Record { … })` against a `tenant_access` `DEFINE ACCESS` rule. Each agent gets its own credential, tied to a `node_agent` row with a verifiable secret. PERMISSIONS clauses become the only tenant-isolation guarantee at the DB layer. |
| 9 | Remote-model abstraction | `superx-inference` | [EXTEND] | Add a `ChatModel` trait covering local Candle inference + remote Anthropic + remote OpenAI behind a single interface. The selected provider is a parameter on the agent or task entity, not a CLI flag. Retries + circuit-breakers are policy parameters in `attr_config`. |
| 10 | LLM-as-judge (alongside wasm scoring) | `superx-harness` | [EXTEND] | Add an `LlmJudge` mode that complements wasm-based deterministic scoring. Useful for subjective criteria (code clarity, doc completeness). The harness can then return a *composite* score: wasm gates pass/fail, LLM provides nuance within the pass band. |
| 11 | Deployment-mode toggle | `superx-cli` + `superx-mcp` | [EXTEND] | Read `attr_config.deployment_mode` at startup and conditionally spawn the right background tasks: collector-mode skips Runner + Designer; designer-mode skips Runner; full-mode spawns everything. `--mode <collector\|designer\|full>` on `bootstrap` writes the value. |
| 12 | Chat UI (downstream consumer) | `superx-chat` | [NEW, FUTURE] | A web/desktop chat surface that reads via MCP + writes new product intents via the Designer. Per §7 invariant 7, the UI is a read-write client of the kernel verbs — not a source of truth, not a schema author. Includes CRDT-backed collaborative editing of the proposed DAG (via `leiden-rs`) before the operator hits *build*. |
| 13 | Typed kernel write verbs (no raw INSERT in blades) | `superx-kernel` | [EXTEND] | Today `superx-ingest`, `superx-bootstrap`, `superx-agent`, `superx-proposer` issue raw `INSERT INTO entity { … }` queries (10 sites — audit §D). All bind `tenant_id = $session_tenant` correctly, but the kernel can't enforce SCD-2/audit invariants centrally. Add `Kernel::create_entity(type_uid, attrs)`, `Kernel::upsert_entity(…)` so every write flows through one chokepoint. |
| 14 | `execution_cursor` SCD-2 conversion | `superx-kernel` | [EXTEND] | Audit §E: today `checkpoint_execution` is in-place `UPSERT`, pre-dating the §7-#5 *every new table is SCD-2* invariant. Convert to close-and-create: prior cursor row gets `valid_to`, new row gets `is_current=true`. Operators get full resume-point history. |
| 15 | Full MCP 2025-11-25 surface (`resources`, `prompts`, `sampling`, `elicitation`, `roots`) | `superx-mcp` | [EXTEND] | Today we expose only `tools`. Map substrate entities to MCP `resources` 1:1 (free with the metamodel). Expose prompt entities (#6) as MCP `prompts`. Add `sampling` so agents request LLM completions through us with `execution_params` knobs applied. Add `elicitation` for HITL confirmations. 4× the protocol surface, immediately ahead of Letta on coverage. |
| 16 | Rig.rs adoption for the `ChatModel` abstraction | `superx-inference` | [EXTEND, REPLACES #9 SCOPE] | Use Rig's `CompletionModel` trait + provider implementations (Anthropic / OpenAI / DeepSeek / Gemini / Ollama / Perplexity / HuggingFace / xAI). Saves 3-4 weeks vs. greenfield. Bonus: `rig-surrealdb` companion crate slots into the vector layer (#5). |
| 17 | OpenTelemetry GenAI Semantic Conventions | `superx-emission` | [EXTEND, REFINES #7] | The `OtelSink` translates `lifecycle_event` rows into OTel spans using the GenAI semantic conventions ratified in 2025 (`gen_ai.operation.name`, `gen_ai.usage.input_tokens`, etc.). Datadog/Honeycomb/Grafana dashboards light up out-of-the-box. |
| 18 | Automerge-rs V3 integration for collaborative DAG editing | `superx-designer` + `superx-chat` | [NEW, FUTURE] | Wrap Designer edits (and chat-UI authoring) as Automerge documents. CRDT belongs at the intent layer (the DAG being authored), not the substrate. Persist Automerge changes alongside `state_ledger` rows so collaborative editing of the proposed product DAG just works between multiple operators. |
| 19 | Actor-model RunnerBlade (supervisor / debate / handoff patterns) | `superx-runner` | [EXTEND, REFINES #2] | Ground the Runner in AutoGen-style actor primitives. `schedule` items are messages; `topics` are `node_topic` entities; subscriptions are `edge_subscribes_to` edges. Supervisor/debate/handoff become declarative DAG shapes Designer produces. AutoGen's communication primitives expressed in our substrate idiom. |
| 20 | Wasm Component Model for tool blades | `superx-harness` | [NEW, FAR FUTURE] | When WASI 0.3 stabilises, each tool blade can be a wasm component with capability-typed imports. The Governor enforces imports at instantiation. Agents can't escape the substrate even with arbitrary code execution. The long-term safety story. |
| 21 | `ContinuousClassifierBlade` (Gemma-powered) | `superx-runtime` | [NEW, MVP] | Subscribes to `telemetry_stream` for `state_supersede` events; classifies new entities via the configured local model; writes `node_classification` rows with `attr_classification` payload (`{role, stack, is_test, is_dead, confidence}`). Background task spawned by CLI + MCP. Default = always on; toggle via `attr_config.classifier_enabled`. |
| 22 | `InsightHarvesterBlade` | `superx-runtime` | [NEW, MVP] | Windowed analysis of `telemetry_stream` (configurable window = 5 min default). Detects anomalies (failure clusters, latency spikes, capability denial surges) and emits `node_insight` rows with structured payload. Background task. Operator-facing semantic firehose. |
| 23 | `SummarizerBlade` + `attr_summary` SCD-2 type | `superx-runtime` + `superx-kernel` | [NEW, MVP] | New `attr_summary` type in the metamodel. Subscribes to `state_supersede` on `attr_desc` rows whose payload exceeds a token threshold (`attr_config.summarize_threshold`); writes an `attr_summary` SCD-2 row alongside on the same entity. `CompilerBlade` consumes summaries instead of raw text when present — massive context savings. |
| 24 | `ModelRouterBlade` + `attr_capability_score` type | `superx-runtime` + `superx-kernel` | [NEW, MVP] | New `attr_capability_score` type in the metamodel. Every dispatch with a model decision flows through the router; the router reads the candidate models' capability scores, picks the cheapest one clearing the per-task threshold, escalates per `attr_config.escalation_rules`. Selection is recorded in `execution_params` for the run so it's auditable. |
| 25 | `EdgeProposerBlade` (background) | `superx-runtime` | [NEW] | Background variant of the existing on-demand `ProposerBlade`. Watches new `node_code` / `node_artifact` entities; runs the local model to propose `edge_implements` / `edge_owns` / `edge_semantic` candidates as `node_proposed_edge` rows. Meta-Harness scores them; promoted candidates become real edges. HITL-gated by default. |
| 26 | `ParamTunerBlade` | `superx-runtime` | [NEW] | Watches `schedule_failed` events. When a (`task_kind`, `model_ref`) pair fails beyond a threshold, the blade reads the run history, proposes new `execution_params` via the local model, writes a `node_param_proposal`. Meta-Harness scores; promoted proposals become the new default. The substrate gets better at running itself. |
| 27 | Models directory convention + `superx-cli install-model` | `superx-cli` + filesystem | [NEW, MVP] | Workspace-relative `./models/` (gitignored) holds local GGUFs. Discovery order: `--model-path` flag → `SUPERX_MODEL_PATH` env → `node_hardened_model.attr_config.local_path` → `./models/<uid>.gguf`. `superx-cli install-model gemma-4-27b-it` downloads + registers + benchmarks the model entity. |
| 28 | Custom prompt entities + operator authoring CLI | `superx-cli` + `superx-kernel` | [EXTEND of #6] | `superx-cli prompts add <name> --body <text>` writes a `node_artifact` of type `prompt_template`. Blades look up their prompt by name; operators can supersede prompts at any time (SCD-2 audit chain). The Meta-Harness can A/B-test prompt versions the same way it scores code proposals. |
| 29 | Auto-onboard all MCP clients on the system | `superx-bootstrap` | [EXTEND, MVP] | Bootstrap currently probes Claude Desktop config + Claude Desktop logs. Extend the probe set to: Claude Code config, Cursor MCP config, Continue/Cody/other-MCP-client configs. Each discovered agent gets a `node_agent` row + `agent_discovered` telemetry. The bar: drop SuperX on a machine, run `bootstrap`, every agent gets onboarded automatically. |

### Implementation roadmap — MVP first, then expand

The user-mandated build order: **first viable product**, then layer on. The MVP is the *collector + CLI + intelligent-RAG* slice defined in §0d. Everything below is grouped into the five MVP phases, then post-MVP phases. Each phase has a single Mandate-5 gate that must pass before the next phase starts.

#### MVP — Phase A: tech-debt baseline (must precede new work)

| Item | Where |
|---|---|
| Backfill missing doc comments on 8 pub items | issues #1 |
| Diligent per-file tech-debt read-through (all 11 crates) | issue #2 |
| Detailed code-comment pass (operator: "post detailed comments") | issue #3 |

#### MVP — Phase B: substrate foundations needed by the intelligence layer

| Roadmap # | Item | New crate? | Library leverage |
|---|---|---|---|
| #1b | `execution_params` SCD-2 table | no — kernel | — |
| #14 | `execution_cursor` SCD-2 conversion | no — kernel | — |
| #6 (subset) | Prompt template entities + `Kernel::load_prompt(name)` verb | no — kernel | — |
| #5 (subset) | `attr_embedding` type + SurrealDB MTREE index | no — kernel | SurrealDB native vectors |
| New | `attr_summary` + `attr_capability_score` types | no — kernel | — |

#### MVP — Phase C: model plug-and-play

| Roadmap # | Item | New crate? | Library leverage |
|---|---|---|---|
| #16 | Rig.rs adoption as the `CompletionModel` provider abstraction | extend `superx-inference` | **`rig-core` + `rig-anthropic` + `rig-openai` + (Candle via existing `superx-inference`)** |
| #27 | `./models/` directory convention + `superx-cli install-model <uid>` | extend `superx-cli` | **`hf-hub`** (already a workspace dep) for model download |
| #24 (scaffold) | `ModelRouterBlade` skeleton + capability-score query path | new crate `superx-runtime` (single home for all continuous-intelligence blades — keeps the Cargo graph flat; modules split out only if a blade outgrows the file) | — |

#### MVP — Phase D: continuous local intelligence blades

| Roadmap # | Item | Where it lives |
|---|---|---|
| #21 | `ContinuousClassifierBlade` | `superx-runtime::classifier` |
| #22 | `InsightHarvesterBlade` | `superx-runtime::insighter` |
| #23 | `SummarizerBlade` | `superx-runtime::summarizer` |
| #24 (full) | `ModelRouterBlade` fully wired into every blade dispatch | `superx-runtime::router` |

#### MVP — Phase E: onboarding completeness + release polish

| Roadmap # | Item |
|---|---|
| #29 | Auto-onboard all detectable MCP clients (extend probe set: Claude Code, Cursor, Continue, Cody, generic `~/.config/mcp/` discovery) |
| #28 | `superx-cli prompts add/list/show/supersede` for operator-authored prompt entities |
| #11 | Deployment-mode toggle (`collector | designer | full`) via `attr_config.deployment_mode` |
| New | MVP integration test suite under `crates/superx-cli/tests/mvp_release.rs` — verifies the full agent-onboarding → telemetry → classification → summarization → insight loop |
| New | v1.0 release tag + GitHub release with binaries (`cargo dist`) |

**Phase A → E green = MVP shipped.** SuperX is then a complete telemetry-capture + CLI + intelligent-RAG agentic OS, with a model always present, fully modular, parametrizable, customizable.

#### Post-MVP — Phase F: autonomous orchestration

The full agentic-OS pillar. Adds Designer + Runner; turns SuperX from collector+RAG into a system that *builds products on its own*.

| Roadmap # | Item |
|---|---|
| #1 | `schedule` table (SCD-2) |
| #2 | `RunnerBlade` |
| #3 | `DesignerBlade` |
| #25 | `EdgeProposerBlade` (background continuous variant) |
| #26 | `ParamTunerBlade` (self-tuning) |
| #4 | `node_data_source` connectors (SQL / Iceberg / RAG / remote-model) |
| #15 | Full MCP 2025-11-25 surface (`resources` / `prompts` / `sampling` / `elicitation`) |

#### Post-MVP — Phase G: enterprise observability + collaboration

| Roadmap # | Item |
|---|---|
| #7 / #17 | OTLP GenAI emission sink |
| #18 | Automerge-rs CRDT integration for collaborative DAG editing |
| #19 | Actor-model RunnerBlade (supervisor / debate / handoff) |
| #12 | Chat UI (CRDT-backed, MCP-consumer-only) |

#### Post-MVP — Phase H: hardening

| Roadmap # | Item |
|---|---|
| #8 | Real service-account auth (`db.signin(Record)` against `tenant_access`) |
| #9 | LLM-as-judge alongside wasm scoring |
| #13 | Typed kernel write verbs (no raw `INSERT` in blades) |
| #20 | Wasm Component Model for tool blades (long-term safety) |

#### Library philosophy

- **Reuse first, build second.** Rig.rs for chat-model abstraction, `hf-hub` for model download, `rdkafka` / `reqwest` already in place, `automerge-rs` for CRDT, `wiremock` for HTTP test mocks, SurrealDB native for vectors. We add a new crate only when no library covers the contract.
- **Internal crates exist to keep the Cargo graph flat.** A blade gets a module first, gets a crate only when it grows past ~500 LoC or needs its own dev-deps. `superx-runtime` houses the continuous-intelligence blades in one place during MVP; split if needed later.
- **No vendored dependencies.** All deps are versioned in `Cargo.toml`. If a vendor disappears, the substrate principle of "compiled data sources" means SuperX can swap implementations without operator effort.

---

## 10. First Viable Product (v1.0) — explicit spec

This section is the contract for what "shipped" means for the MVP. **Every feature listed below must be implemented, fully tested, and have a green Mandate-5 gate before v1.0 is tagged.** Anything not in this section is deferred to a later release.

### 10.1 In-scope features (v1.0)

| # | Feature | Acceptance criteria |
|---|---|---|
| **F1** | **Auto-discover all detectable MCP agents on the host** during `bootstrap` | Probes succeed without operator action for: Claude Desktop (macOS/Windows/Linux), Claude Code, Cursor, Continue, Cody. Each discovered agent gets `node_agent` + `agent_discovered` telemetry. Test injection via `SUPERX_*_CONFIG` env vars. |
| **F2** | **Fine-grained telemetry capture from instant-zero** | Every kernel mutation (`supersede_state`, `create_structural_edge`, `checkpoint_execution`, `compile_context`, `pulse`) emits a typed `telemetry_stream` row. Bootstrap alone emits ≥ 16 events. All events tenant-isolated. Reconstructable history by `valid_from` order. |
| **F3** | **Telemetry egress** to Kafka and HTTP API sinks | Both sinks accept structured `TelemetryRow` payloads. HTTP supports optional bearer-auth. Backpressure-tolerant. (OTLP deferred to post-MVP.) |
| **F4** | **CLI parity across all kernel verbs** | `bootstrap`, `graphify`, `compile`, `propose`, `evaluate`, `promote`, `identify`, `list-agents`, `list-tools`, `demo`, `stats`, plus new for MVP: `install-model`, `prompts {add|list|show|supersede}`, `models {list|set-default}`. Every verb sets session auth, validates inputs, emits its own typed telemetry. |
| **F5** | **A model is always loaded** | Default = Gemma 4 (smallest reasonable Q4_K_M GGUF). Discovered via `--model-path` → `SUPERX_MODEL_PATH` env → substrate `node_hardened_model.attr_config.local_path` → `./models/<uid>.gguf`. Fallback to remote model if configured. Boot warns (does not fail) if no model is available; intelligence blades degrade gracefully. |
| **F6** | **Multi-provider model abstraction via Rig.rs** | `ChatModel` trait covers local Candle + remote Anthropic + remote OpenAI. Per-task model selection lives in `execution_params`, not CLI flags. |
| **F7** | **Continuous classification of ingested entities** | `ContinuousClassifierBlade` subscribes to `telemetry_stream`; classifies new `node_code` / `node_artifact` entities asynchronously; writes `node_classification` rows. Operator-tunable via `attr_config.classifier_enabled` and `classifier_vocabulary`. |
| **F8** | **Continuous summarization of long content** | `SummarizerBlade` writes `attr_summary` SCD-2 rows on entities whose `attr_desc` exceeds the configured token threshold. `CompilerBlade` consumes summaries when present. |
| **F9** | **Insight harvesting from telemetry** | `InsightHarvesterBlade` runs windowed analysis; emits `node_insight` rows when anomaly heuristics (configurable) fire. Operator views via `superx-cli insights --tenant <t>`. |
| **F10** | **Capability-scored model routing** | `ModelRouterBlade` consults `attr_capability_score` per registered model; picks the cheapest one above the per-task threshold; escalates per `attr_config.escalation_rules`. Choice is recorded in `execution_params`. |
| **F11** | **Operator-authored custom prompts** | Prompts are `node_artifact` entities. `superx-cli prompts add <name> --body <text>` writes one. Blades read prompts by name via `Kernel::load_prompt`. Supersession follows SCD-2 (full audit chain). |
| **F12** | **`execution_params` SCD-2 table** | Holds per-run knobs (temperature, top_p, top_k, max_tokens, turns, branch, retry_policy, model_ref, …). Append-only, time-travel-queryable. Knob updates close prior row + open new. |
| **F13** | **`execution_cursor` SCD-2 conversion** | Resume-points stop using in-place `UPSERT`; switch to close-and-create per §7-#5. Full cursor history is recoverable. |
| **F14** | **HITL primitives (foundation, not full)** | New `attr_approval` type. Any blade-proposed substrate mutation (`node_proposed_edge`, `node_param_proposal`, `node_critique`) can be gated behind an `awaiting_human` status with `attr_config.hitl.required = true`. Approval requires an `edge_approves` from an operator agent. Default thresholds favour autonomous flow; operators tune up if desired. |
| **F15** | **Models directory convention** | `./models/` (gitignored) is the workspace-default location. `superx-cli install-model gemma-4-27b-it-q4_k_m` downloads via `hf-hub`, registers the `node_hardened_model` entity, runs the baseline benchmark via Meta-Harness, writes the `attr_capability_score`. |
| **F16** | **Three deployment modes selectable via attr_config** | `collector` (telemetry + classifier only), `designer` (+ Compiler + Designer-stub), `full` (all blades). Default = `full`. Background-task spawn is conditional on the mode. |
| **F17** | **Doc-complete pub surface** | Every pub item on every crate carries a `///` rustdoc comment including `# Errors` / `# Panics` where applicable. The full code-comment pass from issue #3 is landed. |

### 10.2 Out-of-scope for v1.0 (deferred to v1.x or later)

- `DesignerBlade` (Phase F)
- `RunnerBlade` and `schedule` table (Phase F)
- `EdgeProposerBlade`, `ParamTunerBlade` continuous variants (Phase F)
- `node_data_source_*` connectors for MySQL / Postgres / Iceberg (Phase F)
- Full MCP 2025-11-25 surface — only `tools` is exposed at v1.0 (Phase F)
- OTLP egress (Phase G)
- Automerge CRDT integration + Chat UI (Phase G)
- Real service-account auth — `db.set(session_tenant, ...)` is still the model at v1.0 (Phase H)
- Wasm Component Model for tool blades (Phase H)

### 10.3 v1.0 test matrix

Every feature above maps to test files. All tests run under Mandate-5 gates. Numbers below are minimum targets — more is fine.

| Layer | Coverage |
|---|---|
| **Unit (per-crate, in `crates/<c>/src/lib.rs#tests`)** | ≥ 1 test per public function. Verifies the API contract, not internals. |
| **Integration (cross-crate, in `crates/superx-cli/tests/`)** | New file `mvp_release.rs`: drives `bootstrap → discover agents → classify ingested fixture → summarize → emit insight → telemetry capture → stats CLI returns all events`. End-to-end, one tenant. |
| **Capability** (existing `core_capabilities.rs`) | Already at 36 tests; expand to ≥ 50 covering each new MVP feature (F7-F11 each get ≥ 2 tests). |
| **MCP dispatch** | Existing 4 tests in `core_capabilities.rs` cover the policy layer. Extend with one test per new tool surfaced. |
| **Telemetry egress** | Already covered by `apisink_*` (wiremock). Add Kafka emission test using a `wiremock`-equivalent (or `rdkafka` mock producer). |
| **Smoke (`cargo run` driven)** | New shell script `scripts/mvp_smoke.sh` that runs the full operator flow: `bootstrap → install-model → list-agents → list-tools → graphify → stats`. Asserts exit codes + expected telemetry events. |
| **Mandate-5 gates** | `cargo test --workspace` 100% pass; `cargo clippy --workspace --all-targets --all-features -- -D warnings` clean. CI-enforced before any merge to `main`. |

### 10.4 Release checklist for v1.0

1. ☐ All 17 features F1-F17 implemented + tested
2. ☐ All MVP roadmap issues closed
3. ☐ `cargo test --workspace`: 100% pass
4. ☐ `cargo clippy --workspace --all-targets --all-features -- -D warnings`: clean
5. ☐ MVP smoke script `scripts/mvp_smoke.sh` passes locally on macOS + Linux
6. ☐ `ARCHITECTURE.md` updated with "v1.0 shipped" status block
7. ☐ `README.md` updated with v1.0 quickstart
8. ☐ Release notes drafted under `RELEASES.md`
9. ☐ `cargo dist` releases binaries for macOS (Apple Silicon + Intel) + Linux x86_64
10. ☐ GitHub release tagged `v1.0.0` with binaries attached
11. ☐ Architecture issue updated with v1.0 surface
12. ☐ Demo video (or asciinema cast) of `bootstrap → install-model → graphify → stats` for the README

### 9. Operator Quickstart

```bash
# Provision substrate, seed default agents and capability edges
cargo run -p superx-cli -- bootstrap --tenant demo

# One-shot end-to-end smoke run — bootstrap → ingest → propose → promote
cargo run -p superx-cli -- demo --tenant demo

# Ingest a directory as a DAG of node_code entities
cargo run -p superx-cli -- graphify --path ./my_project --tenant demo

# Identify an agent and start a session (handshake)
cargo run -p superx-cli -- identify --agent-uid <local-uuid> --tenant demo

# Inspect what's registered
cargo run -p superx-cli -- list-agents --tenant demo
cargo run -p superx-cli -- list-tools  --tenant demo

# Distill context (tier-filtered)
cargo run -p superx-cli -- compile --root entity:<uuid> --tenant demo --tiers core --tiers working

# Stream recent telemetry (newest first)
cargo run -p superx-cli -- stats --tenant demo --limit 25

# Run as an MCP server (foreground, talks to Claude Desktop / Claude Code / etc.)
cargo run -p superx-mcp

# Same, with external egress
SUPERX_EMISSION_API=https://your-ingest/v1/events \
SUPERX_KAFKA_BROKERS=localhost:9092 SUPERX_KAFKA_TOPIC=superx.telemetry \
cargo run -p superx-mcp
```

State persists in `./db/superx.db` (RocksDB) between runs.

---
*Copyright (c) 2026 Constantin Alexander <constantin@dedomena.io>*
</content>
</invoke>