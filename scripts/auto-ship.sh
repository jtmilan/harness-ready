#!/usr/bin/env bash
# auto-ship.sh — the reusable "gate + PR" tail of the delegate→Bridge fix pipeline.
#
# WHAT IT AUTOMATES (so a human never runs the gate by hand):
#   1. (optional) FOLD — 3-way-merge a set of branches (e.g. the Bridge two-wave
#      pane branches `agent-teams/<pane>`) onto a fresh integration branch.
#   2. GATE — run the AUTHORITATIVE `cargo test` against one or more model-free
#      manifests. This is the same gate the Bridge synthesizer runs; it is the
#      ONLY thing standing between agent code and a PR.
#   3. SHIP — only if the gate is GREEN: push the branch and open a PR for the
#      human's final review. NEVER merges to main (a PR is reversible; an
#      unattended merge is not — see the REJECT drill that motivated this).
#
# WHY model-free manifests only: the integration tree is a fresh `git worktree
# add`, so gitignored files (e.g. app/src-tauri/models/ggml-tiny.en.bin) are
# ABSENT -> testing the app crate fails to BUILD -> a false REJECT regardless of
# correctness. Scope the gate to crates under core/* that need no such fixture.
#
# Usage:
#   scripts/auto-ship.sh \
#       --manifests core/ringbuf/Cargo.toml[,core/mcp/Cargo.toml,...] \
#       --base main \
#       --title "feat(ringbuf): ByteRing::evicted()" \
#       [--body-file <path>]            # PR body (e.g. a Bridge final.md)
#       [--branches agent-teams/ws..-p0,agent-teams/ws..-p1,...]  # fold these first
#       [--integ-branch <name>]         # name for the folded branch (default: auto)
#       [--dry-run]                     # gate only; never push/PR
#
# With no --branches it ships the CURRENT branch. The gh call uses
# `env -u GITHUB_TOKEN` (this machine's GITHUB_TOKEN is bad; the keyring works).
set -euo pipefail

MANIFESTS=""; BASE="main"; TITLE=""; BODY_FILE=""; BRANCHES=""; INTEG=""; DRY=0
CARGO="${CARGO:-/opt/homebrew/bin/cargo}"
GIT="${GIT:-/usr/bin/git}"

while [ $# -gt 0 ]; do
  case "$1" in
    --manifests)    MANIFESTS="$2"; shift 2 ;;
    --base)         BASE="$2"; shift 2 ;;
    --title)        TITLE="$2"; shift 2 ;;
    --body-file)    BODY_FILE="$2"; shift 2 ;;
    --branches)     BRANCHES="$2"; shift 2 ;;
    --integ-branch) INTEG="$2"; shift 2 ;;
    --dry-run)      DRY=1; shift ;;
    *) echo "auto-ship: unknown arg '$1'" >&2; exit 2 ;;
  esac
done

[ -n "$MANIFESTS" ] || { echo "auto-ship: --manifests is required" >&2; exit 2; }

REPO_ROOT="$("$GIT" rev-parse --show-toplevel)"
cd "$REPO_ROOT"

# ── 1. FOLD (optional) ───────────────────────────────────────────────────────
if [ -n "$BRANCHES" ]; then
  [ -n "$INTEG" ] || INTEG="auto-ship-integ-$("$GIT" rev-parse --short "$BASE")"
  echo "==> fold: integration branch '$INTEG' off '$BASE'"
  "$GIT" switch -C "$INTEG" "$BASE"
  IFS=',' read -ra _BR <<< "$BRANCHES"
  for b in "${_BR[@]}"; do
    [ -n "$b" ] || continue
    echo "    merging $b"
    if ! "$GIT" merge --no-edit "$b"; then
      echo "auto-ship: MERGE CONFLICT folding '$b' — aborting (resolve by hand, do not ship a partial merge):" >&2
      "$GIT" --no-pager diff --name-only --diff-filter=U >&2 || true
      "$GIT" merge --abort || true
      exit 1
    fi
  done
fi

BRANCH="$("$GIT" rev-parse --abbrev-ref HEAD)"
if [ "$BRANCH" = "$BASE" ] || [ "$BRANCH" = "HEAD" ]; then
  echo "auto-ship: refusing to ship from '$BRANCH' — work on a feature branch (or pass --branches)" >&2
  exit 2
fi

# ── 2. GATE: authoritative cargo test (model-free manifests only) ────────────
echo "==> gate: cargo test across [$MANIFESTS]"
IFS=',' read -ra _MF <<< "$MANIFESTS"
for m in "${_MF[@]}"; do
  [ -n "$m" ] || continue
  if [ ! -f "$m" ]; then
    echo "auto-ship: manifest not found: $m — aborting (a missing manifest is a non-gate, never a silent pass)" >&2
    exit 1
  fi
  echo "    cargo test --manifest-path $m"
  if ! "$CARGO" test --manifest-path "$m"; then
    echo "auto-ship: GATE FAILED on $m — NOT shipping. Fix the failures and re-run." >&2
    exit 1
  fi
done
echo "==> gate GREEN"

if [ "$DRY" = "1" ]; then
  echo "==> --dry-run: gate passed; skipping push + PR."
  exit 0
fi

# ── 3. SHIP: push + open a PR (never merges to main) ─────────────────────────
[ -n "$TITLE" ] || { echo "auto-ship: --title is required to open a PR (or use --dry-run)" >&2; exit 2; }
echo "==> push: origin $BRANCH"
"$GIT" push -u origin "$BRANCH"

echo "==> open PR (base: $BASE)"
PR_ARGS=(pr create --base "$BASE" --head "$BRANCH" --title "$TITLE")
if [ -n "$BODY_FILE" ] && [ -f "$BODY_FILE" ]; then
  PR_ARGS+=(--body-file "$BODY_FILE")
else
  PR_ARGS+=(--body "Automated PR from auto-ship.sh — gate (cargo test on $MANIFESTS) is GREEN. Final human review + merge required.

🤖 Generated with [Claude Code](https://claude.com/claude-code)")
fi
# GITHUB_TOKEN on this machine is bad; the gh keyring auth works -> strip it.
env -u GITHUB_TOKEN gh "${PR_ARGS[@]}"
echo "==> done. PR opened for human review — merge is yours."
