//! Integration test: spin up an embedded SurrealDB via the `mem://`
//! engine, apply the locked DDL (with the `$SUPERX_SERVICE_PASSWORD`
//! placeholder substituted for a test value), and verify that
//! signin as the `superx` EDITOR service account succeeds against
//! the just-applied schema.
//!
//! The Mem engine does not require an auth context for raw `query()`
//! calls, so the schema applies in one shot without root credentials.
//! Production paths apply the schema via the operator-owned
//! `scripts/deploy-schema.sh` under root, then have the kernel sign
//! in as `superx` against the running server.
//!
//! `log_telemetry` is exercised once the next PR ships the
//! root-tenant bootstrap pattern (the `entity.tenant` FK ASSERT
//! `$value.type.uid = 'node_substrate'` requires a pre-existing
//! tenant entity, which is itself a chicken-and-egg the bootstrap
//! design must resolve — that is operator-approval territory per
//! SKILL.md §7 and lands as its own slice).

use std::error::Error;
use surrealdb::engine::any::connect;
use surrealdb::opt::auth::Database;

use superx_kernel::SCHEMA_DDL;

const TEST_SERVICE_PASSWORD: &str = "test-service-password-for-mem-engine";
const TEST_NS: &str = "superx";
const TEST_DB: &str = "v01";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn signin_as_superx_succeeds_against_freshly_applied_schema(
) -> Result<(), Box<dyn Error>> {
    // Provision substrate: spin up the embedded mem engine and
    // apply the locked DDL with the test password substituted into
    // the DEFINE USER placeholder.
    let db = connect("mem://").await?;
    db.use_ns(TEST_NS).use_db(TEST_DB).await?;

    let ddl = SCHEMA_DDL.replace("$SUPERX_SERVICE_PASSWORD", TEST_SERVICE_PASSWORD);
    db.query(ddl).await?.check()?;

    // Sign in as `superx` — the DEFINE USER statement above just
    // created the user with EDITOR role and 1h session. This is the
    // exact signin shape `Kernel::connect_service` performs against a
    // production server.
    db.signin(Database {
        namespace: TEST_NS.to_string(),
        database: TEST_DB.to_string(),
        username: "superx".to_string(),
        password: TEST_SERVICE_PASSWORD.to_string(),
    })
    .await?;

    // Sanity: connection is alive and authenticated after signin.
    let echo: Option<i64> = db.query("RETURN 42").await?.take(0)?;
    assert_eq!(echo, Some(42));

    Ok(())
}
