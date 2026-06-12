//! `superx` — thin binary shell. Parses args, connects to the
//! substrate as the kernel service account, dispatches to the
//! library handlers, prints what they render.

use clap::Parser;
use superx_cli::{Cli, Command, KernelCommand, ModulesCommand};
use superx_kernel::Kernel;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli).await {
        eprintln!("superx: {e}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> superx_kernel::Result<()> {
    let kernel = Kernel::connect_service(
        &cli.connection.endpoint,
        &cli.connection.namespace,
        &cli.connection.database,
    )
    .await?;

    match cli.command {
        Command::Kernel(KernelCommand::Bootstrap) => {
            let (report, rendered) = superx_cli::run_bootstrap(&kernel).await?;
            print!("{rendered}");
            if superx_cli::capture_is_active(&report) {
                println!(
                    "\ncapture is running — telemetry is streaming into the substrate.\n\
                     watch it live from another terminal:  superx kernel stats --live\n\
                     press ctrl-c to stop."
                );
                tokio::signal::ctrl_c().await.ok();
            }
        }
        Command::Kernel(KernelCommand::Modules(ModulesCommand::List)) => {
            print!("{}", superx_cli::run_modules_list(&kernel).await?);
        }
        Command::Kernel(KernelCommand::Stats { limit, live, module, json }) => {
            let format = if json {
                superx_cli::RenderFormat::Json
            } else {
                superx_cli::RenderFormat::Human
            };
            if live {
                run_stats_live(&kernel, limit, module.as_deref(), format).await?;
            } else {
                print!(
                    "{}",
                    superx_cli::run_stats(&kernel, limit, module.as_deref(), format).await?
                );
            }
        }
        Command::Kernel(KernelCommand::Agents) => {
            print!("{}", superx_cli::run_agents(&kernel).await?);
        }
    }
    Ok(())
}

/// The interactive follow loop: backlog, then poll until ctrl-c.
/// Lives in the binary — library code stays print-free; per-tick
/// errors are reported and the loop continues (a transient substrate
/// hiccup must not kill the operator's live view).
async fn run_stats_live(
    kernel: &superx_kernel::Kernel,
    limit: u32,
    module: Option<&str>,
    format: superx_cli::RenderFormat,
) -> superx_kernel::Result<()> {
    // Fresh-substrate guard — without it a never-bootstrapped
    // substrate dies with a raw error after the follow header prints.
    if superx_cli::substrate_is_bare(kernel).await? {
        println!("substrate has no registry yet — run `superx kernel bootstrap` first");
        return Ok(());
    }

    let (backlog, high_water) = superx_cli::live_backlog(kernel, limit, module, format).await?;
    print!("{backlog}");
    println!("--- following (ctrl-c to stop) ---");

    let interval = superx_cli::live_poll_interval_secs(kernel).await?;
    let overlap =
        chrono::Duration::seconds(i64::try_from(interval).unwrap_or(1).saturating_mul(2));
    let mut tail = superx_cli::LiveTail::new(high_water, overlap, format);

    // One ctrl-c future for the whole loop — re-registering it per
    // select! iteration would swallow a SIGINT that lands mid-tick.
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);
    loop {
        tokio::select! {
            _ = &mut ctrl_c => return Ok(()),
            () = tokio::time::sleep(std::time::Duration::from_secs(interval)) => {}
        }
        match tail.tick(kernel, module).await {
            Ok(out) => print!("{out}"),
            Err(e) => eprintln!("stats: poll failed ({e}); retrying"),
        }
    }
}
