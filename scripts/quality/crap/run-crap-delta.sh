#!/usr/bin/env bash
# run-crap-delta.sh — standalone, REPORT-ONLY CRAP delta runner driver (DESIGN §3.10).
#
# Orchestrates the cheap deterministic CRAP signal end-to-end:
#   Rust core -> cargo-crap (reads `cargo llvm-cov --lcov`)
#   JS/TS     -> fallow health (reads Istanbul coverage-final.json)
# then emits the §3.10 verdict JSON via crap_delta.py:
#   { hotspots, delta, new_over_threshold, gate_would_block }
#
# It is REPORT-ONLY: it prints the verdict + exits 0 regardless of the verdict.
# It does NOT gate, block, push, or touch lib.rs — that wiring is later work.
# `gate_would_block` in the JSON is the advisory signal the loop driver reads.
#
# The --base ref must be a CAPTURED baseline (cargo-crap baseline json / a base
# Istanbul coverage file), NOT live main — fold transiently mutates main.
#
# Usage:
#   run-crap-delta.sh --rust-lcov HEAD.lcov [--rust-project DIR] [--rust-baseline BASE.json]
#                     --js-root DIR --js-coverage HEAD-coverage.json [--js-base-coverage BASE-coverage.json]
#                     [--threshold 30] [--top 10] [--out verdict.json]
#
# Capturing a Rust baseline (do this on the captured origin/main checkout):
#   cargo crap --lcov base.lcov --format json --output base-crap.json
#
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export PATH="$HOME/.cargo/bin:$PATH"

# Resolve to absolute paths (fallow + cargo-crap are cwd-sensitive).
abspath() { [ -z "${1:-}" ] && return 0; python3 -c "import os,sys;print(os.path.abspath(sys.argv[1]))" "$1"; }

ARGS=()
while [ $# -gt 0 ]; do
  case "$1" in
    --rust-lcov)      ARGS+=(--rust-lcov "$(abspath "$2")"); shift 2;;
    --rust-project)   ARGS+=(--rust-project "$(abspath "$2")"); shift 2;;
    --rust-baseline)  ARGS+=(--rust-baseline "$(abspath "$2")"); shift 2;;
    --js-root)        ARGS+=(--js-root "$(abspath "$2")"); shift 2;;
    --js-coverage)    ARGS+=(--js-coverage "$(abspath "$2")"); shift 2;;
    --js-base-coverage) ARGS+=(--js-base-coverage "$(abspath "$2")"); shift 2;;
    --js-base-root)   ARGS+=(--js-base-root "$(abspath "$2")"); shift 2;;
    --threshold)      ARGS+=(--threshold "$2"); shift 2;;
    --top)            ARGS+=(--top "$2"); shift 2;;
    --epsilon)        ARGS+=(--epsilon "$2"); shift 2;;
    --out)            ARGS+=(--out "$(abspath "$2")"); shift 2;;
    -h|--help) grep '^#' "$0" | sed 's/^# \{0,1\}//'; exit 0;;
    *) echo "unknown arg: $1" >&2; exit 64;;
  esac
done

# Tool presence checks (fail loud, not silent).
command -v cargo >/dev/null || { echo "cargo not found on PATH" >&2; exit 69; }
if printf '%s\n' "${ARGS[@]}" | grep -q -- '--rust-lcov'; then
  cargo crap --version >/dev/null 2>&1 || { echo "cargo-crap not installed (cargo install cargo-crap)" >&2; exit 69; }
fi
if printf '%s\n' "${ARGS[@]}" | grep -q -- '--js-coverage'; then
  command -v fallow >/dev/null || { echo "fallow not installed (npm i -g fallow)" >&2; exit 69; }
fi

python3 "$HERE/crap_delta.py" "${ARGS[@]}"
