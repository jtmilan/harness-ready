#!/usr/bin/env bash
# Agent Teams — per-workspace state writer + thin allow/defer policy.
# Plan 01-01, Task 3 (D9 / D10 / Model A).
#
# Invoked by an injected harness hook. Args: $1=harness  $2=event  $3=workspace_id
# The hook payload arrives on stdin.
#
# Behaviour:
#   - Appends a tagged JSONL event to $AGENT_TEAMS_STATE_DIR/<id>/events.jsonl
#     (app-support; NEVER the repo — D6). Each line: {ts,harness,event,workspace_id,decision,payload}
#   - For cursor interceptions (beforeShellExecution / preToolUse) it applies a
#     thin allowlist: read-only/safe commands -> decision=allow (emit
#     {"permission":"allow"} so cursor proceeds); everything else -> decision=defer
#     (emit {} so cursor shows its NATIVE prompt — Model A; the human answers in
#     the live PTY). claude blocks arrive natively via the PermissionRequest hook,
#     so for claude this script is a pure logger (emits {} = continue).
#
# A hook must never abort the agent: this script swallows errors and always exit 0.
#
# NO HARD python3 dependency: the millisecond timestamp uses pure `date` (so a
# python3-less machine never silently writes ts=0); the two JSON steps prefer
# python3 when present (unchanged behaviour) and fall back to `jq`, then to a safe
# degrade (defer / empty payload) if neither is installed.

HARNESS="${1:-unknown}"
EVENT="${2:-unknown}"
WSID="${3:-unknown}"
PAYLOAD="$(cat 2>/dev/null)"

STATE_DIR="${AGENT_TEAMS_STATE_DIR:-$HOME/Library/Application Support/agent-teams}/$WSID"
mkdir -p "$STATE_DIR" 2>/dev/null
LOG="$STATE_DIR/events.jsonl"

# --- millisecond timestamp, no interpreter ---
# GNU date supports %3N (ms); BSD/macOS date prints the literal "3N" instead, so
# detect a non-numeric result and fall back to seconds*1000 (still valid ms, just
# .000 fraction). Pure-shell arithmetic — never 0 on any machine with `date`.
TS="$(date +%s%3N 2>/dev/null)"
case "$TS" in
  ''|*[!0-9]*) TS="$(( $(date +%s 2>/dev/null || echo 0) * 1000 ))" ;;
esac

# Extract the shell command from a cursor payload (.command or .tool_input.command).
# python3 (unchanged) -> jq -> "" (=> defer, the safe default).
extract_cmd() {
  if command -v python3 >/dev/null 2>&1; then
    printf '%s' "$1" | python3 -c 'import sys,json
try:
    d=json.load(sys.stdin)
    print(d.get("command") or (d.get("tool_input") or {}).get("command","") or "")
except Exception:
    print("")' 2>/dev/null
  elif command -v jq >/dev/null 2>&1; then
    printf '%s' "$1" | jq -r '.command // .tool_input.command // ""' 2>/dev/null
  fi
}

# JSON-encode the first 500 chars of the payload as a BOUNDED JSON STRING.
# The payload field is observability-only: no reader parses it (the watcher,
# core/state-adapter/src/watch.rs, consumes only harness/event/decision/ts), yet
# the old "re-serialise the full payload" path wrote ~22 KB/line (a live pane
# measured 9.1 MB / 395 lines, 2026-06-10) — which is what made every queue tick
# re-read megabytes. So we unify on what was previously only the parse-failure
# branch: always a truncated string. The cursor allow/defer policy reads the RAW
# stdin via extract_cmd BEFORE this, so decisions are unaffected.
# python3 (preferred) -> jq -> "" (handled by the caller's '""' fallback).
compact_json() {
  if command -v python3 >/dev/null 2>&1; then
    printf '%s' "$1" | python3 -c 'import sys,json
print(json.dumps(sys.stdin.read()[:500]))' 2>/dev/null
  elif command -v jq >/dev/null 2>&1; then
    printf '%s' "$1" | head -c 500 | jq -Rs . 2>/dev/null
  fi
}

# --- extract the shell command for tool-use events (claude PreToolUse + cursor pre-exec) ---
decision="na"
CMD=""
case "$HARNESS:$EVENT" in
  claude:PreToolUse|cursor:beforeShellExecution|cursor:preToolUse)
    CMD="$(extract_cmd "$PAYLOAD")" ;;
esac

# --- HARD DENY (the human-boundary RED LINE): an agent must NEVER merge a PR or enable auto-merge.
# Workers already can't push (worker_git_deny_env strips git creds), but a hook-capable ORCHESTRATOR/
# MONITOR pane (the one that opened the PR) runs under the operator's gh keyring auth and could
# `gh pr merge` its own work — which is exactly the red line ("agents never auto-merge; the PR is the
# human boundary"). This closes that gap for every hook-capable harness (claude via a PreToolUse
# deny, cursor via a permission deny). State-blind harnesses (codex/commandcode/opencode/bash) have
# no hook surface to gate here — keep mutating delegate work on claude/cursor. The deny is logged
# (decision=deny-merge) for the audit trail.
if [ -n "$CMD" ] && printf '%s' "$CMD" | grep -Eiq 'gh[[:space:]]+pr[[:space:]]+merge([[:space:]]|$)|enable-auto-merge|gh[[:space:]]+api[^|;&]*pulls/[0-9]+/merge'; then
  decision="deny-merge"
fi

# --- cursor allow/defer policy (only when NOT already a merge-deny) ---
if [ "$decision" != "deny-merge" ]; then
  case "$HARNESS:$EVENT" in
    cursor:beforeShellExecution|cursor:preToolUse)
      # allowlist: read-only / inspection commands auto-allow; all else defers to you
      if printf '%s' "$CMD" | grep -Eq '^[[:space:]]*(ls|cat|pwd|echo|grep|rg|find|head|tail|wc|which|stat|file|git (status|diff|log|show|branch))([[:space:]]|$)'; then
        decision="allow"
      else
        decision="defer"
      fi
      ;;
  esac
fi

# --- append the tagged event (payload = bounded <=500-char JSON string; see compact_json) ---
PJSON="$(compact_json "$PAYLOAD")"
[ -z "$PJSON" ] && PJSON='""'

printf '{"ts":%s,"harness":"%s","event":"%s","workspace_id":"%s","decision":"%s","payload":%s}\n' \
  "$TS" "$HARNESS" "$EVENT" "$WSID" "$decision" "$PJSON" >> "$LOG" 2>/dev/null

# --- writer-side rotation (byte-cap) ---
# events.jsonl is append-only and otherwise grows unbounded (a live pane measured
# 9.1 MB / 395 lines on 2026-06-10). When it exceeds ROTATE_BYTES, keep only the
# last ROTATE_KEEP whole lines via a temp + atomic mv — the file stays valid JSONL
# (only complete leading lines are dropped) and the append protocol is unchanged.
# FAIL-SOFT: every step is guarded; a rotation error must NEVER break the state
# write (the hook still returns cleanly below).
ROTATE_BYTES="${AGENT_TEAMS_EVENTS_MAX_BYTES:-2097152}"   # 2 MiB cap
ROTATE_KEEP="${AGENT_TEAMS_EVENTS_KEEP_LINES:-2000}"       # lines retained on rotate
{
  SZ="$(stat -f%z "$LOG" 2>/dev/null || stat -c%s "$LOG" 2>/dev/null || echo 0)"
  if [ "${SZ:-0}" -gt "$ROTATE_BYTES" ] 2>/dev/null; then
    TMP="$LOG.rot.$$"
    if tail -n "$ROTATE_KEEP" "$LOG" > "$TMP" 2>/dev/null; then
      mv -f "$TMP" "$LOG" 2>/dev/null || rm -f "$TMP" 2>/dev/null
    else
      rm -f "$TMP" 2>/dev/null
    fi
  fi
} 2>/dev/null || true

# --- respond to the harness ---
REASON="Agent Teams policy: agents never merge PRs or enable auto-merge. The PR is the human review/merge boundary — open it for review and merge it yourself in GitHub."
if [ "$decision" = "deny-merge" ]; then
  case "$HARNESS" in
    # claude: a PreToolUse permissionDecision=deny HARD-blocks the tool, even under
    # acceptEdits / --dangerously-skip-permissions (hooks gate above the permission mode).
    claude) printf '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"%s"}}\n' "$REASON" ;;
    # cursor: permission=deny blocks the shell exec; agentMessage tells the agent why.
    cursor) printf '{"permission":"deny","agentMessage":"%s"}\n' "$REASON" ;;
    *)      echo '{}' ;;
  esac
elif [ "$HARNESS" = "cursor" ] && { [ "$EVENT" = "beforeShellExecution" ] || [ "$EVENT" = "preToolUse" ]; }; then
  if [ "$decision" = "allow" ]; then
    echo '{"permission":"allow"}'
  else
    # {} = no decision -> cursor falls through to its native prompt (Model A).
    # NOTE [AC-6]: confirm this yields a prompt (not a hang) in true interactive mode.
    echo '{}'
  fi
else
  echo '{}'
fi
exit 0
