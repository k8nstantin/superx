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

// ─────────────────────────────────────────────────────────────────────
// Per-module log routing (epic #141 P2): events whose tracing
// `target` names a compiled-in module are ALSO written to that
// module's own file at `<modules_root>/<name>/logs/<name>.log.<date>`.
// Files are created lazily on the module's first event; writes are
// synchronous (module logs are low-volume; determinism beats
// buffering here). The main self-log keeps the merged stream.
// ─────────────────────────────────────────────────────────────────────

use std::collections::HashMap;
use std::io::Write as _;
use std::sync::Mutex;

use tracing::field::{Field, Visit};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

struct ModuleLogLayer {
    root: std::path::PathBuf,
    names: &'static [&'static str],
    files: Mutex<HashMap<String, std::fs::File>>,
}

struct LineVisitor(String);

impl Visit for LineVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0.push_str(&format!(" {value:?}"));
        } else {
            self.0.push_str(&format!(" {}={value:?}", field.name()));
        }
    }
}

impl<S> Layer<S> for ModuleLogLayer
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let target = event.metadata().target();
        if !self.names.contains(&target) {
            return;
        }
        let mut line = format!(
            "{} {:>5} ",
            chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.6fZ"),
            event.metadata().level()
        );
        let mut visitor = LineVisitor(String::new());
        event.record(&mut visitor);
        line.push_str(visitor.0.trim_start());
        line.push('\n');

        let Ok(mut files) = self.files.lock() else { return };
        if !files.contains_key(target) {
            let dir = self.root.join(target).join("logs");
            if std::fs::create_dir_all(&dir).is_err() {
                return;
            }
            let path = dir.join(format!(
                "{target}.log.{}",
                chrono::Utc::now().format("%Y-%m-%d")
            ));
            match std::fs::OpenOptions::new().create(true).append(true).open(path) {
                Ok(f) => {
                    files.insert(target.to_string(), f);
                }
                Err(_) => return,
            }
        }
        if let Some(f) = files.get_mut(target) {
            if let Err(e) = f.write_all(line.as_bytes()) {
                eprintln!("module log write failed for '{target}': {e}");
            }
        }
    }
}

/// [`init_with_filter`] plus per-module log routing: any event whose
/// `target` matches a name in `module_names` is also appended to that
/// module's own log under `modules_root`.
///
/// # Errors
///
/// As for [`init_with_filter`].
pub fn init_instance(
    log_dir: &Path,
    filter: &str,
    modules_root: &Path,
    module_names: &'static [&'static str],
) -> Result<WorkerGuard> {
    use tracing_subscriber::layer::SubscriberExt as _;
    use tracing_subscriber::util::SubscriberInitExt as _;

    std::fs::create_dir_all(log_dir).map_err(|e| {
        KernelError::Config(format!(
            "cannot create log directory {}: {e}",
            log_dir.display()
        ))
    })?;
    let appender = tracing_appender::rolling::daily(log_dir, LOG_FILE_PREFIX);
    let (writer, guard) = tracing_appender::non_blocking(appender);

    let main_layer = tracing_subscriber::fmt::layer()
        .with_writer(writer)
        .with_ansi(false)
        .with_filter(EnvFilter::new(filter));
    let module_layer = ModuleLogLayer {
        root: modules_root.to_path_buf(),
        names: module_names,
        files: Mutex::new(HashMap::new()),
    };

    tracing_subscriber::registry()
        .with(main_layer)
        .with(module_layer)
        .try_init()
        .map_err(|e| KernelError::Config(format!("tracing subscriber already set: {e}")))?;
    Ok(guard)
}
