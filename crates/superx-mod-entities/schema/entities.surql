-- =========================================================================
-- superx-mod-entities — the entity substrate's OWN schema (epic #353)
-- =========================================================================
--
-- THIS FILE IS THE SCHEMA. To change the model, change it HERE first and
-- have the operator apply it (§11) — code is written against a locked
-- schema, never the other way round. Applied into the module's own
-- database `superx/entities` by `superx modules provision entities`
-- under the operator's one-shot root session. The KERNEL schema is
-- untouched (§7).
--
--
-- THE MODEL ------------------------------------------------------------
--
-- EVERYTHING HAS A UUID7, A NAME, AND LABELS. A thing, a field on a
-- thing, and a link between two things are built from the same parts. A
-- field is not a lesser row with a key and a value: it has its own
-- identity, name, labels and history, exactly as the thing it hangs off.
--
--   an entity   uuid7 · 'DBA'          · labels [role]
--   a field     uuid7 · 'house_rules'  · labels [mandate]  · content
--   an edge     uuid7 · 'runs backups' · labels [contains] · the far end
--   a label     uuid7 · 'role'         · labels [label]
--
-- THERE IS NO LABEL TABLE AND NO TYPE REGISTRY. A label is an entity
-- carrying the label `label`; what it MEANS is a field on it. New labels
-- and new kinds of connection are data — this file never changes to add
-- one.
--
-- AN EDGE IS A FIELD THE ENGINE CAN WALK. It carries a name, labels,
-- history and an author like any other field; only its storage differs,
-- as a native relation, because roles and products get deep and a link
-- held in a column can only be followed one level per query. Measured on
-- this engine: `entity:x.{..6}->(edge WHERE …)->entity` walks six levels
-- filtered by label and returns the nested subtree in ONE query.
--
-- MANY LABELS, ALL OPTIONAL. A rule can be a `mandate` and a
-- `constraint` at once. Nothing is single-valued, because the moment one
-- thing is, something true cannot be written down.
--
-- DELETE IS SOFT, UNDO IS FREE. Deleting appends a version with
-- active = false; undoing appends one with active = true. A delete is
-- itself an event, with an author and a time.
--
-- WHAT THIS MODULE ENFORCES, and it is only this: every label named must
-- exist, and no edge may point at an anchor that does not. What a role
-- does, whether a mandate binds, which links are worth following — the
-- reader's judgement, and nothing here knows who the reader is.
--
--
-- WHAT THE ENGINE TAUGHT US, probed rather than assumed ----------------
--
--   * `record<entity>` is a TYPE check, NOT a foreign key — it stores a
--     link to something never created. Hence the ASSERTs.
--   * `content` is not called `value`: `SELECT value FROM …` collides
--     with `SELECT VALUE expr` and will not parse. Aliasing does not
--     rescue it. Every other column name here was tested.
--   * `TYPE any` takes a string, int, bool, datetime, array, an unset
--     value, a nested object that round-trips intact, and a record link.
--   * `FOR update NONE / FOR delete NONE` does NOT bind a system user.
--     Signed in as `superx_mod_entities` itself — the account the kernel
--     uses — an UPDATE rewrote a row and a DELETE removed one. The
--     permissions are declarative intent; APPEND-ONLY IS HELD BY THE
--     CODE, which never emits UPDATE or DELETE (§10).
--
--
-- READING IT -----------------------------------------------------------
--
-- Current state is computed, never stored: the latest `valid_from` per
-- chain, filtered to active. A raw SELECT returns the whole history —
-- both sides of a rename, both sides of an unlink. Chains are keyed by
-- `entity` on entity_state, and by `uid` on field and edge.

DEFINE USER IF NOT EXISTS superx_mod_entities ON DATABASE
    PASSWORD '$SUPERX_MODULE_PASSWORD'
    ROLES EDITOR
    DURATION FOR TOKEN 1h, FOR SESSION NONE;


-- 1 ─── entity ─────────────────────────────── the anchor, immutable ───
-- A uuid7 and when it came into being. Nothing that can ever change,
-- which is what makes it safe to point at forever.

DEFINE TABLE IF NOT EXISTS entity SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD IF NOT EXISTS created_at  ON TABLE entity TYPE datetime;


-- 2 ─── entity_state ──────────────────── what the thing is right now ───
-- Rename, relabel, delete and undo are one act: append a version.

DEFINE TABLE IF NOT EXISTS entity_state SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD IF NOT EXISTS entity      ON TABLE entity_state TYPE record<entity>;
DEFINE FIELD IF NOT EXISTS name        ON TABLE entity_state TYPE string;
DEFINE FIELD IF NOT EXISTS labels      ON TABLE entity_state TYPE option<array<record<entity>>>
    ASSERT $value == NONE OR array::all($value, |$l| record::exists($l));
DEFINE FIELD IF NOT EXISTS active      ON TABLE entity_state TYPE bool;
DEFINE FIELD IF NOT EXISTS valid_from  ON TABLE entity_state TYPE datetime;
DEFINE FIELD IF NOT EXISTS author_kind ON TABLE entity_state TYPE option<string>;
DEFINE FIELD IF NOT EXISTS author_uid  ON TABLE entity_state TYPE option<string>;
DEFINE FIELD IF NOT EXISTS via_uid     ON TABLE entity_state TYPE option<string>;

DEFINE INDEX IF NOT EXISTS idx_entity_state_chain ON TABLE entity_state FIELDS entity, valid_from;


-- 3 ─── field ────────────────────────────── what the thing holds ───────
-- Built like a thing — uuid7, name, labels — plus its owner and content.
-- `uid` is its identity across versions; changing content appends a row
-- carrying the same uid.

DEFINE TABLE IF NOT EXISTS field SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD IF NOT EXISTS uid         ON TABLE field TYPE string;
DEFINE FIELD IF NOT EXISTS entity      ON TABLE field TYPE record<entity>;
DEFINE FIELD IF NOT EXISTS name        ON TABLE field TYPE string;
DEFINE FIELD IF NOT EXISTS labels      ON TABLE field TYPE option<array<record<entity>>>
    ASSERT $value == NONE OR array::all($value, |$l| record::exists($l));
DEFINE FIELD IF NOT EXISTS datatype    ON TABLE field TYPE string;
DEFINE FIELD IF NOT EXISTS content     ON TABLE field TYPE any;
DEFINE FIELD IF NOT EXISTS active      ON TABLE field TYPE bool;
DEFINE FIELD IF NOT EXISTS valid_from  ON TABLE field TYPE datetime;
DEFINE FIELD IF NOT EXISTS author_kind ON TABLE field TYPE option<string>;
DEFINE FIELD IF NOT EXISTS author_uid  ON TABLE field TYPE option<string>;
DEFINE FIELD IF NOT EXISTS via_uid     ON TABLE field TYPE option<string>;

DEFINE INDEX IF NOT EXISTS idx_field_chain  ON TABLE field FIELDS uid, valid_from;
DEFINE INDEX IF NOT EXISTS idx_field_entity ON TABLE field FIELDS entity, valid_from;


-- 4 ─── edge ──────────────────────── what it points at, walkable ───────
-- A field stored as a native relation. ENFORCED refuses an edge to an
-- anchor that does not exist. Unlinking appends active = false on the
-- same uid; undoing appends another.

DEFINE TABLE IF NOT EXISTS edge SCHEMAFULL TYPE RELATION IN entity OUT entity ENFORCED PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD IF NOT EXISTS uid         ON TABLE edge TYPE string;
DEFINE FIELD IF NOT EXISTS name        ON TABLE edge TYPE string;
DEFINE FIELD IF NOT EXISTS labels      ON TABLE edge TYPE option<array<record<entity>>>
    ASSERT $value == NONE OR array::all($value, |$l| record::exists($l));
DEFINE FIELD IF NOT EXISTS active      ON TABLE edge TYPE bool;
DEFINE FIELD IF NOT EXISTS valid_from  ON TABLE edge TYPE datetime;
DEFINE FIELD IF NOT EXISTS author_kind ON TABLE edge TYPE option<string>;
DEFINE FIELD IF NOT EXISTS author_uid  ON TABLE edge TYPE option<string>;
DEFINE FIELD IF NOT EXISTS via_uid     ON TABLE edge TYPE option<string>;

DEFINE INDEX IF NOT EXISTS idx_edge_chain ON TABLE edge FIELDS uid, valid_from;
