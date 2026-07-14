#!/usr/bin/env bash
# serena-worktree.sh — DESIGN §3.9-B "serena (LSP-MCP) per worktree"
#
# Start ONE serena LSP-MCP server, project-pinned to a single worktree.
#
#   uvx --from git+https://github.com/oraios/serena \
#       serena start-mcp-server --context ide-assistant --project <path>
#
# THE SHARP EDGE (§3.9-B): serena is ONE active project per server process.
# A single global server does NOT fan out across worktrees — concurrent
# worktrees thrash/contaminate the active project + LSP state.
#
#   RULE: one serena server PER worktree, project-pinned.
#         NEVER a shared server multiplexing projects.
#
# The server speaks MCP over stdio, so it is meant to be spawned BY the MCP
# client (e.g. `claude mcp add ... -- <this script> <path>`) and dies with the
# session. This wrapper just resolves the project path and execs serena so the
# client owns the lifecycle.
#
# Backends: rust-analyzer (Rust) · typescript-language-server (JS/TS).
# Warm cold-start with `serena project index <path>` (see --index / README).
#
# Usage:
#   serena-worktree.sh <worktree-path>            # exec the stdio MCP server
#   serena-worktree.sh --index <worktree-path>    # warm the index, then exit
#   serena-worktree.sh --print-cmd <worktree-path># print the command, no exec
#   serena-worktree.sh --check                    # prove uvx can fetch serena
#
# Exit codes: 0 ok · 1 usage/arg error · 2 uvx missing · 3 serena check failed.

set -euo pipefail

SERENA_SPEC="git+https://github.com/oraios/serena"
MODE="serve"
WORKTREE=""

die() { printf 'serena-worktree: %s\n' "$*" >&2; exit "${2:-1}"; }

command -v uvx >/dev/null 2>&1 || die "uvx not found on PATH (install uv: https://docs.astral.sh/uv/)" 2

while [ $# -gt 0 ]; do
  case "$1" in
    --index)     MODE="index"; shift;;
    --print-cmd) MODE="print"; shift;;
    --check)     MODE="check"; shift;;
    -h|--help)   sed -n '2,30p' "$0"; exit 0;;
    -*)          die "unknown flag: $1" 1;;
    *)           [ -z "$WORKTREE" ] && WORKTREE="$1" || die "unexpected arg: $1" 1; shift;;
  esac
done

if [ "$MODE" = "check" ]; then
  # Prove uvx can fetch + launch serena WITHOUT leaving a server running.
  # `serena --help` resolves the package (downloads on first run) and exits 0.
  uvx --from "$SERENA_SPEC" serena --help >/dev/null 2>&1 \
    || die "uvx could not fetch/launch serena" 3
  echo "ok: uvx can fetch serena ($SERENA_SPEC)"
  exit 0
fi

[ -n "$WORKTREE" ] || die "missing <worktree-path> (run with -h for help)" 1
[ -d "$WORKTREE" ] || die "worktree not a directory: $WORKTREE" 1
PROJECT="$(cd "$WORKTREE" && pwd -P)"

case "$MODE" in
  index)
    # Warm rust-analyzer / tsserver cold-start so the first real query is fast.
    exec uvx --from "$SERENA_SPEC" serena project index "$PROJECT"
    ;;
  print)
    printf 'uvx --from %s serena start-mcp-server --context ide-assistant --project %q\n' \
      "$SERENA_SPEC" "$PROJECT"
    ;;
  serve)
    # ONE server, this project only. Client owns stdio + lifecycle.
    exec uvx --from "$SERENA_SPEC" serena start-mcp-server \
      --context ide-assistant --project "$PROJECT"
    ;;
esac
