-- Test fixture (issue #158): sabotage the module ledger so appends
-- are refused — simulating an older/incompatible substrate. Applied
-- ONLY by tests standing in for operator action on a mem engine.
REMOVE TABLE module;
DEFINE TABLE module SCHEMAFULL
    PERMISSIONS
        FOR select FULL
        FOR create FULL
        FOR update NONE
        FOR delete NONE;
DEFINE FIELD blocker ON TABLE module TYPE string;
