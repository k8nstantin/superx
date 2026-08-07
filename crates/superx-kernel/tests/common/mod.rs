//! Shared fixture for kernel integration tests.
//!
//! A fresh `mem://` engine with the locked kernel DDL applied
//! (password placeholder substituted), signed in as `superx_kernel`.
//! In production the schema is applied once by the operator under root
//! (skill §11); tests stand in for that step here.
#![allow(dead_code)] // skill-allow: allow — shared test fixture; each test binary links a subset of these helpers

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

/// [`fresh_kernel`] + every `REQUIRED_METAMODEL_TYPES` row seeded —
/// registry / lifecycle / parameter verbs resolve types through
/// `find_type`, so they must exist first. In production that seeding
/// is `boot()`'s job; tests stand in for it here.
pub async fn fresh_seeded_kernel() -> Result<Kernel, Box<dyn Error>> {
    let kernel = fresh_kernel().await?;
    for t in REQUIRED_METAMODEL_TYPES {
        kernel
            .ensure_type_definition(t.uid, t.category, t.memory_tier)
            .await?;
    }
    Ok(kernel)
}
