//! Loopback-HTTP dial — the verify-before-send transport (Phase 12 / D51).
//!
//! The deferred half of PRD §14 Phase B. The app already LISTENS (opt-in
//! `127.0.0.1:0` + Bearer + Host/Origin, `spawn_http_listener`); the sidecar's UDS
//! `dial_op` is the PREFERRED path (stronger euid gate, no Bearer ever on a wire).
//! This module is the ADDITIVE HTTP fallback for when the Unix socket is absent but
//! `http_enabled` is on.
//!
//! **The security property (D51): verify the listener's identity BEFORE sending the
//! Bearer.** On loopback TCP the UDS peer-euid boundary is GONE; the 0600 token-at-rest
//! is the only same-user boundary. Sending `Authorization: Bearer <token>` to a
//! stale-port squatter is Bearer exfiltration (threat-model T1/T6). So the dial FIRST
//! proves the TCP peer holds the SAME at-rest token via a pre-Bearer HMAC-SHA256
//! identity challenge, and only THEN sends the Bearer mutation.
//!
//! **H1 — single-connection mandate.** The challenge (request 1) and the Bearer mutation
//! (request 2) ride ONE kept-alive `TcpStream`, opened once and reused. If that stream
//! drops between the challenge response and request 2, the dial ABORTS — it MUST NEVER
//! reconnect-and-send-Bearer to the (possibly re-bound) port. "Same Host/Origin" proves
//! nothing about peer continuity on connectionless-identity loopback TCP.
//!
//! Hand-rolled minimal HTTP/1.1 over ONE `std::net::TcpStream` (two requests, keep-alive).
//! NO connection pooling, NO TLS stack, NO async runtime — runs on the existing
//! `spawn_blocking` thread. 127.0.0.1 ONLY.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::time::Duration;

use agent_teams_core::{
    compute_challenge_mac, http_port_path, http_token_path, op_timeout, read_mcp_config,
    verify_challenge_mac, IdentityChallenge, SocketRequest, SocketResponse, CHALLENGE_HEX_LEN,
};

use super::PhaseBError;

/// The connect timeout for the loopback TCP dial (fast — it is the local app or nothing).
const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// Cap on the bytes we read for a single HTTP response (challenge proof / mutation reply).
/// The app's responses are tiny JSON; this bounds a misbehaving/squatting peer.
const HTTP_MAX_RESPONSE_BYTES: usize = 256 * 1024;

/// Discovery result: the validated port + the at-rest token, ready for the dial.
struct Discovered {
    port: u16,
    token: String,
}

/// Local discovery — NO network secret leaves this function. Mirrors the app's
/// `spawn_http_listener` bind gate + `ensure_http_token` perm rule:
/// - require `allow_mutations == true` AND `http_enabled == true`,
/// - read the 0600 token (reject looser perms; reject empty — mirrors
///   `bearer_token_matches`' empty rule),
/// - parse `http_port_path` as `u16` (reject missing/malformed).
///
/// S1 (http_enabled ⇒ port file must exist) + S5 (the MAC challenge, downstream) are the
/// LOAD-BEARING stale-port defenses; S2/S4 (registry liveness/freshness) are ADVISORY
/// only — `read_registry` verifies neither, so we do NOT rely on them here (H3).
fn discover(state_dir: &Path) -> Result<Discovered, PhaseBError> {
    // (1) Config gate (mirror the app's bind gate). SAFE default: absent/malformed ⇒ off.
    let cfg = read_mcp_config(state_dir);
    if !cfg.allow_mutations {
        return Err(PhaseBError::Incomplete(
            "http::discover: allow_mutations is off (MUTATIONS_DISABLED)",
        ));
    }
    if !cfg.http_enabled {
        return Err(PhaseBError::Incomplete(
            "http::discover: http_enabled is off — no HTTP dial (UDS-only / app-down)",
        ));
    }

    // (2) Token: read the 0600 sibling. Reject looser perms + empty (fail closed).
    let Some(token_path) = http_token_path(state_dir) else {
        return Err(PhaseBError::Incomplete(
            "http::discover: state dir has no parent for the http token sibling",
        ));
    };
    let token = read_token_0600(&token_path)?;

    // (3) Port: parse the sibling port file as u16. Missing/malformed ⇒ abort (S1).
    let Some(port_path) = http_port_path(state_dir) else {
        return Err(PhaseBError::Incomplete(
            "http::discover: state dir has no parent for the http port sibling",
        ));
    };
    let port = std::fs::read_to_string(&port_path)
        .ok()
        .and_then(|s| s.trim().parse::<u16>().ok())
        .ok_or(PhaseBError::Incomplete(
            "http::discover: port file absent or not a u16 (S1) — no HTTP dial",
        ))?;
    if port == 0 {
        return Err(PhaseBError::Incomplete(
            "http::discover: port file is 0 (never a bound ephemeral port)",
        ));
    }

    Ok(Discovered { port, token })
}

/// Read the at-rest token, enforcing the 0600 perm + non-empty rule (mirrors the app's
/// `ensure_http_token` repair-or-fail + `bearer_token_matches`' empty-token reject). On a
/// looser-than-0600 mode we FAIL CLOSED (do NOT silently widen / proceed) — a same-user
/// file left group/world-readable is the very leak the 0600 boundary guards.
#[cfg(unix)]
fn read_token_0600(path: &Path) -> Result<String, PhaseBError> {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::metadata(path).map_err(|_| {
        PhaseBError::Incomplete("http::discover: token file absent/unreadable — fail closed")
    })?;
    if meta.permissions().mode() & 0o777 != 0o600 {
        return Err(PhaseBError::Incomplete(
            "http::discover: token file is not 0600 (looser perms) — fail closed, no dial",
        ));
    }
    let token = std::fs::read_to_string(path)
        .map_err(|_| PhaseBError::Incomplete("http::discover: token read failed — fail closed"))?
        .trim()
        .to_string();
    if token.is_empty() {
        return Err(PhaseBError::Incomplete(
            "http::discover: token file is empty — never authorize (fail closed)",
        ));
    }
    Ok(token)
}

#[cfg(not(unix))]
fn read_token_0600(_path: &Path) -> Result<String, PhaseBError> {
    Err(PhaseBError::Incomplete(
        "http::discover: loopback-HTTP dial is unix-only",
    ))
}

/// A fresh 32-byte nonce as 64-char lowercase hex (the challenge nonce). OS CSPRNG.
fn fresh_nonce_hex() -> Result<String, PhaseBError> {
    let mut buf = [0u8; agent_teams_core::CHALLENGE_NONCE_BYTES];
    getrandom::getrandom(&mut buf)
        .map_err(|_| PhaseBError::Incomplete("http::dial: CSPRNG nonce generation failed"))?;
    let mut hex = String::with_capacity(CHALLENGE_HEX_LEN);
    for b in buf {
        use std::fmt::Write as _;
        let _ = write!(hex, "{b:02x}");
    }
    Ok(hex)
}

/// dial_http_op — the verify-before-send HTTP dial (D51), phased + FAIL CLOSED on every step.
///
/// 1. Discovery (local, no network secret) — config gate + 0600 token + u16 port.
/// 2. Open ONE kept-alive `TcpStream` (H1) and POST the identity challenge (NO Bearer).
/// 3. Verify the server's MAC (core SSOT `verify_challenge_mac`: 64-hex-strict + constant
///    time; the MAC match IS the decision — never `==`, never decide on `ok:true`). ABORT
///    on mismatch / non-200 / short-mac.
/// 4. POST the Bearer mutation ON THE SAME STREAM. If that stream dropped after the
///    challenge ⇒ ABORT (never reconnect-and-send-Bearer to the possibly-rebound port).
/// 5. Map the reply through the EXISTING `map_reply` (done by the caller).
pub fn dial_http_op(state_dir: &Path, req: &SocketRequest) -> Result<SocketResponse, PhaseBError> {
    // (1) Discovery — local, fail closed, no network secret sent.
    let Discovered { port, token } = discover(state_dir)?;

    // The exact Host/Origin the server pins (`http_host_allowed` / `http_origin_allowed`).
    // 127.0.0.1 ONLY — never localhost, never a DNS name.
    let host = format!("127.0.0.1:{port}");
    let origin = format!("http://127.0.0.1:{port}");

    // (2) Open the ONE kept-alive TcpStream (H1). 127.0.0.1 ONLY.
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&addr, HTTP_CONNECT_TIMEOUT).map_err(|_| {
        // Connection refused ⇒ the listener is gone ⇒ app-down for this transport.
        PhaseBError::Incomplete("http::dial: connect refused (APP_NOT_RUNNING)")
    })?;
    // Bound the I/O so a wedged/squatting peer can't hang us. The challenge is fast; the
    // mutation gets the per-op window (Orchestrate's wrapped synthesis is long).
    let challenge_timeout = Duration::from_secs(5);
    let _ = stream.set_read_timeout(Some(challenge_timeout));
    let _ = stream.set_write_timeout(Some(challenge_timeout));
    let _ = stream.set_nodelay(true);

    // (2b) Identity challenge — NO Authorization header. Fresh per-dial nonce.
    let nonce = fresh_nonce_hex()?;
    let challenge = IdentityChallenge {
        nonce: nonce.clone(),
    };
    // Reuse the SocketRequest-shaped JSON tag the app classifies on: {"op":"identity_challenge","nonce":...}.
    let challenge_body = format!(
        r#"{{"op":"identity_challenge","nonce":{}}}"#,
        serde_json::to_string(&challenge.nonce)
            .map_err(|_| PhaseBError::Incomplete("http::dial: nonce serialize failed"))?
    );
    let challenge_resp = http_post(
        &mut stream,
        &host,
        &origin,
        None, // NO Bearer on the challenge (AC-1).
        &challenge_body,
    )
    .map_err(|_| PhaseBError::Incomplete("http::dial: challenge write/read failed"))?;

    // (3) Verify — the MAC match is the decision (AC-5). ABORT on non-200 or MAC mismatch.
    if challenge_resp.status != 200 {
        return Err(PhaseBError::Incomplete(
            "http::dial: challenge non-200 — peer is not the genuine listener (ABORT, no Bearer)",
        ));
    }
    // Parse the proof; an `ok` flag is advisory — we decide on the MAC match. A missing/
    // empty/short/long/non-hex `mac` is rejected inside `verify_challenge_mac` (the proof
    // type's `mac` is a NON-OPTIONAL String, so an absent field fails the parse → ABORT).
    let proof: agent_teams_core::IdentityProof = serde_json::from_slice(&challenge_resp.body)
        .map_err(|_| {
            PhaseBError::Incomplete(
                "http::dial: challenge proof unparseable (no/short mac) — ABORT, no Bearer",
            )
        })?;
    if !verify_challenge_mac(&token, &nonce, &proof.mac) {
        // The peer could not prove it holds the at-rest token: a stale-port squatter, a
        // forged MAC, or a representation mismatch. NEVER send the Bearer. Defensively
        // recompute the expected MAC ONLY to ground the abort (does not change the decision).
        let _ = compute_challenge_mac(&token, &nonce);
        return Err(PhaseBError::Incomplete(
            "http::dial: challenge MAC mismatch — peer is NOT the genuine listener (ABORT, no Bearer)",
        ));
    }

    // (4) Mutation request — Bearer ALLOWED now, ON THE SAME STREAM (H1). If the stream
    // dropped after the challenge (genuine listener killed → squatter rebound the port),
    // the write/read fails and we ABORT — we NEVER reconnect-and-send-Bearer.
    let mut_timeout = op_timeout(req);
    let _ = stream.set_read_timeout(Some(mut_timeout));
    let _ = stream.set_write_timeout(Some(mut_timeout));
    let body = serde_json::to_string(req)
        .map_err(|_| PhaseBError::Incomplete("http::dial: request serialize failed"))?;
    let bearer = format!("Bearer {token}");
    let mutation_resp =
        http_post(&mut stream, &host, &origin, Some(&bearer), &body).map_err(|_| {
            // A broken stream here is exactly the H1 abort: the connection we proved the app on
            // is gone. We do NOT reconnect — that could hand the Bearer to a re-bound squatter.
            PhaseBError::Incomplete(
            "http::dial: stream dropped after challenge (H1) — ABORT, never reconnect-and-Bearer",
        )
        })?;
    if mutation_resp.status != 200 {
        // A non-200 on the mutation after a verified challenge is an app-level transport
        // failure (e.g. the app returned 401/400 unexpectedly). Surface as app-down-ish so
        // the caller does not treat it as a clean reply.
        return Err(PhaseBError::Incomplete(
            "http::dial: mutation non-200 after verified challenge",
        ));
    }
    serde_json::from_slice::<SocketResponse>(&mutation_resp.body)
        .map_err(|_| PhaseBError::Incomplete("http::dial: malformed app reply"))
}

/// One parsed HTTP response: status code + body bytes.
struct HttpResponse {
    status: u16,
    body: Vec<u8>,
}

/// Hand-rolled minimal HTTP/1.1 POST over an OPEN `TcpStream`, keep-alive. Writes the
/// request, reads the response (status line + headers + body), and returns it. Reuses the
/// SAME stream for the next request (H1 — the caller never reconnects between the
/// challenge and the Bearer mutation). 127.0.0.1 plaintext only; NO pooling, NO TLS.
fn http_post(
    stream: &mut TcpStream,
    host: &str,
    origin: &str,
    bearer: Option<&str>,
    body: &str,
) -> std::io::Result<HttpResponse> {
    let mut req = String::new();
    req.push_str("POST / HTTP/1.1\r\n");
    req.push_str(&format!("Host: {host}\r\n"));
    req.push_str(&format!("Origin: {origin}\r\n"));
    req.push_str("Content-Type: application/json\r\n");
    req.push_str("Connection: keep-alive\r\n");
    if let Some(b) = bearer {
        req.push_str(&format!("Authorization: {b}\r\n"));
    }
    req.push_str(&format!("Content-Length: {}\r\n", body.len()));
    req.push_str("\r\n");
    req.push_str(body);

    stream.write_all(req.as_bytes())?;
    stream.flush()?;

    read_response(stream)
}

/// Read ONE HTTP/1.1 response from the stream: parse the status line + headers, then read
/// exactly `Content-Length` body bytes (the app always sends a Content-Length for its JSON
/// replies). Bounded by `HTTP_MAX_RESPONSE_BYTES` so a misbehaving peer can't grow us
/// unbounded. Keeps the stream OPEN (keep-alive) for the next request on the same socket.
fn read_response(stream: &mut TcpStream) -> std::io::Result<HttpResponse> {
    // Read until we have the full header block (\r\n\r\n), accumulating any body bytes that
    // arrive in the same read.
    let mut buf: Vec<u8> = Vec::with_capacity(1024);
    let mut tmp = [0u8; 1024];
    let header_end = loop {
        if let Some(pos) = find_subsequence(&buf, b"\r\n\r\n") {
            break pos;
        }
        if buf.len() > HTTP_MAX_RESPONSE_BYTES {
            return Err(std::io::Error::other("response headers exceed cap"));
        }
        let n = stream.read(&mut tmp)?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "connection closed before headers complete",
            ));
        }
        buf.extend_from_slice(&tmp[..n]);
    };

    let header_block = &buf[..header_end];
    let header_text = String::from_utf8_lossy(header_block);
    let mut lines = header_text.split("\r\n");

    // Status line: "HTTP/1.1 200 OK".
    let status_line = lines.next().unwrap_or("");
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| std::io::Error::other("malformed HTTP status line"))?;

    // Content-Length header (case-insensitive). The app always sends one for its replies.
    let mut content_length: usize = 0;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse::<usize>().unwrap_or(0);
            }
        }
    }
    if content_length > HTTP_MAX_RESPONSE_BYTES {
        return Err(std::io::Error::other("response body exceeds cap"));
    }

    // Body: bytes already buffered past the header block, plus any remaining to reach
    // Content-Length.
    let body_start = header_end + 4; // skip the \r\n\r\n
    let mut body: Vec<u8> = buf[body_start..].to_vec();
    while body.len() < content_length {
        let want = (content_length - body.len()).min(tmp.len());
        let n = stream.read(&mut tmp[..want])?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "connection closed before body complete",
            ));
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(content_length);

    Ok(HttpResponse { status, body })
}

/// Find the first index of `needle` in `haystack` (small, no extra dep).
fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_teams_core::compute_challenge_mac;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};

    /// A throwaway root with a `state` subdir; siblings (config/token/port) live in root.
    struct Scratch {
        root: std::path::PathBuf,
    }
    impl Scratch {
        fn new(tag: &str) -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let root = std::env::temp_dir().join(format!("at-http-dial-{tag}-{nonce}"));
            std::fs::create_dir_all(root.join("state")).unwrap();
            Scratch { root }
        }
        fn state_dir(&self) -> std::path::PathBuf {
            self.root.join("state")
        }
        /// Write mcp-config.json (sibling of state_dir) with the given gate flags.
        fn write_config(&self, allow_mutations: bool, http_enabled: bool) {
            let body =
                format!(r#"{{"allow_mutations":{allow_mutations},"http_enabled":{http_enabled}}}"#);
            std::fs::write(self.root.join("mcp-config.json"), body).unwrap();
        }
        /// Write the 0600 token (sibling of state_dir).
        fn write_token(&self, token: &str) {
            let p = self.root.join("agent-teams-mcp-http.token");
            std::fs::write(&p, token).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        /// Write the port file (sibling of state_dir).
        fn write_port(&self, port: u16) {
            std::fs::write(
                self.root.join("agent-teams-mcp-http.port"),
                port.to_string(),
            )
            .unwrap();
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn known_token() -> String {
        "a".repeat(64)
    }

    /// Read one HTTP request (status line + headers + Content-Length body) off the stream.
    /// Returns (had_authorization, body_string).
    fn read_one_http_request(stream: &mut std::net::TcpStream) -> (bool, String) {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut had_auth = false;
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                break; // end of headers
            }
            if let Some((name, value)) = trimmed.split_once(':') {
                if name.trim().eq_ignore_ascii_case("authorization") {
                    had_auth = true;
                }
                if name.trim().eq_ignore_ascii_case("content-length") {
                    content_length = value.trim().parse().unwrap_or(0);
                }
            }
        }
        let mut body = vec![0u8; content_length];
        use std::io::Read as _;
        let _ = reader.read_exact(&mut body);
        (had_auth, String::from_utf8_lossy(&body).to_string())
    }

    /// Extract the 64-hex nonce from a challenge body `{"op":"identity_challenge","nonce":"..."}`.
    fn nonce_from(body: &str) -> Option<String> {
        let v: serde_json::Value = serde_json::from_str(body).ok()?;
        v.get("nonce")
            .and_then(|n| n.as_str())
            .map(|s| s.to_string())
    }

    fn write_http_200_json(stream: &mut std::net::TcpStream, json: &str) {
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{}",
            json.len(),
            json
        );
        let _ = stream.write_all(resp.as_bytes());
        let _ = stream.flush();
    }

    // AC-1 + happy path: a GENUINE listener (holds the token) answers the challenge with the
    // correct core MAC, then receives the Bearer mutation ON THE SAME stream and replies ok.
    // Asserts: the CHALLENGE request carried NO Authorization (AC-1); the mutation request
    // DID carry one; the dial returns the app's ok reply.
    #[test]
    fn ac1_happy_path_challenge_no_bearer_then_bearer_mutation_one_stream() {
        let s = Scratch::new("happy");
        let token = known_token();
        s.write_config(true, true);
        s.write_token(&token);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        s.write_port(port);

        let captured = Arc::new(Mutex::new((false, false))); // (challenge_had_auth, mutation_had_auth)
        let cap = captured.clone();
        let tok = token.clone();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            // Request 1: the challenge (NO Bearer expected).
            let (auth1, body1) = read_one_http_request(&mut stream);
            let nonce = nonce_from(&body1).expect("challenge carries a nonce");
            let mac = compute_challenge_mac(&tok, &nonce).unwrap();
            write_http_200_json(&mut stream, &format!(r#"{{"ok":true,"mac":"{mac}"}}"#));
            // Request 2: the mutation, ON THE SAME stream (Bearer expected).
            let (auth2, _body2) = read_one_http_request(&mut stream);
            write_http_200_json(&mut stream, r#"{"ok":true,"code":"OK","detail":"done"}"#);
            *cap.lock().unwrap() = (auth1, auth2);
        });

        let req = SocketRequest::Focus { id: "w1".into() };
        let resp = dial_http_op(&s.state_dir(), &req).expect("genuine listener round-trips");
        assert!(resp.ok, "the app's ok reply transports");
        handle.join().unwrap();

        let (challenge_had_auth, mutation_had_auth) = *captured.lock().unwrap();
        assert!(
            !challenge_had_auth,
            "AC-1: the challenge must carry NO Authorization"
        );
        assert!(mutation_had_auth, "the mutation must carry the Bearer");
    }

    // AC-2 (static squat): a squatter binds the stale port from the START and captures every
    // header. It cannot forge the MAC (no token), so the dial ABORTS — and the squatter NEVER
    // sees an Authorization header.
    #[test]
    fn ac2_static_squat_never_receives_bearer() {
        let s = Scratch::new("squat");
        s.write_config(true, true);
        s.write_token(&known_token());

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        s.write_port(port);

        let saw_auth = Arc::new(Mutex::new(false));
        let saw = saw_auth.clone();
        let handle = std::thread::spawn(move || {
            // The squatter answers the challenge with a WRONG (zeroed) MAC — it cannot forge.
            if let Ok((mut stream, _)) = listener.accept() {
                let (auth1, _body1) = read_one_http_request(&mut stream);
                if auth1 {
                    *saw.lock().unwrap() = true;
                }
                let wrong_mac = "0".repeat(64);
                write_http_200_json(
                    &mut stream,
                    &format!(r#"{{"ok":true,"mac":"{wrong_mac}"}}"#),
                );
                // If the dial (wrongly) sent a second request, capture its Authorization.
                let _ = stream.set_read_timeout(Some(Duration::from_millis(300)));
                let (auth2, _b2) = read_one_http_request(&mut stream);
                if auth2 {
                    *saw.lock().unwrap() = true;
                }
            }
        });

        let req = SocketRequest::Focus { id: "w1".into() };
        let r = dial_http_op(&s.state_dir(), &req);
        assert!(
            r.is_err(),
            "a forged MAC must ABORT the dial (no Bearer sent)"
        );
        let _ = handle.join();
        assert!(
            !*saw_auth.lock().unwrap(),
            "AC-2: the squatter must NEVER see a Bearer"
        );
    }

    // AC-9 (dynamic squat — H1): the genuine listener answers the challenge correctly, THEN
    // is killed (stream + listener dropped), THEN a squatter binds the freed port before
    // request 2. The kept-alive stream is broken → the dial DETECTS the drop and ABORTS →
    // the squatter NEVER receives an Authorization header, and the dial NEVER reconnects-and-
    // sends-Bearer to the freed port.
    //
    // The squatter uses a NON-BLOCKING, time-bounded accept poll so the test is deterministic
    // and never hangs: the dial MUST NOT reconnect, so the squatter simply observes NO
    // connection within a generous window (which exceeds the dial's mutation read timeout).
    #[test]
    fn ac9_dynamic_squat_inter_request_window_never_leaks_bearer() {
        let s = Scratch::new("dynsquat");
        let token = known_token();
        s.write_config(true, true);
        s.write_token(&token);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        s.write_port(port);

        // Channel: the genuine thread signals "challenge answered + closing" so the squatter
        // rebinds the freed port deterministically.
        let (tx, rx) = mpsc::channel::<()>();
        let squatter_saw_auth = Arc::new(Mutex::new(false));

        let tok = token.clone();
        let genuine = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let (_auth1, body1) = read_one_http_request(&mut stream);
            let nonce = nonce_from(&body1).unwrap();
            let mac = compute_challenge_mac(&tok, &nonce).unwrap();
            write_http_200_json(&mut stream, &format!(r#"{{"ok":true,"mac":"{mac}"}}"#));
            // KILL the genuine listener: drop the stream AND the listener → the port frees.
            drop(stream);
            drop(listener);
            let _ = tx.send(());
        });

        // The squatter waits for the genuine listener to close, rebinds the freed port, then
        // polls (non-blocking) for ANY connection within a bounded window. The dial MUST NOT
        // reconnect → the squatter observes nothing.
        let saw = squatter_saw_auth.clone();
        let squatter = std::thread::spawn(move || {
            rx.recv().ok();
            // Rebind the freed port. Retry briefly — the OS may take a moment to free it.
            let mut bound = None;
            for _ in 0..100 {
                if let Ok(l) = TcpListener::bind(("127.0.0.1", port)) {
                    bound = Some(l);
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            if let Some(l) = bound {
                l.set_nonblocking(true).unwrap();
                // Poll for up to ~2s for a (forbidden) reconnect. The dial's mutation read
                // timeout is FAST_OP_TIMEOUT (5s), but the WRITE to the dead peer + the read
                // failure surface well before that on loopback; 2s is ample to observe the
                // abort without a connect arriving.
                let deadline = std::time::Instant::now() + Duration::from_millis(2000);
                while std::time::Instant::now() < deadline {
                    match l.accept() {
                        Ok((mut stream, _)) => {
                            let _ = stream.set_read_timeout(Some(Duration::from_millis(300)));
                            let (auth, _b) = read_one_http_request(&mut stream);
                            if auth {
                                *saw.lock().unwrap() = true;
                            }
                            break;
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        Err(_) => break,
                    }
                }
            }
        });

        let req = SocketRequest::Focus { id: "w1".into() };
        let r = dial_http_op(&s.state_dir(), &req);
        assert!(
            r.is_err(),
            "H1: the dropped stream after the challenge must ABORT the dial"
        );
        let _ = genuine.join();
        let _ = squatter.join();
        assert!(
            !*squatter_saw_auth.lock().unwrap(),
            "AC-9: the rebound squatter must NEVER receive a Bearer (no reconnect-and-Bearer)"
        );
    }

    // Discovery fail-closed: http_enabled=false ⇒ refuse with no dial.
    #[test]
    fn refuses_when_http_disabled() {
        let s = Scratch::new("httpoff");
        s.write_config(true, false);
        s.write_token(&known_token());
        s.write_port(12345);
        let req = SocketRequest::Focus { id: "w".into() };
        assert!(dial_http_op(&s.state_dir(), &req).is_err());
    }

    // Discovery fail-closed: allow_mutations=false ⇒ refuse.
    #[test]
    fn refuses_when_mutations_disabled() {
        let s = Scratch::new("mutoff");
        s.write_config(false, true);
        s.write_token(&known_token());
        s.write_port(12345);
        let req = SocketRequest::Focus { id: "w".into() };
        assert!(dial_http_op(&s.state_dir(), &req).is_err());
    }

    // Discovery fail-closed: empty token ⇒ refuse (mirror bearer_token_matches empty rule).
    #[test]
    fn refuses_on_empty_token() {
        let s = Scratch::new("emptytok");
        s.write_config(true, true);
        s.write_token("");
        s.write_port(12345);
        let req = SocketRequest::Focus { id: "w".into() };
        assert!(dial_http_op(&s.state_dir(), &req).is_err());
    }

    // Discovery fail-closed: looser-than-0600 token perms ⇒ refuse.
    #[test]
    fn refuses_on_loose_token_perms() {
        let s = Scratch::new("loosetok");
        s.write_config(true, true);
        let p = s.root.join("agent-teams-mcp-http.token");
        std::fs::write(&p, known_token()).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        s.write_port(12345);
        let req = SocketRequest::Focus { id: "w".into() };
        assert!(dial_http_op(&s.state_dir(), &req).is_err());
    }

    // Discovery fail-closed: absent / corrupt port file ⇒ refuse (S1).
    #[test]
    fn refuses_on_missing_or_corrupt_port() {
        let s = Scratch::new("badport");
        s.write_config(true, true);
        s.write_token(&known_token());
        // No port file written → S1 abort.
        let req = SocketRequest::Focus { id: "w".into() };
        assert!(dial_http_op(&s.state_dir(), &req).is_err());
        // Corrupt port file → still aborts.
        std::fs::write(s.root.join("agent-teams-mcp-http.port"), "not-a-port").unwrap();
        assert!(dial_http_op(&s.state_dir(), &req).is_err());
    }
}
