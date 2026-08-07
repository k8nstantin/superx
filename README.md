# SuperX

**The Agentic Operating System.** Rust, backed by SurrealDB.

SurrealDB is the OS's filesystem: every fact SuperX knows — agents,
telemetry, state, configuration, the module registry itself — lives in
the substrate. The kernel's core capability is telemetry: from the
moment the OS boots it captures everything every agent on the machine
emits. Every other capability (data fusion, graphify, …) arrives as a
module on top of the kernel.

## Status — reset in progress (G0)

The project was reset from line zero on **2026-08-06**. The complete
v1 First Viable Product (F0–F11, PRs #96–#110, all gates green) is
preserved at the tag
[`archive/pre-reset-2026-08-06`](../../tree/archive/pre-reset-2026-08-06).

The living canon is [`BLUEPRINT.md`](BLUEPRINT.md) — mission, kernel
spec, phase plan (G0–G7), and the decisions record. Schema truth lives
in [`SUPERX_SCHEMA.md`](SUPERX_SCHEMA.md) (G1 design, 2026-08-06).

| Phase | Scope | Status |
|---|---|---|
| G0 | Reset: archive tag, skeleton workspace, blueprint, skill v2 | done (#111) |
| G1 | Schema design session → `SUPERX_SCHEMA.md` v2, operator applies | done (#112) |
| G2 | Kernel substrate verbs + telemetry primitive | this PR |
| G3–G6 | Module system → capture engine → adapters → CLI | planned |
| G7+ | First modules (data fusion, graphify) | planned |

## Deploy the substrate schema (operator one-shot)

```bash
# 1. Start a SurrealDB server on a FRESH v2 path
export SUPERX_ROOT_PASSWORD='<strong root password>'
surreal start --user root --pass "$SUPERX_ROOT_PASSWORD" rocksdb:./db/superx-v2.db &

# 2. Apply the kernel schema once, under root
export SUPERX_KERNEL_PASSWORD='<kernel service-account password>'
./scripts/deploy-schema.sh
```

From that moment the schema is locked (skill §7) and all kernel code
signs in as the `superx_kernel` service account — never root.

## Development gates (binding)

```bash
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
python3 tools/skill_audit.py
```

Contributors using LLM-assisted development operate under
[`.claude/skills/zero-trust-execution/SKILL.md`](.claude/skills/zero-trust-execution/SKILL.md).

## License

[Apache License 2.0](LICENSE). See [`NOTICE`](NOTICE) for attribution.
