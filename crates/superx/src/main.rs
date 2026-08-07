//! `superx` — the operator CLI for the SuperX agentic OS.
//!
//! The command surface (`boot`, `status`, `agents`, `stats --live`)
//! lands in G6 per `BLUEPRINT.md`. Until then this binary initializes
//! the kernel self-log and identifies itself.

use std::process::ExitCode;

fn main() -> ExitCode {
    // The guard must live until the process exits — dropping it
    // flushes the self-log and stops the writer thread.
    let _log_guard = match superx_kernel::logging::init_default() {
        Ok(guard) => guard,
        Err(e) => {
            eprintln!("superx: cannot initialize the kernel self-log: {e}");
            return ExitCode::FAILURE;
        }
    };

    tracing::info!(version = env!("CARGO_PKG_VERSION"), "superx starting");
    println!("superx {} — agentic-OS reset in progress (G2); see BLUEPRINT.md", env!("CARGO_PKG_VERSION"));
    tracing::info!("superx exiting");
    ExitCode::SUCCESS
}
