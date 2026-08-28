-- =========================================================================
-- superx-mod-entities — the entity substrate's OWN schema (epic #353)
-- =========================================================================
--
-- THIS FILE IS THE SCHEMA. To change the model, change it HERE first and
-- have the operator apply it (§11): code is written against a locked
-- schema, never the other way round. Applied into the module's own
-- database `superx/entities` by `superx modules provision entities`.
-- The KERNEL schema is untouched (§7).
--
--
-- THREE TABLES ---------------------------------------------------------
--
--   entity             the uuid7. That is the whole row.
--   entity_attribute   everything said about it — its custom fields
--   entity_edge        what it points at — walkable by the engine
--
--
-- THE UUID7 IS THE IDENTITY, like a social security number. You can
-- change your name; you are still you. So the entity row holds NOTHING
-- but that number and the instant it was issued, and every edge and
-- every label in the database anchors to it. A rename cannot ripple
-- through the graph, because there is nothing in the graph to ripple
-- into — the name was never there.
--
-- EVERYTHING ELSE IS AN ATTRIBUTE, its name included:
--
--   it is called DBA    name 'name'         · content 'DBA'
--   it is a role        name 'role'         · labels [role]
--   its house rules     name 'house_rules'  · labels [mandate] · content
--   it runs Backups     an EDGE — name 'runs backups' · labels [contains]
--
-- An attribute with labels and no content is how you say what a thing
-- IS. An attribute with content is what it holds. The store does not
-- distinguish them and does not need to.
--
-- A LABEL IS AN ENTITY. `role` is a row in `entity`, and what `role`
-- MEANS is an attribute on it. There is no label table and no type
-- registry: new labels and new kinds of connection are DATA, and this
-- file never changes to add one.
--
--
-- FIVE DATATYPES, AND THAT IS THE WHOLE OF IT ------------------------
--
--   text      → the editor. Always. Big, and resizable.
--   json      → the SAME editor. Only the storage differs.
--   number    → a small numeric box
--   boolean   → a checkbox
--   datetime  → a box just wide enough for a date
--
-- ONE RENDERING PER DATATYPE, and two of them share it. There is no
-- widget layer, no interface setting, no type-to-control mapping to
-- configure. An email is `text` with a label that says `email`; a url is
-- `text` with a label that says `url`; a price is `number` with a label
-- that says what it is. The LABEL carries the meaning — that is what
-- labels are for — and adding twenty-five field kinds would be
-- twenty-five ways for data to be wrong in exchange for nothing.
--
-- WHY `json` EXISTS AT ALL, given it edits like text. Because the
-- database can see INSIDE it and cannot see inside a string. Probed:
-- stored as `json`, `content.replicas` reads 3 and
-- `WHERE content.replicas > 2` finds the row; the identical bytes stored
-- as `text` return NONE and match nothing. It is not for a person typing
-- into a form — it is for what an agent writes back, so structured
-- output stays queryable instead of becoming a lump that has to be
-- re-parsed forever.
--
-- `text` IS ALWAYS THE SAME EDITOR. Two text attributes on one entity
-- get the same control; one is a description and one is a comment
-- because of the label they carry, never because of a different widget.
--
-- FIELDS STACK IN THE ORDER THEY WERE ADDED, and that costs nothing to
-- store: uuid7 is time-ordered, so creation order IS the record id
-- order. Add a description and get an editor; add another text labelled
-- `comment` and it sits below it; add a date and get a small box under
-- that. No ordering column, no layout step, no screen to build.
--
-- `options` is where a resize lands, and anything else one particular
-- field wants. Absent in the ordinary case, which is nearly all of them.
--
-- WHAT THE STORE CANNOT DO FOR US. `content` is `any`, so the engine
-- cannot check that a `number` holds a number — the warning every
-- entity-attribute-value model comes with, and the way they rot. The
-- WRITE PATH must validate content against datatype on every write.
-- That is not a nicety here; it is the only thing standing where a
-- column type normally stands.
--
--
-- WHAT THIS MODULE ENFORCES, and it is only this: every label named must
-- exist, and no edge may point at an entity that does not. What a role
-- does, whether a mandate binds, which links are worth following — the
-- reader's judgement. Nothing here knows who the reader is.
--
--
-- HOW CHANGE WORKS -----------------------------------------------------
--
-- Nothing is overwritten and nothing is removed (§10). A rename, a
-- relabel, a delete and an undo are the same act: APPEND A NEW ROW.
-- Current state is the newest row of a chain; delete is `active = false`
-- and undo is the next row saying `true` again.
--
-- Attributes and edges chain on `uid` — their own identity across their
-- own versions. The entity does not chain at all: it has nothing that
-- can change.
--
--
-- WHAT THE ENGINE TAUGHT US, probed rather than assumed ----------------
--
--   * `record<entity>` is a TYPE check, NOT a foreign key — it will
--     store a link to something never created. Hence the ASSERTs.
--   * `content` is not called `value`: `SELECT value FROM …` collides
--     with `SELECT VALUE expr` and will not parse, and aliasing does not
--     rescue it. Every other column name here was tested.
--   * `TYPE any` takes a string, int, bool, datetime, array, an unset
--     value, and a nested object that round-trips intact.
--   * A native relation can be walked in ONE query —
--     `entity:x.{..6}->(entity_edge WHERE …)->entity` returns a
--     six-level subtree filtered by label. A link held in a column
--     cannot: it costs a round trip per level.
--   * `FOR update NONE / FOR delete NONE` does NOT bind a system user.
--     Signed in as `superx_mod_entities` itself — the account the kernel
--     uses — an UPDATE rewrote a row and a DELETE removed one. The
--     permissions are declarative intent; APPEND-ONLY IS HELD BY THE
--     CODE, which never emits UPDATE or DELETE (§10).

DEFINE USER IF NOT EXISTS superx_mod_entities ON DATABASE
    PASSWORD '$SUPERX_MODULE_PASSWORD'
    ROLES EDITOR
    DURATION FOR TOKEN 1h, FOR SESSION NONE;


-- 1 ─── entity ─────────────────────────── the identity, and nothing else ───
-- Issued once, never altered. This is what every edge and every label
-- points at, forever.

DEFINE TABLE IF NOT EXISTS entity SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD IF NOT EXISTS created_at  ON TABLE entity TYPE datetime;


-- 2 ─── entity_attribute ─────────────── everything said about it ───────
-- Its name, what it is, and what it holds. Any number, any datatype.
-- Content may be absent: an attribute that only carries labels is how
-- you say what a thing IS.

DEFINE TABLE IF NOT EXISTS entity_attribute SCHEMAFULL PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD IF NOT EXISTS uid         ON TABLE entity_attribute TYPE string;
DEFINE FIELD IF NOT EXISTS entity      ON TABLE entity_attribute TYPE record<entity>;
DEFINE FIELD IF NOT EXISTS name        ON TABLE entity_attribute TYPE string;
DEFINE FIELD IF NOT EXISTS labels      ON TABLE entity_attribute TYPE option<array<record<entity>>>
    ASSERT $value == NONE OR array::all($value, |$l| record::exists($l));
DEFINE FIELD IF NOT EXISTS datatype    ON TABLE entity_attribute TYPE string
    ASSERT $value INSIDE ['text', 'number', 'boolean', 'datetime', 'json'];
DEFINE FIELD IF NOT EXISTS content     ON TABLE entity_attribute TYPE any;
DEFINE FIELD IF NOT EXISTS options     ON TABLE entity_attribute TYPE option<object> FLEXIBLE;
DEFINE FIELD IF NOT EXISTS active      ON TABLE entity_attribute TYPE bool;
DEFINE FIELD IF NOT EXISTS valid_from  ON TABLE entity_attribute TYPE datetime;
DEFINE FIELD IF NOT EXISTS author_kind ON TABLE entity_attribute TYPE option<string>;
DEFINE FIELD IF NOT EXISTS author_uid  ON TABLE entity_attribute TYPE option<string>;
DEFINE FIELD IF NOT EXISTS via_uid     ON TABLE entity_attribute TYPE option<string>;

DEFINE INDEX IF NOT EXISTS idx_attribute_chain  ON TABLE entity_attribute FIELDS uid, valid_from;
DEFINE INDEX IF NOT EXISTS idx_attribute_entity ON TABLE entity_attribute FIELDS entity, valid_from;


-- 3 ─── entity_edge ────────────────────── what the entity points at ────
-- A native relation, so the graph can be WALKED — both directions, any
-- depth, one query. ENFORCED refuses an edge to an entity that does not
-- exist. Many edges may join the same pair, each with its own name,
-- labels and history. Unlinking appends `active = false` on the same uid.

DEFINE TABLE IF NOT EXISTS entity_edge SCHEMAFULL TYPE RELATION IN entity OUT entity ENFORCED PERMISSIONS FOR select FULL, FOR create FULL, FOR update NONE, FOR delete NONE;

DEFINE FIELD IF NOT EXISTS uid         ON TABLE entity_edge TYPE string;
DEFINE FIELD IF NOT EXISTS name        ON TABLE entity_edge TYPE string;
DEFINE FIELD IF NOT EXISTS labels      ON TABLE entity_edge TYPE option<array<record<entity>>>
    ASSERT $value == NONE OR array::all($value, |$l| record::exists($l));
DEFINE FIELD IF NOT EXISTS active      ON TABLE entity_edge TYPE bool;
DEFINE FIELD IF NOT EXISTS valid_from  ON TABLE entity_edge TYPE datetime;
DEFINE FIELD IF NOT EXISTS author_kind ON TABLE entity_edge TYPE option<string>;
DEFINE FIELD IF NOT EXISTS author_uid  ON TABLE entity_edge TYPE option<string>;
DEFINE FIELD IF NOT EXISTS via_uid     ON TABLE entity_edge TYPE option<string>;

DEFINE INDEX IF NOT EXISTS idx_edge_chain ON TABLE entity_edge FIELDS uid, valid_from;
