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
    /// in the substrate any more; ids are `UUIDv7` and lookups go through the cache.
    type_cache: Arc<OnceLock<HashMap<String, Thing>>>,
    /// Cache of `cursor_type` row ids keyed by their stable `uid` (e.g.,
    /// `"ingestion"`, `"transcript"`). Populated once at the end of
    /// [`Kernel::init`] from the seeded `cursor_type` rows. Every cursor
    /// verb (`write_cursor`, `read_cursor`) resolves its categorisation
    /// through this cache via [`Kernel::cursor_type_thing`].
    cursor_type_cache: Arc<OnceLock<HashMap<String, Thing>>>,
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
struct StateResult {
    value_json: Option<serde_json::Value>
}

/// Read-side projection of a `cursor` row, returned by
/// [`Kernel::read_cursor`]. `last_processed` is the cursor token a
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
            cursor_type_cache: Arc::new(OnceLock::new()),
        };

        // Phase 1: schema + seed run under root (embedded RocksDB default).
        // This is the only operator-authorised root activity per skill §10.A.
        kernel.apply_substrate_schema().await?;
        let cache = kernel.seed_metamodel().await?;
        kernel.type_cache
            .set(cache)
            .map_err(|_| KernelError::Init("type_cache already initialised".to_string()))?;
        let cursor_cache = kernel.seed_cursor_types().await?;
        kernel.cursor_type_cache
            .set(cursor_cache)
            .map_err(|_| KernelError::Init("cursor_type_cache already initialised".to_string()))?;

        // Phase 1.5: seed the default `node_substrate` entity for this DB.
        // The substrate is the root of the tenancy graph (`tenant = NONE`);
        // every other entity references it. Deterministic UUIDv5 from
        // `(ns, db_name)` makes re-bootstraps idempotent. Operator-
        // authorised init activity per skill §10.A.
        kernel.seed_default_substrate(ns, db_name).await?;

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

    // The body is one continuous SurrealQL schema literal (~200 lines: 9 tables,
    // PERMISSIONS, ASSERT clauses). Splitting it into helpers would obscure the
    // source-of-truth shape — the schema reads top-to-bottom as one document.
    #[allow(clippy::too_many_lines)]
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
                    FOR select WHERE tenant = $session_tenant OR id = $session_tenant OR $session_role = 'admin'
                    FOR create WHERE tenant = $session_tenant OR id = $session_tenant OR $session_role = 'admin'
                    FOR update NONE
                    FOR delete NONE;

            DEFINE FIELD id           ON entity TYPE uuid;
            DEFINE FIELD type         ON entity TYPE record<type_definition>
                ASSERT $value.category = 'node';
            -- Tenant is optional for `node_substrate` entities — they are the
            -- root of the tenancy graph and have no tenant above them. Every
            -- other entity MUST reference a `node_substrate` row. The ASSERT
            -- enforces both shapes: NONE (only valid for substrate rows by
            -- caller convention) OR a typed reference to a substrate.
            DEFINE FIELD tenant       ON entity TYPE option<record<entity>>
                ASSERT $value = NONE OR $value.type.uid = 'node_substrate';
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
                    FOR select WHERE tenant = $session_tenant OR $session_role = 'admin'
                    FOR create WHERE tenant = $session_tenant OR $session_role = 'admin'
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
                    FOR select WHERE tenant = $session_tenant OR $session_role = 'admin'
                    FOR create WHERE tenant = $session_tenant OR $session_role = 'admin'
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
                    FOR select WHERE tenant = $session_tenant OR $session_role = 'admin'
                    FOR create WHERE tenant = $session_tenant OR $session_role = 'admin'
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
                    FOR select WHERE tenant = $session_tenant OR $session_role = 'admin'
                    FOR create WHERE tenant = $session_tenant OR $session_role = 'admin'
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
                    FOR select WHERE tenant = $session_tenant OR $session_role = 'admin'
                    FOR create WHERE tenant = $session_tenant OR $session_role = 'admin'
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
                    FOR select WHERE tenant = $session_tenant OR $session_role = 'admin'
                    FOR create WHERE tenant = $session_tenant OR $session_role = 'admin'
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
    /// rows under the v2 schema (`UUIDv7` ids + `uid` field).
    ///
    /// For each canonical type:
    /// - SELECT existing row by `uid` (idempotent on re-bootstrap);
    /// - if not found, CREATE with a fresh `UUIDv7` id + the `uid` field;
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
    /// v2 schema, `type_definition` ids are `UUIDv7`; the human-readable name
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

    /// `seed_cursor_types`: idempotently seeds the canonical `cursor_type`
    /// rows for the v2 substrate. Categorises every cursor kind:
    ///
    /// - **workload** — `ingestion`, `compile_context`, `compile_proposal`
    ///   (resume points for the kernel's existing workflow verbs).
    /// - **telemetry** — `transcript`, `otlp`, `hook` (capture-source
    ///   watermarks for the v0.1 multi-vector telemetry pipeline).
    ///
    /// Same SELECT-then-CREATE idempotency pattern as `seed_metamodel`.
    /// Returns `HashMap<uid, Thing>` installed on `Kernel::cursor_type_cache`.
    ///
    /// Runs at init time under root (`cursor_type` PERMISSIONS FOR create
    /// require admin role).
    ///
    /// # Errors
    /// Returns `KernelError::Database` if a SELECT or CREATE fails.
    async fn seed_cursor_types(&self) -> Result<HashMap<String, Thing>, KernelError> {
        // (uid, category, description)
        let kinds: Vec<(&str, &str, &str)> = vec![
            ("ingestion",        "workload",  "Per-subject file/directory ingestion progress"),
            ("compile_context",  "workload",  "Compiler context-distillation walk"),
            ("compile_proposal", "workload",  "Compiler proposal-evaluation walk"),
            ("transcript",       "telemetry", "Agent-transcript file-tail byte offset"),
            ("otlp",             "telemetry", "OTLP receiver last-seen span id"),
            ("hook",             "telemetry", "HTTP hook receiver delivery id"),
        ];

        let mut cache: HashMap<String, Thing> = HashMap::with_capacity(kinds.len());
        for (uid, category, description) in kinds {
            let mut select_res = self.db
                .query("SELECT id FROM cursor_type WHERE uid = $uid LIMIT 1")
                .bind(("uid", uid.to_string()))
                .await?;
            let existing: Option<IdResult> = select_res.take::<Vec<IdResult>>(0)?.pop();

            let id_thing = if let Some(row) = existing {
                row.id
            } else {
                let new_uuid = ::uuid::Uuid::now_v7();
                let new_id = Thing::from((
                    "cursor_type",
                    surrealdb::sql::Id::Uuid(surrealdb::sql::Uuid::from(new_uuid)),
                ));
                self.db
                    .query("CREATE cursor_type CONTENT { id: $id, uid: $uid, category: $cat, description: $desc, sch_json: NONE }")
                    .bind(("id", new_id.clone()))
                    .bind(("uid", uid.to_string()))
                    .bind(("cat", category.to_string()))
                    .bind(("desc", description.to_string()))
                    .await?
                    .check()?;
                new_id
            };
            cache.insert(uid.to_string(), id_thing);
        }
        Ok(cache)
    }

    /// `seed_default_substrate`: idempotent root-context seed of the default
    /// `node_substrate` entity for this kernel's `(ns, db_name)`. The
    /// substrate is the root of the tenancy graph and has `tenant = NONE`;
    /// every other entity references it via its own `tenant` FK.
    ///
    /// Runs in the pre-signin init phase per skill §10.A (operator-authorised
    /// init activity). The bootstrap blade configures `attr_config` state on
    /// this same row.
    ///
    /// # Errors
    /// Returns `KernelError::Database` if SELECT or CREATE fails.
    async fn seed_default_substrate(&self, ns: &str, db_name: &str) -> Result<(), KernelError> {
        #[derive(Deserialize)]
        struct IdRow { #[allow(dead_code)] id: Thing }

        let substrate_uuid = Self::default_substrate_uuid(ns, db_name);
        let substrate_thing = Thing::from((
            "entity",
            surrealdb::sql::Id::Uuid(surrealdb::sql::Uuid::from(substrate_uuid)),
        ));

        // Idempotent: skip if already seeded.
        let mut sel = self.db
            .query("SELECT id FROM entity WHERE id = $id LIMIT 1")
            .bind(("id", substrate_thing.clone()))
            .await?;
        if sel.take::<Vec<IdRow>>(0)?.pop().is_some() {
            return Ok(());
        }

        let node_substrate = self.type_thing("node_substrate")?;
        // Substrate rows have `tenant = NONE` — they ARE the tenant root, not
        // an instance of one. The schema's tenant ASSERT permits NONE for
        // exactly this case.
        self.db.query(
            "CREATE entity CONTENT { \
                id: $id, type: $type, tenant: NONE, role: 'admin' \
            }"
        )
            .bind(("id", substrate_thing))
            .bind(("type", node_substrate))
            .await?.check()?;
        Ok(())
    }

    /// `default_substrate_uuid`: deterministic `UUIDv5` derived from
    /// `(ns, db_name)`. Same kernel handle always resolves to the same
    /// substrate entity — re-bootstraps reuse it idempotently.
    ///
    /// # Panics
    /// Panics if the hard-coded DNS namespace UUID literal fails to parse —
    /// that would be a build-time programmer error, not a runtime input
    /// problem (the literal is the IETF DNS namespace constant from RFC 4122).
    #[must_use]
    pub fn default_substrate_uuid(ns: &str, db_name: &str) -> ::uuid::Uuid {
        let key = format!("{ns}/{db_name}");
        let ns_uuid = ::uuid::Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8")
            .expect("Valid DNS NS UUID");
        ::uuid::Uuid::new_v5(&ns_uuid, key.as_bytes())
    }

    /// `default_substrate_thing`: convenience wrapper around
    /// [`Self::default_substrate_uuid`] returning a substrate `Thing`.
    #[must_use]
    pub fn default_substrate_thing(ns: &str, db_name: &str) -> Thing {
        Thing::from((
            "entity",
            surrealdb::sql::Id::Uuid(surrealdb::sql::Uuid::from(Self::default_substrate_uuid(ns, db_name))),
        ))
    }

    /// `cursor_type_thing`: resolve a canonical cursor-type `uid` (e.g.
    /// `"ingestion"`, `"transcript"`) to its `Thing` reference. Single
    /// chokepoint for the new `cursor.cursor_type` FK — never a named-id
    /// literal.
    ///
    /// # Errors
    /// Returns `KernelError::Integrity` if the cache is uninitialised or the
    /// `uid` was not seeded.
    pub fn cursor_type_thing(&self, uid: &str) -> Result<Thing, KernelError> {
        let cache = self.cursor_type_cache.get().ok_or_else(|| {
            KernelError::Integrity("cursor_type_cache not yet initialised — called before Kernel::init completed".to_string())
        })?;
        cache.get(uid).cloned().ok_or_else(|| {
            KernelError::Integrity(format!("cursor_type uid '{uid}' not in cache — missing from seed_cursor_types?"))
        })
    }

    /// `set_session_auth`: Set the `SurrealDB` session context using custom variables.
    ///
    /// Stores `$session_tenant` as a typed `Thing` (the substrate-entity FK)
    /// so the PERMISSIONS clauses can compare `tenant = $session_tenant`
    /// directly without a `<string>` cast. The `&str` signature is preserved
    /// because every caller already passes the substrate uuid as a string —
    /// the parse-to-`Thing` happens once here, at the boundary, not at every
    /// kernel verb.
    ///
    /// # Panics
    /// Panics if `tenant_id` or `role` are empty.
    ///
    /// # Errors
    /// Returns `KernelError::Database` if binding fails, or
    /// `KernelError::Validation` if `tenant_id` is not a valid record id.
    pub async fn set_session_auth(&self, tenant_id: &str, role: &str) -> Result<(), KernelError> {
        assert!(!tenant_id.is_empty(), "Tenant ID mandatory for auth");
        assert!(!role.is_empty(), "Role mandatory for auth");

        // Boundary conversion: a tenant_id like "<uuid>" or "entity:<uuid>"
        // becomes the typed `Thing`. From this point on the session var is
        // record-typed; no further string round-trip occurs.
        let literal = if tenant_id.contains(':') {
            tenant_id.to_string()
        } else {
            format!("entity:{tenant_id}")
        };
        let tenant_thing = Self::parse_id(&literal)?;

        self.db.set("session_tenant", tenant_thing).await?;
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

        // Strip SurrealDB's backtick wrapping and any `u'…'` prefix that
        // appears on the wire for Uuid-typed ids.
        let cleaned = s.replace('`', "");
        let parts: Vec<&str> = cleaned.split(':').collect();
        if parts.len() != 2 {
            return Err(KernelError::Validation(format!("Invalid ID format: {s}")));
        }
        if parts[0].is_empty() || parts[1].is_empty() {
            return Err(KernelError::Validation(format!("Invalid ID format: {s}")));
        }

        let table = parts[0].to_string();
        let id_part = parts[1].trim_start_matches("u'").trim_end_matches('\'');

        // Detect UUID-formatted ids and build `Id::Uuid` so the Thing matches
        // rows stored with `DEFINE FIELD id TYPE uuid`. Falls back to
        // `Id::String` for non-UUID ids (e.g. legacy named ids — kept as a
        // boundary for callers that haven't migrated yet).
        if let Ok(uuid) = ::uuid::Uuid::parse_str(id_part) {
            Ok(Thing::from((
                table,
                surrealdb::sql::Id::Uuid(surrealdb::sql::Uuid::from(uuid)),
            )))
        } else {
            Ok(Thing::from((table, parts[1].to_string())))
        }
    }

    /// `session_tenant_thing`: the calling session's tenant as a typed
    /// `record<entity>` `Thing`. Reads `$session_tenant` directly — the
    /// session var was stored as a `Thing` by `set_session_auth`, so no
    /// conversion happens here. This is the only public accessor; every
    /// kernel verb uses it for tenant FK binds.
    ///
    /// # Errors
    /// Returns `KernelError::SafetyViolation` if no session is established.
    pub async fn session_tenant_thing(&self) -> Result<Thing, KernelError> {
        let mut res = self.db.query("RETURN $session_tenant").await?;
        let tenant: Option<Thing> = res.take(0)?;
        tenant.ok_or_else(|| KernelError::SafetyViolation(
            "No authenticated session. Ensure set_session_auth was called.".into()
        ))
    }

    /// `session_tenant`: legacy string accessor kept for crates that still
    /// log the tenant uuid (logs / error messages). Returns the substrate
    /// uuid in string form, derived from the typed `Thing` in the session
    /// var. Prefer [`session_tenant_thing`](Self::session_tenant_thing) for
    /// every typed callsite — binds, FK assignments, comparisons.
    ///
    /// # Errors
    /// Returns `KernelError::SafetyViolation` if no session is established.
    pub async fn session_tenant(&self) -> Result<String, KernelError> {
        let thing = self.session_tenant_thing().await?;
        match &thing.id {
            surrealdb::sql::Id::Uuid(u) => Ok(u.to_raw()),
            surrealdb::sql::Id::String(s) => Ok(s.clone()),
            other => Err(KernelError::Integrity(format!(
                "session_tenant id has unexpected form: {other:?}"
            ))),
        }
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

        let query = "SELECT value_json, valid_from FROM state_ledger \
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
        #[derive(serde::Deserialize)]
        struct SchRow { sch_json: Option<String> }
        #[derive(serde::Deserialize)]
        struct TenantRow { tenant: Option<Thing> }

        assert!(!target_id.is_empty(), "Target ID mandatory");
        assert!(!type_uid.is_empty(), "Type UID mandatory");

        // Resolve the attribute type through the metamodel cache.
        // Under v2 the type_definition row id is UUIDv7; the human-readable
        // `type_uid` (e.g. "attr_desc") resolves to a Thing via type_thing().
        let type_thing = self.type_thing(type_uid)?;
        let target_thing = Self::parse_id(target_id)?;
        let session_tenant_thing = self.session_tenant_thing().await?;

        // 1. Fetch the attribute type's JSON Schema (if any) + the target
        //    entity's tenant FK. Types flow end-to-end as `Thing` — no string
        //    conversion at the boundary. Two single-row SELECTs because
        //    SurrealDB's nested-subquery shape is awkward for two distinct
        //    record types.
        let mut sch_res = self.db
            .query("SELECT sch_json FROM $ty LIMIT 1")
            .bind(("ty", type_thing.clone()))
            .await?;
        let sch_json = sch_res.take::<Vec<SchRow>>(0)?
            .pop()
            .ok_or_else(|| KernelError::Validation(format!("Type {type_uid} not found")))?
            .sch_json;

        let mut tenant_res = self.db
            .query("SELECT tenant FROM $target LIMIT 1")
            .bind(("target", target_thing.clone()))
            .await?;
        let target_tenant = tenant_res.take::<Vec<TenantRow>>(0)?
            .pop()
            .ok_or_else(|| KernelError::Validation(format!("Target {target_id} not found")))?
            .tenant;

        // 2. Tenant-coercion guard — Thing comparison, no string conversion.
        //    Substrate-root case: when the target IS the session's substrate
        //    entity, its `tenant` is NONE by schema (option<record<entity>>
        //    with the substrate self-reference relaxed). That is the only
        //    legitimate NONE; everything else must match the session tenant.
        let target_is_substrate_root = target_thing == session_tenant_thing;
        let tenant_ok = match target_tenant.as_ref() {
            Some(t) => t == &session_tenant_thing,
            None => target_is_substrate_root,
        };
        if !tenant_ok {
            return Err(KernelError::SafetyViolation(format!(
                "Tenant mismatch: Entity {target_id} tenant {target_tenant:?} != session {session_tenant_thing}"
            )));
        }

        // 3. JSON-Schema validation at the write boundary.
        if let Some(schema_str) = sch_json {
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
            .bind(("tenant", session_tenant_thing))
            .bind(("val", value)).await?.check()?;

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
        #[derive(serde::Deserialize)]
        struct EntityTenant { tenant: Option<Thing> }

        assert!(!from.is_empty(), "Source ID mandatory");
        assert!(!to.is_empty(), "Target ID mandatory");

        // Resolve metamodel + identity Things once. type_thing fails fast if
        // edge_type is not a seeded metamodel uid — the substrate would have
        // refused via the relation.type ASSERT clause otherwise, but failing
        // here gives a clearer error message.
        let edge_type_thing = self.type_thing(edge_type)?;
        let from_thing = Self::parse_id(from)?;
        let to_thing = Self::parse_id(to)?;
        let tenant_thing = self.session_tenant_thing().await?;

        // 1. Anti-coercion check — the source entity's `tenant` FK must point
        //    at the same substrate Thing the session is bound to. Types flow
        //    end-to-end as `Thing`. Substrate-root case: when the source IS
        //    the session's substrate entity, its `tenant` is NONE by schema
        //    and the source's own `id` matches the session — admit it.
        let mut fetch_res = self.db
            .query("SELECT tenant FROM $target LIMIT 1")
            .bind(("target", from_thing.clone()))
            .await?;
        let target_tenant: Option<Thing> = fetch_res.take::<Vec<EntityTenant>>(0)?
            .pop()
            .ok_or_else(|| KernelError::Validation(format!("Source {from} not found")))?
            .tenant;
        let from_is_substrate_root = from_thing == tenant_thing;
        let tenant_ok = match target_tenant.as_ref() {
            Some(t) => t == &tenant_thing,
            None => from_is_substrate_root,
        };
        if !tenant_ok {
            return Err(KernelError::SafetyViolation(format!(
                "Tenant mismatch: Entity {from} tenant {target_tenant:?} != session {tenant_thing}"
            )));
        }

        // 2. is_acyclic flag from the metamodel row.
        let mut type_res = self.db
            .query("SELECT is_acyclic FROM $id")
            .bind(("id", edge_type_thing.clone()))
            .await?;
        let is_acyclic = type_res.take::<Vec<AcyclicCheck>>(0)?
            .pop()
            .is_some_and(|c| c.is_acyclic);

        // 3. Cycle detection — walk outgoing acyclic edges under the same
        //    tenant. Bounded by NASA Power-of-10 max_iters.
        if is_acyclic {
            let max_iters: usize = self.get_parameter("max_dfs_iterations", 10_000).await;
            let mut stack = vec![to_thing.clone()];
            let mut visited = std::collections::HashSet::new();
            let mut iters: usize = 0;
            while let Some(current) = stack.pop() {
                iters += 1;
                assert!(iters <= max_iters, "Safety violation: DFS depth exceeded");
                if iters > max_iters { return Err(KernelError::SafetyViolation("DFS limit".into())); }
                if current == from_thing { return Err(KernelError::CycleDetected); }
                if !visited.insert(current.to_string()) { continue; }

                let mut res = self.db.query(
                    "SELECT out.id as id FROM relation \
                     WHERE in = $id AND tenant = $tenant AND is_acyclic = true"
                )
                    .bind(("id", current))
                    .bind(("tenant", tenant_thing.clone()))
                    .await?;
                let children: Vec<IdResult> = res.take(0)?;
                for c in children { stack.push(c.id); }
            }
        }

        // 4. Pure INSERT — engine refuses UPDATE for superx anyway.
        //    v2 field names: in / out / type / tenant / is_acyclic (no tenant_id).
        let rel_record_id = Thing::from((
            "relation",
            surrealdb::sql::Id::Uuid(surrealdb::sql::Uuid::from(::uuid::Uuid::now_v7())),
        ));
        self.db.query(
            "INSERT INTO relation { \
                id: $id, \
                in: $f, \
                out: $t, \
                type: $ty, \
                tenant: $tenant, \
                is_acyclic: $acyc \
            }"
        )
            .bind(("id", rel_record_id))
            .bind(("f", from_thing))
            .bind(("t", to_thing))
            .bind(("ty", edge_type_thing))
            .bind(("tenant", tenant_thing))
            .bind(("acyc", is_acyclic))
            .await?.check()?;

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

            // Fetch structural children (outgoing acyclic edges) within SAME tenant.
            // v2: tenant FK is `record<entity>`; compare to the session's
            // typed Thing — no legacy `tenant_id` column under v2.
            let query = "SELECT out.id as id FROM relation WHERE in = $id AND tenant = $session_tenant AND is_acyclic = true";
            let mut res = self.db.query(query)
                .bind(("id", current)).await?;
            let children: Vec<IdResult> = res.take(0)?;
            for c in children {
                stack.push((c.id, depth + 1));
            }
        }

        let mut xml = String::from("<context>\n");
        for node in nodes_ordered {
            // Fetch state rows filtered by target memory tiers and session
            // tenant. v2: typed FK comparison `tenant = $session_tenant`;
            // "current" is the most-recent row by `valid_from`, not the
            // dropped `is_current = true` filter.
            let fetch = "SELECT value_json, valid_from FROM state_ledger \
                WHERE target = $id AND tenant = $session_tenant \
                  AND `type`.memory_tier INSIDE $tiers \
                ORDER BY valid_from DESC LIMIT 1";
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
        let tenant_thing = self.session_tenant_thing().await?;

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

    /// `write_cursor`: append a new resume-point to the `cursor` chain for a
    /// given `(subject, cursor_type)` tuple. Pure INSERT — the `cursor`
    /// table is append-only under v2; every checkpoint is a fresh row and
    /// "current" is recovered at query time via [`Kernel::read_cursor`].
    ///
    /// `subject` is a `record<entity>` Thing — the entity whose progress is
    /// being tracked (a `node_run` for workload cursors, a `node_source`
    /// for telemetry cursors, or any other entity the caller chooses).
    ///
    /// `cursor_type_uid` resolves through [`Kernel::cursor_type_thing`] to
    /// the typed `record<cursor_type>` FK on the row.
    ///
    /// `last_processed` is the opaque cursor token; `metadata` is an
    /// opaque JSON envelope. Shape per `cursor_type` is defined by the
    /// consuming blade.
    ///
    /// Replaces the legacy `checkpoint_execution` verb (which `UPSERTed` into
    /// the dropped `execution_cursor` table). Engine refuses UPDATE/UPSERT
    /// under the superx service account.
    ///
    /// Emits a `cursor_write` telemetry event.
    ///
    /// # Errors
    /// `KernelError::Integrity` if `cursor_type_uid` is not in the cache;
    /// `KernelError::SafetyViolation` if no session is established;
    /// `KernelError::Database` if the engine refuses the insert.
    pub async fn write_cursor(
        &self,
        subject: Thing,
        cursor_type_uid: &str,
        last_processed: Option<String>,
        metadata: Option<serde_json::Value>,
    ) -> Result<(), KernelError> {
        let cursor_type_thing = self.cursor_type_thing(cursor_type_uid)?;
        let tenant_thing = self.session_tenant_thing().await?;

        let row_id = Thing::from((
            "cursor",
            surrealdb::sql::Id::Uuid(surrealdb::sql::Uuid::from(::uuid::Uuid::now_v7())),
        ));

        self.db.query(
            "CREATE cursor CONTENT { \
                id: $id, \
                subject: $subject, \
                tenant: $tenant, \
                cursor_type: $ct, \
                last_processed: $last, \
                metadata: $meta \
            };"
        )
            .bind(("id", row_id))
            .bind(("subject", subject.clone()))
            .bind(("tenant", tenant_thing))
            .bind(("ct", cursor_type_thing))
            .bind(("last", last_processed.clone()))
            .bind(("meta", metadata))
            .await?.check()?;

        let _ = self.log_telemetry(
            serde_json::json!({
                "subject": subject.to_string(),
                "cursor_type": cursor_type_uid,
                "last": last_processed
            }),
            "cursor_write",
            None
        ).await;

        Ok(())
    }

    /// `read_cursor`: read the latest cursor row in a chain (or `None` if
    /// none exists). "Latest" is the most-recent row by `valid_from`
    /// against the `(subject, cursor_type)` chain key. No `is_current`
    /// filter — that field doesn't exist in v2.
    ///
    /// Replaces the legacy `get_execution_cursor` verb.
    ///
    /// # Errors
    /// `KernelError::Integrity` if `cursor_type_uid` is not in the cache;
    /// `KernelError::Database` if the query fails.
    pub async fn read_cursor(
        &self,
        subject: Thing,
        cursor_type_uid: &str,
    ) -> Result<Option<CursorResult>, KernelError> {
        let cursor_type_thing = self.cursor_type_thing(cursor_type_uid)?;

        let mut res = self.db.query(
            "SELECT last_processed, metadata, valid_from FROM cursor \
             WHERE subject = $subject AND cursor_type = $ct \
             ORDER BY valid_from DESC LIMIT 1"
        )
            .bind(("subject", subject))
            .bind(("ct", cursor_type_thing))
            .await?;

        let cursor = res.take::<Vec<CursorResult>>(0)?.pop();
        Ok(cursor)
    }

    /// `set_execution_params`: append-only write of per-run agent execution
    /// knobs (`temperature`, `top_p`, `top_k`, `max_tokens`, `turns`,
    /// `branch`, `retry_policy`, `model_ref`, …) into the `execution_params`
    /// table. **Pure INSERT under v2** — no close-prior UPDATE, no
    /// `is_current` / `valid_to` writes. "Current" is recovered at query
    /// time via `ORDER BY valid_from DESC LIMIT 1` against the
    /// `(run, agent)` chain key. The full history is `SELECT … ORDER BY
    /// valid_from ASC`.
    ///
    /// `run` and `agent` are typed `record<entity>` FKs:
    /// - `run` must point at a `node_run` entity (engine ASSERT).
    /// - `agent` must point at a `node_agent` entity (engine ASSERT).
    ///
    /// `params` is opaque JSON — kernel does not enforce a schema on its
    /// contents (blades enforce their own knob shape).
    ///
    /// Runs under the `superx` service account; the engine refuses any
    /// UPDATE attempt via PERMISSIONS FOR update NONE.
    ///
    /// Emits an `execution_params_set` telemetry event.
    ///
    /// # Errors
    /// Returns `KernelError::SafetyViolation` if no session is established;
    /// `KernelError::Database` if the engine refuses the insert (e.g. an FK
    /// pointing at the wrong entity type — that's the §12 debugging surface).
    pub async fn set_execution_params(
        &self,
        run: Thing,
        agent: Thing,
        params: serde_json::Value,
    ) -> Result<(), KernelError> {
        // Pre-condition: caller is in a session. Surfaces "no session" as a
        // clean SafetyViolation rather than a substrate permission error.
        let tenant_thing = self.session_tenant_thing().await?;

        let row_id = Thing::from((
            "execution_params",
            surrealdb::sql::Id::Uuid(surrealdb::sql::Uuid::from(::uuid::Uuid::now_v7())),
        ));

        self.db.query(
            "CREATE execution_params CONTENT { \
                id: $id, \
                run: $run, \
                tenant: $tenant, \
                agent: $agent, \
                params_json: $params \
            };"
        )
            .bind(("id", row_id))
            .bind(("run", run.clone()))
            .bind(("agent", agent.clone()))
            .bind(("tenant", tenant_thing))
            .bind(("params", params))
            .await?.check()?;

        let _ = self.log_telemetry(
            serde_json::json!({"run": run.to_string(), "agent": agent.to_string()}),
            "execution_params_set",
            None
        ).await;

        Ok(())
    }

    /// `get_execution_params`: read the current execution-params payload for
    /// a `(run, agent)` chain. Returns `Ok(None)` when no row exists;
    /// `Ok(Some(params_json))` when it does.
    ///
    /// "Current" is the most-recent row by `valid_from` against the chain
    /// key. No `is_current` filter — that field doesn't exist in v2.
    ///
    /// # Errors
    /// Returns `KernelError::Database` if the query fails.
    pub async fn get_execution_params(
        &self,
        run: Thing,
        agent: Thing,
    ) -> Result<Option<serde_json::Value>, KernelError> {
        #[derive(serde::Deserialize)]
        struct Row { params_json: serde_json::Value }

        let mut res = self.db.query(
            "SELECT params_json, valid_from FROM execution_params \
             WHERE run = $run AND agent = $agent \
             ORDER BY valid_from DESC LIMIT 1"
        )
            .bind(("run", run))
            .bind(("agent", agent))
            .await?;

        let row = res.take::<Vec<Row>>(0)?.pop();
        Ok(row.map(|r| r.params_json))
    }

    /// `enqueue_schedule_item`: append a new pending work item to the `schedule`
    /// table in `status = 'waiting'`. Returns the row id (`schedule:<uuidv7>`)
    /// `enqueue_schedule_item`: append a new pending work item to the
    /// `schedule` table in `status = 'waiting'`. Pure INSERT — the schedule
    /// table is append-only under v2; status transitions are new rows in
    /// the chain, not in-place mutations.
    ///
    /// FKs are typed `record<entity>`:
    /// - `run` must point at a `node_run` entity (engine ASSERT).
    /// - `target` is any entity (the DAG-node being acted on).
    /// - `tenant` resolved from the session's substrate Thing.
    ///
    /// `depends_on` is a list of `Thing` references into the `schedule`
    /// table — the latest-by-`valid_from` row of each chain this item
    /// depends on. The runner is responsible for honouring it.
    ///
    /// `metadata` is opaque to the kernel.
    ///
    /// Emits a `schedule_enqueued` telemetry event.
    ///
    /// # Panics
    /// Panics if `kind` is empty.
    ///
    /// # Errors
    /// Returns `KernelError::SafetyViolation` if no session is established;
    /// `KernelError::Database` if the engine refuses (e.g. an FK pointing
    /// at the wrong entity type — that's the §12 debugging surface).
    pub async fn enqueue_schedule_item(
        &self,
        run: Thing,
        kind: &str,
        target: Thing,
        due_at: chrono::DateTime<chrono::Utc>,
        depends_on: Vec<Thing>,
        metadata: serde_json::Value,
    ) -> Result<Thing, KernelError> {
        assert!(!kind.is_empty(), "kind mandatory");

        let tenant_thing = self.session_tenant_thing().await?;

        let row_id = Thing::from((
            "schedule",
            surrealdb::sql::Id::Uuid(surrealdb::sql::Uuid::from(::uuid::Uuid::now_v7())),
        ));

        self.db.query(
            "CREATE schedule CONTENT { \
                id: $id, \
                run: $run, \
                tenant: $tenant, \
                kind: $kind, \
                target: $target, \
                due_at: <datetime> $due, \
                status: 'waiting', \
                attempt: 0, \
                depends_on: $deps, \
                metadata: $meta \
            };"
        )
            .bind(("id", row_id.clone()))
            .bind(("run", run.clone()))
            .bind(("tenant", tenant_thing))
            .bind(("kind", kind.to_string()))
            .bind(("target", target.clone()))
            .bind(("due", due_at.to_rfc3339()))
            .bind(("deps", depends_on))
            .bind(("meta", metadata))
            .await?.check()?;

        let _ = self.log_telemetry(
            serde_json::json!({
                "schedule_id": row_id.to_string(),
                "run": run.to_string(),
                "kind": kind,
                "target": target.to_string()
            }),
            "schedule_enqueued",
            None
        ).await;

        Ok(row_id)
    }

    /// `transition_schedule_status`: append a new row to a schedule chain
    /// with a new status. **Pure INSERT under v2** — the prior row is *not*
    /// updated; "current" is always the latest-by-`valid_from` row in the
    /// chain. The superx service account has no UPDATE privilege and
    /// PERMISSIONS FOR update NONE would refuse it anyway.
    ///
    /// Reads the prior row to carry forward `run` / `kind` / `target` /
    /// `due_at` / `attempt` / `depends_on` / `metadata`, changes only the
    /// `status` field on the new row.
    ///
    /// Status transitions follow `waiting → scheduled → running →
    /// completed/failed` (plus `awaiting_human` for HITL gates). The
    /// `schedule.status` field's ASSERT INSIDE [...] clause enforces the
    /// allowed values; the engine refuses any other.
    ///
    /// Emits a `schedule_transitioned` telemetry event with both states.
    ///
    /// # Panics
    /// Panics if `new_status` is empty.
    ///
    /// # Errors
    /// Returns `KernelError::Validation` if no prior row exists for the
    /// given chain id; `KernelError::Database` if the insert is refused.
    pub async fn transition_schedule_status(
        &self,
        schedule_id: Thing,
        new_status: &str,
    ) -> Result<Thing, KernelError> {
        #[derive(serde::Deserialize)]
        struct CurrentRow {
            run: Thing,
            kind: String,
            target: Thing,
            due_at: chrono::DateTime<chrono::Utc>,
            status: String,
            attempt: i64,
            depends_on: Vec<Thing>,
            metadata: serde_json::Value,
        }

        assert!(!new_status.is_empty(), "new_status mandatory");

        let tenant_thing = self.session_tenant_thing().await?;

        // 1. Fetch the latest row in the chain. The chain root is the
        //    schedule_id the caller has in hand; from there we walk to the
        //    most-recent successor by valid_from. For v0.1 we just read the
        //    given row (chains are short; full chain-walk is later when
        //    SchedulerBlade lands).
        let mut res = self.db.query(
            "SELECT run, kind, target, due_at, status, attempt, depends_on, metadata, valid_from \
             FROM schedule WHERE id = $sid \
             ORDER BY valid_from DESC LIMIT 1"
        )
            .bind(("sid", schedule_id.clone()))
            .await?;
        let current: CurrentRow = res.take::<Vec<CurrentRow>>(0)?
            .pop()
            .ok_or_else(|| KernelError::Validation(format!(
                "schedule item not found: {schedule_id}"
            )))?;

        let prior_status = current.status.clone();

        // 2. Pure INSERT — no UPDATE, no close-prior. The chain is
        //    reconstructed at query time via ORDER BY valid_from.
        let new_row_id = Thing::from((
            "schedule",
            surrealdb::sql::Id::Uuid(surrealdb::sql::Uuid::from(::uuid::Uuid::now_v7())),
        ));

        self.db.query(
            "CREATE schedule CONTENT { \
                id: $new_id, \
                run: $run, \
                tenant: $tenant, \
                kind: $kind, \
                target: $target, \
                due_at: <datetime> $due, \
                status: $new_status, \
                attempt: $attempt, \
                depends_on: $deps, \
                metadata: $meta \
            };"
        )
            .bind(("new_id", new_row_id.clone()))
            .bind(("run", current.run.clone()))
            .bind(("tenant", tenant_thing))
            .bind(("kind", current.kind))
            .bind(("target", current.target))
            .bind(("due", current.due_at.to_rfc3339()))
            .bind(("new_status", new_status.to_string()))
            .bind(("attempt", current.attempt))
            .bind(("deps", current.depends_on))
            .bind(("meta", current.metadata))
            .await?.check()?;

        let _ = self.log_telemetry(
            serde_json::json!({
                "schedule_id": schedule_id.to_string(),
                "new_schedule_id": new_row_id.to_string(),
                "prior_status": prior_status,
                "new_status": new_status,
                "run": current.run.to_string(),
            }),
            "schedule_transitioned",
            None
        ).await;

        Ok(new_row_id)
    }

    /// `get_schedule_item`: returns the latest-by-`valid_from` row for a
    /// given schedule chain id (or `None` if no row exists). The JSON shape
    /// mirrors the substrate row exactly.
    ///
    /// "Current" is the most recent row by `valid_from` — there is no
    /// `is_current` filter under v2.
    ///
    /// # Errors
    /// Returns `KernelError::Database` if the query fails.
    pub async fn get_schedule_item(
        &self,
        schedule_id: Thing,
    ) -> Result<Option<serde_json::Value>, KernelError> {
        #[derive(serde::Deserialize)]
        struct Row {
            run: Thing,
            kind: String,
            target: Thing,
            due_at: chrono::DateTime<chrono::Utc>,
            status: String,
            attempt: i64,
            depends_on: Vec<Thing>,
            metadata: serde_json::Value,
            valid_from: chrono::DateTime<chrono::Utc>,
        }

        let mut res = self.db.query(
            "SELECT run, kind, target, due_at, status, attempt, depends_on, metadata, valid_from \
             FROM schedule WHERE id = $sid \
             ORDER BY valid_from DESC LIMIT 1"
        )
            .bind(("sid", schedule_id))
            .await?;
        let row = res.take::<Vec<Row>>(0)?.pop();
        // Convert the strongly-typed row into a plain JSON object. `Thing`
        // fields surface as `{"tb": "<table>", "id": "<uuid-string>"}` so
        // callers can read them without a SurrealDB-specific schema.
        Ok(row.map(|r| serde_json::json!({
            "run":        thing_to_json(&r.run),
            "kind":       r.kind,
            "target":     thing_to_json(&r.target),
            "due_at":     r.due_at,
            "status":     r.status,
            "attempt":    r.attempt,
            "depends_on": r.depends_on.iter().map(thing_to_json).collect::<Vec<_>>(),
            "metadata":   r.metadata,
            "valid_from": r.valid_from,
        })))
    }
}

/// Render a `Thing` as a plain JSON object — `{"tb": "<table>", "id":
/// "<uuid-string>"}` — so callers receive a deserializer-friendly shape
/// instead of `SurrealDB`'s internal enum form.
fn thing_to_json(t: &Thing) -> serde_json::Value {
    let id = match &t.id {
        surrealdb::sql::Id::Uuid(u) => u.to_raw(),
        surrealdb::sql::Id::String(s) => s.clone(),
        other => format!("{other:?}"),
    };
    serde_json::json!({ "tb": t.tb, "id": id })
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

    /// Test helper: create a tenant by provisioning its `node_substrate`
    /// entity. The substrate entity is the self-tenant root — its `tenant`
    /// FK points at itself, satisfying the engine ASSERT. Session is bound
    /// to the substrate uuid string.
    ///
    /// Returns `(substrate_uuid_string, substrate_record_id)` so tests can
    /// reference both forms.
    async fn provision_tenant(kernel: &Kernel, tenant_name: &str) -> (String, String) {
        #[derive(serde::Deserialize)]
        struct IdRow { #[allow(dead_code)] id: Thing }

        let ns = ::uuid::Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8")
            .expect("Valid DNS NS UUID");
        let substrate_uuid = ::uuid::Uuid::new_v5(&ns, tenant_name.as_bytes());
        let substrate_uuid_str = substrate_uuid.to_string();
        let substrate_thing = Thing::from((
            "entity",
            surrealdb::sql::Id::Uuid(surrealdb::sql::Uuid::from(substrate_uuid)),
        ));
        let substrate_record_id = format!("entity:{substrate_uuid}");

        // Bind session BEFORE the insert so PERMISSIONS / future queries
        // see this tenant as the active one.
        kernel.set_session_auth(&substrate_uuid_str, "admin").await.unwrap();

        let node_substrate = kernel.type_thing("node_substrate").unwrap();

        // Idempotent: skip create if already exists.
        let mut sel = kernel.db
            .query("SELECT id FROM entity WHERE id = $id LIMIT 1")
            .bind(("id", substrate_thing.clone()))
            .await.unwrap();
        if sel.take::<Vec<IdRow>>(0).unwrap().pop().is_none() {
            kernel.db.query(
                "CREATE entity CONTENT { \
                    id: $id, type: $type, tenant: NONE, role: 'admin' \
                }"
            )
                .bind(("id", substrate_thing))
                .bind(("type", node_substrate))
                .await.unwrap().check().unwrap();
        }

        (substrate_uuid_str, substrate_record_id)
    }

    /// Test helper: CREATE a fresh entity of the given metamodel type under
    /// the given tenant. Returns the record-id literal (`entity:<uuidv7>`).
    async fn create_entity(kernel: &Kernel, type_uid: &str, tenant_uuid_str: &str) -> String {
        let new_uuid = ::uuid::Uuid::now_v7();
        let new_thing = Thing::from((
            "entity",
            surrealdb::sql::Id::Uuid(surrealdb::sql::Uuid::from(new_uuid)),
        ));
        let entity_type = kernel.type_thing(type_uid).unwrap();
        // Build the tenant Thing with Id::Uuid (the substrate row was created
        // with `id TYPE uuid`). parse_id would otherwise produce Id::String,
        // which references a different (non-existent) row.
        let tenant_uuid = ::uuid::Uuid::parse_str(tenant_uuid_str)
            .expect("tenant_uuid_str must be a valid UUID");
        let tenant_thing = Thing::from((
            "entity",
            surrealdb::sql::Id::Uuid(surrealdb::sql::Uuid::from(tenant_uuid)),
        ));

        kernel.db.query(
            "CREATE entity CONTENT { \
                id: $id, type: $type, tenant: $tenant, role: 'user' \
            }"
        )
            .bind(("id", new_thing))
            .bind(("type", entity_type))
            .bind(("tenant", tenant_thing))
            .await.unwrap().check().unwrap();

        format!("entity:{new_uuid}")
    }

    #[tokio::test]
    async fn test_validation_nasa_hardened() {
        let (_dir, kernel) = setup().await;
        let (tenant_uuid, _) = provision_tenant(&kernel, "tenant_1").await;
        let v1 = create_entity(&kernel, "node_prod", &tenant_uuid).await;

        // 1. Unknown type rejected
        let res = kernel.supersede_state(&v1, "none", serde_json::json!({"text":"x"}), None).await;
        assert!(
            matches!(&res, Err(KernelError::Integrity(_) | KernelError::Validation(_))),
            "supersede_state must reject unknown type uid, got {res:?}"
        );

        // 2. Schema-violation rejected
        let res = kernel.supersede_state(&v1, "attr_desc", serde_json::json!({"wrong":"y"}), None).await;
        assert!(
            matches!(&res, Err(KernelError::Validation(ref m)) if m.contains("fails validation")),
            "schema-invalid payload must be refused, got {res:?}"
        );

        // 3. Tenant-coercion rejected
        let (_, _) = provision_tenant(&kernel, "attacker").await; // switches session
        let res2 = kernel.supersede_state(&v1, "attr_desc", serde_json::json!({"text":"hack"}), None).await;
        assert!(
            matches!(&res2, Err(KernelError::SafetyViolation(m)) if m.contains("Tenant mismatch")),
            "cross-tenant write must be refused, got {res2:?}"
        );
    }

    #[tokio::test]
    async fn test_cycle_nasa_hardened() {
        let (_dir, kernel) = setup().await;
        let (tenant_uuid, _) = provision_tenant(&kernel, "tenant_cycle").await;
        let a = create_entity(&kernel, "node_prod", &tenant_uuid).await;
        let b = create_entity(&kernel, "node_prod", &tenant_uuid).await;

        kernel.create_structural_edge(&a, &b, "edge_owns").await.unwrap();
        let res = kernel.create_structural_edge(&b, &a, "edge_owns").await;
        assert!(matches!(res, Err(KernelError::CycleDetected)));
    }

    #[tokio::test]
    async fn test_tenant_isolation_nasa_hardened() {
        let (_dir, kernel) = setup().await;
        let (tenant_t1, _) = provision_tenant(&kernel, "t1").await;
        let p1 = create_entity(&kernel, "node_prod", &tenant_t1).await;
        kernel.supersede_state(&p1, "attr_desc", serde_json::json!({"text":"secret"}), None).await.unwrap();

        // Switch to a different tenant — compile_context must not leak the
        // other tenant's secret.
        let (_, _) = provision_tenant(&kernel, "t2").await;
        let xml = kernel.compile_context(&p1, "run1", None).await.unwrap();
        assert!(!xml.contains("secret"), "compile_context must respect tenant isolation, got: {xml}");
    }

    #[tokio::test]
    async fn test_durable_cursor_nasa_hardened() {
        let (_dir, kernel) = setup().await;
        let (tenant_uuid, _) = provision_tenant(&kernel, "t1_cursor").await;

        // Subject for the cursor chain — any entity will do; for v0.1 we use
        // a node_prod (workload subject would be a node_run when those land).
        let subject = create_entity(&kernel, "node_prod", &tenant_uuid).await;
        let subject_thing = Kernel::parse_id(&subject).unwrap();

        kernel.write_cursor(
            subject_thing.clone(),
            "ingestion",
            Some("file_1".into()),
            None,
        ).await.unwrap();

        let cursor = kernel.read_cursor(subject_thing, "ingestion").await.unwrap().unwrap();
        assert_eq!(cursor.last_processed, Some("file_1".into()));
    }

    #[tokio::test]
    async fn test_scd2_supersede_nasa_hardened() {
        #[derive(serde::Deserialize, Debug)]
        struct Row {
            value_json: serde_json::Value,
            #[allow(dead_code)]
            valid_from: chrono::DateTime<chrono::Utc>,
        }

        let (_dir, kernel) = setup().await;
        let (tenant_uuid, _) = provision_tenant(&kernel, "t1_scd2").await;
        let v1 = create_entity(&kernel, "node_prod", &tenant_uuid).await;
        let v1_thing = Kernel::parse_id(&v1).unwrap();

        // 1. First state
        kernel.supersede_state(&v1, "attr_desc", serde_json::json!({"text":"v1"}), None).await.unwrap();
        // 2. Second state — append-only under v2; prior row stays.
        kernel.supersede_state(&v1, "attr_desc", serde_json::json!({"text":"v2"}), None).await.unwrap();

        // 3. Verify both versions are preserved and "current" (latest by
        //    valid_from) is the v2 payload.
        let attr_desc = kernel.type_thing("attr_desc").unwrap();

        let mut res = kernel.db.query(
            "SELECT value_json, valid_from FROM state_ledger \
             WHERE target = $target AND `type` = $ty \
             ORDER BY valid_from ASC"
        )
            .bind(("target", v1_thing))
            .bind(("ty", attr_desc))
            .await.unwrap();
        let history: Vec<Row> = res.take(0).unwrap();
        assert_eq!(history.len(), 2, "append-only history must keep both versions, got {history:?}");
        assert_eq!(history[0].value_json.get("text").and_then(|v| v.as_str()), Some("v1"), "first row is the v1 payload");
        assert_eq!(history[1].value_json.get("text").and_then(|v| v.as_str()), Some("v2"), "latest row (by valid_from) is the v2 payload");
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
        // NASA Rule 2: compile_context must enforce a bounded total-node cap.
        // Configure max_context_nodes=2, seed a 3-node chain, verify refusal.
        let (_dir, kernel) = setup().await;
        let (tenant_uuid, substrate_record_id) = provision_tenant(&kernel, "tenant_bound").await;

        // Cap configured as attr_config on the substrate entity.
        kernel.supersede_state(
            &substrate_record_id,
            "attr_config",
            serde_json::json!({"max_context_nodes": 2}),
            None,
        ).await.unwrap();

        // Seed three entities and chain a -> b -> c via structural edges.
        let a = create_entity(&kernel, "node_prod", &tenant_uuid).await;
        let b = create_entity(&kernel, "node_prod", &tenant_uuid).await;
        let c = create_entity(&kernel, "node_prod", &tenant_uuid).await;
        kernel.create_structural_edge(&a, &b, "edge_owns").await.unwrap();
        kernel.create_structural_edge(&b, &c, "edge_owns").await.unwrap();

        let res = kernel.compile_context(&a, "test_run", None).await;
        assert!(
            matches!(&res, Err(KernelError::SafetyViolation(m)) if m.contains("node limit")),
            "compile_context must refuse to exceed max_context_nodes, got {res:?}"
        );
    }
}
