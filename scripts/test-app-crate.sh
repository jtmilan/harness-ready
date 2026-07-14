#!/usr/bin/env bash
# test-app-crate.sh — authoritative gate for the Tauri app crate
# (`app/src-tauri`), the one crate the cargo workspace EXCLUDES and that
# `bridge-tests.json` deliberately omits (model-free core/* only).
#
# Why this wrapper exists (Security & Performance Review, gate G2):
# `app/src-tauri/tauri.conf.json` declares `models/ggml-tiny.en.bin` as a bundle
# resource. `tauri-build` (run from build.rs) HARD-CHECKS declared resources at
# build time, so in a fresh `git worktree add` — where the ~74 MB, gitignored
# model is absent — `cargo test --manifest-path app/src-tauri/Cargo.toml` fails
# to BUILD (a false REJECT, not a real test failure).
#
# Policy (mirrors scripts/install-app.sh:36 fetch-first precedent):
#   1. Model present (>1 MB)        -> run the cargo tests.
#   2. Model absent, fetch succeeds -> run the cargo tests.
#   3. Model absent, fetch FAILS    -> SKIP loudly, exit 0. Never a silent pass:
#      the skip is a non-gate (no network / offline CI), never a false REJECT
#      and never a masked failure. Wire the model in to actually gate the crate.
#
# Extra args are forwarded to `cargo test` (e.g. `-- --ignored`, a test name).
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MANIFEST="$REPO/app/src-tauri/Cargo.toml"
MODEL="$REPO/app/src-tauri/models/ggml-tiny.en.bin"
CARGO="${CARGO:-cargo}"

model_present() {
  [[ -f "$MODEL" ]] && \
    [[ "$(stat -f%z "$MODEL" 2>/dev/null || stat -c%s "$MODEL" 2>/dev/null || echo 0)" -gt 1000000 ]]
}

if ! model_present; then
  echo "==> ggml-tiny.en.bin absent — fetching before app-crate tests (gate G2)"
  if ! bash "$REPO/scripts/fetch-whisper-model.sh"; then
    echo "" >&2
    echo "############################################################" >&2
    echo "## SKIPPED: app-crate tests — whisper model UNAVAILABLE." >&2
    echo "## scripts/fetch-whisper-model.sh failed (offline / no net)." >&2
    echo "## This is a NON-GATE, not a pass: the app crate was NOT" >&2
    echo "## tested. Provide app/src-tauri/models/ggml-tiny.en.bin to" >&2
    echo "## gate it (cp from a built checkout, or re-run with network)." >&2
    echo "############################################################" >&2
    exit 0
  fi
fi

echo "==> gate: cargo test --manifest-path $MANIFEST"
"$CARGO" test --manifest-path "$MANIFEST" "$@"

# The autonomous-controller spine (`loop_iteration`, the dispatch loop, the live
# `socket_delegate` body — ~30 `#[cfg(feature = "delegate-live")]` sites) is COMPILED OUT of
# the default build/test. Only `install-app.sh -f delegate-live` compiles it, and that is a
# build with no tests — so a compile error or type mismatch in the live controller would ship
# undetected by every automated gate. Type-check it here (fast; no model beyond the one already
# gated above). This closes review finding "delegate-live never type-checked".
echo "==> gate: cargo check --manifest-path $MANIFEST --features delegate-live (compiled-out controller spine)"
exec "$CARGO" check --manifest-path "$MANIFEST" --features delegate-live
