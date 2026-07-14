#!/usr/bin/env bash
# build-mcp-sidecar.sh — build the agent-teams-mcp sidecar for the Tauri
# externalBin. Mirrors build-whisper-cli.sh: build → cp to binaries/<host-triple>
# (the committed prebuilt the app bundles). Tauri strips the triple suffix at
# bundle time → Contents/MacOS/agent-teams-mcp.
#
# AGENT-WRITE ENABLED (enablement slice). This builds with
#   --features "memory-notes task-tools"
# so a pane's sidecar exposes the durable MEMORY notes + the TASK lifecycle tools
# in addition to the read surface. This is the DELIBERATE, SEC-reviewed enablement
# flip — agents go read → read+WRITE for the shared memory + task model. It ships
# ONLY because the threat-model controls C1–C8 landed and were proven by a LIVE
# stdio probe (scripts/probe-enablement.py): note-id allowlist (C1), append-only
# task creation (C4), provenance (C2), per-pane scope (4c), quotas (C6), pinned
# repo-key (C7). See `.paul/analysis/bridgeswarm-agent-write-threat-model.md`.
#
# STILL OFF — phase-b-mutations is the SEPARATE PTY / Model-A mutation axis
# (team_send_input / team_orchestrate, gated by `allow_mutations`). It is NOT a
# memory/task feature and MUST NOT be compiled into the bundled externalBin here;
# the guard below refuses it. The two axes are independent: this enables the
# ungated file-I/O write surface only.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# host triple — hardcoded aarch64-apple-darwin to match the committed
# whisper-cli-<triple> prebuilt name (macOS aarch64 host only; cross-arch is out
# of scope).
TRIPLE="aarch64-apple-darwin"
DEST="$REPO/app/src-tauri/binaries/agent-teams-mcp-$TRIPLE"

# The enablement feature set. memory-notes + task-tools = the agent-write surface.
# phase-b-mutations is deliberately ABSENT (the gated PTY axis).
FEATURES="memory-notes task-tools"

echo "==> building agent-teams-mcp (--features \"$FEATURES\" — agent-write enabled; NO phase-b-mutations)"

# Guard: the bundled externalBin must carry the memory/task write features AND must
# NOT carry the Phase-B PTY mutation feature (a different, allow_mutations-gated axis).
if [[ "${CARGO_BUILD_FLAGS:-}" == *phase-b-mutations* || "$FEATURES" == *phase-b-mutations* ]]; then
  echo "ERROR: refusing to build — phase-b-mutations must NOT be bundled in the pane externalBin." >&2
  echo "       That is the separate PTY / Model-A mutation axis (gated by allow_mutations)," >&2
  echo "       not a memory/task write feature. Keep it out of this build." >&2
  exit 1
fi
if [[ "$FEATURES" != *memory-notes* || "$FEATURES" != *task-tools* ]]; then
  echo "ERROR: refusing to build — the enablement externalBin MUST carry memory-notes + task-tools." >&2
  echo "       (A read-only sidecar is the pre-enablement posture; this script ships the write surface.)" >&2
  exit 1
fi

cargo build --release -p agent-teams-mcp --features "$FEATURES" --manifest-path "$REPO/Cargo.toml"

SRC="$REPO/target/release/agent-teams-mcp"
if [[ ! -x "$SRC" ]]; then
  echo "ERROR: expected built binary not found at $SRC" >&2
  exit 1
fi

mkdir -p "$(dirname "$DEST")"
cp "$SRC" "$DEST"
chmod 755 "$DEST"

echo "==> installed externalBin: $DEST ($(du -h "$DEST" | cut -f1))"
echo "==> mach-o check (want a native aarch64 Mach-O executable):"
file "$DEST"
if ! file "$DEST" | grep -q "Mach-O"; then
  echo "ERROR: $DEST is not a Mach-O executable" >&2
  exit 1
fi
echo "==> OK: agent-write sidecar bundled (memory-notes + task-tools). Commit binaries/agent-teams-mcp-$TRIPLE alongside the source."

# ── COORDINATOR sidecar: the SAME crate built ALSO with phase-b-mutations (the gated PTY/Model-A
# axis: team_send_input / team_orchestrate). This is the ONLY binary permitted to carry phase-b.
# A Coordinator-ROLE pane gets it (capability-by-role; resolve_coordinator_sidecar_bin); every other
# pane keeps the read-only $DEST above. Runtime stays gated: team_send_input needs send_input_enabled
# (the narrow UI toggle) + the coordinator peer-pid hard gate + a live target + normalize_input. The
# pane externalBin guard above (refusing phase-b in $FEATURES) is intentionally NOT relaxed — only
# this explicit, separately-named coordinator binary carries it.
COORD_FEATURES="$FEATURES phase-b-mutations"
COORD_DEST="$REPO/app/src-tauri/binaries/agent-teams-mcp-coordinator-$TRIPLE"
echo "==> building agent-teams-mcp COORDINATOR (--features \"$COORD_FEATURES\" — phase-b PTY axis ON)"
cargo build --release -p agent-teams-mcp --features "$COORD_FEATURES" --manifest-path "$REPO/Cargo.toml"
# (this overwrites target/release/agent-teams-mcp — the read-only pane binary was already cp'd to $DEST)
if [[ ! -x "$SRC" ]]; then
  echo "ERROR: expected coordinator binary not found at $SRC" >&2
  exit 1
fi
cp "$SRC" "$COORD_DEST"
chmod 755 "$COORD_DEST"
if ! file "$COORD_DEST" | grep -q "Mach-O"; then
  echo "ERROR: $COORD_DEST is not a Mach-O executable" >&2
  exit 1
fi
echo "==> installed coordinator externalBin: $COORD_DEST ($(du -h "$COORD_DEST" | cut -f1)) — phase-b ON"
echo "==> OK: coordinator sidecar bundled. Commit binaries/agent-teams-mcp-coordinator-$TRIPLE too."
