#!/usr/bin/env bash
# =========================================================================
# scripts/deploy-schema.sh — operator-only schema apply
# =========================================================================
#
# Applies schema/superx.surql to a running SurrealDB instance under the
# operator's ROOT credentials. Run ONCE on a fresh substrate. From that
# moment forward the schema is locked; any subsequent change requires an
# `Operator-approved:` marker in the PR body that modifies the schema.
# (See .claude/skills/zero-trust-execution/SKILL.md §7.)
#
# This script is operator-owned. The model never invokes root and never
# runs this in an automated path. It exists as the documented one-shot
# the operator runs the first time a SuperX substrate is provisioned.
#
# Prerequisites:
#   1. surreal CLI installed
#      ( curl --proto '=https' --tlsv1.2 -sSf https://install.surrealdb.com | sh )
#   2. SurrealDB server running. Typical local form:
#        surreal start \
#            --user root --pass "$SUPERX_ROOT_PASSWORD" \
#            rocksdb:./db/superx.db
#   3. envsubst available (gettext package on macOS via Homebrew).
#   4. Environment variables set:
#        SUPERX_ROOT_PASSWORD       root account password (operator-only)
#        SUPERX_SERVICE_PASSWORD    becomes the model service-account
#                                   password ('superx' user). The model
#                                   reads this same env var at signin.
#
# Optional environment variables (defaults shown):
#        SUPERX_DB_ENDPOINT=http://localhost:8000
#        SUPERX_NS=superx
#        SUPERX_DB=v01
#
# Usage:
#   export SUPERX_ROOT_PASSWORD='<your root pwd>'
#   export SUPERX_SERVICE_PASSWORD='<your service pwd>'
#   ./scripts/deploy-schema.sh
# =========================================================================

set -euo pipefail

SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
REPO_ROOT="$( cd "$SCRIPT_DIR/.." && pwd )"
SCHEMA_FILE="$REPO_ROOT/schema/superx.surql"

: "${SUPERX_ROOT_PASSWORD:?env var SUPERX_ROOT_PASSWORD must be set (root account)}"
: "${SUPERX_SERVICE_PASSWORD:?env var SUPERX_SERVICE_PASSWORD must be set (model service account)}"

: "${SUPERX_DB_ENDPOINT:=http://localhost:8000}"
: "${SUPERX_NS:=superx}"
: "${SUPERX_DB:=v01}"

if ! command -v surreal >/dev/null 2>&1; then
    echo "ERROR: surreal CLI not found in PATH." >&2
    echo "       Install: curl --proto '=https' --tlsv1.2 -sSf https://install.surrealdb.com | sh" >&2
    exit 1
fi

if ! command -v envsubst >/dev/null 2>&1; then
    echo "ERROR: envsubst not found in PATH (provided by gettext)." >&2
    echo "       macOS:  brew install gettext && brew link --force gettext" >&2
    echo "       Linux:  apt-get install gettext-base   (Debian/Ubuntu)" >&2
    exit 1
fi

if [ ! -f "$SCHEMA_FILE" ]; then
    echo "ERROR: Schema file not found: $SCHEMA_FILE" >&2
    exit 1
fi

echo "→ SuperX schema deploy"
echo "   endpoint: $SUPERX_DB_ENDPOINT"
echo "   ns / db:  $SUPERX_NS / $SUPERX_DB"
echo "   source:   $SCHEMA_FILE"
echo "   account:  root (operator)"
echo

# envsubst is given an explicit allow-list of variables to substitute,
# so any dollar-sign tokens in the SurrealQL that are not in the list
# (e.g. PERMISSIONS expressions using $session_tenant) pass through
# untouched.
export SUPERX_SERVICE_PASSWORD
envsubst '$SUPERX_SERVICE_PASSWORD' < "$SCHEMA_FILE" | \
    surreal sql \
        --endpoint "$SUPERX_DB_ENDPOINT" \
        --username root --password "$SUPERX_ROOT_PASSWORD" \
        --auth-level root \
        --namespace "$SUPERX_NS" --database "$SUPERX_DB" \
        --pretty

echo
echo "✓ Schema applied. Substrate is locked at the engine layer."
echo "  The 'superx' service account exists with EDITOR role + 1h session."
echo "  All subsequent SuperX code MUST sign in as 'superx' (never root)."
echo "  Append-only invariant is enforced by kernel-verb discipline"
echo "  (no kernel verb emits UPDATE or DELETE). See SKILL.md §10 / §13."
