//! Tests for the `superx` CLI: argument parsing and command handlers
//! driven against an in-memory substrate (no wire connection).

use std::error::Error;

use clap::Parser;
use surrealdb::engine::any::connect;
use surrealdb::opt::auth::Database;

use superx_cli::{run_bootstrap, run_modules_list, run_stats, Cli, Command, KernelCommand, RenderFormat};
use superx_driver_claude_code::{ClaudeCodeDriver, DRIVER_NAME, PROJECTS_ROOT_PARAM};
use superx_kernel::types::Value;
use superx_kernel::{Kernel, KernelModule, NodeKind, SCHEMA_DDL};

const TEST_PASSWORD: &str = "test-kernel-password-for-mem-engine";

/// A completely bare substrate — schema applied, nothing seeded.
/// Used by the pre-bootstrap hint tests.
async fn bare_kernel() -> Result<Kernel, Box<dyn Error>> {
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

/// Fresh substrate with the claude-code driver's projects root
/// pre-pointed at a nonexistent path, so bootstrap's discovery pass
/// finds nothing on the host machine and the spawned capture loop
/// stays quiet — keeps every assertion deterministic instead of
/// racing the developer's live `~/.claude` transcripts.
async fn fresh_kernel() -> Result<Kernel, Box<dyn Error>> {
    let kernel = bare_kernel().await?;
    for t in superx_kernel::REQUIRED_METAMODEL_TYPES {
        kernel.ensure_type_definition(t.uid, t.category, t.memory_tier).await?;
    }
    let entity_id = kernel.register_module(&ClaudeCodeDriver.descriptor()).await?;
    kernel
        .set_parameter(
            entity_id,
            PROJECTS_ROOT_PARAM,
            Value::String("/nonexistent/superx-cli-test-projects-root".to_string()),
        )
        .await?;
    // DRIVER_NAME / NodeKind document which driver this fixture
    // neutralizes; referenced so the imports stay honest.
    let _ = DRIVER_NAME;
    let _ = NodeKind::Contribution;
    Ok(kernel)
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
        Command::Kernel(KernelCommand::Stats { limit, live, module, json }) => {
            assert_eq!(limit, 7);
            assert!(!live);
            assert!(module.is_none());
            assert!(!json, "human rendering is the default");
        }
        other => panic!("expected stats, got {other:?}"),
    }
}

#[test]
fn parses_stats_live_with_module_filter() {
    let cli = Cli::try_parse_from([
        "superx", "kernel", "stats", "--live", "--module", "capture",
    ])
    .expect("must parse");
    match cli.command {
        Command::Kernel(KernelCommand::Stats { live, module, .. }) => {
            assert!(live);
            assert_eq!(module.as_deref(), Some("capture"));
        }
        other => panic!("expected stats, got {other:?}"),
    }
}

#[test]
fn parses_agents() {
    let cli = Cli::try_parse_from(["superx", "kernel", "agents"]).expect("must parse");
    assert!(matches!(cli.command, Command::Kernel(KernelCommand::Agents)));
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
    let kernel = bare_kernel().await?;
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

    let out = run_stats(&kernel, 50, None, RenderFormat::Human).await?;
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

    let out = run_stats(&kernel, 1, None, RenderFormat::Human).await?;
    assert!(out.contains("1 event(s)"), "stats: {out}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stats_module_filter_narrows_output() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;
    run_bootstrap(&kernel).await?;

    let all = run_stats(&kernel, 100, None, RenderFormat::Human).await?;
    let filtered = run_stats(&kernel, 100, Some("driver_claude_code"), RenderFormat::Human).await?;
    assert!(filtered.len() < all.len(), "filter must narrow output");
    assert!(
        filtered.contains("driver_claude_code"),
        "filtered: {filtered}",
    );
    assert!(
        !filtered.contains("system_boot"),
        "unrelated events filtered out: {filtered}",
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_tail_renders_each_event_exactly_once() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;
    run_bootstrap(&kernel).await?;

    // Generous overlap so every tick re-reads the whole window — the
    // dedup must still render each event exactly once.
    let mut tail = superx_cli::LiveTail::new(
        chrono::DateTime::<chrono::Utc>::MIN_UTC,
        chrono::Duration::seconds(3600),
        RenderFormat::Human,
    );

    // First tick from the epoch sees the boot backlog.
    let out = tail.tick(&kernel, None).await?;
    assert!(out.contains("module_active"), "backlog: {out}");
    let high_water = tail.high_water();
    assert!(high_water > chrono::DateTime::<chrono::Utc>::MIN_UTC);

    // Nothing new → empty render despite the overlap re-read
    // (dedup proof), watermark stable.
    let out = tail.tick(&kernel, None).await?;
    assert!(out.is_empty(), "no new events expected, got: {out}");
    assert_eq!(tail.high_water(), high_water);

    // New activity lands → exactly it is rendered, watermark advances.
    let mut payload = superx_kernel::types::Object::new();
    payload.insert(
        "note".to_string(),
        superx_kernel::types::Value::String("fresh".to_string()),
    );
    kernel
        .log_telemetry(
            "live_test_event",
            superx_kernel::types::Value::Object(payload),
            None,
        )
        .await?;
    let out = tail.tick(&kernel, None).await?;
    assert!(out.contains("live_test_event"), "tick: {out}");
    assert!(!out.contains("module_active"), "old events must not repeat: {out}");
    assert!(tail.high_water() > high_water);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agents_lists_discovered_agents_with_sources() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;
    run_bootstrap(&kernel).await?;

    // Deterministic agent + source, independent of what discovery
    // found on this machine.
    let agent = kernel.create_entity("node_agent", "user").await?;
    let mut desc = superx_kernel::types::Object::new();
    desc.insert(
        "name".to_string(),
        superx_kernel::types::Value::String("test_agent_x".to_string()),
    );
    desc.insert(
        "probe".to_string(),
        superx_kernel::types::Value::String("test_probe".to_string()),
    );
    kernel
        .supersede_state(
            agent,
            "attr_agent_descriptor",
            superx_kernel::types::Value::Object(desc),
        )
        .await?;
    let source = kernel.create_entity("node_source", "user").await?;
    let mut sdesc = superx_kernel::types::Object::new();
    for (k, v) in [
        ("name", "/tmp/x"),
        ("locator", "/tmp/x"),
        ("agent", "test_agent_x"),
        ("probe", "test_probe"),
    ] {
        sdesc.insert(
            k.to_string(),
            superx_kernel::types::Value::String(v.to_string()),
        );
    }
    kernel
        .supersede_state(
            source,
            "attr_source_descriptor",
            superx_kernel::types::Value::Object(sdesc),
        )
        .await?;

    let out = superx_cli::run_agents(&kernel).await?;
    assert!(out.contains("test_agent_x"), "agents: {out}");
    assert!(out.contains("1 source(s)"), "agents: {out}");
    assert!(out.contains("/tmp/x"), "agents: {out}");
    assert!(out.contains("agent(s)"), "count line: {out}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agents_hints_before_bootstrap() -> Result<(), Box<dyn Error>> {
    let kernel = bare_kernel().await?;
    let out = superx_cli::run_agents(&kernel).await?;
    assert!(out.contains("run `superx kernel bootstrap` first"), "got: {out}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn human_rendering_is_flat_key_value() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;
    run_bootstrap(&kernel).await?;

    let out = run_stats(&kernel, 50, None, RenderFormat::Human).await?;
    assert!(
        !out.contains("{\"Object\"") && !out.contains("{\"String\""),
        "no serde-tagged noise in human output: {out}",
    );
    // Lifecycle events carry name=… in their payloads.
    assert!(out.contains("name=") || out.contains("service_account="), "k=v pairs: {out}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn json_rendering_is_plain_and_parseable() -> Result<(), Box<dyn Error>> {
    let kernel = fresh_kernel().await?;
    run_bootstrap(&kernel).await?;

    let out = run_stats(&kernel, 10, None, RenderFormat::Json).await?;
    let mut parsed = 0;
    for line in out.lines() {
        if !line.starts_with('{') {
            continue; // trailing count line
        }
        let v: serde_json::Value = serde_json::from_str(line)?;
        assert!(v.get("ts").is_some() && v.get("event").is_some() && v.get("payload").is_some());
        assert!(
            v["payload"].get("Object").is_none(),
            "payload must be plain, not tagged: {line}",
        );
        parsed += 1;
    }
    assert!(parsed > 0, "at least one JSON event line: {out}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_refusal_is_detected_and_hint_is_actionable() -> Result<(), Box<dyn Error>> {
    // A real authentication refusal from a substrate whose deployed
    // password differs from what the connector sends.
    let db = connect("mem://").await?;
    db.use_ns("superx").use_db("kernel").await?;
    let ddl = SCHEMA_DDL.replace("$SUPERX_KERNEL_PASSWORD", "the-deployed-password");
    db.query(ddl).await?.check()?;
    let err = db
        .signin(Database {
            namespace: "superx".to_string(),
            database: "kernel".to_string(),
            username: "superx_kernel".to_string(),
            password: "a-different-password".to_string(),
        })
        .await
        .expect_err("mismatched password must be refused");
    let kernel_err = superx_kernel::KernelError::Db(err);
    assert!(superx_cli::is_auth_error(&kernel_err), "got: {kernel_err}");

    // Non-auth errors are not misclassified.
    let not_auth = superx_kernel::KernelError::NotFound("x".to_string());
    assert!(!superx_cli::is_auth_error(&not_auth));

    // The hint names the env var, the deploy step, and the endpoint.
    let hint = superx_cli::auth_failure_hint("ws://127.0.0.1:8000");
    for needle in ["SUPERX_KERNEL_PASSWORD", "deploy-schema.sh", "ws://127.0.0.1:8000", "EVERY terminal"] {
        assert!(hint.contains(needle), "hint missing '{needle}': {hint}");
    }
    Ok(())
}
