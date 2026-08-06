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
spec, phase plan (G0–G7), and the decisions record. Schema truth will
live in `SUPERX_SCHEMA.md` once the G1 design session produces it.

| Phase | Scope | Status |
|---|---|---|
| G0 | Reset: archive tag, skeleton workspace, blueprint, skill v2 | this PR |
| G1 | Schema design session → `SUPERX_SCHEMA.md` v2, operator applies | next |
| G2–G6 | Kernel verbs → module system → capture engine → adapters → CLI | planned |
| G7+ | First modules (data fusion, graphify) | planned |

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
