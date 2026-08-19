-- =========================================================================
-- superx-mod-hello — the reference module's OWN schema
-- =========================================================================
-- Applied into the module's OWN database (superx/hello) by
--   superx modules provision hello
-- under the operator's one-shot root session. The KERNEL schema is
-- untouched (§7). This file is the template contributors copy.

DEFINE USER IF NOT EXISTS superx_mod_hello ON DATABASE
    PASSWORD '$SUPERX_MODULE_PASSWORD'
    ROLES EDITOR
    DURATION FOR TOKEN 1h, FOR SESSION NONE;

-- The module's one data object: greetings it has spoken.
DEFINE TABLE greeting SCHEMAFULL
    PERMISSIONS
        FOR select FULL
        FOR create FULL
        FOR update NONE
        FOR delete NONE;

DEFINE FIELD text       ON TABLE greeting TYPE string;
DEFINE FIELD valid_from ON TABLE greeting TYPE datetime;
