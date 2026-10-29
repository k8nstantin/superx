#!/usr/bin/env bash
# =========================================================================
# tools/deploy_recon.sh — the recon deploy in one script, operator-only
# =========================================================================
#
# Runs specs/recon/root.xml's method deploy, in its order, from a checkout
# of one commit: builds superx-recon, installs it, stops what runs,
# provisions superx/recon when asked, starts superx-recon and prints its
# status. superx-recon took superx-performance's place at t01 (decision
# retire-performance), so superx-performance is stopped too while it is
# installed; a stop of one that does not run says so and does nothing.
#
# Run it in a terminal of your own, never through an agent: --initialize
# asks for the root password at that terminal, and nothing else sees it.
#
# Every build it installs is kept as <home>/backup/superx-recon-<commit>,
# so the build a deploy replaces is already there under its own commit.
#
# Usage:
#   tools/deploy_recon.sh [--initialize] [checkout]
#     checkout       the folder checked out at the commit to deploy, with
#                    no uncommitted change (default: this script's own)
#     --initialize   provision superx/recon from the structure first: only
#                    when the task file says so (root method deploy, 3)
#
# Optional environment variables (defaults shown):
#   SUPERX_HOME=~/.superx      the instance's home
#   SUPERX_BIN=~/.local/bin    the folder its executables are installed in
# =========================================================================

set -euo pipefail

INITIALIZE=no
CHECKOUT="$( cd "$( dirname "${BASH_SOURCE[0]}" )/.." && pwd )"
for arg in "$@"; do
    case "$arg" in
        --initialize) INITIALIZE=yes ;;
        -*) echo "ERROR: unknown option $arg" >&2; exit 2 ;;
        *) CHECKOUT="$( cd "$arg" && pwd )" ;;
    esac
done

: "${SUPERX_HOME:=$HOME/.superx}"
: "${SUPERX_BIN:=$HOME/.local/bin}"
RECON="$SUPERX_BIN/superx-recon"
PERFORMANCE="$SUPERX_BIN/superx-performance"

if [ "$INITIALIZE" = yes ] && [ ! -t 0 ]; then
    echo "ERROR: --initialize asks for the root password at a terminal: run this in a terminal of your own." >&2
    exit 2
fi
if ! git -C "$CHECKOUT" diff --quiet HEAD --; then
    echo "ERROR: $CHECKOUT has uncommitted changes; a deploy is the build of one commit." >&2
    exit 1
fi
COMMIT="$( git -C "$CHECKOUT" rev-parse --short HEAD )"

echo "→ build superx-recon at $COMMIT, in $CHECKOUT"
( cd "$CHECKOUT" && cargo build --release -p superx-mod-recon )

echo "→ install it in $SUPERX_BIN, kept as $SUPERX_HOME/backup/superx-recon-$COMMIT"
mkdir -p "$SUPERX_HOME/backup"
cp -p "$CHECKOUT/target/release/superx-recon" "$SUPERX_HOME/backup/superx-recon-$COMMIT"
cp "$CHECKOUT/target/release/superx-recon" "$RECON.new" && mv -f "$RECON.new" "$RECON"

echo "→ stop what runs"
"$RECON" --home "$SUPERX_HOME" stop
if [ -x "$PERFORMANCE" ]; then
    "$PERFORMANCE" --home "$SUPERX_HOME" stop
fi

if [ "$INITIALIZE" = yes ]; then
    echo "→ provision superx/recon from the structure"
    "$RECON" --home "$SUPERX_HOME" --initialize
fi

echo "→ start superx-recon"
"$RECON" --home "$SUPERX_HOME" start
"$RECON" --home "$SUPERX_HOME" status
