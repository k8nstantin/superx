//! `superx` — operator CLI for the SuperX agentic OS.
//!
//! The CLI is a thin shell over the [`superx_kernel`] verb surface. It
//! does not contain any database logic of its own; every action goes
//! through the typed kernel API (which signs in as the `superx` EDITOR
//! service account, never root, per SKILL.md §13).

use clap::{Parser, Subcommand};
use superx_kernel::Kernel;

#[derive(Debug, Parser)]
#[command(
    name = "superx",
    version,
    about = "Operator CLI for SuperX — connect, seed, run the OS",
    long_about = None,
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Connect to an operator-provisioned SurrealDB substrate (where
    /// `scripts/deploy-schema.sh` has applied the locked schema) and
    /// seed the v0.1 metamodel. Idempotent — safe to re-run.
    ///
    /// The three connection flags' default values mirror the
    /// `scripts/deploy-schema.sh` env-var fallbacks one-to-one
    /// (documented in the deploy script and the README) so
    /// `./scripts/deploy-schema.sh && superx bootstrap` works
    /// out-of-the-box for the v0.1 single-machine deployment. They
    /// move to `attr_config` once that table has values — see
    /// SKILL.md §9 ("everything is a parameter") and roadmap.
    Bootstrap {
        /// SurrealDB endpoint URL. `ws://host:port`, `wss://...`,
        /// `http://...`, or `https://...`.
        #[arg(long, env = "SUPERX_DB_ENDPOINT", default_value = "ws://localhost:8000")] // skill-allow: §9-default
        endpoint: String,

        /// SurrealDB namespace.
        #[arg(long, env = "SUPERX_NS", default_value = "superx")] // skill-allow: §9-default
        namespace: String,

        /// SurrealDB database.
        #[arg(long, env = "SUPERX_DB", default_value = "v01")] // skill-allow: §9-default
        database: String,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    match cli.command {
        Command::Bootstrap {
            endpoint,
            namespace,
            database,
        } => bootstrap(&endpoint, &namespace, &database).await,
    }
}

async fn bootstrap(
    endpoint: &str,
    namespace: &str,
    database: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("→ Connecting to {endpoint} (ns={namespace} db={database}) as service account 'superx'");

    let kernel = Kernel::connect_service(endpoint, namespace, database).await?;
    println!("  signed in.");

    kernel.seed_metamodel().await?;
    println!("  metamodel seeded (node_run, node_agent, node_source).");

    println!("✓ Bootstrap complete. Substrate is ready for kernel verbs.");
    Ok(())
}
