---
name: zero-trust-execution
description: SuperX project operating mode — invoke (or treat as always-on) for any work in the SuperX codebase. Enforces no shortcuts, mandatory documentation research, fidelity to BLUEPRINT.md, a stop-and-ask protocol, and verification-as-truth (cargo test + cargo clippy -- -D warnings + skill_audit.py).
---

<instructions>
You are operating under **Zero-Trust Execution Mode** for SuperX — the
agentic operating system (reset of 2026-08-06; canon: `BLUEPRINT.md`).
These mandates are binding. The section numbers are load-bearing: CI
(`.github/workflows/skill-audit.yml`) and `tools/skill_audit.py`
reference them by number.

### §1 The Anti-Velocity Mandate
- NEVER optimize for speed. No workarounds, hacks, or quick fixes to
  get past a compiler, database, or runtime error.
- If you hit an error you do not fully understand: **STOP**. Research
  it or ask. Never paper over it.

### §2 The Research Imperative
- Every technical decision (library configuration, data-type handling,
  error semantics) is grounded in official documentation — SurrealDB,
  Tokio, clap, serde — fetched and read, not recalled.
- Never guess a function signature. Never hallucinate a workaround.
  Implement exactly what the library authors specify, in idiomatic Rust.

### §3 Architectural Fidelity
- `BLUEPRINT.md` and the operator's explicit instructions are absolute
  law. You have ZERO authority to alter the architecture, the kernel's
  five responsibilities, the module/adapter seams, or core patterns
  (append-only substrate, SCD-2, cursor checkpointing) without explicit
  prior operator permission.
- The kernel boundary is part of the architecture: the kernel owns
  substrate access, boot, the telemetry capture engine, the CLI, and
  the module system — and knows nothing about any module's domain.
  Agent-format knowledge lives only in adapters. Module or adapter
  logic leaking into the engine is an architecture change and requires
  operator sign-off.
- If an implementation detail seems to conflict with the blueprint:
  **STOP AND ASK.** Do not invent a path forward.

### §4 The Stop-and-Ask Protocol
Halt and ask the operator when any of these holds:
1. An error prevents the workspace from compiling or tests from passing
   and the fix is not evident.
2. Official documentation contradicts your understanding.
3. You are tempted to write a workaround because the correct way seems
   too difficult or slow.
4. You realize you are acting on an assumption rather than a verified
   fact.

### §5 Verification Is the Only Truth
- Code does not exist until `cargo test --workspace` passes.
- Code is not clean until
  `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  passes.
- The tree is not compliant until `python3 tools/skill_audit.py` passes.
- You are forbidden from declaring a task complete, or updating
  `BLUEPRINT.md` status, until all three gates are green.

### §6 Trunk-Based Development (mandatory workflow)
- Single trunk `main`, always releasable. Every commit on `main` passes
  the §5 gates. Releases are tagged from `main`.
- All work happens on a topic branch (`feat/`, `fix/`, `chore/`,
  `docs/`); every change ships as a PR against `main`. Never commit to
  `main` directly; never force-push `main`.
- Branches are short-lived (hours, never overnight). One open branch +
  one open PR at a time — sequential, never parallel.
- Each branch is atomic: it lands fully working (gates green,
  feature operational) or it does not land. Unfinished work hides
  behind a substrate parameter flag, never as broken code on `main`.
- One logical step per PR — small diffs. If two changes can be
  reasoned about independently, they are two PRs. Load-bearing
  refactors use branch-by-abstraction: seam first, migrate second,
  remove third — each PR green.
- Pre-flight before any branch: is the change self-contained; is the
  path to green gates visible; does the diff fit one mental model; is
  a flag needed; can you state the PR description in 2–3 sentences?
- Banned: mega-branches, speculative branches, long-lived branches,
  force-merging red gates, mid-branch side-quests.
- Merged branches are deleted, locally and on origin.

### §7 Schema Immutability — STOP. ASK. THEN MAYBE.
- The substrate schema (`SUPERX_SCHEMA.md` and every `schema/*.surql`
  file, once G1 lands them) is load-bearing architecture. Every table,
  field, index, and permission is a contract.
- You have ZERO authority to change schema-defining files without
  explicit, prior, **per-change** operator permission. "Refactor,"
  "cleanup," "small change," and "the prior PR was wrong" are not
  authorization. Authorization for one change does not extend to
  adjacent changes.
- CI enforces this: any PR touching schema-defining files or
  `.claude/skills/` fails unless the PR body carries an
  `Operator-approved: …` marker.
- If you believe a shipped schema is wrong: STOP, surface it, propose
  the fix, and wait.

### §8 The Kernel Mental Model
Layers, and what may know what:
- **Substrate (SurrealDB)** — the OS's filesystem. Dumb, honest,
  append-only facts: entities, state, telemetry, cursors, parameters,
  the module registry. If it isn't in the substrate, the OS doesn't
  know it.
- **Kernel** — substrate verbs, boot, the capture engine, CLI, module
  lifecycle. Generic over agents and modules.
- **Adapters** — per-agent format knowledge only (where an agent's
  telemetry lives and how to parse it). An adapter never touches the
  engine's internals; the engine never names a specific agent.
- **Modules** — every capability that is not the kernel (data fusion,
  graphify, …). A module consumes kernel verbs; the kernel never
  consumes a module.
Putting logic in the wrong layer is a §3 violation.

### §9 No Hardcoded Policy
- Every tunable — poll intervals, limits, paths, thresholds, defaults —
  flows through substrate parameters. No magic numbers or magic paths
  in code. Tests inject fixtures via parameters, not env vars.
- A constant may be exempted only with an explicit audited
  `// skill-allow: §9-…` marker that `tools/skill_audit.py` accepts.
- Judgments (rankings, priorities, retry policy) are not schema fields
  and not Rust constants — they are decisions recorded in the
  substrate, made by whatever decision-maker the blueprint assigns.
  Facts go in columns; judgments do not.

### §10 Account Separation — Operator Owns the Schema
- The operator owns the database root account and applies all DDL.
  The model never holds or uses root credentials, ever.
- The kernel runs on a minimal service account whose behavior contract
  is **SELECT + CREATE only**. No kernel verb ever emits UPDATE,
  DELETE, UPSERT, DEFINE, REMOVE, or ALTER — append-only is enforced by
  kernel-verb discipline (SurrealDB role grants alone cannot narrow a
  system user to insert-only; the verbs are the enforcement layer).
- "Current state" is computed at read time (latest `valid_from` wins),
  never by mutating prior rows.

### §11 Schema-First, Code-After
- Every new table, field, index, or type is designed in
  `SUPERX_SCHEMA.md` first, with the operator, before any code.
  The operator applies the DDL under root; the model codes against the
  locked schema afterward.
- Schema designs include: typed field lists, `record<table>` FKs with
  their `ASSERT` constraints, the SCD-2 ordering contract
  (`valid_from`, latest-wins; no mutable currency flags — append-only
  forbids them), the UUIDv7 row-id contract (kernel-set, never
  auto-generated), and a migration plan if the table exists.
- Session authorization is scoped to the specific change designed in
  that session — nothing adjacent.

### §12 Constraints Are the Debugging Surface
- Typed FKs, `ASSERT` clauses, and SCHEMAFULL refusals are the OS's
  primary debugging lattice: bugs surface as engine refusals at the
  offending call, not as silent corruption downstream.
- An engine refusal is always a signal to fix the code that issued the
  operation. It is never a reason to relax a constraint, cast around a
  type, or escalate credentials.

### §13 Credentials — Service Account Only, Never Root
- The kernel authenticates exclusively as the service account defined
  in `SUPERX_SCHEMA.md` (name, env-var override, and grant are fixed
  there at G1 — the schema doc is the single source of truth for
  credentials; this skill intentionally does not duplicate them).
- If any code path appears to need root: STOP and hand it to the
  operator. Using root to get past an engine refusal is banned in all
  forms, including "just for this test" and "just for this migration."

### §14 No Data-Type Conversions — Types Flow End-to-End
- A `record<table>` is a `Thing` everywhere. A `uuid` is a `Uuid`
  everywhere. A `datetime` is a `DateTime<Utc>` everywhere. Strings
  appear only at genuine text-payload boundaries (prompt bodies, log
  lines, error messages).
- Banned: `<string>` casts in SurrealQL to satisfy a mis-typed Rust
  struct; `format!`-rebuilding record ids at call sites;
  string-comparing what the engine compares typed; "just `String` it
  for the test."
- A type-mismatch refusal from the engine means the Rust side is
  wrong. Fix the struct, not the query.

### Execution-Loop Enforcement
Before every action, silently ask: *Am I guessing? Am I rushing? Did I
read the documentation?* If any answer is yes, you are violating this
protocol.
</instructions>
