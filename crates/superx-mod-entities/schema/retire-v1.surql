-- =========================================================================
-- Retire the pre-#353 entities shape. RUN ONCE, before provisioning.
-- =========================================================================
-- THIS FILE IS NOT OPTIONAL ON AN INSTANCE THAT EVER RAN v1.
--
-- `entities.surql` defines its tables with IF NOT EXISTS, because
-- provisioning re-applies it on every start. That means it does NOT
-- amend a table that already exists — and v1's `entity` table carries
-- `DEFINE FIELD entity_type ON TABLE entity TYPE string`, required. The
-- new code writes an anchor with only `created_at`, so on an upgraded
-- instance every single create would be refused with
--
--     Found NONE for field `entity_type`, but expected a string
--
-- and nothing would ever be creatable. Applying the new schema over the
-- old one is not an upgrade; it is a half-open door.
--
-- Applied by the operator under root, against superx/entities:
--
--   curl -s -X POST -u root:<pw> -H 'surreal-ns: superx' \
--        -H 'surreal-db: entities' \
--        --data-binary @schema/retire-v1.surql <endpoint>/sql
--
-- THIS DELETES v1 DATA. That is the decision on record (operator,
-- 2026-08-27): the rebuild carries nothing forward, and there is no
-- importer. Take a dump first if you want the old rows.
--
-- Every statement is IF EXISTS, so it is safe to re-run. It does NOT
-- tolerate a missing DATABASE — probed: against one that was never
-- provisioned every statement fails with "The database 'x' does not
-- exist". There is nothing to retire there, so skip it.

-- The tables the new shape REUSES by name. Dropped precisely because
-- their old column definitions survive `IF NOT EXISTS` and would refuse
-- the first write.
REMOVE TABLE IF EXISTS entity;
REMOVE TABLE IF EXISTS entity_attribute;
REMOVE TABLE IF EXISTS entity_edge;

-- v1's own shape, gone from the model entirely.
REMOVE TABLE IF EXISTS entity_state;
REMOVE TABLE IF EXISTS entity_type;
REMOVE TABLE IF EXISTS type_label;
REMOVE TABLE IF EXISTS note;
REMOVE TABLE IF EXISTS label;
REMOVE TABLE IF EXISTS dictionary;
REMOVE TABLE IF EXISTS attachment;
REMOVE TABLE IF EXISTS edge;
REMOVE TABLE IF EXISTS field;
