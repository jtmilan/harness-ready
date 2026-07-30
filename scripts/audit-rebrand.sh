#!/usr/bin/env bash
# audit-rebrand.sh — Tier A rebrand residue gate
#
# Scans the tree for leftover product/server identity that should have been
# renamed in the agent-teams → harness-ready rebrand:
#   - agent-teams-mcp   (binary/crate/path identity — NOT state-file names)
#   - mcp__agent-teams  (MCP tool-prefix allowlists)
#   - "agent-teams"     (MCP server-key in config JSON/TOML)
#
# Tier B residue is ALLOWLISTED (stays until a later pass with a compat shim):
#   - AGENT_TEAMS_* env var names
#   - state-file/socket constants: agent-teams-mcp.sock, agent-teams-live.json,
#     agent-teams-external-mutations.jsonl, agent-teams-mcp-http.token/port,
#     agent-teams-orchestrate, agent-teams-bridge, agent-teams-daemon*.json(l)
#   - agent-teams/<id> worktree branch prefixes / .agent-teams-worktrees
#   - core/mcp PACKAGE name agent-teams-core / agent_teams_core
#   - launchd label com.agent-teams.daemon
#
# Exit 0 when no UNEXPECTED residue remains in L4-owned paths.
# Exit 1 when unexpected residue is found in L4-owned paths.
#
# Hits under other parallel-lane file boundaries (L1/L2/L3) or outside every
# lane's write set (scripts plumbing, historical notes) are reported as
# OTHER_LANE / OUT_OF_SCOPE (informational) and do not fail this gate.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

EXCLUDE_DIRS=(
  --exclude-dir=target
  --exclude-dir=node_modules
  --exclude-dir=.git
  --exclude-dir=dist
  --exclude-dir=binaries
  --exclude-dir=.agent-teams-worktrees
)

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

grep -RIn "${EXCLUDE_DIRS[@]}" --exclude='*.lock' --exclude='Cargo.lock' \
  -e 'agent-teams-mcp' . 2>/dev/null >"$TMP/p1" || true

grep -RIn "${EXCLUDE_DIRS[@]}" --exclude='*.lock' --exclude='Cargo.lock' \
  -e 'mcp__agent-teams' . 2>/dev/null >"$TMP/p2" || true

grep -RIn "${EXCLUDE_DIRS[@]}" --exclude='*.lock' --exclude='Cargo.lock' \
  -e '["'"'"']agent-teams["'"'"']' . 2>/dev/null >"$TMP/p3" || true

cat "$TMP/p1" "$TMP/p2" "$TMP/p3" 2>/dev/null | sort -u >"$TMP/all" || true

# Strip Tier-B substrings from a content snippet; if nothing Tier-A-ish remains, allowlist.
# Returns 0 = fully Tier B (allow), 1 = still has Tier A signal.
is_fully_tier_b() {
  local content="$1"
  local stripped="$content"

  # Remove env var names
  stripped="$(printf '%s' "$stripped" | sed -E 's/AGENT_TEAMS_[A-Z0-9_]+//g')"

  # Remove state-file / socket constants (Tier B)
  stripped="$(printf '%s' "$stripped" | sed -E \
    -e 's/agent-teams-mcp\.sock//g' \
    -e 's/agent-teams-mcp-http\.(token|port)//g' \
    -e 's/agent-teams-mcp-http//g' \
    -e 's/agent-teams-live(-[a-z0-9]+)?\.json//g' \
    -e 's/agent-teams-live//g' \
    -e 's/agent-teams-external-mutations(\.jsonl)?//g' \
    -e 's/agent-teams-orchestrate//g' \
    -e 's/agent-teams-bridge//g' \
    -e 's/agent-teams-daemon(-audit|-worktrees)?(\.jsonl? )?//g' \
    -e 's/agent-teams-daemon\.json//g' \
    -e 's/agent-teams-daemon-audit\.jsonl//g' \
    -e 's/agent-teams-daemon-worktrees\.json//g')"

  # Package name / rust id (Tier B)
  stripped="$(printf '%s' "$stripped" | sed -E \
    -e 's/agent-teams-core//g' \
    -e 's/agent_teams_core//g')"

  # Worktree prefixes / launchd label / managed markers
  stripped="$(printf '%s' "$stripped" | sed -E \
    -e 's/\.agent-teams-worktrees//g' \
    -e 's|agent-teams/[A-Za-z0-9_-]+||g' \
    -e 's/com\.agent-teams\.[A-Za-z0-9_.-]+//g' \
    -e 's/@agent-teams-managed//g')"

  # Application Support layout dir component
  stripped="$(printf '%s' "$stripped" | sed -E \
    -e 's|Application Support/agent-teams||g' \
    -e 's|harness-ready/agent-teams||g')"

  # Structural agent-teams-<suffix> ids that are NOT agent-teams-mcp binary
  # (e.g. agent-teams-ws42, agent-teams-{suffix}) — leave agent-teams-mcp for Tier A check
  stripped="$(printf '%s' "$stripped" | sed -E 's/agent-teams-([a-zA-Z0-9_{}]+)//g')"
  # Wait: the above also eats agent-teams-mcp → strip mcp remnant? We need agent-teams-mcp to REMAIN.
  # Re-do: only strip agent-teams-X when X is not mcp...
  # Actually the sed above already ran and would have removed agent-teams-mcp as agent-teams- + mcp.
  # Fix: re-check original content for Tier-A signals instead of strip-all.

  return 0  # placeholder — real logic below
}

# True (0) when the hit is allowlisted Tier B.
is_tier_b_hit() {
  local content="$1"
  local path="$2"

  # Meta: this audit script + lane briefs
  case "$path" in
    ./scripts/audit-rebrand.sh|./.claude/lanes/*) return 0 ;;
  esac

  # mcp__agent-teams is ALWAYS Tier A (never allowlist)
  if [[ "$content" =~ mcp__agent-teams ]]; then return 1; fi

  # agent-teams-mcp binary/crate/path — Tier A UNLESS it's a Tier B state-file suffix
  if [[ "$content" =~ agent-teams-mcp ]]; then
    # If every agent-teams-mcp occurrence is a Tier B state-file form, allow.
    # Remove Tier B forms; if agent-teams-mcp remains, it's Tier A.
    local rest="$content"
    rest="${rest//agent-teams-mcp.sock/}"
    rest="${rest//agent-teams-mcp-http.token/}"
    rest="${rest//agent-teams-mcp-http.port/}"
    rest="${rest//agent-teams-mcp-http/}"
    if [[ "$rest" =~ agent-teams-mcp ]]; then
      return 1  # still has binary/crate/path identity
    fi
    # only state-file forms remained
  fi

  # "agent-teams" server-key — Tier A for JSON/TOML keys + map lookups.
  # Layout defaults (join/to_string/Application Support path) stay Tier B.
  if [[ "$content" =~ [\"\']agent-teams[\"\'] ]]; then
    if [[ "$content" =~ unwrap_or_else|to_string\(\)|join\([\"\']agent-teams|Application.Support|\\.join\([\"\']agent-teams ]]; then
      return 0
    fi
    # JSON key ("agent-teams":), map index ["agent-teams"], mcpServers blocks → Tier A
    return 1
  fi

  # No Tier A signal matched above → treat remaining agent-teams* as Tier B
  return 0
}

lane_for_path() {
  local path="$1"
  case "$path" in
    ./core/hooks/*|./core/state-adapter/*|./core/supervisor/tests/*)
      echo L1; return 0 ;;
    ./harness-ready-mcp/*|./agent-teams-mcp/*)
      echo L2; return 0 ;;
    ./app/*|./ui/src/*)
      echo L3; return 0 ;;
    ./core/supervisor/src/*|./core/harness/*|./core/daemon/*|./core/mcp/*|\
    ./core/memory/*|./core/agent/*|./core/task/*|./core/ringbuf/*|\
    ./core/flywheel/*|./core/roles/*|./docs/*|./README.md|./scripts/audit-rebrand.sh)
      echo L4; return 0 ;;
  esac
  echo OUT
  return 0
}

UNEXPECTED=()
OTHER_LANE=()
OUT_OF_SCOPE=()
TIER_B=0
TOTAL=0

while IFS= read -r hit || [[ -n "$hit" ]]; do
  [[ -z "$hit" ]] && continue
  TOTAL=$((TOTAL + 1))
  path="${hit%%:*}"
  rest="${hit#*:}"
  # rest = lineno:content
  content="${rest#*:}"

  if is_tier_b_hit "$content" "$path"; then
    TIER_B=$((TIER_B + 1))
    continue
  fi

  lane="$(lane_for_path "$path")"
  case "$lane" in
    L1|L2|L3)
      OTHER_LANE+=("[$lane] $hit")
      ;;
    L4)
      UNEXPECTED+=("$hit")
      ;;
    *)
      OUT_OF_SCOPE+=("$hit")
      ;;
  esac
done <"$TMP/all"

echo "=== audit-rebrand.sh — Tier A residue scan ==="
echo "root: $ROOT"
echo "raw hits: $TOTAL  (tier-b allowlisted: $TIER_B)"
echo

if ((${#OTHER_LANE[@]} > 0)); then
  echo "--- OTHER_LANE residue (L1/L2/L3 not yet applied; non-blocking) ---"
  for h in "${OTHER_LANE[@]}"; do
    echo "  $h"
  done
  echo
fi

if ((${#OUT_OF_SCOPE[@]} > 0)); then
  echo "--- OUT_OF_SCOPE residue (scripts/historical/root; non-blocking for L4 gate) ---"
  for h in "${OUT_OF_SCOPE[@]}"; do
    echo "  $h"
  done
  echo
fi

if ((${#UNEXPECTED[@]} > 0)); then
  echo "--- UNEXPECTED residue in L4-owned paths (FAIL) ---"
  for h in "${UNEXPECTED[@]}"; do
    echo "  $h"
  done
  echo
  echo "FAIL: ${#UNEXPECTED[@]} unexpected residue hit(s) in L4 scope."
  exit 1
fi

echo "--- UNEXPECTED residue (L4 scope) ---"
echo "  (none)"
echo
echo "OK: L4-owned paths are clean of unexpected Tier A residue."
if ((${#OTHER_LANE[@]} > 0)); then
  echo "NOTE: ${#OTHER_LANE[@]} other-lane hit(s) remain for L1/L2/L3 to clear."
fi
if ((${#OUT_OF_SCOPE[@]} > 0)); then
  echo "NOTE: ${#OUT_OF_SCOPE[@]} out-of-scope hit(s) remain (scripts/root/historical)."
fi
exit 0
