-- =========================================================================
-- SuperX substrate schema — SOURCE OF TRUTH
-- =========================================================================
--
-- This file is the locked DDL for the SuperX substrate. It is applied
-- ONCE under the operator's root account on a fresh database via
-- scripts/deploy-schema.sh.
--
-- From the moment this file is applied, the schema is LOCKED. Any change
-- to a DEFINE TABLE, DEFINE FIELD, DEFINE INDEX, DEFINE USER, or DEFINE
-- ACCESS statement here requires an explicit, prior, per-change
-- `Operator-approved: …` marker in the PR body that touches this file.
-- See .claude/skills/zero-trust-execution/SKILL.md §7 (Schema Immutability).
--
-- Authority:
--   * Field list + types — SUPERX_SCHEMA.md (per-table sections).
--   * Cross-reference FKs + ASSERTs — SUPERX_SCHEMA.md
--     ("Cross-reference graph" table).
--   * Enum-style value-domain ASSERTs — SUPERX_SCHEMA.md
--     ("Additional enum-style ASSERTs" table).
--   * NOT-NULL / option<> contract — SUPERX_SCHEMA.md
--     ("NOT NULL contract" table).
--   * Service-account model — SUPERX_SCHEMA.md ("Database users + access")
--     and SKILL.md §10 / §13.
--
-- Workflow contract:
--   1. Operator applies this file ONCE under root account.
--   2. Model code signs in as `superx` (EDITOR role) for everything
--      thereafter. Model NEVER signs in as root after this point.
--   3. PERMISSIONS clauses below are documentation of intent. SurrealDB's
--      EDITOR role does not honour `FOR update / FOR delete NONE` for
--      system users; the append-only invariant is enforced by KERNEL-VERB
--      DISCIPLINE — no kernel verb emits UPDATE or DELETE statements,
--      ever. See SKILL.md §10 (Database Account Separation).
--
-- Substitution:
--   `$SUPERX_SERVICE_PASSWORD` (below) is replaced by `envsubst` in
--   scripts/deploy-schema.sh with the operator-supplied env var value
--   at apply time. The schema file source never contains a real
--   password literal.
-- =========================================================================


-- -------------------------------------------------------------------------
-- Service-account user — the model's only credential after schema deploy.
-- -------------------------------------------------------------------------
--
-- SurrealDB built-in roles are OWNER / EDITOR / VIEWER. EDITOR is the
-- narrowest that allows row creation. Per-table PERMISSIONS narrow the
-- intended grant to SELECT + CREATE (documentation; see workflow contract
-- above for actual enforcement).
DEFINE USER IF NOT EXISTS superx ON DATABASE
    PASSWORD '$SUPERX_SERVICE_PASSWORD'
    ROLES EDITOR
    DURATION FOR SESSION 1h, FOR TOKEN 1h;


-- =========================================================================
-- type_definition — the metamodel
-- =========================================================================
-- Global registry of every entity / edge / attribute type known to the
-- substrate. Not tenant-scoped: type definitions are shared across all
-- tenants. New types are added by INSERT, never by schema change.
DEFINE TABLE type_definition SCHEMAFULL
    PERMISSIONS
        FOR select FULL
        FOR create WHERE $session_role = 'admin'
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
-- cursors, `telemetry` for capture-source cursors. Not tenant-scoped.
DEFINE TABLE cursor_type SCHEMAFULL
    PERMISSIONS
        FOR select FULL
        FOR create WHERE $session_role = 'admin'
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
-- proposal, classification, insight, run, source, tenant, …, lives here.
-- Differentiated by `type`. Multi-tenant; every entity carries a `tenant`
-- FK pointing at a node_substrate entity.
DEFINE TABLE entity SCHEMAFULL
    PERMISSIONS
        FOR select WHERE tenant.id = $session_tenant OR $session_role = 'admin'
        FOR create WHERE tenant.id = $session_tenant OR $session_role = 'admin'
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD type        ON TABLE entity TYPE record<type_definition>
    ASSERT $value.category = 'node';
DEFINE FIELD tenant      ON TABLE entity TYPE record<entity>
    ASSERT $value.type.uid = 'node_substrate';
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
        FOR select WHERE tenant.id = $session_tenant OR $session_role = 'admin'
        FOR create WHERE tenant.id = $session_tenant OR $session_role = 'admin'
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD in          ON TABLE relation TYPE record<entity>;
DEFINE FIELD out         ON TABLE relation TYPE record<entity>;
DEFINE FIELD type        ON TABLE relation TYPE record<type_definition>
    ASSERT $value.category = 'edge';
DEFINE FIELD tenant      ON TABLE relation TYPE record<entity>
    ASSERT $value.type.uid = 'node_substrate';
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
        FOR select WHERE tenant.id = $session_tenant OR $session_role = 'admin'
        FOR create WHERE tenant.id = $session_tenant OR $session_role = 'admin'
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD target      ON TABLE state_ledger TYPE record<entity>;
DEFINE FIELD type        ON TABLE state_ledger TYPE record<type_definition>
    ASSERT $value.category = 'attribute';
DEFINE FIELD tenant      ON TABLE state_ledger TYPE record<entity>
    ASSERT $value.type.uid = 'node_substrate';
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
        FOR select WHERE tenant.id = $session_tenant OR $session_role = 'admin'
        FOR create WHERE tenant.id = $session_tenant OR $session_role = 'admin'
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD subject        ON TABLE cursor TYPE record<entity>;
DEFINE FIELD tenant         ON TABLE cursor TYPE record<entity>
    ASSERT $value.type.uid = 'node_substrate';
DEFINE FIELD cursor_type    ON TABLE cursor TYPE record<cursor_type>;
DEFINE FIELD last_processed ON TABLE cursor TYPE option<string>;
DEFINE FIELD metadata       ON TABLE cursor TYPE option<object>;
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
        FOR select WHERE tenant.id = $session_tenant OR $session_role = 'admin'
        FOR create WHERE tenant.id = $session_tenant OR $session_role = 'admin'
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD run         ON TABLE execution_params TYPE record<entity>
    ASSERT $value.type.uid = 'node_run';
DEFINE FIELD tenant      ON TABLE execution_params TYPE record<entity>
    ASSERT $value.type.uid = 'node_substrate';
DEFINE FIELD agent       ON TABLE execution_params TYPE record<entity>
    ASSERT $value.type.uid = 'node_agent';
DEFINE FIELD params_json ON TABLE execution_params TYPE object;
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
        FOR select WHERE tenant.id = $session_tenant OR $session_role = 'admin'
        FOR create WHERE tenant.id = $session_tenant OR $session_role = 'admin'
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD run         ON TABLE schedule TYPE record<entity>
    ASSERT $value.type.uid = 'node_run';
DEFINE FIELD tenant      ON TABLE schedule TYPE record<entity>
    ASSERT $value.type.uid = 'node_substrate';
DEFINE FIELD kind        ON TABLE schedule TYPE string;
DEFINE FIELD target      ON TABLE schedule TYPE record<entity>;
DEFINE FIELD due_at      ON TABLE schedule TYPE datetime;
DEFINE FIELD status      ON TABLE schedule TYPE string
    ASSERT $value INSIDE ['waiting', 'scheduled', 'running', 'completed', 'failed', 'awaiting_human'];
DEFINE FIELD attempt     ON TABLE schedule TYPE int
    ASSERT $value >= 0;
DEFINE FIELD depends_on  ON TABLE schedule TYPE array<record<schedule>>;
DEFINE FIELD metadata    ON TABLE schedule TYPE object;
DEFINE FIELD valid_from  ON TABLE schedule TYPE datetime;


-- =========================================================================
-- telemetry_stream — unified firehose
-- =========================================================================
-- Every kernel mutation, every dispatch decision, every state write,
-- every transition emits one row. CHANGEFEED-friendly. Subscribers
-- (TelemetrySubscriber, classifier, insight harvester, …) consume via
-- LIVE SELECT.
DEFINE TABLE telemetry_stream SCHEMAFULL
    PERMISSIONS
        FOR select WHERE tenant.id = $session_tenant OR $session_role = 'admin'
        FOR create WHERE tenant.id = $session_tenant OR $session_role = 'admin'
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD lifecycle_event ON TABLE telemetry_stream TYPE string;
DEFINE FIELD payload         ON TABLE telemetry_stream TYPE any;
DEFINE FIELD run             ON TABLE telemetry_stream TYPE option<record<entity>>
    ASSERT $value = NONE OR $value.type.uid = 'node_run';
DEFINE FIELD tenant          ON TABLE telemetry_stream TYPE record<entity>
    ASSERT $value.type.uid = 'node_substrate';
DEFINE FIELD valid_from      ON TABLE telemetry_stream TYPE datetime;


-- =========================================================================
-- END
-- =========================================================================
-- Schema is locked from here forward. Any modification requires a fresh
-- per-change `Operator-approved:` marker in the PR body that touches
-- this file, per SKILL.md §7.
