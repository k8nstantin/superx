# SuperX Architecture: The Hardened Cognitive Substrate + Autonomous Product Builder (v42.15)

> ## ⚠️ HISTORICAL DOCUMENT
>
> This document describes the **pre-2026-05-23 system**, which was wiped in
> the redesign reset (PR #93, tagged `archive/pre-redesign-2026-05-23`).
> The 12-crate workspace, the test counts, and the "blade" components below
> **no longer exist**. It is retained as the long-term vision reference only.
>
> - **Current plan:** [`docs/ROADMAP.md`](docs/ROADMAP.md)
> - **Current schema truth:** [`SUPERX_SCHEMA.md`](SUPERX_SCHEMA.md) + [`schema/kernel.surql`](schema/kernel.surql)
> - **Current code:** `crates/superx-kernel` (F0 atomic core, PR #96),
>   `crates/superx-kernel-bootstrap` (F6 boot orchestrator),
>   `crates/superx-cli` (F7 `superx` operator binary),
>   `crates/superx-kernel-discovery` (F8 agent-discovery probe framework),
>   `crates/superx-driver-claude-code` (F9 Claude Code discovery driver + F10 transcript watcher),
>   `crates/superx-kernel-capture` (F10 telemetry capture loop)

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

### 0c-2. The execution pipeline — design → compile → schedule → run (Terraform-style)

Borrowed wholesale from Terraform's `plan`/`apply` pattern: **nothing executes that hasn't been compiled.** The substrate has four execution-layer phases, each with a distinct blade, each calling the local model when judgment is required. This binds *how* work flows from intent to outcome — and prevents the wrong-layer mistakes called out in §8 and §9 of the operating skill (`zero-trust-execution`).

1. **Design** — operator intent (text + constraints) → DAG of substrate entities. `DesignerBlade` (#3) calls the local model to plan: which `node_product`, which `node_component`s, which `node_task`s, which `node_data_source*` (`_sql` / `_iceberg` / `_postgres` / `_rag` / `_remote_model`), which `node_hardened_model`s, which `node_agent`s, which prompt entities — all linked by `edge_owns` / `edge_implements` / dep edges. Output: an *uncompiled* DAG with `attr_compile_status = 'uncompiled'` on the root.

2. **Compile** (the Terraform-`plan` analog) — `CompilerBlade` (#30) walks the uncompiled DAG and validates **every connection** before anything queues:
   - **Topology checks** via `petgraph`/`daggy`: `is_cyclic_directed` (no impossible cycles), `toposort` (execution order), single-root + reachability of every node from the root.
   - **Substrate checks**: every referenced entity exists; every `node_data_source_sql` is reachable (`sqlx::query!("SELECT 1")`); every `node_rag_source` corpus is queryable; every `node_hardened_model` has its weights resolvable per §0c-1 path-discovery order; every `node_agent` has the capability edges its assigned task requires.
   - **Template checks**: every prompt entity resolves all `{variable}` references against upstream task outputs (via `minijinja` or `tera`).
   - **Semantic checks via the local model** (through Rig.rs once #16 lands): does the prompt match what the upstream RAG actually covers? Are there missing intermediate steps? Does the assigned agent's capability match the task's intent? Each model call records its rationale text.
   
   Output: every entity in the DAG gets `attr_compile_status` written via `state_ledger` (`'compiled'` or `'failed'`) plus an `attr_compile_report` envelope (resolved refs, estimated fuel cost, telemetry budget, failure list). **Uncompiled DAGs cannot enter the schedule.** Idempotent and re-runnable: SCD-2 history of `attr_compile_status` shows the moment a previously-valid DAG became invalid (e.g., upstream data-source schema drift, RAG corpus rotation, model capability-score change).

3. **Schedule** — `SchedulerBlade` (#31) is the local-model layer **over** a dumb queue. Continuous loop: reads compiled DAGs (`attr_compile_status='compiled'`), current `schedule` rows, telemetry windows (latency / failure / success patterns), agent load, `execution_params` history. Proposes scheduling decisions via `node_proposal` rows scored by the Meta-Harness — which compiled DAG enqueues next with what `execution_params`, which failed items retry vs. abandon vs. escalate to HITL, which DAG successors are newly ready, which param defaults to tune for a given task kind. Every model call emits a `scheduler_*` telemetry event with rationale text. **The `schedule` table itself stays dumb** — every judgment lives in proposals with full audit trail, per §9 of the operating skill.

4. **Run** (the Terraform-`apply` analog) — `RunnerBlade` (#2) is the pure mechanical executor. **No model calls, no scheduling decisions** — those live in `SchedulerBlade`. Pops a due schedule row; refuses unless `target_entity.attr_compile_status='compiled'`; walks the compiled DAG via `petgraph::toposort`, parallel up to `attr_config.max_concurrent` (Terraform's pattern); reads each node's prompts/instructions/capability/agent/execution_params; dispatches via `CapabilityGovernor` + the appropriate tool blade; writes results back as superseded state; emits typed telemetry per step; transitions the schedule row.

**Binding invariants:**
- Schedule rows reference **compiled-DAG entity roots** (or sub-DAG nodes). The runner refuses uncompiled targets.
- Compile is **idempotent**: re-compiling the same DAG with unchanged upstream produces the same report.
- Every model call (Designer, Compiler, Scheduler) emits typed telemetry with rationale text. **No silent intelligence** — the model is woven through every decision and every decision is auditable.
- Recompile on upstream change. When a data source's schema drifts or a model's capability scores change, dependent compiled DAGs get re-compiled and their SCD-2 `attr_compile_status` chain shows the transition.

**Library leverage** (per the §8 library philosophy "reuse first, build second"):
- **`petgraph`** + **`daggy`** for in-memory DAG operations (toposort, cycle check, parallel walk order). The Rust standard; ubiquitous, mature.
- **`Rig.rs`** (roadmap #16) for every local-model call inside Designer / Compiler / Scheduler.
- **`minijinja`** or **`tera`** for prompt-template variable binding during Compile (decide at impl time).
- **`jsonschema`** (already a workspace dep) for entity-attr schema validation during Compile.
- **`sqlx`** for `node_data_source_sql` reachability probes during Compile.
- Reference implementations to *study* but **not adopt wholesale**: [`dagrs`](https://github.com/dagrs-dev/dagrs), [`erio-workflow`](https://crates.io/crates/erio-workflow), [`oxigdal-workflow`](https://lib.rs/crates/oxigdal-workflow), `Flow-Like`, `sayiir`. Generic workflow engines don't fit our substrate-driven + telemetry-everywhere + Governor-gated constraints; we borrow patterns (parallelism, checkpointing, conditional nodes), build bespoke.

**The boundary rules** (from the operating skill, restated here because they're load-bearing for this pipeline):
- Don't queue uncompiled DAGs. The Compile step is non-negotiable.
- Don't put intelligence in the queue (§9 skill). All Scheduler judgment lives in proposals.
- Don't put queue logic in the entities (§8 skill). The DAG is design; the queue is execution.
- Designer designs, Compiler validates, Scheduler decides, Runner executes. Four blades, four jobs.

### 0c-extensibility. The extensibility moat — our durable competitive advantage

SuperX's competitive position rests on **two structural advantages**, not on tactical feature counts:

1. **The model is integrated into the OS, not bolted on.** Competitors (OpenFANG, Letta, etc.) are bigger, older, and shipping. But they were *not designed* to make a model a first-class entity inside their substrate. Their model integration is glue code around a Python SDK call. To retrofit "the model is part of the OS" they have to redesign their kernel. Until they do, they're calling an LLM; we *are* an intelligent OS.
2. **Modular from line zero.** The 5-table metamodel, capability-graph governor, model-as-entity, prompt-as-entity, params-as-SCD-2 patterns mean every future capability lands as a *substrate write*, not a schema migration. Competitors need a release cycle to add a new model provider; we need an `INSERT INTO entity`.

These two compound. The moat is not "we have better design" (subjective). The moat is **"adding the Nth feature to SuperX costs less than adding the Nth feature to anything else."** Today that means we ship MVP behind them. In a year it means we ship `N+1` features per quarter while they ship `N`. In two years we're ahead by feature-velocity squared.

#### What this means for design decisions, *every* one

- **Adding a new model provider** = `node_hardened_model` row + an impl of `CompletionModel`. No new crate unless behavior warrants it.
- **Adding a new tool** = `node_tool` row + an `edge_has_capability` grant. The Governor and Router handle dispatch with no special-casing.
- **Adding a new data source connector** = `node_data_source` subtype in the metamodel + a new module in `superx-ingest`. No core kernel change.
- **Adding a new telemetry sink** = a struct implementing the existing `Sink` shape (Kafka/HTTP today, OTLP/Superset/eCharts tomorrow). Composable in the `TelemetrySubscriber::run_loop`.
- **Adding a new analysis blade** = a new module in `superx-runtime` subscribing to `telemetry_stream`. No kernel change.
- **Adding a new prompt strategy** = a `node_artifact` row of type `prompt_template`. No code change.

If a feature *cannot* land this way, the kernel is missing a primitive and we add the primitive first.

#### Future integration vectors we're designed to absorb without redesign

The substrate-first posture lets us pick up these capabilities *without retrofitting*:

| Future capability | How it lands in SuperX | Why this is hard for competitors |
|---|---|---|
| **Federated analytics** — local SuperX nodes phone home to a central Apache Superset for org-wide telemetry | A new `SupersetSink` in `superx-emission` next to `KafkaSink`/`ApiSink`. The "federation" is configuration on `attr_config.emission_targets`. | They have to design a federation layer; we already have multi-sink emission. |
| **Mothership model** — local SuperX nodes route complex queries to a central org-level model | New `node_hardened_model` entity whose `attr_config` points to a private endpoint; the ModelRouterBlade escalates per existing rules. | They don't have a router pattern; every escalation requires app-layer wiring. |
| **Dynamic chart generation** (Apache eCharts) | A new tool (`tool_chart`) + `node_chart` entity carrying the eCharts JSON config; the model produces the config from data via prompt template. | They'd add a hardcoded chart-builder; we add a row. |
| **CRDT-collaborative DAG editing** (Automerge / leiden-rs) | `node_dag_revision` entity + Automerge patches stored as `attr_patch` SCD-2 rows | No retrofit story; we already have SCD-2 + UUIDv7 + entity-of-everything. |
| **Custom org capabilities** (any vertical: legal/medical/finance compliance gates) | `edge_has_capability` to a custom tool + a custom prompt entity | They need policy code paths; we need rows. |
| **New MCP spec revisions** (resources, sampling, elicitation, roots — already in spec 2025-11-25) | Add handlers to `superx-mcp::dispatch_tool` | They have to upgrade their MCP wrapper; ours is one match arm per surface. |

This is the durable moat. **Modularity from line zero is the only way to compete with incumbents who started years earlier.** SuperX is the only Rust agentic OS where every operational concern (auth, dispatch, model selection, telemetry, prompt template, data source) is a row in the same table family, not a code path.

### 0e. Go-to-market wedge — the execution corollary of the moat

The architectural moat (§0c-extensibility) is *what* makes SuperX defensible. The go-to-market wedge is *how* SuperX gets adopted long enough for the moat to compound. **Architecture without distribution is academia.** This section is binding on product, marketing, and engineering priority discussions — not just on code.

#### Three sequencing rules

1. **Pick one deployment mode and dominate it first — collector mode.**
   Three deployment modes (collector / designer / full) is an architectural strength but a positioning *liability*. "We can be three things" reads as "we are nothing specific." The wedge: **be the absolute best telemetry collector for MCP agents** before claiming anything else. Every MCP user already runs Claude Desktop / Claude Code / Cursor / Continue / Cody. Drop SuperX on their machine; it captures fine-grained telemetry for every agent with zero config; the firehose lights up immediately. They never had this. We charge zero. They tell their team. **That is the wedge.**

   - Don't lead the README with "intelligent agentic OS that designs and builds products."
   - Lead with "drop-in agent activity capture for everything MCP. One binary. No config. Free."
   - Designer + Runner are the *upsell*, not the pitch.

2. **The MCP server is the wedge into every agent-using developer's workflow.**
   `superx-mcp` is already an MCP server. The 2025-11-25 spec adds `resources`, `prompts`, `sampling`, `elicitation`, `roots` — we're behind on surface area but designed to add them as match arms (§0c-extensibility). **Get the MCP surface to feature-complete + 1 (something the field doesn't have) before broadcasting.** That "something" is structured agent telemetry exposed as MCP resources — no other MCP server does this. Once `superx-mcp` is *strictly better than the alternatives*, every MCP user is a potential install.

3. **Kernel is durable IP; everything above is commodity.**
   The 5-table metamodel + SCD-2-everywhere + capability-governor + model-as-entity combination is the defensible piece. The CLI surface is copyable in a sprint. The MCP wrapper is copyable in a week. The substrate is *months* to replicate even if someone copies the visible spec — and the substrate-vs-glue distinction shapes every blade we'll ever ship. **Lean into the kernel as the IP; commodify everything above it.** Open-source the kernel design publicly; let the CLI / MCP / blades remain in our repo but accept that they will be cloned. Defend the substrate; concede the surface.

#### What this means in practice — DOs and DON'Ts

| ✅ DO | ❌ DON'T |
|---|---|
| Optimize the README headline for collector-mode value: *"Drop-in fine-grained telemetry for every MCP agent on your machine, with a local model running RAG for free."* | Lead with "intelligent agentic OS that designs and builds products" — that's v3.0 positioning fronting a v1.0 binary. |
| Make `superx-cli stats` and `superx-cli list-agents` look impressive on day one — these are the wedge demos. | Spend MVP energy on Designer/Runner polish that competes with LangGraph/AutoGen on their turf. |
| Treat every MCP user as a potential SuperX install. Build for them. | Build for "the agentic OS market" — that market doesn't exist yet; ride the MCP wave instead. |
| Open-source the substrate design (this doc); accept the CLI / MCP surface will be copied. | Try to hide architectural choices; the moat is in execution velocity, not secrecy. |
| Ship `cargo dist` binaries for macOS Apple Silicon + Intel + Linux x86_64 day one. Friction-free install is non-negotiable for a collector. | Require operators to `cargo build`. That kills the wedge. |
| Brag about "we have a local model integrated" only in the second paragraph, after the telemetry firehose demo wins them. | Position the model as the differentiator first — Ollama is winning that comparison on convenience. |

#### Window math

The agentic-OS field is < 2 years old. MCP spec just shipped 2025-11-25. Letta is 0.x. AutoGen rewrote itself for 0.4 last year. Rules aren't settled.

- **Today**: SuperX behind on features, ahead on substrate design, no users.
- **12 months**: SuperX shipped MVP collector + intelligence blades, has 100-1000 users running it, has compounding feature velocity advantage.
- **18 months**: Anthropic / Microsoft / Google can copy the substrate idea but they don't (large orgs move slowly on architecture rewrites). Letta / AutoGen could; they're more agile but their existing codebases fight them.
- **24 months**: Either we have the user-velocity moat OR the architectural-moat thesis was wrong and an incumbent caught up. Bet is real.

The wedge has to land in months 0-12. After that we're on equal footing with whichever incumbent reorganizes fastest. **Use the window.**

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
- **The schedule + runner of (6) is shipped** (`superx-runner` + `superx-dispatcher`). `superx-cli runner-demo` drives the full pipeline end-to-end.

#### Runner dispatcher coverage (v0.1)

`KernelDispatcher` (in `superx-dispatcher`) currently routes three schedule `kind` values to real tool blades after a `CapabilityGovernor::check_capability` gate:

| `kind` | Wired in v0.1 | Blade | Inputs read from |
| --- | --- | --- | --- |
| `compile` | ✅ | `CompilerBlade::compile` | schedule row only (target + run) |
| `promote` | ✅ | `MetaHarness::promote` | `attr_config.promote_threshold` (substrate parameter) |
| `ingest` | ✅ | `UniversalIngestor::ingest(FileSource)` | target's `attr_desc.text` (file path) |
| `propose` | ❌ (deferred) | `ProposerBlade::propose_relation` | needs peer entity discovery |
| `evaluate` | ❌ (deferred) | `MetaHarness::evaluate` | needs wasm bytes |

**Why `propose` and `evaluate` are deferred**: both are inner steps of autonomous loops that don't exist yet (`EdgeProposerBlade` #25, `DesignerBlade` #3, autonomous `MetaHarness` continuous-scoring loop). Wiring them now means designing schema (a new `edge_proposes_against` type, or `attr_evaluator_path/wasm` attr types) **before** the caller that motivates the design is built — exactly the §11 "schema-first / code-after" trap in reverse. When `EdgeProposerBlade` / `DesignerBlade` / autonomous `MetaHarness` ship, the corresponding dispatcher kind + its schema are designed in the same PR as the caller. Until then, the existing direct-CLI paths (`superx-cli propose --from --to`, `superx-cli evaluate --proposal --wasm`) remain the only way to trigger those blades.

This is documented coverage, not a TODO — there is nothing to "fix"; the autonomous callers don't exist yet.

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
| 1 | `schedule` table | `superx-kernel` | [EXTEND] | A new SCHEMAFULL table next to `execution_cursor`, holding pending work items: `{run_id, tenant_id, kind, target_entity, due_at, status, attempt, depends_on, metadata, is_current, valid_from, valid_to}`. Indexed by `(due_at, is_current)` and `(run_id, is_current)`. **SCD-2 + append-only** per §7 invariant 5: every status transition (`waiting → scheduled → running → completed/failed`) creates a new row; prior row gets `valid_to`. **Per §0c-2**, `target_entity` references the root (or sub-root) of a **compiled product DAG** (`attr_compile_status='compiled'`); `RunnerBlade` refuses uncompiled targets. The table itself stays dumb — all scheduling judgment lives in `SchedulerBlade` (#31) proposals, never in schema columns (§9 of the operating skill). |
| 1b | `execution_params` table | `superx-kernel` | [EXTEND] | A new SCHEMAFULL table holding per-run agent knobs: `{run_id, tenant_id, agent_id, params_json, is_current, valid_from, valid_to}`. `params_json` carries the full set: `{temperature, top_p, top_k, max_tokens, turns, branch, retry_policy, model_ref, …}`. **SCD-2 + append-only**: changing any knob closes the prior row and writes a new one. The Runner reads the current row at dispatch time. |
| 2 | `RunnerBlade` (Terraform-`apply` analog) | `superx-runner` | [NEW] | Background loop spawned by `superx-cli` and `superx-mcp`. **Pure mechanical executor** — no local-model calls, no scheduling decisions (those live in `SchedulerBlade` #31). Pops a due schedule row, **refuses unless `target_entity.attr_compile_status='compiled'`** (per §0c-2), walks the compiled DAG via `petgraph::toposort` with parallel execution up to `attr_config.max_concurrent` (Terraform's pattern), reads `execution_params` for the run, sets session auth, reads each DAG node's prompts/instructions/capability/agent attrs via `state_ledger`, dispatches via `CapabilityGovernor` + the appropriate tool blade, writes results back as superseded state, transitions the schedule row. Every step emits typed `schedule_*` / `runner_*` telemetry — no silent failures, no untracked retries. |
| 3 | `DesignerBlade` (intent → uncompiled DAG) | `superx-designer` | [NEW] | Consumes a product-intent payload (text + constraints) and the substrate's catalogue of `node_data_source*` / `node_component` / `node_hardened_model` / `node_agent` entities. Produces the full cascading DAG (`node_product → component* → task*`) with `edge_owns` / `edge_implements` / dep edges. Marks the root with `attr_compile_status='uncompiled'` via `state_ledger`. **Does not touch the schedule** — per §0c-2, uncompiled DAGs flow through `CompilerBlade` (#30) and `SchedulerBlade` (#31) before any schedule row is created. Uses the local model (via Rig.rs once #16 lands) for the planning step; every model call emits typed telemetry with rationale text. |
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
| 30 | `CompilerBlade` (DAG validation, Terraform-`plan` analog) | `superx-planner` | [NEW] | Walks an uncompiled product DAG (root: `node_product` with `attr_compile_status='uncompiled'`) and validates **every connection** before anything queues. Topology via `petgraph`/`daggy`: `is_cyclic_directed` (no impossible cycles), `toposort` (execution order), single-root + reachability. Substrate: every referenced entity exists; every `node_data_source_sql` reachable via `sqlx::query!("SELECT 1")`; every `node_rag_source` corpus queryable; every `node_hardened_model` weights resolvable; every `node_agent` has the capability edges its task requires. Templates: every prompt entity resolves all `{variable}` refs against upstream task outputs (`minijinja`/`tera`). Local-model checks via Rig.rs: semantic sanity ("does this prompt match this RAG?"), missing-step detection, agent/intent alignment, rationale recorded. Writes `attr_compile_status='compiled'` + `attr_compile_report` (resolved refs, fuel cost estimate, telemetry budget, failure list) on every DAG entity via `state_ledger`. **Uncompiled DAGs cannot enter the schedule.** Idempotent + re-runnable; SCD-2 history shows the moment a previously-valid DAG became invalid (upstream drift). Requires two new metamodel types (`attr_compile_status`, `attr_compile_report`) — gated by §7 of the operating skill (operator approval per change). |
| 31 | `SchedulerBlade` (Gemma over the dumb queue) | `superx-scheduler` | [NEW] | The local-model intelligence layer above the `schedule` table. Continuous background loop: reads compiled DAGs (`attr_compile_status='compiled'`), current `schedule` rows, telemetry windows, agent load, `execution_params` history. Proposes scheduling decisions via `node_proposal` rows scored by the Meta-Harness — which compiled DAG enqueues next + with what `execution_params`, which failed items retry-with-new-knobs / abandon / escalate-to-HITL, which DAG successors are newly ready (graph-walk via `petgraph` from the just-completed entity), which param defaults to tune per `task_kind` (subsumes the `ParamTunerBlade` #26 pattern). Every model call emits a typed `scheduler_*` telemetry event with rationale text. Accepted proposals (auto-promote per capability or HITL approval) become schedule mutations `RunnerBlade` (#2) executes mechanically. **The `schedule` table itself stays dumb** — all judgment lives in proposals, full audit trail per §9 of the operating skill. Generalises the on-demand `ProposerBlade` pattern that already exists for structural edges. |
| 32 | `KernelTapBlade` (eBPF / EndpointSecurity / ETW; kernel-verified telemetry) | `superx-kernel-tap` | [NEW, **v2.x+** — security-buyer / closed-source-agent trigger] | Kernel-level observation of agent processes via eBPF on Linux ([`aya`](https://github.com/aya-rs/aya), Rust-native loader, BTF-portable across kernel versions), EndpointSecurity on macOS (Apple's signed-app successor to dtrace; needs `com.apple.developer.endpoint-security.client` entitlement), and ETW providers on Windows. Observes process exec/fork, stdio between agent ↔ MCP server subprocesses (uprobes on read/write or pipe fd taps), file open/read/write tracepoints, network connect/sendmsg syscalls (endpoints + timing — TLS payload remains encrypted), syscall latency. **Industry validation (2026):** Metoro, Grafana Beyla, and AgentSight ([arxiv:2508.02736](https://arxiv.org/abs/2508.02736)) all ship eBPF-based AI-agent monitoring as a primary mechanism, with measured **&lt;3% performance overhead**, framework-agnostic capture, and **decrypted LLM-payload visibility at the syscall layer** (more efficient than network/proxy approaches because the data is intercepted before TLS encryption). Security advantage: eBPF runs out of the application process, uses push-only telemetry, opens no ports — strictly tighter than a proxy. **Out of scope for v0.1–v1.x** because: (a) requires root / `CAP_BPF` / signed-entitlement / admin, raising the trust bar customers haven't yet vetted us for; (b) three OS-specific implementations for ≈the same data transcript-watcher + OTLP-receiver vectors already capture passively; (c) per-kernel-version drift on uprobes against stripped userspace binaries; (d) enterprise compliance review burden. **Triggers that promote it to in-scope earlier:** (i) a closed-source agent that exposes no transcript / hook / MCP-proxy / OTel surface — kernel observation becomes the only path; (ii) a security/compliance buyer who demands kernel-verified telemetry that user-space layers can't be coerced into lying about (the AgentSight thesis); (iii) sub-millisecond performance instrumentation transcript watchers cannot give. Distribution model: a separate optional crate the operator installs as a signed package with elevated permissions; the main `superx` binary stays unprivileged. Format-on-the-wire: emit OpenTelemetry GenAI semantic-convention spans so the kernel-tap output drops into any OTel-aware backend without translation. |

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

The full agentic-OS pillar. Adds the **design → compile → schedule → run** pipeline of §0c-2 (Terraform-style); turns SuperX from collector+RAG into a system that *builds products on its own*. Ordered so each blade lands on a working baseline.

| Roadmap # | Item | Why it lands in this order |
|---|---|---|
| #1 | `schedule` table (SCD-2) — already merged | Dumb queue, no judgment, ready for consumers. |
| #1b | `execution_params` table (SCD-2) — already merged | Per-run knobs, ready for consumers. |
| #4 | `node_data_source` connectors (SQL / Iceberg / Postgres / RAG / remote-model) | Compiler needs to probe these during validation. |
| #3 | `DesignerBlade` (intent → uncompiled DAG) | Produces what Compiler validates. |
| #30 | `CompilerBlade` (Terraform-`plan` analog) | Gates entry into the schedule. |
| #31 | `SchedulerBlade` (local-model intelligence over the dumb queue) | Decides what runs when; subsumes #26 once it ships. |
| #2 | `RunnerBlade` (Terraform-`apply` analog; pure mechanical executor) | Pops compiled schedule rows, walks the DAG, dispatches. |
| #25 | `EdgeProposerBlade` (background continuous variant) | Enriches the substrate graph for future Designer runs. |
| #15 | Full MCP 2025-11-25 surface (`resources` / `prompts` / `sampling` / `elicitation`) | Exposes the pipeline to external clients. |

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

## 10. First Viable Product (v1.0) and the Blade Roadmap

### 10.1 What v1.0 *is*, in one sentence

> **Drop-in fine-grained telemetry capture for every MCP agent on the host machine, with a local model running RAG over ingested data for free, driven entirely from a CLI.**

That's the product. Everything below is *how* we ship that, blade by blade.

### 10.2 The seven v1.0 capabilities (and nothing else)

| # | Capability | What the operator sees |
|---|---|---|
| **C1** | **Auto-onboarding** | `superx-cli bootstrap --tenant demo` discovers every MCP agent on the system (Claude Desktop, Claude Code, Cursor, Continue, Cody) with no further config. **The local model classifies each discovered agent at discovery time** and emits a welcome `node_insight` summarising what it found and suggesting next steps. |
| **C2** | **Fine-grained telemetry firehose** | Every kernel mutation produces a typed `telemetry_stream` row. `superx-cli stats` shows the live feed. Optional Kafka / HTTP egress. |
| **C3** | **Local model integrated, always present, actively used** | A GGUF model loads on boot from `./models/`. `superx-cli install-model gemma-4-…` fetches one if absent. The model is a `node_hardened_model` entity. **Touches bootstrap, classification, summarization, insight, RAG, and the `ask` verb — not a passive dependency.** |
| **C4** | **RAG over any ingested source** | `superx-cli graphify --path <dir>` ingests + classifies + summarizes. `superx-cli compile --root <id>` returns a tier-filtered, summary-enriched context window the model has already pre-processed. |
| **C5** | **Continuous local intelligence** | Four background blades (Classifier, Summarizer, Insighter, Router) running 24/7, subscribed to `telemetry_stream`, producing classifications / summaries / insights without operator action. All four are model-driven. |
| **C6** | **Natural-language OS interface** ⭐ | `superx-cli ask "<question>"` — the model translates English to substrate queries, executes them, and returns an answer with citations. Example: `superx-cli ask "what agents have been most active in the last hour"` → reads telemetry, summarises, cites the source events. **This is what makes SuperX an *intelligent* agentic OS, not a fancy logger.** |
| **C7** | **Everything is a parameter** | No hardcoded values. Models, prompts, thresholds, sink targets, classifier vocabulary, HITL gates — all `attr_config` rows. `superx-cli prompts add/list/show/supersede`, `superx-cli config set/get`. |

If a behavior isn't covered by C1-C7, it's not v1.0. Designer, Runner, schedule table, federation, chat UI — all v1.x or later.

### 10.2.1 How the model is woven into v1.0 — every touchpoint

The model is not a passive "loaded" dependency. It is actively used at six distinct points in v1.0, each with a clear contract and graceful degradation if the model is unavailable:

| Touchpoint | What the model produces | Fallback if no model loaded |
|---|---|---|
| **At bootstrap, per discovered agent** | A `node_classification` row labelling the agent (`{role: "filesystem-server", trust: "user", expected_tools: [...]}` ) | Agent registered as `unclassified`; classifier blade picks it up later if model becomes available |
| **At bootstrap, post-discovery** | A `node_insight` welcome row: "I found 3 MCP agents, 2 of which are filesystem tools. You have a Rust project at /Users/…/SuperX — would you like to graphify it for RAG?" | Bootstrap completes silently; operator sees census telemetry only |
| **At ingest** (`graphify`) | Per-file `node_classification` (test / config / production-logic / dead-code) + `attr_summary` rows for long content | Files ingested with raw `attr_desc` only; classification rows missing |
| **At telemetry windows** (continuous) | `node_insight` rows when anomaly heuristics fire — but with the model's narrative explanation attached | Insights still fire from heuristic rules; the narrative field is empty |
| **At `compile_context`** (RAG) | Tier-filtered context, *with summaries inlined* where they exist | Raw `attr_desc` text without summaries; larger context window consumed |
| **At `ask` verb** | English → substrate query plan → execute → natural-language answer with cited entity ids | `ask` fails fast with "no model loaded; use `superx-cli install-model gemma-…` first" |

**Graceful degradation is non-negotiable.** If the model file is missing, SuperX still boots, still captures telemetry, still ingests data — every blade logs a clear warning and idles. The operator can install a model anytime and the blades wake up. This is what "intelligent OS" requires: the *OS* part must work even when the *intelligent* part is offline. The Router blade encodes this explicitly: tasks that require a model and have no fallback fail with a specific error code the operator can act on.

### 10.3 The blade roster — every feature = exactly one blade

This is the modular contract. **A "feature" is a blade with a config schema, a subscription, an output type, and a parameter set.** Adding a feature post-v1.0 means adding a row to this table and implementing it. The substrate doesn't change.

#### Blades shipping in v1.0

| Blade | Lives in | Subscribes to | Writes | Parameters (on `attr_config`) | Mode |
|---|---|---|---|---|---|
| **`TelemetrySubscriber`** | `superx-emission` | `LIVE SELECT telemetry_stream` | Kafka + HTTP egress | `emission_enabled`, `data_scope`, `kafka_brokers`, `kafka_topic`, `emission_api` | All |
| **`HeartbeatPulse`** | `superx-mcp` (bin) | `tokio::interval(60s)` | `system_pulse` telemetry rows | `pulse_interval_secs` | All |
| **`BootstrapBlade`** | `superx-bootstrap` | (one-shot on `bootstrap` CLI) | substrate seed + `agent_discovered` events + model-classified agent labels + welcome `node_insight` (when model is loaded) | `bootstrap_default_admins`, `mcp_client_probe_set`, `bootstrap_model_classify_enabled`, `bootstrap_welcome_prompt_uid` | All |
| **`AskBlade`** ⏳ | `superx-runtime::ask` | (called by CLI `ask` verb) | Natural-language answer + citation entity ids | `ask_planning_prompt_uid`, `ask_max_context_tokens`, `ask_default_model` | All |
| **`ContinuousClassifierBlade`** ⏳ | `superx-runtime::classifier` | `state_supersede` on `node_code` / `node_artifact` | `node_classification` + `edge_classifies` | `classifier_enabled`, `classifier_vocabulary`, `classifier_threshold` | Collector, Designer, Full |
| **`SummarizerBlade`** ⏳ | `superx-runtime::summarizer` | `state_supersede` on `attr_desc` rows > token threshold | `attr_summary` SCD-2 rows | `summarize_threshold_tokens`, `summarize_max_output_tokens` | Collector, Designer, Full |
| **`InsightHarvesterBlade`** ⏳ | `superx-runtime::insighter` | Windowed scan of telemetry | `node_insight` + `edge_observes` to source events | `insight_window_secs`, `insight_anomaly_thresholds` | Collector, Designer, Full |
| **`ModelRouterBlade`** ⏳ | `superx-runtime::router` | Every model-using dispatch | `execution_params` row recording the choice | `routing_escalation_rules`, `routing_default_thresholds` | All |
| **`CapabilityGovernor`** | `superx-agent` | (called inline on every MCP dispatch) | (no writes, returns Result) | `capability_audit_enabled` | All |
| **`MetaHarness`** | `superx-harness` | (called by `MetaHarness::evaluate/promote`) | `attr_score`, `edge_promotes` | `metaharness_fuel_limit`, `metaharness_default_threshold` | All |
| **`ContextCompilerBlade`** (a.k.a. `CompilerBlade` in `superx-compiler` today; the rename lands when the DAG `CompilerBlade` #30 ships, to avoid the name collision) | `superx-compiler` | (called by CLI `compile` + Proposer + Runner) | XML **context** output (entity DAG → agent-ready XML) | `compile_max_depth`, `compile_max_nodes`, `compile_default_tiers` | All |
| **`UniversalIngestor`** | `superx-ingest` | (called by CLI `graphify`) | new entities + `edge_owns` + `attr_desc` | `max_ingestion_entries`, `ingest_exclude_patterns` | Collector, Designer, Full |

⏳ = not yet implemented, ships in v1.0.

Every blade above is independently testable, independently togglable, independently parametrizable. No blade reaches into another blade's internals. Adding a blade is a Cargo file + a module file + a registration call in the host binary.

#### Blades shipping post-v1.0 (the visible horizon)

Each row in this table is one future release. Operators see exactly what each release adds.

| Release | New blades | Capability gained |
|---|---|---|
| **v1.1** | `EdgeProposerBlade` (background) — Phase F start | Continuous structural-edge proposal; Meta-Harness gates promotion. Plus refinement of v1.0 blades based on usage data. |
| **v1.2** | `ParamTunerBlade` | Self-tuning `execution_params` based on schedule outcomes. Substrate gets better at running itself. |
| **v2.0** | `RunnerBlade` (Terraform-`apply` analog; pure mechanical executor) | First autonomous execution. Schedule items get consumed by background workers; tasks run without operator triggering. **Crosses from "intelligent collector" to "autonomous OS."** |
| **v2.1** | `DesignerBlade` + `CompilerBlade` (DAG, Terraform-`plan`) + `SchedulerBlade` (Gemma over the queue) | The full §0c-2 pipeline: operator describes a product in English → Designer emits an uncompiled DAG → Compiler validates every connection (`petgraph`/`daggy` + Rig-driven semantic checks) → Scheduler decides what enqueues + with what knobs → Runner executes. Subsumes the standalone `ParamTunerBlade` (now a SchedulerBlade proposal kind). |
| **v2.2** | `DataSourceConnectorBlade` (SQL / Iceberg / RAG / remote-model subtypes) | Connectors for MySQL, Postgres, Iceberg, S3, Confluence, etc. The "compiled data sources" pillar. |
| **v2.3** | `OtelSink` + GenAI semantic conventions | Datadog / Honeycomb / Grafana plug-and-play observability. |
| **v3.0** | `SupersetSink` + `MothershipModelRouter` | Federated analytics: local SuperX nodes phone home to a central Superset for org-wide telemetry; complex queries escalate to a central mothership model. |
| **v3.1** | `EchartsToolBlade` | Dynamic chart generation: the model produces eCharts JSON configs; a new tool serves them. |
| **v3.2** | `CrdtDagEditingBlade` (Automerge-rs) + `ChatUiBlade` | Collaborative DAG authoring; chat UI is a downstream consumer of the substrate via MCP. |
| **v4.0** | `WasmComponentBlade` | Tool blades are wasm components with capability-typed imports. Long-term safety story. |

Every release is **strictly additive**. No release breaks an earlier release's API contract. SCD-2 + entity-of-everything means migrations are substrate writes, not schema changes.

### 10.4 v1.0 implementation phases (the work breakdown)

| Phase | Work | Issues | Acceptance |
|---|---|---|---|
| **A** — Tech-debt baseline | Backfill doc comments → diligent read-through → detailed comment pass. | #1, #2, #3 | All pub items documented. Zero `unwrap()` outside known-safe sites. |
| **B** — Substrate foundations | `execution_params` SCD-2 table + `execution_cursor` SCD-2 conversion + prompt-template entity helper + `attr_summary` + `attr_capability_score` types. | #5 (subset), #6 (subset), #14, new | New verbs + types pass per-feature tests. |
| **C** — Model plug-and-play | Rig.rs adoption + `./models/` convention + `superx-cli install-model` + `ModelRouterBlade` scaffold. | #16, #24, #27 (new) | One local + one remote `CompletionModel` implementation in tree, both selectable via `execution_params`. |
| **D** — Continuous intelligence blades + Ask | New crate `superx-runtime` with Classifier / Summarizer / Insighter / Router fully wired, **plus `AskBlade` and the `superx-cli ask` verb**. Model-aware bootstrap (classify discovered agents + emit welcome insight). | #20 (revised), #21, #22, #23 (new), Ask | Each blade independently togglable; integration test proves end-to-end firehose lights up; `ask` returns a sensible answer with citations on a fixture-bootstrapped substrate. |
| **E** — Onboarding completeness + release polish | Extend bootstrap probes to Claude Code / Cursor / Continue / Cody. `superx-cli prompts` subcommand. MVP integration test suite. `cargo dist` release pipeline. Tag v1.0.0. | #28 (new), #29 (new) | Operator can `cargo install` (or download binary) and have the full collector+RAG+Ask running in < 5 minutes. |

Each phase ends with a green Mandate-5 gate. No phase starts before the previous phase's gate passes.

### 10.5 v1.0 release checklist

1. ☐ Capabilities C1-C6 all implemented + smoke-tested end-to-end
2. ☐ All v1.0 blades from §10.3 implemented + per-blade tests
3. ☐ `cargo test --workspace`: 100% pass
4. ☐ `cargo clippy --workspace --all-targets --all-features -- -D warnings`: clean
5. ☐ `scripts/mvp_smoke.sh` passes locally on macOS + Linux
6. ☐ Every pub item has a `///` rustdoc comment
7. ☐ `README.md` updated with v1.0 quickstart matching §10.1's one-sentence positioning
8. ☐ `RELEASES.md` v1.0 release notes drafted
9. ☐ `cargo dist` builds binaries for macOS (Apple Silicon + Intel) + Linux x86_64
10. ☐ GitHub release tagged `v1.0.0` with binaries attached + 30-second asciinema cast
11. ☐ Pinned ARCHITECTURE issue updated with "v1.0 shipped — see §10.3 blade roster" surface

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