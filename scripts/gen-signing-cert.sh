#!/usr/bin/env bash
# gen-signing-cert.sh — idempotently mint a STABLE self-signed code-signing
# identity in the (login) keychain, so local dev builds sign with a FIXED
# identity instead of ad-hoc ('-'). No paid Apple Developer account required.
#
# WHY THIS EXISTS
#   macOS TCC (Screen Recording, Microphone, …) attaches each permission grant to
#   an app's *Designated Requirement* (DR). Ad-hoc signing (`codesign -s -`) emits
#   a DR that pins the cdhash — which changes on EVERY `tauri build` — so TCC sees
#   each rebuild as a brand-new app and DROPS the grant. Signing with a fixed
#   identity makes the DR read instead:
#         identifier "<bundle-id>" and certificate leaf = H"<cert-sha1>"
#   Both halves are stable across rebuilds (same cert → same leaf hash; same
#   bundle id), so the TCC grant SURVIVES updates. Verified empirically: this
#   leaf-pinned DR is emitted even by an UNTRUSTED self-signed cert — codesign
#   needs the private key, not a trusted chain, so no `add-trusted-cert` step
#   (which is interactive) is required.
#
# SCOPE: LOCAL DEV ONLY. Gatekeeper will NOT trust this cert; install-app.sh
# clears the quarantine xattr so the locally-built app still launches.
#
# IDEMPOTENT: if an identity with the same CN already exists this is a no-op and
# the existing cert (hence the existing leaf hash, hence the existing DR) is kept.
# Re-minting would churn the leaf hash and defeat the entire purpose, so we never
# do. Safe to call unconditionally from install-app.sh / setup / CI.
#
# Usage:
#   bash scripts/gen-signing-cert.sh                       # → login keychain
#   IDENTITY_NAME="My Dev Cert" bash scripts/gen-signing-cert.sh
#   KEYCHAIN=/tmp/x.keychain-db KEYCHAIN_PASSWORD=pw bash scripts/gen-signing-cert.sh
#
# Remove later:
#   security delete-identity -c "Agent Teams Dev" ~/Library/Keychains/login.keychain-db
set -euo pipefail

IDENTITY_NAME="${IDENTITY_NAME:-Agent Teams Dev}"
KEYCHAIN="${KEYCHAIN:-$HOME/Library/Keychains/login.keychain-db}"
CERT_DAYS="${CERT_DAYS:-3650}"
OPENSSL="${OPENSSL:-openssl}"

# ---- idempotency: bail if an identity with this CN is already present --------
# (no -v: a self-signed identity is untrusted, so it shows under "Matching
# identities" but never under "Valid identities only" — still usable for signing.)
existing="$(security find-identity -p codesigning "$KEYCHAIN" 2>/dev/null \
            | grep -F "\"$IDENTITY_NAME\"" || true)"
if [[ -n "$existing" ]]; then
  echo "==> code-signing identity already present (idempotent no-op):"
  echo "   $existing"
  exit 0
fi

command -v "$OPENSSL" >/dev/null 2>&1 || {
  echo "ERROR: openssl not found (override with \$OPENSSL)" >&2; exit 1; }

# ---- scratch dir (private; auto-clean even on error) -------------------------
WORK="$(mktemp -d "${TMPDIR:-/tmp}/agent-teams-signcert.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
chmod 700 "$WORK"

# ---- 1. self-signed cert with a CRITICAL codeSigning EKU ---------------------
cat > "$WORK/req.cnf" <<EOF
[ req ]
distinguished_name = dn
x509_extensions    = ext
prompt             = no
[ dn ]
CN = $IDENTITY_NAME
[ ext ]
basicConstraints     = critical, CA:false
keyUsage             = critical, digitalSignature
extendedKeyUsage     = critical, codeSigning
EOF

"$OPENSSL" req -x509 -newkey rsa:2048 -nodes \
  -keyout "$WORK/key.pem" -out "$WORK/cert.pem" \
  -days "$CERT_DAYS" -config "$WORK/req.cnf" >/dev/null 2>&1

# ---- 2. PKCS#12 bundle -------------------------------------------------------
# OpenSSL 3.x defaults to a SHA-256 PKCS#12 MAC that macOS's Security framework
# cannot read ("MAC verification failed during PKCS12 import"); -legacy forces
# the older, importable algorithm. LibreSSL (Apple's /usr/bin/openssl) neither
# needs nor accepts -legacy — so add it ONLY for OpenSSL 3.x+.
LEGACY_FLAG=""
case "$("$OPENSSL" version 2>/dev/null)" in
  "OpenSSL 3."* | "OpenSSL 4."* | "OpenSSL 5."*) LEGACY_FLAG="-legacy" ;;
esac
P12PASS="agent-teams-import"   # transient: only guards the in-flight .p12 file
# shellcheck disable=SC2086  # LEGACY_FLAG is intentionally a single optional flag
"$OPENSSL" pkcs12 -export $LEGACY_FLAG \
  -inkey "$WORK/key.pem" -in "$WORK/cert.pem" \
  -name "$IDENTITY_NAME" -out "$WORK/id.p12" \
  -passout "pass:$P12PASS" >/dev/null 2>&1

# ---- 3. import into the keychain ---------------------------------------------
# -T /usr/bin/codesign: add ONLY codesign to the key's trusted-application ACL.
# (No -A: that would let any app use the signing key silently — needless surface.)
if [[ -n "${KEYCHAIN_PASSWORD:-}" ]]; then
  security unlock-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN" >/dev/null 2>&1 || true
fi
security import "$WORK/id.p12" -k "$KEYCHAIN" -P "$P12PASS" -T /usr/bin/codesign
echo "==> imported identity \"$IDENTITY_NAME\" into $KEYCHAIN"

# ---- 4. partition list: let codesign use the key with NO GUI prompt ----------
# Needs the keychain password. With it → codesign is silent forever. Without it →
# the FIRST codesign shows a one-time "codesign wants to use a key in your
# keychain … Always Allow" dialog; clicking it (login password once) is the GUI
# equivalent and is just as durable.
if [[ -n "${KEYCHAIN_PASSWORD:-}" ]]; then
  if security set-key-partition-list -S apple-tool:,apple: \
       -s -k "$KEYCHAIN_PASSWORD" "$KEYCHAIN" >/dev/null 2>&1; then
    echo "==> key partition list set — codesign will not prompt"
  else
    echo "WARN: could not set partition list; first codesign may prompt once." >&2
  fi
else
  echo "NOTE: set KEYCHAIN_PASSWORD to silence the one-time codesign keychain"
  echo "      prompt, or just click \"Always Allow\" the first time you build."
fi

# ---- report the SHA-1: this IS the leaf hash the DR will pin -----------------
echo "==> done. signing identity (leaf hash = DR pin):"
security find-identity -p codesigning "$KEYCHAIN" | grep -F "\"$IDENTITY_NAME\""
echo "    next: install-app.sh now signs with \"$IDENTITY_NAME\" automatically."
