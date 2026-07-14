#!/usr/bin/env bash
# Launch Harness Ready (standalone) in dev. Fully isolated from ~/Personal/agent-teams:
#   - identifier com.jeffrymilan.harnessready.dev (tauri.dev.conf.json)
#   - AGENT_TEAMS_STATE_DIR → harness-ready's OWN state dir. The default state root is the
#     hardcoded ".../agent-teams" name (core is not yet rebranded), so this override is MANDATORY:
#     without it, Harness Ready shares the socket + live registry with agent-teams and its startup
#     can wipe/reap the other app's fleet. Passed EXPLICITLY (a stale inherited value would defeat
#     a ${VAR:-default} fallback — that env-leak reaped a fleet before).
set -euo pipefail
cd "$(dirname "$0")/../app"

export AGENT_TEAMS_STATE_DIR="$HOME/Library/Application Support/harness-ready/state"
mkdir -p "$AGENT_TEAMS_STATE_DIR"

echo "[harness-ready] identifier : com.jeffrymilan.harnessready.dev"
echo "[harness-ready] state dir  : $AGENT_TEAMS_STATE_DIR"
echo "[harness-ready] frontend   : ../../ui (vite :5173) → ../../ui/dist"

exec ./node_modules/.bin/tauri dev --config src-tauri/tauri.dev.conf.json
