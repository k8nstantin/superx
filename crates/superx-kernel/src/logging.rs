//! Kernel self-log — the OS's own rolling log file, independent of
//! the substrate.
//!
//! Substrate telemetry is the OS's audit log, but it can only exist
//! when the substrate is reachable. The self-log is the layer below:
//! boot diagnostics, connection/signin failures, and anything the
//! kernel must be able to say when the database is down. Operator
//! directive of 2026-08-07; recorded in `BLUEPRINT.md` §2.
//!
//! One file per day (`superx.log.YYYY-MM-DD`) in the log directory,
//! written through a non-blocking worker. The returned [`WorkerGuard`]
//! must be held for the life of the process — dropping it flushes and
//! stops the writer thread.

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

use crate::error::{KernelError, Result};

/// Env var overriding where the kernel writes its self-log.
pub const LOG_DIR_ENV: &str = "SUPERX_LOG_DIR";

/// Env var overriding the self-log verbosity, in `EnvFilter` syntax
/// (same grammar as `RUST_LOG`, e.g. `debug` or `superx_kernel=trace`).
pub const LOG_FILTER_ENV: &str = "SUPERX_LOG";

/// Default log directory, relative to the process working directory.
/// Bootstrap-time configuration: the self-log must exist before (and
/// without) a reachable substrate, so this cannot be a substrate
/// parameter. Operators override via `SUPERX_LOG_DIR`.
// skill-allow: §9-default — pre-substrate bootstrap config, env-overridable
pub const DEFAULT_LOG_DIR: &str = "logs";

/// Rolling-file prefix; the appender suffixes the date
/// (`superx.log.YYYY-MM-DD`).
const LOG_FILE_PREFIX: &str = "superx.log";

/// Default verbosity when `SUPERX_LOG` is unset.
// skill-allow: §9-default — pre-substrate bootstrap config, env-overridable
const DEFAULT_FILTER: &str = "info";

/// Initialize the kernel self-log into the directory named by
/// `SUPERX_LOG_DIR`, falling back to [`DEFAULT_LOG_DIR`].
///
/// # Errors
///
/// See [`init`].
pub fn init_default() -> Result<WorkerGuard> {
    let dir = std::env::var(LOG_DIR_ENV).unwrap_or_else(|_| DEFAULT_LOG_DIR.to_string());
    init(Path::new(&dir))
}

/// Initialize the kernel self-log into `log_dir` (created if absent)
/// and install the global tracing subscriber.
///
/// Returns the [`WorkerGuard`] the caller must hold for the life of
/// the process; dropping it flushes buffered lines and stops the
/// writer.
///
/// # Errors
///
/// [`KernelError::Config`] if the directory cannot be created or a
/// global subscriber is already installed (the self-log is
/// initialized exactly once, by the binary's entry point).
pub fn init(log_dir: &Path) -> Result<WorkerGuard> {
    let filter = std::env::var(LOG_FILTER_ENV).unwrap_or_else(|_| DEFAULT_FILTER.to_string());
    init_with_filter(log_dir, &filter)
}

/// [`init`] with the verbosity filter supplied by the caller —
/// the CLI resolves it (flag > env > params file) before calling.
///
/// # Errors
///
/// As for [`init`].
pub fn init_with_filter(log_dir: &Path, filter: &str) -> Result<WorkerGuard> {
    std::fs::create_dir_all(log_dir).map_err(|e| {
        KernelError::Config(format!(
            "cannot create log directory {}: {e}",
            log_dir.display()
        ))
    })?;

    let appender = tracing_appender::rolling::daily(log_dir, LOG_FILE_PREFIX);
    let (writer, guard) = tracing_appender::non_blocking(appender);

    let filter = EnvFilter::new(filter);

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false)
        .try_init()
        .map_err(|e| KernelError::Config(format!("tracing subscriber already set: {e}")))?;

    Ok(guard)
}
