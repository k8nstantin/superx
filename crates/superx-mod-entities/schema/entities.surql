-- =========================================================================
-- superx-mod-entities — the entity substrate's OWN schema (epic #353)
-- =========================================================================
-- Applied into the module's OWN database (superx/entities) by
--   superx modules provision entities
-- under the operator's one-shot root session. The KERNEL schema is
-- untouched (§7). Every DEFINE carries IF NOT EXISTS because module
-- provisioning re-applies this file verbatim (no tolerant path).
--
--
--        EVERYTHING HAS A UUID7, A NAME, AND LABELS. FIELDS TOO.
--
-- A thing and a field on a thing are built from the same three parts. A
-- field is not a lesser kind of row with a key and a value — it has its
-- own identity, its own name, its own labels and its own history,
-- exactly as the thing it hangs off does.
--
--   an entity     uuid7 · name 'DBA'          · labels [role]
--   a field       uuid7 · name 'house_rules'  · labels [mandate]  · content
--   an edge       uuid7 · name 'runs on'      · labels [contains] · content = another entity
--                 (datatype 'link' — that is the whole of what makes
--                  a field an edge)
--   a label       uuid7 · name 'role'         · labels [label]
--
-- THERE IS NO EDGE TABLE AND NO LABEL TABLE. An edge is a field whose
-- content is a link to another entity; what the connection MEANS is its
-- labels. A label is an entity carrying the label `label`; what it means
-- is a field on it. Both extend by writing data, never by changing this
-- file.
--
-- MANY LABELS, EVERYWHERE, and all optional. A rule can be a `mandate`
-- and a `constraint` at once; a connection can be a `link` and a
-- `depends_on` at once. Nothing is single-valued, because the moment one
-- thing is, something true cannot be written down.
--
-- ONLY A READER KNOWS WHAT ANY OF IT MEANS. This module enforces exactly
-- two things: a label it is given must exist, and content must be what
-- its datatype says. What a role does, whether a mandate binds, which
-- links are worth following — none of that is encoded here.
--
-- DELETE IS SOFT AND UNDO IS FREE. Nothing is ever removed: deleting
-- appends a version with active = false, undoing appends one with
-- active = true, and both stay on the record. A delete is therefore
-- itself an event with an author and a timestamp, which is the point.
--
-- WHAT ACTUALLY ENFORCES THAT IS THE CODE, NOT THIS FILE. The
-- `FOR update NONE / FOR delete NONE` blocks below are declarative
-- intent and a guard against record-level users; they do NOT bind a
-- system user. Probed rather than assumed: signed in as
-- `superx_mod_entities` — the very account the kernel uses, ROLES
-- EDITOR — an UPDATE rewrote a row and a DELETE removed one, both
-- accepted. Append-only holds because no verb in this module ever emits
-- UPDATE or DELETE (§10). The permissions say what is intended; the
-- verbs are the enforcement layer.
--
-- Contract:
--   * append-only — every change is an INSERT; "current" is computed as
--     ORDER BY valid_from DESC LIMIT 1 per chain (§10)
--   * the ANCHOR is immutable, which is what lets a label reference or a
--     link be a typed record the engine can check (D20). Only entities
--     are pointed at, so only entities need one; a field versions in
--     place, chained by its own uid
--   * uuid7 record ids everywhere — time-ordered, the historical log

DEFINE USER IF NOT EXISTS superx_mod_entities ON DATABASE
    PASSWORD '$SUPERX_MODULE_PASSWORD'
    ROLES EDITOR
    DURATION FOR TOKEN 1h, FOR SESSION NONE;

-- --------------------------------------------------------------- 1 of 3 --
-- THE ANCHOR. A uuid7 and the instant it came into being — nothing that
-- can ever change, which is exactly what makes it safe for every label
-- reference and every link in the database to point at it forever.
DEFINE TABLE IF NOT EXISTS entity SCHEMAFULL
    PERMISSIONS
        FOR select FULL
        FOR create FULL
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD IF NOT EXISTS created_at ON TABLE entity TYPE datetime;

-- --------------------------------------------------------------- 2 of 3 --
-- WHAT THE THING IS RIGHT NOW: its name and its labels. Renaming,
-- relabelling, deleting and undoing are all the same act — append a new
-- version. Current is the latest valid_from for the anchor.
DEFINE TABLE IF NOT EXISTS entity_state SCHEMAFULL
    PERMISSIONS
        FOR select FULL
        FOR create FULL
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD IF NOT EXISTS entity ON TABLE entity_state TYPE record<entity>;
DEFINE FIELD IF NOT EXISTS name   ON TABLE entity_state TYPE string;
-- OPTIONAL AND MANY. Every one must exist: `record<entity>` is a TYPE
-- check and not a foreign key — probed, and it will happily store a link
-- to something never created. A dangling label reads as "this means
-- something" while there is nothing to resolve.
DEFINE FIELD IF NOT EXISTS labels ON TABLE entity_state TYPE option<array<record<entity>>>
    ASSERT $value == NONE OR array::all($value, |$l| record::exists($l));
-- Soft delete. `false` is deleted; the next version can say `true` again.
DEFINE FIELD IF NOT EXISTS active     ON TABLE entity_state TYPE bool;
DEFINE FIELD IF NOT EXISTS valid_from ON TABLE entity_state TYPE datetime;
-- WHO SAID IT. An agent labelling a thing, or linking a role to a
-- credential, IS the authorization record — not decoration on it. Plain
-- strings, because an author may live in another database entirely.
DEFINE FIELD IF NOT EXISTS author_kind ON TABLE entity_state TYPE option<string>;
DEFINE FIELD IF NOT EXISTS author_uid  ON TABLE entity_state TYPE option<string>;
DEFINE FIELD IF NOT EXISTS via_uid     ON TABLE entity_state TYPE option<string>;

DEFINE INDEX IF NOT EXISTS idx_entity_state_chain
    ON TABLE entity_state FIELDS entity, valid_from;

-- --------------------------------------------------------------- 3 of 3 --
-- THE FIELDS. Constructed exactly like a thing — uuid7, name, labels —
-- plus what they belong to and what they hold.
--
-- `uid` is the field's identity across its versions: changing its
-- content appends a NEW row carrying the same uid, and current is the
-- latest valid_from in that chain.
--
-- `content TYPE any` is what makes "any number of fields, any datatype"
-- literal. Probed on this engine rather than assumed: `any` accepts a
-- string, an int, a bool, a datetime, an array, an unset value, a nested
-- object that round-trips intact — AND a record link, which is what lets
-- a field be an edge. It is not called `value`: `SELECT value FROM …`
-- collides with SurrealDB's `SELECT VALUE expr` and will not parse, and
-- aliasing does not rescue it.
--
-- Holding nothing is a legitimate statement: a field named `is a role`
-- with no content says exactly that, and that is the whole claim.
DEFINE TABLE IF NOT EXISTS field SCHEMAFULL
    PERMISSIONS
        FOR select FULL
        FOR create FULL
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD IF NOT EXISTS uid      ON TABLE field TYPE string;
DEFINE FIELD IF NOT EXISTS entity   ON TABLE field TYPE record<entity>;
DEFINE FIELD IF NOT EXISTS name     ON TABLE field TYPE string;
DEFINE FIELD IF NOT EXISTS labels   ON TABLE field TYPE option<array<record<entity>>>
    ASSERT $value == NONE OR array::all($value, |$l| record::exists($l));
DEFINE FIELD IF NOT EXISTS datatype ON TABLE field TYPE string;
-- A LINK MUST RESOLVE, exactly as a label must. `$this` reaches the
-- sibling column, so the check applies only where it means something: a
-- field declared `link` must point at an entity that exists, and a field
-- holding prose is untouched by it. Without this a link to nothing is
-- storable, and it is the same silent wrong as a dangling label —
-- something that reads as a connection with nothing at the other end.
DEFINE FIELD IF NOT EXISTS content  ON TABLE field TYPE any
    ASSERT $this.datatype != 'link' OR record::exists($value);
DEFINE FIELD IF NOT EXISTS active     ON TABLE field TYPE bool;
DEFINE FIELD IF NOT EXISTS valid_from ON TABLE field TYPE datetime;
DEFINE FIELD IF NOT EXISTS author_kind ON TABLE field TYPE option<string>;
DEFINE FIELD IF NOT EXISTS author_uid  ON TABLE field TYPE option<string>;
DEFINE FIELD IF NOT EXISTS via_uid     ON TABLE field TYPE option<string>;

-- The chain read: latest row per uid.
DEFINE INDEX IF NOT EXISTS idx_field_chain  ON TABLE field FIELDS uid, valid_from;
-- Everything said about one thing — the read every reader makes.
DEFINE INDEX IF NOT EXISTS idx_field_entity ON TABLE field FIELDS entity, valid_from;
-- WHAT POINTS AT THIS. Following a link forwards is free — the links are
-- fields of the entity you already hold. Backwards is the question a
-- graph normally needs a second edge direction to answer, and without
-- this index it is a table scan. EXPLAIN confirms the engine takes it:
-- `WHERE content = entity:x` plans as an IndexScan on this index.
DEFINE INDEX IF NOT EXISTS idx_field_content ON TABLE field FIELDS content;
