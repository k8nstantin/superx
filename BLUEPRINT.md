# SuperX Blueprint — The Agentic OS (Reset of 2026-08-06)

> **Status: v1.0 — decisions D1–D4 locked by the operator 2026-08-06.**
> This document lands with the G0 reset PR; merging that PR is the
> operator's approval of the reset. The complete v1 FVP is preserved at
> the tag `archive/pre-reset-2026-08-06`. This is the living canon —
> it replaces `ARCHITECTURE.md` (retired with v1).

## 1. Mission

SuperX is an **agentic operating system** written in Rust.

- **SurrealDB is the OS's filesystem.** Every fact the OS knows —
  agents, telemetry, state, configuration, the module registry itself —
  lives in the substrate. If it isn't in SurrealDB, the OS doesn't know
  it. No sidecar config files, no in-memory-only state that matters —
  with one deliberately narrow bootstrap-scope exception (D12/D16):
  what must be known BEFORE the substrate is reachable lives in the
  `<home>/{params,logs,db}` instance layout (`params/superx.json`,
  the credentials file, the pidfile, the logs).
- **Telemetry capture is the kernel's core capability.** From the moment
  the OS boots, it captures all telemetry emitted by every agent on the
  system. This is not a module, not an option, not a plugin — it is
  what the kernel *is*. An agentic OS that cannot see its agents is not
  an OS.
- **Everything else is a module.** Data fusion, graphify, and every
  future capability arrive as modules loaded on top of the kernel. The
  kernel has zero knowledge of any module's domain.

## 2. What the kernel does (and nothing else)

1. **Substrate access** — typed verbs over SurrealDB. Append-only:
   SELECT + CREATE only, no verb ever issues UPDATE / DELETE / DDL.
   "Current state" is computed, never mutated (SCD-2).
2. **Boot** — `superx boot` connects to the substrate, registers
   modules, brings them up with failure isolation, and starts the
   telemetry engine. Boot always completes; a failed module never
   takes the OS down.
3. **Agent telemetry capture** — the engine that discovers agents on
   the machine, tails whatever they emit, writes typed telemetry rows
   **and first-class conversation rows** (`message` table — every
   session with every agent, very detailed, raw event preserved), and
   checkpoints with cursors so restarts are lossless. Runs from boot
   until shutdown; captures everything by default (operator decision
   2026-08-07 — no per-agent grant gate; OS-level file prompts are the
   permission surface).
4. **CLI** — the operator interface: `--initialize` (one-command
   provisioning + background boot, idempotent per instance; the
   terminal returns), `start` / `stop` / `restart` / `status` (background-OS
   lifecycle, pidfile-backed), `agents`, `actions [--agent] [--live]` (the
   telemetry stream), `sessions` (list conversations), `read
   <session-fragment> [--live]` (render a conversation, historical
   then follow), `logs [--follow] [--daemon]` (the OS's own log).
   `boot` remains as the foreground debug mode. All bootstrap config
   resolves flag > env > `params/superx.json` > fallback. The CLI is
   part of the kernel deliverable, not an app bolted on later.
5. **Module system** — modules register with the kernel, declare
   dependencies, and get lifecycle management (starting / active /
   failed / skipped). The registry lives in the substrate like
   everything else.

**Self-log.** The kernel keeps its own rolling log file in a local
`logs/` directory (override: `SUPERX_LOG_DIR`), independent of the
substrate — boot diagnostics and signin failures must be observable
when the database is down. Substrate telemetry remains the audit log
of record; the self-log is the layer below it. (Operator directive,
2026-08-07.)

**Agent adapters.** The capture *engine* is kernel-core. Knowing how a
*specific* agent emits telemetry (Claude Code writes JSONL transcripts;
another agent may expose OTLP, logs, or an API) is adapter knowledge.
Adapters are the one plugin interface the kernel itself defines — small,
per-agent, and hot-swappable without touching the engine.

## 3. What carries forward from v1 (proven mechanics, not code)

The v1 codebase is archived wholesale; no file survives on `main`. But
these v1 mechanics were verified live and are carried forward as
*design decisions*, re-implemented clean:

- Append-only substrate with SCD-2 (`valid_from`, latest-wins reads).
- Types-as-data metamodel: new entity/attr types arrive by INSERT,
  never by schema migration.
- Cursor-checkpointed capture: poll → emit telemetry → write cursor;
  restart-lossless; errors become telemetry, never panics.
- Service-account discipline: the kernel signs in as a minimal-privilege
  user, never root; the operator owns the schema and applies it once.
- Module lifecycle with failure isolation and topological boot order.
- Tolerant parsing: unknown telemetry shapes are captured raw, not
  dropped.

## 4. What is retired

- The 6-crate L0–L3 layering (kernel / kernel-modules / drivers / apps).
  v2 started as **one workspace, two crates**: `superx-kernel` (lib) and
  `superx` (bin, the CLI). Crates are extracted only when a module
  graduates and proves the seam — the graduates are
  `superx-mod-hello`, the reference module (epic #141), `superx-ops` (P3): the shared runners/renderers both the CLI and
  modules consume — kernel-only dependencies, so modules never depend
  on the bin — and `superx-mod-ui` (P4+): the UI module itself, an
  axum server inside the daemon on `attr_ui_port`: the template
  contributors copy, exercising every facility of the module contract
  (own schema/db, own dir, own log, own CLI).
- The F-phase roadmap, the 2026-06 quality backlog, and all 20 open
  pre-reset GitHub issues (they reference retired concepts).
- `ARCHITECTURE.md` (v42.15, 88 KB) — the historical vision document.
  This blueprint replaces it as the living canon. Bloat was a named
  failure mode; this document stays under 300 lines or it gets split.

## 5. The new FVP

> The operator runs `superx boot`. Every coding agent on the machine
> is discovered and stored in the substrate. Detailed telemetry AND
> full conversations stream in from Claude Code, Gemini CLI, and
> Claude Desktop. `superx actions --live` shows what agents are doing;
> `superx read <session> --live` lets the operator read any
> conversation — historical first, then live as it continues. Adding a
> new agent adapter touches zero engine code.

Measured by: **three** agent adapters working day one (Claude Code,
Gemini CLI, Claude Desktop), conversations readable end-to-end,
lossless restart mid-stream, and the whole workspace green at the
gates.

## 6. Phase plan (each phase = one branch, one PR, one day max)

| # | Scope | Exit criterion |
|---|---|---|
| G0 | Reset PR: archive tag, wipe to skeleton, this blueprint, fresh README | `main` = blueprint + empty 2-crate workspace, gates green |
| G1 | Schema design session (operator + model, §11) → `SUPERX_SCHEMA.md` v2 + `schema/kernel.surql` v2; operator applies under root | Schema deployed and locked |
| G2 | Kernel substrate verbs + telemetry primitive + tests | Verbs green on kv-mem |
| G3 | Module system + boot orchestrator | Fake-module boot tests green |
| G4 | Capture engine + agent adapter #1 (Claude Code): telemetry + conversations | Live message + action rows on a real machine |
| G5 | Agent adapters #2 + #3: **Gemini CLI**, **Claude Desktop** (amended 2026-08-07) | Three adapters, zero engine edits |
| G6 | CLI complete: `boot`, `status`, `agents`, `actions --live`, `sessions`, `read` — **FVP** | The demo sentence above, live |
| G7+ | First modules: data fusion, graphify | Module seam proven |
| **R1** | **v1.0.0 released** (2026-08-18): FVP + one-command background initialize + params file + logs | Tagged from `main`; operator-QA'd live |
| **G9** | **Runner module** (2026-08-19, epic #189, R0–R4 on one branch/PR): `superx-mod-runner` — schedules entities (D23), plans waves over `depends_on` with cycle refusal, executes task nodes via the configured agent command with parallel dispatch + write-back (D25/D26), in-flight steerable with version pinning (D27), perpetual `every:` recurrence; cross-module via CLI dispatch (D24) | 17 contract tests incl. the full firing lifecycle; operator's example fires end-to-end on a live instance |
| **G8** | **Entities module — the product graph substrate** (2026-08-19, epic #166, E1–E4): `superx-mod-entities` — typed entities as graph nodes (18 seeded types incl. repo/credential + the runner's depends_on/produced, runtime-extensible), native SurrealDB edges (D19), identity/state split (D20), text-as-entity (D22), documents as nodes, BFS tree/JSON traversal, full append-only history; executor is the NEXT epic (D21) | Operator's example graph (product → components/task → rag/texts/document) built + traversed end-to-end via CLI; 15 contract tests |
| **G7** | **Module framework + UI module shipped** (2026-08-19, epic #141): full facility contract (D17/D18), module ledger (v2.2), `superx-mod-hello` reference, `superx-mod-ui` dashboard (Status/Activity/Sessions/Console on Mantine), docs/MODULES.md | Every facility live-QA'd; contribution path documented |

## 7. Decisions record

| # | Decision | Outcome (operator, 2026-08-06) |
|---|---|---|
| D1 | Reset mechanics | **Same repo**; v1 preserved at tag `archive/pre-reset-2026-08-06` |
| D2 | Where capture lives | **In-kernel**; only per-agent adapters are pluggable |
| D3 | FVP adapters (Claude Code is #1) | **Claude Desktop** (2026-08-06); **amended 2026-08-07**: + Gemini CLI — three adapters, Gemini CLI second |
| D4 | Zero-trust skill fate | **Rewritten from scratch** against this blueprint; lands in the G0 PR |
| D5 | Dead `feat/ingest-rust-codebase` branch (one unmerged pre-reset commit) | **Deleted** 2026-08-06 |
| D6 | v1 GitHub issues (23 open, all pre-reset) | **Closed** 2026-08-06 with pointers here |
| D7 | Conversation storage (2026-08-07) | **Dedicated `message` table** — first-class, readable historical + live; raw source event always preserved |
| D8 | Capture consent (2026-08-07) | **Capture everything by default** — no per-agent grant gate; OS file prompts are the permission surface |
| D9 | Agent storage (2026-08-07) | **Agents stay entities** (`node_agent`); entity row id = agent_id; new agent properties are data writes, never schema migrations |
| D10 | CLI viewing surface (2026-08-07) | `actions [--live]` + `sessions` + `read <session> [--live]` |
| D11 | One-command init (2026-08-18, issue #120) | `superx --initialize` prompts for ONE password (any accepted at this phase) serving both database root and the service account; root is used once in-process to apply the schema and never stored |
| D12 | Credential persistence (2026-08-18) | Service password saved to a `0600` `superx-credentials` file beside the datastore so query commands need no exports; env var still wins |
| D13 | Git discipline (2026-08-18) | Every change ties to a GitHub issue; branches + PRs reference it |
| D14 | Session lookup (2026-08-18, #122) | Sessions identified `<agent>/<uuid7>`; `read` matches any unique fragment; source id rides as `src=` |
| D15 | Background OS (2026-08-18, #124) | `--initialize` returns the terminal: OS runs detached (pidfile, daemon log); `superx stop`; duplicate-capture guards; shutdown honored between source polls |
| D16 | Instance config (2026-08-18, #125) | `params/superx.json` controls all bootstrap config inside the `<home>/{params,logs,db}` layout; precedence flag > env > file > fallback; `superx logs` surfaces the self-log. Released as **v1.0.0** |
| D17 | Module facility contract (2026-08-19, epic #141) | A module is a self-contained mini-app: own db `superx/<name>` + service account (via `modules provision`), own dir, own log target, own CLI namespace, own parameters; kernel is the hub — modules depend on the kernel only, never on each other |
| D18 | Module identity (2026-08-19, epic #141) | Every module is a registry entity with a uuid7 id; same-kind modules coexist (nothing assumes a singleton); compiled-in v1 contract (linkme), WASM parked |
| D19 | Native graph edges (2026-08-19, epic #166) | The entities module links nodes with SurrealDB `RELATE` edges (`TYPE RELATION IN entity OUT entity ENFORCED`): traversal follows record pointers (cost = a node's degree, never table size); RELATE is creation-only — blessed as CREATE-equivalent under the append-only contract, module db only |
| D20 | Identity/state split (2026-08-19, epic #166) | Graph nodes = one immutable uuid7 anchor row (stable edge target, forever) + SCD-2 `entity_state` version chains; edge history = `edge_uid` chains where unlink appends `active=false` |
| D21 | Executor is the next epic (2026-08-19, epic #166) | This epic ships the graph substrate + CLI; the runner module that walks the graph and executes task nodes — and the dashboard graph page — follow separately |
| D22 | Text-as-entity (2026-08-19, epic #166) | All long-form text (descriptions, comments, instructions) is a `text` NODE linked by role edges (`describes`/`comments`/`instructs`), direction owner → target; entities keep a one-line `name` label; describe/instruct evolve one node (its history = the text's evolution), comments multiply (threads = comment a comment) |
| D23 | Runner schema (2026-08-19, epic #189) | Two tables, SCD-2, inserts only, uuid7 everywhere: `schedule` intent chains (time + entity ref + recurrence — NOTHING else; the graph is the plan) + `run` fact chains (one per firing×task; statuses appended) |
| D24 | Cross-module contract v1 (2026-08-19, epic #189) | Modules talk through each other's CLIs via the kernel's in-process dispatch (`entities graph --json`, `entities create/link`) — the same surface humans, the Console, and agents use; no module-to-module crate deps |
| D25 | Dependency semantics (2026-08-19, epic #189) | `depends_on` edges among task nodes; runnable = all deps done THIS firing; the runnable frontier dispatches in parallel up to `attr_runner_max_parallel`; failed deps block dependents (never dispatched) |
| D26 | Executor v1 (2026-08-19, epic #189) | One contract: spawn `attr_runner_agent_cmd` (NO default — unset refuses loudly) with the assembled prompt (task name · instructs · product describes · 1-hop linked context); stdout writes back as `task —produced→ text`; recurrence `every:<n><s\|m\|h\|d>` |
| D27 | In-flight steering (2026-08-19, epic #189) | The subgraph re-reads at every frontier re-evaluation — mid-run graph/instruction edits apply to everything not yet dispatched; every run row pins the instruct version it dispatched; cancel halts future dispatch, in-flight finishes and records |

## 8. Binding rules (unchanged from v1, restated)

Trunk-based development: single trunk `main`, always releasable, short-
lived branches, one PR at a time, all-or-nothing branches. Gates:
`cargo test --workspace` + `cargo clippy --workspace --all-targets
--all-features -- -D warnings` + `python3 tools/skill_audit.py`
(CI-enforced as required checks). Schema changes are operator-approved,
per-change, schema-first (`SUPERX_SCHEMA.md` before code). The kernel
authenticates as a service account, never root. No hardcoded policy —
constants flow through substrate parameters.
