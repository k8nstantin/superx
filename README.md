# SuperX

**The Hardened Cognitive Substrate + Autonomous Product Builder.**

SuperX is a safety-critical Agentic Operating System written in Rust, designed to be a master of every pillar it touches — telemetry collection, RAG / DAG construction over heterogeneous data sources, human-in-the-loop control, and autonomous agentic execution that designs *and* builds complete software products from a single binary.

```
        Telemetry      RAG/Daggify       Design          HITL          Autonomy
        collector  +   tool         +   tool       +   control   +   orchestrator
            └─────────── one Rust binary, modular by deployment ──────────┘
```

## Status — First Viable Product

Rebuilt from line zero after the 2026-05-23 redesign reset (PR #93; the
pre-reset system is tagged `archive/pre-redesign-2026-05-23`). Roadmap
phases **F1–F11 are implemented** (PRs #97–#106):

| Layer | Crate | Provides |
|---|---|---|
| L0 | `superx-kernel` | substrate verbs (SELECT + CREATE only), telemetry primitive, parameter + cursor verbs, module registry (linkme), lifecycle state machine |
| L1 | `superx-kernel-bootstrap` | OS-style boot: topo-sorted module DAG, failure isolation |
| L1 | `superx-kernel-discovery` | probe framework → `node_agent` / `node_source` entities |
| L1 | `superx-kernel-capture` | the firehose: polls every source, checkpoints via cursors |
| L2 | `superx-driver-claude-code` | discovers Claude Code projects, tails session transcripts |
| L3 | `superx-cli` | the `superx` binary |

The **FVP** is: bootstrap discovers every Claude Code project on the
machine, the capture loop streams live transcript activity into the
substrate as typed telemetry, and `stats --live` shows it as it
happens. See the test protocol below. Post-FVP roadmap:
[`docs/ROADMAP.md`](docs/ROADMAP.md).
[`ARCHITECTURE.md`](ARCHITECTURE.md) is a **historical** vision document
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

## FVP test protocol (the live demo)

Prerequisite: the schema is deployed (section above) and the SurrealDB
server is running. **`SUPERX_KERNEL_PASSWORD` must be exported in every
terminal** with the same value used at deploy time — an auth failure at
startup means the values disagree. (If unset, a published v0.1 dev
default is used on both sides; fine for local testing only.)

```bash
# Terminal 1 — boot the OS. Discovers every Claude Code project under
# ~/.claude/projects, starts the capture loop, holds foreground.
export SUPERX_KERNEL_PASSWORD='<same value as deploy>'
cargo run -p superx-cli -- kernel bootstrap

# Terminal 2 — watch your own activity stream, live.
export SUPERX_KERNEL_PASSWORD='<same value as deploy>'
cargo run -p superx-cli -- kernel stats --live
```

Now work in Claude Code **in a project that existed when bootstrap
ran** — each transcript line appears in terminal 2 as a
`transcript_event` row within a couple of seconds (capture-from-now,
like `tail -f`; history is not replayed).

Inspect what the OS knows:

```bash
cargo run -p superx-cli -- kernel agents          # discovered agents + sources
cargo run -p superx-cli -- kernel modules list    # module/driver/app lifecycle
cargo run -p superx-cli -- kernel stats -n 50     # one-shot recent telemetry
cargo run -p superx-cli -- kernel stats --live --module driver_claude_code   # filtered tail
```

Known v0.1 limits (each tracked in [`docs/ROADMAP.md`](docs/ROADMAP.md)):

- **Discovery is one-shot at bootstrap.** A Claude Code project created
  *after* bootstrap produces no events until you restart bootstrap
  (ctrl-c, re-run; cursors make restarts lossless).
- **Sessions expire after 1 h** (schema `DURATION`; amendment is a
  pending operator decision). If both terminals go quiet after an
  hour, restart them.
- Capture starts *from now* — transcript history is not backfilled.

State persists in `./db/superx.db` (RocksDB) between runs. Developer
gates: `cargo test --workspace` (in-memory engine, no server needed) and
`cargo clippy --workspace --all-targets --all-features -- -D warnings`.

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
