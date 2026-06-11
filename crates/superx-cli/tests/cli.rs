//! Tests for the `superx` CLI: argument parsing and command handlers
//! driven against an in-memory substrate (no wire connection).

use std::error::Error;

use clap::Parser;
use surrealdb::engine::any::connect;
use surrealdb::opt::auth::Database;

use superx_cli::{run_bootstrap, run_modules_list, run_stats, Cli, Command, KernelCommand};
use superx_kernel::{Kernel, SCHEMA_DDL};

const TEST_PASSWORD: &str = "test-kernel-password-for-mem-engine";

async fn fresh_kernel() -> Result<Kernel, Box<dyn Error>> {
    let db = connect("mem://").await?;
    db.use_ns("superx").use_db("kernel").await?;
    let ddl = SCHEMA_DDL.replace("$SUPERX_KERNEL_PASSWORD", TEST_PASSWORD);
    db.query(ddl).await?.check()?;
    db.signin(Database {
        namespace: "superx".to_string(),
        database: "kernel".to_string(),
        username: "superx_kernel".to_string(),
        password: TEST_PASSWORD.to_string(),
    })
    .await?;
    Ok(Kernel::from_db(db))
}

// ─────────────────────────────────────────────────────────────────────
// Argument parsing
// ─────────────────────────────────────────────────────────────────────

#[test]
fn parses_kernel_bootstrap() {
    let cli = Cli::try_parse_from(["superx", "kernel", "bootstrap"]).expect("must parse");
    assert!(matches!(cli.command, Command::Kernel(KernelCommand::Bootstrap)));
    assert_eq!(cli.connection.endpoint, "ws://127.0.0.1:8000");
    assert_eq!(cli.connection.namespace, "superx");
    assert_eq!(cli.connection.database, "kernel");
}

#[test]
fn parses_stats_with_limit() {
    let cli = Cli::try_parse_from(["superx", "kernel", "stats", "-n", "7"]).expect("must parse");
    match cli.command {
        Command::Kernel(KernelCommand::Stats { limit }) => assert_eq!(limit, 7),
        other => panic!("expected stats, got {other:?}"),
    }
}

#[test]
fn parses_endpoint_override() {
    let cli = Cli::try_parse_from([
        "superx",
        "--endpoint",
        "ws://10.0.0.5:9000",
        "kernel",
        "modules",
        "list",
    ])
    .expect("must parse");
    assert_eq!(cli.connection.endpoint, "ws://10.0.0.5:9000");
}

#[test]
fn rejects_unknown_subcommand() {
    assert!(Cli::try_parse_from(["superx", "kernel", "explode"]).is_err());
}

// ─────────────────────────────────────────────────────────────────────
// Handlers against an in-memory substrate
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn modules_list_hints_before_bootstrap() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;
    let out = run_modules_list(&kernel).await?;
    assert!(out.contains("run `superx kernel bootstrap` first"), "got: {out}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bootstrap_then_modules_list_shows_inventory() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;

    let (report, boot_out) = run_bootstrap(&kernel).await?;
    // The FVP inventory in this binary: orchestrator, CLI app,
    // discovery, capture, and the claude-code driver.
    for name in ["bootstrap", "app_cli", "discovery", "capture", "driver_claude_code"] {
        assert!(boot_out.contains(name), "boot report missing {name}: {boot_out}");
    }
    assert!(boot_out.contains("active"), "boot report: {boot_out}");
    assert!(
        superx_cli::capture_is_active(&report),
        "capture must boot Active for the FVP foreground hold",
    );

    let list_out = run_modules_list(&kernel).await?;
    assert!(list_out.contains("bootstrap"), "list: {list_out}");
    assert!(list_out.contains("app_cli"), "list: {list_out}");
    assert!(list_out.contains("app"), "category shown: {list_out}");
    assert!(list_out.contains("5 registered"), "count line: {list_out}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stats_renders_boot_telemetry() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;
    run_bootstrap(&kernel).await?;

    let out = run_stats(&kernel, 50).await?;
    // Boot leaves module_starting / module_active events behind.
    assert!(out.contains("module_starting"), "stats: {out}");
    assert!(out.contains("module_active"), "stats: {out}");
    assert!(out.contains("event(s)"), "count line: {out}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stats_respects_limit() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;
    run_bootstrap(&kernel).await?;

    let out = run_stats(&kernel, 1).await?;
    assert!(out.contains("1 event(s)"), "stats: {out}");
    Ok(())
}
