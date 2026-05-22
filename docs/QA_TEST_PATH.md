# SuperX v0.1 — QA Test Path

A scan-able operator guide for everything that's shipped on `main` today. Run through it in order; each step builds on the previous one's outputs. Total time: 30–45 min if everything goes well.

## 0. Prerequisites

```bash
# Rust toolchain (1.81+ recommended).
rustup show
cargo --version

# Build (always use `cargo build`, NOT a release-stripped artefact).
cargo build --workspace

# Pick a fresh substrate path for QA runs (DESTROYED between test sessions).
export SUPERX_DB_PATH=/tmp/superx_qa/superx.db
rm -rf /tmp/superx_qa
```

**Expected**: `cargo build --workspace` finishes with no errors, 12 crates compiled.

---

## 1. Mandate-5 gates (the green-bar baseline)

Before anything else, prove the workspace is internally consistent.

```bash
# Test gate
cargo test --workspace

# Linter gate
cargo clippy --all-targets --all-features -- -D warnings
```

**Expected**:
- `cargo test --workspace`: **64/64 passed, 0 failed** across these test targets:
  - `superx-kernel` lib unit tests: 7
  - `superx-runner` lib unit tests: 4
  - `superx-cli` integration `core_capabilities`: 46
  - `superx-cli` integration `integration`: 1
  - `superx-cli` integration `runner_e2e`: 2
  - `superx-cli` integration `schedule_scd2`: 4
- Clippy: `Finished ... target(s)` with no warnings.

**Fail signal**: any test failure, any clippy warning. Stop here and investigate.

---

## 2. End-to-end runner pipeline (the FVP smoke test)

The single command that demonstrates everything works together — bootstrap, capability check, schedule queue, runner, dispatcher, blade execution, SCD-2 transitions.

```bash
rm -rf /tmp/superx_qa
cargo run -p superx-cli -- runner-demo --tenant smoke
```

**Expected output** (UUIDs will differ):

```
=== SuperX Runner Demo (tenant=`smoke`) ===
[1/5] Bootstrapping tenant…
      admin agent: entity:<uuidv5>
[2/5] Provisioning source entity + fixture…
      source: entity:<uuidv7>
      fixture: /var/folders/.../tmp...
[3/5] Enqueueing ingest schedule item…
      run: entity:<uuidv7>
[4/5] Ticking RunnerBlade with KernelDispatcher…
      tick 1: processed 1 chain(s)
[5/5] Final chain status: completed
=== Done ===
```

**What this proved**:
- ✅ SurrealDB substrate provisioned + 9 tables initialised.
- ✅ Bootstrap seeded 2 admin agents + 5 tools + 10 capability edges.
- ✅ A `node_source_external` entity was created and tagged with `attr_desc.text`.
- ✅ A `schedule` row was enqueued with `kind=ingest`.
- ✅ `RunnerBlade::tick()` picked it up.
- ✅ `CapabilityGovernor::check_capability(agent, "tool_ingest")` passed.
- ✅ `UniversalIngestor::ingest()` ran the actual blade.
- ✅ Schedule chain walked `waiting → scheduled → running → completed`.

**Fail signal**: stops before `[5/5]`, or reports `Final chain status: failed`. Re-run with `RUST_LOG=info` for diagnostic output.

---

## 3. Manual operator workflow (step-by-step CLI)

Same pipeline as #2 but driven by the operator instead of the demo helper. Demonstrates each subcommand independently.

### 3a. Bootstrap a tenant

```bash
rm -rf /tmp/superx_qa
cargo run -p superx-cli -- bootstrap --tenant qa1
```

**Expected**: prints `Bootstrap complete` (or similar) with a substrate UUID.

### 3b. List bootstrap-seeded agents and tools

```bash
cargo run -p superx-cli -- list-agents --tenant qa1
cargo run -p superx-cli -- list-tools --tenant qa1
```

**Expected**:
- `list-agents`: at least 2 admin agents (`system_controller`, `gemini_cli`); also any Claude Desktop / Cursor MCP servers if you have `~/Library/Application Support/Claude/claude_desktop_config.json` populated.
- `list-tools`: exactly 5 tools (`tool_compile`, `tool_evaluate`, `tool_ingest`, `tool_promote`, `tool_propose`).

### 3c. Agent identity check

Pick an `entity:<uuid>` from `list-agents` (the admin agents only).

```bash
cargo run -p superx-cli -- identify --tenant qa1 --agent-uid entity:<uuid>
```

**Expected**: `Session established. session_uid: <uuidv7>` (the `entity:<uuid>` portion).

### 3d. Schedule listing (empty)

```bash
cargo run -p superx-cli -- schedule-list --tenant qa1
```

**Expected**: header row + `(no schedule chains in tenant `qa1`)`.

### 3e. Enqueue a schedule item

You'll need a target entity. The simplest is to create one with `graphify` first (which creates a `node_code_root` entity), then enqueue a `compile` schedule for it.

```bash
# Make a temp directory the ingestor will walk
mkdir -p /tmp/qa_fixture
echo "QA payload" > /tmp/qa_fixture/note.txt

# Ingest — outputs the root entity id
cargo run -p superx-cli -- graphify --path /tmp/qa_fixture --tenant qa1
# → "Ingestion complete. Root Entity: entity:<root-uuid>"

# Enqueue a compile schedule against the root
cargo run -p superx-cli -- enqueue \
    --tenant qa1 \
    --kind compile \
    --target entity:<root-uuid>
# → "Provisioned fresh node_run: entity:<run-uuid>"
# → "Enqueued: schedule:<sid> (kind=compile, target=entity:<root-uuid>)"
```

### 3f. Verify the enqueue landed

```bash
cargo run -p superx-cli -- schedule-list --tenant qa1
```

**Expected**: one row showing `kind=compile`, `status=waiting`, `attempt=0`.

### 3g. Run the runner against it

Get an admin agent id (see 3b output), then:

```bash
cargo run -p superx-cli -- runner \
    --tenant qa1 \
    --agent-id entity:<admin-agent-uuid> \
    --interval-ms 500
```

**Expected**:
- Startup line: `RunnerBlade started for tenant ...`
- Within ~1 second: `tick: processed 1 schedule chain(s)`.
- Idle ticks stay silent.

Hit `Ctrl-C` after you see the first processed line. Then:

```bash
cargo run -p superx-cli -- schedule-list --tenant qa1
```

**Expected**: the same chain is now `status=completed`.

---

## 4. Schedule SCD-2 invariants (sanity-check the substrate)

Direct-substrate inspection that history is append-only. These commands use the same `qa1` substrate from §3.

### 4a. Count rows in the schedule chain

Walk the same `run` you enqueued under (the one printed by `enqueue`). For each transition, a new row was appended; total should be 4 for a `waiting → scheduled → running → completed` chain.

In a Rust REPL or via the SurrealDB CLI, hitting the same DB file:

```sql
SELECT count() FROM schedule WHERE run = entity:<run-uuid> GROUP ALL;
-- → 4
```

### 4b. Confirm all four rows are still there

```sql
SELECT status, valid_from FROM schedule
WHERE run = entity:<run-uuid>
ORDER BY valid_from ASC;
-- → waiting, scheduled, running, completed (4 rows)
```

**Fail signal**: fewer than 4 rows means a verb destroyed history.

These invariants are also covered automatically by the test suite:
`cargo test --workspace --test schedule_scd2`.

---

## 5. Direct-API blade paths (unchanged from v0.1 baseline)

Each of these calls the blade directly without going through the schedule queue. Useful for operator-driven workflows and as building blocks for the autonomous loops we haven't built yet.

```bash
# 5a. Full one-shot demo (direct API, not via runner)
rm -rf /tmp/superx_qa
cargo run -p superx-cli -- demo --tenant democli

# 5b. Stats — last N telemetry events
cargo run -p superx-cli -- stats --tenant democli --limit 20
```

**Expected**:
- `demo`: completes with messages for bootstrap → ingest → propose → promote.
- `stats`: prints recent `system_bootstrap`, `agent_seeded`, `state_supersede`, `edge_create`, `relation_proposed`, `agent_handshake`, etc. events.

**Note on `propose` / `evaluate`**: these blades work via direct CLI but are intentionally **not** wired into the runner dispatcher in v0.1. See `ARCHITECTURE.md §0c-2` for why (autonomous callers don't exist yet).

---

## 6. MCP server (substrate-as-MCP-tool surface)

The MCP server exposes the same kernel verbs as MCP tools so agent runtimes (Claude Code, Cursor) can drive SuperX.

### 6a. Start the server in one terminal

```bash
rm -rf /tmp/superx_qa
SUPERX_TENANT=mcptest cargo run --bin superx-mcp
```

**Expected**: `SuperX MCP Server starting on stdio (Tenant: mcptest)...` then it blocks on stdin awaiting MCP protocol messages. Background tasks log telemetry-subscriber + pulse + runner activity.

If you set `SUPERX_RUNNER_AGENT_ID=entity:<admin-uuid>`, the background runner uses `KernelDispatcher` (capability-checked). Unset, it uses `NoopDispatcher` (loop-only).

### 6b. Issue a tool call (manual test)

In another terminal, with the server attached to a real MCP client, the standard tool surface (`graphify`, `compile`, `evaluate`, `promote`, `identify`) should be callable. Each goes through `dispatch_tool` which sets session auth, capability-checks, dispatches.

If you don't have an MCP client handy, this is harder to QA from the CLI alone. The integration tests in `crates/superx-cli/tests/core_capabilities.rs::mcp_dispatch_*` exercise the same code path; running them is the substitute.

```bash
cargo test --workspace --test core_capabilities mcp_dispatch
```

**Expected**: 4 passing tests covering allow/deny/missing-args/unknown-tool paths.

---

## 7. Substrate integrity spot-checks

Quick checks against a substrate from any earlier step.

### 7a. The 9 v2 tables exist and are schema-valid

```sql
INFO FOR DB;
```

**Expected**: `type_definition`, `entity`, `relation`, `state_ledger`, `cursor_type`, `cursor`, `execution_params`, `schedule`, `telemetry_stream`.

### 7b. Every entity has typed FKs

```sql
SELECT id, type.uid, tenant.type.uid FROM entity LIMIT 10;
```

**Expected**: every row has a `type.uid` (e.g. `node_substrate`, `node_agent`, `node_prod`) and a `tenant.type.uid = 'node_substrate'` (except the substrate-root row itself, where `tenant` is `NONE`).

### 7c. Telemetry firehose has events

```sql
SELECT lifecycle_event, valid_from FROM telemetry_stream
ORDER BY valid_from DESC LIMIT 20;
```

**Expected**: `system_bootstrap`, `agent_seeded`, `state_supersede`, `edge_create`, `schedule_enqueued`, `schedule_transitioned`, `context_compile`, `relation_proposed`, etc. — depending on what you exercised above.

---

## 8. Known non-capabilities (deferred, by design)

These are documented gaps you'll find if you go looking:

| Gap | Why | When it lands |
| --- | --- | --- |
| Runner doesn't dispatch `kind=propose` | No autonomous proposer-blade caller exists yet | Same PR as `EdgeProposerBlade` (#25 roadmap) |
| Runner doesn't dispatch `kind=evaluate` | No autonomous MetaHarness scoring loop yet | Same PR as that loop |
| `attr_compile_status='compiled'` refusal in Runner | No DesignerBlade producing compiled DAGs yet | Same PR as `DesignerBlade` (#3 roadmap) |
| DAG-walking via `petgraph::toposort` | Same dependency on DesignerBlade | Same PR |
| Engine-level UPDATE/DELETE refusal under `superx` | `DEFINE USER ROLES EDITOR` bypasses table `PERMISSIONS`; deliberate choice to avoid maintaining a custom permissions system | Not on roadmap; append-only enforced by kernel-verb discipline (see skill §10 / §13) |
| Real LLM integration (Gemma/Rig.rs) inside the dispatcher | `ProposerBlade` calls the local model directly today; the autonomous routing of LLM calls needs Rig.rs adoption | Roadmap #16 |

---

## 9. If something breaks

1. Re-run with `RUST_LOG=info,superx_kernel=debug` to see verb traces.
2. Inspect `state_ledger` and `telemetry_stream` directly — every kernel write logs an event. Engine refusals also surface there.
3. `cargo test --workspace -- <failing-test-name>` to isolate.
4. The §12 contract: engine refusals are debugging information, not failures to bypass. Reproduce the refusal, then fix the code that issued the rejected operation.

## 10. Sign-off checklist

If everything above passes, the v0.1 surface is functioning. The signal is:

- [ ] §1 gates green (64/64 tests, clippy clean).
- [ ] §2 `runner-demo` reports `Final chain status: completed`.
- [ ] §3 manual workflow lands a chain in `completed` end-to-end.
- [ ] §4 substrate has the expected 4-row SCD-2 chain for that workflow.
- [ ] §5 `demo` completes without errors.
- [ ] §6 MCP server starts cleanly + `mcp_dispatch` tests pass.
- [ ] §7 substrate spot-checks reveal no schema drift or missing telemetry.

That's the floor. Anything above and beyond is exploration territory.
