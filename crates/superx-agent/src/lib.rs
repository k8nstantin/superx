/*
 * SuperX Agent Governance - Revision 42.11 (Hardened)
 * 
 * Copyright (c) 2026 Constantin Alexander <constantin@dedomena.io>
 */

#![deny(warnings)]
#![deny(clippy::pedantic)]

use superx_kernel::{Kernel, KernelError};
use serde_json::json;
use surrealdb::sql::Thing;
use uuid::Uuid;

/// `CapabilityGovernor`: Enforces graph-inferred security policies for agents.
pub struct CapabilityGovernor<'a> {
    pub kernel: &'a Kernel,
}

impl<'a> CapabilityGovernor<'a> {
    /// Creates a new `CapabilityGovernor`.
    #[must_use]
    pub fn new(kernel: &'a Kernel) -> Self {
        Self { kernel }
    }

    /// `handshake`: Authenticates an agent and initiates a durable session.
    ///
    /// # Panics
    /// Panics if `agent_uid` is empty.
    ///
    /// # Errors
    /// Returns `KernelError` if the agent or session records cannot be created.
    pub async fn handshake(&self, agent_uid: &str) -> Result<String, KernelError> {
        assert!(!agent_uid.is_empty(), "Agent identity mandatory");
        
        // 1. Resolve Session Tenant (Physical Isolation)
        let mut t_res = self.kernel.db.query("RETURN $session_tenant").await?;
        let session_tenant: String = t_res.take::<Option<String>>(0)?
            .ok_or_else(|| KernelError::SafetyViolation("No session tenant set before handshake".into()))?;

        let agent_thing = Thing::from(("entity".to_string(), agent_uid.to_string()));
        let session_uid = Uuid::now_v7().to_string();
        let session_thing = Thing::from(("entity".to_string(), session_uid.clone()));

        // 2. Identity Coercion Check: Ensure agent belongs to this tenant
        let check_query = "SELECT tenant_id FROM $id LIMIT 1";
        let mut check_res = self.kernel.db.query(check_query).bind(("id", agent_thing.clone())).await?;
        if let Ok(Some(row)) = check_res.take::<Vec<serde_json::Value>>(0).map(|mut v| v.pop()) {
            let existing_tenant = row.get("tenant_id").and_then(serde_json::Value::as_str).unwrap_or_default();
            if !existing_tenant.is_empty() && existing_tenant != session_tenant {
                return Err(KernelError::SafetyViolation(format!("Agent {agent_uid} belongs to tenant {existing_tenant}, but session is {session_tenant}")));
            }
        }

        // 3. Upsert Identity
        let _ = self.kernel.db.query("UPSERT $id SET tenant_id = $t, type = type_definition:node_agent")
            .bind(("id", agent_thing.clone()))
            .bind(("t", session_tenant.clone())).await?.check()?;

        // 4. Create session
        let _ = self.kernel.db.query("INSERT INTO entity { id: $id, tenant_id: $t, type: type_definition:node_session }")
            .bind(("id", session_thing.clone()))
            .bind(("t", session_tenant.clone())).await?.check()?;

        // 5. Link session to agent
        self.kernel.create_structural_edge(&format!("entity:{session_uid}"), &format!("entity:{agent_uid}"), "edge_participates_in").await?;

        self.kernel.log_telemetry(
            json!({"agent": agent_uid, "session": session_uid}),
            "agent_handshake",
            Some(session_uid.clone())
        ).await?;

        Ok(session_uid)
    }

    /// `check_capability`: Verifies if an agent has permission to execute a tool.
    ///
    /// Record IDs flow through this function as `Thing` values throughout — never as
    /// `<string>id`-cast strings. Hyphen-bearing UUIDs would otherwise be double-escaped
    /// by `EscapeRidKey` on the way out and fail to compare equal on the way back in.
    /// Existence is verified via `count()` to avoid deserializing the matched row's id
    /// (which is itself a `Thing` enum, not a JSON value).
    ///
    /// # Errors
    /// Returns `KernelError::Validation` if `agent_id` is malformed.
    /// Returns `KernelError::SafetyViolation` if no `edge_has_capability` exists between
    /// the agent and the tool within the current session tenant.
    pub async fn check_capability(&self, agent_id: &str, tool_uid: &str) -> Result<(), KernelError> {
        #[derive(serde::Deserialize)]
        struct CountRow { count: u64 }

        let agent_thing = Kernel::parse_id(agent_id)?;
        let tool_thing = Kernel::parse_id(&format!("entity:{tool_uid}"))?;

        let query = "SELECT count() AS count FROM relation \
            WHERE in = $agent AND out = $tool \
            AND type = type_definition:edge_has_capability \
            AND tenant_id = $session_tenant GROUP ALL";
        let mut res = self.kernel.db.query(query)
            .bind(("agent", agent_thing))
            .bind(("tool", tool_thing)).await?;

        let count = res.take::<Vec<CountRow>>(0)?.pop().map_or(0, |r| r.count);
        if count == 0 {
            return Err(KernelError::SafetyViolation(format!("Agent {agent_id} lacks capability for {tool_uid}")));
        }

        Ok(())
    }
}
