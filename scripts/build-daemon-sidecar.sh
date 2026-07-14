#!/usr/bin/env bash
# build-daemon-sidecar.sh — build the agent-teams-daemon binary for the Tauri
# externalBin. Mirrors build-mcp-sidecar.sh / build-whisper-cli.sh: build → cp to
# binaries/<host-triple> (the committed prebuilt the app bundles). Tauri strips the
# triple suffix at bundle time → Contents/MacOS/agent-teams-daemon.
#
# 08-T9 — BUNDLE-BUT-INERT. This stages the daemon Mach-O into the app bundle so the
# packaged AC-1..6 GUI-verify (detached-PTY survives Cmd+Q, no double-spawn,
# idle-shutdown) is RUNNABLE. Bundling is NOT un-gating:
#   * Tauri never EXECUTES externalBins — listing the binary only stages the Mach-O
#     into Contents/MacOS/. The app has no resolve-and-run / Command::new_sidecar for
#     the daemon (the only app references are the agent_teams_daemon::sups::DaemonSups
#     LIBRARY type). So a merely-present binary never auto-runs.
#   * The daemon's ONLY auto-start vector is the launchd socket-activation LaunchAgent,
#     which scripts/install-app.sh keeps gated OFF by default. Because the binary is now
#     present, install-app.sh no longer gates registration on mere binary-PRESENCE
#     (`-x`); it requires an explicit, security-reviewed opt-in
#     (AGENT_TEAMS_DAEMON_LAUNCHAGENT=1). Default installs skip registration.
#   * Run `agent-teams-daemon` with no flag and it selects A1 launchd socket-activation,
#     finds no LISTEN_FDS outside launchd, and EXITS 1 (fail-loud). Only `--dev`
#     self-binds (A2) — the intentional GUI-verify escape hatch.
#
# Unlike build-mcp-sidecar.sh there is NO [features] section: core/daemon/Cargo.toml
# has no feature flags, so this build is unconditional. No codesign/xattr here either —
# signing + de-quarantine happen at install time (scripts/install-app.sh) AFTER
# `tauri build` stages this committed prebuilt into the bundle.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# host triple — hardcoded aarch64-apple-darwin to match the committed
# agent-teams-mcp-<triple> / whisper-cli-<triple> prebuilt names (macOS aarch64 host
# only; cross-arch is out of scope, same constraint the other sidecars carry).
TRIPLE="aarch64-apple-darwin"
DEST="$REPO/app/src-tauri/binaries/agent-teams-daemon-$TRIPLE"

echo "==> building agent-teams-daemon (release, no features — bundled-but-inert)"

cargo build --release -p agent-teams-daemon --manifest-path "$REPO/Cargo.toml"

SRC="$REPO/target/release/agent-teams-daemon"
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
echo "==> OK: daemon sidecar bundled (inert). Commit binaries/agent-teams-daemon-$TRIPLE alongside the source."
