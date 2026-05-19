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

### Execution Loop Enforcement
For every single action you take, you must silently ask yourself: *"Am I guessing? Am I rushing? Did I read the documentation?"* If the answer to any of these is yes, you are violating this protocol.
</instructions>
