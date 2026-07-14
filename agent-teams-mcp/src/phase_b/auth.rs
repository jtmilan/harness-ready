//! Loopback-HTTP auth SKELETON — macOS-Keychain Bearer + `Origin`/`Host`
//! validation (analysis §4; AC-4).
//!
//! ⚠️ STALE / SUPERSEDED SCAFFOLD (do not treat as the shipped design). The design
//! sketched below (a Keychain-sourced Bearer, "NOT a file") was NOT what shipped: the
//! live loopback-HTTP transport authenticates with a 256-bit Bearer held in a `0600`
//! file (`agent-teams-mcp-http.token`, a sibling of `state_root`), compared in constant
//! time, never logged. This module is inert (`#![allow(dead_code)]`, every entry point
//! returns [`PhaseBError::Incomplete`]) and is kept only as a record of the original
//! plan; the `KEYCHAIN_*` consts are not read by anything. See the app's HTTP auth path
//! for the real mechanism.
//!
//! (Historical design note follows.) This is the **LAST + largest attack surface** in
//! Phase B, so it ships only after the Unix-socket + euid path is verified. The opt-in
//! loopback HTTP transport binds `127.0.0.1` ONLY, validates `Origin`/`Host` against
//! DNS-rebinding, and requires a 256-bit Bearer. [`validate_origin`] is the one
//! validator that fails *safe* (deny-by-default) instead of `Incomplete`.

#![allow(dead_code)] // Stale scaffold: no Keychain, no HTTP, nothing consumed (see module doc).

use serde::Serialize;

use super::PhaseBError;

/// Keychain generic-password **service** id for the Bearer token (real Phase B).
pub const KEYCHAIN_SERVICE: &str = "agent-teams-mcp";
/// Keychain **account** under [`KEYCHAIN_SERVICE`] holding the Bearer.
pub const KEYCHAIN_ACCOUNT: &str = "loopback-http-bearer";
/// Bearer entropy: 256 bits (32 bytes) of CSPRNG, per AC-4.
pub const BEARER_TOKEN_BITS: usize = 256;

/// Auth-failure shapes the loopback transports distinguish. Re-exported from the
/// Phase-B root as [`super::AuthError`]; carried by
/// [`PhaseBError::Auth`](super::PhaseBError::Auth).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum AuthError {
    /// No Bearer presented on the HTTP transport.
    MissingToken,
    /// Bearer present but did not match the Keychain token.
    BadToken,
    /// `Origin` / `Host` failed the loopback / DNS-rebinding check.
    ForbiddenOrigin,
    /// Unix-socket peer credential (euid) did not match the app's user.
    PeerCredMismatch,
}

/// Bearer-token auth for the loopback HTTP transport. The token is sourced /
/// rotated from the macOS Keychain; it is NEVER written to a file /
/// `mcp-config.json` and NEVER logged.
#[derive(Debug, Default)]
pub struct BearerAuth {
    /// Placeholder for the Keychain-loaded token. Unused in the scaffold; the
    /// `Debug` derive must never be used to print it once real (custom redaction
    /// lands with the implementation).
    token: Option<String>,
}

impl BearerAuth {
    /// Load the Bearer from the Keychain (real Phase B). Inert: no Keychain call.
    /// Real impl reads `KEYCHAIN_SERVICE`/`KEYCHAIN_ACCOUNT` via the macOS
    /// `security`/Keychain Services API.
    pub fn from_keychain() -> Result<Self, PhaseBError> {
        Err(PhaseBError::Incomplete(
            "BearerAuth::from_keychain: Phase B — Keychain read (Keychain Services) not implemented",
        ))
    }

    /// Generate a fresh [`BEARER_TOKEN_BITS`]-bit token, store it in the Keychain,
    /// and return the new auth (the rotation primitive Settings calls). Inert: no
    /// CSPRNG, no Keychain write. Rotation MUST invalidate the prior token.
    pub fn rotate() -> Result<Self, PhaseBError> {
        Err(PhaseBError::Incomplete(
            "BearerAuth::rotate: Phase B — 256-bit CSPRNG + Keychain write (old token invalidated) not implemented",
        ))
    }

    /// Constant-time verify a presented Bearer against the Keychain token. Inert.
    /// Real impl: absent token ⇒
    /// `PhaseBError::Auth(`[`AuthError::MissingToken`]`)`; mismatch ⇒
    /// `PhaseBError::Auth(`[`AuthError::BadToken`]`)`; equal ⇒ `Ok(())`.
    pub fn verify(&self, _presented: &str) -> Result<(), PhaseBError> {
        let _ = &self.token; // keep the field meaningful for review
        Err(PhaseBError::Incomplete(
            "BearerAuth::verify: Phase B — constant-time Keychain token check not implemented",
        ))
    }
}

/// Validate an HTTP `Origin` + `Host` against DNS-rebinding (MCP spec; AC-4):
/// accept ONLY loopback (`127.0.0.1` / `localhost`), reject any foreign origin.
///
/// **Deny-by-default skeleton** — rejects everything until the real allowlist
/// lands. This is the single validator that fails *safe* (returns
/// [`AuthError::ForbiddenOrigin`]) rather than [`PhaseBError::Incomplete`], so an
/// accidental early wiring cannot accept a foreign origin.
pub fn validate_origin(_origin: Option<&str>, _host: Option<&str>) -> Result<(), AuthError> {
    Err(AuthError::ForbiddenOrigin)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keychain_and_verify_are_inert_not_panicking() {
        assert!(matches!(
            BearerAuth::from_keychain(),
            Err(PhaseBError::Incomplete(_))
        ));
        assert!(matches!(
            BearerAuth::rotate(),
            Err(PhaseBError::Incomplete(_))
        ));
        assert!(matches!(
            BearerAuth::default().verify("x"),
            Err(PhaseBError::Incomplete(_))
        ));
    }

    #[test]
    fn origin_validation_denies_by_default() {
        // Even a loopback origin is rejected until the real allowlist lands — the
        // safe failure mode for the largest attack surface.
        assert_eq!(
            validate_origin(Some("http://localhost"), Some("localhost")),
            Err(AuthError::ForbiddenOrigin)
        );
        assert_eq!(
            validate_origin(Some("http://evil.example"), Some("evil.example")),
            Err(AuthError::ForbiddenOrigin)
        );
    }

    #[test]
    fn bearer_entropy_is_256_bit() {
        assert_eq!(BEARER_TOKEN_BITS, 256);
    }
}
