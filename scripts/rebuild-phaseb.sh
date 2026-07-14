#!/usr/bin/env bash
# rebuild-phaseb.sh — rebuild + install + RE-SIGN the dialed phase-b MCP sidecar
# (~/.local/bin/agent-teams-mcp-phaseb) in one safe step.
#
# WHY THIS SCRIPT EXISTS (gap #14): rebuilding this binary by hand has twice
# caused regressions —
#   (a) a bare `cp` hit the operator's `cp -i` alias and silently SKIPPED the
#       overwrite → the "new" binary was actually the old one;
#   (b) overwriting the Mach-O in place without re-signing leaves an INVALID
#       ad-hoc signature → macOS AMFI SIGKILLs the binary on every spawn, which
#       surfaces to MCP clients as "MCP server closed the connection".
# This script makes the safe path the easy path: /bin/cp -f (alias-proof),
# mandatory ad-hoc re-sign, then a live stdio probe proving the installed
# binary actually executes and speaks MCP.
#
# Usage:
#   bash scripts/rebuild-phaseb.sh
#
# Env overrides:
#   PHASEB_FEATURES  cargo feature set (default: "memory-notes task-tools phase-b-mutations")
#   PHASEB_DEST      install path      (default: ~/.local/bin/agent-teams-mcp-phaseb)
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# The dialed phase-b feature set: agent-write surface (memory-notes + task-tools)
# PLUS the gated PTY/Model-A mutation axis (phase-b-mutations). Unlike the bundled
# pane externalBin (build-mcp-sidecar.sh, which REFUSES phase-b), this binary is
# the operator's own external-orchestrator sidecar — phase-b belongs here.
FEATURES="${PHASEB_FEATURES:-memory-notes task-tools phase-b-mutations}"
DEST="${PHASEB_DEST:-$HOME/.local/bin/agent-teams-mcp-phaseb}"

# ── 1. build ──
echo "==> building agent-teams-mcp (--features \"$FEATURES\")"
cargo build --release -p agent-teams-mcp --features "$FEATURES" --manifest-path "$REPO/Cargo.toml"

SRC="$REPO/target/release/agent-teams-mcp"
if [[ ! -x "$SRC" ]]; then
  echo "ERROR: expected built binary not found at $SRC" >&2
  exit 1
fi

# ── 2. install ──
# /bin/cp -f, NEVER bare `cp`: a bare `cp` resolves through shell aliases, and a
# `cp -i` alias silently skips the overwrite (regression (a) above). The absolute
# /bin/cp path bypasses aliases entirely; -f forces the overwrite.
echo "==> installing -> $DEST"
mkdir -p "$(dirname "$DEST")"
/bin/cp -f "$SRC" "$DEST"
chmod 755 "$DEST"

if ! file "$DEST" | grep -q "Mach-O"; then
  echo "ERROR: $DEST is not a Mach-O executable" >&2
  exit 1
fi

# ── 3. re-sign (MANDATORY) ──
# Overwriting a signed Mach-O in place invalidates its ad-hoc signature: the
# code-directory hashes no longer match the file contents. macOS AMFI then
# SIGKILLs the binary on EVERY spawn (Exit Code 137 / "Killed: 9"), which MCP
# clients report only as "MCP server closed the connection" — a maddeningly
# indirect symptom (regression (b) above). A forced ad-hoc re-sign after every
# install is therefore non-optional.
echo "==> re-signing (ad-hoc) — skipping this is what gets the binary AMFI-SIGKILLed"
codesign --force --sign - --timestamp=none "$DEST"

# ── 4a. verify signature ──
echo "==> verifying signature"
codesign -v "$DEST"

# ── 4b. stdio smoke probe ──
# A valid signature alone doesn't prove the binary RUNS (AMFI kills at spawn
# time). Pipe a real MCP initialize + tools/list over stdio with a timeout and
# require a JSON-RPC "result" back — the only proof the installed binary
# executes post-sign and speaks the protocol.
echo "==> stdio smoke probe (initialize + tools/list, 10s timeout)"

# `timeout` is coreutils (not stock macOS); fall back to gtimeout, then to a
# perl alarm (perl ships with macOS) so the probe can never hang the script.
_timeout() {
  if command -v timeout >/dev/null 2>&1; then
    timeout "$@"
  elif command -v gtimeout >/dev/null 2>&1; then
    gtimeout "$@"
  else
    perl -e 'alarm shift; exec @ARGV' "$@"
  fi
}

PROBE_OUT="$(
  printf '%s\n%s\n%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"rebuild-phaseb-probe","version":"0"}}}' \
    '{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}' \
    '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
    | _timeout 10 "$DEST" 2>/dev/null
)" || true

if ! grep -q '"result"' <<<"$PROBE_OUT"; then
  echo "ERROR: stdio probe FAILED — no JSON-RPC result from $DEST within 10s." >&2
  echo "       The binary likely did not execute (bad signature → AMFI SIGKILL?)" >&2
  echo "       or did not speak MCP. Raw probe output follows:" >&2
  printf '%s\n' "$PROBE_OUT" >&2
  exit 1
fi
echo "==> probe OK: initialize answered with a result"

# Best-effort tool count (cheap to parse; never fails the script).
TOOL_COUNT="$(
  PROBE_OUT="$PROBE_OUT" python3 -c '
import json, os
for line in os.environ["PROBE_OUT"].splitlines():
    line = line.strip()
    if not line:
        continue
    try:
        msg = json.loads(line)
    except json.JSONDecodeError:
        continue
    tools = msg.get("result", {}).get("tools")
    if isinstance(tools, list):
        print(len(tools))
        break
' 2>/dev/null || true
)"
if [[ -n "$TOOL_COUNT" ]]; then
  echo "==> advertised tools: $TOOL_COUNT"
fi

echo "==> phaseb rebuilt, signed, and probe-verified: $DEST"
