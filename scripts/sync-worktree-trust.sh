#!/usr/bin/env bash
# sync-worktree-trust.sh — register git worktree paths as trusted projects in
# ~/.codex/config.toml so Codex CLI uses the same settings in worktrees as in
# the main repo.
#
# Mirrors the Rust module app/src-tauri/src/codex_trust.rs — same append-only,
# idempotent, fail-soft design. The app calls the Rust version on startup and
# on every worktree creation; this script is the CLI equivalent for manual use
# or CI.
#
# Usage:
#   ./scripts/sync-worktree-trust.sh [repo-path]
#
# With no argument, syncs the current directory's git worktrees.
# Safe to re-run — skips paths already present.

set -euo pipefail

CONFIG="$HOME/.codex/config.toml"
REPO="${1:-.}"

if [[ ! -f "$CONFIG" ]]; then
  echo "[codex-trust] $CONFIG not found — nothing to sync (Codex CLI will create it on first run)."
  exit 0
fi

echo "[codex-trust] syncing worktree trust entries into $CONFIG ..."

added=0
skipped=0

git -C "$REPO" worktree list --porcelain \
  | grep '^worktree ' \
  | awk '{print $2}' \
  | while IFS= read -r path; do
      # Escape the path for grep (literal match)
      escaped=$(printf '%s\n' "$path" | sed 's/[[\.*^$()+?{|]/\\&/g')
      if grep -q "projects\.\"${escaped}\"" "$CONFIG" 2>/dev/null; then
        echo "  = exists: $path"
      else
        printf '\n[projects."%s"]\ntrust_level = "trusted"\n' "$path" >> "$CONFIG"
        echo "  + added:  $path"
      fi
    done

echo "[codex-trust] done."
