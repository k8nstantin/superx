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

/// Pin every linked adapter's root parameter to an empty directory so
/// no test ever reads REAL machine data (un-parameterized adapters
/// fall back to `$HOME` locations that exist on dev machines). Each
/// test then re-points its own adapter at its fixture — parameters
/// supersede, latest wins.
pub async fn isolate_adapters(
    kernel: &Kernel,
    empty_root: &std::path::Path,
) -> Result<(), Box<dyn Error>> {
    use superx_kernel::registry::KernelModule as _;
    use superx_kernel::types::Value;
    std::fs::create_dir_all(empty_root)?;
    let root = Value::String(empty_root.to_string_lossy().to_string());

    let cc = kernel
        .register_module(&superx_kernel::adapters::claude_code::ClaudeCodeAdapter.descriptor())
        .await?;
    kernel
        .set_parameter(
            cc,
            superx_kernel::adapters::claude_code::PROJECTS_ROOT_PARAM,
            root.clone(),
        )
        .await?;
    let gm = kernel
        .register_module(&superx_kernel::adapters::gemini_cli::GeminiCliAdapter.descriptor())
        .await?;
    kernel
        .set_parameter(
            gm,
            superx_kernel::adapters::gemini_cli::ROOT_PARAM,
            root.clone(),
        )
        .await?;
    let cd = kernel
        .register_module(
            &superx_kernel::adapters::claude_desktop::ClaudeDesktopAdapter.descriptor(),
        )
        .await?;
    kernel
        .set_parameter(
            cd,
            superx_kernel::adapters::claude_desktop::LOGS_ROOT_PARAM,
            root,
        )
        .await?;
    Ok(())
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
