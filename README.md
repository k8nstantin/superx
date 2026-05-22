# SuperX

**The Hardened Cognitive Substrate + Autonomous Product Builder.**

SuperX is a safety-critical Agentic Operating System written in Rust, designed to be a master of every pillar it touches — telemetry collection, RAG / DAG construction over heterogeneous data sources, human-in-the-loop control, and autonomous agentic execution that designs *and* builds complete software products from a single binary.

```
        Telemetry      RAG/Daggify       Design          HITL          Autonomy
        collector  +   tool         +   tool       +   control   +   orchestrator
            └─────────── one Rust binary, modular by deployment ──────────┘
```

## Status

- **`cargo test --workspace`**: 44 / 44 passing
- **`cargo clippy --workspace --all-targets --all-features -- -D warnings`**: clean
- **Verification gate**: enforced before any `ARCHITECTURE.md` update lands
- See [`ARCHITECTURE.md`](ARCHITECTURE.md) for the full vision, roadmap, and excellence-bar criteria.

## Deploy substrate schema (operator one-shot)

The substrate schema is shipped as a single locked DDL file —
[`schema/superx.surql`](schema/superx.surql) — and applied **once** under
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
export SUPERX_SERVICE_PASSWORD='<choose the model service-account password>'
./scripts/deploy-schema.sh
```

After this point, all SuperX code signs in as the `superx` service
account (EDITOR role) — never root. The append-only invariant is
enforced by **kernel-verb discipline**: no kernel verb emits UPDATE or
DELETE statements (SKILL.md §10 / §13).

## Quickstart

```bash
# Provision substrate, seed default agents and capability edges
cargo run -p superx-cli -- bootstrap --tenant demo

# One-shot end-to-end smoke run — bootstrap → ingest → propose → promote
cargo run -p superx-cli -- demo --tenant demo

# Ingest a directory as a DAG of node_code entities
cargo run -p superx-cli -- graphify --path ./my_project --tenant demo

# Inspect what's registered
cargo run -p superx-cli -- list-agents --tenant demo
cargo run -p superx-cli -- list-tools  --tenant demo

# Stream recent telemetry (newest first)
cargo run -p superx-cli -- stats --tenant demo --limit 25

# Run as an MCP server (foreground; talks to Claude Desktop / Claude Code / etc.)
cargo run -p superx-mcp

# Same, with external egress:
SUPERX_EMISSION_API=https://your-ingest/v1/events \
SUPERX_KAFKA_BROKERS=localhost:9092 SUPERX_KAFKA_TOPIC=superx.telemetry \
cargo run -p superx-mcp
```

State persists in `./db/superx.db` (RocksDB) between runs.

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
  superx-kernel/      5-table substrate, sessions, SCD-2, schema, cycle detection
  superx-bootstrap/   First-run substrate provisioning, agent + tool seeding
  superx-ingest/      Universal ingestor: FileSource, JsonSource (+ planned: SQL/Iceberg/RAG)
  superx-compiler/    Tier-aware context distillation, optional LLM-distilled output
  superx-inference/   Local Candle + GGUF runner (zero-cloud inference)
  superx-proposer/    LLM-driven structural-edge proposer
  superx-harness/     wasmtime Meta-Harness (fuel-metered) + promote
  superx-agent/       Capability Governor (handshake, check_capability)
  superx-emission/    Telemetry subscriber + Kafka/HTTP sinks (+ planned: OTLP)
  superx-cli/         Operator CLI: bootstrap / graphify / compile / propose /
                                    evaluate / promote / identify / list-agents /
                                    list-tools / demo / stats
  superx-mcp/         MCP stdio server (lib + bin)
```

## Documentation

- [`ARCHITECTURE.md`](ARCHITECTURE.md) — full vision, principles, invariants, roadmap, operator quickstart.
- GitHub Issues — task tracker for the roadmap, mirrored from the in-repo task list.
- `.claude/skills/zero-trust-execution/SKILL.md` — operator-mandated execution mode for contributors using LLM-assisted development.

## License

[Apache License 2.0](LICENSE). See [`NOTICE`](NOTICE) for attribution.
