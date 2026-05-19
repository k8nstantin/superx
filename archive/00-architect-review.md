# SuperX — Applications Architect's Review

**Reviewing:** `01-architecture.md`, `02-state-and-persistence.md`,
`03-execution-and-vfs.md`, `04-graphify-ingestion.md`,
`05-meta-harness.md`.

**Reviewer's stance:** experienced applications architect. Goal is to
surface gaps + risks honestly while acknowledging the bold ideas that
work. Where SuperX does something better than what I'd otherwise
recommend, I say so.

---

## TL;DR

| Verdict | Component |
|---|---|
| **Keep, it's a real differentiator** | Continuous graph ingestion via Wasm micro-extractors (< 10ms); SCD-2 as the cold-path ledger; Meta-Harness loop on time-traveled state; **xyflow** for the React spatial canvas; **loro** CRDT for genuine concurrent edits |
| **Re-scope, current spec overpromises** | Dual hot/cold state (CRDT ⇄ SCD-2 sync); FUSE-based VFS as the primary bridge; "OS writes its own code" auto-deploy; "microsecond execution" framing |
| **Add — currently missing** | MCP protocol surface; tenant model; secrets management; structured (XML) prompt discipline; system-agent inventory beyond Proposer/Evaluator/Warden; hardware-tier degradation; interactive activity capture (humans, not just agents); explicit failure-mode matrix; Cargo workspace layout |
| **Risks that need an explicit answer** | CRDT operation log persistence on restart; cross-platform FUSE viability; LLM-in-the-security-loop determinism; multi-tenancy retrofit cost |

---

## What works (and you should keep doing)

### 1. SCD-2 as the cold ledger
You've internalized the universal invariant — every state change is a
new row, `valid_to` stamps the prior, no updates or deletes. This is
right. It pays for time travel, audit, replay, Meta-Harness, compliance
— all free. Don't compromise.

### 2. Continuous Wasm extractor swarm for code ingestion
Tree-sitter → tree-sitter-graph → stack-graphs in Wasm modules that
share memory with the kernel for zero-copy patches, < 10ms latency
from Cmd+S to indexed graph. This is **excellent**. The decision to run
extractors as Wasm rather than native processes is a real safety +
isolation win. It's better than what I'd recommended (`graphify-rs` as
an external MCP driver) for code-graph specifically, because the
kernel-shared-memory path is genuinely faster than any RPC.

### 3. Meta-Harness on time-traveled reality
`fn::current_at()` letting the Proposer "rewind to the exact
millisecond before failure" — this is the Meta-Harness done right. Most
agent frameworks try to replay from logs and never get faithful state.
SCD-2 makes it possible. Lean into this.

### 4. xyflow for the React canvas
A node-edge editor library (xyflow / react-flow) is the right choice
for a spatial graph dashboard — better than my prior recommendation of
Apache ECharts graph series. xyflow handles drag/drop, viewport
control, custom node renderers, edge labels, minimaps natively.
**Keep this.**

### 5. CRDT (loro) for collaborative edits
Genuine concurrent editing by multiple agents + humans on the same
state is a real need (agent editing a harness while a human reviews it
in the dashboard). Loro is the right tool for the job. **Don't lose
this.** (But see Critical Issue #1 — the hot/cold sync needs more
spec.)

---

## Critical issues (would sink the project as currently spec'd)

### CRITICAL #1 — Dual hot/cold state is the trickiest part of the design and it's under-specified

The hybrid architecture has petgraph + loro in RAM, SurrealDB SCD-2 on
disk. The doc says "Asynchronously, but immediately, a
`superx_db::supersede()` call is made." That handwaves the hardest
problem in the system.

**Risks:**

| Risk | Today's spec answer | What's actually needed |
|---|---|---|
| CRDT mutation applies in RAM; SurrealDB write fails | Not specified | Compensating action: roll the CRDT back? Mark the entity "diverged"? Block further mutations until resync? |
| Multiple CRDT changes batched faster than SurrealDB can absorb | Not specified | Backpressure: how does the hot path know SCD-2 is behind? Bounded queue? Drop oldest? Block? |
| Crash during a `supersede()` transaction after CRDT applied but before commit | Not specified | Rehydration finds prior state; CRDT history (loro op log) must be persisted independently so loro can re-converge after recovery |
| **Loro's operation log itself is state.** Loro maintains operation IDs, peer IDs, vector clocks. If you persist only the materialized snapshot to SCD-2, you lose merge capability after restart. | Not addressed | Persist BOTH the snapshot (in SCD-2) AND the loro op log (in a separate SurrealKV store or as `string_value` rows on the entity). On rehydrate, replay op log. |
| Two peers connected to two SuperX nodes both edit the same entity | Hand-waved as "CRDTs solve it" | True only if both nodes have a connected loro session; cross-node CRDT requires a sync protocol (NATS? gossip? hub-spoke?) not in the doc |
| Time-travel queries — does `fn::current_at($t)` give the CRDT state at $t, or the SCD-2 state? | Not addressed | They diverge: CRDT may have applied mutations not yet in SCD-2. Need to define which is canonical for which query |

**Recommendation:**
- Write a dedicated "06-state-sync.md" document with **explicit answers** for every divergence case.
- Persist the loro op log alongside the SCD-2 snapshot. Without it, restart loses CRDT merge identity.
- Define a clear primacy: either **hot-wins** (CRDT is canonical; SCD-2 is a derived view eventual-consistently lagged) or **cold-wins** (SCD-2 is canonical; CRDT is a high-speed cache that rehydrates). The current spec mixes both modes.
- Honest alternative: **start with SCD-2 only**, with native SurrealDB LIVE SELECT for low-latency reads. Introduce loro CRDTs only when you've **measured** a real bottleneck. The complexity tax of dual state is enormous.

### CRITICAL #2 — FUSE / VFS has serious cross-platform and operational issues

The VFS-via-`fuser` is presented as a universal bridge. Reality:

| Platform | FUSE story |
|---|---|
| Linux | Native, usually fine. Some restrictions in containers (need `--privileged` or specific capabilities — production deployments often disallow). |
| macOS | Requires **macFUSE** — a third-party kext. As of recent macOS versions, kexts require admin install + reboot, and Apple is actively deprecating kext support in favor of file-provider extensions. macFUSE breakage on every macOS major release is common. |
| Windows | FUSE doesn't exist. The closest analog is **WinFsp** (different API). `fuser` does not support Windows. |
| Docker | Mounting host FUSE into a container requires `--privileged` or `--cap-add SYS_ADMIN`. Many production policies forbid both. |

**Performance:** every read/write is a userspace round trip through
the kernel. Tools like `grep -r foo` against a real disk vs. against
FUSE differ by 10–100x. Tools that use `mmap`, `O_DIRECT`, locking,
xattrs may fail outright.

**Recommendation:**
- **Demote FUSE from "the universal bridge" to "an optional inspection projection."** Useful for `ls /SuperX/graph/concepts/auth` exploration in a terminal, debug, demos.
- **Primary IPC for tools should be MCP** (rmcp), which works cross-platform, is the agent-ecosystem standard, and doesn't require admin install.
- **Docker container integration:** rmcp via stdio or SSE, not FUSE mount.
- **Wasm tool integration:** shared memory + host functions, not FUSE.
- If you keep FUSE-everywhere as a vision, document a fallback for macOS-without-macFUSE and Windows users (a normal HTTP API that returns the same JSON the FUSE projection would synthesize).

### CRITICAL #3 — No MCP / no protocol story

`01-architecture.md` mentions "Wasm WASI host functions" and "Docker
via VFS" as the agent ABIs. **MCP / rmcp is not mentioned anywhere in
the docs.**

This is a major omission:
- **The entire agent ecosystem speaks MCP.** Claude Code, Cursor, Aider,
  Continue, Goose, Cline — all use MCP for tool calling.
- **There are dozens of community MCP servers** (GitHub, Slack, Linear,
  Jira, Notion, Atlassian, browser automation, code-graph) that you'd
  use immediately if you spoke MCP — and have to re-implement if you don't.
- **Your harness format mentions "Tool Manifest"** — but doesn't say
  what protocol tools speak. Without MCP, you're defining a custom ABI
  for every capability, which is the work of years.

**Recommendation:**
- **Adopt MCP as the tool-calling ABI.** `rmcp` (3.4k★, v1.7) is the
  Rust SDK. `#[tool]` macros auto-generate JSON Schema.
- **Wasm tools expose themselves as MCP servers.** Same protocol, just
  hosted in-process. The Wasm-with-shared-memory benefit is preserved
  for trusted tools; the MCP wire protocol is preserved for third-party
  interop.
- **Native + Docker tools are also MCP servers** (child processes or
  HTTP/SSE).
- **One protocol everywhere.** Each tool implementer learns one ABI.

### CRITICAL #4 — Multi-tenancy is not addressed

Nowhere in the 5 docs does "tenant" appear. The system seems implicitly
single-org. This is the single most painful retrofit you can do.

**Recommendation:**
- Add `tenant: record<tenant>` to every table on day 1.
- Define the deployment topology: one DB many tenants with
  record-level access (lower ops) vs. DB-per-tenant (hard isolation).
- Capability checks must include tenant. Cross-tenant capability
  grants are an explicit pattern (with audit) not an accident.

### CRITICAL #5 — Semantic Warden as a primary security gate is too risky

`01-architecture.md`: *"The Semantic Warden (local Gemma via `candle`)
evaluates the intent against fuzzy semantic grants."*

LLM-based capability checks have:
- **Non-determinism**: re-evaluating the same call may give different
  answers (sampling temperature, model updates).
- **Adversarial vulnerability**: a hostile prompt can talk the warden
  into a wrong decision.
- **Audit opacity**: "this call was denied because the warden didn't
  like the vibe" is unauditable.
- **Latency**: every tool call paying inference latency on the security
  path is a non-starter.

**Recommendation:**
- **Deterministic capability check is THE primary gate.** Read `grant`
  rows; allow/deny without any LLM in the path.
- **Semantic warden runs as DEFENSE-IN-DEPTH**: it watches the pattern
  of allowed calls and flags anomalies (writes `policy.violation`
  events for review). It does **not** block in line.
- This is the pattern in `SuperX/docs/intelligence.md` —
  `superx-anomaly-watcher` is non-blocking by design.

### CRITICAL #6 — "OS writes its own code" auto-deploy needs a human gate in production

`05-meta-harness.md`: *"The OS literally writes its own code, compiles
it to Wasm, tests it against time-traveled reality, and deploys it
without human intervention."*

This is technically achievable and rhetorically powerful. It's also
how you get an agentic supply-chain attack:
- A proposed Wasm tool passes the evaluation set (which the proposer
  chose) but fails in production on inputs the eval set didn't cover.
- A subtle regression in the new tool corrupts an entity that the
  evaluator wasn't grading against.
- A compromised proposer agent writes a backdoored tool that scores
  well on graded benchmarks.

**Recommendation:**
- **Auto-promote only in `tenant.environment = "dev"`/`"sandbox"`.**
- **In `production`, the promotion gate emits an `approval.requested`
  event** and the new harness/tool waits for a human grant.
- Document this explicitly. The "OS writes its own code" claim becomes
  the **dev experience**; production is gated.

---

## Moderate gaps (real but addressable)

### G1 — No structured prompt discipline
Prompts in `05-meta-harness.md` are described as "the base prompt" —
free-form text. Lessons from OpenPraxis and SuperX: prompts should be
**XML/tagged** (`<system>`, `<context>`, `<instructions>`,
`<output_schema>`, `<example>`, `<constraints>`, `<variable
name="X"/>`) with write-time validation. Catch malformed prompts
before they enter the DB.

### G2 — System-agent roster is incomplete
The docs name Proposer, Evaluator, Semantic Warden. Missing kernel
hygiene agents:
- **Classifier** — accurate failure-mode labels (`max_turns`,
  `timeout`, `process_error`, `cost_blown`, `prompt_confusion`, …)
- **Summarizer** — rolling session summaries to bound context cost
- **Redactor** — PII scrubbing at the emitter boundary
- **Consolidator** — Letta-style memory consolidation (episodes → facts)

Without these, the firehose is verbose, classifications drift, and
emission leaks PII.

### G3 — No hardware-tier strategy
Where does Gemma 3 12B run? What if the user has only a laptop CPU?
What if there's no GPU at all? The OS needs auto-detection +
graceful degradation (T0–T5 tiers like SuperX), or it won't run on
half the target hardware.

### G4 — Activity emission is one paragraph
`01-architecture.md`: *"A tokio background task that tails the
SurrealDB `execution_log` and CRDT mutation events. Uses `rdkafka` to
stream all events to a centralized Kafka cluster."* That's it.

Missing: outbox cursor persistence, ordering guarantees,
at-least-once + idempotent-consume semantics, backpressure /
drop-to-disk, topic naming convention, schema evolution, PII
scrubbing position in the pipeline, multi-sink fan-out, what happens
when Kafka is down.

This is a full engineering subsystem. Treat it as one. (See
`SuperX/docs/roadmap.md` and `SuperX/docs/architecture-review.md`
sections on the emission pipeline.)

### G5 — Interactive (human) activity capture is missing
The 5 docs focus on agents. Humans appear only as developers editing
files in VS Code (caught via VFS). Missing: dashboard clicks, edits,
queries, grants, comments, exports. These should land in
`execution_log` under `interactive.*` event kinds so the firehose
captures human + agent activity uniformly. Without it, the audit
story is half-finished.

### G6 — No secrets management
API keys, OAuth tokens, model credentials, signing keys, TLS certs.
Where do they live? Nowhere in the docs. Putting them in entity rows
or string_value is a leak waiting to happen.

**Recommendation:** dedicated `secret` table with:
- Encrypted ciphertext + KEK reference (KMS / OS keychain / Vault / SOPS)
- Audited resolution (every read emits `secret.resolved` to the
  firehose, **never the plaintext**)
- Capability gate (`resolve_secret`) + per-secret `permitted_actors`
- Rotation tracking

### G7 — Failure-mode matrix undefined
The docs don't say what happens when:

| Subsystem failure | Behavior expected? |
|---|---|
| SurrealDB embedded is corrupted | Crash + restore from emitter sink? Crash + restart hot-only? |
| Wasm runtime panics in an extractor | Restart it? Mark file as un-indexed? |
| Local Gemma OOM | Fall back to which model? Disable which system agents? |
| Kafka emission is down for an hour | In-memory queue? Disk spill? At what size do we drop? |
| FUSE driver crash | Does the kernel die or restart the driver? |
| Two CRDT peers diverge | Latest-write-wins? Merge conflict surfaced to user? |

A production agentic OS needs a deliberate answer for each.

### G8 — No multi-tenant resource limits / quotas
Without `resource_account` rows + per-tenant budgets, a runaway agent
can burn through every dollar in the AI account before anyone
notices. Token / cost / wall-clock / CPU caps need to be enforced
synchronously at the tool-call boundary.

### G9 — No Cargo workspace layout
What crates exist? Which are libraries vs binaries? How do they depend
on each other? Without this, contributors can't navigate the code.

**Recommendation:** explicit workspace document like SuperX has —
named crates with one-line role descriptions.

### G10 — No mention of `rig-core` for provider routing
`rig-core` (7.3k★) is the mature Rust agent framework with 20+ LLM
provider abstractions and conversation memory traits. Rolling your
own is reinventing months of work that's already done. Mention how
you route between Anthropic / OpenAI / Vertex / Ollama / Mistral.rs
without authoring the abstraction.

### G11 — `single-clustering` is the only library named for Leiden — good — but the broader inventory is missing
What's the Tantivy story (full-text search)? Embedding backend
(fastembed-rs? Candle direct?). Markdown / PDF / DOCX parsers. NATS
for cross-agent RPC. State the **full** library inventory so
contributors don't reinvent.

### G12 — xyflow is named but the broader React stack isn't
Vite? Tailwind? shadcn/ui? TanStack Router/Query/Table? BlockNote for
prompts? Without the stack, the frontend is undecided.

---

## Recommendations: a converged path forward

If SuperX continues as its own product, here are the concrete moves to
make it production-ready. If SuperX and SuperX merge (or you pick the
best of both), the same recommendations apply.

### Adopt from SuperX into your unified design
1. **Wasm extractor swarm with shared memory** for code graph — keep the < 10ms latency claim.
2. **xyflow** as the React graph canvas (replace ECharts graph series).
3. **CRDT support as an opt-in collaboration layer** — but persisted properly (snapshot + op log) and only for entities with concurrent-edit needs.
4. **Continuous indexing as a kernel-internal primitive**, not as an offline batch.

### Drop or re-scope from SuperX
1. **FUSE as primary IPC** — demote to optional inspection projection.
2. **"OS writes its own code without human intervention"** — gate production promotion behind explicit approval.
3. **Wasm-as-the-only-fast-path** — Wasm for trusted internal tools, MCP for everyone else's tools.
4. **Semantic Warden as primary gate** — defense-in-depth only.

### Add to SuperX
1. **MCP / rmcp protocol surface** for agent tool calling.
2. **`tenant` field on every table** — multi-tenant from day 0.
3. **`secret` table** with encrypted ciphertext + KMS backends.
4. **XML/tagged prompt discipline** with write-time validation.
5. **Five+ system agents** beyond Proposer/Evaluator/Warden:
   classifier, summarizer, redactor, consolidator, anomaly-watcher.
6. **Hardware-tier auto-detection (T0–T5)** with graceful degradation.
7. **Interactive activity capture** (`interactive.*` event kinds) for
   human dashboard actions.
8. **Failure-mode matrix** — explicit answers for every subsystem
   outage.
9. **Resource accounting + quotas** enforced at the tool-call boundary.
10. **Outbox emission engineering** — cursor persistence, idempotent
    consume, backpressure, multi-sink, PII scrubbing position.

### Verify by spike (before code-locking)
1. **State sync correctness** — write a test harness that randomly
   crashes mid-supersede() and verifies the rehydration produces
   correct + consistent state. If this is flaky, redesign the dual
   state.
2. **FUSE on every target platform** — does macFUSE install cleanly
   on a fresh macOS 15? Does it survive a macOS update? Does
   `--privileged`-less Docker mount work? Decide based on data.
3. **Wasm performance** — measure end-to-end agent loop latency
   (Wasm-bound) vs. native rmcp. If the gap is < 2x, the complexity
   isn't worth it for non-extractor agents.
4. **CRDT merge semantics under adversarial concurrent edits** —
   include this in tests; loro behavior under pathological cases
   should be predictable.

---

## Honest comparison: SuperX vs SuperX (the doc set I just helped design)

| Dimension | SuperX spec | SuperX spec | Better |
|---|---|---|---|
| SCD-2 universal invariant | Yes | Yes | Tie |
| UUIDv7 everywhere | Implicit | Explicit | SuperX |
| Storage strategy | Hot CRDT + cold SCD-2 | Cold SCD-2 only (Loro deferred to v2) | SuperX if dual-state correctness is proven; SuperX if it isn't |
| Code graph | Continuous Wasm extractors in-kernel | graphify-rs via MCP bridge | SuperX (lower latency, tighter integration) |
| Tool ABI | Wasm WASI + Docker via VFS | MCP / rmcp | SuperX (ecosystem compat) |
| Multi-tenancy | Not addressed | First-class from day 0 | SuperX |
| Secrets management | Not addressed | `secret` table + KMS | SuperX |
| Prompt discipline | Not addressed | XML/tagged, write-time validation | SuperX |
| System agents | Proposer + Evaluator + Warden | Classifier + Summarizer + Redactor + Consolidator + Anomaly-watcher | SuperX (more complete) |
| Hardware tiers | Not addressed | T0–T5 auto-detect | SuperX |
| Interactive (human) capture | Not addressed | `interactive.*` event taxonomy | SuperX |
| Emission pipeline | One paragraph | Full outbox + sink trait + cursor + backpressure spec | SuperX |
| Library inventory | Partial | Complete (Rust + React) | SuperX |
| Graph canvas (React) | xyflow | ECharts graph series | SuperX |
| Capability check | LLM-fuzzy primary | Deterministic primary + LLM anomaly-watcher | SuperX |
| Self-improvement loop | Auto-deploy | Approval-gated in prod | SuperX |
| Differentiation pitch | Hot/cold + Wasm + VFS + self-modifying | Time-traveled data + insert-only + MCP-driver + central analytics | Both strong, different bets |

**My architect's verdict:** SuperX has 4 great ideas that SuperX should
adopt (Wasm extractors with shared memory; xyflow canvas; loro CRDT for
opt-in collaboration; continuous in-kernel indexing). SuperX has ~10
specifications SuperX is missing (MCP, tenants, secrets, prompt
discipline, full system-agent roster, hardware tiers, interactive
capture, emission engineering, library inventory, failure-mode matrix).

The right move: **merge them.** Take SuperX's hot-path innovations
into SuperX's foundation. Result: an agentic OS with both the
sub-10ms code-graph reactivity AND the production-readiness of a
properly tenant-isolated, secret-managed, capability-gated, MCP-fluent
system.

If they remain separate products, SuperX as currently specified is a
research demo waiting to bite production users on (a) state-sync
divergence under load, (b) FUSE on macOS, (c) LLM-in-the-security-loop
incidents, and (d) the absence of multi-tenancy + secrets. Each is
fixable; none is fixed by the docs as they stand.

---

## What I'd do next (if asked)

1. **Spike the state-sync correctness story** (1 week). Build a
   minimal CRDT-to-SCD-2 sync, randomly inject crashes mid-write,
   measure divergence rate. If it's not provably zero, drop the dual
   state and use SurrealDB LIVE SELECT.
2. **Add MCP / rmcp** as the tool ABI (no spike needed, just a
   decision). Wasm tools can expose MCP server interfaces over
   shared-memory channels — keep the speed, gain the ecosystem.
3. **Write the missing docs**:
   - `06-state-sync.md` (the answer to Critical #1)
   - `07-multi-tenancy.md`
   - `08-secrets.md`
   - `09-system-agents.md` (full roster, not just three)
   - `10-failure-modes.md`
   - `11-cargo-workspace.md`
   - `12-react-stack.md` (with xyflow as the headline)
4. **Promote the Wasm extractor swarm** as the headline differentiator
   — it's genuinely novel and worth fronting.
5. **Re-pitch:** "SuperX — the agentic OS that indexes your code in
   under 10 milliseconds, time-travels every state change forever, and
   lets agents + humans collaborate on the same living graph." Drop
   "OS writes its own code without human intervention" — replace with
   "OS proposes its own improvements, audited and reviewed by you."

The bones are good. The gaps are filling, not redesigning.
