//! `superx` — the operator CLI for the SuperX agentic OS.
//!
//! Thin dispatcher over the runners in `lib.rs`. The FVP surface
//! (BLUEPRINT.md G6): `boot`, `status`, `agents`, `actions [--live]`,
//! `sessions`, `read <session> [--live]`.

use std::process::ExitCode;
use std::time::Duration;

use superx::{Cli, Command, Config, Parser as _};

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    // Resolve the instance config FIRST (flag > env > params file >
    // fallback) — the log location itself is a parameter.
    let params = match superx::config::load_params(&cli.home) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("superx: {e}");
            return ExitCode::FAILURE;
        }
    };
    let config = superx::config::resolve(
        cli.home.clone(),
        &params,
        cli.conn.endpoint.clone(),
        cli.conn.namespace.clone(),
        cli.conn.database.clone(),
        cli.data_dir.clone(),
        cli.log_dir.clone(),
        cli.log_filter.clone(),
    );

    // The guard must live until the process exits — dropping it
    // flushes the self-log and stops the writer thread.
    let _log_guard =
        match superx_kernel::logging::init_with_filter(&config.log_dir, &config.log_filter) {
            Ok(guard) => guard,
            Err(e) => {
                eprintln!("superx: cannot initialize the kernel self-log: {e}");
                return ExitCode::FAILURE;
            }
        };

    match run(cli, config).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %e, "command failed");
            eprintln!("superx: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli, config: Config) -> Result<(), String> {
    if cli.initialize {
        return superx::initialize::initialize(&config).await;
    }
    let Some(command) = cli.command else {
        return Err("nothing to do — pass a command or --initialize (see --help)".to_string());
    };
    // Lifecycle + logs commands manage their own connections.
    if matches!(command, Command::Stop) {
        superx::emit(&superx::run_stop(&config.data_dir).await?);
        return Ok(());
    }
    if matches!(command, Command::Start) {
        return superx::run_start(&config).await;
    }
    if matches!(command, Command::Restart) {
        if superx::initialize::read_live_pid(&config.data_dir).is_some() {
            superx::emit(&superx::run_stop(&config.data_dir).await?);
        } else {
            superx::emit("OS was not running — starting it\n");
        }
        return superx::run_start(&config).await;
    }
    if let Command::Logs { lines, follow, daemon } = command {
        let (text, path, mut seen) = superx::run_logs(&config, lines, daemon)?;
        superx::emit(&text);
        if !follow {
            return Ok(());
        }
        loop {
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {} // skill-allow: §9-duration — log-follow cadence, pre-substrate viewer
                _ = tokio::signal::ctrl_c() => return Ok(()),
            }
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            let len = text.len() as u64;
            if len > seen {
                superx::emit(&text[seen as usize..]);
                seen = len;
            } else if len < seen {
                seen = 0; // rotated
            }
        }
    }
    let kernel = superx::connect(&config).await?;

    match command {
        Command::Boot { daemonized } => {
            if !daemonized {
                if let Some(pid) = superx::initialize::read_live_pid(&config.data_dir) {
                    return Err(format!(
                        "the OS is already running in background (pid {pid}) — \
                         `superx stop` first if you want a foreground boot"
                    ));
                }
            }
            superx::run_boot(&kernel).await
        }
        Command::Start | Command::Stop | Command::Restart | Command::Logs { .. } => {
            unreachable!("handled above")
        }
        Command::Status => {
            superx::emit(&superx::run_status(&kernel, &config.data_dir).await?);
            Ok(())
        }
        Command::Modules { action } => {
            let text = match action {
                superx::ModulesAction::List => superx::run_modules_list(&kernel).await?,
                superx::ModulesAction::Enable { name } => {
                    superx::run_modules_set(&kernel, &name, true).await?
                }
                superx::ModulesAction::Disable { name } => {
                    superx::run_modules_set(&kernel, &name, false).await?
                }
            };
            superx::emit(&text);
            Ok(())
        }
        Command::Agents => {
            superx::emit(&superx::run_agents(&kernel).await?);
            Ok(())
        }
        Command::Sessions { agent } => {
            superx::emit(&superx::run_sessions(&kernel, agent.as_deref()).await?);
            Ok(())
        }
        Command::Actions { limit, agent, live } => {
            let (text, high_water) =
                superx::run_actions(&kernel, limit, agent.as_deref()).await?;
            superx::emit(&text);
            if !live {
                return Ok(());
            }
            let agent_id = match agent.as_deref() {
                Some(name) => Some(
                    kernel
                        .find_entity_by_name("node_agent", "attr_agent_descriptor", name)
                        .await
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| format!("no agent named '{name}'"))?,
                ),
                None => None,
            };
            let poll = superx::live_poll_secs(&kernel).await;
            let mut after = high_water.unwrap_or_else(chrono::Utc::now);
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(poll)) => {}
                    _ = tokio::signal::ctrl_c() => return Ok(()),
                }
                let (text, high) =
                    superx::actions_tail_tick(&kernel, after, agent_id.as_ref(), limit.max(100))
                        .await?;
                superx::emit(&text);
                after = high;
            }
        }
        Command::Read { session, live } => {
            let session_id = superx::resolve_session(&kernel, &session).await?;
            let (text, high_water) = superx::run_read(&kernel, session_id.clone()).await?;
            superx::emit(&text);
            if !live {
                return Ok(());
            }
            let poll = superx::live_poll_secs(&kernel).await;
            let mut after = high_water.unwrap_or_else(chrono::Utc::now);
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(poll)) => {}
                    _ = tokio::signal::ctrl_c() => return Ok(()),
                }
                let (text, high) =
                    superx::read_tail_tick(&kernel, session_id.clone(), after).await?;
                superx::emit(&text);
                after = high;
            }
        }
    }
}
