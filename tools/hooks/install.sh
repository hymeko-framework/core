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
