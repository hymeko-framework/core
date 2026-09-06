#!/bin/sh
# Install the contract hooks into .git/hooks.
#
#   sh tools/hooks/install.sh            # install
#   sh tools/hooks/install.sh --status   # report what is installed
#   sh tools/hooks/install.sh --remove   # uninstall
#
# A hook that exists in the repository and not in .git/hooks enforces nothing,
# which is the state most "we added a pre-commit hook" commits actually leave
# behind. This script is the difference, and `--status` is how the report can
# state which of the two is true.
#
# Copies rather than symlinks: git does not follow a symlinked hook on every
# platform, and a hook that silently does not run is worse than none.
#
# The gates themselves are NOT copied into this repository. They are one
# implementation in hymeko/hymeko, pinned by revision in the hook files; this
# script installs that pinned version so the hooks take their fast path.

set -u

ROOT=$(git rev-parse --show-toplevel 2>/dev/null) || { echo "not a git checkout" >&2; exit 2; }
HOOKS_DIR=$(git rev-parse --git-path hooks)
SOURCE="$ROOT/tools/hooks"
NAMES="pre-commit commit-msg"

action="${1:---install}"

case "$action" in
    --status)
        for name in $NAMES; do
            target="$HOOKS_DIR/$name"
            if [ ! -f "$target" ]; then
                printf '%-12s NOT INSTALLED\n' "$name"
            elif cmp -s "$SOURCE/$name" "$target"; then
                printf '%-12s installed, current\n' "$name"
            else
                printf '%-12s installed, DIFFERS from tools/hooks/%s\n' "$name" "$name"
            fi
        done
        ;;
    --remove)
        for name in $NAMES; do
            target="$HOOKS_DIR/$name"
            if [ -f "$target" ] && cmp -s "$SOURCE/$name" "$target"; then
                rm -f "$target"
                printf '%-12s removed\n' "$name"
            elif [ -f "$target" ]; then
                printf '%-12s left alone — it is not the version this script installs\n' "$name"
            else
                printf '%-12s was not installed\n' "$name"
            fi
        done
        ;;
    --install|"")
        # Install the pinned tool so the hooks take their fast path. Measured:
        # ~320 ms per gate call installed, ~785 ms resolving from git each time,
        # three calls per commit. Without this the hooks still work -- they fall
        # back -- they are just slow enough to get uninstalled.
        SPEC=$(grep -m1 '^SPEC=' "$SOURCE/pre-commit" | cut -d'"' -f2)
        if command -v uv >/dev/null 2>&1 && [ -n "$SPEC" ]; then
            echo "installing the pinned gates: $SPEC"
            uv tool install --force --quiet --from "$SPEC" hymeko-contract-tools                 || echo "  (install failed; the hooks will resolve from git per call instead)" >&2
        fi

        mkdir -p "$HOOKS_DIR"
        for name in $NAMES; do
            cp "$SOURCE/$name" "$HOOKS_DIR/$name"
            chmod +x "$HOOKS_DIR/$name"
            printf '%-12s installed\n' "$name"
        done
        echo
        echo "Bypass for a single commit: git commit --no-verify"
        echo "Bypass for a justified new artifact: a DISCOVERY: line in the message"
        ;;
    *)
        echo "usage: sh tools/hooks/install.sh [--install|--status|--remove]" >&2
        exit 2
        ;;
esac
