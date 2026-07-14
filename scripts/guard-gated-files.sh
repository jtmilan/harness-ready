#!/usr/bin/env bash
# ────────────────────────────────────────────────────────────────
# grep-guard: enforce that combined worktree diffs touch NO gated
# files (persona/SSOT code).  Fails with a diff when violated.
#
# Invocation (from repo root):
#   ./scripts/guard-gated-files.sh [--list-worktrees]
#
# Gated:
#   core/roles/src/lib.rs          -- typed agent-role persona SSOT
#   core/supervisor/src/lib.rs     -- persona-injection gate
#   any .rs under core/ or agent-teams-mcp/src/
#     whose content references "persona" or "SSOT"
#
# ────────────────────────────────────────────────────────────────
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
WORKTREE_BASE="${REPO_ROOT}/.agent-teams-worktrees"

GATED_EXACT=(
  "core/roles/src/lib.rs"
  "core/supervisor/src/lib.rs"
)

# ── resolve panes ──────────────────────────────────────────────
if [[ "${1:-}" == "--list-worktrees" ]]; then
  ls -1 "$WORKTREE_BASE" 2>/dev/null || echo "(no worktrees dir)"
  exit 0
fi

panes=()
for d in "$WORKTREE_BASE"/ws*-p*; do
  [ -d "$d" ] || continue
  panes+=("$(basename "$d")")
done

if [ ${#panes[@]} -eq 0 ]; then
  echo "OK  (no agent-teams worktrees found)" >&2
  exit 0
fi

# ── collect touched paths ──────────────────────────────────────
declare -A TOUCHED
ANY_VIOLATION=0

for pane in "${panes[@]}"; do
  wd="$WORKTREE_BASE/$pane"
  if ! cd "$wd" 2>/dev/null; then
    echo "WARN  cannot cd to $wd, skipping" >&2
    continue
  fi

  while IFS=$'\t' read -r status path rest; do
    [ -z "$path" ] && continue
    # For rename lines (Rnnn) take the dest path (second field)
    [[ "$status" == R* ]] && { path="$rest"; }
    # Strip leading status char for simple status lines
    path="${path#M }"; path="${path#A }"; path="${path#D }"
    [ -z "$path" ] && continue

    if [ -z "${TOUCHED[$path]:-}" ]; then
      TOUCHED[$path]="$pane"
    else
      TOUCHED[$path]="${TOUCHED[$path]},$pane"
    fi
  done < <(git diff HEAD --name-status 2>/dev/null || true)
done

cd "$REPO_ROOT"

# ── check exact paths ──────────────────────────────────────────
for gated in "${GATED_EXACT[@]}"; do
  if [ -n "${TOUCHED[$gated]:-}" ]; then
    echo "VIOLATION  $gated touched by pane(s): ${TOUCHED[$gated]}" >&2
    ANY_VIOLATION=1
    git diff HEAD -- "$gated" 2>/dev/null | head -40 || true
  fi
done

# ── check persona/SSOT content in .rs diffs ────────────────────
for f in "${!TOUCHED[@]}";  do
  case "$f" in
    core/*.rs|agent-teams-mcp/src/*.rs) ;;
    *) continue ;;
  esac
  full="$REPO_ROOT/$f"
  [ -f "$full" ] || continue
  if grep -qE 'persona|SSOT' "$full" 2>/dev/null; then
    echo "VIOLATION  $f (contains persona/SSOT code) touched by pane(s): ${TOUCHED[$f]}" >&2
    ANY_VIOLATION=1
  fi
done

# ── verdict ────────────────────────────────────────────────────
if [ "$ANY_VIOLATION" -ne 0 ]; then
  echo "" >&2
  echo "✗ GATE BLOCKED — gated files touched" >&2
  exit 1
fi

echo "OK  no gated files touched across ${#panes[@]} pane(s)" >&2
exit 0
