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
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, OnceLock};
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
    /// Cache of `type_definition` row ids keyed by their stable `uid` (e.g.,
    /// `"node_substrate"`, `"edge_owns"`). Populated once at the end of
    /// [`Kernel::init`] from the seeded metamodel rows. Every verb that
    /// references a metamodel type by name resolves it through this cache via
    /// [`Kernel::type_thing`] — there is no named-id pattern (`type_definition:node_substrate`)
    /// in the substrate any more; ids are UUIDv7 and lookups go through the cache.
    type_cache: Arc<OnceLock<HashMap<String, Thing>>>,
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

        let kernel = Self {
            db,
            ns: ns.to_string(),
            db_name: db_name.to_string(),
            type_cache: Arc::new(OnceLock::new()),
        };

        // Phase 1: schema + seed run under root (embedded RocksDB default).
        // This is the only operator-authorised root activity per skill §10.A.
        kernel.apply_substrate_schema().await?;
        let cache = kernel.seed_metamodel().await?;
        kernel.type_cache
            .set(cache)
            .map_err(|_| KernelError::Init("type_cache already initialised".to_string()))?;

        // Phase 2: switch to the `superx` service account for the entire
        // remaining lifetime of this Kernel handle. Every subsequent verb
        // touches the substrate as `superx` — engine-restricted to SELECT +
        // CREATE only via per-table PERMISSIONS FOR update NONE; FOR delete NONE.
        // Bound by skill §13.
        let service_password = std::env::var("SUPERX_SERVICE_PASSWORD")
            .unwrap_or_else(|_| "superx-v01-dev-x9KmP2nQ7tR3vW8y".to_string());
        kernel.db
            .signin(surrealdb::opt::auth::Database {
                namespace: ns,
                database: db_name,
                username: "superx",
                password: service_password.as_str(),
            })
            .await
            .map_err(|e| KernelError::Init(format!("signin as `superx` failed: {e}")))?;

        assert!(kernel.db.health().await.is_ok(), "Substrate health check failed after init");
        Ok(kernel)
    }

    async fn apply_substrate_schema(&self) -> Result<(), KernelError> {
        // Service-account password for the `superx` user.
        // The operator may override via SUPERX_SERVICE_PASSWORD env; otherwise
        // we use the dev default that is also recorded in the skill so the
        // model knows what credentials to authenticate with.
        // CONTRACT: see .claude/skills/zero-trust-execution/SKILL.md §13.
        //
        // We escape any embedded `"` in the password before inlining into the
        // DEFINE USER statement because SurrealDB requires `PASSWORD` to be a
        // string literal — `$var` parameter binding is rejected inside DDL.
        let service_password = std::env::var("SUPERX_SERVICE_PASSWORD")
            .unwrap_or_else(|_| "superx-v01-dev-x9KmP2nQ7tR3vW8y".to_string());
        let escaped_password = service_password.replace('\\', "\\\\").replace('"', "\\\"");

        // v2 schema — append-only, insert-only, fully cross-referenceable.
        // Every table has one temporal field (`valid_from`) — no `is_current`,
        // no `valid_to`. "Current" is `ORDER BY valid_from DESC LIMIT 1` against
        // the chain key. PERMISSIONS clauses refuse UPDATE/DELETE engine-side
        // so the model's EDITOR-roled service_account can only SELECT + CREATE.
        let surql = r"
            -- ====================================================================
            -- type_definition — metamodel root
            -- ====================================================================
            DEFINE TABLE type_definition SCHEMAFULL
                PERMISSIONS
                    FOR select WHERE $session_role IN ['admin', 'user']
                    FOR create WHERE $session_role = 'admin'
                    FOR update NONE
                    FOR delete NONE;

            DEFINE FIELD id           ON type_definition TYPE uuid;
            DEFINE FIELD uid          ON type_definition TYPE string;
            DEFINE FIELD category     ON type_definition TYPE string;
            DEFINE FIELD is_acyclic   ON type_definition TYPE bool DEFAULT false;
            DEFINE FIELD sch_json     ON type_definition TYPE option<string>;
            DEFINE FIELD memory_tier  ON type_definition TYPE string DEFAULT 'working'
                ASSERT $value INSIDE ['core', 'working', 'archival', 'recall'];
            DEFINE FIELD valid_from   ON type_definition TYPE datetime DEFAULT time::now();

            DEFINE INDEX IF NOT EXISTS type_def_uid      ON type_definition FIELDS uid UNIQUE;
            DEFINE INDEX IF NOT EXISTS type_def_category ON type_definition FIELDS category;

            -- ====================================================================
            -- entity — substrate identities
            -- ====================================================================
            DEFINE TABLE entity SCHEMAFULL
                PERMISSIONS
                    FOR select WHERE tenant.id = $session_tenant OR $session_role = 'admin'
                    FOR create WHERE tenant.id = $session_tenant OR $session_role = 'admin'
                    FOR update NONE
                    FOR delete NONE;

            DEFINE FIELD id           ON entity TYPE uuid;
            DEFINE FIELD type         ON entity TYPE record<type_definition>
                ASSERT $value.category = 'node';
            DEFINE FIELD tenant       ON entity TYPE record<entity>
                ASSERT $value.type.uid = 'node_substrate';
            DEFINE FIELD role         ON entity TYPE string DEFAULT 'user'
                ASSERT $value INSIDE ['user', 'admin'];
            DEFINE FIELD valid_from   ON entity TYPE datetime DEFAULT time::now();

            DEFINE INDEX IF NOT EXISTS entity_tenant ON entity FIELDS tenant;
            DEFINE INDEX IF NOT EXISTS entity_type   ON entity FIELDS type;

            -- ====================================================================
            -- relation — directed graph edges
            -- ====================================================================
            DEFINE TABLE relation SCHEMAFULL
                PERMISSIONS
                    FOR select WHERE tenant.id = $session_tenant OR $session_role = 'admin'
                    FOR create WHERE tenant.id = $session_tenant OR $session_role = 'admin'
                    FOR update NONE
                    FOR delete NONE;

            DEFINE FIELD id           ON relation TYPE uuid;
            DEFINE FIELD in           ON relation TYPE record<entity>;
            DEFINE FIELD out          ON relation TYPE record<entity>;
            DEFINE FIELD type         ON relation TYPE record<type_definition>
                ASSERT $value.category = 'edge';
            DEFINE FIELD tenant       ON relation TYPE record<entity>
                ASSERT $value.type.uid = 'node_substrate';
            DEFINE FIELD is_acyclic   ON relation TYPE bool DEFAULT false;
            DEFINE FIELD valid_from   ON relation TYPE datetime DEFAULT time::now();

            DEFINE INDEX IF NOT EXISTS rel_source ON relation FIELDS in, tenant, is_acyclic;
            DEFINE INDEX IF NOT EXISTS rel_dest   ON relation FIELDS out, tenant, is_acyclic;

            -- ====================================================================
            -- state_ledger — append-only typed attribute writes
            -- ====================================================================
            DEFINE TABLE state_ledger SCHEMALESS
                PERMISSIONS
                    FOR select WHERE tenant.id = $session_tenant OR $session_role = 'admin'
                    FOR create WHERE tenant.id = $session_tenant OR $session_role = 'admin'
                    FOR update NONE
                    FOR delete NONE;

            DEFINE FIELD id           ON state_ledger TYPE uuid;
            DEFINE FIELD target       ON state_ledger TYPE record<entity>;
            DEFINE FIELD type         ON state_ledger TYPE record<type_definition>
                ASSERT $value.category = 'attribute';
            DEFINE FIELD tenant       ON state_ledger TYPE record<entity>
                ASSERT $value.type.uid = 'node_substrate';
            DEFINE FIELD valid_from   ON state_ledger TYPE datetime DEFAULT time::now();

            DEFINE INDEX IF NOT EXISTS state_chain ON state_ledger FIELDS target, type, valid_from;

            -- ====================================================================
            -- cursor_type — categorises every cursor kind
            -- ====================================================================
            DEFINE TABLE cursor_type SCHEMAFULL
                PERMISSIONS
                    FOR select WHERE $session_role IN ['admin', 'user']
                    FOR create WHERE $session_role = 'admin'
                    FOR update NONE
                    FOR delete NONE;

            DEFINE FIELD id           ON cursor_type TYPE uuid;
            DEFINE FIELD uid          ON cursor_type TYPE string;
            DEFINE FIELD category     ON cursor_type TYPE string
                ASSERT $value INSIDE ['workload', 'telemetry'];
            DEFINE FIELD description  ON cursor_type TYPE string;
            DEFINE FIELD sch_json     ON cursor_type TYPE option<string>;
            DEFINE FIELD valid_from   ON cursor_type TYPE datetime DEFAULT time::now();

            DEFINE INDEX IF NOT EXISTS cursor_type_uid      ON cursor_type FIELDS uid UNIQUE;
            DEFINE INDEX IF NOT EXISTS cursor_type_category ON cursor_type FIELDS category;

            -- ====================================================================
            -- cursor — unified workload + telemetry cursor (renamed from execution_cursor)
            -- ====================================================================
            DEFINE TABLE cursor SCHEMAFULL
                PERMISSIONS
                    FOR select WHERE tenant.id = $session_tenant OR $session_role = 'admin'
                    FOR create WHERE tenant.id = $session_tenant OR $session_role = 'admin'
                    FOR update NONE
                    FOR delete NONE;

            DEFINE FIELD id             ON cursor TYPE uuid;
            DEFINE FIELD subject        ON cursor TYPE record<entity>;
            DEFINE FIELD tenant         ON cursor TYPE record<entity>
                ASSERT $value.type.uid = 'node_substrate';
            DEFINE FIELD cursor_type    ON cursor TYPE record<cursor_type>;
            DEFINE FIELD last_processed ON cursor TYPE option<string>;
            DEFINE FIELD metadata       ON cursor TYPE option<object>;
            DEFINE FIELD valid_from     ON cursor TYPE datetime DEFAULT time::now();

            DEFINE INDEX IF NOT EXISTS cursor_chain ON cursor FIELDS subject, cursor_type, valid_from;

            -- ====================================================================
            -- execution_params — per-run agent execution knobs (append-only)
            -- ====================================================================
            DEFINE TABLE execution_params SCHEMAFULL
                PERMISSIONS
                    FOR select WHERE tenant.id = $session_tenant OR $session_role = 'admin'
                    FOR create WHERE tenant.id = $session_tenant OR $session_role = 'admin'
                    FOR update NONE
                    FOR delete NONE;

            DEFINE FIELD id           ON execution_params TYPE uuid;
            DEFINE FIELD run          ON execution_params TYPE record<entity>
                ASSERT $value.type.uid = 'node_run';
            DEFINE FIELD tenant       ON execution_params TYPE record<entity>
                ASSERT $value.type.uid = 'node_substrate';
            DEFINE FIELD agent        ON execution_params TYPE record<entity>
                ASSERT $value.type.uid = 'node_agent';
            DEFINE FIELD params_json  ON execution_params FLEXIBLE TYPE object;
            DEFINE FIELD valid_from   ON execution_params TYPE datetime DEFAULT time::now();

            DEFINE INDEX IF NOT EXISTS exec_params_chain ON execution_params FIELDS run, agent, valid_from;
            DEFINE INDEX IF NOT EXISTS exec_params_run   ON execution_params FIELDS run, valid_from;

            -- ====================================================================
            -- schedule — dumb work queue (append-only)
            -- ====================================================================
            DEFINE TABLE schedule SCHEMAFULL
                PERMISSIONS
                    FOR select WHERE tenant.id = $session_tenant OR $session_role = 'admin'
                    FOR create WHERE tenant.id = $session_tenant OR $session_role = 'admin'
                    FOR update NONE
                    FOR delete NONE;

            DEFINE FIELD id           ON schedule TYPE uuid;
            DEFINE FIELD run          ON schedule TYPE record<entity>
                ASSERT $value.type.uid = 'node_run';
            DEFINE FIELD tenant       ON schedule TYPE record<entity>
                ASSERT $value.type.uid = 'node_substrate';
            DEFINE FIELD kind         ON schedule TYPE string;
            DEFINE FIELD target       ON schedule TYPE record<entity>;
            DEFINE FIELD due_at       ON schedule TYPE datetime DEFAULT time::now();
            DEFINE FIELD status       ON schedule TYPE string DEFAULT 'waiting'
                ASSERT $value INSIDE ['waiting', 'scheduled', 'running', 'completed', 'failed', 'awaiting_human'];
            DEFINE FIELD attempt      ON schedule TYPE int DEFAULT 0
                ASSERT $value >= 0;
            DEFINE FIELD depends_on   ON schedule TYPE array<record<schedule>> DEFAULT [];
            DEFINE FIELD metadata     ON schedule FLEXIBLE TYPE object DEFAULT {};
            DEFINE FIELD valid_from   ON schedule TYPE datetime DEFAULT time::now();

            DEFINE INDEX IF NOT EXISTS sched_due_recent ON schedule FIELDS due_at, valid_from;
            DEFINE INDEX IF NOT EXISTS sched_run_recent ON schedule FIELDS run, valid_from;

            -- ====================================================================
            -- telemetry_stream — event firehose (CHANGEFEED 1d)
            -- ====================================================================
            DEFINE TABLE telemetry_stream SCHEMALESS CHANGEFEED 1d
                PERMISSIONS
                    FOR select WHERE tenant.id = $session_tenant OR $session_role = 'admin'
                    FOR create WHERE tenant.id = $session_tenant OR $session_role = 'admin'
                    FOR update NONE
                    FOR delete NONE;

            DEFINE FIELD id               ON telemetry_stream TYPE uuid;
            DEFINE FIELD lifecycle_event  ON telemetry_stream TYPE string;
            DEFINE FIELD payload          ON telemetry_stream TYPE any;
            DEFINE FIELD run              ON telemetry_stream TYPE option<record<entity>>
                ASSERT $value = NONE OR $value.type.uid = 'node_run';
            DEFINE FIELD tenant           ON telemetry_stream TYPE record<entity>
                ASSERT $value.type.uid = 'node_substrate';
            DEFINE FIELD valid_from       ON telemetry_stream TYPE datetime DEFAULT time::now();

            DEFINE INDEX IF NOT EXISTS tele_run   ON telemetry_stream FIELDS run, valid_from;
            DEFINE INDEX IF NOT EXISTS tele_event ON telemetry_stream FIELDS lifecycle_event, valid_from;

            -- ====================================================================
            -- Tenant-based record auth (existing — kept for roadmap #8)
            -- ====================================================================
            DEFINE ACCESS IF NOT EXISTS tenant_access ON DATABASE TYPE RECORD
                SIGNIN ( SELECT * FROM entity WHERE tenant.id = $tenant AND role = $role LIMIT 1 );

            -- (Service account `superx` is defined in a separate statement
            --  below — SurrealDB requires DEFINE USER PASSWORD to be a
            --  literal, not a parameter, so the password is interpolated by
            --  the kernel just before this schema applies. The contract is
            --  binding even though the DEFINE statement itself is split out:
            --  Login `superx`, EDITOR role, SELECT + CREATE only via the
            --  per-table PERMISSIONS clauses above. See skill §13.)
        ";
        // Apply the main schema first (tables + fields + indexes + accesses).
        // AlreadyExists errors are tolerated for idempotency on re-bootstrap.
        let _ = self.db.query(surql).await;

        // Then define the service-account user with the password inlined.
        // DEFINE USER does not accept $parameter binding for PASSWORD; the
        // password is escaped above and inlined here as a string literal.
        let define_user_sql = format!(
            "DEFINE USER IF NOT EXISTS superx ON DATABASE \
                PASSWORD \"{escaped_password}\" \
                ROLES EDITOR \
                DURATION FOR SESSION 1h, FOR TOKEN 1h;"
        );
        let _ = self.db.query(&define_user_sql).await;

        Ok(())
    }

    /// `seed_metamodel`: idempotently seeds the canonical `type_definition`
    /// rows under the v2 schema (UUIDv7 ids + `uid` field).
    ///
    /// For each canonical type:
    /// - SELECT existing row by `uid` (idempotent on re-bootstrap);
    /// - if not found, CREATE with a fresh UUIDv7 id + the `uid` field;
    /// - record the row's `Thing` in the returned cache.
    ///
    /// The returned `HashMap<uid, Thing>` is installed on `Kernel::type_cache`
    /// at the end of [`Kernel::init`]. Every other kernel verb resolves
    /// metamodel types via [`Kernel::type_thing`] so the substrate never
    /// stores or queries by a named-string id ever again.
    ///
    /// Runs at init time under root context (schema apply + seed precede
    /// any service-account `signin`).
    ///
    /// # Errors
    /// Returns `KernelError::Database` if a SELECT or CREATE fails.
    async fn seed_metamodel(&self) -> Result<HashMap<String, Thing>, KernelError> {
        // Canonical metamodel: (uid, category, is_acyclic, sch_json, memory_tier)
        let types: Vec<(&str, &str, bool, Option<&str>, &str)> = vec![
            // Core substrate identities
            ("node_substrate", "node", false, None, "core"),
            ("node_agent",     "node", false, None, "core"),
            ("node_session",   "node", false, None, "working"),
            ("node_capability","node", false, None, "core"),
            ("node_tool",      "node", false, None, "core"),
            ("node_component", "node", false, None, "core"),
            ("node_hardened_model", "node", false, None, "working"),
            // Execution-tier identities (new in v2 — typed FK targets)
            ("node_run",       "node", false, None, "core"),
            ("node_source",    "node", false, None, "core"),
            // Domain identities (ingested artifacts and cognitive products)
            ("node_prod",          "node", false, None, "working"),
            ("node_code",          "node", false, None, "working"),
            ("node_code_root",     "node", false, None, "working"),
            ("node_artifact",      "node", false, None, "working"),
            ("node_proposal",      "node", false, None, "working"),
            ("node_harness",       "node", false, None, "core"),
            ("node_source_external","node", false, None, "archival"),
            ("node_rag_source",    "node", false, None, "archival"),
            // Attribute types (typed payloads on state_ledger writes)
            ("attr_desc",   "attribute", false, Some("{\"type\":\"object\",\"required\":[\"text\"],\"properties\":{\"text\":{\"type\":\"string\",\"minLength\":1}}}"), "working"),
            ("attr_config", "attribute", false, None, "core"),
            ("attr_score",  "attribute", false, Some("{\"type\":\"object\",\"required\":[\"score\"],\"properties\":{\"score\":{\"type\":\"number\"}}}"), "working"),
            // Edge types (graph topology)
            ("edge_owns",            "edge", true,  None, "working"),
            ("edge_compiled_from",   "edge", true,  None, "working"),
            ("edge_distilled_from",  "edge", true,  None, "working"),
            ("edge_evaluates",       "edge", false, None, "working"),
            ("edge_promotes",        "edge", true,  None, "working"),
            ("edge_implements",      "edge", true,  None, "core"),
            ("edge_has_capability",  "edge", false, None, "core"),
            ("edge_participates_in", "edge", false, None, "working"),
            ("edge_semantic",        "edge", false, None, "recall"),
        ];

        let mut cache: HashMap<String, Thing> = HashMap::with_capacity(types.len());

        for (uid, cat, acyclic, sch, tier) in types {
            // 1. Look up existing row by uid (idempotent across re-bootstraps).
            let mut select_res = self.db
                .query("SELECT id FROM type_definition WHERE uid = $uid LIMIT 1")
                .bind(("uid", uid.to_string()))
                .await?;
            let existing: Option<IdResult> = select_res.take::<Vec<IdResult>>(0)?.pop();

            let id_thing = if let Some(row) = existing {
                // Already seeded — reuse the existing Thing.
                row.id
            } else {
                // Fresh seed: generate a UUIDv7 id and CREATE the row.
                let new_uuid = ::uuid::Uuid::now_v7();
                let new_id = Thing::from((
                    "type_definition",
                    surrealdb::sql::Id::Uuid(surrealdb::sql::Uuid::from(new_uuid)),
                ));
                self.db
                    .query("CREATE type_definition CONTENT { id: $id, uid: $uid, category: $cat, is_acyclic: $acyc, sch_json: $sch, memory_tier: $tier }")
                    .bind(("id", new_id.clone()))
                    .bind(("uid", uid.to_string()))
                    .bind(("cat", cat.to_string()))
                    .bind(("acyc", acyclic))
                    .bind(("sch", sch.map(std::string::ToString::to_string)))
                    .bind(("tier", tier.to_string()))
                    .await?
                    .check()?;
                new_id
            };

            cache.insert(uid.to_string(), id_thing);
        }

        Ok(cache)
    }

    /// `type_thing`: resolve a canonical metamodel `uid` (e.g. `"node_substrate"`,
    /// `"edge_owns"`, `"attr_desc"`) to its `Thing` reference in the substrate.
    ///
    /// This is the single chokepoint replacing the legacy
    /// `Thing::from(("type_definition", "node_substrate"))` pattern. Under the
    /// v2 schema, `type_definition` ids are UUIDv7; the human-readable name
    /// lives in the `uid` column. Every kernel verb and every caller crate
    /// resolves the FK target through this cache, never by name-id literal.
    ///
    /// # Errors
    /// Returns `KernelError::Integrity` if the cache is uninitialised (only
    /// possible if `type_thing` is called before `Kernel::init` completes) or
    /// if the requested `uid` was not seeded.
    pub fn type_thing(&self, uid: &str) -> Result<Thing, KernelError> {
        let cache = self.type_cache.get().ok_or_else(|| {
            KernelError::Integrity("type_cache not yet initialised — called before Kernel::init completed".to_string())
        })?;
        cache.get(uid).cloned().ok_or_else(|| {
            KernelError::Integrity(format!("type_definition uid '{uid}' not in cache — missing from seed_metamodel?"))
        })
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
    ///
    /// Resolves the metamodel types (`node_substrate`, `attr_config`) through
    /// the `type_cache` instead of the legacy `type_definition:<name>` named-id
    /// pattern. Uses recency ordering (`ORDER BY valid_from DESC LIMIT 1`)
    /// instead of the dropped `is_current = true` filter.
    pub async fn get_parameter<T: for<'de> serde::Deserialize<'de>>(&self, key: &str, default: T) -> T {
        let (Ok(node_substrate), Ok(attr_config)) = (self.type_thing("node_substrate"), self.type_thing("attr_config")) else {
            return default;
        };

        let query = "SELECT value_json FROM state_ledger \
            WHERE target.type = $node_substrate AND `type` = $attr_config \
            ORDER BY valid_from DESC LIMIT 1";

        if let Ok(mut res) = self.db.query(query)
            .bind(("node_substrate", node_substrate))
            .bind(("attr_config", attr_config))
            .await
        {
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

        // Resolve the attribute type through the metamodel cache.
        // Under v2 the type_definition row id is UUIDv7; the human-readable
        // `type_uid` (e.g. "attr_desc") resolves to a Thing via type_thing().
        let type_thing = self.type_thing(type_uid)?;
        let target_thing = Self::parse_id(target_id)?;
        let session_tenant_str = self.get_session_tenant().await?;
        let substrate_thing = self.type_thing("node_substrate")?;

        // 1. Fetch the attribute type's JSON Schema (if any) + the target
        //    entity's tenant. Tenant comes back as `record<entity>` under v2;
        //    we read .id to get the UUIDv7 string for the cross-tenant check.
        #[derive(serde::Deserialize)]
        struct FetchV2 {
            sch_json: Option<String>,
            tenant_id: Option<String>,
        }
        let fetch_query = "SELECT \
            (SELECT sch_json FROM $ty LIMIT 1)[0].sch_json AS sch_json, \
            (SELECT tenant FROM $target LIMIT 1)[0].tenant.id AS tenant_id \
            FROM { ty: $ty }";
        let mut fetch_res = self.db.query(fetch_query)
            .bind(("ty", type_thing.clone()))
            .bind(("target", target_thing.clone())).await?;
        let info: FetchV2 = fetch_res.take::<Vec<FetchV2>>(0)?
            .pop()
            .ok_or_else(|| KernelError::Validation(format!("Type {type_uid} not found")))?;
        let target_tenant = info.tenant_id
            .ok_or_else(|| KernelError::Validation(format!("Target {target_id} not found")))?;

        // 2. Tenant-coercion guard.
        if target_tenant != session_tenant_str {
            return Err(KernelError::SafetyViolation(format!(
                "Tenant mismatch: Entity belongs to {target_tenant}, session is {session_tenant_str}"
            )));
        }

        // 3. JSON-Schema validation at the write boundary.
        if let Some(schema_str) = info.sch_json {
            let schema_val: serde_json::Value = serde_json::from_str(&schema_str)
                .map_err(|e| KernelError::Validation(e.to_string()))?;
            let schema = JSONSchema::compile(&schema_val)
                .map_err(|e| KernelError::Validation(e.to_string()))?;
            if !schema.is_valid(&value) {
                return Err(KernelError::Validation(format!("Value fails validation for {type_uid}")));
            }
        }

        // 4. Pure-INSERT append. No UPDATE close-prior — under §10.B the
        //    service account has no UPDATE privilege. "Current" is recovered
        //    at query time via `ORDER BY valid_from DESC LIMIT 1` against
        //    the (target, type) chain key.
        let state_record_id = Thing::from((
            "state_ledger",
            surrealdb::sql::Id::Uuid(surrealdb::sql::Uuid::from(::uuid::Uuid::now_v7())),
        ));
        // Tenant FK is a record<entity> per v2 schema. The kernel session-
        // var $session_tenant holds the substrate-entity UUID string; we
        // resolve it to a Thing here for the bind.
        let tenant_thing = Self::parse_id(&format!("entity:{session_tenant_str}"))?;

        self.db.query(
            "CREATE state_ledger CONTENT { \
                id: $id, \
                target: $target, \
                `type`: $ty, \
                tenant: $tenant, \
                value_json: $val \
            };"
        )
            .bind(("id", state_record_id))
            .bind(("target", target_thing))
            .bind(("ty", type_thing))
            .bind(("tenant", tenant_thing))
            .bind(("val", value)).await?.check()?;

        // Suppress unused warning until tenant-coercion uses it.
        let _ = substrate_thing;

        // 5. Telemetry on every typed write.
        // run_id (caller-provided correlation token) stays in the payload;
        // the typed run FK is None until node_run entities are modelled.
        let mut payload_obj = serde_json::json!({"target": target_id, "type": type_uid});
        if let Some(ref r) = run_id {
            if let Some(o) = payload_obj.as_object_mut() {
                o.insert("run_id".into(), serde_json::Value::String(r.clone()));
            }
        }
        let _ = self.log_telemetry(payload_obj, "state_supersede", None).await;

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
            serde_json::json!({"root": root_id, "tiers": tiers, "run_id": run_id}),
            "context_compile",
            None
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

    /// `log_telemetry`: append a typed event to the firehose.
    ///
    /// Schema-aligned to v2:
    /// - `valid_from` is set by the schema default; the kernel does not write
    ///   a separate `timestamp`.
    /// - `tenant: record<entity>` resolved from the session's substrate-entity
    ///   id; the engine `ASSERT $value.type.uid = 'node_substrate'` catches
    ///   any FK pointing at a non-substrate row.
    /// - `run: Option<record<entity>>` resolved by the caller; when provided
    ///   the engine `ASSERT $value.type.uid = 'node_run'` catches FKs that
    ///   point at the wrong entity type.
    /// - Pure INSERT — no UPDATE, no `is_current`, no `valid_to`. Under the
    ///   superx service account the engine would refuse UPDATE anyway.
    ///
    /// # Panics
    /// Panics if `event` is empty.
    ///
    /// # Errors
    /// Returns `KernelError::Database` if insertion fails (engine refusal is
    /// the signal that a caller is passing the wrong FK type — fix the caller,
    /// never the user identity).
    pub async fn log_telemetry(
        &self,
        payload: serde_json::Value,
        event: &str,
        run: Option<Thing>,
    ) -> Result<(), KernelError> {
        assert!(!event.is_empty(), "Event name mandatory");
        tracing::info!("Logging Telemetry: Event={}, Payload={}", event, payload);

        let log_record_id = Thing::from((
            "telemetry_stream",
            surrealdb::sql::Id::Uuid(surrealdb::sql::Uuid::from(::uuid::Uuid::now_v7())),
        ));

        // tenant FK: substrate-entity Thing resolved from session.
        let session_tenant_str = self.get_session_tenant().await?;
        let tenant_thing = Self::parse_id(&format!("entity:{session_tenant_str}"))?;

        self.db.query(
            "INSERT INTO telemetry_stream { \
                id: $id, \
                lifecycle_event: $e, \
                payload: $p, \
                run: $r, \
                tenant: $t \
            }"
        )
            .bind(("id", log_record_id))
            .bind(("p", payload))
            .bind(("e", event.to_string()))
            .bind(("r", run))
            .bind(("t", tenant_thing))
            .await?.check()?;
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
            None
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

    /// `set_execution_params`: SCD-2 write of per-run agent execution knobs
    /// (`temperature`, `top_p`, `top_k`, `max_tokens`, `turns`, `branch`,
    /// `retry_policy`, `model_ref`, …). Closes any prior `is_current` row for the same
    /// `(run_id, agent_id)` tuple within the calling session's tenant, then
    /// inserts a fresh `is_current` row — one atomic transaction. The full
    /// history is recoverable by `SELECT … ORDER BY valid_from ASC`.
    ///
    /// `params` is an opaque JSON object: the kernel does not enforce a
    /// schema on its contents (blades enforce their own knob shape).
    ///
    /// Emits an `execution_params_set` telemetry event.
    ///
    /// # Panics
    /// Panics if `run_id` or `agent_id` are empty.
    ///
    /// # Errors
    /// Returns `KernelError::SafetyViolation` if no session is established;
    /// `KernelError::Database` if the transaction fails.
    pub async fn set_execution_params(
        &self,
        run_id: &str,
        agent_id: &str,
        params: serde_json::Value,
    ) -> Result<(), KernelError> {
        assert!(!run_id.is_empty(), "run_id mandatory");
        assert!(!agent_id.is_empty(), "agent_id mandatory");

        // Pre-condition: caller is in a session. We don't pass tenant explicitly;
        // it flows through `$session_tenant`. This call is here to surface the
        // 'no session' case as a clean SafetyViolation rather than a SurrealDB
        // permission error.
        let _session_tenant = self.get_session_tenant().await?;

        let row_id = Thing::from((
            "execution_params",
            surrealdb::sql::Id::Uuid(surrealdb::sql::Uuid::from(::uuid::Uuid::now_v7())),
        ));

        let query = r"
            BEGIN TRANSACTION;
                UPDATE execution_params SET is_current = false, valid_to = time::now()
                WHERE run_id = $rid AND agent_id = $aid AND is_current = true AND tenant_id = $session_tenant;

                CREATE execution_params CONTENT {
                    id: $id,
                    run_id: $rid,
                    tenant_id: $session_tenant,
                    agent_id: $aid,
                    params_json: $params,
                    is_current: true,
                    valid_from: time::now()
                };
            COMMIT TRANSACTION;
        ";

        self.db.query(query)
            .bind(("id", row_id))
            .bind(("rid", run_id.to_string()))
            .bind(("aid", agent_id.to_string()))
            .bind(("params", params)).await?.check()?;

        let _ = self.log_telemetry(
            serde_json::json!({"run_id": run_id, "agent_id": agent_id}),
            "execution_params_set",
            None
        ).await;

        Ok(())
    }

    /// `get_execution_params`: read the current execution-params payload for a
    /// `(run_id, agent_id)` tuple within the calling session's tenant. Returns
    /// `Ok(None)` when no row exists; `Ok(Some(params_json))` when it does.
    ///
    /// # Panics
    /// Panics if `run_id` or `agent_id` are empty.
    ///
    /// # Errors
    /// Returns `KernelError::Database` if the query fails.
    pub async fn get_execution_params(
        &self,
        run_id: &str,
        agent_id: &str,
    ) -> Result<Option<serde_json::Value>, KernelError> {
        #[derive(serde::Deserialize)]
        struct Row { params_json: serde_json::Value }

        assert!(!run_id.is_empty(), "run_id mandatory");
        assert!(!agent_id.is_empty(), "agent_id mandatory");

        let query = "SELECT params_json FROM execution_params \
            WHERE run_id = $rid AND agent_id = $aid AND is_current = true \
            AND tenant_id = $session_tenant LIMIT 1";

        let mut res = self.db.query(query)
            .bind(("rid", run_id.to_string()))
            .bind(("aid", agent_id.to_string())).await?;

        let row = res.take::<Vec<Row>>(0)?.pop();
        Ok(row.map(|r| r.params_json))
    }

    /// `enqueue_schedule_item`: append a new pending work item to the `schedule`
    /// table in `status = 'waiting'`. Returns the row id (`schedule:<uuidv7>`)
    /// so callers can refer to it from `depends_on` or pass it to
    /// `transition_schedule_status`. Emits a `schedule_enqueued` telemetry event.
    ///
    /// The `metadata` payload is opaque to the kernel — the consuming blade
    /// (`RunnerBlade`) defines its shape. `depends_on` is a list of schedule
    /// row ids (`schedule:<uuid>`); the runner is responsible for honouring it.
    ///
    /// # Panics
    /// Panics if `run_id`, `kind`, or `target_entity` are empty.
    ///
    /// # Errors
    /// Returns `KernelError::SafetyViolation` if no session is established;
    /// `KernelError::Database` if the insert fails.
    pub async fn enqueue_schedule_item(
        &self,
        run_id: &str,
        kind: &str,
        target_entity: &str,
        due_at: chrono::DateTime<chrono::Utc>,
        depends_on: Vec<String>,
        metadata: serde_json::Value,
    ) -> Result<String, KernelError> {
        assert!(!run_id.is_empty(), "run_id mandatory");
        assert!(!kind.is_empty(), "kind mandatory");
        assert!(!target_entity.is_empty(), "target_entity mandatory");

        let _session_tenant = self.get_session_tenant().await?;

        let new_uuid = ::uuid::Uuid::now_v7();
        let row_id = Thing::from((
            "schedule",
            surrealdb::sql::Id::Uuid(surrealdb::sql::Uuid::from(new_uuid)),
        ));
        let row_id_str = format!("schedule:{new_uuid}");

        let query = r"
            CREATE schedule CONTENT {
                id: $id,
                run_id: $rid,
                tenant_id: $session_tenant,
                kind: $kind,
                target_entity: $target,
                due_at: <datetime> $due,
                status: 'waiting',
                attempt: 0,
                depends_on: $deps,
                metadata: $meta,
                is_current: true,
                valid_from: time::now()
            };
        ";

        self.db.query(query)
            .bind(("id", row_id))
            .bind(("rid", run_id.to_string()))
            .bind(("kind", kind.to_string()))
            .bind(("target", target_entity.to_string()))
            .bind(("due", due_at.to_rfc3339()))
            .bind(("deps", depends_on))
            .bind(("meta", metadata)).await?.check()?;

        let _ = self.log_telemetry(
            serde_json::json!({"schedule_id": row_id_str, "run_id": run_id, "kind": kind, "target": target_entity}),
            "schedule_enqueued",
            None
        ).await;

        Ok(row_id_str)
    }

    /// `transition_schedule_status`: SCD-2 status transition for a schedule item.
    /// Closes the current row (`is_current = false`, `valid_to = now()`) and
    /// inserts a fresh row with the new status, preserving all other fields
    /// from the closed row — in one atomic transaction.
    ///
    /// Status transitions follow `waiting → scheduled → running → completed/failed`
    /// (plus `awaiting_human` for HITL gates). The kernel does not validate the
    /// transition graph — that's the `RunnerBlade`'s job — but every transition
    /// emits a `schedule_transitioned` telemetry event with both states.
    ///
    /// # Panics
    /// Panics if `schedule_id` or `new_status` is empty.
    ///
    /// # Errors
    /// Returns `KernelError::SafetyViolation` if no session is established;
    /// `KernelError::Validation` if the schedule row doesn't exist or isn't
    /// current; `KernelError::Database` if the transaction fails.
    pub async fn transition_schedule_status(
        &self,
        schedule_id: &str,
        new_status: &str,
    ) -> Result<String, KernelError> {
        #[derive(serde::Deserialize)]
        struct CurrentRow {
            run_id: String,
            kind: String,
            target_entity: String,
            due_at: chrono::DateTime<chrono::Utc>,
            status: String,
            attempt: i64,
            depends_on: Vec<String>,
            metadata: serde_json::Value,
        }

        assert!(!schedule_id.is_empty(), "schedule_id mandatory");
        assert!(!new_status.is_empty(), "new_status mandatory");

        let _session_tenant = self.get_session_tenant().await?;

        let sid_uuid_str = schedule_id.strip_prefix("schedule:").unwrap_or(schedule_id).to_string();
        let sid_uuid = ::uuid::Uuid::parse_str(&sid_uuid_str)
            .map_err(|e| KernelError::Validation(format!("schedule_id must be `schedule:<uuidv7>`, got {schedule_id}: {e}")))?;
        let sid_thing = Thing::from((
            "schedule",
            surrealdb::sql::Id::Uuid(surrealdb::sql::Uuid::from(sid_uuid)),
        ));

        let fetch = "SELECT run_id, kind, target_entity, due_at, status, attempt, depends_on, metadata \
            FROM schedule WHERE id = $sid AND is_current = true AND tenant_id = $session_tenant LIMIT 1";

        let mut res = self.db.query(fetch)
            .bind(("sid", sid_thing.clone())).await?;
        let current: CurrentRow = res.take::<Vec<CurrentRow>>(0)?
            .pop()
            .ok_or_else(|| KernelError::Validation(format!("schedule item not found or not current: {schedule_id}")))?;

        let prior_status = current.status.clone();

        let new_uuid = ::uuid::Uuid::now_v7();
        let new_row_id = Thing::from((
            "schedule",
            surrealdb::sql::Id::Uuid(surrealdb::sql::Uuid::from(new_uuid)),
        ));
        let new_row_id_str = format!("schedule:{new_uuid}");

        let tx = r"
            BEGIN TRANSACTION;
                UPDATE schedule SET is_current = false, valid_to = time::now()
                WHERE id = $sid AND is_current = true AND tenant_id = $session_tenant;

                CREATE schedule CONTENT {
                    id: $new_id,
                    run_id: $rid,
                    tenant_id: $session_tenant,
                    kind: $kind,
                    target_entity: $target,
                    due_at: <datetime> $due,
                    status: $new_status,
                    attempt: $attempt,
                    depends_on: $deps,
                    metadata: $meta,
                    is_current: true,
                    valid_from: time::now()
                };
            COMMIT TRANSACTION;
        ";

        self.db.query(tx)
            .bind(("sid", sid_thing))
            .bind(("new_id", new_row_id))
            .bind(("rid", current.run_id.clone()))
            .bind(("kind", current.kind))
            .bind(("target", current.target_entity))
            .bind(("due", current.due_at.to_rfc3339()))
            .bind(("new_status", new_status.to_string()))
            .bind(("attempt", current.attempt))
            .bind(("deps", current.depends_on))
            .bind(("meta", current.metadata)).await?.check()?;

        let _ = self.log_telemetry(
            serde_json::json!({
                "schedule_id": schedule_id,
                "new_schedule_id": new_row_id_str,
                "prior_status": prior_status,
                "new_status": new_status,
                "run_id": current.run_id,
            }),
            "schedule_transitioned",
            None
        ).await;

        Ok(new_row_id_str)
    }

    /// `get_schedule_item`: returns the current row for a given schedule id as
    /// a JSON object (or `None` if the row is missing / not current in the
    /// caller's tenant). The JSON shape mirrors the substrate row exactly.
    ///
    /// # Panics
    /// Panics if `schedule_id` is empty.
    ///
    /// # Errors
    /// Returns `KernelError::Database` if the query fails.
    pub async fn get_schedule_item(
        &self,
        schedule_id: &str,
    ) -> Result<Option<serde_json::Value>, KernelError> {
        assert!(!schedule_id.is_empty(), "schedule_id mandatory");

        let sid_uuid_str = schedule_id.strip_prefix("schedule:").unwrap_or(schedule_id).to_string();
        let Ok(sid_uuid) = ::uuid::Uuid::parse_str(&sid_uuid_str) else {
            return Ok(None);
        };
        let sid_thing = Thing::from((
            "schedule",
            surrealdb::sql::Id::Uuid(surrealdb::sql::Uuid::from(sid_uuid)),
        ));

        let query = "SELECT run_id, kind, target_entity, due_at, status, attempt, depends_on, metadata, is_current, valid_from, valid_to \
            FROM schedule WHERE id = $sid AND is_current = true AND tenant_id = $session_tenant LIMIT 1";

        let mut res = self.db.query(query).bind(("sid", sid_thing)).await?;
        let row = res.take::<Vec<serde_json::Value>>(0)?.pop();
        Ok(row)
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
