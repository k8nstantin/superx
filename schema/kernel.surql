-- =========================================================================
-- SuperX kernel schema — SOURCE OF TRUTH for the kernel layer
-- =========================================================================
--
-- This file is the locked DDL for the SuperX KERNEL substrate. The
-- kernel + every kernel module shares this one schema. It is applied
-- ONCE under the operator's root account on a fresh database via
-- scripts/deploy-schema.sh.
--
-- From the moment this file is applied, the kernel schema is LOCKED.
-- Any change to a DEFINE TABLE, DEFINE FIELD, DEFINE INDEX, DEFINE
-- USER, or DEFINE ACCESS statement here requires an explicit, prior,
-- per-change `Operator-approved: …` marker in the PR body that touches
-- this file. See SKILL.md §7 (Schema Immutability).
--
-- Other layers ship their own schemas in their own crates:
--   • drivers: crates/superx-driver-<name>/schema/<name>.surql
--              each with its own service account `superx_driver_<name>`
--              and its own database `superx.<name>`
--   • apps:    crates/superx-<app>/schema/<app>.surql
--              each with its own service account `superx_<app>` and its
--              own database `superx.<app>`
-- The kernel sees all of them (cross-database SELECT permissions); each
-- driver/app sees only its own DB + read-only access to kernel for
-- shared identity lookups. None of that is in FVP — only the kernel
-- schema lands in this file.
--
-- Workflow contract:
--   1. Operator applies this file ONCE under root account.
--   2. Kernel code signs in as `superx_kernel` (EDITOR role) for
--      everything thereafter. Kernel NEVER signs in as root after
--      this point.
--   3. PERMISSIONS clauses below are documentation of intent. SurrealDB's
--      EDITOR role does not honour `FOR update / FOR delete NONE` for
--      system users; the append-only invariant is enforced by KERNEL-VERB
--      DISCIPLINE — no kernel verb emits UPDATE or DELETE statements,
--      ever. See SKILL.md §10 (Database Account Separation).
--
-- Deployment model:
--   SuperX v0.1 is a SINGLE GLOBAL AGENTIC OS — one installation, one
--   operator, telemetry global across everything. No multi-tenant
--   isolation at the data layer. If multi-customer SaaS isolation is
--   added in a future release, it lands as a separate schema PR with
--   explicit operator approval. Until then, every table is global and
--   every kernel verb writes without a tenant FK.
--
-- Registry pattern (no hardcoded category tables):
--   The kernel's "what's installed" knowledge — kernel modules,
--   drivers, apps, future contributions of unknown categories — lives
--   as DATA in the 9-table substrate:
--     • node_kernel_module entities for kernel modules
--     • node_contribution entities for drivers / apps / future things
--     • attr_contribution_category captures the category string (driver,
--       app, model, quantum_thing, …) as PAYLOAD, not as schema
--     • attr_contribution_database captures which database the
--       contribution owns
--     • attr_contribution_schema_dag mirrors the contribution's DAG
--   The kernel does NOT bake "driver" or "app" as table names. Future
--   categories arrive without schema changes — just new
--   type_definition INSERTs by kernel-bootstrap.
--
-- Substitution:
--   `$SUPERX_KERNEL_PASSWORD` (below) is replaced by `envsubst` in
--   scripts/deploy-schema.sh with the operator-supplied env var value
--   at apply time. The schema file source never contains a real
--   password literal.
-- =========================================================================


-- -------------------------------------------------------------------------
-- Kernel service-account user — the kernel's only credential after
-- schema deploy. Drivers and apps each get their own service accounts
-- (`superx_driver_<name>`, `superx_<app>`) defined in their own schemas
-- when they ship. Only the kernel account lives in this file.
-- -------------------------------------------------------------------------
DEFINE USER IF NOT EXISTS superx_kernel ON DATABASE
    PASSWORD '$SUPERX_KERNEL_PASSWORD'
    ROLES EDITOR
    DURATION FOR SESSION 1h, FOR TOKEN 1h;


-- =========================================================================
-- type_definition — the metamodel
-- =========================================================================
-- Global registry of every entity / edge / attribute type known to the
-- substrate. New types are added by INSERT, never by schema change.
DEFINE TABLE type_definition SCHEMAFULL
    PERMISSIONS
        FOR select FULL
        FOR create FULL
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD uid         ON TABLE type_definition TYPE string;
DEFINE FIELD category    ON TABLE type_definition TYPE string;
DEFINE FIELD is_acyclic  ON TABLE type_definition TYPE bool;
DEFINE FIELD sch_json    ON TABLE type_definition TYPE option<string>;
DEFINE FIELD memory_tier ON TABLE type_definition TYPE string
    ASSERT $value INSIDE ['core', 'working', 'archival', 'recall'];
DEFINE FIELD valid_from  ON TABLE type_definition TYPE datetime;


-- =========================================================================
-- cursor_type — the cursor-kind metamodel
-- =========================================================================
-- Global registry of cursor categories. `workload` for run-progress
-- cursors, `telemetry` for capture-source cursors.
DEFINE TABLE cursor_type SCHEMAFULL
    PERMISSIONS
        FOR select FULL
        FOR create FULL
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD uid         ON TABLE cursor_type TYPE string;
DEFINE FIELD category    ON TABLE cursor_type TYPE string
    ASSERT $value INSIDE ['workload', 'telemetry'];
DEFINE FIELD description ON TABLE cursor_type TYPE string;
DEFINE FIELD sch_json    ON TABLE cursor_type TYPE option<string>;
DEFINE FIELD valid_from  ON TABLE cursor_type TYPE datetime;


-- =========================================================================
-- entity — identity (the persistent "who")
-- =========================================================================
-- Every product, component, task, agent, model, prompt, data-source,
-- proposal, classification, insight, run, source, …, lives here.
-- Differentiated by `type`.
DEFINE TABLE entity SCHEMAFULL
    PERMISSIONS
        FOR select FULL
        FOR create FULL
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD type        ON TABLE entity TYPE record<type_definition>
    ASSERT $value.category = 'node';
DEFINE FIELD role        ON TABLE entity TYPE string
    ASSERT $value INSIDE ['user', 'admin'];
DEFINE FIELD valid_from  ON TABLE entity TYPE datetime;


-- =========================================================================
-- relation — unified graph edges (in → out, typed)
-- =========================================================================
-- Every edge in the SuperX graph: edge_owns, edge_implements,
-- edge_classifies, edge_has_capability, etc. Edge types are themselves
-- entities (rows of type_definition with category='edge').
DEFINE TABLE relation SCHEMAFULL
    PERMISSIONS
        FOR select FULL
        FOR create FULL
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD in          ON TABLE relation TYPE record<entity>;
DEFINE FIELD out         ON TABLE relation TYPE record<entity>;
DEFINE FIELD type        ON TABLE relation TYPE record<type_definition>
    ASSERT $value.category = 'edge';
DEFINE FIELD is_acyclic  ON TABLE relation TYPE bool;
DEFINE FIELD valid_from  ON TABLE relation TYPE datetime;


-- =========================================================================
-- state_ledger — SCD-2 content (the versioned "what")
-- =========================================================================
-- Typed attribute writes. attr_desc, attr_score, attr_summary,
-- attr_classification, attr_config, attr_compile_status, … — every
-- piece of state attached to an entity is a row here. New rows
-- supersede older ones for the same (target, type) chain; "current" is
-- recovered by ORDER BY valid_from DESC LIMIT 1.
DEFINE TABLE state_ledger SCHEMAFULL
    PERMISSIONS
        FOR select FULL
        FOR create FULL
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD target      ON TABLE state_ledger TYPE record<entity>;
DEFINE FIELD type        ON TABLE state_ledger TYPE record<type_definition>
    ASSERT $value.category = 'attribute';
-- `payload` holds the typed value for this attribute write. The
-- type-specific shape (e.g. `attr_desc` requires `{ text: string }`,
-- `attr_score` requires `{ score: number }`) is documented per
-- attribute type and enforced by kernel-verb discipline at the
-- boundary, not by the engine. Mirrors the
-- `execution_params.params_json` / `schedule.metadata` pattern.
--
-- FLEXIBLE because SurrealDB SCHEMAFULL otherwise requires every
-- nested key (`payload.text`, `payload.score`, …) to be explicitly
-- DEFINE FIELD'd, which would defeat the "opaque-to-the-engine"
-- payload contract. FLEXIBLE lets the kernel verb hand any
-- well-formed object through unchanged.
DEFINE FIELD payload     ON TABLE state_ledger TYPE object FLEXIBLE;
DEFINE FIELD valid_from  ON TABLE state_ledger TYPE datetime;


-- =========================================================================
-- cursor — durable workflow / capture-source checkpoints
-- =========================================================================
-- Polymorphic subject: a node_run entity for workload cursors, a
-- node_source entity for telemetry-capture cursors. Discriminated by
-- the linked cursor_type's category. Append-only — progress updates
-- create new rows; "current" is the latest by (subject, cursor_type).
DEFINE TABLE cursor SCHEMAFULL
    PERMISSIONS
        FOR select FULL
        FOR create FULL
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD subject        ON TABLE cursor TYPE record<entity>;
DEFINE FIELD cursor_type    ON TABLE cursor TYPE record<cursor_type>;
DEFINE FIELD last_processed ON TABLE cursor TYPE option<string>;
DEFINE FIELD metadata       ON TABLE cursor TYPE option<object> FLEXIBLE;
DEFINE FIELD valid_from     ON TABLE cursor TYPE datetime;


-- =========================================================================
-- execution_params — per-run agent knobs
-- =========================================================================
-- Temperature, top_p, top_k, max_tokens, turns, branch, retry policy,
-- model_ref, … carried as an opaque JSON object so new knobs are
-- substrate writes, not schema changes. Append-only; the runner reads
-- the latest row for (run, agent) at dispatch time.
DEFINE TABLE execution_params SCHEMAFULL
    PERMISSIONS
        FOR select FULL
        FOR create FULL
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD run         ON TABLE execution_params TYPE record<entity>
    ASSERT $value.type.uid = 'node_run';
DEFINE FIELD agent       ON TABLE execution_params TYPE record<entity>
    ASSERT $value.type.uid = 'node_agent';
DEFINE FIELD params_json ON TABLE execution_params TYPE object FLEXIBLE;
DEFINE FIELD valid_from  ON TABLE execution_params TYPE datetime;


-- =========================================================================
-- schedule — pending / running / completed work items (dumb queue)
-- =========================================================================
-- A row references a target entity (the root of a compiled DAG, or a
-- sub-node within one) and says "kick this off." All scheduling
-- judgment lives elsewhere (SchedulerBlade proposals); this table is a
-- dumb FIFO. Append-only — status transitions create new rows.
DEFINE TABLE schedule SCHEMAFULL
    PERMISSIONS
        FOR select FULL
        FOR create FULL
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD run         ON TABLE schedule TYPE record<entity>
    ASSERT $value.type.uid = 'node_run';
DEFINE FIELD kind        ON TABLE schedule TYPE string;
DEFINE FIELD target      ON TABLE schedule TYPE record<entity>;
DEFINE FIELD due_at      ON TABLE schedule TYPE datetime;
DEFINE FIELD status      ON TABLE schedule TYPE string
    ASSERT $value INSIDE ['waiting', 'scheduled', 'running', 'completed', 'failed', 'awaiting_human'];
DEFINE FIELD attempt     ON TABLE schedule TYPE int
    ASSERT $value >= 0;
DEFINE FIELD depends_on  ON TABLE schedule TYPE array<record<schedule>>;
DEFINE FIELD metadata    ON TABLE schedule TYPE object FLEXIBLE;
DEFINE FIELD valid_from  ON TABLE schedule TYPE datetime;


-- =========================================================================
-- telemetry_stream — unified firehose
-- =========================================================================
-- Every kernel mutation, every dispatch decision, every state write,
-- every transition emits one row. CHANGEFEED-friendly. Subscribers
-- (TelemetrySubscriber, classifier, insight harvester, …) consume via
-- LIVE SELECT. Telemetry is GLOBAL — no tenant scoping; events are
-- observed across the entire OS.
DEFINE TABLE telemetry_stream SCHEMAFULL
    PERMISSIONS
        FOR select FULL
        FOR create FULL
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD lifecycle_event ON TABLE telemetry_stream TYPE string;
DEFINE FIELD payload         ON TABLE telemetry_stream TYPE any;
DEFINE FIELD run             ON TABLE telemetry_stream TYPE option<record<entity>>
    ASSERT $value = NONE OR $value.type.uid = 'node_run';
DEFINE FIELD valid_from      ON TABLE telemetry_stream TYPE datetime;


-- =========================================================================
-- END
-- =========================================================================
-- Schema is locked from here forward. Any modification requires a fresh
-- per-change `Operator-approved:` marker in the PR body that touches
-- this file, per SKILL.md §7.
