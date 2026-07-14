#!/usr/bin/env bash
# link-skills.sh — DESIGN §3.8 "Worktree references global skills"
#
# Symlink the global Claude Code skills dir into a worktree so a headless
# `claude -p` worker can REACH FOR real skills (one source of truth, referenced
# not copied — the Skill Symlink Convention).
#
#   <worktree>/.claude/skills  ->  ~/.claude/skills
#
# Idempotent: safe to run repeatedly. If the link already points at the global
# skills dir it is a no-op. A wrong/stale symlink is repaired. A real directory
# (someone copied skills in) is refused unless --force is given, so we never
# clobber non-symlink content silently.
#
# Usage:
#   link-skills.sh <worktree-path> [--skills-dir <dir>] [--force] [--quiet]
#
# Exit codes: 0 ok (linked or already correct) · 1 usage/arg error ·
#             2 refused (existing non-symlink, no --force) · 3 link failed.

set -euo pipefail

QUIET=0
FORCE=0
SKILLS_DIR="${CLAUDE_SKILLS_DIR:-$HOME/.claude/skills}"
WORKTREE=""

log() { [ "$QUIET" -eq 1 ] || printf '%s\n' "$*" >&2; }
die() { printf 'link-skills: %s\n' "$*" >&2; exit "${2:-1}"; }

while [ $# -gt 0 ]; do
  case "$1" in
    --skills-dir) SKILLS_DIR="${2:?--skills-dir needs a value}"; shift 2;;
    --force)      FORCE=1; shift;;
    --quiet)      QUIET=1; shift;;
    -h|--help)    sed -n '2,20p' "$0"; exit 0;;
    -*)           die "unknown flag: $1" 1;;
    *)            [ -z "$WORKTREE" ] && WORKTREE="$1" || die "unexpected arg: $1" 1; shift;;
  esac
done

[ -n "$WORKTREE" ] || die "missing <worktree-path> (run with -h for help)" 1
[ -d "$WORKTREE" ] || die "worktree not a directory: $WORKTREE" 1
[ -d "$SKILLS_DIR" ] || die "global skills dir not found: $SKILLS_DIR" 1

# Resolve to an absolute, canonical target so the symlink survives a cwd change.
SKILLS_ABS="$(cd "$SKILLS_DIR" && pwd -P)"
DOT_CLAUDE="$WORKTREE/.claude"
LINK="$DOT_CLAUDE/skills"

mkdir -p "$DOT_CLAUDE"

if [ -L "$LINK" ]; then
  CURRENT="$(readlink "$LINK" || true)"
  # Compare canonical targets when the current target exists.
  if [ -e "$LINK" ] && [ "$(cd "$LINK" 2>/dev/null && pwd -P)" = "$SKILLS_ABS" ]; then
    log "ok: $LINK already -> $SKILLS_ABS"
    exit 0
  fi
  log "repairing stale symlink ($CURRENT)"
  rm -f "$LINK"
elif [ -e "$LINK" ]; then
  # A real file or directory occupies the slot.
  if [ "$FORCE" -eq 1 ]; then
    log "removing existing non-symlink at $LINK (--force)"
    rm -rf "$LINK"
  else
    die "refusing to overwrite existing non-symlink: $LINK (pass --force)" 2
  fi
fi

ln -s "$SKILLS_ABS" "$LINK" || die "failed to create symlink" 3

# Verify it resolves.
[ -d "$LINK" ] || die "symlink created but does not resolve to a dir" 3
log "linked: $LINK -> $SKILLS_ABS"
exit 0
