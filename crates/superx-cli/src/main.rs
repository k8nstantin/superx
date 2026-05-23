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

    /// Print recent `telemetry_stream` rows, newest first. Read-only.
    Stats {
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

        /// Maximum number of telemetry rows to show.
        #[arg(long, default_value_t = 25)]
        limit: u32,
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
        Command::Stats {
            endpoint,
            namespace,
            database,
            limit,
        } => stats(&endpoint, &namespace, &database, limit).await,
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

async fn stats(
    endpoint: &str,
    namespace: &str,
    database: &str,
    limit: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let kernel = Kernel::connect_service(endpoint, namespace, database).await?;
    let events = kernel.recent_telemetry(limit).await?;

    if events.is_empty() {
        println!("(no telemetry rows yet — run `superx bootstrap` to seed the metamodel)");
        return Ok(());
    }

    for event in &events {
        let run_field = match &event.run {
            Some(r) => format!("run={r:?}"),
            None => String::new(),
        };
        println!(
            "{ts}  {event:<22}  {run_field}  {payload:?}",
            ts = event.valid_from.to_rfc3339(),
            event = event.lifecycle_event,
            run_field = run_field,
            payload = event.payload,
        );
    }

    println!();
    println!("({n} row{s}, newest first)", n = events.len(), s = if events.len() == 1 { "" } else { "s" });
    Ok(())
}
