#!/usr/bin/env bash
# ensure-dev-cert.sh — provisioning entry point for the stable "Agent Teams Dev"
# code-signing identity. Closes the c3 cert gap: the in-app self-updater
# (apply_update) and install-app.sh both PREFER signing with a fixed identity so
# the microphone / Screen-Recording TCC grant survives updates, but that branch
# only fires when the identity actually exists. This script guarantees it does.
#
# WHAT IT DOES
#   Thin wrapper that delegates to gen-signing-cert.sh — the single source of
#   truth for minting the identity. We do NOT re-implement the cert logic here so
#   that the identity literal ("Agent Teams Dev") and the cert-creation steps stay
#   defined in exactly one place; duplicating them risks the two copies drifting
#   and silently defeating TCC survival (the Designated Requirement pins the cert's
#   leaf hash, so two different certs = two different DRs).
#
# HEADLESS HONESTY (read this before assuming it is fully silent)
#   Identity CREATION is fully headless: gen-signing-cert.sh builds a self-signed
#   cert with a codeSigning EKU via openssl and imports it with `security import`.
#   No GUI, no operator instructions, no Keychain Access clicks. It is idempotent —
#   a no-op (exit 0) when an identity with the same CN is already present.
#
#   It is NOT guaranteed prompt-free end-to-end: unless KEYCHAIN_PASSWORD is
#   exported (so the key partition list can be set), the FIRST codesign that uses
#   the key pops a one-time macOS "codesign wants to use a key … Always Allow"
#   dialog. Clicking it once (login password) is just as durable as the headless
#   partition-list path. So: creation never prompts; first signing use might, once.
#
# SAFE TO CALL UNCONDITIONALLY. Idempotent, and intended to be invoked from
# install-app.sh before its codesign step. Never fail the caller's install on our
# account — install-app.sh guards this invocation so a non-zero exit here falls
# through to its ad-hoc signing fallback (a fresh machine must still install + run).
#
# Usage:
#   bash scripts/ensure-dev-cert.sh
#   IDENTITY_NAME="My Dev Cert" bash scripts/ensure-dev-cert.sh
#   KEYCHAIN=/tmp/x.keychain-db KEYCHAIN_PASSWORD=pw bash scripts/ensure-dev-cert.sh
#     (throwaway keychain — used by tests to provision headlessly with no GUI)
#
# All env overrides (IDENTITY_NAME, KEYCHAIN, KEYCHAIN_PASSWORD, CERT_DAYS,
# OPENSSL) are passed straight through to gen-signing-cert.sh.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec bash "$REPO/scripts/gen-signing-cert.sh" "$@"
