//! Unix-domain socket — the net-new MCP→app mutation seam (analysis §2).
//!
//! There is **no external IPC seam today** except this one: the only OTHER path to
//! the app's `send_input` is the Tauri webview `invoke` bridge, which an external
//! process (the sidecar) cannot reach. Phase B adds a Unix-domain socket the **app
//! binds** and the **sidecar dials**.
//!
//! **SSOT (06-02).** The wire protocol ([`SocketRequest`]/[`SocketResponse`]/
//! [`response_code`]) and the path policy ([`socket_path`]/[`SOCKET_FILE`]) now live
//! in `agent_teams_core` (beside `registry_path`) so the **app-side binder** and the
//! **sidecar dialer** serialize the EXACT SAME definition and can never drift. This
//! module re-exports them and implements only the **sidecar-side** [`dial`] client.
//! The app-side `bind` + peer-cred(euid) check live in `app/src-tauri/src/lib.rs`.

#![allow(dead_code)] // Re-exports + the sidecar dial client.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use super::PhaseBError;

// Re-export the shared SSOT types so `mutations.rs` (and any reviewer) resolves the
// wire contract through the SINGLE definition in core, not a sidecar-local copy.
// `socket_path`/`SOCKET_FILE` are re-exported for contract completeness even though
// the sidecar resolves the path via `agent_teams_core` directly. `op_timeout` is the
// per-op read/write window SSOT both sides import (06-03 GAP 2).
#[allow(unused_imports)]
pub use agent_teams_core::{
    op_timeout, response_code, socket_path, PaneSpec, SocketRequest, SocketResponse, SOCKET_FILE,
};

/// Dial the app's mutation socket and round-trip one request, using the PER-OP
/// timeout from the shared SSOT [`op_timeout`] (06-03 GAP 2). The 06-02 fast ops
/// (`SendInput`/`Focus`) keep the 5s window; the synthesis-wrapping `Orchestrate`
/// gets the long (>120s) window so the sidecar's READ-WAIT does not give up while
/// the app runs the headless-claude synthesis. The READ side is the load-bearing
/// one — it blocks waiting for the app's reply through the whole synthesis.
///
/// This is the dial every tool should use. [`dial`] is kept as a thin alias for the
/// 06-02 tools/tests that hard-code the fast contract.
pub fn dial_op(socket: &Path, req: &SocketRequest) -> Result<SocketResponse, PhaseBError> {
    dial_with_timeout(socket, req, op_timeout(req))
}

/// Dial the app's mutation socket and round-trip one request (SIDECAR side) with an
/// explicit `timeout` on the read/write.
///
/// Connects [`socket_path`], writes one JSON line, reads one [`SocketResponse`]
/// line. A socket that is **absent / refuses to connect** ⇒ the caller maps to
/// [`super::mutations::APP_NOT_RUNNING`] (the live app is required to mutate; reads
/// keep working app-independently). A connect that succeeds but then fails to
/// round-trip (timeout / malformed reply) surfaces as a distinct transport error.
///
/// Blocking std I/O: the tool wrapper runs this on a `spawn_blocking` thread so the
/// async runtime is never stalled by a single local client.
pub fn dial_with_timeout(
    socket: &Path,
    req: &SocketRequest,
    timeout: Duration,
) -> Result<SocketResponse, PhaseBError> {
    // Absent / refused ⇒ app-down. Any connect error is treated as "app not
    // running" so the tool returns the structured APP_NOT_RUNNING code.
    let mut stream = UnixStream::connect(socket).map_err(|_| {
        PhaseBError::Incomplete("socket::dial: app socket absent/unreachable (APP_NOT_RUNNING)")
    })?;
    // Bound the I/O so a wedged app can't block the client forever. The window is
    // per-op: long enough for Orchestrate's wrapped synthesis, fast for the rest.
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));

    // One request per line.
    let mut line = serde_json::to_string(req)
        .map_err(|_| PhaseBError::Incomplete("socket::dial: request serialize failed"))?;
    line.push('\n');
    stream.write_all(line.as_bytes()).map_err(|_| {
        PhaseBError::Incomplete("socket::dial: write failed (app gone mid-request)")
    })?;
    let _ = stream.flush();

    // Read exactly one reply line.
    let mut reader = BufReader::new(stream);
    let mut reply = String::new();
    reader
        .read_line(&mut reply)
        .map_err(|_| PhaseBError::Incomplete("socket::dial: read failed/timeout"))?;
    serde_json::from_str::<SocketResponse>(reply.trim_end())
        .map_err(|_| PhaseBError::Incomplete("socket::dial: malformed app reply"))
}

/// 06-02 fast-op dial: round-trip one request at the per-op timeout (an alias for
/// [`dial_op`]). Kept so the 06-02 `team_send_input`/`team_focus_workspace` callers
/// + tests read unchanged.
pub fn dial(socket: &Path, req: &SocketRequest) -> Result<SocketResponse, PhaseBError> {
    dial_op(socket, req)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reexports_resolve_through_core() {
        // The shape comes from core; assert the re-export composes here.
        let req = SocketRequest::Focus { id: "w".into() };
        let s = serde_json::to_string(&req).unwrap();
        assert_eq!(s, r#"{"op":"focus","id":"w"}"#);
        assert_eq!(response_code::OK, "OK");
    }

    #[test]
    fn dial_on_absent_socket_is_app_not_running_not_panic() {
        // A socket that doesn't exist ⇒ connect refused ⇒ structured Incomplete
        // (the caller maps this to APP_NOT_RUNNING). Never panics.
        let nope = std::env::temp_dir().join("agent-teams-mcp-DEFINITELY-ABSENT.sock");
        let _ = std::fs::remove_file(&nope);
        let r = dial(&nope, &SocketRequest::Focus { id: "w".into() });
        assert!(matches!(r, Err(PhaseBError::Incomplete(_))));
    }

    /// The load-bearing happy path + the two connected-but-failed branches against a
    /// LIVE socket (the only external IPC seam; previously only the absent-socket path
    /// was tested). Binds a one-shot UnixListener and asserts dial_with_timeout for:
    /// a valid ok reply, a valid err reply (app-level rejection, still transports), a
    /// garbage line (malformed-reply error — NOT mistaken for app-down), and a
    /// connected-but-silent server (read-timeout — distinct from connect-refused).
    #[test]
    fn dial_round_trips_live_replies_and_classifies_transport_errors() {
        use std::io::{BufRead, Write};

        // One-shot server: read the request line, then write `reply` and close, or
        // (None) hold the connection open WITHOUT replying so the read-timeout fires.
        fn serve_once(tag: &str, reply: Option<Vec<u8>>) -> std::path::PathBuf {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir().join(format!("at-dial-{tag}-{nonce}.sock"));
            let _ = std::fs::remove_file(&path);
            let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
            std::thread::spawn(move || {
                if let Ok((mut stream, _)) = listener.accept() {
                    let mut r = std::io::BufReader::new(stream.try_clone().unwrap());
                    let mut req = String::new();
                    let _ = r.read_line(&mut req);
                    match reply {
                        Some(bytes) => {
                            let _ = stream.write_all(&bytes);
                            let _ = stream.flush();
                        }
                        None => std::thread::sleep(Duration::from_secs(2)), // hold open, no reply
                    }
                }
            });
            std::thread::sleep(Duration::from_millis(30)); // let bind/listen settle
            path
        }

        let req = SocketRequest::Focus { id: "w".into() };
        let t = Duration::from_secs(2);

        // (a) valid ok reply → Ok(ok:true)
        let mut ok_line = serde_json::to_string(&SocketResponse::ok("done")).unwrap();
        ok_line.push('\n');
        let p = serve_once("ok", Some(ok_line.into_bytes()));
        let resp = dial_with_timeout(&p, &req, t).expect("ok reply transports");
        assert!(resp.ok);
        let _ = std::fs::remove_file(&p);

        // (b) valid err reply → Ok(ok:false): an app-level rejection still transports,
        // so the caller maps it to Rejected, not APP_NOT_RUNNING.
        let mut err_line =
            serde_json::to_string(&SocketResponse::err("DEAD_PANE", "dead")).unwrap();
        err_line.push('\n');
        let p = serve_once("err", Some(err_line.into_bytes()));
        let resp = dial_with_timeout(&p, &req, t).expect("err reply still transports");
        assert!(!resp.ok);
        assert_eq!(resp.code, "DEAD_PANE");
        let _ = std::fs::remove_file(&p);

        // (c) garbage line → distinct malformed-reply error (NOT app-down).
        let p = serve_once("garbage", Some(b"this is not json\n".to_vec()));
        let r = dial_with_timeout(&p, &req, t);
        assert!(matches!(r, Err(PhaseBError::Incomplete(m)) if m.contains("malformed")));
        let _ = std::fs::remove_file(&p);

        // (d) connected but silent → read-timeout (distinct from connect-refused).
        let p = serve_once("silent", None);
        let r = dial_with_timeout(&p, &req, Duration::from_millis(200));
        assert!(matches!(r, Err(PhaseBError::Incomplete(m)) if m.contains("read")));
        let _ = std::fs::remove_file(&p);
    }
}
