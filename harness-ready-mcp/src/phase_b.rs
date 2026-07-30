//! PHASE B — INCOMPLETE SCAFFOLD (mutations + loopback IPC + auth), split into
//! three reviewable submodules. This module exists to make the **shape** of Phase
//! B reviewable in code without shipping any of its behavior or attack surface.
//!
//! Compiled only under `--features phase-b-mutations`; **not** registered in the
//! `#[tool_router]` (registering would be the live wiring this task forbids);
//! performs **no real I/O** — no socket is bound, no HTTP listens, no Keychain is
//! touched. Every *behavioral* entry point returns [`PhaseBError::Incomplete`] —
//! never `unimplemented!()` / `todo!()` (those panic if reached). The types,
//! signatures, and doc-comments are the deliverable; the bodies are inert. Pure
//! path/const helpers ([`socket::socket_path`], [`socket::SOCKET_FILE`], the
//! Keychain id consts) return real values; [`auth::validate_origin`] is the one
//! validator that fails *safe* (deny-by-default) rather than returning `Incomplete`.
//!
//! ## Modules
//! - [`socket`] — the net-new Unix-domain seam: path policy (SIBLING of
//!   `state_root`, beside `agent-teams-live.json`), the line/JSON wire protocol
//!   both sides agree on, the **app-side** listener/peer-cred(euid) skeleton, and
//!   the **sidecar-side** dial client.
//! - [`auth`] — loopback-HTTP auth (Phase-B-LAST): macOS-Keychain Bearer
//!   (generate / verify / rotate) + `Origin`/`Host` DNS-rebinding validation. No
//!   Keychain / `security` dependency is added here.
//! - [`mutations`] — the two mutation tools `team_send_input` /
//!   `team_focus_workspace` (plain *unregistered* fns) + the structured
//!   `APP_NOT_RUNNING` error + the Model-A / `\n`-rule contract.
//!
//! ## Cross-pane contract (anchored, NOT sidecar-authoritative)
//! The socket path + wire protocol are a CONTRACT with the **app-side binder**
//! (`app/src-tauri/src/lib.rs` — a different lane). The app *binds*; this crate
//! *dials*. [`socket::socket_path`] mirrors `agent_teams_core::registry_path` so
//! the `.sock` co-locates with `agent-teams-live.json`. **Caveat:** `registry_path`
//! lives in `core/mcp` precisely so writer + reader can never drift; the socket
//! path + the [`socket::SocketRequest`]/[`socket::SocketResponse`] types do NOT yet
//! live there. Real Phase B MUST promote them into `core/mcp` (beside
//! `registry_path`) so the app-side binder imports the same definition. Until then
//! both sides hand-define the shape (recorded in p2.md UNVERIFIED).
//!
//! ## Precise implementation sequence — de-risk: stdio+euid FIRST, HTTP+Keychain LAST
//! 1. **Unix socket + euid (smallest surface; ship + verify before anything else):**
//!    a. App (lib.rs, outside this lane): bind a `tokio::net::UnixListener` at
//!       [`socket::socket_path`]`(state_root)`; remove a stale socket on bind; run
//!       it on its own task (never block the Tauri main thread). The `.sock` is a
//!       SIBLING of `state_root` (state_root is wiped on startup, lib.rs:753).
//!    b. App: on accept, check peer credentials (`getpeereid` / `SO_PEERCRED`) →
//!       reject unless the connecting euid == the app's uid (same-user local only).
//!    c. App: parse one [`socket::SocketRequest`] line; route `SendInput` to the
//!       EXISTING gated `send_input` (lib.rs:626 — `is_alive()` + write), enforcing
//!       the `\n`-rule (one line + a single trailing `\n`) at THAT boundary; route
//!       `Focus` to the existing jump/raise path; reply a [`socket::SocketResponse`].
//!    d. Sidecar (THIS crate): implement [`socket::dial`] + wire
//!       [`mutations::team_send_input`] / [`mutations::team_focus_workspace`] to
//!       send the op and return the app's result; socket absent ⇒
//!       [`mutations::APP_NOT_RUNNING`].
//!    e. Register the two tools in `#[tool_router]` (main.rs), gated by
//!       `allow_mutations`. Verify AC-1 / AC-2 / AC-3 and AC-5 (routing).
//! 2. **Gate + Settings + `mcp-config.json` (SIBLING of `state_root`):** safe
//!    defaults (mutations off / confirm, HTTP off, stdio+euid on); the socket
//!    handler + tools honor `allow_mutations`; Settings UI toggles + the `\n`-rule
//!    contract test (app-side). [app + main.rs lanes.]
//! 3. **Loopback HTTP + Keychain Bearer (LAST — the largest attack surface):**
//!    [`auth`] generates a 256-bit token → macOS Keychain; [`auth::BearerAuth::verify`]
//!    on every request; [`auth::validate_origin`] (`127.0.0.1` + `Origin`/`Host`);
//!    bind HTTP on `127.0.0.1` ONLY (never `0.0.0.0`); rotation. Verify AC-4.

#![allow(dead_code)] // Scaffold: nothing here is wired or consumed yet (propagates to submodules).

use serde::Serialize;

pub mod auth;
pub mod http;
pub mod mutations;
pub mod socket;

pub use auth::AuthError;

/// Transport selector (Phase 12 / D51): UDS PREFERRED, HTTP additive fallback.
///
/// - If the Unix socket CONNECTS → dial it (the euid gate, no Bearer ever on a wire). The
///   UDS path is BYTE-FOR-BYTE unchanged (`socket::dial` / `socket::dial_op`) — no
///   regression. `per_op` picks the per-op read window (`dial_op`) for the Context-Router
///   ops vs the fast `dial` for the 06-02 mutations.
/// - ELSE if `http_enabled` (checked inside `dial_http_op`'s discovery) → the verify-before-
///   send HTTP fallback (`dial_http_op`): challenge → MAC verify → Bearer mutation, all on
///   ONE kept-alive TcpStream (H1). NEVER eager / Bearer-first.
/// - ELSE → `APP_NOT_RUNNING` (the caller's `map_reply` maps any `Incomplete` to it).
///
/// We probe UDS by attempting a connect FIRST: a present, connectable socket always wins
/// (stronger boundary). Only its ABSENCE opens the HTTP path. `http_enabled=false`,
/// `allow_mutations=false`, or a missing port/token make the HTTP discovery fail closed.
pub fn dial_selected(
    socket: &std::path::Path,
    state_dir: &std::path::Path,
    req: &socket::SocketRequest,
    per_op: bool,
) -> Result<socket::SocketResponse, PhaseBError> {
    // Probe UDS: a successful connect ⇒ the live app's socket is up ⇒ PREFER it.
    if std::os::unix::net::UnixStream::connect(socket).is_ok() {
        return if per_op {
            socket::dial_op(socket, req)
        } else {
            socket::dial(socket, req)
        };
    }
    // UDS absent/refused → the verify-before-send HTTP fallback (gated inside discovery).
    http::dial_http_op(state_dir, req)
}

/// Error returned by every Phase-B *behavioral* entry point until the phase is
/// implemented. Pure helpers (paths / consts) and the deny-by-default
/// [`auth::validate_origin`] do NOT use this — they return real values / a typed
/// [`AuthError`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum PhaseBError {
    /// Compiles, but the behavior is deliberately not implemented yet. The
    /// `&'static str` names the precise unimplemented step (for review + logs).
    Incomplete(&'static str),
    /// An auth failure the real loopback transports will distinguish.
    Auth(AuthError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_b_error_wraps_reexported_auth_error() {
        // The root re-export (`pub use auth::AuthError`) is the type other code
        // will pattern-match on; confirm it composes into `PhaseBError`.
        assert!(matches!(
            PhaseBError::Auth(AuthError::BadToken),
            PhaseBError::Auth(AuthError::BadToken)
        ));
        assert!(matches!(
            PhaseBError::Incomplete("x"),
            PhaseBError::Incomplete(_)
        ));
    }
}
