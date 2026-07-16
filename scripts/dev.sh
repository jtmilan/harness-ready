#!/usr/bin/env bash
# Launch Harness Ready (standalone) in dev. Fully isolated from ~/Personal/agent-teams:
#   - identifier com.jeffrymilan.harnessready.dev (tauri.dev.conf.json)
#   - AGENT_TEAMS_STATE_DIR → this dev run's OWN state dir. The default state root is now
#     fork-private (~/Library/Application Support/harness-ready/agent-teams — isolated from
#     production agent-teams), but a dev run must ALSO not share the socket + live registry
#     siblings (which land in the state dir's PARENT) with the INSTALLED fork app — so nest
#     under harness-ready-dev/. Passed EXPLICITLY (a stale inherited value would defeat
#     a ${VAR:-default} fallback — that env-leak reaped a fleet before).
set -euo pipefail
cd "$(dirname "$0")/../app"

export AGENT_TEAMS_STATE_DIR="$HOME/Library/Application Support/harness-ready-dev/state"
mkdir -p "$AGENT_TEAMS_STATE_DIR"

echo "[harness-ready] identifier : com.jeffrymilan.harnessready.dev"
echo "[harness-ready] state dir  : $AGENT_TEAMS_STATE_DIR"
echo "[harness-ready] frontend   : ../../ui (vite :5173) → ../../ui/dist"

exec ./node_modules/.bin/tauri dev --config src-tauri/tauri.dev.conf.json
