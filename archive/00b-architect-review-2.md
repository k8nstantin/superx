# SuperX — Architect's Review #2 (Follow-up after revisions)

**Reviewing:** `01-architecture.md`, `02-state-and-persistence.md`,
`03-execution-and-mcp.md`, `04-graphify-ingestion.md`,
`05-meta-harness.md` — all modified since Review #1 (`00-architect-review.md`).

**TL;DR:** the revisions took most of the Review #1 feedback
seriously. **9 of 12 prior concerns are materially addressed.** Three
new issues are introduced, and the file rename from
`03-execution-and-vfs.md` → `03-execution-and-mcp.md` was not
accompanied by a content rewrite, leaving a **direct contradiction**
with `01-architecture.md`. Five gaps remain.

---

## What was fixed (well done)

| Prior issue | Status | Evidence |
|---|---|---|
| **Critical #3 — No MCP / protocol story** | **Fixed** | `01-architecture.md` §C: "MCP Bridge … native `rmcp` server over Unix Domain Sockets or gRPC." MCP is now the integration ABI. |
| **Critical #4 — No multi-tenancy** | **Fixed** | `02-state-and-persistence.md`: `tenant_id` mandatory on every row; `fn::current_at` now takes `$tenant`; rehydration filters by tenant. |
| **Critical #5 — LLM in primary security path** | **Fixed** | `01-architecture.md` §3.2: "Deterministic Security Check … (No slow LLM in the authorization path)." Capability manifests are pre-compiled. |
| **Critical #6 — Auto-deploy of agent-written code** | **Fixed** | `05-meta-harness.md` §2.3: "SuperX strictly avoids the supply-chain attack surface … Only upon explicit human approval is the `supersede()` transaction fired." |
| **G1 — No structured prompt discipline** | **Acknowledged** | `05-meta-harness.md` §1: "System Instructions: The base prompt (using strict XML/tagged prompt discipline)." But not yet defined (see new issue N3). |
| **G2 — Sparse system-agent roster** | **Expanded** | `01-architecture.md` §E: Proposer, Evaluator, Classifier, Summarizer, Redactor — 5 named. |
| **G4 — Emission pipeline thin** | **Partially fixed** | Redactor in pipeline before Kafka publish. Full outbox engineering still missing — see G4-remaining below. |
| **G6 — No secrets management** | **Fixed** | `02-state-and-persistence.md` §2.2: dedicated `secrets` table, KMS-backed (AWS KMS, HashiCorp Vault). |
| **Critical #1 — Dual-state sync spec** | **Improved** | Epoch-based sync replaces synchronous per-op writes; crash recovery is now cold-wins ("un-flushed CRDT operations are lost"). Major progress, but still incomplete — see C1-remaining. |

That's an excellent revision pass. The system is materially safer to
ship now than it was 16 hours ago.

---

## The critical inconsistency — `03-execution-and-mcp.md`

The file was **renamed** from `-vfs.md` to `-mcp.md`. But the content
is unchanged. **The doc title is still "Execution Layer & The
Virtual Filesystem (VFS)"** and the entire §4 is "The Virtual
Filesystem (VFS) Bridge" with FUSE / `fuser` / VFS-everywhere
language.

This directly contradicts the architecture overview:

| `01-architecture.md` says | `03-execution-and-mcp.md` says |
|---|---|
| "**The MCP Bridge** *(Replaces the legacy FUSE VFS.)* A native rmcp server exposed over Unix Domain Sockets or gRPC." | "Using the `fuser` crate, the SuperX Rust kernel implements a FUSE (Filesystem in Userspace) driver. It mounts the abstract, in-memory CRDT graph as a physical volume on the host machine (e.g., `/var/run/superx/vfs/`)." |
| "Docker heavy-lifters … interact with the OS via the **MCP Bridge** or Virtio-FS, completely isolated from host paths." | "Docker containers cannot access the Rust kernel's shared memory. They must interact with the Living Graph through the **VFS bridge**." |

This is a **showstopper inconsistency.** A contributor reading both
docs cannot answer: do we use MCP or FUSE? The two specs are
incompatible.

**Recommendation — must fix:**

1. **Rewrite `03-execution-and-mcp.md` from scratch.** Pivot the doc
   to be MCP-centric:
   - Section 1: `AgentRuntime` trait (keep).
   - Section 2: Wasm runtime — call out the **MCP server-in-Wasm
     pattern** (Wasm tools expose MCP interfaces over shared-memory
     channels; speed AND ecosystem compat).
   - Section 3: Docker runtime — drop the VFS-as-bridge language;
     specify "Docker containers connect to the kernel via rmcp over
     Unix Domain Sockets (`/var/run/superx/mcp.sock`) bind-mounted into
     the container."
   - Section 4: External tooling (IDEs, third-party agents) — connect
     via the same rmcp endpoint over UDS or gRPC.
   - **Optional Section 5:** "VFS Projection (Inspection Only)" —
     keep FUSE as a *read-mostly debug projection* for `ls
     /SuperX/graph/...` exploration. Document the platform caveats
     (macFUSE deprecation, Windows-no-FUSE, Docker `--privileged`
     requirement). Make it clearly opt-in, not the bridge.

2. **Pick one between `petgraph` and "concurrent multi-maps (ECS-style)."**
   `01-architecture.md` §A says concurrent multi-maps. `03` line 30
   still says "the `petgraph` Living Graph resides." `04` line 19 says
   "multi-map Living Graph." This needs to converge. If you've moved
   off petgraph (good — petgraph isn't designed for concurrent
   mutation), pick the actual structure (`dashmap`-backed ECS?
   `evmap`? `concurrent_hashmap`?) and name it everywhere.

3. **Justify Virtio-FS for Docker** if it stays, or drop it.
   Virtio-FS is a VM-level paravirtualized filesystem — it requires
   a hypervisor (KVM/QEMU). It's not a Docker primitive; you'd be
   running Docker inside a VM and exposing Virtio-FS that way. If
   that's the intent, say so explicitly. Otherwise, MCP-over-UDS is
   the simpler answer and you can drop Virtio-FS from the doc.

---

## Critical #1 remaining — dual-state sync still under-specified

Epoch-based sync addresses **write amplification** (the obvious
problem). It does not address several real ones:

| Sub-problem | Status after revision | What's missing |
|---|---|---|
| Loro operation log persistence across crashes | Crash-discards-RAM is acceptable for single-node single-writer; but for **cross-process collaboration** (human in IDE + agent in Wasm both editing) the CRDT op log is *the state*. Cold-wins on crash means concurrent edits that hadn't yet flushed are silently dropped. | Document this trade-off explicitly. If cross-process CRDT collaboration is a feature (the `02` doc claims it is), the op log MUST be persisted incrementally (snapshot every N ops to SurrealKV) so loro can re-converge on restart. Otherwise mark CRDT collaboration as "single-process, in-memory only" — a different feature. |
| Multi-node CRDT sync | Not addressed | If SuperX scales to multiple nodes, how do their loro stores converge? A sync protocol (gossip? NATS? hub-and-spoke?) is needed. Or commit to single-node v1 explicitly. |
| Partial Epoch failure | Not addressed | The Epoch sync is a multi-table SurrealDB transaction. If one table's supersede fails mid-transaction, the kernel must either roll the whole Epoch back or mark the entity diverged. Today's spec is silent. |
| Time-travel within an Epoch | Not addressed | `fn::current_at($t)` returns SCD-2 state. But between Epoch flushes, the CRDT in RAM has newer state. A query for "as of right now" gives the LAST-EPOCH state, not actual current. Tradeoff: explain, accept, or define a `fn::current_now()` that reads CRDT. |
| Epoch trigger heuristics | Mentioned ("file save, turn completion, 5-second debounce") but not formalized | Define the trigger taxonomy as a config table (parameterized, not hard-coded) — what events trigger an Epoch, with what priority, with what max-Epoch-size cap. |
| CRDT op-log shallow pruning | Mentioned but risky | Pruning the loro op log compromises merge semantics for any concurrent op that arrives with a vector clock older than the pruned horizon. Spec the safe-pruning algorithm. |

**Recommendation:** write a dedicated `06-state-sync.md` document
that answers each row above. Until that exists, the dual-state
architecture remains the project's largest correctness risk.

---

## Gaps from Review #1 still outstanding

### G3 — Hardware-tier strategy (still missing)

`01-architecture.md` §E mentions "local ML models in Wasm" with no
detail on what runs on what hardware. A CI server with no GPU and an
M4 Max are both target hardware. Without a tier table:

- Embedding generation may be infeasible on T0 (laptop CPU, < 8 GB RAM)
- 12B Gemma is out of reach for the majority of dev laptops
- Local Wasm ML inference is genuinely tight on memory — `candle`
  models run, but at notable cost

**Recommendation:** add `06-hardware-tiers.md` (or section in `01`)
with T0–T5 like the SuperX spec, with model-residency rules per tier
and graceful degradation for missing capability.

### G5 — Interactive (human) activity capture (still missing)

The docs focus on agents. Humans appear only as IDE-users via the
former VFS (now MCP). The dashboard / CLI / API click-and-edit
events still don't have an event-kind taxonomy
(`interactive.viewed`, `interactive.edited`, `interactive.searched`,
…). Without this, the firehose is half a story — you can't
diff-and-replay a debugging session of "what did the human do between
two agent failures."

### G7 — Failure-mode matrix (still missing)

Open questions still:

| Failure | Specified behavior |
|---|---|
| SurrealDB embedded corruption | ? |
| Wasm runtime panic in an extractor | ? |
| Local model OOM | ? |
| Kafka emission down for 1h | ? |
| Loro op log overflows RAM | ? |
| MCP UDS socket unavailable to Docker | ? |
| Epoch sync fails mid-transaction | ? |
| Tenant deletion (compliance request) | ? |

A production agentic OS needs a deliberate answer for each.

### G8 — Resource quotas (still missing)

`resource_account` (token, cost, wall-clock, CPU per tenant/agent)
not addressed. Runaway-agent risk persists.

### G9 / G11 / G12 — Cargo layout + library inventory + React stack

Still no explicit Cargo workspace. `rig-core` is not mentioned (will
you author the model-routing layer? That's months of work
re-inventing what `rig` already does). React stack beyond xyflow
unspecified (Vite? Tailwind? shadcn? TanStack? BlockNote?).

**Recommendation:** add three short docs / sections:
- `07-cargo-workspace.md` — every crate with one-line role
- `08-libraries.md` — `rig-core`, `tantivy`, `fastembed-rs`, `async-nats`, `whisper-rs`, `pulldown-cmark`, `git2`, etc., with rationale
- `09-react-stack.md` — Vite + Tailwind v4 + shadcn/ui + TanStack Router/Query/Table + xyflow + ECharts + BlockNote + Zustand + react-hook-form + CodeMirror (XML editor for prompts)

### G4 remaining — emission engineering depth

Current spec: "background task tails `execution_log`, Redactor scrubs,
Kafka publishes." Still missing:

- **Cursor persistence** (how is the high-water-mark stored across kernel restarts? — SurrealKV key suggested)
- **At-least-once + idempotent consume** (log_uid as dedup key)
- **Backpressure / drop-to-disk** when Kafka is down — what disk path? what cap?
- **Topic naming convention** (suggest: `superx.activity.<event_family>.<tenant>`)
- **Schema evolution** (Variant/JSON for payload; Avro / Protobuf consideration)
- **Multi-sink fan-out** (Vector.dev sidecar pattern when shipping to Iceberg + Datadog + Loki at once)

A dedicated `10-emission.md` would close this.

---

## New issues introduced by the revisions

### N1 — Inconsistent state-structure naming

| Doc | Term used |
|---|---|
| `01-architecture.md` | "concurrent multi-maps (ECS-style) holding Loro CRDTs" |
| `02-state-and-persistence.md` | "ECS / Multi-Map" |
| `03-execution-and-mcp.md` | "the `petgraph` Living Graph resides" |
| `04-graphify-ingestion.md` | "patches the multi-map Living Graph in RAM" |

Pick one. If petgraph is out, every reference must update. If
petgraph is in (for read-side graph algorithms like Leiden), say so
explicitly and define the relationship to the ECS multi-map.

### N2 — Virtio-FS mention is unjustified

`01-architecture.md` §B says Docker interacts via "MCP Bridge or
Virtio-FS." Virtio-FS is a VM paravirtualized fs requiring KVM/QEMU
— it's not a Docker primitive without a containing VM. Either:
- Drop the mention (MCP over UDS bind-mounted into the container is
  the simpler path), OR
- Document the assumed deployment topology (Docker inside Firecracker
  / KVM, with Virtio-FS plumbing exposed)

### N3 — "Strict XML/tagged prompt discipline" is named but not defined

`05-meta-harness.md` §1 introduces the term but doesn't:
- Define the canonical tag vocabulary (`<system>`, `<context>`,
  `<instructions>`, `<output_schema>`, `<example>`, `<constraints>`,
  `<tools>`, `<memory>`, `<variable name="X"/>`, …)
- Specify write-time validation (XML parse + tag-vocabulary check)
- Address prompt templating (parameter slots, includes, version
  history via SCD-2)

**Recommendation:** add `11-prompt-discipline.md` with the
vocabulary, validation rules, and template format.

### N4 — "Production environment" is referenced without a schema definition

`05-meta-harness.md` §2.3 distinguishes auto-promotion from
human-approved promotion based on environment. But where is the
"environment" attribute defined? Per-tenant config? Per-entity tag?
The promotion gate needs a deterministic answer:

```
IF target_environment = 'production'
  → emit approval.requested + wait
ELSE
  → auto-promote
```

Document the environment dimension (suggest: a field on `tenant` and/or
a tag on `harness` entity rows).

### N5 — "Shallow pruning" of Loro op log

`02-state-and-persistence.md` §2.4: "Once safely stored in SCD-2, the
hot-path Loro CRDT history can be shallowly pruned to preserve RAM."

Pruning a CRDT op log can break merge with any op that arrives with a
vector clock older than the pruned horizon. For collaborative-edit
scenarios with offline peers (an agent that was paused for an hour
then resumes with stale ops), pruning is dangerous. Document the safe
algorithm or scope CRDT use to in-process only (where all ops are
trivially newer than any pruning horizon).

---

## Net assessment

**Revision quality: high.** The author took the Review #1 feedback
seriously and shipped meaningful changes within hours: MCP adopted,
multi-tenancy added, secrets table, human-in-the-loop promotion,
deterministic capability check, expanded system-agent roster,
epoch-based sync, redactor in emission pipeline, XML prompt
acknowledgment. Of 17 numbered items in Review #1, 9 are materially
fixed, 3 are partially addressed, 5 are unchanged. That's a good
ratio for one revision.

**Remaining risk: the `03` doc is now lying.** The file rename without
content rewrite is the single most pressing fix. It creates a
contradiction between architecture doc and execution doc that will
mislead any contributor who reads them in order. Fix in the next
revision.

**Second-priority risk: dual-state sync.** Epoch-based sync is
progress but not closure. The Loro op-log persistence question + the
multi-node sync question + partial-Epoch-failure semantics still need
explicit answers before code-locks. Spike the correctness story
before committing the architecture.

**Lower-priority but real: hardware tiers, failure modes, quotas,
interactive capture, full library inventory, Cargo workspace, react
stack inventory.** Each is a 1-3 page doc. None blocks Phase 1 but
all block production-ready Phase 4+.

---

## Specific deliverables I'd request in Revision #3

1. **Rewrite `03-execution-and-mcp.md`** — MCP-first, drop FUSE-as-bridge language (keep FUSE as opt-in inspection projection if you must); pick petgraph-or-multi-map and commit; justify or drop Virtio-FS.
2. **`06-state-sync.md`** — Loro op-log persistence policy; multi-node sync protocol (or scope to single-node v1); partial-Epoch handling; Epoch trigger catalog as data; safe pruning algorithm.
3. **`07-hardware-tiers.md`** — T0–T5 tier table + auto-detection + graceful degradation.
4. **`08-failure-modes.md`** — the failure matrix (8 rows above).
5. **`09-cargo-workspace.md`** — every crate, one line each.
6. **`10-libraries.md`** — full Rust + React inventory with rationale per pick.
7. **`11-prompt-discipline.md`** — XML tag vocabulary + write-time validation + template format.
8. **`12-emission.md`** — cursor persistence, idempotent consume, backpressure, topic naming, multi-sink.
9. **`13-quotas-and-resource-accounting.md`** — `resource_account` table usage, per-tenant budgets, enforcement layer.
10. **`14-interactive-capture.md`** — `interactive.*` event taxonomy for human dashboard actions.

That's 9 docs of 1–3 pages each. Each is well-scoped and
self-contained. Allocate one engineering-day per doc; total ~2 weeks
to close all gaps. After that, you're ready to scaffold Cargo and
start writing code on a foundation that won't bite you in Phase 4.

The bones remain good. The revisions are real progress. Three more
focused doc-passes and SuperX is production-design-ready.
