/*
 * SuperX Integration Tests - Revision 42.14 (NASA Hardened)
 *
 * End-to-end smoke test: bootstrap → ingest → propose → promote → verify
 * the structural `edge_promotes` link landed on the substrate.
 *
 * Copyright (c) 2026 Constantin Alexander <constantin@dedomena.io>
 */

use superx_bootstrap::BootstrapBlade;
use superx_harness::MetaHarness;
use superx_ingest::{JsonSource, UniversalIngestor};
use superx_kernel::Kernel;
use surrealdb::sql::Thing;
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
    assert!(substrate_id.starts_with("entity:"));
    let substrate_thing = Kernel::parse_id(&substrate_id).expect("substrate id parses");

    // 2. Ingestion (Universal Source)
    let ingestor = UniversalIngestor::new(&kernel);
    let source = Box::new(JsonSource {
        data: serde_json::json!({"text": "Component A implementation details"}),
    });
    let _root_id = ingestor.ingest(source, run_id).await.expect("Ingestion failed");

    // 3. Agentic Proposal — create a proposal entity via the kernel's typed
    //    CREATE path (no raw INSERT, no string-typed ids).
    let proposal_uuid = uuid::Uuid::now_v7();
    let proposal_thing = Thing::from((
        "entity",
        surrealdb::sql::Id::Uuid(surrealdb::sql::Uuid::from(proposal_uuid)),
    ));
    let proposal_id = format!("entity:{proposal_uuid}");
    let node_proposal = kernel.type_thing("node_proposal").unwrap();
    let tenant_thing = kernel.session_tenant_thing().await.unwrap();
    kernel
        .db
        .query("CREATE entity CONTENT { \
            id: $id, type: $type, tenant: $tenant, role: 'user' \
        }")
        .bind(("id", proposal_thing.clone()))
        .bind(("type", node_proposal))
        .bind(("tenant", tenant_thing.clone()))
        .await
        .unwrap()
        .check()
        .unwrap();

    kernel
        .supersede_state(
            &proposal_id,
            "attr_score",
            serde_json::json!({"score": 0.95}),
            Some(run_id.to_string()),
        )
        .await
        .unwrap();

    // 4. Meta-Harness Promotion
    let meta = MetaHarness::new(&kernel);
    let promoted = meta.promote(&proposal_id, 0.8).await.expect("Promotion failed");
    assert!(promoted, "score 0.95 ≥ 0.8 must promote");

    // 5. Session-tenant identity check — the kernel's typed accessor returns
    //    the same `Thing` we resolved from the substrate id literal.
    let session_thing = kernel.session_tenant_thing().await.unwrap();
    assert_eq!(session_thing, substrate_thing, "session must be bound to the substrate row");

    // 6. Physical Verification — `edge_promotes` relation from proposal → substrate.
    let edge_promotes = kernel.type_thing("edge_promotes").unwrap();
    #[derive(serde::Deserialize)]
    #[allow(dead_code)]
    struct EdgeRow { id: Thing }
    let mut res = kernel
        .db
        .query("SELECT id FROM relation \
                WHERE in = $proposal AND out = $sub AND type = $ty AND tenant = $tenant")
        .bind(("proposal", proposal_thing))
        .bind(("sub", substrate_thing))
        .bind(("ty", edge_promotes))
        .bind(("tenant", tenant_thing))
        .await
        .unwrap();
    let edges: Vec<EdgeRow> = res.take(0).unwrap_or_default();
    assert_eq!(
        edges.len(),
        1,
        "promotion must create exactly one edge_promotes relation from proposal to substrate"
    );
}
