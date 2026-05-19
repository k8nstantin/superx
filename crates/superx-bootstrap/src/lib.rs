//! # superx-bootstrap — first-run provisioning + agent discovery
//!
//! Implements the **auto-onboarding pillar** (`ARCHITECTURE.md` §0d MVP
//! capability C1). The `run` method is what the operator-facing
//! `superx-cli bootstrap --tenant <t>` invokes; it provisions the substrate
//! entity, configures default safety bounds, seeds canonical admin agents and
//! tools, probes the local environment for MCP clients (Claude Desktop config
//! and logs), and emits a `bootstrap_census` summary so consumers can prove
//! the scan ran.
//!
//! ## Entry point
//!
//! [`BootstrapBlade::run`] — idempotent. The substrate entity id is a
//! `UUIDv5(DNS-NS, tenant_id)` so re-running for the same tenant is a no-op.
//!
//! ## Telemetry contract
//!
//! Bootstrap emits at minimum these typed events:
//!
//! - `agent_seeded` × N — one per canonical admin agent (today: 2)
//! - `agent_discovered` × M — one per `mcpServers` entry found across
//!   probed Claude Desktop configs (cross-platform: macOS, Windows, Linux)
//! - `agent_activity_observed` × K — one per `mcp-server-*.log` found in
//!   the Claude Desktop logs directory (live-agent signal)
//! - `bootstrap_census` — final summary with counts
//! - `system_bootstrap` — overall lifecycle event
//!
//! ## Design notes
//!
//! - **Verification step at the end.** Bootstrap counts the telemetry rows
//!   it just produced and fails loudly if the firehose did not capture them.
//!   This is the early-warning that the substrate's telemetry pipe broke.
//! - **Env-var overrides for testing.** `SUPERX_CLAUDE_CONFIG` and
//!   `SUPERX_CLAUDE_LOGS` redirect the probes to fixture paths so
//!   integration tests can be hermetic on developer machines.
//! - **Cross-platform probes** — macOS `~/Library/Application Support/Claude/`,
//!   Windows `%APPDATA%\Claude\`, Linux `~/.config/Claude/` (best-effort;
//!   Anthropic does not officially document the Linux path).
//!
//! Copyright (c) 2026 Constantin Alexander <constantin@dedomena.io>.
//! Licensed under the Apache License, Version 2.0.

#![deny(warnings)]
#![deny(clippy::pedantic)]

use superx_kernel::{Kernel, KernelError};
use serde_json::json;
use std::path::{Path, PathBuf};
use surrealdb::sql::Thing;
use uuid::Uuid;

/// `BootstrapBlade`: Orchestrates the verifiable first-run sequence.
pub struct BootstrapBlade<'a> {
    pub kernel: &'a Kernel,
}

#[derive(serde::Deserialize)] 
struct CountRes { 
    count: u64 
}

impl<'a> BootstrapBlade<'a> {
    /// Creates a new `BootstrapBlade`.
    #[must_use]
    pub fn new(kernel: &'a Kernel) -> Self {
        Self { kernel }
    }

    /// `run`: Executes the substrate provisioning and verification.
    ///
    /// # Panics
    /// Panics if the default DNS namespace UUID is invalid or verification fails.
    ///
    /// # Errors
    /// Returns `KernelError` if bootstrapping or verification fails.
    pub async fn run(&self, tenant_id: &str) -> Result<String, KernelError> {
        let run_id = Uuid::now_v7().to_string();
        tracing::info!("Starting NASA-Grade Bootstrap (Run ID: {run_id}) for Tenant: {tenant_id}");

        // 1. Establish Physical Session (Physical Isolation)
        self.kernel.set_session_auth(tenant_id, "admin").await?;

        // 2. Provision Substrate Entity (Deterministic UUIDv5 from Tenant)
        let ns_uuid = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").expect("Valid DNS NS");
        let substrate_uuid = Uuid::new_v5(&ns_uuid, tenant_id.as_bytes()).to_string();
        let substrate_id = format!("entity:{substrate_uuid}");
        let substrate_thing = Thing::from(("entity".to_string(), substrate_uuid));

        // The substrate is a `node_substrate`, not a `node_agent` — it does not
        // carry a role. The PERMISSIONS clauses gate reads/writes via
        // `$session_role`, not via this entity's own role field.
        self.kernel.db.query("UPSERT $id SET tenant_id = $t, type = type_definition:node_substrate")
            .bind(("id", substrate_thing))
            .bind(("t", tenant_id.to_string())).await?.check()?;


        // 3. Configure Substrate Parameters (Fine-grained state)
        self.kernel.supersede_state(
            &substrate_id,
            "attr_config",
            json!({
                "max_dfs_iterations": 10_000,
                "max_traversal_depth": 10,
                "max_ingestion_entries": 10_000,
                "emission_enabled": true,
                "data_scope": "all"
            }),
            Some(run_id.clone())
        ).await?;

        // 4. Agent Discovery Phase
        self.discover_local_agents(tenant_id, &run_id, &substrate_id).await?;

        // 5. Log Bootstrap Event (Telemetry)
        self.kernel.log_telemetry(
            json!({"substrate_id": substrate_id, "status": "provisioned"}),
            "system_bootstrap",
            Some(run_id.clone())
        ).await?;

        // 6. Verify the telemetry firehose actually captured our writes — this
        //    is the early-warning that the substrate's `telemetry_stream` pipe
        //    broke. If we just provisioned a tenant but no events landed, the
        //    operator needs to know immediately, not at first usage.
        let verify_query = "SELECT count() FROM telemetry_stream WHERE run_id = $rid GROUP ALL";
        let mut verify_res = self.kernel.db.query(verify_query)
            .bind(("rid", run_id.clone())).await?;

        let counts: Vec<CountRes> = verify_res.take(0)?;
        if counts.is_empty() || counts[0].count == 0 {
            let mut all_res = self.kernel.db.query("SELECT count() FROM telemetry_stream GROUP ALL").await?;
            let all_counts: Vec<CountRes> = all_res.take(0)?;
            let total = all_counts.first().map_or(0, |c| c.count);
            return Err(KernelError::SafetyViolation(format!("Telemetry failed to capture bootstrap event. Total events in table: {total}")));
        }

        tracing::info!("Bootstrap healthy (run_id={run_id}, substrate_id={substrate_id})");
        Ok(substrate_id)
    }

    /// `discover_local_agents`: probes the local environment and emits fine-grained
    /// telemetry. Four event types are produced per call:
    /// `agent_seeded`, `agent_discovered`, `agent_activity_observed`, `bootstrap_census`.
    ///
    /// # Panics
    /// Panics if the DNS namespace UUID constant is invalid (compile-time fact).
    async fn discover_local_agents(&self, tenant_id: &str, run_id: &str, substrate_id: &str) -> Result<(), KernelError> {
        assert!(!tenant_id.is_empty(), "Tenant ID mandatory for discovery");
        assert!(!run_id.is_empty(), "Run ID mandatory for discovery");
        assert!(!substrate_id.is_empty(), "Substrate ID mandatory for discovery");
        tracing::info!("Probing local environment for agents...");

        let sys_ns = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").expect("Valid DNS NS UUID");

        let admin_agents = self.seed_admin_agents(tenant_id, run_id, substrate_id, &sys_ns).await?;
        self.seed_standard_tools(tenant_id, &admin_agents).await?;
        let discovered_count = self.probe_claude_desktop_configs(tenant_id, run_id, substrate_id, &sys_ns).await?;
        let activity_count = self.probe_claude_desktop_logs(run_id).await?;

        self.kernel.log_telemetry(
            json!({
                "agents_seeded": admin_agents.len() as u64,
                "agents_discovered": discovered_count,
                "agents_with_activity": activity_count
            }),
            "bootstrap_census",
            Some(run_id.to_string()),
        ).await?;

        Ok(())
    }

    /// Seed mandatory admin agents (`system_controller` + `gemini_cli`) and emit
    /// one `agent_seeded` telemetry row each.
    async fn seed_admin_agents(
        &self,
        tenant_id: &str,
        run_id: &str,
        substrate_id: &str,
        sys_ns: &Uuid,
    ) -> Result<Vec<(String, String)>, KernelError> {
        let mut admin_agents: Vec<(String, String)> = Vec::new();
        for name in ["system_controller", "gemini_cli"] {
            let uid = Uuid::new_v5(sys_ns, format!("{tenant_id}:{name}").as_bytes()).to_string();
            let id_literal = format!("entity:{uid}");
            self.kernel.db.query("UPSERT $id SET tenant_id = $t, role = $r, type = type_definition:node_agent")
                .bind(("id", Thing::from(("entity".to_string(), uid.clone()))))
                .bind(("t", tenant_id.to_string()))
                .bind(("r", "admin".to_string())).await?.check()?;
            self.kernel.create_structural_edge(substrate_id, &id_literal, "edge_owns").await?;
            let desc = if name == "system_controller" {
                "SuperX Core System Controller"
            } else {
                "Gemini CLI Operator Agent"
            };
            self.kernel.supersede_state(&id_literal, "attr_desc", json!({"text": desc}), Some(run_id.to_string())).await?;
            self.kernel.log_telemetry(
                json!({"agent_id": id_literal, "name": name, "role": "admin", "source": "seeded"}),
                "agent_seeded",
                Some(run_id.to_string()),
            ).await?;
            admin_agents.push((name.to_string(), id_literal));
        }
        Ok(admin_agents)
    }

    /// Seed the five canonical tools and grant each admin agent capability to all of them.
    async fn seed_standard_tools(
        &self,
        tenant_id: &str,
        admin_agents: &[(String, String)],
    ) -> Result<(), KernelError> {
        let tools = ["tool_ingest", "tool_compile", "tool_propose", "tool_evaluate", "tool_promote"];
        for tool in tools {
            let tool_id = format!("entity:{tool}");
            let tool_thing = Thing::from(("entity".to_string(), tool.to_string()));
            self.kernel.db.query("UPSERT $id SET tenant_id = $t, role = 'user', type = type_definition:node_tool")
                .bind(("id", tool_thing))
                .bind(("t", tenant_id.to_string())).await?.check()?;
            for (_, agent_id) in admin_agents {
                self.kernel.create_structural_edge(agent_id, &tool_id, "edge_has_capability").await?;
            }
        }
        Ok(())
    }

    /// Read every probable Claude Desktop config path for the current platform and emit one
    /// `agent_discovered` event per declared `mcpServers` entry. Returns the count.
    async fn probe_claude_desktop_configs(
        &self,
        tenant_id: &str,
        run_id: &str,
        substrate_id: &str,
        sys_ns: &Uuid,
    ) -> Result<u64, KernelError> {
        let mut discovered_count: u64 = 0;
        for cfg_path in claude_desktop_config_candidates() {
            if !cfg_path.exists() {
                continue;
            }
            tracing::info!("Probing Claude config at {}", cfg_path.display());
            let Some(servers) = read_mcp_servers(&cfg_path) else { continue };
            for (name, server_cfg) in servers {
                let agent_uuid = Uuid::new_v5(sys_ns, format!("{tenant_id}:{name}").as_bytes()).to_string();
                let agent_record_id = format!("entity:{agent_uuid}");

                self.kernel.db.query("UPSERT $id SET tenant_id = $t, role = $r, type = type_definition:node_agent")
                    .bind(("id", Thing::from(("entity".to_string(), agent_uuid.clone()))))
                    .bind(("t", tenant_id.to_string()))
                    .bind(("r", "user".to_string())).await?.check()?;
                self.kernel.create_structural_edge(substrate_id, &agent_record_id, "edge_owns").await?;

                let command = server_cfg.get("command").and_then(|v| v.as_str()).unwrap_or("");
                let arg_count = server_cfg.get("args").and_then(|v| v.as_array()).map_or(0, Vec::len);

                self.kernel.supersede_state(
                    &agent_record_id,
                    "attr_desc",
                    json!({"text": format!("Claude Desktop MCP agent: {name} ({command})")}),
                    Some(run_id.to_string()),
                ).await?;
                self.kernel.log_telemetry(
                    json!({
                        "agent_id": agent_record_id,
                        "name": name,
                        "role": "user",
                        "source": "claude_desktop",
                        "config_path": cfg_path.display().to_string(),
                        "command": command,
                        "arg_count": arg_count
                    }),
                    "agent_discovered",
                    Some(run_id.to_string()),
                ).await?;
                discovered_count += 1;
            }
        }
        Ok(discovered_count)
    }

    /// Probe the Claude Desktop logs directory and emit one `agent_activity_observed`
    /// event per matching `mcp-server-*.log` file.
    async fn probe_claude_desktop_logs(&self, run_id: &str) -> Result<u64, KernelError> {
        let mut activity_count: u64 = 0;
        let Some(log_dir) = claude_desktop_log_dir() else { return Ok(0) };
        let Ok(entries) = std::fs::read_dir(&log_dir) else { return Ok(0) };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            let Some(name) = file_name
                .strip_prefix("mcp-server-")
                .and_then(|rest| rest.strip_suffix(".log"))
            else {
                continue;
            };

            let mtime_iso = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or_else(String::new, |d| format!("{d:?}"));

            self.kernel.log_telemetry(
                json!({
                    "name": name,
                    "log_path": path.display().to_string(),
                    "mtime_since_epoch": mtime_iso
                }),
                "agent_activity_observed",
                Some(run_id.to_string()),
            ).await?;
            activity_count += 1;
        }
        Ok(activity_count)
    }
}

/// Read an `mcpServers` block from a Claude Desktop config file, returning a clone
/// of the map so the caller doesn't have to hold a reference into the parsed JSON.
fn read_mcp_servers(cfg_path: &Path) -> Option<serde_json::Map<String, serde_json::Value>> {
    let raw = std::fs::read_to_string(cfg_path).ok()?;
    let cfg: serde_json::Value = serde_json::from_str(&raw).ok()?;
    cfg.get("mcpServers").and_then(|v| v.as_object()).cloned()
}

/// Possible Claude Desktop config locations per platform. Order is "most
/// authoritative" first. `SUPERX_CLAUDE_CONFIG` env override (when set) is
/// returned first so integration tests can inject a fixture deterministically.
fn claude_desktop_config_candidates() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();

    if let Ok(override_path) = std::env::var("SUPERX_CLAUDE_CONFIG") {
        if !override_path.is_empty() {
            out.push(PathBuf::from(override_path));
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            out.push(Path::new(&home).join("Library/Application Support/Claude/claude_desktop_config.json"));
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            out.push(Path::new(&appdata).join("Claude").join("claude_desktop_config.json"));
        }
    }
    #[cfg(target_os = "linux")]
    {
        // Linux is not officially documented by Anthropic; probe the conventional XDG path
        // as a best-effort fallback so Linux operators get the same discovery story.
        if let Ok(home) = std::env::var("HOME") {
            out.push(Path::new(&home).join(".config/Claude/claude_desktop_config.json"));
        }
    }

    out
}

/// Directory where Claude Desktop writes per-agent MCP logs, by platform.
fn claude_desktop_log_dir() -> Option<PathBuf> {
    if let Ok(override_path) = std::env::var("SUPERX_CLAUDE_LOGS") {
        if !override_path.is_empty() {
            return Some(PathBuf::from(override_path));
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            return Some(Path::new(&home).join("Library/Logs/Claude"));
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return Some(Path::new(&appdata).join("Claude").join("logs"));
        }
    }
    None
}
