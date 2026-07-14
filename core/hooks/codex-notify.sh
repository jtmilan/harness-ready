#!/usr/bin/env bash
# Agent Teams — codex turn-end → state writer.
#
# codex's ONLY turn signal is its `notify` hook. The supervisor overrides codex's
# notify PER-PANE (`-c notify=["bash","<this>","<wsid>","<state_dir>"]`) — keeping the
# user's GLOBAL notify (e.g. BridgeSpace) untouched — so a codex turn-end appends a
# `notify` event the state-adapter maps to Done/TurnEnd (codex panes otherwise have no
# turn-end signal; the supervisor's synthetic SessionStart only covers spawn/ready).
#
# Args: $1=workspace_id  $2=state_dir base.  codex appends its own JSON payload as a
# trailing arg (unused — any notify ~ a turn boundary). A hook must never abort the
# agent: swallow errors, ALWAYS exit 0. No hard python3 dependency (pure date + printf).

WSID="${1:-unknown}"
STATE_BASE="${2:-${AGENT_TEAMS_STATE_DIR:-$HOME/Library/Application Support/harness-ready/agent-teams}}"
DIR="$STATE_BASE/$WSID"
mkdir -p "$DIR" 2>/dev/null

# millisecond timestamp, no interpreter (GNU %3N, else BSD seconds*1000) — mirrors
# state-writer.sh so the line shape is byte-identical to the hook-written events.
TS="$(date +%s%3N 2>/dev/null)"
case "$TS" in
  ''|*[!0-9]*) TS="$(( $(date +%s 2>/dev/null || echo 0) * 1000 ))" ;;
esac

printf '{"ts":%s,"harness":"codex","event":"notify","workspace_id":"%s","decision":"na","payload":"{}"}\n' \
  "$TS" "$WSID" >> "$DIR/events.jsonl" 2>/dev/null

exit 0
