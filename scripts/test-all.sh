#!/usr/bin/env bash
# test-all.sh — run the 5 CI checks LOCALLY, in one shot, before opening a PR.
# Mirrors .github/workflows/ci.yml so you can reproduce a green (or red) CI run
# without pushing. Each suite runs regardless of earlier failures (NOT `set -e`);
# a final summary reports which suites passed / failed and the script exits
# non-zero if ANY suite failed.
#
#   chmod +x scripts/test-all.sh   # (already tracked executable)
#   bash scripts/test-all.sh       # or ./scripts/test-all.sh
#
# The 5 suites (matching CI jobs):
#   1. Rust workspace          — cargo test --workspace --locked  (core/* + agent-teams-mcp)
#   2. App crate               — bash scripts/test-app-crate.sh
#   3. Frontend unit tests     — cd app && npm ci && npm test     (vitest)
#   4. Frontend lint           — npm run lint                     (eslint; advisory)
#   5. Visual regression       — npm run test:visual              (playwright webkit; advisory)
#
# NOTE: -u (unset vars are errors) + pipefail, but NOT -e — we want every suite to
# run so the summary is complete. `npm ci` needs network on first run.
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO"

# ── result tracking ────────────────────────────────────────────────────────
declare -a NAMES=()
declare -a CODES=()

section() {
  echo ""
  echo "════════════════════════════════════════════════════════════════════════"
  echo "▶ $1"
  echo "════════════════════════════════════════════════════════════════════════"
}

run_suite() {
  # run_suite "<name>" <command...>
  local name="$1"; shift
  section "$name"
  "$@"
  local code=$?
  NAMES+=("$name")
  CODES+=("$code")
  if [ "$code" -eq 0 ]; then
    echo "✔ PASS: $name"
  else
    echo "✗ FAIL: $name (exit $code)"
  fi
}

# 1. Rust workspace (core/* + agent-teams-mcp; app/src-tauri is its own crate → suite 2)
run_suite "1/5 Rust workspace (cargo test --workspace)" \
  cargo test --workspace --locked

# 2. Tauri app crate
run_suite "2/5 App crate (scripts/test-app-crate.sh)" \
  bash "$REPO/scripts/test-app-crate.sh"

# 3-5. Frontend suites — all run inside app/. Install deps once up front.
section "frontend deps (npm ci)"
( cd "$REPO/app" && npm ci )
NPM_CI_CODE=$?
NAMES+=("frontend deps (npm ci)")
CODES+=("$NPM_CI_CODE")
if [ "$NPM_CI_CODE" -eq 0 ]; then echo "✔ PASS: npm ci"; else echo "✗ FAIL: npm ci (exit $NPM_CI_CODE)"; fi

run_suite "3/5 Frontend unit tests (npm test / vitest)" \
  bash -c "cd '$REPO/app' && npm test"

run_suite "4/5 Frontend lint (npm run lint / eslint) [advisory in CI]" \
  bash -c "cd '$REPO/app' && npm run lint"

run_suite "5/5 Visual regression (npm run test:visual) [advisory in CI]" \
  bash -c "cd '$REPO/app' && npm run test:visual"

# ── summary ────────────────────────────────────────────────────────────────
echo ""
echo "════════════════════════════════════════════════════════════════════════"
echo "  SUMMARY"
echo "════════════════════════════════════════════════════════════════════════"
FAILED=0
for i in "${!NAMES[@]}"; do
  if [ "${CODES[$i]}" -eq 0 ]; then
    printf "  PASS  %s\n" "${NAMES[$i]}"
  else
    printf "  FAIL  %s (exit %s)\n" "${NAMES[$i]}" "${CODES[$i]}"
    FAILED=$((FAILED + 1))
  fi
done
echo "────────────────────────────────────────────────────────────────────────"
if [ "$FAILED" -eq 0 ]; then
  echo "  ALL SUITES PASSED"
  exit 0
else
  echo "  $FAILED SUITE(S) FAILED"
  echo "  (lint + visual are ADVISORY in CI — a failure there does not block a PR)"
  exit 1
fi
