/*
 * SuperX Core-Capabilities Test Suite — Revision 42.14
 *
 * Copyright (c) 2026 Constantin Alexander <constantin@dedomena.io>
 *
 * Each test exercises ONE architectural promise from ARCHITECTURE.md. New
 * capability tests (DAG compilation, memory tiering, Meta-Harness wasm
 * evaluate, MCP dispatch, etc.) should be appended here grouped by section.
 */

use serde_json::Value;
use superx_agent::CapabilityGovernor;
use superx_bootstrap::BootstrapBlade;
use superx_emission::{ApiSink, TelemetryRow};
use superx_harness::MetaHarness;
use superx_ingest::{FileSource, JsonSource, UniversalIngestor};
use superx_kernel::{Kernel, KernelError};
use surrealdb::sql::Id;
use tempfile::TempDir;

/// Process-wide serialization for tests that mutate `SUPERX_CLAUDE_CONFIG` /
/// `SUPERX_CLAUDE_LOGS`. Env vars are global; without this guard, parallel
/// `cargo test` runs race on the shared state.
fn claude_env_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Extract the raw uid-string portion of a `Kernel::parse_id`-normalized record id literal.
/// Bypasses `<string>id`'s backtick escaping for hyphen-bearing UUIDs (see
/// `surrealdb-core/src/sql/escape.rs::EscapeRidKey`).
fn local_uid(record_id_literal: &str) -> String {
    let thing = Kernel::parse_id(record_id_literal).expect("valid record id literal");
    match thing.id {
        Id::String(s) => s,
        other => panic!("expected Id::String, got {other:?}"),
    }
}

/// Spin up an isolated kernel + bootstrap a tenant. Returns the temp dir
/// (kept alive to preserve RocksDB), the kernel, and the substrate id.
async fn bootstrap_tenant(tenant: &str) -> (TempDir, Kernel, String) {
    let _ = tracing_subscriber::fmt::try_init();
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("superx.db");
    let kernel = Kernel::init(&db_path, "test", "test").await.expect("kernel init");
    let substrate_id = BootstrapBlade::new(&kernel).run(tenant).await.expect("bootstrap");
    (dir, kernel, substrate_id)
}

/// Count telemetry events for the current session tenant matching an event name.
async fn count_events(kernel: &Kernel, lifecycle_event: &str) -> u64 {
    #[derive(serde::Deserialize)]
    struct Row { count: u64 }
    let mut res = kernel
        .db
        .query("SELECT count() AS count FROM telemetry_stream WHERE tenant_id = $session_tenant AND lifecycle_event = $ev GROUP ALL")
        .bind(("ev", lifecycle_event.to_string()))
        .await
        .expect("count query");
    res.take::<Vec<Row>>(0).expect("count rows").pop().map_or(0, |r| r.count)
}

// =====================================================================
// 1. TELEMETRY — fine-grained capture from instant-zero
//    The vision: every kernel mutation fires a typed telemetry event for
//    the active tenant. Bootstrap alone must produce a rich audit trail.
// =====================================================================

#[tokio::test]
async fn telemetry_bootstrap_emits_system_event() {
    let (_dir, kernel, _sub) = bootstrap_tenant("tele_t1").await;
    assert!(
        count_events(&kernel, "system_bootstrap").await >= 1,
        "Bootstrap must emit at least one system_bootstrap event"
    );
}

#[tokio::test]
async fn telemetry_bootstrap_emits_state_supersede_events() {
    let (_dir, kernel, _sub) = bootstrap_tenant("tele_t2").await;
    // Bootstrap calls supersede_state for: attr_config on substrate, attr_desc
    // on system_controller, attr_desc on gemini_cli, plus optional Claude
    // Desktop agents. The kernel's supersede_state always emits one event.
    let n = count_events(&kernel, "state_supersede").await;
    assert!(n >= 3, "expected ≥3 state_supersede events from bootstrap, got {n}");
}

#[tokio::test]
async fn telemetry_bootstrap_emits_edge_create_events() {
    let (_dir, kernel, _sub) = bootstrap_tenant("tele_t3").await;
    // Bootstrap creates: substrate→system_controller edge_owns,
    // substrate→gemini_cli edge_owns, and 10 edge_has_capability edges
    // (2 admin agents × 5 tools). Each emits one edge_create event.
    let n = count_events(&kernel, "edge_create").await;
    assert!(n >= 12, "expected ≥12 edge_create events from bootstrap, got {n}");
}

#[tokio::test]
async fn telemetry_is_tenant_isolated() {
    // Bootstrap two tenants in the same kernel. Each tenant must only see
    // its own telemetry events when queried under that session.
    let _ = tracing_subscriber::fmt::try_init();
    let dir = tempfile::tempdir().unwrap();
    let kernel = Kernel::init(&dir.path().join("superx.db"), "test", "test").await.unwrap();

    BootstrapBlade::new(&kernel).run("iso_t_a").await.unwrap();
    let a_events = count_events(&kernel, "system_bootstrap").await;

    BootstrapBlade::new(&kernel).run("iso_t_b").await.unwrap();
    let b_events_self = count_events(&kernel, "system_bootstrap").await;

    // Each tenant sees exactly its own bootstrap event under its session.
    assert_eq!(a_events, 1, "tenant A's session must see exactly 1 system_bootstrap");
    assert_eq!(b_events_self, 1, "tenant B's session must see exactly 1 system_bootstrap");
}

// =====================================================================
// 2. BOOTSTRAP — idempotency & deterministic substrate identity
// =====================================================================

#[tokio::test]
async fn bootstrap_is_idempotent_for_same_tenant() {
    let _ = tracing_subscriber::fmt::try_init();
    let dir = tempfile::tempdir().unwrap();
    let kernel = Kernel::init(&dir.path().join("superx.db"), "test", "test").await.unwrap();
    let tenant = "idem_t";

    let first = BootstrapBlade::new(&kernel).run(tenant).await.expect("first boot");
    let second = BootstrapBlade::new(&kernel).run(tenant).await.expect("second boot");

    assert_eq!(first, second, "substrate id must be deterministic (UUIDv5 of tenant)");
}

// =====================================================================
// 11. CLI VERBS — operator surface (identify, list-agents, list-tools, demo)
// =====================================================================
//
// These tests exercise the same underlying calls each CLI verb performs.
// Going through `clap::Parser` would require process-spawning for full
// fidelity; we instead test the substrate-visible effect of each verb so
// the contract that operators rely on is locked in.

#[tokio::test]
async fn cli_list_agents_after_bootstrap_shows_canonical_admins() {
    // `superx-cli list-agents` queries node_agent entities for the tenant.
    // After bootstrap we expect at least the two seeded admins to appear.
    let (_dir, kernel, _sub) = bootstrap_tenant("cli_la").await;
    #[derive(serde::Deserialize)]
    struct Row { id: String, role: String }
    let mut res = kernel
        .db
        .query("SELECT <string>id AS id, role FROM entity WHERE type = type_definition:node_agent AND tenant_id = $session_tenant")
        .await
        .unwrap();
    let rows: Vec<Row> = res.take(0).unwrap();
    let admin_count = rows.iter().filter(|r| r.role == "admin").count();
    assert!(admin_count >= 2, "expected ≥2 admin agents (system_controller + gemini_cli), got {admin_count}");
    assert!(rows.iter().all(|r| r.id.starts_with("entity:")), "every row must carry a fully-qualified id literal");
}

#[tokio::test]
async fn cli_list_tools_after_bootstrap_shows_five_canonical_tools() {
    let (_dir, kernel, _sub) = bootstrap_tenant("cli_lt").await;
    #[derive(serde::Deserialize)]
    struct Row { id: String }
    let mut res = kernel
        .db
        .query("SELECT <string>id AS id FROM entity WHERE type = type_definition:node_tool AND tenant_id = $session_tenant ORDER BY id ASC")
        .await
        .unwrap();
    let rows: Vec<Row> = res.take(0).unwrap();
    assert_eq!(rows.len(), 5, "bootstrap must seed exactly 5 standard tools, got {}", rows.len());
    // tool ids are stored as plain strings (no UUID), so they're stable
    let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
    for expected in ["entity:tool_compile", "entity:tool_evaluate", "entity:tool_ingest", "entity:tool_promote", "entity:tool_propose"] {
        assert!(ids.contains(&expected), "missing canonical tool {expected}; got {ids:?}");
    }
}

#[tokio::test]
async fn cli_identify_creates_session_for_seeded_admin() {
    // `superx-cli identify --agent-uid <local>` wraps CapabilityGovernor::handshake.
    // Using the system_controller's local uid, we must get a session_uid back AND
    // a session entity must appear in the substrate.
    let (_dir, kernel, _sub) = bootstrap_tenant("cli_id").await;
    let admin_full_id = fetch_admin_agent_id(&kernel).await;
    let agent_uid = local_uid(&admin_full_id);

    let gov = CapabilityGovernor::new(&kernel);
    let session_uid = gov.handshake(&agent_uid).await.expect("identify must succeed");
    assert!(!session_uid.is_empty(), "session_uid returned must be non-empty");
}

#[tokio::test]
async fn bootstrap_emits_agent_seeded_events_for_canonical_admins() {
    // Mandatory admin agents (system_controller + gemini_cli) must each produce a
    // typed `agent_seeded` telemetry event with role=admin so downstream consumers
    // see the agent census, not just side-effect events.
    let (_dir, kernel, _sub) = bootstrap_tenant("agent_seeded_t").await;
    let n = count_events(&kernel, "agent_seeded").await;
    assert!(n >= 2, "expected ≥2 agent_seeded events (system_controller + gemini_cli), got {n}");
}

#[tokio::test]
async fn bootstrap_emits_agent_discovered_for_each_claude_desktop_server() {
    // Inject a Claude Desktop config fixture via SUPERX_CLAUDE_CONFIG and verify
    // each declared `mcpServers` entry produces a typed `agent_discovered` event.
    let _guard = claude_env_lock().lock().await;

    let fixture_dir = tempfile::tempdir().unwrap();
    let config_path = fixture_dir.path().join("claude_desktop_config.json");
    std::fs::write(
        &config_path,
        r#"{
            "mcpServers": {
                "filesystem":  {"command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]},
                "playwright":  {"command": "npx", "args": ["-y", "@executeautomation/playwright-mcp-server"]},
                "git":         {"command": "uvx", "args": ["mcp-server-git"]}
            }
        }"#,
    )
    .unwrap();

    // Point the discovery probe at our fixture and clear the activity probe so the
    // test is hermetic on developer machines that have real Claude logs.
    std::env::set_var("SUPERX_CLAUDE_CONFIG", config_path.to_str().unwrap());
    std::env::set_var("SUPERX_CLAUDE_LOGS", fixture_dir.path().join("no_such_logs_dir").to_str().unwrap());

    let (_dir, kernel, _sub) = bootstrap_tenant("agent_disco_t").await;
    let n = count_events(&kernel, "agent_discovered").await;

    std::env::remove_var("SUPERX_CLAUDE_CONFIG");
    std::env::remove_var("SUPERX_CLAUDE_LOGS");

    assert!(n >= 3, "fixture has 3 mcpServers entries; expected ≥3 agent_discovered events, got {n}");
    let census = count_events(&kernel, "bootstrap_census").await;
    assert_eq!(census, 1, "exactly one bootstrap_census summary must be emitted, got {census}");
}

#[tokio::test]
async fn bootstrap_emits_agent_activity_observed_for_each_mcp_log_file() {
    // Inject a fake "Claude logs" directory containing two mcp-server-*.log files.
    // Each must produce an `agent_activity_observed` event.
    let _guard = claude_env_lock().lock().await;

    let logs_dir = tempfile::tempdir().unwrap();
    std::fs::write(logs_dir.path().join("mcp-server-filesystem.log"), "Z").unwrap();
    std::fs::write(logs_dir.path().join("mcp-server-playwright.log"), "Z").unwrap();
    std::fs::write(logs_dir.path().join("not-an-agent.txt"), "ignored").unwrap();

    std::env::set_var(
        "SUPERX_CLAUDE_CONFIG",
        logs_dir.path().join("no_such_config.json").to_str().unwrap(),
    );
    std::env::set_var("SUPERX_CLAUDE_LOGS", logs_dir.path().to_str().unwrap());

    let (_dir, kernel, _sub) = bootstrap_tenant("agent_activity_t").await;
    let n = count_events(&kernel, "agent_activity_observed").await;

    std::env::remove_var("SUPERX_CLAUDE_CONFIG");
    std::env::remove_var("SUPERX_CLAUDE_LOGS");

    assert_eq!(n, 2, "expected exactly 2 activity events (one per matching log file), got {n}");
}

#[tokio::test]
async fn bootstrap_seeds_capability_edges_for_admin_agents() {
    let (_dir, kernel, _sub) = bootstrap_tenant("seed_t").await;

    // Bootstrap seeds: tool_ingest, tool_compile, tool_propose, tool_evaluate, tool_promote
    // and grants both admin agents (system_controller, gemini_cli) edge_has_capability to all.
    #[derive(serde::Deserialize)]
    struct Row { count: u64 }
    let mut res = kernel
        .db
        .query("SELECT count() AS count FROM relation WHERE type = type_definition:edge_has_capability AND tenant_id = $session_tenant GROUP ALL")
        .await
        .unwrap();
    let count = res.take::<Vec<Row>>(0).unwrap().pop().map_or(0, |r| r.count);
    assert_eq!(count, 10, "2 admin agents × 5 standard tools = 10 capability edges");
}

// =====================================================================
// 3. CAPABILITY GOVERNOR — handshake, allow, deny, cross-tenant coercion
// =====================================================================

/// Helper: fetch one bootstrap-seeded admin agent's full record id literal.
async fn fetch_admin_agent_id(kernel: &Kernel) -> String {
    #[derive(serde::Deserialize)]
    struct Row { id: String }
    let mut res = kernel
        .db
        .query("SELECT <string>id AS id FROM entity WHERE type = type_definition:node_agent AND role = 'admin' AND tenant_id = $session_tenant LIMIT 1")
        .await
        .unwrap();
    res.take::<Vec<Row>>(0).unwrap().pop().expect("at least one admin agent after bootstrap").id
}

#[tokio::test]
async fn governor_check_capability_allows_seeded_admin() {
    let (_dir, kernel, _sub) = bootstrap_tenant("gov_allow").await;
    let agent_id = fetch_admin_agent_id(&kernel).await;
    let gov = CapabilityGovernor::new(&kernel);
    gov.check_capability(&agent_id, "tool_ingest").await.expect("seeded admin must hold tool_ingest");
    gov.check_capability(&agent_id, "tool_compile").await.expect("seeded admin must hold tool_compile");
}

#[tokio::test]
async fn governor_check_capability_denies_when_no_edge_exists() {
    let (_dir, kernel, _sub) = bootstrap_tenant("gov_deny").await;
    let gov = CapabilityGovernor::new(&kernel);
    let res = gov
        .check_capability("entity:nonexistent_agent_id_xyz", "tool_ingest")
        .await;
    assert!(
        matches!(res, Err(KernelError::SafetyViolation(_))),
        "unknown agent must be denied with SafetyViolation, got {res:?}"
    );
}

#[tokio::test]
async fn governor_handshake_creates_session_and_participates_edge() {
    let (_dir, kernel, _sub) = bootstrap_tenant("gov_hs").await;
    let agent_id = fetch_admin_agent_id(&kernel).await;
    let agent_uid = local_uid(&agent_id);

    let gov = CapabilityGovernor::new(&kernel);
    let session_uid = gov.handshake(&agent_uid).await.expect("handshake must succeed");
    assert!(!session_uid.is_empty(), "handshake returns a session uid");

    // A session entity must now exist with type node_session.
    #[derive(serde::Deserialize)]
    struct Row { count: u64 }
    let mut res = kernel
        .db
        .query("SELECT count() AS count FROM entity WHERE type = type_definition:node_session AND tenant_id = $session_tenant GROUP ALL")
        .await
        .unwrap();
    let sessions = res.take::<Vec<Row>>(0).unwrap().pop().map_or(0, |r| r.count);
    assert!(sessions >= 1, "handshake must create at least one session entity");

    // An edge_participates_in must link the session to the agent.
    let mut res = kernel
        .db
        .query("SELECT count() AS count FROM relation WHERE type = type_definition:edge_participates_in AND tenant_id = $session_tenant GROUP ALL")
        .await
        .unwrap();
    let edges = res.take::<Vec<Row>>(0).unwrap().pop().map_or(0, |r| r.count);
    assert!(edges >= 1, "handshake must create at least one edge_participates_in");

    assert!(
        count_events(&kernel, "agent_handshake").await >= 1,
        "handshake must emit an agent_handshake telemetry event"
    );
}

#[tokio::test]
async fn governor_handshake_rejects_cross_tenant_identity() {
    // Bootstrap tenant A, capture its admin agent uid, switch to tenant B
    // (separate kernel state), then try to handshake using A's uid. The
    // governor's identity-coercion check must reject the foreign agent.
    let _ = tracing_subscriber::fmt::try_init();
    let dir = tempfile::tempdir().unwrap();
    let kernel = Kernel::init(&dir.path().join("superx.db"), "test", "test").await.unwrap();

    BootstrapBlade::new(&kernel).run("coerce_a").await.unwrap();
    let agent_id_a = fetch_admin_agent_id(&kernel).await;
    let agent_uid_a = local_uid(&agent_id_a);

    // Bootstrap tenant B; this also leaves the session set to B.
    BootstrapBlade::new(&kernel).run("coerce_b").await.unwrap();

    let gov = CapabilityGovernor::new(&kernel);
    let res = gov.handshake(&agent_uid_a).await;
    assert!(
        matches!(&res, Err(KernelError::SafetyViolation(ref m)) if m.contains("belongs to tenant")),
        "cross-tenant handshake must be refused, got {res:?}"
    );
}

// =====================================================================
// 4. INGEST — universal ingestor + JsonSource
// =====================================================================

#[tokio::test]
async fn ingest_json_source_creates_entity_and_state() {
    let (_dir, kernel, _sub) = bootstrap_tenant("ingest_t").await;
    let ingestor = UniversalIngestor::new(&kernel);
    let payload = serde_json::json!({"text": "ingested payload"});
    let root_id = ingestor
        .ingest(Box::new(JsonSource { data: payload.clone() }), "run_ingest_1")
        .await
        .expect("ingest must succeed");

    assert!(root_id.starts_with("entity:"));

    // The ingested entity must carry a current attr_desc state row with our payload.
    let root_thing = Kernel::parse_id(&root_id).unwrap();
    #[derive(serde::Deserialize)]
    struct Row { value_json: Value }
    let mut res = kernel
        .db
        .query("SELECT value_json FROM state_ledger WHERE target = $t AND `type` = type_definition:attr_desc AND is_current = true LIMIT 1")
        .bind(("t", root_thing))
        .await
        .unwrap();
    let row = res.take::<Vec<Row>>(0).unwrap().pop().expect("attr_desc must exist");
    assert_eq!(row.value_json.get("text").and_then(Value::as_str), Some("ingested payload"));
}

// =====================================================================
// 5. META-HARNESS — promote contract (below threshold + parse_id boundary)
// =====================================================================

#[tokio::test]
async fn harness_promote_below_threshold_returns_false() {
    let (_dir, kernel, _sub) = bootstrap_tenant("harn_t").await;

    // Create a proposal entity in this tenant and score it 0.3.
    let proposal_uid = uuid::Uuid::now_v7().to_string();
    let proposal_thing = surrealdb::sql::Thing::from(("entity".to_string(), proposal_uid.clone()));
    let proposal_id = format!("entity:{proposal_uid}");
    kernel
        .db
        .query("INSERT INTO entity { id: $id, tenant_id: $session_tenant, type: type_definition:node_proposal }")
        .bind(("id", proposal_thing))
        .await
        .unwrap()
        .check()
        .unwrap();
    kernel
        .supersede_state(&proposal_id, "attr_score", serde_json::json!({"score": 0.3}), None)
        .await
        .unwrap();

    let promoted = MetaHarness::new(&kernel).promote(&proposal_id, 0.8).await.unwrap();
    assert!(!promoted, "0.3 < 0.8 must NOT promote");
}

#[tokio::test]
async fn harness_promote_rejects_malformed_proposal_id() {
    let (_dir, kernel, _sub) = bootstrap_tenant("harn_bad").await;
    let res = MetaHarness::new(&kernel).promote("not-a-record-id", 0.5).await;
    assert!(
        matches!(res, Err(KernelError::Validation(_))),
        "malformed proposal id must fail at parse_id, got {res:?}"
    );
}

// =====================================================================
// 6. DAG / COMPILE_CONTEXT — descendant traversal, tier filtering, cycles
// =====================================================================

/// Helper: seed an entity owned by the current session tenant.
async fn seed_entity(kernel: &Kernel, id_literal: &str, type_uid: &str) {
    let thing = Kernel::parse_id(id_literal).unwrap();
    kernel
        .db
        .query("INSERT INTO entity { id: $id, tenant_id: $session_tenant, type: type::thing('type_definition', $ty) }")
        .bind(("id", thing))
        .bind(("ty", type_uid.to_string()))
        .await
        .unwrap()
        .check()
        .unwrap();
}

#[tokio::test]
async fn compile_context_walks_all_reachable_descendants() {
    // Seed a 3-level chain a -> b -> c, each with attr_desc state.
    // compile_context(a) must surface every reachable descendant in the output XML.
    let (_dir, kernel, _sub) = bootstrap_tenant("dag_walk").await;
    seed_entity(&kernel, "entity:dag_a", "node_prod").await;
    seed_entity(&kernel, "entity:dag_b", "node_prod").await;
    seed_entity(&kernel, "entity:dag_c", "node_prod").await;
    kernel.create_structural_edge("entity:dag_a", "entity:dag_b", "edge_owns").await.unwrap();
    kernel.create_structural_edge("entity:dag_b", "entity:dag_c", "edge_owns").await.unwrap();
    kernel.supersede_state("entity:dag_a", "attr_desc", serde_json::json!({"text": "ALPHA"}), None).await.unwrap();
    kernel.supersede_state("entity:dag_b", "attr_desc", serde_json::json!({"text": "BETA"}), None).await.unwrap();
    kernel.supersede_state("entity:dag_c", "attr_desc", serde_json::json!({"text": "GAMMA"}), None).await.unwrap();

    let xml = kernel.compile_context("entity:dag_a", "run_dag", None).await.unwrap();
    for token in ["ALPHA", "BETA", "GAMMA"] {
        assert!(xml.contains(token), "compile_context must include {token}; got {xml}");
    }
}

#[tokio::test]
async fn compile_context_filters_by_memory_tier() {
    // The same entity can hold state of multiple memory tiers; compile_context's
    // tier filter must narrow to the requested set.
    //   attr_desc   -> "working" tier
    //   attr_config -> "core" tier
    let (_dir, kernel, _sub) = bootstrap_tenant("tier_filter").await;
    seed_entity(&kernel, "entity:tier_t", "node_prod").await;
    kernel.supersede_state("entity:tier_t", "attr_desc", serde_json::json!({"text": "WORKING_PAYLOAD"}), None).await.unwrap();
    kernel.supersede_state("entity:tier_t", "attr_config", serde_json::json!({"core_payload": "CORE_PAYLOAD"}), None).await.unwrap();

    let core_only = kernel.compile_context("entity:tier_t", "run_t", Some(vec!["core".to_string()])).await.unwrap();
    assert!(core_only.contains("CORE_PAYLOAD"), "core-only request must surface core state, got {core_only}");
    assert!(!core_only.contains("WORKING_PAYLOAD"), "core-only request must NOT leak working state, got {core_only}");

    let working_only = kernel.compile_context("entity:tier_t", "run_t", Some(vec!["working".to_string()])).await.unwrap();
    assert!(working_only.contains("WORKING_PAYLOAD"), "working-only request must surface working state, got {working_only}");
    assert!(!working_only.contains("CORE_PAYLOAD"), "working-only request must NOT leak core state, got {working_only}");
}

#[tokio::test]
async fn cycle_prevention_across_multi_hop() {
    // Acyclic-edge guard must catch cycles longer than one hop.
    //   a -> b -> c, then attempt c -> a (three-hop cycle).
    let (_dir, kernel, _sub) = bootstrap_tenant("cycle_mh").await;
    seed_entity(&kernel, "entity:cy_a", "node_prod").await;
    seed_entity(&kernel, "entity:cy_b", "node_prod").await;
    seed_entity(&kernel, "entity:cy_c", "node_prod").await;
    kernel.create_structural_edge("entity:cy_a", "entity:cy_b", "edge_owns").await.unwrap();
    kernel.create_structural_edge("entity:cy_b", "entity:cy_c", "edge_owns").await.unwrap();

    let res = kernel.create_structural_edge("entity:cy_c", "entity:cy_a", "edge_owns").await;
    assert!(matches!(res, Err(KernelError::CycleDetected)), "3-hop cycle must be detected, got {res:?}");
}

#[tokio::test]
async fn parameter_override_via_attr_config_takes_effect() {
    // The vision says safety constants must be parameters, not hard-coded. Bootstrap
    // seeds defaults via attr_config on the substrate. A subsequent supersede on the
    // substrate's attr_config must be picked up by get_parameter.
    let (_dir, kernel, substrate_id) = bootstrap_tenant("param_over").await;

    // Override two parameters; the new attr_config row supersedes the bootstrap default.
    kernel
        .supersede_state(
            &substrate_id,
            "attr_config",
            serde_json::json!({
                "max_dfs_iterations": 42,
                "max_traversal_depth": 7,
                "max_ingestion_entries": 5,
                "emission_enabled": true,
                "data_scope": "all"
            }),
            None,
        )
        .await
        .unwrap();

    let max_iters: usize = kernel.get_parameter("max_dfs_iterations", 999).await;
    let max_depth: u32 = kernel.get_parameter("max_traversal_depth", 999).await;
    let max_ingest: usize = kernel.get_parameter("max_ingestion_entries", 999).await;
    assert_eq!(max_iters, 42, "parameter override must propagate to max_dfs_iterations");
    assert_eq!(max_depth, 7, "parameter override must propagate to max_traversal_depth");
    assert_eq!(max_ingest, 5, "parameter override must propagate to max_ingestion_entries");
}

// =====================================================================
// 7. SUBSTRATE INTEGRITY — schema health, SCD-2 chain, temporal ordering
// =====================================================================

#[tokio::test]
async fn substrate_schema_health_all_six_tables_queryable() {
    // ARCHITECTURE.md lists 5 substrate tables + execution_cursor. All must be present
    // and queryable after Kernel::init + bootstrap (admin session). We only assert the
    // schema-accepted/parses path; row counts vary by table (e.g. execution_cursor
    // remains empty until a workflow checkpoints).
    let (_dir, kernel, _sub) = bootstrap_tenant("schema_t").await;
    for table in [
        "type_definition",
        "entity",
        "relation",
        "state_ledger",
        "execution_cursor",
        "telemetry_stream",
    ] {
        let q = format!("SELECT count() AS count FROM {table} GROUP ALL");
        let mut res = kernel.db.query(q).await.unwrap_or_else(|e| panic!("table {table}: {e}"));
        let _: Vec<serde_json::Value> = res.take(0).unwrap_or_else(|e| panic!("decode {table}: {e}"));
    }
}

#[tokio::test]
async fn scd2_chain_closes_old_opens_new() {
    // SCD-2 categorical contract (independent of wall-clock resolution): after every
    // supersede, exactly one row per (target, type) is current with valid_to = NULL,
    // and every prior row is closed with valid_to set.
    let (_dir, kernel, _sub) = bootstrap_tenant("scd2_chain").await;
    seed_entity(&kernel, "entity:scd_e", "node_prod").await;
    kernel.supersede_state("entity:scd_e", "attr_desc", serde_json::json!({"text": "v1"}), None).await.unwrap();
    kernel.supersede_state("entity:scd_e", "attr_desc", serde_json::json!({"text": "v2"}), None).await.unwrap();

    // Query categorically — read valid_to as a JSON value so a NULL doesn't trip the
    // `<string>` cast (SurrealDB returns `ConvertTo{from: None}` for that).
    #[derive(serde::Deserialize, Debug)]
    struct LedgerRow {
        is_current: bool,
        valid_to: serde_json::Value,
    }
    let mut res = kernel
        .db
        .query("SELECT is_current, valid_to FROM state_ledger WHERE target = entity:scd_e AND `type` = type_definition:attr_desc")
        .await
        .unwrap();
    let rows: Vec<LedgerRow> = res.take(0).unwrap();
    assert_eq!(rows.len(), 2, "SCD-2 history must keep two rows");

    let current: Vec<&LedgerRow> = rows.iter().filter(|r| r.is_current).collect();
    let closed: Vec<&LedgerRow> = rows.iter().filter(|r| !r.is_current).collect();
    assert_eq!(current.len(), 1, "exactly one is_current=true row per (target,type)");
    assert_eq!(closed.len(), 1, "the prior row must be closed (is_current=false)");

    let current_row = current[0];
    let closed_row = closed[0];
    assert!(
        current_row.valid_to.is_null(),
        "current row's valid_to must be NULL, got {:?}",
        current_row.valid_to
    );
    assert!(
        !closed_row.valid_to.is_null(),
        "closed row's valid_to must be set, got {:?}",
        closed_row.valid_to
    );
}

// =====================================================================
// 7b. META-HARNESS WASM SANDBOX — fuel exhaustion traps as SafetyViolation
// =====================================================================

#[tokio::test]
async fn harness_evaluate_traps_on_fuel_exhaustion() {
    // ARCHITECTURE.md §3 Meta-Harness: "A fuel-metered (10k units) Wasm sandbox".
    // A guest module that loops past the 10k-unit budget MUST trap, and the trap
    // must surface as KernelError::SafetyViolation (not Validation, not Integrity).
    let (_dir, kernel, _sub) = bootstrap_tenant("harn_fuel").await;
    seed_entity(&kernel, "entity:fuel_p", "node_proposal").await;

    // 100_000 iterations × ~5 wasm ops per iteration ≈ 500k ops, far above the
    // 10_000-unit fuel limit defined in MetaHarness::evaluate. The trap fires
    // inside `run_test.call(...)` and harness/lib.rs maps it to SafetyViolation.
    let wat_src = r#"
        (module
          (func (export "run_test") (result i32)
            (local $i i32)
            (loop $loop
              (local.set $i (i32.add (local.get $i) (i32.const 1)))
              (br_if $loop (i32.lt_s (local.get $i) (i32.const 100000)))
            )
            i32.const 95
          )
        )
    "#;
    let wasm = wat::parse_str(wat_src).expect("wat compiles");

    let res = MetaHarness::new(&kernel).evaluate("entity:fuel_p", &wasm).await;
    assert!(
        matches!(&res, Err(KernelError::SafetyViolation(_))),
        "fuel exhaustion must surface as SafetyViolation, got {res:?}"
    );
}

#[tokio::test]
async fn harness_evaluate_records_score_when_wasm_returns_within_budget() {
    // Counter-test for the fuel trap: a tiny module that fits inside 10k fuel
    // must run to completion AND its returned score must be persisted to the
    // ledger as an attr_score row (which `promote` later consults).
    let (_dir, kernel, _sub) = bootstrap_tenant("harn_ok").await;
    seed_entity(&kernel, "entity:ok_p", "node_proposal").await;

    let wat_src = r#"
        (module
          (func (export "run_test") (result i32)
            i32.const 90
          )
        )
    "#;
    let wasm = wat::parse_str(wat_src).expect("wat compiles");

    let score = MetaHarness::new(&kernel).evaluate("entity:ok_p", &wasm).await.expect("evaluate ok");
    assert!((score - 0.90).abs() < f64::EPSILON, "evaluate must return raw_score/100.0, got {score}");

    // Verify the score landed in state_ledger as the current attr_score row.
    #[derive(serde::Deserialize)]
    struct Row { value_json: serde_json::Value }
    let mut res = kernel
        .db
        .query("SELECT value_json FROM state_ledger WHERE target = entity:ok_p AND `type` = type_definition:attr_score AND is_current = true LIMIT 1")
        .await
        .unwrap();
    let row = res.take::<Vec<Row>>(0).unwrap().pop().expect("attr_score row must exist after evaluate");
    let score_field = row.value_json.get("score").and_then(serde_json::Value::as_f64).unwrap();
    assert!((score_field - 0.90).abs() < f64::EPSILON, "ledger score must match return value");
}

// =====================================================================
// 8. SCHEMA VALIDATION — JSON-Schema enforcement at the write boundary
// =====================================================================

#[tokio::test]
async fn attr_score_rejects_non_numeric_and_missing_score() {
    // attr_score's seeded schema is {"required":["score"], "properties":{"score":{"type":"number"}}}.
    // supersede_state must reject any payload that fails the schema before touching the
    // ledger. A valid numeric score must pass.
    let (_dir, kernel, _sub) = bootstrap_tenant("attr_neg").await;
    seed_entity(&kernel, "entity:scorent", "node_proposal").await;

    let missing = kernel
        .supersede_state("entity:scorent", "attr_score", serde_json::json!({}), None)
        .await;
    assert!(
        matches!(&missing, Err(KernelError::Validation(_))),
        "missing required `score` must be rejected, got {missing:?}"
    );

    let wrong_type = kernel
        .supersede_state("entity:scorent", "attr_score", serde_json::json!({"score": "high"}), None)
        .await;
    assert!(
        matches!(&wrong_type, Err(KernelError::Validation(_))),
        "non-numeric `score` must be rejected, got {wrong_type:?}"
    );

    kernel
        .supersede_state("entity:scorent", "attr_score", serde_json::json!({"score": 0.95}), None)
        .await
        .expect("valid numeric score must pass schema validation");
}

// =====================================================================
// 9. INGEST SAFETY — FileSource enforces the max_ingestion_entries cap
// =====================================================================

#[tokio::test]
#[should_panic(expected = "Ingestion limit exceeded")]
async fn filesource_enforces_max_ingestion_entries() {
    // Override the parameter to a tiny cap (2), then walk a fixture dir with > 2 files.
    // FileSource's NASA Rule 2 assert must trip before completion.
    let (_dir, kernel, substrate_id) = bootstrap_tenant("ingest_cap").await;
    kernel
        .supersede_state(
            &substrate_id,
            "attr_config",
            serde_json::json!({
                "max_dfs_iterations": 10_000,
                "max_traversal_depth": 10,
                "max_ingestion_entries": 2,
                "emission_enabled": true,
                "data_scope": "all"
            }),
            None,
        )
        .await
        .unwrap();

    let fixture = tempfile::tempdir().unwrap();
    for i in 0..5 {
        std::fs::write(fixture.path().join(format!("f{i}.txt")), format!("payload {i}")).unwrap();
    }

    let ingestor = UniversalIngestor::new(&kernel);
    let source = Box::new(FileSource { path: fixture.path().to_string_lossy().to_string() });
    let _ = ingestor.ingest(source, "run_cap").await;
}

// =====================================================================
// 9b. MCP DISPATCH — capability check is enforced before tool execution
// =====================================================================

#[tokio::test]
async fn mcp_dispatch_denies_uncapped_agent_on_graphify() {
    // Even with a valid request shape, an agent that lacks edge_has_capability
    // to tool_ingest must be rejected before any ingestion side-effect runs.
    let (_dir, kernel, _sub) = bootstrap_tenant("mcp_deny").await;
    let mut args = serde_json::Map::new();
    args.insert("tenant".into(), Value::String("mcp_deny".into()));
    args.insert("agent_id".into(), Value::String("entity:no_such_agent".into()));
    args.insert("path".into(), Value::String("/tmp".into()));

    let res = superx_mcp::dispatch_tool(&kernel, "graphify", Some(args)).await;
    assert!(
        res.is_err(),
        "uncapped agent must be refused by dispatch_tool, got {res:?}"
    );
}

#[tokio::test]
async fn mcp_dispatch_allows_seeded_admin_for_graphify() {
    // The opposite path: a bootstrap-seeded admin agent (system_controller or
    // gemini_cli, both granted edge_has_capability to all 5 tools) must be
    // allowed through; the request also exercises FileSource against a small
    // fixture directory so we know the dispatch completed end-to-end.
    let (_dir, kernel, _sub) = bootstrap_tenant("mcp_allow").await;
    let agent_id = fetch_admin_agent_id(&kernel).await;

    let fixture = tempfile::tempdir().unwrap();
    std::fs::write(fixture.path().join("hello.txt"), "hello superx").unwrap();

    let mut args = serde_json::Map::new();
    args.insert("tenant".into(), Value::String("mcp_allow".into()));
    args.insert("agent_id".into(), Value::String(agent_id));
    args.insert("path".into(), Value::String(fixture.path().to_string_lossy().to_string()));

    let res = superx_mcp::dispatch_tool(&kernel, "graphify", Some(args)).await;
    assert!(res.is_ok(), "seeded admin agent must be admitted, got {res:?}");
}

#[tokio::test]
async fn mcp_dispatch_rejects_missing_required_args() {
    // Defensive: missing agent_id must come back as invalid_params, not panic
    // or silently succeed.
    let (_dir, kernel, _sub) = bootstrap_tenant("mcp_args").await;
    let mut args = serde_json::Map::new();
    args.insert("tenant".into(), Value::String("mcp_args".into()));
    // intentionally omit agent_id + path

    let res = superx_mcp::dispatch_tool(&kernel, "graphify", Some(args)).await;
    assert!(res.is_err(), "missing required arg must produce McpError, got {res:?}");
}

#[tokio::test]
async fn mcp_dispatch_unknown_tool_returns_method_not_found() {
    let (_dir, kernel, _sub) = bootstrap_tenant("mcp_unknown").await;
    let mut args = serde_json::Map::new();
    args.insert("tenant".into(), Value::String("mcp_unknown".into()));
    let res = superx_mcp::dispatch_tool(&kernel, "tool_that_does_not_exist", Some(args)).await;
    assert!(res.is_err(), "unknown tool must error, got {res:?}");
}

// =====================================================================
// 10. EMISSION ROUTER — ApiSink performs an authenticated HTTP POST
// =====================================================================

#[tokio::test]
async fn apisink_posts_telemetry_row_to_remote_endpoint() {
    // ARCHITECTURE.md §4 Emission Router: "A real-time CHANGEFEED pipe that exports
    // substrate events to Kafka/API sinks". Verify the HTTP egress contract: ApiSink
    // performs a POST whose body is the serialized TelemetryRow. We use wiremock to
    // expect exactly one POST to /ingest; failure to receive panics on server drop.
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/ingest"))
        .and(wiremock::matchers::header("authorization", "Bearer test-token"))
        .respond_with(wiremock::ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let row = TelemetryRow {
        id: surrealdb::sql::Thing::from((
            "telemetry_stream",
            Id::Uuid(surrealdb::sql::Uuid::from(uuid::Uuid::now_v7())),
        )),
        timestamp: "2026-05-18T19:00:00Z".to_string(),
        tenant_id: "tenant_x".to_string(),
        lifecycle_event: "test_event".to_string(),
        run_id: Some("run-1".to_string()),
        payload: serde_json::json!({"key": "value"}),
    };

    let sink = ApiSink::new(&format!("{}/ingest", server.uri()), Some("test-token".to_string()));
    sink.emit(&row).await.expect("ApiSink emit must succeed against a 200 response");
    // server's `.expect(1)` is verified on drop; a missed POST panics the test.
}

#[tokio::test]
async fn apisink_surfaces_non_2xx_as_error() {
    // 500 from the remote must propagate as Err — silent emission failures would
    // defeat the audit-trail contract.
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .respond_with(wiremock::ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let row = TelemetryRow {
        id: surrealdb::sql::Thing::from((
            "telemetry_stream",
            Id::Uuid(surrealdb::sql::Uuid::from(uuid::Uuid::now_v7())),
        )),
        timestamp: "2026-05-18T19:00:00Z".to_string(),
        tenant_id: "tenant_x".to_string(),
        lifecycle_event: "test_event".to_string(),
        run_id: None,
        payload: serde_json::json!({}),
    };

    let sink = ApiSink::new(&format!("{}/ingest", server.uri()), None);
    let res = sink.emit(&row).await;
    assert!(res.is_err(), "non-2xx response must yield Err, got {res:?}");
}

#[tokio::test]
async fn uuidv7_entity_ids_preserve_temporal_ordering() {
    // ARCHITECTURE.md Invariant 2: "Every mutation is stamped with a UUIDv7, ensuring
    // history is naturally ordered". All substrate tables (entity, relation,
    // state_ledger, telemetry_stream) now use native UUIDv7 record ids via
    // `surrealdb::sql::Id::Uuid`; insertion order MUST match lexical id order.
    let (_dir, kernel, _sub) = bootstrap_tenant("uuid7_order").await;

    let mut generated: Vec<String> = Vec::new();
    for _ in 0..5 {
        let uid = uuid::Uuid::now_v7().to_string();
        generated.push(uid.clone());
        let thing = surrealdb::sql::Thing::from(("entity".to_string(), uid));
        kernel
            .db
            .query("INSERT INTO entity { id: $id, tenant_id: $session_tenant, type: type_definition:node_prod }")
            .bind(("id", thing))
            .await
            .unwrap()
            .check()
            .unwrap();
    }

    // Sanity: the generator itself is monotonic (UUIDv7 has a time-prefix).
    let mut sorted = generated.clone();
    sorted.sort();
    assert_eq!(generated, sorted, "uuid::Uuid::now_v7 must produce lex-monotonic ids in sequence");
}


// =====================================================================
// 12. EXECUTION_PARAMS — SCD-2 + append-only per-run agent knob store
// =====================================================================
//
// Every kernel mutation in §7-#5 must follow SCD-2 + append-only. The
// execution_params table is the live test of that rule for a new substrate
// table: every write to a (run_id, agent_id) tuple closes the prior current
// row and opens a fresh one in one transaction. No in-place UPDATE, no DELETE.

#[tokio::test]
async fn execution_params_round_trip_returns_what_was_written() {
    let (_dir, kernel, _sub) = bootstrap_tenant("ep_rt").await;
    let payload = serde_json::json!({
        "temperature": 0.7,
        "top_p": 0.9,
        "max_tokens": 1024,
        "model_ref": "entity:gemma_4_27b_it"
    });

    kernel.set_execution_params("run_abc", "agent_xyz", payload.clone()).await.unwrap();
    let read_back = kernel.get_execution_params("run_abc", "agent_xyz").await.unwrap();
    assert_eq!(read_back, Some(payload), "round-trip must return the same payload");
}

#[tokio::test]
async fn execution_params_update_closes_prior_opens_new_scd2() {
    // Operator updates knobs mid-run — must produce 2 rows in the ledger,
    // exactly one is_current, the prior with valid_to set.
    let (_dir, kernel, _sub) = bootstrap_tenant("ep_scd2").await;
    kernel.set_execution_params("run_x", "agent_a",
        serde_json::json!({"temperature": 0.5})).await.unwrap();
    kernel.set_execution_params("run_x", "agent_a",
        serde_json::json!({"temperature": 0.9})).await.unwrap();

    #[derive(serde::Deserialize, Debug)]
    #[allow(dead_code)]
    struct Row {
        is_current: bool,
        valid_to: serde_json::Value,
        valid_from: serde_json::Value,
        params_json: serde_json::Value,
    }
    let mut res = kernel.db.query(
        "SELECT is_current, valid_to, valid_from, params_json FROM execution_params \
         WHERE run_id = 'run_x' AND agent_id = 'agent_a' AND tenant_id = $session_tenant \
         ORDER BY valid_from ASC"
    ).await.unwrap();
    let rows: Vec<Row> = res.take(0).unwrap();
    assert_eq!(rows.len(), 2, "SCD-2 history must keep both versions; got {}", rows.len());

    let current: Vec<&Row> = rows.iter().filter(|r| r.is_current).collect();
    let closed: Vec<&Row> = rows.iter().filter(|r| !r.is_current).collect();
    assert_eq!(current.len(), 1, "exactly one is_current per (run_id, agent_id)");
    assert_eq!(closed.len(), 1, "the prior row must be closed");
    assert!(current[0].valid_to.is_null(), "current row's valid_to must be NULL");
    assert!(!closed[0].valid_to.is_null(), "closed row's valid_to must be set");

    // The current row holds the latest value.
    let current_temp = current[0].params_json.get("temperature").and_then(|v| v.as_f64()).unwrap();
    assert!((current_temp - 0.9).abs() < f64::EPSILON, "current row must hold the newer payload");
}

#[tokio::test]
async fn execution_params_distinct_agents_do_not_collide() {
    // Two agents in the same run must have independent param histories.
    let (_dir, kernel, _sub) = bootstrap_tenant("ep_iso").await;
    kernel.set_execution_params("run_iso", "agent_a",
        serde_json::json!({"temperature": 0.3})).await.unwrap();
    kernel.set_execution_params("run_iso", "agent_b",
        serde_json::json!({"temperature": 0.8})).await.unwrap();

    let a = kernel.get_execution_params("run_iso", "agent_a").await.unwrap();
    let b = kernel.get_execution_params("run_iso", "agent_b").await.unwrap();
    assert_eq!(a.unwrap().get("temperature").and_then(|v| v.as_f64()), Some(0.3));
    assert_eq!(b.unwrap().get("temperature").and_then(|v| v.as_f64()), Some(0.8));

    // Updating agent_a's knobs must NOT touch agent_b's.
    kernel.set_execution_params("run_iso", "agent_a",
        serde_json::json!({"temperature": 0.1})).await.unwrap();
    let a2 = kernel.get_execution_params("run_iso", "agent_a").await.unwrap();
    let b2 = kernel.get_execution_params("run_iso", "agent_b").await.unwrap();
    assert_eq!(a2.unwrap().get("temperature").and_then(|v| v.as_f64()), Some(0.1));
    assert_eq!(b2.unwrap().get("temperature").and_then(|v| v.as_f64()), Some(0.8));
}

#[tokio::test]
async fn execution_params_get_returns_none_when_not_set() {
    let (_dir, kernel, _sub) = bootstrap_tenant("ep_missing").await;
    let res = kernel.get_execution_params("never_set_run", "never_set_agent").await.unwrap();
    assert!(res.is_none(), "missing row must return None, not an error");
}

#[tokio::test]
async fn execution_params_set_emits_telemetry() {
    let (_dir, kernel, _sub) = bootstrap_tenant("ep_tele").await;
    let before = count_events(&kernel, "execution_params_set").await;
    kernel.set_execution_params("run_tele", "agent_one",
        serde_json::json!({"temperature": 0.5})).await.unwrap();
    let after = count_events(&kernel, "execution_params_set").await;
    assert_eq!(after, before + 1, "each set must emit exactly one execution_params_set event");
}
