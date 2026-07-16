#!/usr/bin/env bash
# Launch an ISOLATED dev instance of Agent Teams (the preset-wizard build) that will
# NOT affect your installed /Applications app or its data. Safe to run while the
# production app is open.
#
# Isolation:
#   - identifier  com.jeffrymilan.agentteams.dev   → its OWN localStorage (presets,
#     workspaces, recents) + its OWN macOS TCC identity, separate from production.
#   - AGENT_TEAMS_STATE_DIR → its OWN registry / run-log / live-worktree record, so the
#     app's startup state-dir wipe NEVER touches this fork's installed default
#     (~/Library/Application Support/harness-ready/agent-teams) nor production's
#     ~/Library/Application Support/agent-teams.
#
# CRITICAL: the MCP mutation socket (agent-teams-mcp.sock) and the live registry
# (agent-teams-live.json) are FIXED-NAME SIBLINGS of the state dir — i.e. they live in
# the state dir's PARENT. So the dev state dir must sit in its OWN dedicated parent
# (…/agent-teams-dev/state), NOT directly in …/Application Support — otherwise those two
# files would collide with production's and the dev app would steal/clobber them
# (spawn_socket_listener does remove_file()+bind on startup). The nested path isolates them.
#
# It runs the debug build from target/ (NOT /Applications), so it can't overwrite the
# installed app. Stop it with Ctrl-C (or close the "Agent Teams Dev" window).
set -euo pipefail

cd "$(dirname "$0")/../app"

# nested under harness-ready/agent-teams-dev/ so the socket + registry siblings land in
# harness-ready/agent-teams-dev/ — private to this fork's dev instance (NOT harness-ready/,
# where the installed fork app's siblings live, and NOT the prod repo's agent-teams-dev/)
export AGENT_TEAMS_STATE_DIR="${AGENT_TEAMS_STATE_DIR:-$HOME/Library/Application Support/harness-ready/agent-teams-dev/state}"
mkdir -p "$AGENT_TEAMS_STATE_DIR"

echo "[dev-isolated] identifier : com.jeffrymilan.agentteams.dev (separate from production)"
echo "[dev-isolated] state dir  : $AGENT_TEAMS_STATE_DIR"
echo "[dev-isolated] binary     : target/debug (NOT /Applications)"
echo "[dev-isolated] launching… first run may recompile the app crate for the dev identifier."

exec ./node_modules/.bin/tauri dev --config src-tauri/tauri.dev.conf.json
