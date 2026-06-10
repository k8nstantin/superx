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

    let rendered = match cli.command {
        Command::Kernel(KernelCommand::Bootstrap) => superx_cli::run_bootstrap(&kernel).await?,
        Command::Kernel(KernelCommand::Modules(ModulesCommand::List)) => {
            superx_cli::run_modules_list(&kernel).await?
        }
        Command::Kernel(KernelCommand::Stats { limit }) => {
            superx_cli::run_stats(&kernel, limit).await?
        }
    };
    print!("{rendered}");
    Ok(())
}
