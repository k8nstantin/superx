//! `superx` — operator CLI internals (BLUEPRINT.md G6).
//!
//! The binary in `main.rs` is a thin dispatcher; everything testable
//! lives here: command runners, session resolution, and the
//! human-readable renderers.

use std::io::Write as _;

use superx_kernel::Kernel;

pub mod config;
pub mod initialize;

// Modules are compiled in (epic #141 v1 contract): an explicit link
// reference per module crate keeps its linkme registration alive.
use superx_mod_entities as _;
use superx_mod_hello as _;
use superx_mod_runner as _;
use superx_mod_ui as _;

pub use clap::Parser;
pub use config::Config;

// P3 (epic #141): the shared runners/renderers live in superx-ops;
// re-exported here so every pre-extraction path stays valid.
pub use superx_ops::*;

#[derive(Debug, Parser)]
#[command(
    name = "superx",
    version,
    about = "SuperX — the agentic OS. Boots, discovers coding agents, captures their telemetry and conversations."
)]
pub struct Cli {
    /// Provision this instance end-to-end (prompt for the root
    /// password, create the database + schema, initialize everything)
    /// and start gathering data in the background. Idempotent: an
    /// initialized instance boots straight away.
    #[arg(long, global = true)]
    pub initialize: bool,
    /// The instance home: holds params/superx.json, logs/, db/.
    #[arg(long, global = true, env = "SUPERX_HOME", default_value = ".")] // skill-allow: §9-default — the instance anchor itself
    pub home: std::path::PathBuf,
    /// Where the instance's datastore lives (default from params file).
    #[arg(long, global = true, env = "SUPERX_DATA_DIR")]
    pub data_dir: Option<std::path::PathBuf>,
    /// Log directory (default from params file).
    #[arg(long, global = true, env = "SUPERX_LOG_DIR")]
    pub log_dir: Option<std::path::PathBuf>,
    /// Log verbosity, EnvFilter syntax (default from params file).
    #[arg(long, global = true, env = "SUPERX_LOG")]
    pub log_filter: Option<String>,
    #[command(flatten)]
    pub conn: ConnectionArgs,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, clap::Args)]
pub struct ConnectionArgs {
    /// SurrealDB connection URL (default from params file).
    #[arg(long, global = true, env = "SUPERX_ENDPOINT")]
    pub endpoint: Option<String>,
    /// SurrealDB namespace (default from params file).
    #[arg(long, global = true, env = "SUPERX_NAMESPACE")]
    pub namespace: Option<String>,
    /// SurrealDB database (default from params file).
    #[arg(long, global = true, env = "SUPERX_DATABASE")]
    pub database: Option<String>,
}

#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// Boot the OS in the foreground (debugging): register modules,
    /// discover agents, run capture until ctrl-c. Normal operation
    /// uses `--initialize`, which runs the OS in the background.
    Boot {
        /// Internal: this process IS the background OS (pidfile owned
        /// by the parent). Skips the duplicate-capture guard.
        #[arg(long, hide = true)]
        daemonized: bool,
    },
    /// Start the background OS on an already-initialized instance
    /// (no provisioning, no prompt).
    Start,
    /// Stop the background OS started by --initialize or start.
    Stop,
    /// Restart the background OS: stop (if running), then start —
    /// one command to pick up a freshly built binary (the schema
    /// self-upgrades on version mismatch).
    Restart,
    /// Apply pending kernel-schema deltas to this instance
    /// (tolerant re-apply; also runs automatically at start on
    /// version mismatch).
    Upgrade,
    /// Manage modules: list, enable, disable (live effect on the
    /// running OS within one capture tick).
    Modules {
        #[command(subcommand)]
        action: ModulesAction,
    },
    /// Show the OS's own log (the self-log; --daemon for the
    /// background process output).
    Logs {
        /// How many trailing lines to show.
        #[arg(short = 'n', long, default_value_t = 40)] // skill-allow: §9-default — render page size, flag-overridable
        lines: usize,
        /// Keep following as new lines arrive.
        #[arg(long)]
        follow: bool,
        /// Show the daemon stdout/stderr log instead of the self-log.
        #[arg(long)]
        daemon: bool,
    },
    /// Module + adapter lifecycle status.
    Status,
    /// Discovered agents with their session and source counts.
    Agents,
    /// The action stream (telemetry), newest first.
    Actions {
        /// How many recent events to show.
        #[arg(short = 'n', long, default_value_t = 20)]
        // skill-allow: §9-default — render page size, flag-overridable
        limit: u32,
        /// Only events concerning this agent (name, e.g. claude_code).
        #[arg(long)]
        agent: Option<String>,
        /// Keep tailing new events as they arrive.
        #[arg(long)]
        live: bool,
    },
    /// List captured conversations.
    Sessions {
        /// Only sessions of this agent (name, e.g. gemini_cli).
        #[arg(long)]
        agent: Option<String>,
    },
    /// A module's own CLI: `superx <module-name> [args…]`.
    #[command(external_subcommand)]
    Module(Vec<String>),
    /// Render one conversation, oldest first.
    Read {
        /// Session name (`claude_code/<id>`) or any unique fragment
        /// of one (id prefix works).
        session: String,
        /// Keep following the conversation as it continues.
        #[arg(long)]
        live: bool,
    },
}

#[derive(Debug, clap::Subcommand)]
pub enum ModulesAction {
    /// Compiled-in inventory × substrate state.
    List,
    /// Enable a module (starts/resumes within one tick on a running OS).
    Enable { name: String },
    /// Disable a module (pauses/stops within one tick on a running OS).
    Disable { name: String },
    /// Provision a module's OWN database (its schema + service
    /// account) — operator one-shot per module.
    Provision { name: String },
    /// Restart a module in place: shutdown() then startup(), inside
    /// one capture tick. The OS keeps running.
    Restart { name: String },
}

/// Connect + signin with an actionable hint on auth refusal. The
/// password comes from `SUPERX_KERNEL_PASSWORD` or the instance
/// credentials file written by `--initialize`.
pub async fn connect(config: &Config) -> Result<Kernel, String> {
    let Some(password) = initialize::resolve_password(&config.data_dir) else {
        return Err(format!(
            "no credentials: export {} or run `superx --initialize` first",
            initialize::PASSWORD_ENV
        ));
    };
    superx_kernel::Kernel::connect_service_with_password(
        &config.endpoint,
        &config.namespace,
        &config.database,
        &password,
    )
        .await
        // Query-path kernels carry the instance home too, so module
        // CLIs can reach their own dirs (issue #170: attach).
        .map(|kernel| kernel.with_home(config.home.clone()))
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("authentication") || msg.contains("credentials") {
                format!(
                    "{msg}\nhint: the password must match the one used at initialize/deploy \
                     time — re-run `superx --initialize`, or export SUPERX_KERNEL_PASSWORD, \
                     and confirm the server at {} is this instance's",
                    config.endpoint
                )
            } else {
                msg
            }
        })
}

/// `superx logs` body (no substrate needed): tail of today's self-log
/// or the daemon log. Returns (rendered text, the file path, its
/// current length) so `--follow` can resume.
pub fn run_logs(
    config: &Config,
    lines: usize,
    daemon: bool,
) -> Result<(String, std::path::PathBuf, u64), String> {
    let path = if daemon {
        config.log_dir.join("superx-daemon.log")
    } else {
        config.log_dir.join(format!(
            "superx.log.{}",
            chrono::Utc::now().format("%Y-%m-%d")
        ))
    };
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("no log at {} ({e}) — has the OS run today?", path.display()))?;
    let len = text.len() as u64;
    let all: Vec<&str> = text.lines().collect();
    let start = all.len().saturating_sub(lines);
    let mut out = all[start..].join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    Ok((out, path, len))
}

/// `superx boot` body: boot, render the report, start the capture
/// loop, block until ctrl-c (or SIGINT from `superx stop` when
/// daemonized).
pub async fn run_boot(kernel: &Kernel) -> Result<(), String> {
    let report = superx_kernel::boot(kernel).await.map_err(|e| e.to_string())?;
    emit(&render_boot_report(&report));

    let (tx, rx) = tokio::sync::watch::channel(false);
    let loop_kernel = kernel.clone();
    let capture = tokio::spawn(async move {
        superx_kernel::capture::run_loop(&loop_kernel, rx).await
    });
    // Module lifecycle converges HERE (M0). `superx modules disable`
    // runs in a different process and cannot reach into this one's
    // memory — it writes the substrate, and this is what notices.
    superx_kernel::supervise::spawn_reconciler(kernel.clone(), tx.subscribe());
    emit("capture loop running — ctrl-c to shut down\n");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| format!("signal handler: {e}"))?;
    tracing::info!("ctrl-c received; shutting down");
    if tx.send(true).is_err() {
        tracing::warn!("capture loop already gone at shutdown signal");
    }
    // Modules get their stop notice and their shutdown() call before
    // the process exits — servers close their listeners, buffers
    // flush. Previously they were simply torn down mid-flight.
    superx_kernel::supervise::shutdown_all(kernel).await;
    match capture.await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.to_string()),
        Err(e) => Err(format!("capture task: {e}")),
    }
}

/// `superx start` body: background-boot an already-initialized
/// instance. Provisioning belongs to `--initialize`; this connects
/// with existing credentials and hands off to the (duplicate-guarded)
/// background starter.
pub async fn run_start(config: &Config) -> Result<(), String> {
    if initialize::resolve_password(&config.data_dir).is_none() {
        return Err(
            "this instance is not initialized — run `superx --initialize` first".to_string(),
        );
    }
    let kernel = connect(config).await.map_err(|e| {
        format!("{e}
hint: is the database server up? `superx --initialize` also restarts it")
    })?;
    initialize::start_background_os(&kernel, config).await
}

/// `superx stop` body: SIGINT the background OS, wait for exit.
pub async fn run_stop(data_dir: &std::path::Path) -> Result<String, String> {
    let Some(pid) = initialize::read_live_pid(data_dir) else {
        return Ok("OS is not running\n".to_string());
    };
    let ok = std::process::Command::new("kill")
        .arg("-INT")
        .arg(pid.to_string())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        return Err(format!("could not signal pid {pid}"));
    }
    // Graceful shutdown: the loop stops between source polls, so this
    // normally lands in seconds even mid-backfill.
    for i in 0..300 {
        if !initialize::pid_alive(pid) {
            let _removed = std::fs::remove_file(initialize::pid_path(data_dir));
            return Ok(format!("OS stopped (pid {pid})\n"));
        }
        if i == 50 {
            emit("waiting for the current source poll to finish…\n");
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await; // skill-allow: §9-duration — stop-poll cadence, bounded loop
    }
    Err(format!(
        "OS (pid {pid}) is still shutting down — check `superx status` shortly"
    ))
}

/// `superx modules provision <name>` body: apply the module's own
/// schema into its own database under the operator path (D11
/// single-password phase — the stored password IS the root password).
pub async fn run_modules_provision(
    config: &Config,
    kernel: &Kernel,
    name: &str,
) -> Result<String, String> {
    let desc = resolve_module(kernel, name).await?;
    let name = desc.name;
    let module = superx_kernel::KERNEL_MODULES
        .iter()
        .find(|m| m.descriptor().name == name)
        .expect("resolved from inventory");
    let Some(ddl) = module.schema_ddl() else {
        return Ok(format!("module '{name}' owns no data objects — nothing to provision\n"));
    };
    let Some(password) = initialize::resolve_password(&config.data_dir) else {
        return Err("no credentials — run `superx --initialize` first".to_string());
    };
    superx_kernel::provision::provision_module_schema(
        &config.endpoint,
        name,
        &password,
        &password, // D11: one password serves root + all service accounts this phase
        ddl,
    )
    .await
    .map_err(|e| e.to_string())?;
    // Record the fact on the module's registry entity + firehose.
    if let Ok(Some(entity)) = kernel
        .find_module_by_name(module.descriptor().kind, name)
        .await
    {
        let _ignored = kernel
            .set_parameter(
                entity,
                "attr_provisioned",
                superx_kernel::types::Value::Bool(true),
            )
            .await;
    }
    // v2.2: the ledger records the provisioning fact first-class.
    if let Ok(Some(entity)) = kernel.find_module_by_name(desc.kind, name).await {
        kernel
            .append_module_record(&desc, entity, true)
            .await
            .map_err(|e| e.to_string())?;
    }
    kernel
        .log_telemetry(
            "module_provisioned",
            superx_kernel::types::Value::String(name.to_string()),
            None,
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(format!(
        "module '{name}' provisioned: own database superx/{name} + service account superx_mod_{name}\n\
         restart the OS (superx restart) to activate its startup (schema-dependent seeding)\n"
    ))
}

/// Print + flush (live tails must not sit in stdout buffers).
pub fn emit(text: &str) {
    print!("{text}");
    if let Err(e) = std::io::stdout().flush() {
        tracing::warn!(error = %e, "stdout flush failed");
    }
}
