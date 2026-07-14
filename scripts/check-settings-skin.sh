#!/usr/bin/env bash
# ────────────────────────────────────────────────────────────────
# check-settings-skin.sh  —  settings-skin contract gate
# P4-owned: verifies the settings redesign (P5-W0) against the
# pinned CONTRACT.md.  Four checks; fails fast.
#
# Invocation (from repo root):
#   ./scripts/check-settings-skin.sh
#
# Exit codes:
#   0  ALL CHECKS PASS
#   1  One or more checks failed
# ────────────────────────────────────────────────────────────────
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
PASS=0
FAIL=0

RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m' # No Color

pass() { PASS=$((PASS+1)); echo -e "  ${GREEN}PASS${NC}  $1"; }
fail() { FAIL=$((FAIL+1)); echo -e "  ${RED}FAIL${NC}  $1"; }

cd "$REPO_ROOT"

echo "─── Check 1: Google-fonts @import / insecure url(http) ───"
VIOL=0

# Google-fonts @import in CSS files (NOT comments about removal)
mapfile -t matches < <(grep -rnI '@import.*fonts\.googleapis\|@import.*Google' app/src/ 2>/dev/null || true)
if [ ${#matches[@]} -gt 0 ]; then
  for m in "${matches[@]}"; do
    # Skip comment lines that say "removed"
    if echo "$m" | grep -qi 'removed\|was.*google\|CDN.*removed'; then
      continue
    fi
    fail "Google-fonts @import: $m"
    VIOL=1
  done
fi

# url(http: — insecure, must use https: everywhere
mapfile -t matches2 < <(grep -rnI 'url(http:' app/src/ 2>/dev/null || true)
if [ ${#matches2[@]} -gt 0 ]; then
  for m in "${matches2[@]}"; do
    # Skip comment lines
    if echo "$m" | grep -qi 'removed\|was\|comment'; then
      continue
    fi
    fail "Insecure url(http:): $m"
    VIOL=1
  done
fi

# Active (non-comment) Google Fonts CDN refs in HTML/CSS
mapfile -t matches3 < <(grep -rnI 'fonts\.googleapis\|googleapis\.com' app/src/index.html app/src/styles.css 2>/dev/null || true)
if [ ${#matches3[@]} -gt 0 ]; then
  for m in "${matches3[@]}"; do
    if echo "$m" | grep -qi 'removed\|was\|comment\|<!--'; then
      continue
    fi
    fail "Google Fonts CDN ref: $m"
    VIOL=1
  done
fi

[ "$VIOL" -eq 0 ] && pass "No Google-fonts @import or insecure url(http:) in app/src"

echo ""
echo "─── Check 2: No toggle wired to file-only gate ───"
VIOL=0
# Gate section must be read-only (never toggles).
# The contract says: "Gates = READ-ONLY visibility — NEVER toggles (PR274 invariant)".
# Check main.js for any toggle wired to allow_mutations / send_input_enabled / daemon gate.
if [ -f app/src/main.js ]; then
  # Look for toggle/onToggle wired inside a gate context
  readarray -t gate_toggle_lines < <(grep -n 'onToggle\|aria-checked\|role.*switch' app/src/main.js 2>/dev/null | grep -i 'gate\|allow_mutations\|send_input' || true)
  if [ ${#gate_toggle_lines[@]} -gt 0 ]; then
    for m in "${gate_toggle_lines[@]}"; do
      fail "Toggle found in gate context: $m"
    done
    VIOL=1
  fi
  # Check settings-core.js for gate toggles
  if [ -f app/src/settings-core.js ]; then
    readarray -t core_gate_toggles < <(grep -n 'onToggle\|buildToggle' app/src/settings-core.js 2>/dev/null | grep -i 'gate\|allow_mutations\|send_input' || true)
    if [ ${#core_gate_toggles[@]} -gt 0 ]; then
      for m in "${core_gate_toggles[@]}"; do
        fail "Toggle wired to gate in settings-core.js: $m"
      done
      VIOL=1
    fi
  fi
  # Check for .ats-toggle[role=switch] with onToggle on gates
  readarray -t settings_toggle_lines < <(grep -n 'ats-toggle\|role.*switch\|aria-checked' app/src/main.js 2>/dev/null || true)
  for line in "${settings_toggle_lines[@]}"; do
    if echo "$line" | grep -qi 'allow_mutations\|send_input\|daemon'; then
      fail "Toggle for file-only gate: $line"
      VIOL=1
    fi
  done
else
  pass "main.js not found — cannot check"
fi
[ "$VIOL" -eq 0 ] && pass "No toggle wired to file-only gate (gates remain read-only)"

echo ""
echo "─── Check 3: Fonts/css/icons files exist per CONTRACT.md ───"
VIOL=0

# 3a: settings-skin.css (p7, new)
if [ -f app/src/assets/settings-skin.css ]; then
  pass "settings-skin.css exists"
else
  fail "app/src/assets/settings-skin.css MISSING (owned by p7)"
  VIOL=1
fi

# 3b: settings-icons.js (p2, new)
if [ -f app/src/assets/settings-icons.js ]; then
  pass "settings-icons.js exists"
else
  fail "app/src/assets/settings-icons.js MISSING (owned by p2)"
  VIOL=1
fi

# 3c: Spotify Mix fonts in target dir (p2, copy from bridge/design-handoff-spotify/fonts/)
for f in spotify-mix-extrabold.woff2 spotify-mix-regular.woff2; do
  if [ -f "app/src/assets/fonts/$f" ]; then
    pass "font app/src/assets/fonts/$f exists"
  else
    fail "app/src/assets/fonts/$f MISSING (owned by p2 — copy from bridge/design-handoff-spotify/fonts/)"
    VIOL=1
  fi
done

# 3d: Source fonts in the contract dir (should always exist)
for f in spotify-mix-extrabold.woff2 spotify-mix-regular.woff2; do
  if [ -f "bridge/design-handoff-spotify/fonts/$f" ]; then
    pass "source font bridge/design-handoff-spotify/fonts/$f exists"
  else
    fail "bridge/design-handoff-spotify/fonts/$f MISSING (source) — check the repository"
    VIOL=1
  fi
done

# 3e: docs/settings-spotify-redesign.md (p3, new)
if [ -f docs/settings-spotify-redesign.md ]; then
  pass "docs/settings-spotify-redesign.md exists"
else
  fail "docs/settings-spotify-redesign.md MISSING (owned by p3)"
  VIOL=1
fi

[ "$VIOL" -eq 0 ] && pass "All contract-mandated files exist"

echo ""
echo "─── Check 4: Only contract-owned files changed ───"
# For each agent-teams pane, verify git diff touches ONLY files
# listed in CONTRACT.md's file ownership table.
VIOL=0

OWNED_BY_CONTRACT=(
  "app/src/main.js"                                   # p6
  "app/src/assets/settings-skin.css"                   # p7
  "app/src/index.html"                                 # p7
  "app/src/settings-core.js"                           # p1
  "app/src/settings-core.test.js"                      # p1
  "app/src/assets/settings-icons.js"                   # p2
  "app/src/assets/fonts/"                              # p2
  "docs/settings-spotify-redesign.md"                  # p3
  "scripts/check-settings-skin.sh"                     # p4
)

any_touched=0
non_contract=0

WORKTREE_BASE="${REPO_ROOT}/.agent-teams-worktrees"
if [ -d "$WORKTREE_BASE" ]; then
  for pane_dir in "$WORKTREE_BASE"/ws*-p*; do
    [ -d "$pane_dir" ] || continue
    pane="$(basename "$pane_dir")"
    [[ "$pane" != ws46817x5-* ]] && continue

    mapfile -t touched < <(cd "$pane_dir" && git diff HEAD --name-only 2>/dev/null || true)
    for f in "${touched[@]}"; do
      [ -z "$f" ] && continue
      any_touched=1
      [[ "$f" == app/src/vendor/fonts/* ]] && continue
      OWNED=0
      for owned in "${OWNED_BY_CONTRACT[@]}"; do
        if [[ "$owned" == */ ]]; then
          [[ "$f" == "$owned"* ]] && { OWNED=1; break; }
        else
          [[ "$f" == "$owned" ]] && { OWNED=1; break; }
        fi
      done
      if [ "$OWNED" -eq 0 ]; then
        fail "Pane $pane touches non-contract file: $f"
        non_contract=1
      fi
    done
  done
fi

# Also check the main repo for any staged/unstaged changes outside contract
mapfile -t main_touched < <(git diff --name-only HEAD 2>/dev/null || true)
for f in "${main_touched[@]}"; do
  [ -z "$f" ] && continue
  any_touched=1
  [[ "$f" == app/src/vendor/fonts/* ]] && continue
  [[ "$f" == .commandcode/* ]] && continue
  [[ "$f" == .gitignore || "$f" == .gitattributes ]] && continue
  OWNED=0
  for owned in "${OWNED_BY_CONTRACT[@]}"; do
    if [[ "$owned" == */ ]]; then
      [[ "$f" == "$owned"* ]] && { OWNED=1; break; }
    else
      [[ "$f" == "$owned" ]] && { OWNED=1; break; }
    fi
  done
  if [ "$OWNED" -eq 0 ]; then
    fail "Repo has unowned change in: $f"
    non_contract=1
  fi
done

if [ "$non_contract" -eq 0 ]; then
  if [ "$any_touched" -eq 0 ]; then
    pass "No files changed — contract boundaries preserved (no-op)"
  else
    pass "All changed files are within contract ownership"
  fi
fi

echo ""
echo "═══════════════════════════════════════════════════════════"
echo -n "Result: "
if [ "$FAIL" -gt 0 ]; then
  echo -e "${RED}${FAIL} FAIL, ${PASS} PASS${NC}"
  exit 1
else
  echo -e "${GREEN}${PASS} PASS, 0 FAIL${NC}"
  echo "Verdict: ALL CHECKS PASS"
fi
