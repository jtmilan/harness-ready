//! Thin socket client to the daemon — the GUI's dial-the-daemon seam (Phase 08
//! Sub-build 2 / 08-T4, filled in for the Q4 Stage-4 app-client slice).
//!
//! ## Role
//!
//! When the `daemon_spawn` GUI routing flag (mcp-config, default OFF) is ON, the app is
//! a CLIENT of the daemon's socket server: it DIALS `Spawn`/`Close`/`SendInput`/`ListLive`
//! over the same Unix-domain socket the daemon binds (`agent_teams_core::socket_path`),
//! and the daemon owns the PTY master fd + the `Child` from birth (approach B). With the
//! flag OFF none of these are ever called — `do_spawn` keeps spawning in-process and the
//! seam is inert.
//!
//! Each fn maps the daemon's [`SocketResponse`] `{ok,code,detail}` into a flat
//! `Result<_, String>`: `ok` → `Ok`, otherwise an `Err` carrying the daemon's code (so the
//! caller's FAIL-SAFE policy can surface it). The `UNCOMMITTED_WORK` sentinel `detail` is
//! passed through UNCHANGED so the EXISTING frontend confirm dialog fires (same contract as
//! the local `do_spawn` sentinel). NO behavior change on a default build.
//!
//! Implemented with `std` + `agent_teams_core` ONLY (the app already links core/mcp); it
//! replicates the newline-JSON round-trip the sidecar uses (`agent-teams-mcp` `phase_b::socket`)
//! without depending on the sidecar crate.

#![allow(dead_code)] // called only on the `daemon_spawn`-ON routing path.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

use agent_teams_core::{
    op_timeout, response_code, socket_path, SocketData, SocketRequest, SocketResponse, SpawnSpec,
};

/// Dial the daemon's Unix-domain socket and round-trip ONE request line. Bounds the I/O
/// with the per-op timeout SSOT (`op_timeout`: `Spawn`=60s, everything else fast/5s) so a
/// wedged daemon can never block a GUI command thread forever.
///
/// Transport errors are mapped to stable `Err` codes the caller's fail-safe policy keys on:
/// `DAEMON_UNREACHABLE` (socket absent / connect refused / write failed),
/// `DAEMON_READ_TIMEOUT` (connected but silent), `DAEMON_MALFORMED` (garbage / empty reply).
/// A connect that SUCCEEDS but returns `{ok:false}` is NOT a transport error — it transports
/// the daemon's structured rejection up to the caller (e.g. the app's OWN socket answering
/// `BAD_REQUEST`, a `SPAWN_DISABLED` gate, an `ALREADY_LIVE` reject).
fn dial(state_root: &Path, req: &SocketRequest) -> Result<SocketResponse, String> {
    let sock = socket_path(state_root).ok_or("DAEMON_UNREACHABLE: state_root has no parent for socket")?;
    let mut stream = UnixStream::connect(&sock)
        .map_err(|_| "DAEMON_UNREACHABLE: daemon socket absent/unreachable".to_string())?;
    let t = op_timeout(req);
    let _ = stream.set_read_timeout(Some(t));
    let _ = stream.set_write_timeout(Some(t));

    let mut line = serde_json::to_string(req).map_err(|e| format!("DAEMON_SERIALIZE: {e}"))?;
    line.push('\n');
    stream
        .write_all(line.as_bytes())
        .map_err(|_| "DAEMON_UNREACHABLE: write failed (daemon gone mid-request)".to_string())?;
    let _ = stream.flush();

    let mut reader = BufReader::new(stream);
    let mut reply = String::new();
    reader
        .read_line(&mut reply)
        .map_err(|_| "DAEMON_READ_TIMEOUT: read failed/timeout".to_string())?;
    serde_json::from_str::<SocketResponse>(reply.trim_end())
        .map_err(|_| "DAEMON_MALFORMED: malformed daemon reply".to_string())
}

/// Map a transported [`SocketResponse`] into the flat `Result` the spawn/lifecycle callers
/// expect. `ok` → `Ok(())`. A non-ok `UNCOMMITTED_WORK` returns its `detail` VERBATIM (it
/// begins with the `UNCOMMITTED_WORK:<id>:…` prefix the frontend parses) so the existing
/// confirm dialog fires unchanged; every other rejection surfaces `<code> (<detail>)`.
fn ok_or_err(resp: SocketResponse) -> Result<(), String> {
    if resp.ok {
        Ok(())
    } else if resp.code == response_code::UNCOMMITTED_WORK {
        // Pass the sentinel through UNCHANGED (same contract as the local do_spawn sentinel).
        Err(resp.detail)
    } else {
        Err(format!("daemon refused: {} ({})", resp.code, resp.detail))
    }
}

/// Classify a `daemon_spawn`/`dial` error string by whether the daemon DEFINITIVELY did NOT
/// create a pane for the id. The D6 write-ahead anchor must be dropped ONLY when this is `true`;
/// on any other error the anchor is KEPT so the next reconcile's `ListLive` can adopt-or-drop a
/// pane the daemon may (or does) own. This OWNS the inverse of [`dial`]/[`ok_or_err`]'s error
/// formats and must track them.
///
/// `true` (definitively NO pane → safe to drop the anchor):
///   - `DAEMON_UNREACHABLE` / `DAEMON_SERIALIZE` — connect refused, write failed, or the request
///     was never serialized/sent: the daemon never received a complete Spawn line.
///   - any structured `{ok:false}` pre-spawn rejection EXCEPT `ALREADY_LIVE` — `SPAWN_DISABLED`,
///     `SPAWN_UNAVAILABLE`, `SPAWN_REJECTED`, `CAP_EXCEEDED`, `BAD_REQUEST`, `UNCOMMITTED_WORK`:
///     the daemon refused BEFORE opening a PTY, so no pane exists.
///
/// `false` (the daemon MAY or DOES hold a live pane → KEEP the anchor):
///   - `DAEMON_READ_TIMEOUT` / `DAEMON_MALFORMED` — the Spawn line was DELIVERED and only the
///     reply was lost; the daemon may have already committed the spawn (the very race the
///     write-ahead exists to survive).
///   - `ALREADY_LIVE` — the daemon CONFIRMS a live pane already exists for this id (an app↔daemon
///     desync, e.g. a prior lost-ack); dropping the anchor here would orphan that live pane, so
///     it is KEPT for `reconcile`/`ListLive` to adopt authoritatively.
///   - any unrecognized error — fail safe toward KEEPING the anchor.
pub fn dial_error_is_definitive_no_spawn(err: &str) -> bool {
    // Ambiguous post-send transport failures → daemon may hold a live pane → KEEP.
    if err.starts_with("DAEMON_READ_TIMEOUT") || err.starts_with("DAEMON_MALFORMED") {
        return false;
    }
    // ALREADY_LIVE confirms a live daemon pane for this id → KEEP (reconcile adopts it).
    if err.contains(response_code::ALREADY_LIVE) {
        return false;
    }
    // Pre-send transport failures (no complete request reached the daemon) → no pane → DROP.
    if err.starts_with("DAEMON_UNREACHABLE") || err.starts_with("DAEMON_SERIALIZE") {
        return true;
    }
    // A structured pre-spawn rejection (`daemon refused: <code> (...)`) or the `UNCOMMITTED_WORK:`
    // sentinel — the daemon refused before opening a PTY → no pane → DROP.
    if err.starts_with("daemon refused: ") || err.starts_with("UNCOMMITTED_WORK:") {
        return true;
    }
    // Unrecognized → fail safe: KEEP the anchor (never silently orphan a possibly-live pane).
    false
}

/// Spawn a workspace IN THE DAEMON (so the PTY master fd never enters the GUI). The daemon
/// re-validates every field of `spec` independently of the app (the gates answer WHO, never
/// WHAT). Returns `Ok` once the daemon owns the new pane; on refusal the caller FAILS SAFE
/// (no local fallback of the same id). The `UNCOMMITTED_WORK` sentinel passes through.
pub fn daemon_spawn(state_root: &Path, spec: &SpawnSpec) -> Result<(), String> {
    let resp = dial(state_root, &SocketRequest::Spawn { spec: spec.clone() })?;
    ok_or_err(resp)
}

/// Type into a daemon-owned pane via the daemon's gated `SendInput` op (the daemon owns the
/// C7 claude prime + split-submit + D30 alive gate). Strips the app-appended trailing `\r`
/// (the daemon's `normalize_input` re-appends the single CR submit); the body is sent as the
/// one-line `text`.
pub fn daemon_send_input(state_root: &Path, id: &str, data: &[u8]) -> Result<(), String> {
    let body = data.strip_suffix(b"\r").unwrap_or(data);
    let text = String::from_utf8_lossy(body).into_owned();
    let resp = dial(state_root, &SocketRequest::SendInput { id: id.to_string(), text })?;
    ok_or_err(resp)
}

/// Close a daemon-owned pane via the daemon's `Close` op (it kills the child, removes the
/// worktree, rewrites the live registry, audit-logs). Idempotent-OK for an absent id.
pub fn daemon_close(state_root: &Path, id: &str) -> Result<(), String> {
    let resp = dial(state_root, &SocketRequest::Close { id: id.to_string() })?;
    ok_or_err(resp)
}

/// Query the daemon's authoritative live-pane set (D7) — the AC-6 anti-double-spawn anchor.
/// Returns the live ids from [`SocketData::LivePanes`]; a non-ok reply or a missing payload
/// surfaces an `Err` (never a silent empty set that would let a reconcile re-spawn a live id).
pub fn daemon_list_live(state_root: &Path) -> Result<Vec<String>, String> {
    let resp = dial(state_root, &SocketRequest::ListLive)?;
    if !resp.ok {
        return Err(format!("daemon ListLive failed: {} ({})", resp.code, resp.detail));
    }
    match resp.data {
        Some(SocketData::LivePanes { ids, .. }) => Ok(ids),
        _ => Err("daemon ListLive returned no LivePanes payload".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc;
    use std::time::Duration;

    /// One-shot daemon stand-in: bind the REAL `socket_path(state_root)`, accept one
    /// connection, capture the request line (forwarded on the returned channel), then write
    /// `reply` and close. Mirrors the sidecar's `serve_once` harness. Returns the receiver so
    /// a test can assert exactly which wire op the client dialed (Spawn-not-local proof).
    fn serve_once(state_root: &Path, reply: SocketResponse) -> mpsc::Receiver<String> {
        let sock = socket_path(state_root).unwrap();
        let _ = std::fs::remove_file(&sock);
        let listener = UnixListener::bind(&sock).unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut r = BufReader::new(stream.try_clone().unwrap());
                let mut req = String::new();
                let _ = r.read_line(&mut req);
                let _ = tx.send(req);
                let mut line = serde_json::to_string(&reply).unwrap();
                line.push('\n');
                let _ = stream.write_all(line.as_bytes());
                let _ = stream.flush();
            }
        });
        std::thread::sleep(Duration::from_millis(30)); // let bind/listen settle
        rx
    }

    // A SHORT unique dir name keeps `<dir>/agent-teams-mcp.sock` under the macOS AF_UNIX
    // ~104-byte path limit (temp_dir is ~48 + the 21-char socket name).
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    /// A temp dir whose CHILD is the state_root, so `socket_path` = `<dir>/agent-teams-mcp.sock`
    /// lands inside the temp dir (cleaned on drop).
    struct Scratch {
        dir: std::path::PathBuf,
    }
    impl Scratch {
        fn new(_tag: &str) -> Self {
            let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("q4c{:x}_{n:x}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self { dir }
        }
        /// state_root is a CHILD of the temp dir → `socket_path` is `<dir>/agent-teams-mcp.sock`.
        fn state_root(&self) -> std::path::PathBuf {
            self.dir.join("s")
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn sample_spec(id: &str) -> SpawnSpec {
        SpawnSpec {
            id: id.to_string(),
            harness: "claude".to_string(),
            repo: "/repo".to_string(),
            session_id: None,
            resume: false,
            role: None,
            is_worker: false,
            extra_dirs: vec![],
            model: None,
            fresh_from_main: false,
            require_worktree: false,
        }
    }

    #[test]
    fn daemon_spawn_dials_spawn_op_not_local() {
        let s = Scratch::new("spawn-ok");
        let rx = serve_once(&s.state_root(), SocketResponse::ok("spawned"));
        let r = daemon_spawn(&s.state_root(), &sample_spec("ws-1"));
        assert!(r.is_ok(), "ok reply → Ok: {r:?}");
        // The daemon received a Spawn op carrying our spec id — proof the app DIALED, not
        // local-spawned.
        let line = rx.recv_timeout(Duration::from_secs(2)).expect("server saw a request");
        let req: SocketRequest = serde_json::from_str(line.trim_end()).unwrap();
        assert!(matches!(req, SocketRequest::Spawn { spec } if spec.id == "ws-1"));
    }

    #[test]
    fn daemon_send_input_strips_trailing_cr_and_dials() {
        let s = Scratch::new("send");
        let rx = serve_once(&s.state_root(), SocketResponse::ok("written"));
        // write_to_pane hands us body + a single trailing \r; the daemon re-appends CR.
        daemon_send_input(&s.state_root(), "ws-1", b"approve\r").unwrap();
        let line = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let req: SocketRequest = serde_json::from_str(line.trim_end()).unwrap();
        match req {
            SocketRequest::SendInput { id, text } => {
                assert_eq!(id, "ws-1");
                assert_eq!(text, "approve", "trailing CR stripped (daemon re-appends it)");
            }
            other => panic!("expected SendInput, got {other:?}"),
        }
    }

    #[test]
    fn daemon_close_dials_close_op() {
        let s = Scratch::new("close");
        let rx = serve_once(&s.state_root(), SocketResponse::ok("closed"));
        daemon_close(&s.state_root(), "ws-9").unwrap();
        let line = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let req: SocketRequest = serde_json::from_str(line.trim_end()).unwrap();
        assert!(matches!(req, SocketRequest::Close { id } if id == "ws-9"));
    }

    #[test]
    fn daemon_list_live_parses_ids() {
        let s = Scratch::new("list");
        let reply = SocketResponse::ok("live").with_data(SocketData::LivePanes {
            ids: vec!["a".to_string(), "b".to_string()],
            workspaces: None,
        });
        let _rx = serve_once(&s.state_root(), reply);
        let ids = daemon_list_live(&s.state_root()).unwrap();
        assert_eq!(ids, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn spawn_disabled_surfaces_code_no_fallback() {
        // The daemon gate is OFF → SPAWN_DISABLED. The client surfaces it as an Err (the
        // caller FAILS SAFE; it must NOT local-spawn the same id).
        let s = Scratch::new("disabled");
        let _rx = serve_once(
            &s.state_root(),
            SocketResponse::err(response_code::SPAWN_DISABLED, "daemon-spawn gate off"),
        );
        let r = daemon_spawn(&s.state_root(), &sample_spec("ws-1"));
        let e = r.unwrap_err();
        assert!(e.contains(response_code::SPAWN_DISABLED), "surfaces the code: {e}");
    }

    #[test]
    fn bad_request_from_own_socket_is_hard_error() {
        // Deployment misconfig: the app's OWN socket answered (it refuses Spawn with
        // BAD_REQUEST). FAIL SAFE — surface, never fall back to a local spawn.
        let s = Scratch::new("badreq");
        let _rx = serve_once(
            &s.state_root(),
            SocketResponse::err(response_code::BAD_REQUEST, "app refuses Spawn"),
        );
        let r = daemon_spawn(&s.state_root(), &sample_spec("ws-1"));
        assert!(r.unwrap_err().contains(response_code::BAD_REQUEST));
    }

    #[test]
    fn uncommitted_work_sentinel_passes_through_unchanged() {
        // The daemon refuses the destructive freshen over the wire (C5) and returns the
        // sentinel; the client passes the detail VERBATIM so the frontend confirm dialog fires.
        let s = Scratch::new("uncommitted");
        let detail = "UNCOMMITTED_WORK:ws-1:2 uncommitted change(s) in /repo. First entries: M a.rs; ?? b.rs";
        let _rx = serve_once(
            &s.state_root(),
            SocketResponse::err(response_code::UNCOMMITTED_WORK, detail),
        );
        let r = daemon_spawn(&s.state_root(), &sample_spec("ws-1"));
        let e = r.unwrap_err();
        assert!(e.starts_with("UNCOMMITTED_WORK:"), "sentinel prefix preserved: {e}");
        assert_eq!(e, detail, "detail passed through byte-for-byte");
    }

    #[test]
    fn definitive_no_spawn_classifier_drops_only_pre_spawn_errors() {
        // Pre-send transport failures: no complete request reached the daemon → DROP the anchor.
        assert!(dial_error_is_definitive_no_spawn(
            "DAEMON_UNREACHABLE: daemon socket absent/unreachable"
        ));
        assert!(dial_error_is_definitive_no_spawn("DAEMON_SERIALIZE: oops"));
        // Structured pre-spawn rejections → no pane → DROP.
        for code in [
            response_code::SPAWN_DISABLED,
            response_code::SPAWN_UNAVAILABLE,
            response_code::SPAWN_REJECTED,
            response_code::CAP_EXCEEDED,
            response_code::BAD_REQUEST,
        ] {
            assert!(
                dial_error_is_definitive_no_spawn(&format!("daemon refused: {code} (detail)")),
                "{code} is a pre-spawn refusal → drop"
            );
        }
        assert!(dial_error_is_definitive_no_spawn(
            "UNCOMMITTED_WORK:ws-1:2 uncommitted change(s)"
        ));

        // AMBIGUOUS post-send / confirmed-live → KEEP the anchor (reconcile adopts authoritatively).
        assert!(!dial_error_is_definitive_no_spawn(
            "DAEMON_READ_TIMEOUT: read failed/timeout"
        ));
        assert!(!dial_error_is_definitive_no_spawn(
            "DAEMON_MALFORMED: malformed daemon reply"
        ));
        assert!(
            !dial_error_is_definitive_no_spawn(&format!(
                "daemon refused: {} (id already live)",
                response_code::ALREADY_LIVE
            )),
            "ALREADY_LIVE confirms a live pane → KEEP the anchor (never orphan it)"
        );
        // Unknown error → fail safe toward KEEP.
        assert!(!dial_error_is_definitive_no_spawn("SOME_FUTURE_CODE: ???"));
    }

    #[test]
    fn absent_socket_is_unreachable_not_panic() {
        // No server bound → connect refused → DAEMON_UNREACHABLE (the caller fails safe).
        let s = Scratch::new("absent");
        let r = daemon_spawn(&s.state_root(), &sample_spec("ws-1"));
        assert!(r.unwrap_err().contains("DAEMON_UNREACHABLE"));
        // ListLive on an absent socket also errors (never a silent empty set).
        assert!(daemon_list_live(&s.state_root()).is_err());
    }
}
