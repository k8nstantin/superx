# SuperX

**The Agentic Operating System.** Rust, backed by SurrealDB.

SurrealDB is the OS's filesystem: every fact SuperX knows — agents,
telemetry, state, configuration, the module registry itself — lives in
the substrate. The kernel's core capability is telemetry: from the
moment the OS boots it captures everything every agent on the machine
emits. Every other capability (data fusion, graphify, …) arrives as a
module on top of the kernel.

## v1.0.0 — Capabilities

SuperX v1.0.0 is the first release of the agentic OS (reset 2026-08-06 →
release 2026-08-18; v1-era code preserved at
[`archive/pre-reset-2026-08-06`](../../tree/archive/pre-reset-2026-08-06)).

- **One-command initialize** — `superx --initialize` prompts you to
  create the instance password (any password accepted at this phase),
  starts a local SurrealDB, applies the locked schema, saves
  credentials (`0600`), boots the OS **in the background**, and
  returns your terminal. Idempotent per instance; `superx stop` shuts
  the OS down; `superx status` shows the pid + module lifecycle.
- **Three agent adapters, built from real machine data** — Claude Code
  (full conversations, history backfill + live), Gemini CLI
  (conversations with re-emission dedupe and byte-capped backfill),
  Claude Desktop (honest telemetry — it stores no conversations
  locally). Adding an adapter touches zero engine code.
- **Conversations first-class** — sessions identified
  `<agent>/<uuid7>` (source id as `src=`); `superx read <any-unique-
  fragment> --live` renders history then follows live.
- **Everything queryable by agent_id** — `agents`, `sessions`,
  `actions [--agent] [--live]` over an append-only SCD-2 substrate
  (SurrealDB), UUIDv7 row ids, minimal-privilege service account.
- **Configured, not hardcoded** — `params/superx.json` controls all
  bootstrap config (precedence: flag > env > file > fallback) inside
  the instance layout below; runtime tunables live in the substrate
  as parameters. `superx logs [--follow]` surfaces the OS's own log.
- **Engineering substrate** — failure-isolated module boot, cursor-
  checkpointed lossless restarts, tolerant parsing (unknown shapes
  captured raw, never dropped), kernel self-log, 56 tests, three
  CI-enforced gates.

## The UI

The OS serves its own dashboard — the first module on the module
framework:

```bash
superx modules provision ui   # once: the UI's own database (command history…)
superx ui url                 # → http://127.0.0.1:5150 (attr_ui_port parameter)
```

Open it: **Status** (stat cards, module table, live charts — events/min,
per-agent activity, message roles, boot durations), **Activity** (every
captured action + message streaming via SSE, each row attributed to its
session — who is doing what; filter + pause), **Sessions** (the top-level
grouping: open a session to see EVERYTHING captured for it — messages
and actions merged chronologically, historical then live, auto-scrolling),
**Console** (run the read CLI commands from the browser; history
persisted in the UI's own db). Dev flow: `npm run dev` in
`crates/superx-mod-ui/ui` proxies `/api` to the running OS;
`npm run build` + restart ships it.

Modules are how SuperX grows — see [`docs/MODULES.md`](docs/MODULES.md)
to contribute one (the framework supports several modules of the same
kind side by side: your UI and a contributed one on different ports).

## The entities module — the product graph

Typed entities as graph nodes, native SurrealDB edges, full append-only
history (epic #166). Provision once (`superx modules provision entities`,
then restart), then:

```sh
superx entities create --type product --describe "What this product is." Widget X
superx entities create --type task Build the widget
superx entities link <product-uuid> <task-uuid> --rel linked
superx entities instruct <task-uuid> Read the description; build each component.
superx entities comment <task-uuid> Priority: high.
superx entities tree <product-uuid>          # the whole product, one view
superx entities graph <product-uuid> --json  # subgraph export (nodes + edges)
superx entities attach <product-uuid> ./spec.pdf
superx entities create --type repo --attrs '{"url":"git@github.com:org/widget.git","branch":"main"}' Widget repo
superx entities create --type credential --attrs '{"kind":"ssh","keychain":"widget-deploy-key"}' Widget deploy key
superx entities show <task-uuid>             # state + instructs/comments inline
superx entities types add review --category relation
```

Nodes: product · task · rag · model · document · text · repo ·
credential (extensible at runtime). A credential node stores a secret
REFERENCE (env var / keychain item) in its attributes, linked
`repo —authenticates→ credential` — so an agent following a task edge
to a repo also finds how to authenticate, all in one graph. Long-form text is itself a node linked by role edges
(describes / comments / instructs) — descriptions evolve with their own
history, comments thread. Every change is an INSERT; nothing is ever
lost. Agents executing task nodes from the graph is the next epic.

## Instance layout

```
<home>/                     # --home flag / SUPERX_HOME env, default "."
  params/superx.json        # THE parameter file — all bootstrap config
  logs/superx.log.<date>    # kernel self-log     (superx logs)
  logs/superx-daemon.log    # background OS output (superx logs --daemon)
  logs/surreal-server.log   # database server output
  db/superx-v2.db/          # datastore (rocksdb)
  db/superx-credentials     # service password, 0600
  db/superx.pid             # background OS pid
```

## Quick start / QA protocol

```bash
# ONE command: prompts for a password, provisions everything, boots
# the OS in the background, and RETURNS YOUR TERMINAL.
superx --initialize

# Confirm data is being gathered (no exports needed — credentials are
# read from the instance):
superx status                  # OS: running (pid N) + module lifecycle
superx agents                  # discovered agents + agent_ids + counts
superx sessions | head         # captured conversations
superx actions --live          # watch agents act, as it happens
superx read <fragment> --live  # read a conversation, history then live
superx logs -n 40              # the OS's own log (--follow to tail)

# Lifecycle:
superx stop                    # graceful shutdown (lands in seconds,
                               # even mid-backfill)
superx start                   # boot back (already-initialized instance)
superx restart                 # stop + start — one command to pick up
                               # a freshly built binary
```

After upgrading the binary, `superx restart` moves the background OS
to the new build. Live viewers (`actions --live`, `read --live`)
render client-side — re-run them after an upgrade to pick up renderer
changes.

The first minutes after a fresh initialize are history backfill (your
full Claude Code history; Gemini chat files capped at 8 MiB each,
announced via `backfill_capped` events). Claude Desktop shows zero
sessions by design — its conversations are cloud-side; its telemetry
is captured. `superx boot` (foreground) remains for debugging and
refuses while the background OS runs.

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
