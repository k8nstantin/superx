//! # superx-agent — Capability Governor
//!
//! Implements the **graph-derived security policy** primitive
//! (`ARCHITECTURE.md` §0c Cognitive Governance). Agents are entities; their
//! ability to invoke a tool is determined by whether an `edge_has_capability`
//! relation exists between the agent entity and the tool entity *within the
//! current session tenant*.
//!
//! ## Entry points
//!
//! - [`CapabilityGovernor::handshake`] — authenticates an agent and opens a
//!   `node_session` entity linked to it via `edge_participates_in`. Returns
//!   the session uid the agent can later present.
//! - [`CapabilityGovernor::check_capability`] — predicate: does this agent
//!   hold capability for this tool? Returns `Ok(())` or a `SafetyViolation`
//!   with explicit ALLOW / DENY tracing on each decision.
//!
//! ## Design notes
//!
//! - **Identity coercion is rejected.** `handshake` refuses if the agent
//!   already exists in a different tenant — operators cannot accidentally
//!   re-tenant a registered agent.
//! - **Record IDs flow as `Thing`, never as `<string>id` round-trips.** The
//!   `EscapeRidKey` escaping rule (see `surrealdb-core/src/sql/escape.rs`)
//!   double-escapes hyphen-bearing UUIDs on round-trip; using `Thing` end-to-end
//!   avoids this entirely. The existence query uses `count()` instead of
//!   selecting the matched row's id to avoid deserialising a `Thing` enum
//!   into a `serde_json::Value` (which fails).
//!
//! Copyright (c) 2026 Constantin Alexander <constantin@dedomena.io>.
//! Licensed under the Apache License, Version 2.0.

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
        tracing::info!("governor handshake: agent_uid={agent_uid}");

        // 1. Resolve session tenant + metamodel FKs.
        let session_tenant_str = self.kernel.session_tenant().await?;
        let tenant_thing = Kernel::parse_id(&format!("entity:{session_tenant_str}"))?;
        let node_agent = self.kernel.type_thing("node_agent")?;
        let node_session = self.kernel.type_thing("node_session")?;

        let agent_thing = Kernel::parse_id(agent_uid)?;
        let session_uid = Uuid::now_v7();
        let session_thing = Thing::from((
            "entity",
            surrealdb::sql::Id::Uuid(surrealdb::sql::Uuid::from(session_uid)),
        ));

        // 2. Identity-coercion check: if the agent row already exists, it
        //    must belong to the session's tenant. Under v2 we read the
        //    typed FK's id field (`tenant.id`) instead of the dropped
        //    `tenant_id` string.
        #[derive(serde::Deserialize)]
        struct TenantOnly { tenant_id: Option<String> }
        let mut check_res = self.kernel.db
            .query("SELECT tenant.id AS tenant_id FROM $id LIMIT 1")
            .bind(("id", agent_thing.clone()))
            .await?;
        if let Some(row) = check_res.take::<Vec<TenantOnly>>(0)?.pop() {
            if let Some(existing) = row.tenant_id {
                if existing != session_tenant_str {
                    return Err(KernelError::SafetyViolation(format!(
                        "Agent {agent_uid} belongs to tenant {existing}, but session is {session_tenant_str}"
                    )));
                }
            }
        } else {
            // Fresh agent — CREATE under superx (UPSERT is engine-refused).
            self.kernel.db.query(
                "CREATE entity CONTENT { \
                    id: $id, \
                    type: $type, \
                    tenant: $tenant, \
                    role: 'user' \
                }"
            )
                .bind(("id", agent_thing.clone()))
                .bind(("type", node_agent))
                .bind(("tenant", tenant_thing.clone()))
                .await?.check()?;
        }

        // 3. Create session entity (node_session, tenant FK to substrate).
        self.kernel.db.query(
            "CREATE entity CONTENT { \
                id: $id, \
                type: $type, \
                tenant: $tenant, \
                role: 'user' \
            }"
        )
            .bind(("id", session_thing.clone()))
            .bind(("type", node_session))
            .bind(("tenant", tenant_thing))
            .await?.check()?;

        // 4. Link session → agent (edge_participates_in).
        let session_record_id = format!("entity:{session_uid}");
        self.kernel.create_structural_edge(&session_record_id, agent_uid, "edge_participates_in").await?;

        self.kernel.log_telemetry(
            json!({"agent": agent_uid, "session": session_record_id}),
            "agent_handshake",
            None,
        ).await?;

        Ok(session_uid.to_string())
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

        tracing::info!("capability check: agent_id={agent_id} tool_uid={tool_uid}");

        let agent_thing = Kernel::parse_id(agent_id)?;
        let tool_thing = Kernel::parse_id(&format!("entity:{tool_uid}"))?;
        let edge_has_capability = self.kernel.type_thing("edge_has_capability")?;
        let session_tenant_str = self.kernel.session_tenant().await?;
        let tenant_thing = Kernel::parse_id(&format!("entity:{session_tenant_str}"))?;

        let query = "SELECT count() AS count FROM relation \
            WHERE in = $agent AND out = $tool \
            AND type = $edge_type \
            AND tenant = $tenant GROUP ALL";
        let mut res = self.kernel.db.query(query)
            .bind(("agent", agent_thing))
            .bind(("tool", tool_thing))
            .bind(("edge_type", edge_has_capability))
            .bind(("tenant", tenant_thing))
            .await?;

        let count = res.take::<Vec<CountRow>>(0)?.pop().map_or(0, |r| r.count);
        if count == 0 {
            tracing::info!("capability DENY: agent_id={agent_id} tool_uid={tool_uid}");
            return Err(KernelError::SafetyViolation(format!("Agent {agent_id} lacks capability for {tool_uid}")));
        }
        tracing::info!("capability ALLOW: agent_id={agent_id} tool_uid={tool_uid}");

        Ok(())
    }
}
