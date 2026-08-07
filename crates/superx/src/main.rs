//! `superx` — the operator CLI for the SuperX agentic OS.
//!
//! Thin dispatcher over the runners in `lib.rs`. The FVP surface
//! (BLUEPRINT.md G6): `boot`, `status`, `agents`, `actions [--live]`,
//! `sessions`, `read <session> [--live]`.

use std::process::ExitCode;
use std::time::Duration;

use superx::{Cli, Command, Parser as _};

#[tokio::main]
async fn main() -> ExitCode {
    // The guard must live until the process exits — dropping it
    // flushes the self-log and stops the writer thread.
    let _log_guard = match superx_kernel::logging::init_default() {
        Ok(guard) => guard,
        Err(e) => {
            eprintln!("superx: cannot initialize the kernel self-log: {e}");
            return ExitCode::FAILURE;
        }
    };

    let cli = Cli::parse();
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %e, "command failed");
            eprintln!("superx: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<(), String> {
    let kernel = superx::connect(&cli.conn).await?;

    match cli.command {
        Command::Boot => superx::run_boot(&kernel).await,
        Command::Status => {
            superx::emit(&superx::run_status(&kernel).await?);
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
