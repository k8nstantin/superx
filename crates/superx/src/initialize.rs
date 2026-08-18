//! `superx --initialize` — one-command provisioning (issue #120).
//!
//! Operator directive (2026-08-18): on startup, prompt the user to
//! create the root password — at this phase ANY password is accepted —
//! then create the database with the schema, initialize everything,
//! and start gathering data. Runs for every new instance; re-running
//! against an existing instance skips provisioning and boots.
//!
//! Decisions (BLUEPRINT.md D11–D12): one password serves both the
//! database root and the `superx_kernel` service account at this
//! phase; the service password is persisted to a `0600` credentials
//! file next to the datastore so the query commands work in any
//! terminal without exports. The root password itself is used once,
//! in-process, to apply the locked schema, and is never stored.

use std::io::IsTerminal as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use superx_kernel::Kernel;

use crate::ConnectionArgs;

/// Env var that always wins over the credentials file.
pub const PASSWORD_ENV: &str = "SUPERX_KERNEL_PASSWORD";

/// How long to wait for a freshly spawned server to accept
/// connections.
const SERVER_READY_TIMEOUT_SECS: u64 = 30; // skill-allow: §9-const — provisioning timeout bound
const SERVER_POLL_MS: u64 = 300; // skill-allow: §9-const — provisioning poll cadence

/// The credentials file for an instance: sibling of the datastore
/// directory, named `superx-credentials`.
#[must_use]
pub fn credentials_path(data_dir: &Path) -> PathBuf {
    data_dir
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("superx-credentials")
}

/// Resolve the service-account password: env var first, then the
/// instance credentials file.
pub fn resolve_password(data_dir: &Path) -> Option<String> {
    if let Ok(pw) = std::env::var(PASSWORD_ENV) {
        if !pw.is_empty() {
            return Some(pw);
        }
    }
    let path = credentials_path(data_dir);
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim_end_matches(['\r', '\n']).to_string())
        .filter(|s| !s.is_empty())
}

/// Persist the service password, `0600`, one line.
pub fn save_credentials(data_dir: &Path, password: &str) -> Result<PathBuf, String> {
    let path = credentials_path(data_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create {parent:?}: {e}"))?;
    }
    std::fs::write(&path, format!("{password}\n")).map_err(|e| format!("write {path:?}: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("chmod {path:?}: {e}"))?;
    }
    Ok(path)
}

/// `ws://127.0.0.1:8000` → `127.0.0.1:8000` (the server bind address).
pub fn bind_from_endpoint(endpoint: &str) -> String {
    endpoint
        .trim_start_matches("ws://")
        .trim_start_matches("wss://")
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .trim_end_matches('/')
        .to_string()
}

/// Prompt for the instance password. Hidden input on a TTY; plain
/// line-read otherwise (scripted QA). Re-prompts on empty — any
/// non-empty password is accepted (operator directive: no strength
/// requirements at this phase).
pub fn prompt_password() -> Result<String, String> {
    loop {
        let pw = if std::io::stdin().is_terminal() {
            rpassword::prompt_password("Create the root password for this SuperX instance: ")
                .map_err(|e| format!("password prompt: {e}"))?
        } else {
            let mut line = String::new();
            std::io::stdin()
                .read_line(&mut line)
                .map_err(|e| format!("password read: {e}"))?;
            line.trim_end_matches(['\r', '\n']).to_string()
        };
        if pw.is_empty() {
            crate::emit("password cannot be empty — try again\n");
            continue;
        }
        return Ok(pw);
    }
}

use superx_kernel::provision::server_reachable;

/// Spawn `surreal start` detached (survives our ctrl-c: own process
/// group) with server logs beside the datastore. Credentials flags
/// only matter on a datastore's FIRST start; passing them on an
/// existing one is harmless.
fn spawn_server(
    bind: &str,
    data_dir: &Path,
    root_password: Option<&str>,
) -> Result<(), String> {
    let log_path = data_dir
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("surreal-server.log");
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create {parent:?}: {e}"))?;
    }
    let log = std::fs::File::create(&log_path).map_err(|e| format!("open {log_path:?}: {e}"))?;
    let log_err = log.try_clone().map_err(|e| format!("clone log handle: {e}"))?;

    let mut cmd = std::process::Command::new("surreal");
    cmd.arg("start").arg("--bind").arg(bind);
    if let Some(pw) = root_password {
        cmd.arg("--user").arg("root").arg("--pass").arg(pw);
    }
    cmd.arg(format!("rocksdb:{}", data_dir.display()))
        .stdout(log)
        .stderr(log_err)
        .stdin(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        cmd.process_group(0);
    }
    cmd.spawn().map_err(|e| {
        format!(
            "cannot start `surreal` ({e}) — install it with:\n  curl --proto '=https' \
             --tlsv1.2 -sSf https://install.surrealdb.com | sh"
        )
    })?;
    Ok(())
}

async fn wait_ready(endpoint: &str) -> Result<(), String> {
    let deadline =
        std::time::Instant::now() + Duration::from_secs(SERVER_READY_TIMEOUT_SECS);
    while std::time::Instant::now() < deadline {
        if server_reachable(endpoint).await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(SERVER_POLL_MS)).await;
    }
    Err(format!(
        "server did not become ready within {SERVER_READY_TIMEOUT_SECS}s — see the \
         surreal-server.log next to the datastore"
    ))
}

/// The full `--initialize` flow. Returns a connected service-account
/// kernel, ready for boot.
pub async fn initialize(conn: &ConnectionArgs, data_dir: &Path) -> Result<Kernel, String> {
    // Already provisioned and reachable? Straight to boot.
    if let Some(pw) = resolve_password(data_dir) {
        if let Ok(kernel) = Kernel::connect_service_with_password(
            &conn.endpoint,
            &conn.namespace,
            &conn.database,
            &pw,
        )
        .await
        {
            crate::emit("instance already initialized — booting\n");
            return Ok(kernel);
        }
    }

    let fresh = !data_dir.exists();
    let password = if fresh {
        prompt_password()?
    } else {
        crate::emit("existing datastore found without working credentials\n");
        crate::emit("enter this instance's root password to re-provision access\n");
        prompt_password()?
    };

    let bind = bind_from_endpoint(&conn.endpoint);
    if server_reachable(&conn.endpoint).await {
        crate::emit(&format!("server already running at {}\n", conn.endpoint));
    } else {
        crate::emit(&format!(
            "starting SurrealDB at {} (datastore {})\n",
            bind,
            data_dir.display()
        ));
        spawn_server(&bind, data_dir, fresh.then_some(password.as_str()))?;
        wait_ready(&conn.endpoint).await?;
    }

    crate::emit("applying the kernel schema…\n");
    superx_kernel::provision::apply_schema_as_root(
        &conn.endpoint,
        &conn.namespace,
        &conn.database,
        &password,
        &password, // one password for root + service at this phase (D11)
    )
    .await
    .map_err(|e| e.to_string())?;

    let saved = save_credentials(data_dir, &password)?;
    crate::emit(&format!(
        "credentials saved to {} (0600) — query commands need no exports\n",
        saved.display()
    ));

    Kernel::connect_service_with_password(
        &conn.endpoint,
        &conn.namespace,
        &conn.database,
        &password,
    )
    .await
    .map_err(|e| format!("service signin after provisioning failed: {e}"))
}
