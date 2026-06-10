//! Shared fixture for kernel integration tests.
//!
//! Mirrors the `tests/substrate.rs` setup: a fresh `mem://` engine with
//! the locked kernel DDL applied (password placeholder substituted),
//! signed in as `superx_kernel`. Adds a metamodel seeder for tests that
//! exercise registry / lifecycle verbs — in production that seeding is
//! `superx-kernel-bootstrap`'s job (roadmap F6), so tests stand in for
//! it here.

use std::error::Error;

use surrealdb::engine::any::connect;
use surrealdb::opt::auth::Database;

use superx_kernel::{Kernel, REQUIRED_METAMODEL_TYPES, SCHEMA_DDL};

const TEST_PASSWORD: &str = "test-kernel-password-for-mem-engine";
const TEST_NS: &str = "superx";
const TEST_DB: &str = "kernel";

/// Spin up a mem:// substrate with the kernel schema applied + sign
/// in as superx_kernel. Returns a ready Kernel.
pub async fn fresh_kernel() -> Result<Kernel, Box<dyn Error>> {
    let db = connect("mem://").await?;
    db.use_ns(TEST_NS).use_db(TEST_DB).await?;
    let ddl = SCHEMA_DDL.replace("$SUPERX_KERNEL_PASSWORD", TEST_PASSWORD);
    db.query(ddl).await?.check()?;
    db.signin(Database {
        namespace: TEST_NS.to_string(),
        database: TEST_DB.to_string(),
        username: "superx_kernel".to_string(),
        password: TEST_PASSWORD.to_string(),
    })
    .await?;
    Ok(Kernel::from_db(db))
}

/// Seed every `REQUIRED_METAMODEL_TYPES` row — the registry / lifecycle
/// verbs resolve `node_kernel_module`, `node_contribution`,
/// `attr_module_descriptor`, `attr_module_status`, and
/// `attr_lifecycle_state` through `find_type`, so they must exist
/// before those verbs are called.
pub async fn fresh_seeded_kernel() -> Result<Kernel, Box<dyn Error>> {
    let kernel = fresh_kernel().await?;
    for t in REQUIRED_METAMODEL_TYPES {
        kernel
            .ensure_type_definition(t.uid, t.category, t.memory_tier)
            .await?;
    }
    Ok(kernel)
}
