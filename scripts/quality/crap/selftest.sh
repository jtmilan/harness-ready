#!/usr/bin/env bash
# selftest.sh — proves the standalone CRAP delta pipeline end-to-end on a TINY
# sample (one small Rust fn + lcov, one small JS fn + coverage). Does NOT build
# the full agent-teams app (too slow). Verifies CRAP compute + delta + the
# gate_would_block decision + the assertion-density anti-gaming floor.
#
# Exit 0 = all assertions held. Exit 1 = a check failed (prints which).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export PATH="$HOME/.cargo/bin:/opt/homebrew/opt/llvm/bin:$PATH"
RS="$HERE/sample/rust-sample"
JS="$HERE/sample/js-sample"
FAIL=0

say() { printf '\n=== %s ===\n' "$1"; }
check() { # check <label> <expr-as-string>  (expr evaluated via python truthy)
  if [ "$2" = "True" ]; then echo "  PASS: $1"; else echo "  FAIL: $1 (got '$2')"; FAIL=1; fi
}

# -------------------------------------------------------------------------
say "1. Rust: generate lcov via cargo llvm-cov (one instrumented test run)"
( cd "$RS" && rm -f lcov.info && \
  LLVM_COV=/opt/homebrew/opt/llvm/bin/llvm-cov \
  LLVM_PROFDATA=/opt/homebrew/opt/llvm/bin/llvm-profdata \
  cargo llvm-cov --lcov --output-path lcov.info >/dev/null 2>&1 )
[ -s "$RS/lcov.info" ] && echo "  lcov.info generated" || { echo "  FAIL: no lcov.info"; exit 1; }

say "2. Rust: capture a BASE baseline (pretend gnarly was previously low-CRAP)"
# Real head baseline:
cargo crap --lcov "$RS/lcov.info" --path "$RS" --format json --output "$RS/baseline_head.json" >/dev/null
# Synthetic captured base: gnarly used to be CRAP 10, simple_add not present (=> new).
python3 - "$RS/baseline_head.json" "$RS/baseline_base.json" <<'PY'
import json,sys
b=json.load(open(sys.argv[1]))
b['entries']=[dict(e, crap=10.0) for e in b['entries'] if e['function']=='gnarly']
json.dump(b,open(sys.argv[2],'w'),indent=2)
PY
echo "  base baseline captured"

say "3. Rust: run the verdict (head vs captured base)"
RUST_VERDICT=$(python3 "$HERE/crap_delta.py" \
  --rust-lcov "$RS/lcov.info" --rust-project "$RS" \
  --rust-baseline "$RS/baseline_base.json" --top 5)
echo "$RUST_VERDICT" | python3 -m json.tool >/dev/null && echo "  valid JSON"
check "gnarly is the #1 hotspot" \
  "$(echo "$RUST_VERDICT" | python3 -c "import sys,json;d=json.load(sys.stdin);print('True' if d['hotspots'][0]['function']=='gnarly' and d['hotspots'][0]['crap']>30 else 'False')")"
check "gnarly shows as a CRAP regression in delta" \
  "$(echo "$RUST_VERDICT" | python3 -c "import sys,json;d=json.load(sys.stdin);print('True' if any(x['function']=='gnarly' and x['status']=='regressed' and x['delta']>0 for x in d['delta']) else 'False')")"
check "gate_would_block is True (regression present)" \
  "$(echo "$RUST_VERDICT" | python3 -c "import sys,json;print(str(json.load(sys.stdin)['gate_would_block']))")"

say "3b. Rust: head-vs-head baseline => NO regression => gate False"
RUST_CLEAN=$(python3 "$HERE/crap_delta.py" \
  --rust-lcov "$RS/lcov.info" --rust-project "$RS" \
  --rust-baseline "$RS/baseline_head.json" --top 5)
check "gate_would_block is False on identical base" \
  "$(echo "$RUST_CLEAN" | python3 -c "import sys,json;print(str(not json.load(sys.stdin)['gate_would_block']))")"

# -------------------------------------------------------------------------
say "4. JS: generate head + base Istanbul coverage via c8"
( cd "$JS" && npx --yes c8 --reporter=json --report-dir=./coverage node test.js >/dev/null 2>&1 )
( cd "$JS" && npx --yes c8 --reporter=json --report-dir=./coverage-base node test_base.js >/dev/null 2>&1 )
[ -s "$JS/coverage/coverage-final.json" ] && [ -s "$JS/coverage-base/coverage-final.json" ] \
  && echo "  head + base coverage generated" || { echo "  FAIL: missing coverage"; exit 1; }

say "5. JS: run the verdict (head: gnarly uncovered vs base: gnarly covered)"
JS_VERDICT=$(python3 "$HERE/crap_delta.py" \
  --js-root "$JS" --js-coverage "$JS/coverage/coverage-final.json" \
  --js-base-coverage "$JS/coverage-base/coverage-final.json" --js-base-root "$JS" --top 5)
echo "$JS_VERDICT" | python3 -m json.tool >/dev/null && echo "  valid JSON"
check "gnarly hotspot over threshold (CRAP>30)" \
  "$(echo "$JS_VERDICT" | python3 -c "import sys,json;d=json.load(sys.stdin);print('True' if any(h['function']=='gnarly' and h['crap']>30 for h in d['hotspots']) else 'False')")"
check "gnarly appears in new_over_threshold" \
  "$(echo "$JS_VERDICT" | python3 -c "import sys,json;d=json.load(sys.stdin);print('True' if any(x['function']=='gnarly' for x in d['new_over_threshold']) else 'False')")"
check "gate_would_block is True" \
  "$(echo "$JS_VERDICT" | python3 -c "import sys,json;print(str(json.load(sys.stdin)['gate_would_block']))")"

# -------------------------------------------------------------------------
say "6. Anti-gaming assertion-density floor"
GAMED=$(python3 "$HERE/assertion_density.py" --diff "$HERE/sample/diffs/gamed.diff" --floor 1 --covered-lines-delta 18)
CLEAN=$(python3 "$HERE/assertion_density.py" --diff "$HERE/sample/diffs/clean.diff" --floor 1 --covered-lines-delta 18)
check "assertion-free coverage-padding diff => gamed_suspected True" \
  "$(echo "$GAMED" | python3 -c "import sys,json;print(str(json.load(sys.stdin)['gamed_suspected']))")"
check "real-assertion diff => gamed_suspected False" \
  "$(echo "$CLEAN" | python3 -c "import sys,json;print(str(not json.load(sys.stdin)['gamed_suspected']))")"
check "clean diff counts real assertions (>=1 each)" \
  "$(echo "$CLEAN" | python3 -c "import sys,json;d=json.load(sys.stdin);print('True' if all(t['assertions']>=1 for t in d['all_tests']) else 'False')")"

# -------------------------------------------------------------------------
say "7. Wrapper script smoke (run-crap-delta.sh end-to-end, Rust+JS combined)"
# Write to a file (robust under set -e; avoids fragile nested command-subst).
"$HERE/run-crap-delta.sh" \
  --rust-lcov "$RS/lcov.info" --rust-project "$RS" --rust-baseline "$RS/baseline_base.json" \
  --js-root "$JS" --js-coverage "$JS/coverage/coverage-final.json" \
  --js-base-coverage "$JS/coverage-base/coverage-final.json" --js-base-root "$JS" --top 8 \
  > "$HERE/sample/.combined-verdict.json"
# Assign to a var first — a nested command-subst with brace-set literals inline
# in check's argument mis-parses under some shells; the var form is robust.
COMBINED_LANGS="$(python3 -c "import json;d=json.load(open('$HERE/sample/.combined-verdict.json'));print('True' if set(d['languages'])=={'rust','js'} else 'False')")"
check "combined verdict spans both languages" "$COMBINED_LANGS"
check "combined gate_would_block is True" \
  "$(python3 -c "import json;d=json.load(open('$HERE/sample/.combined-verdict.json'));print(str(d['gate_would_block']))")"

# -------------------------------------------------------------------------
say "RESULT"
if [ "$FAIL" -eq 0 ]; then echo "  ALL CHECKS PASSED"; exit 0; else echo "  SOME CHECKS FAILED"; exit 1; fi
