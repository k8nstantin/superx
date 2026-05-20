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
//!   probed MCP-client configs (Claude Desktop, Claude Code, Gemini CLI;
//!   cross-platform: macOS, Windows, Linux). The event carries a `source`
//!   field naming which client declared the agent (`claude_desktop` /
//!   `claude_code` / `gemini_cli`) so downstream consumers can fan out per
//!   client.
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
//! - **Env-var overrides for testing.** `SUPERX_CLAUDE_CONFIG` (Claude Desktop),
//!   `SUPERX_CLAUDE_CODE_CONFIG` (Claude Code's `~/.claude.json`),
//!   `SUPERX_GEMINI_CONFIG` (Gemini CLI's `~/.gemini/settings.json`), and
//!   `SUPERX_CLAUDE_LOGS` redirect the probes to fixture paths so
//!   integration tests can be hermetic on developer machines.
//! - **Probe set covers every coding-agent surface we know about today:**
//!   Claude Desktop (`~/Library/Application Support/Claude/claude_desktop_config.json`
//!   on macOS, `%APPDATA%\Claude\claude_desktop_config.json` on Windows,
//!   `~/.config/Claude/claude_desktop_config.json` on Linux), Claude Code
//!   (`~/.claude.json`, cross-platform), Gemini CLI
//!   (`~/.gemini/settings.json`, cross-platform). All three use an identical
//!   `mcpServers` JSON schema, so the parser is one function. Adding a
//!   future client = one extra entry in [`mcp_client_config_candidates`].
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
        let substrate_uuid = Uuid::new_v5(&ns_uuid, tenant_id.as_bytes());
        let substrate_thing = Thing::from((
            "entity",
            surrealdb::sql::Id::Uuid(surrealdb::sql::Uuid::from(substrate_uuid)),
        ));
        let substrate_id = format!("entity:{substrate_uuid}");

        // Under v2: typed FK type=$node_substrate, tenant points at the
        // substrate itself (self-reference is how the substrate entity
        // satisfies its own `tenant.type.uid = 'node_substrate'` ASSERT).
        // Idempotent SELECT-then-CREATE — UPSERT is forbidden under the
        // superx service account (PERMISSIONS FOR update NONE).
        let node_substrate = self.kernel.type_thing("node_substrate")?;
        let mut sel = self.kernel.db
            .query("SELECT id FROM entity WHERE id = $id LIMIT 1")
            .bind(("id", substrate_thing.clone()))
            .await?;
        #[derive(serde::Deserialize)]
        struct IdRow { #[allow(dead_code)] id: Thing }
        let exists = sel.take::<Vec<IdRow>>(0)?.pop().is_some();
        if !exists {
            self.kernel.db.query(
                "CREATE entity CONTENT { \
                    id: $id, \
                    type: $type, \
                    tenant: $tenant_self, \
                    role: 'admin' \
                }"
            )
                .bind(("id", substrate_thing.clone()))
                .bind(("type", node_substrate))
                .bind(("tenant_self", substrate_thing.clone()))
                .await?.check()?;
        }


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
            None
        ).await?;

        // 4. Agent Discovery Phase
        self.discover_local_agents(tenant_id, &run_id, &substrate_id).await?;

        // 5. Log Bootstrap Event (Telemetry)
        self.kernel.log_telemetry(
            json!({"substrate_id": substrate_id, "status": "provisioned"}),
            "system_bootstrap",
            None
        ).await?;

        // 6. Verify the telemetry firehose actually captured our writes —
        //    early-warning that the substrate's `telemetry_stream` pipe
        //    broke. Under v2 the typed `run` FK isn't populated yet
        //    (node_run entities aren't modelled), so we filter on the
        //    `run_id` field that the caller embedded in the payload JSON.
        let verify_query = "SELECT count() FROM telemetry_stream \
            WHERE payload.run_id = $rid GROUP ALL";
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
        self.seed_standard_tools(tenant_id, substrate_id, &admin_agents).await?;
        let discovered_count = self.probe_mcp_client_configs(tenant_id, run_id, substrate_id, &sys_ns).await?;
        let activity_count = self.probe_claude_desktop_logs(run_id).await?;

        self.kernel.log_telemetry(
            json!({
                "agents_seeded": admin_agents.len() as u64,
                "agents_discovered": discovered_count,
                "agents_with_activity": activity_count
            }),
            "bootstrap_census",
            None,
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
        let node_agent = self.kernel.type_thing("node_agent")?;
        let substrate_thing = superx_kernel::Kernel::parse_id(substrate_id)?;

        let mut admin_agents: Vec<(String, String)> = Vec::new();
        for name in ["system_controller", "gemini_cli"] {
            let uid = Uuid::new_v5(sys_ns, format!("{tenant_id}:{name}").as_bytes());
            let agent_thing = Thing::from((
                "entity",
                surrealdb::sql::Id::Uuid(surrealdb::sql::Uuid::from(uid)),
            ));
            let id_literal = format!("entity:{uid}");

            // Idempotent SELECT-then-CREATE under superx (no UPSERT).
            let mut sel = self.kernel.db
                .query("SELECT id FROM entity WHERE id = $id LIMIT 1")
                .bind(("id", agent_thing.clone()))
                .await?;
            #[derive(serde::Deserialize)]
            struct IdRow { #[allow(dead_code)] id: Thing }
            if sel.take::<Vec<IdRow>>(0)?.pop().is_none() {
                self.kernel.db.query(
                    "CREATE entity CONTENT { \
                        id: $id, \
                        type: $type, \
                        tenant: $tenant, \
                        role: 'admin' \
                    }"
                )
                    .bind(("id", agent_thing.clone()))
                    .bind(("type", node_agent.clone()))
                    .bind(("tenant", substrate_thing.clone()))
                    .await?.check()?;
            }
            self.kernel.create_structural_edge(substrate_id, &id_literal, "edge_owns").await?;
            let desc = if name == "system_controller" {
                "SuperX Core System Controller"
            } else {
                "Gemini CLI Operator Agent"
            };
            self.kernel.supersede_state(&id_literal, "attr_desc", json!({"text": desc}), None).await?;
            self.kernel.log_telemetry(
                json!({"agent_id": id_literal, "name": name, "role": "admin", "source": "seeded", "run_id": run_id}),
                "agent_seeded",
                None,
            ).await?;
            admin_agents.push((name.to_string(), id_literal));
        }
        Ok(admin_agents)
    }

    /// Seed the five canonical tools and grant each admin agent capability to all of them.
    /// Tool entities use deterministic UUIDv5 ids derived from `(tenant, tool_uid)` so
    /// re-bootstraps reuse the same row idempotently.
    async fn seed_standard_tools(
        &self,
        tenant_id: &str,
        substrate_id: &str,
        admin_agents: &[(String, String)],
    ) -> Result<(), KernelError> {
        let node_tool = self.kernel.type_thing("node_tool")?;
        let substrate_thing = superx_kernel::Kernel::parse_id(substrate_id)?;
        let ns_uuid = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").expect("Valid DNS NS");

        let tools = ["tool_ingest", "tool_compile", "tool_propose", "tool_evaluate", "tool_promote"];
        for tool in tools {
            let tool_uuid = Uuid::new_v5(&ns_uuid, format!("{tenant_id}:{tool}").as_bytes());
            let tool_thing = Thing::from((
                "entity",
                surrealdb::sql::Id::Uuid(surrealdb::sql::Uuid::from(tool_uuid)),
            ));
            let tool_id_literal = format!("entity:{tool_uuid}");

            let mut sel = self.kernel.db
                .query("SELECT id FROM entity WHERE id = $id LIMIT 1")
                .bind(("id", tool_thing.clone()))
                .await?;
            #[derive(serde::Deserialize)]
            struct IdRow { #[allow(dead_code)] id: Thing }
            if sel.take::<Vec<IdRow>>(0)?.pop().is_none() {
                self.kernel.db.query(
                    "CREATE entity CONTENT { \
                        id: $id, \
                        type: $type, \
                        tenant: $tenant, \
                        role: 'user' \
                    }"
                )
                    .bind(("id", tool_thing))
                    .bind(("type", node_tool.clone()))
                    .bind(("tenant", substrate_thing.clone()))
                    .await?.check()?;
            }

            // Tag the tool entity with its canonical uid for identification —
            // the operational pattern was `entity:<tool_uid>` literal; now it's
            // `entity:<uuidv7>` + an attr_desc carrying the uid.
            self.kernel.supersede_state(
                &tool_id_literal,
                "attr_desc",
                json!({"text": tool}),
                None,
            ).await?;

            for (_, agent_id) in admin_agents {
                self.kernel.create_structural_edge(agent_id, &tool_id_literal, "edge_has_capability").await?;
            }
        }
        Ok(())
    }

    /// Probe every known MCP-client config on the current host (Claude Desktop,
    /// Claude Code, Gemini CLI; cross-platform) and emit one `agent_discovered`
    /// event per declared `mcpServers` entry. Each event carries a `source`
    /// field labelling which client surfaced the agent, so downstream
    /// consumers can fan out per client. Returns the total agent count.
    ///
    /// The agent's substrate uid is `UUIDv5(DNS, "<tenant>:<name>")` so the
    /// same `mcpServers` entry declared in multiple clients (e.g. an MCP
    /// server registered in both Claude Code and Gemini CLI) maps to one
    /// `node_agent` row — re-probing is idempotent and merges sources
    /// rather than duplicating identities.
    async fn probe_mcp_client_configs(
        &self,
        tenant_id: &str,
        run_id: &str,
        substrate_id: &str,
        sys_ns: &Uuid,
    ) -> Result<u64, KernelError> {
        let node_agent = self.kernel.type_thing("node_agent")?;
        let substrate_thing = superx_kernel::Kernel::parse_id(substrate_id)?;

        let mut discovered_count: u64 = 0;
        for (cfg_path, source) in mcp_client_config_candidates() {
            if !cfg_path.exists() {
                continue;
            }
            tracing::info!("Probing {source} config at {}", cfg_path.display());
            let Some(servers) = read_mcp_servers(&cfg_path) else { continue };
            for (name, server_cfg) in servers {
                let agent_uuid = Uuid::new_v5(sys_ns, format!("{tenant_id}:{name}").as_bytes());
                let agent_thing = Thing::from((
                    "entity",
                    surrealdb::sql::Id::Uuid(surrealdb::sql::Uuid::from(agent_uuid)),
                ));
                let agent_record_id = format!("entity:{agent_uuid}");

                let mut sel = self.kernel.db
                    .query("SELECT id FROM entity WHERE id = $id LIMIT 1")
                    .bind(("id", agent_thing.clone()))
                    .await?;
                #[derive(serde::Deserialize)]
                struct IdRow { #[allow(dead_code)] id: Thing }
                if sel.take::<Vec<IdRow>>(0)?.pop().is_none() {
                    self.kernel.db.query(
                        "CREATE entity CONTENT { \
                            id: $id, \
                            type: $type, \
                            tenant: $tenant, \
                            role: 'user' \
                        }"
                    )
                        .bind(("id", agent_thing))
                        .bind(("type", node_agent.clone()))
                        .bind(("tenant", substrate_thing.clone()))
                        .await?.check()?;
                }
                self.kernel.create_structural_edge(substrate_id, &agent_record_id, "edge_owns").await?;

                let command = server_cfg.get("command").and_then(|v| v.as_str()).unwrap_or("");
                let arg_count = server_cfg.get("args").and_then(|v| v.as_array()).map_or(0, Vec::len);

                self.kernel.supersede_state(
                    &agent_record_id,
                    "attr_desc",
                    json!({"text": format!("MCP agent `{name}` declared in {source} ({command})")}),
                    None,
                ).await?;
                self.kernel.log_telemetry(
                    json!({
                        "agent_id": agent_record_id,
                        "name": name,
                        "role": "user",
                        "source": source,
                        "config_path": cfg_path.display().to_string(),
                        "command": command,
                        "arg_count": arg_count,
                        "run_id": run_id,
                    }),
                    "agent_discovered",
                    None,
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
                    "mtime_since_epoch": mtime_iso,
                    "run_id": run_id
                }),
                "agent_activity_observed",
                None,
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

/// Probe set: every MCP-client config we know how to read on the current host,
/// each tagged with a stable `source` label that flows into `agent_discovered`
/// telemetry so downstream consumers can fan out per client.
///
/// Order is "most authoritative" first. Env-var overrides (`SUPERX_CLAUDE_CONFIG`,
/// `SUPERX_CLAUDE_CODE_CONFIG`, `SUPERX_GEMINI_CONFIG`) are returned ahead of
/// their default path so integration tests can inject fixtures deterministically.
///
/// Adding a new MCP client (Cursor, Continue, Cody, …) is a one-line entry here
/// — the parser ([`read_mcp_servers`]) already handles the shared `mcpServers`
/// JSON schema all three clients use.
fn mcp_client_config_candidates() -> Vec<(PathBuf, &'static str)> {
    let mut out: Vec<(PathBuf, &'static str)> = Vec::new();

    // Claude Desktop — platform-specific path, env override.
    if let Ok(p) = std::env::var("SUPERX_CLAUDE_CONFIG") {
        if !p.is_empty() {
            out.push((PathBuf::from(p), "claude_desktop"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            out.push((
                Path::new(&home).join("Library/Application Support/Claude/claude_desktop_config.json"),
                "claude_desktop",
            ));
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            out.push((
                Path::new(&appdata).join("Claude").join("claude_desktop_config.json"),
                "claude_desktop",
            ));
        }
    }
    #[cfg(target_os = "linux")]
    {
        // Linux is not officially documented by Anthropic; probe the conventional XDG path
        // as a best-effort fallback so Linux operators get the same discovery story.
        if let Ok(home) = std::env::var("HOME") {
            out.push((
                Path::new(&home).join(".config/Claude/claude_desktop_config.json"),
                "claude_desktop",
            ));
        }
    }

    // Claude Code — cross-platform `~/.claude.json`, env override.
    if let Ok(p) = std::env::var("SUPERX_CLAUDE_CODE_CONFIG") {
        if !p.is_empty() {
            out.push((PathBuf::from(p), "claude_code"));
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        out.push((Path::new(&home).join(".claude.json"), "claude_code"));
    }

    // Gemini CLI — cross-platform `~/.gemini/settings.json`, env override.
    if let Ok(p) = std::env::var("SUPERX_GEMINI_CONFIG") {
        if !p.is_empty() {
            out.push((PathBuf::from(p), "gemini_cli"));
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        out.push((Path::new(&home).join(".gemini/settings.json"), "gemini_cli"));
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
