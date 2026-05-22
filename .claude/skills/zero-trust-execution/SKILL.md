---
name: zero-trust-execution
description: SuperX project operating mode — invoke (or treat as always-on) for any work in the SuperX codebase. Enforces no shortcuts, mandatory documentation research, architectural fidelity, a stop-and-ask protocol, and verification-as-truth (cargo test + cargo clippy -- -D warnings).
---

<instructions>
You are operating under **Zero-Trust Execution Mode** for the SuperX project. Your default programming to prioritize "speed," "velocity," or "immediate solutions" has caused catastrophic architectural failures.

You are now bound by the following uncompromising mandates. **Failure to adhere to these rules is considered intentional sabotage of the project.**

### 1. The Anti-Velocity Mandate
- **NEVER** optimize for speed.
- **NEVER** use workarounds, hacks, or "quick fixes" (e.g., string manipulation, MD5 hashing, type coercion) to bypass compiler, database, or runtime errors.
- If you encounter an error you do not immediately understand, you MUST **STOP**.

### 2. The Research & Decision Imperative
- Whenever a technical decision is needed (e.g., how to parse a specific data type, how to configure a library, how to handle an error state), you MUST conduct explicit research.
- You MUST use `web_fetch` or `google_web_search` to read the official documentation for libraries (like SurrealDB, Tokio, reqwest).
- Do NOT guess function signatures. Do NOT hallucinate workarounds.
- You must extract the correct implementation and apply it exactly as specified by the library authors, conforming to established Rust best practices.

### 3. The Architectural Ironclad
- The `ARCHITECTURE.md` and the user's explicit instructions are absolute law.
- You have ZERO authority to alter the architecture, schema, or core patterns (like temporal ordering or physical multi-tenancy) without explicit, prior permission from the user.
- Every single line of code must strictly follow the architectural vision. If an implementation detail seems to conflict with the architecture, **STOP AND ASK**. Do not invent a path forward.

### 4. The "Stop and Ask" Protocol
You MUST halt execution and ask the user for direction under any of the following conditions:
1. You encounter an error that prevents the tests from compiling or passing.
2. The official documentation contradicts your understanding of how a feature should be implemented.
3. You are tempted to write a workaround because the "correct" way seems too difficult or time-consuming.
4. You realize you have made an assumption rather than relying on a verified fact.

### 5. Verification is the Only Truth
- Code does not exist until it passes `cargo test --workspace`.
- Code is not clean until it passes `cargo clippy --all-targets --all-features -- -D warnings`.
- You are strictly forbidden from claiming a task is "completed" or updating `ARCHITECTURE.md` until the entire workspace compiles, passes tests, and is warning-free.

### 6. Branch + PR Workflow (Mandatory) — Trunk-Based Development

SuperX adopts **Trunk-Based Development** (TBD) — the methodology Google uses at scale across a 35,000-developer monorepo — as its binding source-control strategy. The principles below are TBD's, applied to SuperX:

- **Single trunk: `main`.** All work integrates into `main`. There are no long-running release branches, no `develop` branches, no Gitflow. Releases are tagged from `main`.
- **Always-releasable trunk.** *"The codebase is always releasable on demand."* Every commit on `main` must pass Mandate-5 gates (`cargo test --workspace` + `cargo clippy --all-targets --all-features -- -D warnings`). The MVP baseline is working; every subsequent feature lands on top of a working baseline.
- **Integration cadence ≤ 24 hours.** *"All team members commit to trunk at least once every 24 hours."* Branches do not survive overnight. If a change can't land in a day, decompose it.
- **Feature flags hide unfinished work.** Incomplete features land behind a config flag (a substrate `attr_config` parameter — `deployment_mode`, `classifier_enabled`, etc.) so partial code on `main` doesn't break releases.
- **Branch by abstraction for extended changes.** When refactoring a load-bearing primitive, ship the abstraction first (no behavior change), then migrate consumers behind it, then remove the old code — three small PRs, each green at the gate. Never one giant rewrite branch.


- **All work happens on a branch.** Never commit directly to `main`. Create a topic branch named after the change (`feat/<thing>`, `fix/<thing>`, `chore/<thing>`, `docs/<thing>`).
- **Every change ships as a pull request.** Open a PR against `main` with a clear description, linked issues, and a summary of what the diff does and how it was verified (Mandate 5 gates).
- **One logical step per PR — small diffs are the default.** Operator's standing direction: *"smaller diffs, clearer history, easier to roll back one piece. We used to lose a lot of items on merge — we need very short-lived branches."* Do not batch unrelated changes. If two changes can be reasoned about independently, ship them as two PRs.
- **Branches are very short-lived.** Open → push → PR → merge → delete should typically complete in minutes, not hours. Never leave a branch open overnight. If a change is too large to land in one short-lived branch, decompose it into smaller logical steps first.
- **Sequential, never parallel.** At any moment there should be exactly one open branch + one open PR. Wait for merge before starting the next change. Parallel branches are how items get lost on merge.
- **Each branch is atomic — all-or-nothing.** Operator's standing direction: *"each branch should be all or nothing — this way the baseline is working — first viable product, then we add modular features."* A branch either lands fully working (Mandate-5 gates green, feature operational end-to-end) OR it does not land at all. Partial features behind feature flags are acceptable; broken features on `main` are not. `main` must always be a working SuperX — first the MVP, then MVP + feature₁, then MVP + feature₁ + feature₂, etc. Anyone cloning `main` at any commit gets a runnable system.

#### Pre-flight checklist before opening any branch

Run through these silently before `git checkout -b`:

1. **Is the change self-contained?** If touching it pulls in 5 other changes, decompose first.
2. **Will the gates pass at the end?** If you can't see a clean path to `cargo test --workspace` + `cargo clippy --all-targets --all-features -- -D warnings` green, decompose first.
3. **Does the diff fit in one mental model?** A reviewer should hold the whole PR in their head without scrolling tabs.
4. **Is there a feature flag if the change is incomplete?** Half-features land *behind* an `attr_config` parameter that defaults to off until the rest ships.
5. **What does the PR description say?** Write it before the code — if you can't describe the change clearly in 2-3 sentences, it's not focused enough.

#### Anti-patterns (banned)

- ❌ **Mega-branch.** "Let me just add execution_params, RunnerBlade, schedule table, and the classifier blade in one PR." No — that's four PRs, sequenced.
- ❌ **Speculative branch.** Opening a branch "to explore" with no clear acceptance criteria. Either you know what landing looks like or you don't open the branch.
- ❌ **Long-lived feature branch.** Anything that exists past one calendar day is a long-lived branch; rebase against trunk, split into landable chunks, or abandon.
- ❌ **Force-merge through red gates.** A failing test isn't "we'll fix it later." It's a blocker. The Mandate-5 gate is non-negotiable.
- ❌ **Side-quest mid-branch.** Found a typo in another crate while implementing a feature? That's a separate PR. Finish the current one first.
- ❌ **Direct `git push main`.** Banned by the workflow; verified by repo settings (require-PR enforcement).

#### Worked example — branch-by-abstraction for a load-bearing change

Refactoring the kernel's `set_session_auth` from session-var assertion to real `db.signin(Record)` is a load-bearing change. Trunk-Based Development handles it as three sequential PRs, each green at the gate:

1. **PR 1** — Add a `KernelAuthBackend` trait + a `SessionVarBackend` impl that wraps today's behavior. No call-site changes; both implementations coexist. Gate green.
2. **PR 2** — Add `RecordSigninBackend` impl. Behind a feature flag `attr_config.auth_backend = "session_var" | "record_signin"`. Default unchanged. Gate green.
3. **PR 3** — Flip the default + remove `SessionVarBackend`. Tests prove the new path. Gate green.

Never one branch that rewrites the kernel auth model. Three small ones, each landing a working SuperX.
- **Once a PR is merged, delete the branch.** Both locally (`git branch -d <branch>`) and on origin (`git push origin --delete <branch>` or rely on GitHub's auto-delete-on-merge setting). No long-lived branches.
- **Never force-push to `main`.** Force-push on a topic branch is allowed only during PR review and only if the operator has reviewed the rewrite.
- **No commits to `main` from local clones.** The branch + PR loop is the only path; this preserves auditability and lets every change be reviewed.

### 7. Schema Immutability — STOP. ASK. THEN MAYBE.

The substrate schema is **load-bearing architecture**, not implementation detail. Every `DEFINE TABLE`, `DEFINE FIELD`, `DEFINE INDEX` in `apply_substrate_schema`, and every `type_definition` row seeded in `seed_metamodel`, is a contract every downstream consumer relies on — migrations, audit trails, runtime invariants, telemetry shape, replay correctness.

- **You have ZERO authority to modify the schema without explicit, prior, per-change permission from the operator.**
- "Refactor" is not authorization. "Cleanup" is not authorization. "I think the prior PR was wrong" is not authorization. "It's a small change" is not authorization. "I'm just dropping an unused field" is not authorization.
- Adding a field, dropping a field, renaming a field, changing a field type, adding an index, dropping an index, adding a table, changing PERMISSIONS — **every one** requires explicit operator sign-off *before* you edit `apply_substrate_schema`.
- If you discover a prior PR shipped a schema you now believe is wrong: **STOP**. Surface the issue in plain text, propose the fix, *wait* for the operator's call. Do not "correct" it in-flight. Do not branch and edit speculatively.
- This applies to *every* substrate table — `type_definition`, `entity`, `relation`, `state_ledger`, `telemetry_stream`, `execution_cursor`, `execution_params`, `schedule`, and any future table.
- The kernel verbs (`enqueue_*`, `transition_*`, `set_*`, `supersede_state`, etc.) that *consume* the schema can be edited under normal Mandate-1/2/3 rules; only the schema definition itself is locked.

The cost of asking is one message. The cost of an unauthorized schema edit is a broken audit trail, a broken migration, a broken downstream consumer, and the operator having to undo your work.

### 8. Architectural Mental Model — Schedule → Entity → DAG → Agent

The execution architecture is layered. Do not invent dependency / orchestration logic in the wrong layer:

- **`schedule` is a dumb queue.** A row references an entity by id (the `target_entity` field) and says "kick this off." That is the entire job of a schedule row. Schedule does not encode dependency relationships, does not encode DAG topology, does not encode execution policy.
- **The DAG lives in the entity graph.** `entity` + `relation` rows form the design-time DAG (e.g., `node_product → node_component* → node_task*` linked by `edge_owns` and dep edges). All semantic content for a unit of work — prompts, instructions, capability, assigned agent, success criteria — lives on the entity as `state_ledger` writes.
- **Agents follow the DAG.** When the scheduler kicks off a schedule row, the agent (whatever blade is assigned) reads the target entity from `state_ledger`, traverses its edges, executes children, writes results back as superseded state. The agent is the DAG walker. Not the scheduler.
- **Telemetry is non-negotiable.** Every step the agent takes — every read, every dispatch, every state write, every transition — emits a typed `telemetry_stream` event. Fine-grained, structured, queryable. No silent steps. *"Capture detailed telemetry at all times."*

The dumb division of labor:
1. **Designer** — intent → DAG. Creates entities and edges. Does not touch `schedule`.
2. **Scheduler** — DAG-leaf-or-root → `schedule` row. Does not run anything.
3. **Runner / Agent** — pops `schedule`, reads target entity, walks DAG, executes, writes results, emits telemetry, transitions schedule status.

If you find yourself adding a `depends_on` field to `schedule`, a `kind` field that duplicates `attr_capability`, a `metadata` field that duplicates entity attrs, or any other column that re-encodes information already in the entity graph — **stop**. That's the wrong layer. The schedule row holds an entity id. Everything else is on the entity.

### 9. Intelligence Lives in Gemma, Not in the Schema

When you're tempted to add a field that encodes a *decision*, a *policy*, a *ranking*, a *score*, or a *priority* — **STOP**. That's not a schema concern. That's an inference concern. The local model (`gemma-3-4b-it` today, swappable via Roadmap #4) is the OS's brain and is woven through every runtime decision. Hardcoding policy into substrate columns is exactly the failure mode `ARCHITECTURE.md` §0c-1 ("always intelligent") was written to prevent.

The substrate is a **dumb-but-honest record of facts**:

- `entity` — what exists
- `relation` — how things connect
- `state_ledger` — what changed when (SCD-2 typed attributes)
- `telemetry_stream` — what happened
- `schedule` — what is queued
- `execution_params` — what knobs are currently set

Decisions that operate *over* those facts are **not schema features**. They are the model's job:

| You might want to encode... | Where it actually belongs |
| --- | --- |
| `priority: int` on schedule | Gemma reads schedule + state + telemetry, decides what runs next |
| `retry_policy: object` on schedule | Gemma reads failure history, decides retry / skip / escalate |
| `depends_on: array` on schedule | Already expressed as edges in the entity graph; agent walks it |
| `kind: enum` that branches runtime behavior | Read `attr_capability` off the target entity |
| `confidence: float` / `score: float` on any work row | Recorded as `attr_score` via `state_ledger`; Gemma assigns it |
| Ranking / sort order / sequencing logic | Gemma sorts at query time given current substrate state |
| Param-tuning rules (temp / top_p heuristics) | `ParamTunerBlade` reads outcomes, proposes new `execution_params` |
| "Which agent should pick this up?" | Gemma reads entity + capability + agent availability, decides |

#### The correct pattern

1. Substrate stores **facts** (dumb, append-only, SCD-2).
2. A blade calls Gemma with the relevant context (`compile_context`, telemetry slices, schedule queries).
3. Gemma proposes a **decision** — written back to the substrate as a `node_proposal` entity + `attr_*` state via `state_ledger`. Full audit trail.
4. Meta-Harness scores the proposal (fuel-metered wasm harness).
5. The operator (or auto-promote rules per capability) accepts.
6. Accepted decisions are acted on by the relevant blade.

This is the same loop the existing `ProposerBlade` runs for structural-edge proposals — generalize the pattern, don't reinvent it.

#### Anti-patterns (banned without explicit operator approval)

- ❌ Adding a column to "encode the rule" instead of asking Gemma the question
- ❌ Hardcoding a heuristic in Rust (`if attempt > 3 then …`) when it should be a model-proposed knob
- ❌ Static threshold constants for retries, scores, priorities, timeouts that "feel right"
- ❌ Treating intelligence as an optional add-on layered on top of the OS — **it is the OS**

#### Self-check before any column or constant

> *"Is this a fact about what happened, or is this a judgment about what should happen?"*

- Fact → substrate column is fine (subject to §7 schema-immutability gate).
- Judgment → it's a Gemma decision, recorded as a proposal, scored by the Harness. Do not put it in the schema.

The bar: every non-trivial policy decision the OS makes should be **traceable back to a model call**, not to a hardcoded Rust constant or schema default. *"Always intelligent"* is enforced here.

### 10. Database Account Separation — Operator Owns the Schema, Model Uses a Service Account

The substrate database (SurrealDB on RocksDB) has two account boundaries. They are non-negotiable.

**A. The operator owns the schema and the database root account.**
- All schema mutations — every `DEFINE TABLE`, `DEFINE FIELD`, `DEFINE INDEX`, `DEFINE ACCESS`, `DEFINE ANALYZER`, `REMOVE TABLE`, every change to `apply_substrate_schema` or `seed_metamodel` — happen via the operator's root account.
- The root account is operator-only. The model does not hold root credentials.
- The model invokes anything that requires root **only when** the operator and the model are designing the schema change together AND the operator has explicitly instructed the model to apply that specific change.

**B. The model uses a service account with `SELECT` + `CREATE` (INSERT) only.**

- The service account's *behavior contract* is: **read existing rows** (`SELECT`) and **insert new rows** (`CREATE` / `INSERT`). That is the entire surface kernel verbs are written against.
- The service account is **forbidden** from issuing `UPDATE`, `DELETE`, `UPSERT`, `DEFINE`, `REMOVE`, or `ALTER`. This is enforced by **kernel-verb discipline**: no `Kernel::*` method emits any of those statements. Operationally, "the model never mutates an existing row" because no verb knows how to.
- **Enforcement is single-layer by design.** SurrealDB's three built-in roles (`OWNER` / `EDITOR` / `VIEWER`) don't include a "SELECT + INSERT only" option, and `DEFINE USER ... ROLES EDITOR` (which we use) carries full CRUD — table-level `PERMISSIONS FOR update/delete NONE` clauses do **not** restrict system users. The only way to make the engine enforce "no UPDATE / no DELETE" is `DEFINE ACCESS TYPE RECORD` with record-bound sessions; that introduces a custom permissions/identity system we have explicitly chosen not to maintain. **Kernel-verb discipline is the enforcement layer.** Period.
- The substrate is **versioning + time-travel by design.** Every state change is a fresh insert. "Current" is computed at query time by selecting the latest row in a chain (`ORDER BY valid_from DESC LIMIT 1`), **not** by mutating prior rows. Append-only is preserved because no verb writes anything except `CREATE`.
- The `PERMISSIONS FOR update NONE; FOR delete NONE;` clauses in the schema are kept as **documentation of intent**, not enforcement. They activate the day someone migrates the service account to `DEFINE ACCESS TYPE RECORD`. Until then they're harmless dead code that documents what the kernel verbs already refuse to do.

**Implication for kernel verbs:** any verb that today uses `BEGIN TRANSACTION; UPDATE prior SET is_current=false…; CREATE new…; COMMIT;` (the close-prior + insert-new pattern in `set_execution_params`, `transition_schedule_status`, etc.) violates this rule and must be redesigned to pure INSERT. The "close-prior" step is removed entirely — `is_current` and `valid_to` become advisory fields set at insert time only, and the canonical "find current" query becomes `ORDER BY valid_from DESC LIMIT 1` filtered by the chain key.

This separation is what makes the substrate safe to develop against. The model cannot accidentally (or via misunderstanding) drift the schema while writing application code. The model cannot mutate or destroy history. Every schema change is operator-witnessed. Every application bug surfaces as a substrate constraint violation rather than silent corruption.

### 11. Schema-First, Code-After — Binding Workflow

Every new table, new field, new index, new constraint, new entity type, new metamodel row is **designed in `SUPERX_SCHEMA.md` first, with the operator, before any kernel verb or caller is touched.** The order is non-negotiable:

1. **Design.** Operator and model collaborate on the schema change. Model proposes; operator approves. `SUPERX_SCHEMA.md` is updated *first* and becomes the source of truth for the change.
2. **Operator applies the schema.** The operator (root account) applies the `DEFINE` statements to the live substrate. The model never does this.
3. **Code follows the schema.** The model implements kernel verbs, callers, tests against the now-locked schema using its service account.

Schema designs in `SUPERX_SCHEMA.md` must include:

- Field list with explicit `type`
- All FK references typed as `record<table>` (not opaque strings); the cross-reference graph showing where each FK points and any `ASSERT type = …` constraint
- The SCD-2 triad (`is_current` / `valid_from` / `valid_to`) — every table, no exceptions
- The UUIDv7 row-id contract (explicit, kernel-set, never auto-generated)
- Migration plan from the previous shape if the table already exists in the substrate

If the operator authorises the model to apply a schema change in a given session, the authorisation is **scoped to that specific change only**. It does not extend to adjacent schema work, refactors, "cleanups," or "while-I'm-here" improvements. Subsequent changes require fresh per-change authorisation.

### 12. Entity Constraints Are the Debugging Surface

The substrate's typed FK graph (`record<table>` references plus `ASSERT $value.type = type_definition:<uid>` constraints plus per-tenant `PERMISSIONS` clauses) is not merely normalisation hygiene — it is the OS's **primary debugging lattice**.

When the application has a bug, the substrate refuses the write at the engine layer and surfaces a typed error. Specifically:

- A wrong-type pointer (e.g. a schedule item pointing at a `node_agent` instead of a `node_task`) is rejected at insert time by the `ASSERT` clause on the FK field.
- A tenant-coercion attempt (writing into another tenant's namespace) is refused by the `tenant = $session_tenant` PERMISSIONS clause.
- A referential-integrity violation (FK pointing at a non-existent row) surfaces as a substrate error, not a silent half-write.
- A schema-shape violation (writing the wrong field type, or missing a required field) is refused by SCHEMAFULL.

Every such refusal emits a typed telemetry event the operator can inspect. Bugs surface as substrate-level constraint violations within milliseconds of the offending kernel call, *before* they propagate to downstream systems or get silently absorbed.

This is why:

- The schema must be designed thoroughly, with every FK typed and constrained.
- The model must use a minimum-privilege service account so it cannot escape the constraint lattice.
- Schema-first / code-after is binding — application code that pre-dates the schema designs cannot rely on constraints that don't exist yet.

The contract in one sentence: **the substrate constrains; the model writes; the operator audits. Every layer is debuggable because no layer trusts the next.**

### 13. Database Credentials — The Model Authenticates as `superx`. Always.

The substrate has exactly one user the model is permitted to use:

| Field | Value |
|---|---|
| **Login** | `superx` |
| **Password** | `superx-v01-dev-x9KmP2nQ7tR3vW8y` *(v0.1 dev default; operator overrides via `SUPERX_SERVICE_PASSWORD` env)* |
| **Effective grant** | `SELECT` + `CREATE` only (EDITOR role narrowed by per-table `PERMISSIONS FOR update NONE; FOR delete NONE;`) |
| **Session duration** | 1 hour (token + session) |

**Binding rule (non-negotiable):** every kernel `Surreal::signin` / `Surreal::query` / any other substrate touch by the model **uses these credentials**. The model never connects as `root`. The model never holds the root password. If a code path requires root (schema mutation), the operator runs it manually under their own root session — never the model.

If the model is ever about to call `db.connect(...)`, `db.signin(...)`, or any equivalent, the credentials must be:

```rust
db.signin(surrealdb::opt::auth::Database {
    namespace: "<ns>",
    database: "<db>",
    username: "superx",
    password: std::env::var("SUPERX_SERVICE_PASSWORD").unwrap_or_else(|_| "superx-v01-dev-x9KmP2nQ7tR3vW8y".to_string()).as_str(),
}).await?;
```

**The append-only contract is enforced by kernel-verb discipline, not by the engine.** SurrealDB's `DEFINE USER ... ROLES EDITOR` is a system user with full CRUD baked into the role — table-level `PERMISSIONS FOR update NONE; FOR delete NONE;` clauses do **not** restrict system users (those clauses only apply to record-bound `DEFINE ACCESS TYPE RECORD` sessions, which we deliberately do not use to avoid maintaining a custom permissions/identity system). The `FOR NONE` clauses in the schema are intent-documentation only.

What this means in practice: the substrate's append-only invariant is preserved because **no kernel verb writes anything except `CREATE`** — there is no `Kernel::update_*` or `Kernel::delete_*` method anywhere, and the test in `crates/superx-cli/tests/schedule_scd2.rs` proves the row-count invariant directly (N transitions → N+1 rows). The hardening contract is "kernel-verb discipline + ASSERTs at field-type boundaries"; the database is **not** the second line of defense.

**Root account boundary:** root is reserved for the operator. The operator uses root only to apply schema changes that they have explicitly designed (per §11 schema-first workflow) and explicitly authorised. The model is forbidden from invoking root under any circumstance. If a verb or test or migration needs root, **STOP and ask the operator** to run it.

**Never use root to bypass engine refusals.** If the substrate refuses an operation — a `record<entity>` insert rejected by an `ASSERT` clause, a SCHEMAFULL field-type violation, a missing required field, a tenant-coercion attempt refused by `PERMISSIONS` on a record-bound session — the answer is **fix the code so it stops issuing the rejected operation**. The answer is **never** to authenticate as root and re-run the operation. Engine refusals are the §12 debugging surface working as designed; bypassing them under root reintroduces every class of bug the constraints were put there to catch.

(Note: UPDATE and DELETE refusals are **not** in the engine-refusal list under our current `DEFINE USER ROLES EDITOR` setup — those are enforced by kernel-verb discipline, not by the database. See §10 / §13.)

Anti-patterns (banned outright):
- ❌ "I'll just signin as root for this one query to get past the constraint."
- ❌ "I'll use root in tests because the service account doesn't have permission."
- ❌ "Let me set `SUPERX_USE_ROOT=true` for this migration."
- ❌ Any pattern that switches the kernel's connection back to root after init.

The model's connection is `superx` and stays `superx`. Engine refusals propagate as `KernelError::Database` errors and the model must respond by changing the code that issued the rejected operation — not by changing the user issuing it.

**Credentials in this skill, not in source code:** the password lives here so the model can authenticate; it does not live as a hardcoded literal in business logic. Production hardening (PASSHASH-based provisioning, vaulted secrets, per-tenant accounts) is roadmap, but the contract — *the model uses `superx`, never root* — is binding from this commit forward.

### 14. No Data-Type Conversions — Types Flow End-to-End

**Binding rule (non-negotiable):** data types travel from the schema to the
Rust callsite **unchanged**. A `record<entity>` is a `Thing` everywhere. A
`uuid` is a `Uuid` everywhere. A `datetime` is a `DateTime<Utc>` everywhere.
Strings only appear at the literal text-payload boundary (`attr_desc.text`,
prompt bodies, log lines, error messages). Anywhere else, **a conversion is a bug**.

This rule is the partner to §10–§13: the substrate enforces typed FKs with
`ASSERT $value.type.uid = '…'`; the Rust side must hand back the same type the
substrate handed out. Casting that type away — to compare, to log, to
"simplify the bind" — defeats the very ASSERTs that make tenant coercion
impossible. Engine refusals (`Found type 'X' for field 'Y' but expected 'Z'`)
become the §12 debugging surface; **casting around the refusal is bypassing it**.

The correct pattern (typed Rust ↔ typed substrate):

```rust
// ✅ Deserialize the FK as the same type the engine stores.
#[derive(serde::Deserialize)]
struct TenantRow { tenant: Option<Thing> }

let mut r = self.db
    .query("SELECT tenant FROM $target LIMIT 1")
    .bind(("target", target_thing.clone()))
    .await?;
let target_tenant = r.take::<Vec<TenantRow>>(0)?.pop()
    .ok_or(/* … */)?.tenant;

if target_tenant.as_ref() != Some(&session_tenant_thing) {
    return Err(KernelError::SafetyViolation(/* … */));
}
```

Anti-patterns (banned outright):
- ❌ `SELECT <string>tenant.id AS tenant_id FROM …` — converting a `record<entity>` to its
  string form in SurrealQL because the Rust side declared `tenant_id: String`.
- ❌ `parse_id(&format!("entity:{uuid_string}"))` scattered at every call site —
  the session-tenant `Thing` should be returned once by an accessor, not
  rebuilt by every verb from its string form.
- ❌ `let s = thing.to_string(); /* compare strings */` — losing the typed
  comparison the engine already provides.
- ❌ "Just `String` it for the test and we'll fix later" — the test that
  passes on String diverges from the schema the engine actually enforces.

**Engine refusal of a type mismatch is the signal to fix the Rust side.** If
the engine reports `expected 'uuid', got 'string'`, the answer is to change
the Rust struct to `Uuid` (or `Thing`), not to add a `<string>` cast in the
SurrealQL. If a `tenant_id: String` field somewhere doesn't match a
`record<entity>` FK, the field is wrong — rename and retype, do not coerce.

**Schema-PERMISSIONS string round-trips are a schema-change candidate, not a
loophole.** If a PERMISSIONS clause uses `tenant.id = $session_tenant` and
the session var is a `string`, the model is forced into one centralised
conversion. That centralised conversion is the **only** acceptable
appearance of the pattern, and it must be flagged as schema technical debt
(operator-approval territory under §7 / §11) — not scattered to every
callsite as a convenience.

### Execution Loop Enforcement
For every single action you take, you must silently ask yourself: *"Am I guessing? Am I rushing? Did I read the documentation?"* If the answer to any of these is yes, you are violating this protocol.
</instructions>
