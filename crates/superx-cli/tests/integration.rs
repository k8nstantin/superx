/*
 * SuperX Integration Tests - Revision 42.14 (NASA Hardened)
 * 
 * Copyright (c) 2026 Constantin Alexander <constantin@dedomena.io>
 */

use superx_kernel::Kernel;
use superx_bootstrap::BootstrapBlade;
use superx_ingest::{UniversalIngestor, JsonSource};
use superx_harness::MetaHarness;
use tempfile::tempdir;

#[tokio::test]
async fn test_nasa_cognitive_loop_end_to_end() {
    let _ = tracing_subscriber::fmt::try_init();
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("superx.db");
    let kernel = Kernel::init(&db_path, "test", "test").await.unwrap();
    let tenant = "sa_nasa";
    let run_id = "test_run_1";

    // 1. Bootstrap (Physical Isolation + Agent Discovery)
    let boot = BootstrapBlade::new(&kernel);
    let substrate_id = boot.run(tenant).await.expect("Bootstrap failed");
    assert!(substrate_id.contains("entity:"));

    // 2. Ingestion (Universal Source)
    let ingestor = UniversalIngestor::new(&kernel);
    let source = Box::new(JsonSource { data: serde_json::json!({"text": "Component A implementation details"}) });
    let _root_id = ingestor.ingest(source, run_id).await.expect("Ingestion failed");

    // 3. Agentic Proposal (Inference-less mock for structural check)
    let proposal_uid = uuid::Uuid::now_v7().to_string();
    let proposal_thing = surrealdb::sql::Thing::from(("entity".to_string(), proposal_uid.clone()));
    let proposal_id = format!("entity:{proposal_uid}");
    
    // Propose an implements relation
    kernel.db.query("INSERT INTO entity { id: $id, tenant_id: $session_tenant, type: type_definition:node_proposal }")
        .bind(("id", proposal_thing)).await.unwrap().check().unwrap();

    kernel.supersede_state(&proposal_id, "attr_score", serde_json::json!({"score": 0.95}), Some(run_id.to_string())).await.unwrap();

    // 4. Meta-Harness Promotion
    let meta = MetaHarness::new(&kernel);
    let promoted = meta.promote(&proposal_id, 0.8).await.expect("Promotion failed");
    assert!(promoted);

    let mut who_res = kernel.db.query("RETURN <string>$session_tenant").await.unwrap();
    let who: Option<String> = who_res.take(0).unwrap();
    assert_eq!(who, Some(tenant.to_string()));

    // 5. Physical Verification: Check for edge_promotes link to substrate
    let proposal_thing_v = surrealdb::sql::Thing::from(("entity".to_string(), proposal_uid.clone()));
    let substrate_id_only = substrate_id.split(':').nth(1).unwrap();
    let substrate_thing_v = surrealdb::sql::Thing::from(("entity".to_string(), substrate_id_only.to_string()));
    
    println!("DEBUG: Verifying edge from {} to {}", proposal_uid, substrate_id_only);

    let mut all_rels_res = kernel.db.query("SELECT <string>id as id, <string>in as in, <string>out as out, <string>type as type, tenant_id FROM relation").await.unwrap();
    let all_rels: Vec<serde_json::Value> = all_rels_res.take(0).unwrap();
    println!("DEBUG: All relations in substrate: {}", serde_json::to_string_pretty(&all_rels).unwrap());

    let mut res = kernel.db.query("SELECT <string>id as id FROM relation WHERE in = $id AND out = $sub AND type = type_definition:edge_promotes")
        .bind(("id", proposal_thing_v))
        .bind(("sub", substrate_thing_v)).await.unwrap();
    
    let edges: Vec<serde_json::Value> = res.take(0).unwrap_or_default();
    assert_eq!(edges.len(), 1, "Proposal must be physically linked to substrate after promotion");

    println!("NASA Cognitive Loop Verified: Bootstrap -> Ingest -> Propose -> Promote");
}
