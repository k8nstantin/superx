# SuperX v0.1 — QA Test Path

A walkthrough for a tester who has never seen this codebase before. Read top-to-bottom in order. Total runtime if everything works: ~60 min.

---

## 0. What is SuperX?

SuperX is an **agentic operating system** — a long-running program designed to autonomously design, build, schedule, and execute software products through a population of coding agents, with a local LLM (Gemma 3 today, swappable) woven through every runtime decision. Think *"Terraform for agentic workflows"* with a graph database underneath.

The vision (`ARCHITECTURE.md §0`) is two-phase:

1. **Design phase**: an operator declares an intent (*"build me a Postgres-backed SaaS that ingests Confluence and serves a REST API"*). A `DesignerBlade` consults the substrate's catalogue of compiled data sources, reusable components, and model entities, and **produces a cascading DAG** of work items (`node_product → node_component* → node_task* → node_artifact*`).
2. **Orchestration phase**: the DAG enters a `schedule` table; a background `RunnerBlade` consumes due items, dispatches each to the appropriate tool blade through a capability check, writes results back to the substrate as new state rows, and emits fine-grained telemetry per step.

Today we have the **orchestration half** working end-to-end. The design half (DesignerBlade, SchedulerBlade) is roadmap.

### Why this is interesting (and not just *"another workflow engine"*)

- **Everything is an entity.** Products, components, tasks, agents, tools, model versions, source files — all rows in one `entity` table. The metamodel is itself entities (`type_definition`). New "kinds of things" don't need schema migrations.
- **Append-only substrate.** Every state change is a new row (SCD-2 chains keyed by `(target, type)`). The current value is *the most recent row by `valid_from`*. History is never destroyed; you can replay any moment.
- **Engine-typed FKs everywhere.** Every cross-table reference is `record<entity>` / `record<type_definition>` (a typed `Thing` in SurrealDB). Adding a tenant-coercion bug is engine-refused at insert time, not silently corrupted.
- **Tenant isolation.** Every entity is scoped to a `node_substrate` row (the "tenant"). Sessions are bound to one substrate; the kernel refuses cross-tenant reads/writes.
- **Capability-gated dispatch.** Agents have `edge_has_capability` relations to tools they're allowed to invoke. Every dispatch checks this before running.

### What you're testing today

The slice of the vision that's shipped and working on `main`:

- The substrate (9 tables, SurrealDB on RocksDB).
- The kernel verbs (CREATE-only writes — no UPDATE/DELETE).
- Bootstrap (provisioning a tenant with substrate + agents + tools + capability edges).
- Direct-API blades: ingest, compile, propose, evaluate, promote.
- The **schedule queue + runner pipeline** — the new piece. An operator can enqueue work; a background process picks it up; dispatches to the right blade; transitions the schedule row through `waiting → scheduled → running → completed/failed`.
- A CLI (`superx-cli`) that gives operators every verb as a shell command.
- An MCP server (`superx-mcp`) that exposes the same verbs as MCP tools so agent runtimes (Claude Code, Cursor) can drive SuperX.

You are **not** yet testing:

- DesignerBlade (intent → DAG): doesn't exist.
- SchedulerBlade (Gemma-driven autonomous queuing): doesn't exist.
- LLM-driven autonomous loops: the inference engine is wired but not in continuous operation.
- Engine-level UPDATE/DELETE refusal: deliberately not implemented (see §11).

---

## 1. Mental model — the four primitives

You'll see these terms throughout the test. Cheat sheet:

| Term | What it is | Real-world analogue |
|---|---|---|
| **Entity** | A row in the `entity` table — a *thing* (product, component, task, agent, tool, source file, run, …) | A row in a domain table |
| **Tenant** | A specific `node_substrate` entity. All other entities scope to it. | A tenant in a SaaS app |
| **State ledger** | An append-only `state_ledger` table of typed attribute values keyed by `(target_entity, type)`. The current value of "task X's description" is the most-recent ledger row matching that key. | A change-history table |
| **Schedule row** | A row in the `schedule` table that says *"run this kind of work against this entity"*. SCD-2: every status change appends a new row. | A work-queue item |
| **Runner** | A background loop that polls the schedule, picks due `waiting` rows, dispatches them through the right blade, and walks the schedule chain through scheduled → running → completed (or failed). | A worker process consuming a queue |
| **Dispatcher (`KernelDispatcher`)** | The seam where the runner meets the actual tool blades. Decides which blade to invoke based on the schedule row's `kind`, checks capability, runs the blade, returns success/failure. | A router |
| **Blade** | A focused crate that does one job — ingest a directory, compile context for an entity, propose an edge between two entities, evaluate a wasm scorer, promote a passing proposal. The system's "tools". | A microservice |
| **Capability check** | Before any dispatch, the runner asks *"does this agent hold `edge_has_capability` to this tool?"* via `CapabilityGovernor`. If not, the dispatch is refused before any side effect. | RBAC check |

The flow you're testing:

```
operator types `enqueue` → schedule row appears (status=waiting)
runner ticks every N ms → picks the row → transition: scheduled
runner ticks → transition: running → dispatcher invoked
dispatcher → capability check → tool blade runs → returns Result
runner transitions: completed | failed
telemetry row written for every step
```

---

## 2. Prerequisites

```bash
# Rust toolchain (1.81+ recommended).
rustup show
cargo --version

# Build the full workspace. Always `cargo build`; do not invoke
# rustc directly or use a release-stripped artefact.
cargo build --workspace

# Pick a fresh substrate path for QA runs. This will be DESTROYED
# between test sessions to ensure isolation.
export SUPERX_DB_PATH=/tmp/superx_qa/superx.db
rm -rf /tmp/superx_qa
```

**What you should see**: `cargo build --workspace` completes with no errors. 12 crates compile (`superx-kernel`, `superx-runner`, `superx-dispatcher`, `superx-agent`, `superx-bootstrap`, `superx-compiler`, `superx-emission`, `superx-harness`, `superx-inference`, `superx-ingest`, `superx-proposer`, `superx-cli`, `superx-mcp`). First build takes ~5 min; incremental rebuilds are seconds.

**If this fails**: stop here. Build is the floor. Common causes:
- Rust toolchain too old → `rustup update`.
- Missing system deps (some crates pull rdkafka / candle) → `brew install librdkafka` on macOS, `apt install libssl-dev pkg-config` on Linux.

---

## 3. Mandate-5 gates — the green-bar baseline

Two commands the project considers non-negotiable: the full test suite must pass, and the linter (configured to deny warnings) must be clean. These are the gates every PR has to clear before merging.

```bash
# Test gate — runs every test target in every crate.
cargo test --workspace

# Linter gate — clippy at strict pedantic level, warnings denied.
cargo clippy --all-targets --all-features -- -D warnings
```

**What you should see**:

`cargo test --workspace` should print 6 "test result" lines, totaling **64 passing, 0 failing**:

| Test target | Tests | What it covers |
|---|---|---|
| `superx-kernel` (lib unit tests) | 7 | Kernel-verb invariants: NASA-rule cycle prevention, tenant isolation, SCD-2 supersede, durable cursors, context-compile node limit, parse_id boundary |
| `superx-runner` (lib unit tests) | 4 | RunnerBlade tick loop: dispatch happy path, future-dated chains skipped, no double-processing, dispatch errors land chains in `failed` |
| `superx-cli` integration `core_capabilities` | 46 | Most of the v0.1 surface — bootstrap idempotency, agent discovery, governor allow/deny, ingest blade, harness wasm sandbox, schema validation, MCP dispatch, SCD-2 chain semantics, etc. |
| `superx-cli` integration `integration` | 1 | End-to-end NASA cognitive loop (bootstrap → ingest → propose → promote) |
| `superx-cli` integration `runner_e2e` | 2 | Runner end-to-end: happy compile, capability-denial → failed |
| `superx-cli` integration `schedule_scd2` | 4 | SCD-2 invariants: reconstructability, append-only, byte-identical prior rows, long-chain reconstruction |

`cargo clippy` should finish with `Finished` and no warning/error output. If you see anything else, the linter has caught something.

**If this fails**: stop. The codebase is supposed to maintain these gates green at all times; a failure here means something regressed and the rest of the QA path will likely fail too.

---

## 4. The FVP smoke test — `runner-demo` (the one-command proof)

This is the **headline test**. One command runs the entire orchestration pipeline end-to-end against a fresh substrate and reports the result.

What's happening under the hood (you do not need to type any of this; the demo does it for you):

1. Initialize SurrealDB on RocksDB at the temp path.
2. Apply the v2 substrate schema (9 tables, all typed FKs and ASSERT constraints).
3. Seed the metamodel (every `type_definition` row — `node_agent`, `node_run`, `attr_desc`, `edge_owns`, etc.).
4. Switch from root context (used only for schema setup) to the `superx` service account for everything that follows.
5. Bootstrap the tenant: create a substrate entity, two admin agents (`system_controller`, `gemini_cli`), five tools, and capability edges from every admin to every tool.
6. Create a `node_source_external` entity tagged with `attr_desc.text = "/path/to/temp/file"` — this is the work-item the operator wants ingested.
7. Provision a `node_run` entity (correlation id for the work).
8. Enqueue a schedule row: `kind=ingest, target=<source entity>, run=<run entity>, status=waiting`.
9. Build a `KernelDispatcher` configured with the admin agent's identity.
10. Run `RunnerBlade::tick()` in a loop with the real dispatcher.
11. On the first tick: dispatcher finds the waiting chain, runs the capability check (admin holds `tool_ingest`), calls `UniversalIngestor::ingest(FileSource{path=...})`, which walks the directory, creates ingested entities + ledger rows, returns Ok.
12. Runner transitions the schedule chain: scheduled → running → completed.
13. Loop exits when the chain reports `completed`.

**The command**:

```bash
rm -rf /tmp/superx_qa
SUPERX_DB_PATH=/tmp/superx_qa/superx.db \
cargo run -p superx-cli -- runner-demo --tenant smoke
```

**What you should see** (UUIDs will differ; the structure is what matters):

```
=== SuperX Runner Demo (tenant=`smoke`) ===
[1/5] Bootstrapping tenant…
      admin agent: entity:1e98a6a8-f9bf-57c6-ad91-061bf39ce85b
[2/5] Provisioning source entity + fixture…
      source: entity:019e4f37-ce5b-77f2-9bd6-be198506fa65
      fixture: /var/folders/hj/.../tmp...
[3/5] Enqueueing ingest schedule item…
      run: entity:019e4f37-ce5e-7273-b705-c05fd7a85404
[4/5] Ticking RunnerBlade with KernelDispatcher…
      tick 1: processed 1 chain(s)
[5/5] Final chain status: completed
=== Done ===
```

**What this proves (the test grade)**:

| If you see… | …then this works |
|---|---|
| `[1/5]` reaches without error | SurrealDB setup + schema apply + metamodel seed + bootstrap |
| `admin agent: entity:...` line | Bootstrap seeded admin agents under the tenant |
| `[2/5]` reaches without error | Entity creation under the runner's session works (typed FKs, tenant scoping) |
| `[3/5]` reaches without error | `Kernel::enqueue_schedule_item` works with typed Things |
| `tick 1: processed 1 chain(s)` | RunnerBlade picked up the `waiting` row and walked it |
| `Final chain status: completed` | Capability check passed + ingest blade ran successfully + SCD-2 transitions appended |

**Failure modes**:

| Symptom | Likely cause |
|---|---|
| Stops at `[1/5]` | DB path inaccessible, RocksDB corruption, schema-apply syntax error |
| Stops at `[2/5]` or `[3/5]` | Kernel-verb regression — investigate with `RUST_LOG=info` |
| `tick 1: processed 0 chain(s)` then exits | Schedule row didn't land or query didn't find it. Check the `due_at` clock-skew handling. |
| `Final chain status: failed` | Capability check denied OR the ingest blade itself errored. Re-run with `RUST_LOG=info,superx_kernel=debug,superx_dispatcher=debug` to see the refusal reason. |

After this command, the substrate at `/tmp/superx_qa/superx.db` is populated and inspectable. Keep it for §5.

---

## 5. Manual operator workflow (every subcommand)

Same pipeline as §4 but you drive each step yourself. Useful for testing each subcommand independently and for understanding what `runner-demo` automated.

### 5a. Bootstrap a new tenant

```bash
rm -rf /tmp/superx_qa
SUPERX_DB_PATH=/tmp/superx_qa/superx.db \
cargo run -p superx-cli -- bootstrap --tenant qa1
```

**What happens**: Same step-by-step as `runner-demo` §[1/5] — provisions a substrate entity with `attr_config` (the operational parameters), seeds admin agents, seeds 5 tools (`tool_ingest`, `tool_compile`, `tool_propose`, `tool_evaluate`, `tool_promote`), grants every admin `edge_has_capability` to every tool, and also probes the local environment for Claude Desktop / Cursor MCP configs (discovering external agents if present).

**You should see**: messages indicating each step (`Starting NASA-Grade Bootstrap`, `Provisioning Substrate Entity`, etc.), no error output.

### 5b. List bootstrap-seeded agents and tools

```bash
cargo run -p superx-cli -- list-agents --tenant qa1
cargo run -p superx-cli -- list-tools --tenant qa1
```

**Expected**:

`list-agents` — at least 2 admin agents:
- One ending in the hash for `system_controller`.
- One ending in the hash for `gemini_cli`.

If your machine has Claude Desktop / Cursor MCP configs populated, you'll also see those servers as discovered user-role agents.

`list-tools` — exactly 5 tools (any order):
- `tool_ingest`, `tool_compile`, `tool_propose`, `tool_evaluate`, `tool_promote`.

Each is a `node_tool` entity with a deterministic UUIDv5 id derived from `(substrate_uuid, tool_name)`.

### 5c. Agent identity handshake

```bash
# Pick one admin agent's entity:<uuid> from `list-agents` output.
cargo run -p superx-cli -- identify --tenant qa1 --agent-uid entity:<uuid>
```

**What this proves**: `CapabilityGovernor::handshake` works — creates a `node_session` entity linked to the agent via `edge_participates_in`, returns the session uid.

**Expected**: `Session established. session_uid: <uuidv7>`.

### 5d. Empty schedule listing

```bash
cargo run -p superx-cli -- schedule-list --tenant qa1
```

**Expected**: a header row, then `(no schedule chains in tenant `qa1`)`. Nothing's been queued yet.

### 5e. Create a target entity to enqueue work against

The runner needs a target entity to work on. We'll ingest a directory first (which creates the entity as a side effect) and then enqueue follow-on work against it.

```bash
# Create a tiny fixture dir
mkdir -p /tmp/qa_fixture
echo "Component A: handles user authentication" > /tmp/qa_fixture/component_a.txt
echo "Component B: persists session tokens"     > /tmp/qa_fixture/component_b.txt

# Ingest it — outputs the root entity id
cargo run -p superx-cli -- graphify --path /tmp/qa_fixture --tenant qa1
```

**Expected**: lines like:
```
Ingesting from: /tmp/qa_fixture (Run ID: <uuid>)
Ingestion complete. Root Entity: entity:<root-uuid>
```

**What just happened**: `UniversalIngestor::ingest(FileSource{path=...})` walked the directory, created a `node_code_root` entity for the root, child `node_code` entities for each file, set `attr_desc` ledger rows with the file contents, and emitted telemetry. Save the `Root Entity` UUID for the next step.

### 5f. Enqueue a `compile` schedule item against the root

```bash
cargo run -p superx-cli -- enqueue \
    --tenant qa1 \
    --kind compile \
    --target entity:<root-uuid>
```

**Expected**:
```
Provisioned fresh node_run: entity:<run-uuid>
Enqueued: schedule:<sid> (kind=compile, target=entity:<root-uuid>)
```

**What just happened**: the CLI provisioned a fresh `node_run` (so the schedule row's typed `run` FK has a valid target), then created a `schedule` row with `status=waiting`, `kind=compile`, `target=<root-uuid>`, `run=<run-uuid>`. **It is not yet executed** — no runner is running.

### 5g. Confirm the row landed

```bash
cargo run -p superx-cli -- schedule-list --tenant qa1
```

**Expected**: one row like:
```
RUN                            KIND         STATUS         ATT TARGET                           VALID_FROM
<run-uuid-prefix>              compile      waiting          0 <target-uuid-prefix>             2026-05-22T...
```

Status `waiting`, attempt 0. The runner has not picked it up yet.

### 5h. Run the runner against it

The runner is a long-running daemon. Start it in the foreground; it'll process the waiting row and then idle.

```bash
# You need an admin agent's entity:<uuid> for the --agent-id flag. Use one from 5b.
cargo run -p superx-cli -- runner \
    --tenant qa1 \
    --agent-id entity:<admin-agent-uuid> \
    --interval-ms 500
```

**Expected** (in ≤1 second):
```
Dispatching via KernelDispatcher (agent_id=entity:<admin-uuid>).
RunnerBlade started for tenant `qa1` (substrate <uuid>). Tick interval = 500ms. Ctrl-C to stop.
tick: processed 1 schedule chain(s)
```

After that one line, the daemon goes quiet — idle ticks don't log. Hit `Ctrl-C`.

**What just happened**: each 500ms the runner queries for `waiting` schedule rows due to run. It found ours, ran the capability check (admin holds `tool_compile`), called `CompilerBlade::compile(root, run, None)` which walked the root's entity DAG and produced an XML context blob (logged as telemetry), then transitioned the schedule chain `waiting → scheduled → running → completed`.

### 5i. Confirm completion

```bash
cargo run -p superx-cli -- schedule-list --tenant qa1
```

**Expected**: same row, but now `STATUS = completed`. Same `RUN` uuid. Append-only — the original `waiting` row is still in the substrate; `schedule-list` shows only the chain's latest state.

### 5j. Inspect the telemetry firehose

```bash
cargo run -p superx-cli -- stats --tenant qa1 --limit 30
```

**Expected**: a sequence of recent events. You should see (any order, depending on what you ran):
- `system_bootstrap` (one)
- `agent_seeded` (≥2 — system_controller, gemini_cli)
- `state_supersede` (many — every `attr_*` write)
- `edge_create` (many — capability edges, ownership edges)
- `agent_handshake` (one if you ran §5c)
- `schedule_enqueued` (one for §5f)
- `schedule_transitioned` (three for §5h: waiting→scheduled, scheduled→running, running→completed)
- `context_compile` (one — emitted by `CompilerBlade::compile`)

**What this proves**: every kernel mutation in §5a–5i emitted a typed telemetry event. The audit-trail invariant holds.

---

## 6. Failure path — capability denial

The runner's safety contract: if the agent doesn't hold capability for the requested kind, the dispatch is refused before any side effect.

```bash
# Run the runner with a bogus agent id that holds no capabilities.
cargo run -p superx-cli -- enqueue --tenant qa1 --kind compile --target entity:<root-uuid>

cargo run -p superx-cli -- runner \
    --tenant qa1 \
    --agent-id entity:00000000-0000-0000-0000-000000000000 \
    --interval-ms 500
# wait ~1 second for one tick, then Ctrl-C
```

**Expected**:
```
Dispatching via KernelDispatcher (agent_id=entity:00000000-...).
RunnerBlade started for tenant `qa1` ...
tick: processed 1 schedule chain(s)
```

`schedule-list` will now show the new chain as `STATUS = failed`. The agent didn't hold `edge_has_capability` to `tool_compile`, so the dispatch was refused inside `CapabilityGovernor::check_capability` — the failed status proves the safety contract held.

---

## 7. Direct-API blades (without the runner)

The same blades the runner dispatches through can also be called directly from the CLI. This is how SuperX worked before the runner shipped, and it's still useful for one-shot operator workflows.

```bash
# Full one-shot: bootstrap + ingest + propose + promote against a fresh tenant.
rm -rf /tmp/superx_qa
SUPERX_DB_PATH=/tmp/superx_qa/superx.db \
cargo run -p superx-cli -- demo --tenant democli
```

**What this proves**: every blade works when called directly, independent of the schedule queue. If `runner-demo` failed but this passes, the issue is in the queue / dispatcher path, not the blades themselves.

The CLI also exposes each step individually:
- `superx-cli graphify --path ... --tenant ...` — ingest only.
- `superx-cli compile --root entity:<uuid> --tenant ...` — compile only.
- `superx-cli propose --from entity:<a> --to entity:<b> --tenant ...` — LLM-driven edge proposal.
- `superx-cli evaluate --proposal entity:<p> --wasm /path/to/test.wasm --tenant ...` — wasm scoring.
- `superx-cli promote --proposal entity:<p> --threshold 0.8 --tenant ...` — promotion if score ≥ threshold.

These are not part of the runner pipeline yet (see §11), but they all work directly.

---

## 8. MCP server smoke test

`superx-mcp` exposes the same kernel verbs as MCP tools. Agent runtimes (Claude Code, Cursor) can connect over stdio and drive SuperX as a tool surface.

### 8a. Start the server

```bash
rm -rf /tmp/superx_qa
SUPERX_DB_PATH=/tmp/superx_qa/superx.db \
SUPERX_TENANT=mcptest \
cargo run --bin superx-mcp
```

**Expected**: `SuperX MCP Server starting on stdio (Tenant: mcptest)...` then the process blocks on stdin awaiting MCP protocol traffic.

In the logs you'll also see the three background tasks start:
- Telemetry subscriber (LIVE SELECT on `telemetry_stream`).
- Heartbeat pulse (one telemetry row per minute).
- Runner background loop (using `NoopDispatcher` unless `SUPERX_RUNNER_AGENT_ID` is set).

If you set `SUPERX_RUNNER_AGENT_ID=entity:<admin-uuid>` before starting, the runner uses the real `KernelDispatcher` like the CLI runner does.

### 8b. Verify dispatch surface via the integration tests

If you don't have an MCP client to hand, the integration tests exercise the same `dispatch_tool` code path. Run them:

```bash
cargo test --workspace --test core_capabilities mcp_dispatch
```

**Expected**: 4 tests pass:
- `mcp_dispatch_allows_seeded_admin_for_graphify`
- `mcp_dispatch_denies_uncapped_agent_on_graphify`
- `mcp_dispatch_rejects_missing_required_args`
- `mcp_dispatch_unknown_tool_returns_method_not_found`

These cover the allow / deny / invalid-args / method-not-found paths of the MCP tool surface.

---

## 9. Substrate integrity spot-checks

You can inspect the substrate directly with `surreal sql` against the file. This is mostly useful when something looks wrong and you want to see the raw data.

```bash
# Connect to the QA substrate.
surreal sql --endpoint surrealkv://${SUPERX_DB_PATH} --ns test --db test --auth-level root --username root --password root
```

(Adjust `surreal sql` flags to match your installed SurrealDB version; `surrealkv` is the v2.x RocksDB backend.)

### 9a. The 9 v2 tables exist

```sql
INFO FOR DB;
```

**Expected** keys in the output: `type_definition`, `entity`, `relation`, `state_ledger`, `cursor_type`, `cursor`, `execution_params`, `schedule`, `telemetry_stream`. Each is `SCHEMAFULL` (except `state_ledger` and `telemetry_stream` which are `SCHEMALESS` for the value-blob field).

### 9b. Every entity has typed FKs to the metamodel

```sql
SELECT id, type.uid, tenant.type.uid AS tenant_type FROM entity LIMIT 10;
```

**Expected**: every row has a `type.uid` (a string like `node_agent`, `node_tool`, `node_substrate`) and a `tenant_type = 'node_substrate'` — *except* the substrate-root row itself, where `tenant` is `NONE` (that's the only entity allowed `NONE`; the ASSERT clause enforces this).

### 9c. The append-only schedule invariant

```sql
SELECT count() FROM schedule GROUP ALL;
-- Should match: 1 (initial waiting) + 3 (waiting→scheduled→running→completed) = 4 per chain you ran.
```

```sql
SELECT status, valid_from FROM schedule
WHERE run = entity:<run-uuid>
ORDER BY valid_from ASC;
-- Should show: waiting, scheduled, running, completed (4 rows).
```

If you see fewer than 4 rows for a chain you walked all the way through, history has been destroyed — that's a hard regression.

### 9d. Telemetry firehose

```sql
SELECT lifecycle_event, valid_from FROM telemetry_stream
ORDER BY valid_from DESC LIMIT 20;
```

**Expected**: a mix of `system_bootstrap`, `agent_seeded`, `state_supersede`, `edge_create`, `schedule_enqueued`, `schedule_transitioned`, `context_compile`, etc.

---

## 10. Test the test suite (meta-check)

The full test surface exists for regression protection. Run targeted suites to verify each:

```bash
# Kernel-verb correctness invariants (NASA-rule loops, tenant isolation, SCD-2).
cargo test -p superx-kernel

# RunnerBlade tick loop alone (no real dispatcher).
cargo test -p superx-runner

# End-to-end runner pipeline + capability denial.
cargo test -p superx-cli --test runner_e2e

# SCD-2 invariants on schedule.
cargo test -p superx-cli --test schedule_scd2

# Wide coverage of v0.1 surface (46 tests).
cargo test -p superx-cli --test core_capabilities

# Cognitive-loop end-to-end (bootstrap → ingest → propose → promote).
cargo test -p superx-cli --test integration
```

Each suite must pass cleanly. Any failure means a regression in the corresponding subsystem.

---

## 11. Known non-capabilities (deferred by design, not bugs)

If you go looking for these, you won't find them — by design, not because they're broken. The reasoning is documented in `ARCHITECTURE.md §0c-2`.

| Gap | What it means | Why it's deferred | When it lands |
|---|---|---|---|
| Runner doesn't dispatch `kind=propose` | You can enqueue `kind=propose`, but the dispatcher returns a `Validation` error and the chain lands `failed` | No autonomous proposer-blade caller exists yet (`EdgeProposerBlade`, `DesignerBlade`). Wiring this without a real caller means designing schema speculatively. | Same PR as `EdgeProposerBlade` (roadmap #25) |
| Runner doesn't dispatch `kind=evaluate` | Same shape | No autonomous Meta-Harness scoring loop yet | Same PR as the autonomous Meta-Harness loop |
| Runner doesn't refuse uncompiled targets | `attr_compile_status='compiled'` is not checked before dispatch | No `DesignerBlade` producing compiled DAGs yet | Same PR as `DesignerBlade` (roadmap #3) |
| Runner doesn't walk a DAG via `petgraph::toposort` | The dispatcher operates on one entity at a time | Same dependency on `DesignerBlade` | Same PR |
| Engine doesn't refuse UPDATE/DELETE under `superx` | Table `PERMISSIONS FOR update NONE; FOR delete NONE;` clauses are dead code under `DEFINE USER ROLES EDITOR` (system users bypass them in SurrealDB v2.x) | Deliberate choice to avoid maintaining a custom permissions/identity system (would require `DEFINE ACCESS TYPE RECORD`). Append-only enforced by kernel-verb discipline only. | Not on roadmap — see skill `§10 / §13` |
| Real LLM autonomy inside the dispatcher | `ProposerBlade` and `MetaHarness` call the local model directly; the autonomous routing of LLM calls (continuous loops, Rig.rs adapter) isn't built | Roadmap #16 (Rig.rs adoption) + the autonomous-blade roadmap items above | Multi-PR sequence |

So if a QA tester finds, for example, `enqueue --kind propose` results in `Final chain status: failed`, **that's expected** — read this section and verify the failure reason matches.

---

## 12. If something breaks

The triage order:

1. **Re-run with logging.** `RUST_LOG=info,superx_kernel=debug,superx_runner=debug,superx_dispatcher=debug,superx_agent=debug` gives you verb-by-verb traces.
2. **Inspect telemetry directly.** Every kernel mutation logs an event. Engine refusals (typed ASSERT violations, PERMISSIONS denials for record-bound sessions) also surface there. Use `superx-cli stats --tenant X --limit 100`.
3. **Re-run the related test in isolation.** `cargo test --workspace -- <failing-test-name>` lets you reproduce off-CLI.
4. **Check the substrate directly via `surreal sql`** (§9 commands).
5. **Read the §12 contract in the skill** (`.claude/skills/zero-trust-execution/SKILL.md`): engine refusals are debugging information. The fix is to change the code that issued the refused operation, never to bypass the constraint.

---

## 13. Sign-off checklist

If everything above passes, the v0.1 surface is functioning. The signal:

- [ ] §2 — `cargo build --workspace` finishes cleanly.
- [ ] §3 — `cargo test --workspace` reports 64/64 passed, 0 failed; clippy is clean.
- [ ] §4 — `runner-demo` reports `Final chain status: completed`.
- [ ] §5 — Manual workflow lands a `compile` schedule chain in `completed` end-to-end, with the right telemetry events visible in `stats`.
- [ ] §6 — Capability denial lands a bogus-agent chain in `failed`.
- [ ] §7 — `demo` direct-API path completes without errors.
- [ ] §8 — MCP server starts cleanly, `mcp_dispatch` tests pass.
- [ ] §9 — Substrate inspection reveals the expected table set, typed FKs, append-only chain, and telemetry events.
- [ ] §10 — Targeted test suites all pass when run individually.
- [ ] §11 — None of the documented non-capabilities surfaced as bugs (they failed as expected, with the documented rationale).

That's the floor. Anything above and beyond is exploration territory.
