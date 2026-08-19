-- =========================================================================
-- superx-mod-entities — the product graph substrate's OWN schema
-- =========================================================================
-- Applied into the module's OWN database (superx/entities) by
--   superx modules provision entities
-- under the operator's one-shot root session. The KERNEL schema is
-- untouched (§7). Every DEFINE carries IF NOT EXISTS because module
-- provisioning re-applies this file verbatim (no tolerant path).
--
-- Contract (epic #166; D19/D20/D22):
--   * append-only — every change is an INSERT; "current" is computed as
--     ORDER BY valid_from DESC LIMIT 1 per chain; no is_current, no
--     valid_to, ever
--   * entity rows are IMMUTABLE ANCHORS (one row per entity, forever)
--     so native graph edges always point at a stable record
--   * edges are SurrealDB native relations written by RELATE (creation
--     only, D19); unlink appends a new row on the same edge_uid with
--     active = false — the full link history is the table
--   * uuid7 record ids everywhere — time-ordered, the historical log

DEFINE USER IF NOT EXISTS superx_mod_entities ON DATABASE
    PASSWORD '$SUPERX_MODULE_PASSWORD'
    ROLES EDITOR
    DURATION FOR TOKEN 1h, FOR SESSION NONE;

-- ---------------------------------------------------------------- types --
-- Types-as-data: node kinds (category 'entity') and edge kinds (category
-- 'relation') are ROWS, extensible at runtime with no DDL. Create-once
-- rows guarded by the UNIQUE name index.
DEFINE TABLE IF NOT EXISTS entity_type SCHEMAFULL
    PERMISSIONS
        FOR select FULL
        FOR create FULL
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD IF NOT EXISTS name        ON TABLE entity_type TYPE string;
DEFINE FIELD IF NOT EXISTS category    ON TABLE entity_type TYPE string
    ASSERT $value INSIDE ['entity', 'relation'];
DEFINE FIELD IF NOT EXISTS description ON TABLE entity_type TYPE option<string>;
DEFINE FIELD IF NOT EXISTS valid_from  ON TABLE entity_type TYPE datetime;

DEFINE INDEX IF NOT EXISTS idx_entity_type_name ON TABLE entity_type FIELDS name UNIQUE;

-- -------------------------------------------------------------- anchors --
-- One immutable row per entity, forever (D20). The uuid7 record id is
-- the entity's identity; everything that can change lives in
-- entity_state chains. Anchors are what edges point at.
DEFINE TABLE IF NOT EXISTS entity SCHEMAFULL
    PERMISSIONS
        FOR select FULL
        FOR create FULL
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD IF NOT EXISTS entity_type ON TABLE entity TYPE string;
DEFINE FIELD IF NOT EXISTS created_at  ON TABLE entity TYPE datetime;

-- --------------------------------------------------------------- states --
-- SCD-2 version rows: one chain per entity. `name` is the one-line
-- label lists and trees render from; `content` is the unbounded text
-- payload of content-bearing nodes (chiefly type 'text' — D22);
-- `attributes` is the FLEXIBLE per-type JSON bag (SCHEMAFULL would
-- otherwise demand a DEFINE FIELD per nested key).
DEFINE TABLE IF NOT EXISTS entity_state SCHEMAFULL
    PERMISSIONS
        FOR select FULL
        FOR create FULL
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD IF NOT EXISTS entity     ON TABLE entity_state TYPE record<entity>;
DEFINE FIELD IF NOT EXISTS name       ON TABLE entity_state TYPE string;
DEFINE FIELD IF NOT EXISTS content    ON TABLE entity_state TYPE option<string>;
DEFINE FIELD IF NOT EXISTS attributes ON TABLE entity_state TYPE option<object> FLEXIBLE;
DEFINE FIELD IF NOT EXISTS valid_from ON TABLE entity_state TYPE datetime;

DEFINE INDEX IF NOT EXISTS idx_entity_state_chain ON TABLE entity_state FIELDS entity, valid_from;

-- ---------------------------------------------------------------- edges --
-- Native graph relation (D19): the engine stores record pointers, so
-- traversal (->edge->entity) costs a node's degree, never the table
-- size. ENFORCED: edges to non-existent anchors are refused. Link =
-- RELATE with a fresh uuid7 edge_uid; unlink = a NEW row, same
-- edge_uid, active = false; current edge state = latest per edge_uid.
DEFINE TABLE IF NOT EXISTS edge SCHEMAFULL TYPE RELATION IN entity OUT entity ENFORCED
    PERMISSIONS
        FOR select FULL
        FOR create FULL
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD IF NOT EXISTS edge_uid   ON TABLE edge TYPE string;
DEFINE FIELD IF NOT EXISTS rel_type   ON TABLE edge TYPE string;
DEFINE FIELD IF NOT EXISTS active     ON TABLE edge TYPE bool;
DEFINE FIELD IF NOT EXISTS valid_from ON TABLE edge TYPE datetime;

DEFINE INDEX IF NOT EXISTS idx_edge_chain ON TABLE edge FIELDS edge_uid, valid_from;
