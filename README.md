# SuperX

**The Hardened Cognitive Substrate + Autonomous Product Builder.**

SuperX is a safety-critical Agentic Operating System written in Rust, designed to be a master of every pillar it touches — telemetry collection, RAG / DAG construction over heterogeneous data sources, human-in-the-loop control, and autonomous agentic execution that designs *and* builds complete software products from a single binary.

```
        Telemetry      RAG/Daggify       Design          HITL          Autonomy
        collector  +   tool         +   tool       +   control   +   orchestrator
            └─────────── one Rust binary, modular by deployment ──────────┘
```

## Status

Rebuilt from line zero after the 2026-05-23 redesign reset (PR #93; the
pre-reset system is tagged `archive/pre-redesign-2026-05-23`).

- **Implemented:** `crates/superx-kernel` — the F0 atomic core (PR #96):
  substrate verbs (SELECT + CREATE only), telemetry primitive, kernel-module
  registry (linkme), lifecycle state machine. 10 / 10 tests passing,
  clippy-clean under `-D warnings`.
- **Not yet implemented:** CLI, bootstrap orchestrator, agent discovery,
  capture loop — see [`docs/ROADMAP.md`](docs/ROADMAP.md) for the phased
  plan to the First Viable Product.
- **FVP bar:** run `superx kernel bootstrap`, open Claude Code, watch live
  activity stream in `superx kernel stats --live`.
- [`ARCHITECTURE.md`](ARCHITECTURE.md) is a **historical** vision document
  (pre-reset); current schema truth is [`SUPERX_SCHEMA.md`](SUPERX_SCHEMA.md).

## Deploy substrate schema (operator one-shot)

The substrate schema is shipped as a single locked DDL file —
[`schema/kernel.surql`](schema/kernel.surql) — and applied **once** under
the operator's root account. From that moment forward the schema is
locked; every subsequent change requires an explicit per-change
`Operator-approved:` marker on the PR that touches it
(see [SKILL.md §7](.claude/skills/zero-trust-execution/SKILL.md)).

```bash
# 1. Install surreal CLI
curl --proto '=https' --tlsv1.2 -sSf https://install.surrealdb.com | sh

# 2. Start a SurrealDB server (rocksdb-backed, local persistence)
export SUPERX_ROOT_PASSWORD='<choose a strong root password>'
surreal start --user root --pass "$SUPERX_ROOT_PASSWORD" rocksdb:./db/superx.db &

# 3. Apply the schema once
export SUPERX_KERNEL_PASSWORD='<choose the kernel service-account password>'
./scripts/deploy-schema.sh
```

After this point, all SuperX kernel code signs in as the `superx_kernel`
service account (EDITOR role) — never root. Drivers and apps each
ship their own schemas + service accounts (post-FVP). The append-only
invariant is
enforced by **kernel-verb discipline**: no kernel verb emits UPDATE or
DELETE statements (SKILL.md §10 / §13).

## Quickstart

The operator-facing CLI ships at roadmap phase F7
(see [`docs/ROADMAP.md`](docs/ROADMAP.md)). Until then the kernel is a
library crate:

```bash
# Verify the kernel (in-memory engine; no server required)
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

State persists in `./db/superx.db` (RocksDB) between runs of a deployed
substrate (see schema deployment above).

## Architectural principles (binding)

1. Everything is an entity.
2. Types are themselves entities — runtime-extensible without schema migrations.
3. Products are cascading DAGs (`node_product → node_component* → node_task* → node_artifact*`).
4. Data sources are *compiled* entities — ingested once, referenced thereafter.
5. Reusable components, graph-promoted by the Meta-Harness.
6. Schedule + Runner drive execution autonomously.
7. **No hardcoding** — every constant flows through `attr_config`.
8. **No database-rule bypass** — service-account auth, no super-user at runtime.

Plus the §7 invariants in [`ARCHITECTURE.md`](ARCHITECTURE.md): SCD-2 + append-only for every new table, time-travel-queryable audit trail, CRDT-ready substrate, chat UI as downstream consumer only.

## Workspace layout

```
crates/
  superx-kernel/      L0 atomic core: substrate verbs (SELECT + CREATE only),
                      telemetry primitive, kernel-module registry (linkme),
                      lifecycle state machine
```

Future layers land per [`docs/ROADMAP.md`](docs/ROADMAP.md): kernel modules
(`superx-kernel-<feature>`), drivers (`superx-driver-<name>`), apps
(`superx-<name>`, starting with `superx-cli`).

## Documentation

- [`docs/ROADMAP.md`](docs/ROADMAP.md) — current phased plan to FVP and beyond.
- [`SUPERX_SCHEMA.md`](SUPERX_SCHEMA.md) — schema source of truth.
- [`ARCHITECTURE.md`](ARCHITECTURE.md) — **historical** long-term vision (pre-reset).
- `.claude/skills/zero-trust-execution/SKILL.md` — operator-mandated execution mode for contributors using LLM-assisted development.

## License

[Apache License 2.0](LICENSE). See [`NOTICE`](NOTICE) for attribution.
