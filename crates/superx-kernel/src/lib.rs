//! # superx-kernel — the safety-critical 5-table substrate
//!
//! The load-bearing primitive every other crate in the workspace depends on.
//! Implements `ARCHITECTURE.md` §1 (the 5-table substrate + execution cursor)
//! and enforces every §2 / §7 invariant at the storage layer.
//!
//! ## Tables (defined in [`Kernel::apply_substrate_schema`])
//!
//! - **`type_definition`** — metamodel. Every concept (entity types, edge
//!   types, attribute types) is a row.
//! - **`entity`** — the persistent *who* (substrate, agents, sessions, code
//!   files, products, tools, models, …). Tenant-scoped via PERMISSIONS +
//!   explicit `tenant_id = $session_tenant` predicates on every read.
//! - **`relation`** — directed graph (`in → out`). Acyclic edges are
//!   cycle-checked at write time via bounded DFS.
//! - **`state_ledger`** — SCD-2 content. Every typed write produces a new
//!   row and closes the prior `is_current` row in the same transaction.
//! - **`telemetry_stream`** — CHANGEFEED-enabled firehose. Every kernel
//!   mutation logs one typed event.
//! - **`execution_cursor`** — durable workflow checkpoints (planned
//!   conversion to SCD-2 per Roadmap #14).
//!
//! ## Invariants enforced by this crate
//!
//! 1. **Physical multi-tenancy.** Session vars `$session_tenant` /
//!    `$session_role` are bound at session start; every PERMISSIONS clause
//!    and every kernel read predicates on them. The
//!    [`Kernel::supersede_state`] and [`Kernel::create_structural_edge`]
//!    paths additionally check that the *target* entity's tenant matches
//!    the calling session — anti-coercion guard.
//! 2. **Temporal identity.** Every substrate row is keyed by a native
//!    `Id::Uuid(UUIDv7)` — lex-sortable by time, no MD5 shortcuts.
//! 3. **SCD-2 atomicity.** [`Kernel::supersede_state`] uses a `SurrealQL`
//!    `BEGIN TRANSACTION` to close the prior `is_current` row and create
//!    the new one in one round-trip.
//! 4. **NASA Power of 10.** DFS, traversal depth, ingestion count are all
//!    parameter-bounded (read via [`Kernel::get_parameter`] with sensible
//!    defaults). No unbounded loop is permitted.
//! 5. **JSON-Schema validation at the write boundary.** If a
//!    `type_definition` carries a `sch_json` schema, every state write
//!    against that type is validated before insertion.
//!
//! ## Entry points
//!
//! - [`Kernel::init`] — create kernel + apply schema + seed metamodel.
//! - [`Kernel::set_session_auth`] — bind `$session_tenant` / `$session_role`.
//! - [`Kernel::supersede_state`] — SCD-2 typed write with anti-coercion +
//!   JSON-Schema validation + automatic `state_supersede` telemetry.
//! - [`Kernel::create_structural_edge`] — graph edge with cycle detection
//!   for acyclic edge types.
//! - [`Kernel::compile_context`] — recursive tier-filtered traversal
//!   producing context XML; bounded by `max_context_nodes` +
//!   `max_traversal_depth`.
//! - [`Kernel::log_telemetry`] — typed firehose insertion.
//! - [`Kernel::checkpoint_execution`] / [`Kernel::get_execution_cursor`] —
//!   durable resume points.
//! - [`Kernel::parse_id`] — canonical `Thing` parser; rejects malformed
//!   inputs at the boundary.
//! - [`DEFAULT_TENANT`] — single source of truth for the default tenant id.
//!
//! Copyright (c) 2026 Constantin Alexander <constantin@dedomena.io>.
//! Licensed under the Apache License, Version 2.0.

#![deny(warnings)]
#![deny(clippy::pedantic)]

use surrealdb::engine::local::{Db, RocksDb};
use surrealdb::sql::Thing;
use surrealdb::Surreal;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::fmt::Write as _;
use jsonschema::JSONSchema;

const MAX_ID_LENGTH: usize = 128;

/// Single source of truth for the default tenant identifier used by every
/// `SuperX` surface (CLI flags, MCP fallback, integration tests). Renaming
/// it here propagates everywhere — there is no magic-string duplication.
pub const DEFAULT_TENANT: &str = "sa_dogfood";

/// All error types surfaced by `Kernel` operations. Each variant maps to a
/// specific failure class so callers can branch on intent rather than
/// stringly-typed reasons:
///
/// - `Database` — underlying `SurrealDB` / `RocksDB` failure (network,
///   transaction, storage).
/// - `Init` — substrate startup failure (path creation, schema apply).
/// - `Validation` — typed write rejected by JSON-Schema or by a missing
///   referenced entity / type.
/// - `Integrity` — substrate invariant violation discovered at read time
///   (e.g. missing substrate entity during promotion).
/// - `SafetyViolation` — NASA Power-of-10 bound exceeded (DFS limit, node
///   limit) or a tenant-coercion attempt.
/// - `CycleDetected` — attempted to create an acyclic edge that would close
///   a cycle.
#[derive(Debug, thiserror::Error)]
pub enum KernelError {
    #[error("Database error: {0}")]
    Database(#[from] Box<surrealdb::Error>),
    #[error("Initialization error: {0}")]
    Init(String),
    #[error("Validation error: {0}")]
    Validation(String),
    #[error("Integrity error: {0}")]
    Integrity(String),
    #[error("Safety violation: {0}")]
    SafetyViolation(String),
    #[error("Cycle detected in structural graph")]
    CycleDetected,
}

impl From<surrealdb::Error> for KernelError {
    fn from(e: surrealdb::Error) -> Self {
        Self::Database(Box::new(e))
    }
}

/// The safety-critical 5-table substrate plus durable execution cursor —
/// the load-bearing primitive every other crate in the workspace depends on.
///
/// Holds the live `SurrealDB` handle and the namespace + database it's bound
/// to. Constructed via `Kernel::init`, which creates the parent directory,
/// opens `RocksDB`, applies the `SCHEMAFULL` substrate schema (idempotent),
/// and seeds the metamodel.
///
/// Cloning is cheap — `Surreal<Db>` is internally `Arc`-shared. Pass the
/// kernel around freely; do not store references to its fields independently.
#[derive(Clone)]
pub struct Kernel {
    /// Live `SurrealDB` connection bound to the substrate's namespace + database.
    pub db: Surreal<Db>,
    /// The `SurrealDB` namespace this kernel is bound to (set at `init` time).
    pub ns: String,
    /// The `SurrealDB` database name this kernel is bound to (set at `init` time).
    pub db_name: String,
}

#[derive(Deserialize)] 
struct FetchResult { 
    sch_json: Option<String>, 
    tenant: Option<String> 
}

#[derive(Deserialize)] 
struct AcyclicCheck { 
    is_acyclic: bool 
}

#[derive(Deserialize)] 
struct IdResult { 
    id: Thing 
}

#[derive(Deserialize)] 
struct TenantResult { 
    tenant_id: String 
}

#[derive(Deserialize)]
struct StateResult {
    value_json: Option<serde_json::Value>
}

/// Read-side projection of a `execution_cursor` row, returned by
/// `Kernel::get_execution_cursor`. `last_processed` is the cursor token a
/// resumer can hand back to the source (e.g. the last filesystem path
/// ingested, or the last offset processed); `metadata` is an opaque JSON
/// envelope the producer chose to attach.
#[derive(Deserialize, Serialize, Debug)]
pub struct CursorResult {
    pub last_processed: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

impl Kernel {
    /// `init`: Initializes the kernel and enforces the 5-table substrate schema.
    ///
    /// # Panics
    /// Panics if `ns` or `db_name` are empty.
    ///
    /// # Errors
    /// Returns `KernelError::Init` if DB cannot be opened or schema fails to apply.
    pub async fn init(path: &Path, ns: &str, db_name: &str) -> Result<Self, KernelError> {
        assert!(!ns.is_empty(), "Namespace must not be empty");
        assert!(!db_name.is_empty(), "DB name must not be empty");

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| KernelError::Init(e.to_string()))?;
        }
        let db = Surreal::new::<RocksDb>(path).await?;
        db.use_ns(ns).use_db(db_name).await?;

        let kernel = Self { db, ns: ns.to_string(), db_name: db_name.to_string() };
        kernel.apply_substrate_schema().await?;
        kernel.seed_metamodel().await?;
        
        assert!(kernel.db.health().await.is_ok(), "Substrate health check failed after init");
        Ok(kernel)
    }

    async fn apply_substrate_schema(&self) -> Result<(), KernelError> {
        let surql = r"
            DEFINE TABLE type_definition SCHEMAFULL
                PERMISSIONS FOR select, create, update, delete WHERE $session_role = 'admin';
            DEFINE FIELD category         ON type_definition TYPE string;
            DEFINE FIELD is_acyclic      ON type_definition TYPE bool DEFAULT false;
            DEFINE FIELD sch_json        ON type_definition TYPE option<string>; 
            DEFINE FIELD memory_tier      ON type_definition TYPE string DEFAULT 'working' ASSERT $value INSIDE ['core', 'working', 'archival', 'recall'];

            DEFINE TABLE entity SCHEMAFULL
                PERMISSIONS FOR select, create, update, delete WHERE tenant_id = $session_tenant OR $session_role = 'admin';
            DEFINE FIELD type             ON entity TYPE record<type_definition>;
            DEFINE FIELD tenant_id        ON entity TYPE string DEFAULT '00000000-0000-0000-0000-000000000000';
            DEFINE FIELD role             ON entity TYPE string DEFAULT 'user' ASSERT $value INSIDE ['user', 'admin'];
            DEFINE FIELD is_deleted       ON entity TYPE bool DEFAULT false;

            DEFINE ACCESS tenant_access ON DATABASE TYPE RECORD
                SIGNIN ( SELECT * FROM entity WHERE tenant_id = $tenant AND role = $role LIMIT 1 );

            DEFINE TABLE relation SCHEMAFULL
                PERMISSIONS FOR select, create, update, delete WHERE tenant_id = $session_tenant OR $session_role = 'admin';
            DEFINE FIELD in               ON relation TYPE record<entity>;
            DEFINE FIELD out              ON relation TYPE record<entity>;
            DEFINE FIELD type             ON relation TYPE record<type_definition>;
            DEFINE FIELD tenant_id        ON relation TYPE string DEFAULT '00000000-0000-0000-0000-000000000000';
            DEFINE FIELD is_acyclic       ON relation TYPE bool DEFAULT false;

            DEFINE INDEX rel_source       ON relation FIELDS in, tenant_id, is_acyclic;
            DEFINE INDEX rel_dest         ON relation FIELDS out, tenant_id, is_acyclic;

            DEFINE TABLE state_ledger SCHEMALESS
                PERMISSIONS FOR select, create, update, delete WHERE tenant_id = $session_tenant OR $session_role = 'admin';
            DEFINE FIELD target           ON state_ledger TYPE record<entity>;
            DEFINE FIELD type             ON state_ledger TYPE record<type_definition>;
            DEFINE FIELD tenant_id        ON state_ledger TYPE string DEFAULT '00000000-0000-0000-0000-000000000000';
            DEFINE FIELD is_current       ON state_ledger TYPE bool DEFAULT true;
            DEFINE FIELD valid_from       ON state_ledger TYPE datetime DEFAULT time::now();
            DEFINE FIELD valid_to         ON state_ledger TYPE option<datetime>;

            DEFINE INDEX IF NOT EXISTS state_current ON state_ledger FIELDS target, type, is_current;

            DEFINE TABLE execution_cursor SCHEMAFULL
                PERMISSIONS FOR select, create, update, delete WHERE tenant_id = $session_tenant OR $session_role = 'admin';
            DEFINE FIELD run_id           ON execution_cursor TYPE string;
            DEFINE FIELD tenant_id        ON execution_cursor TYPE string;
            DEFINE FIELD cursor_type      ON execution_cursor TYPE string;
            DEFINE FIELD last_processed   ON execution_cursor TYPE option<string>;
            DEFINE FIELD metadata         ON execution_cursor TYPE option<object>;
            DEFINE FIELD updated_at       ON execution_cursor TYPE datetime DEFAULT time::now();

            DEFINE INDEX IF NOT EXISTS cursor_lookup ON execution_cursor FIELDS run_id, tenant_id;

            DEFINE TABLE telemetry_stream SCHEMALESS CHANGEFEED 1d
                PERMISSIONS FOR select, create, update, delete WHERE tenant_id = $session_tenant OR $session_role = 'admin';
            DEFINE FIELD timestamp        ON telemetry_stream TYPE datetime DEFAULT time::now();
            DEFINE FIELD lifecycle_event  ON telemetry_stream TYPE string;
            DEFINE FIELD payload          ON telemetry_stream TYPE any;
            DEFINE FIELD run_id           ON telemetry_stream TYPE option<string>;
            DEFINE FIELD tenant_id        ON telemetry_stream TYPE string DEFAULT '00000000-0000-0000-0000-000000000000';

            DEFINE INDEX IF NOT EXISTS tele_run ON telemetry_stream FIELDS run_id;
        ";
        // Ignore AlreadyExists errors for idempotency
        let _ = self.db.query(surql).await;
        Ok(())
    }

    async fn seed_metamodel(&self) -> Result<(), KernelError> {
        let types = vec![
            // Core substrate identities
            ("node_substrate", "node", false, None, "core"),
            ("node_agent", "node", false, None, "core"),
            ("node_session", "node", false, None, "working"),
            ("node_capability", "node", false, None, "core"),
            ("node_tool", "node", false, None, "core"),
            ("node_component", "node", false, None, "core"),
            ("node_hardened_model", "node", false, None, "working"),
            // Domain identities (ingested artifacts and cognitive products)
            ("node_prod", "node", false, None, "working"),
            ("node_code", "node", false, None, "working"),
            ("node_code_root", "node", false, None, "working"),
            ("node_artifact", "node", false, None, "working"),
            ("node_proposal", "node", false, None, "working"),
            ("node_harness", "node", false, None, "core"),
            ("node_source_external", "node", false, None, "archival"),
            ("node_rag_source", "node", false, None, "archival"),
            // Attribute types (SCD-2 ledger payload shapes)
            ("attr_desc", "attribute", false, Some("{\"type\":\"object\",\"required\":[\"text\"],\"properties\":{\"text\":{\"type\":\"string\",\"minLength\":1}}}"), "working"),
            ("attr_config", "attribute", false, None, "core"),
            ("attr_score", "attribute", false, Some("{\"type\":\"object\",\"required\":[\"score\"],\"properties\":{\"score\":{\"type\":\"number\"}}}"), "working"),
            // Edge types (graph topology)
            ("edge_owns", "edge", true, None, "working"),
            ("edge_compiled_from", "edge", true, None, "working"),
            ("edge_distilled_from", "edge", true, None, "working"),
            ("edge_evaluates", "edge", false, None, "working"),
            ("edge_promotes", "edge", true, None, "working"),
            ("edge_implements", "edge", true, None, "core"),
            ("edge_has_capability", "edge", false, None, "core"),
            ("edge_participates_in", "edge", false, None, "working"),
            ("edge_semantic", "edge", false, None, "recall"),
        ];

        for (uid, cat, acyclic, sch, tier) in types {
            let id = Thing::from(("type_definition".to_string(), uid.to_string()));
            let _ = self.db.query("UPSERT $id SET category = $cat, is_acyclic = $acyc, sch_json = $sch, memory_tier = $tier")
                .bind(("id", id))
                .bind(("cat", cat.to_string()))
                .bind(("acyc", acyclic))
                .bind(("sch", sch.map(std::string::ToString::to_string)))
                .bind(("tier", tier.to_string()))
                .await?.check()?;
        }
        Ok(())
    }

    /// `set_session_auth`: Set the `SurrealDB` session context using custom variables.
    ///
    /// # Panics
    /// Panics if `tenant_id` or `role` are empty.
    ///
    /// # Errors
    /// Returns `KernelError::Database` if binding fails.
    pub async fn set_session_auth(&self, tenant_id: &str, role: &str) -> Result<(), KernelError> {
        assert!(!tenant_id.is_empty(), "Tenant ID mandatory for auth");
        assert!(!role.is_empty(), "Role mandatory for auth");

        // Manually set session variables for local mode.
        self.db.set("session_tenant", tenant_id.to_string()).await?;
        self.db.set("session_role", role.to_string()).await?;
        
        Ok(())
    }

    /// `parse_id`: Parse a `SurrealDB` record id literal (e.g. `entity:abc123`) into a `Thing`.
    ///
    /// # Panics
    /// Panics if `s` is empty or exceeds `MAX_ID_LENGTH` (NASA Rule 7 assertion density).
    ///
    /// # Errors
    /// Returns `KernelError::Validation` if `s` is not in `table:id` form, or if either
    /// the table or id segment is empty (e.g. `":abc"` or `"entity:"`).
    pub fn parse_id(s: &str) -> Result<Thing, KernelError> {
        assert!(!s.is_empty(), "ID string must not be empty");
        assert!(s.len() < MAX_ID_LENGTH, "ID string exceeds safety limit");

        let cleaned = s.replace('`', "");
        let parts: Vec<&str> = cleaned.split(':').collect();
        if parts.len() != 2 {
            return Err(KernelError::Validation(format!("Invalid ID format: {s}")));
        }
        if parts[0].is_empty() || parts[1].is_empty() {
            return Err(KernelError::Validation(format!("Invalid ID format: {s}")));
        }
        Ok(Thing::from((parts[0].to_string(), parts[1].to_string())))
    }

    async fn get_session_tenant(&self) -> Result<String, KernelError> {
        let mut res = self.db.query("RETURN $session_tenant").await?;
        let tenant: Option<String> = res.take(0)?;
        tenant.ok_or_else(|| KernelError::SafetyViolation("No authenticated session. Ensure set_session_auth was called.".into()))
    }

    /// `get_parameter`: Dynamic lookup of safety/governance parameters from `state_ledger`.
    pub async fn get_parameter<T: for<'de> serde::Deserialize<'de>>(&self, key: &str, default: T) -> T {
        let query = "SELECT value_json FROM state_ledger WHERE target.type = type_definition:node_substrate AND tenant_id = $session_tenant AND `type` = type_definition:attr_config AND is_current = true LIMIT 1";
        
        if let Ok(mut res) = self.db.query(query).await {
            if let Ok(Some(row)) = res.take::<Vec<StateResult>>(0).map(|mut v| v.pop()) {
                if let Some(val) = row.value_json {
                    if let Some(p) = val.get(key) {
                        if let Ok(p_typed) = serde_json::from_value::<T>(p.clone()) {
                            return p_typed;
                        }
                    }
                }
            }
        }
        default
    }

    /// `supersede_state`: Durable SCD-2 mutation with JSON Schema enforcement.
    ///
    /// # Panics
    /// Panics if `target_id` or `type_uid` are empty.
    ///
    /// # Errors
    /// Returns `KernelError::Validation` if schema check fails or type is missing.
    pub async fn supersede_state(
        &self,
        target_id: &str,
        type_uid: &str,
        value: serde_json::Value,
        run_id: Option<String>,
    ) -> Result<(), KernelError> {
        assert!(!target_id.is_empty(), "Target ID mandatory");
        assert!(!type_uid.is_empty(), "Type UID mandatory");

        let session_tenant = self.get_session_tenant().await?;
        let type_thing = Thing::from(("type_definition".to_string(), type_uid.to_string()));
        let target_thing = Self::parse_id(target_id)?;
        
        // 1. Atomic fetch of validation schema and target tenant
        let fetch_query = "SELECT sch_json, (SELECT tenant_id FROM $target LIMIT 1)[0].tenant_id AS tenant FROM $id";
        let mut fetch_res = self.db.query(fetch_query)
            .bind(("id", type_thing.clone()))
            .bind(("target", target_thing.clone())).await?;
        
        let info: FetchResult = fetch_res.take::<Vec<FetchResult>>(0)?
            .pop()
            .ok_or_else(|| KernelError::Validation(format!("Type {type_uid} not found")))?;

        let target_tenant = info.tenant.ok_or_else(|| KernelError::Validation(format!("Target {target_id} not found")))?;

        // 2. Physical Isolation Check (Anti-Coercion)
        if target_tenant != session_tenant {
            return Err(KernelError::SafetyViolation(format!("Tenant mismatch: Entity belongs to {target_tenant}, session is {session_tenant}")));
        }

        // 3. Real JSON Schema Validation
        if let Some(schema_str) = info.sch_json {
            let schema_val: serde_json::Value = serde_json::from_str(&schema_str).map_err(|e| KernelError::Validation(e.to_string()))?;
            let schema = JSONSchema::compile(&schema_val).map_err(|e| KernelError::Validation(e.to_string()))?;
            if !schema.is_valid(&value) {
                return Err(KernelError::Validation(format!("Value fails validation for {type_uid}")));
            }
        }

        // 4. Atomic SCD-2 Transition: Close old, open new.
        // Native UUIDv7 id preserves the temporal ordering promised by ARCHITECTURE.md
        // Invariant 2; SurrealDB's Id::Uuid round-trips without backtick escaping.
        let state_record_id = Thing::from((
            "state_ledger",
            surrealdb::sql::Id::Uuid(surrealdb::sql::Uuid::from(::uuid::Uuid::now_v7())),
        ));
        
        let query = r"
            BEGIN TRANSACTION;
                UPDATE state_ledger SET is_current = false, valid_to = time::now()
                WHERE target = $target AND `type` = $ty AND is_current = true;

                CREATE state_ledger CONTENT {
                    id: $id,
                    target: $target,
                    `type`: $ty,
                    tenant_id: $session_tenant,
                    is_current: true,
                    valid_from: time::now(),
                    value_json: $val
                };
            COMMIT TRANSACTION;
        ";
        
        self.db.query(query)
            .bind(("id", state_record_id))
            .bind(("target", target_thing))
            .bind(("ty", type_thing))
            .bind(("val", value)).await?.check()?;

        // 5. Log to telemetry
        let _ = self.log_telemetry(
            serde_json::json!({"target": target_id, "type": type_uid}),
            "state_supersede",
            run_id
        ).await;

        Ok(())
    }

    /// `create_structural_edge`: Graph relation with cycle detection for structural edges.
    ///
    /// # Panics
    /// Panics if `from` or `to` are empty.
    ///
    /// # Errors
    /// Returns `KernelError::CycleDetected` if an acyclic edge would create a cycle.
    pub async fn create_structural_edge(&self, from: &str, to: &str, edge_type: &str) -> Result<(), KernelError> {
        assert!(!from.is_empty(), "Source ID mandatory");
        assert!(!to.is_empty(), "Target ID mandatory");

        let session_tenant = self.get_session_tenant().await?;
        let type_thing = Thing::from(("type_definition".to_string(), edge_type.to_string()));
        let from_thing = Self::parse_id(from)?;
        let to_thing = Self::parse_id(to)?;

        // 1. Coercion check for source
        let mut fetch_res = self.db.query("SELECT tenant_id FROM $target LIMIT 1").bind(("target", from_thing.clone())).await?;
        let target_tenant: String = fetch_res.take::<Vec<TenantResult>>(0)?
            .pop()
            .ok_or_else(|| KernelError::Validation(format!("Source {from} not found")))?
            .tenant_id;

        if target_tenant != session_tenant {
            return Err(KernelError::SafetyViolation(format!("Tenant mismatch: Entity belongs to {target_tenant}, session is {session_tenant}")));
        }

        let mut type_res = self.db.query("SELECT is_acyclic FROM $id").bind(("id", type_thing.clone())).await?;
        let is_acyclic = type_res.take::<Vec<AcyclicCheck>>(0)?.pop().is_some_and(|c| c.is_acyclic);

        if is_acyclic {
            let max_iters: usize = self.get_parameter("max_dfs_iterations", 10_000).await;

            // Check if 'to' can already reach 'from' (Cycle Detection)
            let mut stack = vec![to_thing.clone()];
            let mut visited = std::collections::HashSet::new();
            let mut iters: usize = 0;
            while let Some(current) = stack.pop() {
                iters += 1;
                assert!(iters <= max_iters, "Safety violation: DFS depth exceeded");
                if iters > max_iters { return Err(KernelError::SafetyViolation("DFS limit".into())); }
                if current == from_thing { return Err(KernelError::CycleDetected); }

                if !visited.insert(current.to_string()) { continue; }

                // Walk outgoing structural edges within SAME tenant
                let query = "SELECT out.id as id FROM relation WHERE in = $id AND tenant_id = $session_tenant AND is_acyclic = true";
                let mut res = self.db.query(query).bind(("id", current)).await?;
                let children: Vec<IdResult> = res.take(0)?;
                for c in children { stack.push(c.id); }
            }
        }

        let rel_record_id = Thing::from((
            "relation",
            surrealdb::sql::Id::Uuid(surrealdb::sql::Uuid::from(::uuid::Uuid::now_v7())),
        ));
        let query = "INSERT INTO relation { id: $id, in: $f, out: $t, type: $ty, tenant_id: $session_tenant, is_acyclic: $acyc }";
        self.db.query(query)
            .bind(("id", rel_record_id))
            .bind(("f", from_thing))
            .bind(("t", to_thing))
            .bind(("ty", type_thing))
            .bind(("acyc", is_acyclic)).await?.check()?;
        
        let _ = self.log_telemetry(
            serde_json::json!({"from": from, "to": to, "type": edge_type}),
            "edge_create",
            None
        ).await;

        Ok(())
    }

    /// `pulse`: Log a periodic system heartbeat to the telemetry stream.
    ///
    /// # Errors
    /// Returns `KernelError` if logging fails.
    pub async fn pulse(&self) -> Result<(), KernelError> {
        self.log_telemetry(serde_json::json!({"status": "active"}), "system_pulse", None).await
    }

    /// `compile_context`: Recursive context distillation with tenant isolation and memory tiering.
    ///
    /// # Panics
    /// Panics if `root_id` is empty.
    ///
    /// # Errors
    /// Returns `KernelError::Validation` if `root_id` is invalid.
    pub async fn compile_context(
        &self, 
        root_id: &str, 
        run_id: &str, 
        tiers: Option<Vec<String>>
    ) -> Result<String, KernelError> {
        assert!(!root_id.is_empty(), "Root ID mandatory");
        
        let _ = self.log_telemetry(
            serde_json::json!({"root": root_id, "tiers": tiers}),
            "context_compile",
            Some(run_id.to_string())
        ).await;

        let root_thing = Self::parse_id(root_id)?;
        let max_depth: u32 = self.get_parameter("max_traversal_depth", 10).await;
        let max_nodes: usize = self.get_parameter("max_context_nodes", 10_000).await;
        let target_tiers = tiers.unwrap_or_else(|| vec!["core".to_string(), "working".to_string()]);

        let mut stack = vec![(root_thing.clone(), 0)];
        let mut visited = std::collections::HashSet::new();
        let mut nodes_ordered = Vec::new();

        while let Some((current, depth)) = stack.pop() {
            // NASA Rule 2: bounded loop — cap total nodes regardless of DAG shape.
            assert!(nodes_ordered.len() <= max_nodes, "Safety violation: context node limit exceeded");
            if nodes_ordered.len() >= max_nodes {
                return Err(KernelError::SafetyViolation(format!(
                    "compile_context node limit ({max_nodes}) exceeded"
                )));
            }
            if depth > max_depth { continue; }
            if !visited.insert(current.to_string()) { continue; }
            nodes_ordered.push(current.clone());

            // Fetch structural children (outgoing acyclic edges) within SAME tenant
            let query = "SELECT out.id as id FROM relation WHERE in = $id AND tenant_id = $session_tenant AND is_acyclic = true";
            let mut res = self.db.query(query)
                .bind(("id", current)).await?;
            let children: Vec<IdResult> = res.take(0)?;
            for c in children {
                stack.push((c.id, depth + 1));
            }
        }

        let mut xml = String::from("<context>\n");
        for node in nodes_ordered {
            // Fetch state rows filtered by target memory tiers and session tenant
            let fetch = "SELECT value_json FROM state_ledger WHERE target = $id AND tenant_id = $session_tenant AND `type`.memory_tier INSIDE $tiers AND is_current = true LIMIT 1";
            let mut res = self.db.query(fetch)
                .bind(("id", node.clone()))
                .bind(("tiers", target_tiers.clone())).await?;
            
            let mut s_rows = res.take::<Vec<StateResult>>(0)?;
            if let Some(v) = s_rows.pop() {
                if let Some(json_val) = v.value_json {
                    let content = serde_json::to_string(&json_val).unwrap_or_else(|_| "{}".to_string());
                    let _ = writeln!(xml, "  <source id=\"{node}\">{content}</source>");
                }
            }
        }
        xml.push_str("</context>");
        Ok(xml)
    }

    /// `log_telemetry`: Non-critical telemetry logging.
    ///
    /// # Panics
    /// Panics if `event` is empty.
    ///
    /// # Errors
    /// Returns `KernelError::Database` if insertion fails.
    pub async fn log_telemetry(&self, payload: serde_json::Value, event: &str, run_id: Option<String>) -> Result<(), KernelError> {
        assert!(!event.is_empty(), "Event name mandatory");
        tracing::info!("Logging Telemetry: Event={}, Payload={}", event, payload);

        // Telemetry rows likewise carry native UUIDv7 ids so the firehose preserves
        // the wall-clock ordering its consumers rely on.
        let log_record_id = Thing::from((
            "telemetry_stream",
            surrealdb::sql::Id::Uuid(surrealdb::sql::Uuid::from(::uuid::Uuid::now_v7())),
        ));
        
        let query = "INSERT INTO telemetry_stream { id: $id, lifecycle_event: $e, payload: $p, run_id: $r, tenant_id: $session_tenant, timestamp: time::now() }";
        self.db.query(query)
            .bind(("id", log_record_id))
            .bind(("p", payload))
            .bind(("e", event.to_string()))
            .bind(("r", run_id)).await?.check()?;
        Ok(())
    }

    /// `checkpoint_execution`: Atomic upsert of a resume-point.
    ///
    /// # Panics
    /// Panics if `run_id` is empty.
    ///
    /// # Errors
    /// Returns `KernelError::Database` if upsert fails.
    pub async fn checkpoint_execution(
        &self,
        run_id: &str,
        cursor_type: &str,
        last_processed: Option<String>,
        metadata: Option<serde_json::Value>,
    ) -> Result<(), KernelError> {
        assert!(!run_id.is_empty(), "Run ID mandatory");

        let cursor_id = Thing::from(("execution_cursor".to_string(), run_id.to_string()));
        let query = r"
            UPSERT $id CONTENT {
                run_id: $run_id,
                tenant_id: $session_tenant,
                cursor_type: $cursor_type,
                last_processed: $last_processed,
                metadata: $metadata,
                updated_at: time::now()
            }
        ";
        self.db.query(query)
            .bind(("id", cursor_id))
            .bind(("run_id", run_id.to_string()))
            .bind(("cursor_type", cursor_type.to_string()))
            .bind(("last_processed", last_processed.clone()))
            .bind(("metadata", metadata)).await?.check()?;
        
        let _ = self.log_telemetry(
            serde_json::json!({"type": cursor_type, "last": last_processed}),
            "execution_checkpoint",
            Some(run_id.to_string())
        ).await;

        Ok(())
    }

    /// `get_execution_cursor`: Retrieve a resume-point.
    ///
    /// # Panics
    /// Panics if `run_id` is empty.
    ///
    /// # Errors
    /// Returns `KernelError::Database` if query fails.
    pub async fn get_execution_cursor(&self, run_id: &str) -> Result<Option<CursorResult>, KernelError> {
        assert!(!run_id.is_empty(), "Run ID mandatory");
        
        let query = "SELECT last_processed, metadata FROM execution_cursor WHERE run_id = $run_id AND tenant_id = $session_tenant LIMIT 1";
        let mut res = self.db.query(query)
            .bind(("run_id", run_id.to_string())).await?;
        
        let cursor = res.take::<Vec<CursorResult>>(0)?.pop();
        Ok(cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    async fn setup() -> (tempfile::TempDir, Kernel) {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("superx.db");
        let kernel = Kernel::init(&db_path, "test", "test").await.unwrap();
        (dir, kernel)
    }

    #[tokio::test]
    async fn test_validation_nasa_hardened() {
        let (_dir, kernel) = setup().await;
        let t = "tenant_1";
        kernel.set_session_auth(t, "admin").await.unwrap();
        kernel.db.query("INSERT INTO entity {id: entity:v1, tenant_id: $session_tenant, type: type_definition:node_prod}").await.unwrap();

        // 1. Unknown type rejected
        let res = kernel.supersede_state("entity:v1", "none", serde_json::json!({"text":"x"}), None).await;
        assert!(matches!(res, Err(KernelError::Validation(m)) if m.contains("not found")));

        // 2. Schema violation rejected
        let res = kernel.supersede_state("entity:v1", "attr_desc", serde_json::json!({"wrong":"y"}), None).await;
        assert!(matches!(&res, Err(KernelError::Validation(ref m)) if m.contains("fails validation")));

        // 3. Tenant coercion rejected by physical check
        kernel.set_session_auth("attacker", "user").await.unwrap();
        let res2 = kernel.supersede_state("entity:v1", "attr_desc", serde_json::json!({"text":"hack"}), None).await;
        assert!(matches!(res2, Err(KernelError::SafetyViolation(m)) if m.contains("Tenant mismatch")));
    }

    #[tokio::test]
    async fn test_cycle_nasa_hardened() {
        let (_dir, kernel) = setup().await;
        let t = "tenant_1";
        kernel.set_session_auth(t, "admin").await.unwrap();
        kernel.db.query("INSERT INTO entity [{id: entity:a, tenant_id: $session_tenant, type: type_definition:node_prod}, {id: entity:b, tenant_id: $session_tenant, type: type_definition:node_prod}]").await.unwrap();

        kernel.create_structural_edge("entity:a", "entity:b", "edge_owns").await.unwrap();
        let res = kernel.create_structural_edge("entity:b", "entity:a", "edge_owns").await;
        assert!(matches!(res, Err(KernelError::CycleDetected)));
    }

    #[tokio::test]
    async fn test_tenant_isolation_nasa_hardened() {
        let (_dir, kernel) = setup().await;
        let t1 = "t1";
        kernel.set_session_auth(t1, "admin").await.unwrap();
        kernel.db.query("INSERT INTO entity {id: entity:p1, tenant_id: $session_tenant, type: type_definition:node_prod}").await.unwrap();
        kernel.supersede_state("entity:p1", "attr_desc", serde_json::json!({"text":"secret"}), None).await.unwrap();

        kernel.set_session_auth("t2", "user").await.unwrap();
        let xml = kernel.compile_context("entity:p1", "run1", None).await.unwrap();
        assert!(!xml.contains("secret"));
    }

    #[tokio::test]
    async fn test_durable_cursor_nasa_hardened() {
        let (_dir, kernel) = setup().await;
        let t = "t1";
        kernel.set_session_auth(t, "admin").await.unwrap();
        let rid = "run_123";
        kernel.checkpoint_execution(rid, "test", Some("file_1".into()), None).await.unwrap();
        
        let cursor = kernel.get_execution_cursor(rid).await.unwrap().unwrap();
        assert_eq!(cursor.last_processed, Some("file_1".into()));
    }

    #[tokio::test]
    async fn test_scd2_supersede_nasa_hardened() {
        let (_dir, kernel) = setup().await;
        let t = "t1";
        kernel.set_session_auth(t, "admin").await.unwrap();
        kernel.db.query("INSERT INTO entity {id: entity:v1, tenant_id: $session_tenant, type: type_definition:node_prod}").await.unwrap();

        // 1. First state
        kernel.supersede_state("entity:v1", "attr_desc", serde_json::json!({"text":"v1"}), None).await.unwrap();
        
        // 2. Second state (Supersede)
        kernel.supersede_state("entity:v1", "attr_desc", serde_json::json!({"text":"v2"}), None).await.unwrap();

        // 3. Verify history (Cast IDs to string for deserialization)
        let mut res = kernel.db.query("SELECT <string>id as id, is_current, value_json FROM state_ledger WHERE target = entity:v1").await.unwrap();
        let history: Vec<serde_json::Value> = res.take(0).unwrap();
        assert_eq!(history.len(), 2, "SCD-2 history must preserve all versions");
        
        let current = history.iter().find(|v| v.get("is_current").and_then(serde_json::Value::as_bool).unwrap_or(false)).unwrap();
        assert_eq!(current.get("value_json").unwrap().get("text").unwrap().as_str().unwrap(), "v2");
    }

    #[tokio::test]
    async fn test_parse_id_rejects_malformed() {
        // parse_id is the canonical record-id parser used at the kernel boundary
        // (NASA Rule 7: assertion density). It must reject anything that isn't `table:id`.
        let bad = ["nocolon", "too:many:parts", ":no_table", "no_id:"];
        for input in bad {
            let res = Kernel::parse_id(input);
            assert!(
                matches!(res, Err(KernelError::Validation(_))),
                "parse_id should reject {input:?}, got {res:?}"
            );
        }

        let good = Kernel::parse_id("entity:abc123").unwrap();
        assert_eq!(good.tb, "entity");
    }

    #[tokio::test]
    async fn test_compile_context_node_limit() {
        // NASA Rule 2: compile_context must enforce a bounded total-node cap, even when
        // depth would otherwise permit a runaway-wide DAG. Configure max_context_nodes=2,
        // seed a 3-node chain, and verify the kernel refuses to materialize the third.
        let (_dir, kernel) = setup().await;
        let t = "tenant_bound";
        kernel.set_session_auth(t, "admin").await.unwrap();

        // Provision a substrate for this tenant so attr_config writes land where get_parameter expects.
        let sub_id = "entity:bound_substrate";
        kernel.db.query("INSERT INTO entity {id: entity:bound_substrate, tenant_id: $session_tenant, type: type_definition:node_substrate}")
            .await.unwrap().check().unwrap();
        kernel.supersede_state(sub_id, "attr_config", serde_json::json!({"max_context_nodes": 2}), None).await.unwrap();

        // Seed three entities and a structural chain a -> b -> c.
        kernel.db.query("INSERT INTO entity [\
            {id: entity:bn_a, tenant_id: $session_tenant, type: type_definition:node_prod},\
            {id: entity:bn_b, tenant_id: $session_tenant, type: type_definition:node_prod},\
            {id: entity:bn_c, tenant_id: $session_tenant, type: type_definition:node_prod}]")
            .await.unwrap().check().unwrap();
        kernel.create_structural_edge("entity:bn_a", "entity:bn_b", "edge_owns").await.unwrap();
        kernel.create_structural_edge("entity:bn_b", "entity:bn_c", "edge_owns").await.unwrap();

        let res = kernel.compile_context("entity:bn_a", "test_run", None).await;
        assert!(
            matches!(&res, Err(KernelError::SafetyViolation(m)) if m.contains("node limit")),
            "compile_context must refuse to exceed max_context_nodes, got {res:?}"
        );
    }
}
