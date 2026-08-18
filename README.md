# SuperX

**The Agentic Operating System.** Rust, backed by SurrealDB.

SurrealDB is the OS's filesystem: every fact SuperX knows — agents,
telemetry, state, configuration, the module registry itself — lives in
the substrate. The kernel's core capability is telemetry: from the
moment the OS boots it captures everything every agent on the machine
emits. Every other capability (data fusion, graphify, …) arrives as a
module on top of the kernel.

## Status — FVP complete

The project was reset from line zero on **2026-08-06** (v1 preserved
at [`archive/pre-reset-2026-08-06`](../../tree/archive/pre-reset-2026-08-06));
the **v2 First Viable Product shipped 2026-08-07**. The living canon
is [`BLUEPRINT.md`](BLUEPRINT.md); schema truth is
[`SUPERX_SCHEMA.md`](SUPERX_SCHEMA.md) (v2.1).

| Phase | Scope | Status |
|---|---|---|
| G0 | Reset: archive tag, skeleton workspace, blueprint, skill v2 | done (#111) |
| G1 | Schema v2 + v2.1 (conversations first-class) | done (#112, #115) |
| G2 | Kernel substrate verbs + telemetry primitive | done (#113) |
| G3 | Module system + in-kernel boot orchestrator | done (#116) |
| G4 | Capture engine + Claude Code adapter | done (#117) |
| G5 | Gemini CLI + Claude Desktop adapters | done (#118) |
| G6 | CLI: boot / status / agents / actions / sessions / read — **FVP** | this PR |
| G7+ | First modules (data fusion, graphify) | planned |

## FVP test protocol (the live demo)

```bash
# Terminal 1 — ONE command: prompts you to create the instance
# password (any password will do at this phase), starts a local
# SurrealDB, applies the schema, boots the OS, discovers every coding
# agent on the machine (Claude Code, Gemini CLI, Claude Desktop),
# backfills conversation history, then captures live. Ctrl-c to stop.
# Re-running an initialized instance skips provisioning and boots.
superx --initialize

# Terminal 2 — watch what agents are doing, as it happens. No
# exports needed: commands read the instance credentials file written
# by --initialize (env SUPERX_KERNEL_PASSWORD still wins if set).
superx actions --live

# Inspect what the OS knows.
superx status                      # module + adapter lifecycle
superx agents                      # discovered agents, agent_ids, counts
superx sessions                    # every captured conversation
superx sessions --agent gemini_cli # per-agent

# Read a conversation — full history, then follow it live.
superx read <session-name-or-suffix> --live

# Query per agent (agent_id = the entity id shown by `superx agents`).
superx actions --agent claude_code -n 50
```

Working in Claude Code or Gemini CLI while `boot` runs streams new
messages into the substrate within seconds; `read --live` shows the
conversation as it continues.

## Manual provisioning (alternative to `--initialize`)

```bash
# 1. Start a SurrealDB server on a FRESH v2 path
export SUPERX_ROOT_PASSWORD='<root password>'
surreal start --user root --pass "$SUPERX_ROOT_PASSWORD" rocksdb:./db/superx-v2.db &

# 2. Apply the kernel schema once, under root
export SUPERX_KERNEL_PASSWORD='<kernel service-account password>'
./scripts/deploy-schema.sh
```

Either way the schema is locked from first apply (skill §7) and all
kernel runtime code signs in as the `superx_kernel` service account —
never root (`--initialize` uses the root password once, in-process,
and never stores it).

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
