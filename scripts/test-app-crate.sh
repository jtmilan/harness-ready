#!/usr/bin/env bash
# test-app-crate.sh — authoritative gate for the Tauri app crate
# (`app/src-tauri`), the one crate the cargo workspace EXCLUDES and that
# `bridge-tests.json` deliberately omits (model-free core/* only).
#
# Extra args are forwarded to `cargo test` (e.g. `-- --ignored`, a test name).
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MANIFEST="$REPO/app/src-tauri/Cargo.toml"
CARGO="${CARGO:-cargo}"

echo "==> gate: cargo test --manifest-path $MANIFEST"
"$CARGO" test --manifest-path "$MANIFEST" "$@"

# The autonomous-controller spine (`loop_iteration`, the dispatch loop, the live
# `socket_delegate` body — ~30 `#[cfg(feature = "delegate-live")]` sites) is COMPILED OUT of
# the default build/test. Only `install-app.sh -f delegate-live` compiles it, and that is a
# build with no tests — so a compile error or type mismatch in the live controller would ship
# undetected by every automated gate. Type-check it here (fast). This closes review finding
# "delegate-live never type-checked".
echo "==> gate: cargo check --manifest-path $MANIFEST --features delegate-live (compiled-out controller spine)"
exec "$CARGO" check --manifest-path "$MANIFEST" --features delegate-live
