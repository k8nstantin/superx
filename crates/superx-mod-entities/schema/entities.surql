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

-- ----------------------------------------------------------- dictionary --
-- The dictionary (issue #266). Types say what a thing IS; labels say what
-- a TERM MEANS. A fact without a dimension is uninterpretable, which is
-- why an edge needs a label for the same reason an entity needs a type.
--
-- One vocabulary, two kinds:
--   * slot labels — what an entity carries  (description, spec, mandate)
--   * link labels — how entities connect    (contains, depends_on, granted)
--
-- Versioned from the first row, unlike entity_type: the chain is
-- (key, label_kind) and current is the latest valid_from, so changing what
-- a label MEANS is a diffable, attributed event rather than a silent edit.
-- There is deliberately NO unique index on key — a unique index would make
-- rows create-once and forbid the version chain.
DEFINE TABLE IF NOT EXISTS label SCHEMAFULL
    PERMISSIONS
        FOR select FULL
        FOR create FULL
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD IF NOT EXISTS key             ON TABLE label TYPE string;
DEFINE FIELD IF NOT EXISTS label_kind      ON TABLE label TYPE string
    ASSERT $value INSIDE ['slot', 'link'];
DEFINE FIELD IF NOT EXISTS display         ON TABLE label TYPE string;
-- How a reader must TREAT it. Closed vocabularies, because code acts on
-- this and free text cannot be acted on reliably.
--   slot: binding | directive | context | guidance | dialogue | data | secret
--   link: composition | ordering | sequence | reach | reference | governance
DEFINE FIELD IF NOT EXISTS semantics       ON TABLE label TYPE string;
DEFINE FIELD IF NOT EXISTS description     ON TABLE label TYPE option<string>;
-- Prose for the model, injected when the field is presented.
DEFINE FIELD IF NOT EXISTS agent_note      ON TABLE label TYPE option<string>;
-- Slot labels only. `value_kind` decides STORAGE: prose kinds live as note
-- chains keyed by the label; value kinds live in the attributes bag.
DEFINE FIELD IF NOT EXISTS value_kind      ON TABLE label TYPE option<string>;
DEFINE FIELD IF NOT EXISTS cardinality     ON TABLE label TYPE option<string>;
DEFINE FIELD IF NOT EXISTS allowed_sources ON TABLE label TYPE option<array<string>>;
DEFINE FIELD IF NOT EXISTS writable_by     ON TABLE label TYPE option<string>;
-- Link labels only. Endpoint types are what make a nonsense edge
-- impossible: without them anything links to anything.
DEFINE FIELD IF NOT EXISTS source_types    ON TABLE label TYPE option<array<string>>;
DEFINE FIELD IF NOT EXISTS target_types    ON TABLE label TYPE option<array<string>>;
DEFINE FIELD IF NOT EXISTS inverse         ON TABLE label TYPE option<string>;
DEFINE FIELD IF NOT EXISTS acyclic         ON TABLE label TYPE option<bool>;
-- Extensible like every registry row: you cannot enumerate in advance what
-- a future label needs.
DEFINE FIELD IF NOT EXISTS attributes      ON TABLE label TYPE option<object> FLEXIBLE;
-- Hidden from default views, still real and still readable. Nothing is
-- ever deleted, so a superseded label must be hideable or the dictionary
-- becomes a graveyard.
DEFINE FIELD IF NOT EXISTS archived        ON TABLE label TYPE bool;
DEFINE FIELD IF NOT EXISTS valid_from      ON TABLE label TYPE datetime;
DEFINE FIELD IF NOT EXISTS author_kind     ON TABLE label TYPE option<string>;
DEFINE FIELD IF NOT EXISTS author_uid      ON TABLE label TYPE option<string>;
DEFINE FIELD IF NOT EXISTS via_uid         ON TABLE label TYPE option<string>;

DEFINE INDEX IF NOT EXISTS idx_label_chain ON TABLE label FIELDS key, label_kind, valid_from;

-- Which types carry which slot labels, and in what order. `active = false`
-- retires the binding without erasing that it existed.
DEFINE TABLE IF NOT EXISTS type_label SCHEMAFULL
    PERMISSIONS
        FOR select FULL
        FOR create FULL
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD IF NOT EXISTS entity_type       ON TABLE type_label TYPE string;
DEFINE FIELD IF NOT EXISTS label             ON TABLE type_label TYPE string;
DEFINE FIELD IF NOT EXISTS required          ON TABLE type_label TYPE bool;
DEFINE FIELD IF NOT EXISTS display_order     ON TABLE type_label TYPE int;
-- `description` is context on a product and directive on a task: the same
-- label treated differently by what carries it.
DEFINE FIELD IF NOT EXISTS semantics_override ON TABLE type_label TYPE option<string>;
DEFINE FIELD IF NOT EXISTS active            ON TABLE type_label TYPE bool;
DEFINE FIELD IF NOT EXISTS valid_from        ON TABLE type_label TYPE datetime;
DEFINE FIELD IF NOT EXISTS author_kind       ON TABLE type_label TYPE option<string>;
DEFINE FIELD IF NOT EXISTS author_uid        ON TABLE type_label TYPE option<string>;
DEFINE FIELD IF NOT EXISTS via_uid           ON TABLE type_label TYPE option<string>;

DEFINE INDEX IF NOT EXISTS idx_type_label_chain
    ON TABLE type_label FIELDS entity_type, label, valid_from;

-- The dictionary's cache key. Per-row valid_from cannot answer "has
-- anything changed?", so every dictionary write appends a revision here;
-- a reader caches by it and revalidates with one cheap read. A run pins
-- the revision it interpreted under, so a semantics change between a run
-- and its audit is visible rather than mysterious.
DEFINE TABLE IF NOT EXISTS dictionary SCHEMAFULL
    PERMISSIONS
        FOR select FULL
        FOR create FULL
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD IF NOT EXISTS revision   ON TABLE dictionary TYPE int;
DEFINE FIELD IF NOT EXISTS reason     ON TABLE dictionary TYPE option<string>;
DEFINE FIELD IF NOT EXISTS valid_from ON TABLE dictionary TYPE datetime;

DEFINE INDEX IF NOT EXISTS idx_dictionary_rev ON TABLE dictionary FIELDS valid_from;

-- Registry rows get the model's discipline: extensible, archivable,
-- attributed. Additive and option<> throughout — rows written before
-- these fields genuinely do not have them, and pretending otherwise
-- would be a lie in the data.
DEFINE FIELD IF NOT EXISTS attributes  ON TABLE entity_type TYPE option<object> FLEXIBLE;
DEFINE FIELD IF NOT EXISTS archived    ON TABLE entity_type TYPE option<bool>;
DEFINE FIELD IF NOT EXISTS author_kind ON TABLE entity_type TYPE option<string>;
DEFINE FIELD IF NOT EXISTS author_uid  ON TABLE entity_type TYPE option<string>;
DEFINE FIELD IF NOT EXISTS via_uid     ON TABLE entity_type TYPE option<string>;
