#!/usr/bin/env bash
# Self-test for the calibration checker (DESIGN §3.6).
# Feeds calibrate.py a CORRECT pair (bad->block, good->approve) -> expect exit 0,
# and a DEGRADED pair (bad->approve)                            -> expect exit 1.
# Also exercises the stdin path. Exits 0 only if every case behaves as expected.
set -u

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
CAL="$HERE/calibrate.py"
S="$ROOT/tests/calibration/samples"

fail=0

expect() {
  # expect <expected_code> <label> -- <cmd...>
  local want="$1"; local label="$2"; shift 3
  "$@" >/dev/null 2>&1
  local got=$?
  if [ "$got" -eq "$want" ]; then
    echo "ok   [$label] exit $got (expected $want)"
  else
    echo "FAIL [$label] exit $got (expected $want)"
    fail=1
  fi
}

echo "== calibrate.py self-test =="

# 1) correct pair -> calibrated -> exit 0
expect 0 "correct pair (bad=block, good=approve)" -- \
  python3 "$CAL" --bad "$S/correct_bad.json" --good "$S/correct_good.json"

# 2) degraded pair (reviewer rubber-stamps the bad fixture) -> untrusted -> exit 1
expect 1 "degraded pair (bad=approve)" -- \
  python3 "$CAL" --bad "$S/degraded_bad.json" --good "$S/correct_good.json"

# 3) degraded the other way (reviewer wrongly rejects the good fixture) -> exit 1
expect 1 "degraded pair (good=request_changes)" -- \
  python3 "$CAL" --bad "$S/correct_bad.json" --good "$S/correct_bad.json"

# 4) stdin path works (bad via stdin) -> exit 0
cat "$S/correct_bad.json" | python3 "$CAL" --bad - --good "$S/correct_good.json" >/dev/null 2>&1
if [ $? -eq 0 ]; then echo "ok   [stdin bad] exit 0 (expected 0)"; else echo "FAIL [stdin bad]"; fail=1; fi

# 5) malformed JSON -> usage/parse error -> exit 2 (fail-closed, not silently ok)
echo 'not json' | python3 "$CAL" --bad - --good "$S/correct_good.json" >/dev/null 2>&1
if [ $? -eq 2 ]; then echo "ok   [malformed json] exit 2 (expected 2)"; else echo "FAIL [malformed json]"; fail=1; fi

echo
if [ "$fail" -eq 0 ]; then
  echo "ALL CASES PASS"
else
  echo "SOME CASES FAILED"
fi
exit "$fail"
