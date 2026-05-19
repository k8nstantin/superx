# SuperX Tech-Debt Audit — 2026-05-19

**Phase A.2** of the MVP roadmap. Diligent, careful read of every `crates/*/src/*.rs` file in the workspace, checked against the nine categories specified in issue #2 + the §0 / §7 invariants in `ARCHITECTURE.md`.

This is a **read-only survey**. Each finding is a candidate for its own follow-up PR. The findings are categorised by severity so Phase A.3 (detailed comment pass) and Phase B (substrate foundations) can address them in the right order.

## Files reviewed (all 12 source files in the workspace)

| File | LoC | Findings |
|---|---|---|
| `crates/superx-agent/src/lib.rs` | 112 | 1 medium, 2 low |
| `crates/superx-bootstrap/src/lib.rs` | 342 | 2 medium |
| `crates/superx-cli/src/main.rs` | 373 | 2 medium, 2 low |
| `crates/superx-compiler/src/lib.rs` | 91 | 1 medium, 2 low |
| `crates/superx-emission/src/lib.rs` | 211 | 0 (clean post Phase A.1) |
| `crates/superx-harness/src/lib.rs` | 109 | **1 high**, 1 medium |
| `crates/superx-inference/src/lib.rs` | 106 | **3 high**, 2 low |
| `crates/superx-ingest/src/lib.rs` | 147 | 0 (clean post task #12) |
| `crates/superx-kernel/src/lib.rs` | 795 | 1 medium |
| `crates/superx-mcp/src/lib.rs` | 224 | 0 (clean post task #15-prep) |
| `crates/superx-mcp/src/main.rs` | 61 | **2 high**, 1 medium |
| `crates/superx-proposer/src/lib.rs` | 97 | 1 medium error-class mis-use, 1 cosmetic |

**Totals: 6 high, 11 medium, 8 low = 25 findings.** No commented-out code, no dead code, no TODOs/FIXMEs/XXXs anywhere. The bones are clean; the findings below are polish + roadmap pre-work.

---

## H — HIGH (fix during MVP Phase A.3 or earlier)

### H1 — `tracing::info!("DEBUG: …")` is mis-leveled at INFO

`crates/superx-harness/src/lib.rs:79`

```rust
tracing::info!("DEBUG: Raw score records for {proposal_id}: {all_records:?}");
```

A DEBUG-labelled line emitted at INFO. Confuses log filtering: operators who set `RUST_LOG=info` get debug noise; operators who set `RUST_LOG=debug` get duplicates. Should be `tracing::debug!`.

**Fix scope**: 1 line.

### H2 — `predict` logs the full user prompt at INFO

`crates/superx-inference/src/lib.rs:79`

```rust
tracing::info!("Running local GGUF inference for prompt: {prompt}");
```

Two problems:

1. **Privacy.** Prompts may contain user-confidential content (file contents, secrets surfaced from code, agent state). Logging at INFO means they land in any default log sink including remote OTel/Kafka.
2. **Volume.** Long context windows (the whole point of the compiler blade calling `predict`) make every INFO line megabyte-class.

Should log prompt *length* + a short prefix at INFO; full prompt at DEBUG only.

**Fix scope**: 1 line + 1 line.

### H3 — `assert!(path.exists())` in `InferenceEngine::new` panics where `Err` is correct

`crates/superx-inference/src/lib.rs:55-56`

```rust
assert!(model_path.exists(), "Model path mandatory");
assert!(tokenizer_path.exists(), "Tokenizer path mandatory");
```

`InferenceEngine::new` returns `Result<Self, InferenceError>` for every other error path. These two `assert!`s panic instead. Operator-facing API: a wrong CLI flag value (`--model-path /nope/missing.gguf`) crashes the whole binary instead of returning a clean error the CLI can print and exit gracefully on. Should be:

```rust
if !model_path.exists() {
    return Err(InferenceError::Load(format!("model path does not exist: {}", model_path.display())));
}
```

Same for tokenizer.

**Fix scope**: ~6 lines.

### H4 — `superx-mcp` binary discards critical background-task errors

`crates/superx-mcp/src/main.rs:42, 53`

```rust
let _ = sub.run_loop(k_sink.as_ref(), a_sink.as_ref(), &sub_tenant).await;
…
let _ = k_pulse.pulse().await;
```

If the emission `run_loop` dies (Kafka broker down, LIVE SELECT permission denied, …), nobody knows. If `pulse` ever errors, nobody knows. For a long-lived MCP server process this is dangerous: the telemetry firehose can silently stop while the binary stays up. Should be:

```rust
if let Err(e) = sub.run_loop(...).await {
    tracing::error!("emission run_loop terminated: {e}");
}
```

**Fix scope**: 4 lines total across two spawns.

### H5 — Hardcoded substrate path + namespace + "prod" db name in MCP binary

`crates/superx-mcp/src/main.rs:25-26`

```rust
let db_path = PathBuf::from("./db/superx.db");
let kernel = Arc::new(Kernel::init(&db_path, "superx", "prod").await?);
```

Three magic strings hardcoded. `./db/superx.db` should honor `SUPERX_DB_PATH`. `"superx"` should honor `SUPERX_NS`. `"prod"` is especially misleading since this binary doesn't know what env it's deployed in — should be `SUPERX_DB_NAME` (or just remove the env distinction; substrate doesn't differentiate behavior on it).

CLI has the same issue at `crates/superx-cli/src/main.rs:133`.

**Fix scope**: 6 lines (3 in each binary).

### H6 — `LogitsProcessor` has 3 hardcoded magic numbers

`crates/superx-inference/src/lib.rs:84`

```rust
let mut logits_processor = LogitsProcessor::new(299_792_458, Some(0.7), Some(0.9));
```

- `299_792_458` = the speed of light in m/s. Cute as a deterministic seed, but it's literally a magic number with no operator visibility. Should be a parameter (`execution_params.seed`).
- `0.7` = temperature. Already flagged elsewhere; belongs in `execution_params`.
- `0.9` = top_p. Same.

This is exactly what roadmap #1b (`execution_params` SCD-2 table) closes. The fix lands as part of Phase B, not Phase A.3 — but **noted now so we don't ship v1.0 with hardcoded sampling params.**

### H7 — EOS magic number is hardcoded per the wrong assumption

`crates/superx-inference/src/lib.rs:99`

```rust
if next_token == 2 { // Typical EOS
    break;
}
```

The "typical" comment is the smell. **Different models have different EOS tokens.** Gemma 4's EOS is not Llama's EOS is not Qwen 3's EOS. The model's tokenizer + config carries the canonical EOS token id; we should be reading it from `self.tokenizer` or from the model config, not hardcoded.

**Fix scope**: ~5 lines — query the tokenizer for the EOS token id at engine construction; store on `self`; reference here.

---

## M — MEDIUM (fix before Phase D ships, OK after Phase B starts)

### M1 — Bootstrap's `attr_config` defaults duplicate kernel `get_parameter` defaults

`crates/superx-bootstrap/src/lib.rs:63-67` writes `{"max_dfs_iterations": 10_000, "max_traversal_depth": 10, "max_ingestion_entries": 10_000, ...}` to `attr_config`. The kernel's `get_parameter` calls also pass the *same* numbers as defaults:

- `crates/superx-kernel/src/lib.rs:407` — `max_dfs_iterations` default 10_000
- `crates/superx-kernel/src/lib.rs:480` — `max_traversal_depth` default 10
- `crates/superx-kernel/src/lib.rs:481` — `max_context_nodes` default 10_000
- `crates/superx-ingest/src/lib.rs:52` — `max_ingestion_entries` default 10_000

Two sources of truth. Should be one — a `pub const DEFAULT_ATTR_CONFIG: &str` JSON in the kernel that bootstrap loads + `get_parameter` reads.

### M2 — Inference / proposer error-class confusion: inference failure → `SafetyViolation`

`crates/superx-proposer/src/lib.rs:60`

```rust
let proposal_type = engine.predict(&prompt, 10).map_err(|e| KernelError::SafetyViolation(e.to_string()))?;
```

A model inference failure is not a NASA-rule-bounded-loop safety violation. It's a `Validation` error (the model can't produce a usable answer) or a new `InferenceError`-bridged variant. Conflating it as `SafetyViolation` muddles the error taxonomy and makes the alerting story noisier than it should be.

### M3 — Substrate entity is given a `role`

`crates/superx-bootstrap/src/lib.rs:53`

```rust
self.kernel.db.query("UPSERT $id SET tenant_id = $t, role = 'admin', type = type_definition:node_substrate")
```

The substrate is a `node_substrate`, not a `node_agent`. Roles belong on agents and sessions. Schema-wise this is harmless (the `role` field has a default of `'user'` and `'admin'` passes the ASSERT), but conceptually wrong. The PERMISSIONS clauses use `$session_role` not the substrate's own role, so this assignment never reads. Drop the `role = 'admin'` for cleanliness.

### M4 — `Commands::Stats` query uses `SELECT *, <string>id, <string>timestamp`

`crates/superx-cli/src/main.rs:248`

```rust
"SELECT *, <string>id as id, <string>timestamp as timestamp FROM telemetry_stream …"
```

The `*` plus the two casts is wasteful — `*` already includes `id` and `timestamp` (in their native types); the cast aliases add escaped string copies. Cleaner: enumerate `lifecycle_event`, `payload`, `run_id`, `tenant_id`, and `<string>timestamp` only.

### M5 — `tracing_subscriber::fmt::init()` ignores `RUST_LOG` and lacks structured output

Both `crates/superx-cli/src/main.rs:130` and `crates/superx-mcp/src/main.rs:23` use the default `tracing_subscriber::fmt::init()`. Operators expect `RUST_LOG=debug ./superx-cli …` to work. It doesn't — they must edit code. Should use `tracing_subscriber::fmt().with_env_filter(EnvFilter::from_default_env()).init()`. For production deployments, JSON output (`.with_format(Format::Json)`) is the right default. Could land as a small shared `superx-observability::init_tracing()` helper.

### M6 — `proposer` hardcodes the allowed edge-type set

`crates/superx-proposer/src/lib.rs:64`

```rust
let final_type = if ["edge_owns", "edge_implements", "edge_semantic"].contains(&sanitized_type.as_str()) {
```

Adding a new edge type to the metamodel requires re-compiling. Should query `SELECT meta::id(id) AS uid FROM type_definition WHERE category = 'edge' AND tenant_id = $session_tenant` and use the actual set. Otherwise the moat principle "adding a feature = a substrate write" doesn't hold for proposer.

### M7 — `harness::promote` query backticks `\`type\`` but others don't

`crates/superx-harness/src/lib.rs:74` uses `` AND `type` = type_definition:attr_score ``. Other queries in the same crate use bare `type =`. Both forms work in SurrealDB 2.x; pick one and stick with it. Recommendation: always backtick `type` since it's a SurrealQL keyword — silent breakage on a future SurrealDB version is non-zero.

### M8 — `Demo` writes `attr_score` via raw query bypassing kernel verbs

`crates/superx-cli/src/main.rs:346-350` writes the proposal entity via raw `kernel.db.query("CREATE $id CONTENT …")`. The roadmap #13 (typed kernel write verbs) handles this — listing here so it's not forgotten when that lands.

### M9 — Tracing missing on dispatch-decision points

Three blade entry points emit no `tracing::info!` on entry, only via telemetry:

- `superx-harness::evaluate` — no entry log; operator debugging a failing eval has no log line to grep for.
- `superx-agent::handshake` — same.
- `superx-agent::check_capability` — runs silently; capability denials are visible only via the returned error.

Adding `tracing::info!` at entry of each + on the success/deny decision in `check_capability` would help operators trace runtime behavior without tailing the substrate's `telemetry_stream`.

### M10 — `JsonSource::ingest` writes via raw `INSERT INTO`

`crates/superx-ingest/src/lib.rs:114-119` — same shape as M8, roadmap #13 covers it.

### M11 — Proposer prompt typo "semanticly"

`crates/superx-proposer/src/lib.rs:54` — "is semanticly related" should be "is semantically related". Model is probably tolerant but it's a literal typo in the prompt.

---

## L — LOW (defer to Phase D or v1.1 polish; OK to ship v1.0 with these)

### L1 — Hardcoded prompt templates (compiler, proposer)

`compiler/lib.rs:69`, `proposer/lib.rs:53-58` — both blades have inline prompt strings. Roadmap #6 "prompts as substrate entities" addresses this systematically. **Do not patch piecemeal; wait for the full Roadmap-#6 PR.**

### L2 — Hardcoded `max_tokens` per call site

- `compiler/lib.rs:70` — 512 tokens
- `proposer/lib.rs:60` — 10 tokens
- `inference/lib.rs:44` — `MAX_PREDICT_TOKENS = 4096` cap

All become `execution_params` rows per Roadmap #1b. Same "wait for the full PR" guidance as L1.

### L3 — `Duration::from_mins(1)` heartbeat interval hardcoded

`superx-mcp/src/main.rs:49`. Pulse cadence is an operator concern — should be `attr_config.pulse_interval_secs`. Roadmap #14 (deployment-mode toggle) is the right home.

### L4 — `superx-agent::handshake` builds `entity:` literals via `format!`

`agent/lib.rs:67` — `format!("entity:{session_uid}")` and `format!("entity:{agent_uid}")`. Could use `Thing::from(...).to_string()` for consistency. Cosmetic.

### L5 — `Stats` CLI deserializes telemetry as `serde_json::Value` rather than a typed struct

`crates/superx-cli/src/main.rs:253`. The typed-`TelemetryRow` from `superx-emission` exists; could be reused. Trade-off: typed struct catches schema drift, Value is permissive. Either is defensible.

### L6 — `inference/lib.rs:88-90` has an off-by-context-window subtlety

`let context_size = if i > 0 { 1 } else { tokens_ids.len() };` — first iteration sends the full prompt, subsequent iterations send only the new token. This is the standard "KV cache after prefill" pattern; correct but uncommented. A 2-line comment explaining "after prefill, only the new token feeds the next forward pass" would help future maintainers.

### L7 — Tests use mixed result-decoding patterns

`tests/core_capabilities.rs` sometimes uses `Vec<serde_json::Value>` and sometimes typed structs (`CountRow`) for similar count queries. Pick one and stick with it. Trade-off as in L5.

### L8 — `bootstrap.run` emits two `tracing::info!` lines around verification

Lines 83 + 96. Could merge into one structured event. Minor.

---

## Cross-cutting observations (no specific line)

### O1 — Every blade speaks to the kernel via raw `db.query`

The blades that issue raw queries (`agent`, `bootstrap`, `ingest`, `proposer`, `cli`, `mcp` — 6 of 11 crates) bypass any chance for the kernel to centralise audit / SCD-2 / write-validation logic. **Roadmap #13** ("Typed kernel write verbs") is the load-bearing fix. The audit confirms how widely this is needed.

### O2 — The metamodel's `node_session` type isn't actively used post-handshake

Agent `handshake` creates `node_session` entities + `edge_participates_in` edges. Nothing else queries them. They accumulate forever (no SCD-2 lifecycle for sessions). Either:

- Add a session lifecycle (open / heartbeat / close → SCD-2 close-out), OR
- Drop `node_session` from the metamodel.

Defer to v1.1 — not blocking MVP.

### O3 — `node_data_source` is in `ARCHITECTURE.md` but not in the metamodel seed

`apply_substrate_schema` seeds these node types: `node_substrate, node_source_external, node_component, node_hardened_model, node_agent, node_session, node_capability, node_tool, node_prod, node_code, node_code_root, node_artifact, node_proposal, node_harness, node_rag_source`.

The vision in §0c refers to `node_data_source` (and its subtypes for SQL / Iceberg / RAG / remote-model). **These do not yet exist in the seed.** Roadmap #4 lands them. Note in audit so they don't get silently forgotten.

### O4 — `pulse` method on `Kernel` is not audited here

The `Kernel::pulse` method (`crates/superx-kernel/src/lib.rs` somewhere; called from MCP `main.rs:53`) was not directly read in this audit pass. Should be in scope for Phase A.3.

---

## Recommended Phase A.3 work-order

Phase A.3 is the "detailed comment pass." Combine it with these quick fixes from the audit:

1. **H1 + H2** — fix the two tracing-level mistakes (5 lines total).
2. **H3** — convert the two `assert!(path.exists())` to `Err` returns (6 lines).
3. **H4** — wire `tracing::error!` around the two `let _ = ...await` spawns in `superx-mcp` (4 lines).
4. **H5** — read `SUPERX_DB_PATH` / `SUPERX_NS` / `SUPERX_DB_NAME` env vars in both binaries (6 lines).
5. **M2** — proposer's inference error → `Validation`, not `SafetyViolation` (1 line).
6. **M3** — drop the `role = 'admin'` on the substrate entity UPSERT (1 line).
7. **M5** — share a `superx-observability::init_tracing()` helper using `EnvFilter` (~20 lines, new file).
8. **M9** — add `tracing::info!` at entry of `evaluate`, `handshake`, `check_capability` and on the deny decision in `check_capability` (4 lines).
9. **M11** — fix the "semanticly" typo (1 line).
10. **Doc-comment pass** — touch every pub item across all crates, addressing L6 + L8 as inline comments.

**H6, H7 deferred** to Phase B (they need `execution_params` and per-model config to land first).

**M1, M6 deferred** to Phase B/C (they need the typed kernel verbs and metamodel-driven edge listing).

**All L-class items deferred** to Phase D or v1.1 per the call-outs above.

---

## Verification

- `cargo test --workspace`: 44 / 44 passing (Mandate-5 satisfied)
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: clean
- **No code touched in this audit** — read-only survey.

Total survey time: targeted reading of 12 files (~2 700 LoC) plus targeted greps for the nine categories. Findings cataloged above are exhaustive against those categories.
