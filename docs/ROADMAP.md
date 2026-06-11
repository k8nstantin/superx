# SuperX Roadmap — F0 → FVP and Beyond

> **Status (2026-06-11):** phases **F1–F11 are merged** (PRs #97–#106) —
> **the First Viable Product is complete**; the test protocol lives in
> [`README.md`](../README.md). This document tracks the post-FVP plan and
> the quality backlog from the FVP readiness review. It supersedes the
> roadmap sections of `ARCHITECTURE.md` (v42.15), which describes the
> pre-2026-05-23 system and is retained as a historical vision document.
> Schema truth lives in [`SUPERX_SCHEMA.md`](../SUPERX_SCHEMA.md)
> and [`schema/kernel.surql`](../schema/kernel.surql).

## Terminology

The four-layer architecture (locked 2026-05-23):

| Layer | Name | Crate pattern | Examples |
|---|---|---|---|
| L0 | atomic kernel | `superx-kernel` | substrate verbs, telemetry primitive, registry, lifecycle |
| L1 | kernel modules | `superx-kernel-<feature>` | bootstrap, discovery, capture, parameters (future) |
| L2 | drivers | `superx-driver-<name>` | claude-code, claude-desktop (future) |
| L3 | apps | `superx-<name>` | cli, mcp (future), gmaster (future) |

The term **"blade"** (used throughout the historical `ARCHITECTURE.md`) is
**retired**. Long-running background workers are **kernel modules**; specific
integrations are **drivers**; operator/agent-facing capabilities are **apps**.
Do not reintroduce "blade" in new code or docs.

## FVP definition (the shipping bar)

> The operator runs `superx kernel bootstrap`, opens Claude Code, and watches
> live activity stream in `superx kernel stats --live`.

Kernel + bootstrap + agent discovery + continuous telemetry capture loop +
stats. Everything else is post-FVP.

## Why the prior implied sequencing was revised

1. It built the bootstrap orchestrator on unverified verbs — `registry.rs`
   and `lifecycle.rs` shipped in F0 with zero test coverage.
2. Stale top-level docs (`ARCHITECTURE.md` claiming "44/44 tests passing" for
   a deleted 12-crate system) misled every future session.
3. It deferred kernel primitives the FVP path needs (parameter verbs, cursor
   verbs), which would have forced bloated mid-stream PRs.
4. It implicitly bet `stats --live` on LIVE SELECT, which diverges between
   the kv-mem engine (tests) and protocol-ws (production).
5. The lifecycle deserializer silently mapped unknown state tags to
   `Enabled`, letting a boot orchestrator misread corrupt state as healthy.

## Standing decisions

- **D1 — Parameter verbs in kernel core.** `set_parameter` / `get_parameter`
  land in `superx-kernel` (~120 LOC composing `ensure_type_definition` +
  `supersede_state` + `current_state`). The canon lists them in the
  kernel-core API; the reserved `superx-kernel-parameters` L1 crate is the
  post-FVP *framework* (listing, `sch_json` validation, CLI surface).
- **D2 — Cursor verbs in kernel core.** `ensure_cursor_type`, `write_cursor`,
  `latest_cursor` — verbs over locked kernel tables are the L0 storage
  pillar. No schema change required.
- **D3 — First driver is Claude Code.** Transcript JSONL under
  `~/.claude/projects` is an event stream — the firehose. Claude Desktop's
  config probe is static metadata with no stream; it ships post-FVP to prove
  the framework generalizes.
- **D4 — Test backfill is its own PR.** Registry/lifecycle tests + the
  silent-`Enabled` fix land together, before anything consumes those verbs.
- **D5 — FVP `stats --live` is poll-based.** Identical operator experience,
  testable on kv-mem. LIVE SELECT is a post-FVP upgrade with its own
  ws-gated integration test and poll fallback.
- **D6 — `superx kernel bootstrap` runs foreground for FVP.** Provision +
  seed + discover + start capture, then block until ctrl-c (graceful
  shutdown walk). Daemonization is post-FVP. *(Operator to confirm before F10.)*

## Phases

Each phase = one branch, one PR, < 1 day, green at the Mandate-5 gate:
`cargo test --workspace` + `cargo clippy --workspace --all-targets
--all-features -- -D warnings` + `python3 tools/skill_audit.py`.

| # | PR | Scope | Crates |
|---|---|---|---|
| F1 | docs: roadmap + stale-doc banners | This document; historical banners on `ARCHITECTURE.md` / `REMEDIATION.md`; truthful `README.md` status | — |
| F2 | kernel: test backfill + lifecycle hardening | New `tests/registry.rs` + `tests/lifecycle.rs` (register/idempotent-reregister, descriptor supersession, `list_with_status`, `detailed_status`, all four `mark_*` verbs incl. telemetry emission); fix `lifecycle.rs` unknown-tag fallback: error, never silent `Enabled` | superx-kernel |
| F3 | kernel: parameter verbs | `set_parameter` / `get_parameter` per D1 + kernel-singleton entity convention for global parameters; tests: roundtrip, supersession (latest wins), unset → `None`, history preserved | superx-kernel |
| F4 | kernel: cursor verbs | Per D2; tests: ensure-idempotent, write/read roundtrip, latest-wins, per-(subject, cursor_type) isolation; rustdoc alignment note (11-type metamodel supersedes the canon's 8-type list) | superx-kernel |
| F6 | new crate: `superx-kernel-bootstrap` | Boot orchestrator: seed `REQUIRED_METAMODEL_TYPES` + per-module metamodel; topo-sort `KERNEL_MODULES` by `depends_on` (cycle → `Failed`, rest continue); `mark_starting → startup() → mark_active/mark_failed`; dependents of failures `Skipped`; boot always continues; returns structured `BootReport`. Tests: fake modules via linkme in the test binary (succeed / fail / depend-on-failed / disabled / cycle) | +bootstrap |
| F7 | new crate: `superx-cli` | `superx kernel bootstrap` / `superx kernel modules list` / `superx kernel stats [-n N]`; clap; `SUPERX_ENDPOINT` env (default `ws://127.0.0.1:8000`). The CLI is itself an app-layer module | +cli |
| F8 | new crate: `superx-kernel-discovery` | `DiscoveryProbe` trait + `DISCOVERY_PROBES` distributed slice; `startup()` iterates probes, idempotently creates `node_agent` / `node_source` entities + relation, emits `agent_discovered` / `source_discovered` telemetry; per-probe failure isolation | +discovery |
| F9 | new crate: `superx-driver-claude-code` | Registers in `KERNEL_MODULES` (category `"driver"`) + `DISCOVERY_PROBES`; probes `~/.claude/projects` via `attr_claude_code_projects_root` parameter (tests inject fixture paths via `set_parameter` — no env hacks); one agent + one `node_source` per project transcript dir | +driver |
| F10 | new crate: `superx-kernel-capture` + watcher | `CaptureSource` trait + poll loop (interval = parameter, default 2 s): `latest_cursor → poll → log_telemetry per event → write_cursor`; poll errors become `capture_error` telemetry, never panics; tolerant JSONL parsing (unknown shapes → `transcript_raw` events); `capture_tick()` exposed for timer-free tests; bootstrap now blocks foreground while capture runs (D6) | +capture, driver, cli |
| F11 | cli: `stats --live` (poll) + `agents` — **FVP complete** | Rolling poll of `recent_telemetry` newer-than-last-seen, interval = parameter (default 1 s); `--module <name>` firehose filter; `agents` lists discovered agents with source counts; manual FVP demo protocol in the PR description | cli (+ tiny kernel verb if needed) |

*(F5 — standalone rustdoc-alignment PR — was folded into F4.)*

### Capture-loop test strategy (F10)

Fixture transcript dirs copied to a tempdir; the projects-root parameter
pointed at the tempdir via `set_parameter`; one explicit `capture_tick()` →
assert telemetry rows + cursor row; append lines to the fixture → second
tick → assert only-new events (cursor-resume proof); malformed line →
`transcript_raw` / `capture_error` event with the loop still alive. The
timer-based loop gets one smoke test with a short interval and a
cancellation assert.

## Post-FVP (sequenced)

1. **F12** — LIVE SELECT upgrade for `--live`: ws-only fast path with poll
   fallback; integration test gated behind `SUPERX_WS_TEST_ENDPOINT`.
2. **F13** — `superx-driver-claude-desktop`: config-presence probe; proves
   the discovery framework generalizes (two drivers, zero framework edits).
3. **F14** — `superx-kernel-parameters` framework crate (listing, `sch_json`
   validation, `superx kernel params list|get|set`).
4. **F15** — DAG compile step (JSON-canonical singleton + graph derivative +
   CID hashing, per the locked canon).

## Quality backlog — FVP readiness review (2026-06-11)

Findings from the 14-agent adversarial review that shipped F11's
demo-critical fixes; everything below is verified-real but deferred.
Severity is the review's verdict.

**Needs an operator-approved schema PR (§7):**
- 1 h `DURATION FOR SESSION/TOKEN` on `superx_kernel` kills both
  long-running FVP processes after an hour (minor → operator decision).
- No UNIQUE indexes backing the `ensure_*` / `register_module`
  find-then-create idempotency — cross-process races can fork SCD-2
  chains (minor).
- No index on `telemetry_stream.valid_from` — every poll full-scans a
  growing table (note; measure first).

**Capture / discovery robustness (code, post-FVP PRs):**
- Discovery is one-shot at bootstrap; project dirs created later are
  never discovered (major for UX; continuous re-probing).
- Per-process `STOP` static: re-bootstrap in one process leaks a
  duplicate capture loop; shutdown affects all instances (minor).
- Checkpoint-after-emit = at-least-once delivery; a mid-poll failure
  re-emits already-captured lines next tick (minor).
- Equal-or-longer file replacement is not detected (only shrinkage
  resets the offset); cursor `files` metadata never pruned (minor).
- Probe panics are not per-probe isolated; one failing probe marks the
  whole discovery module Failed — per-probe lifecycle granularity
  (minor). Persistent per-tick `capture_error` has no backoff (note).
- Disabled module on a dependency cycle is marked Failed, overriding
  operator intent; skip reason says "unknown module" for
  failed-registration deps (minor).

**Kernel / CLI quality (code, post-FVP PRs):**
- `find_entity_by_name` matches the latest row *that ever carried the
  name* — a renamed entity's old name resolves forever; resolve per
  target's current descriptor instead (minor).
- Every CLI invocation emits a `system_boot` row — readers mutate the
  stream they observe (minor).
- `stats` payload rendering is externally-tagged serde JSON — noisy;
  human rendering pass (minor). `--module` is a raw substring match
  (note). Dead `KernelError::Json` variant (note).
- The entire ws wire path is untested in CI — F12's
  `SUPERX_WS_TEST_ENDPOINT` environment is the fix (note).

**Deferred with no dates:** emission sinks (Kafka/HTTP/OTLP), model router +
providers, scheduler / runner / harness, MCP app, gmaster, permissions
framework, A2A comm, vector / cache / secrets, daemonization,
`schedule` / `execution_params` verbs (no consumer until the scheduler).

## Risk register

| # | Risk | Mitigation |
|---|---|---|
| R1 | LIVE SELECT ws-vs-mem divergence breaks `stats --live` at demo time | FVP polls (D5); LIVE SELECT isolated in F12 with a real-ws gated test + fallback |
| R2 | Claude Code transcript JSONL format drift crashes or starves capture | Tolerant `Value`-based parsing, unknown → `transcript_raw`, errors → telemetry not panics, versioned fixtures, uuid-based cursors where possible |
| R3 | Doc/canon drift compounds and misleads future sessions | F1 banners now; each PR that refines the canon adds a one-line "canon delta" note in its description |
| R4 | Append-only query cost on ever-growing `state_ledger` / `telemetry_stream` once the firehose runs | Cache boot-time lookups in the boot report; measure during F10; indexes only via an operator-approved schema PR if measurements demand it |
| R5 | Crate / scope explosion ("while I'm here" gold-plating) | A crate is created only in the PR that ships its behavior; FVP capped at 5 new crates; anything beyond the FVP sentence goes to the post-FVP list |

## Operator decisions (flagged, not acted on)

1. **Credential naming drift** — the zero-trust skill §13 says user `superx`
   / `SUPERX_SERVICE_PASSWORD`; the deployed schema + kernel code say
   `superx_kernel` / `SUPERX_KERNEL_PASSWORD`. Recommendation: amend the
   skill to match the deployed schema. Operator-owned file.
2. **Full `ARCHITECTURE.md` rewrite** — F1 only banners it as historical.
   The rewrite (and any scrub of the retired "blade" terminology there and
   in the skill) is operator-owned.
3. **Canon memory updates** — the locked canon's §12 metamodel list (8 types
   incl. `node_driver` / `node_app`) is superseded by F0's shipped 11 types
   with `node_contribution`; the D1 parameters clarification should also be
   recorded. Memory-file edits, operator's call.
4. **D6 foreground-bootstrap semantics** — shipped as designed in F10
   (foreground hold until ctrl-c); retro-confirm or amend.
5. **Schema amendment candidates** from the 2026-06-11 readiness review
   (each its own operator-approved schema PR per §7): session/token
   `DURATION` longer than 1 h; `DEFINE INDEX` UNIQUE on
   `type_definition.uid` / `cursor_type.uid`; index on
   `telemetry_stream.valid_from`.
