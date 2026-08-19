//! Provisioning primitives for `superx --initialize` — the operator's
//! one-shot tool (issue #120, operator directive 2026-08-18).
//!
//! **§13 carve-out, explicit and narrow:** the functions here use a
//! ROOT session. They exist solely for the operator-invoked
//! `--initialize` flow: the operator types the root password
//! interactively, it is used once in-process to apply the locked
//! schema, and it is never stored. No runtime code path — no verb, no
//! module, no adapter — may call into this module. The kernel's
//! runtime connection is `superx_kernel`, always.

use surrealdb::engine::any::connect;
use surrealdb::opt::auth::Root; // skill-allow: §13-init — operator-invoked --initialize carve-out (issue #120)

use crate::error::{KernelError, Result};
use crate::schema::SCHEMA_DDL;

/// Is a server answering at the endpoint? (Cheap connect probe.)
pub async fn server_reachable(endpoint: &str) -> bool {
    connect(endpoint).await.is_ok()
}

/// Escape a password for embedding in a single-quoted SurrealQL
/// string (the `DEFINE USER … PASSWORD '…'` statement).
#[must_use]
pub fn escape_surql(pw: &str) -> String {
    pw.replace('\\', "\\\\").replace('\'', "\\'")
}

/// Apply the locked kernel schema under a root session, creating the
/// `superx_kernel` service account with `kernel_password`. The root
/// password is used here and only here.
///
/// # Errors
///
/// [`KernelError::Db`] for connection errors; [`KernelError::Config`]
/// for a refused root signin or a refused schema statement (the
/// message says which).
pub async fn apply_schema_as_root(
    endpoint: &str,
    namespace: &str,
    database: &str,
    root_password: &str,
    kernel_password: &str,
) -> Result<()> {
    let db = connect(endpoint).await?;
    db.signin(Root {
        username: "root".to_string(), // skill-allow: §13-username — operator types this password; used once, never stored
        password: root_password.to_string(),
    })
    .await
    .map_err(|e| KernelError::Config(format!("root signin refused: {e}")))?;
    db.use_ns(namespace).use_db(database).await?;
    let ddl = SCHEMA_DDL.replace("$SUPERX_KERNEL_PASSWORD", &escape_surql(kernel_password));
    db.query(ddl)
        .await?
        .check()
        .map_err(|e| KernelError::Config(format!("schema statement refused: {e}")))?;
    Ok(())
}

/// Provision one MODULE's own database (`superx/<name>`): apply its
/// schema DDL under a one-shot root session, substituting the
/// module's service-account password (`$SUPERX_MODULE_PASSWORD`;
/// D11 single-password phase). Same operator-invoked carve-out as
/// [`apply_schema_as_root`] — no runtime path calls this.
///
/// # Errors
///
/// [`KernelError::Db`] / [`KernelError::Config`] as for
/// [`apply_schema_as_root`].
pub async fn provision_module_schema(
    endpoint: &str,
    module_name: &str,
    root_password: &str,
    module_password: &str,
    ddl: &str,
) -> Result<()> {
    let db = connect(endpoint).await?;
    db.signin(Root {
        username: "root".to_string(), // skill-allow: §13-username — operator-invoked provisioning carve-out (epic #141)
        password: root_password.to_string(),
    })
    .await
    .map_err(|e| KernelError::Config(format!("root signin refused: {e}")))?;
    db.use_ns("superx").use_db(module_name).await?;
    let ddl = ddl.replace("$SUPERX_MODULE_PASSWORD", &escape_surql(module_password));
    db.query(ddl)
        .await?
        .check()
        .map_err(|e| KernelError::Config(format!("module schema statement refused: {e}")))?;
    Ok(())
}
