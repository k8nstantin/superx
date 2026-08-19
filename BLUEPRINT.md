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
   terminal returns), `stop` / `status` (background-OS lifecycle,
   pidfile-backed), `agents`, `actions [--agent] [--live]` (the
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
  v2 starts as **one workspace, two crates**: `superx-kernel` (lib) and
  `superx` (bin, the CLI). Crates are extracted only when a module
  graduates and proves the seam.
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

## 8. Binding rules (unchanged from v1, restated)

Trunk-based development: single trunk `main`, always releasable, short-
lived branches, one PR at a time, all-or-nothing branches. Gates:
`cargo test --workspace` + `cargo clippy --workspace --all-targets
--all-features -- -D warnings` + `python3 tools/skill_audit.py`
(CI-enforced as required checks). Schema changes are operator-approved,
per-change, schema-first (`SUPERX_SCHEMA.md` before code). The kernel
authenticates as a service account, never root. No hardcoded policy —
constants flow through substrate parameters.
