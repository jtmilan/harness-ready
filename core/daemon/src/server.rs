//! The daemon's accept-loop socket SERVER — the role-inversion request path
//! (Phase 08 Sub-build 3 / slice 2).
//!
//! This is the daemon-side mirror of the app's `serve_socket_conn` +
//! `spawn_socket_listener` (`app/src-tauri/src/lib.rs`), but with three hardened
//! differences the role-inversion security review made CONDITIONAL-GO must-fixes:
//!
//! * **MF-A — one thread PER CONNECTION (not serial).** The app serves connections
//!   serially on one listener thread; here [`serve`] spawns a fresh thread per
//!   accepted connection so a slow same-user peer can wedge only its OWN connection,
//!   never the listener or another peer (Finding 2.1). Each connection thread also
//!   wraps the per-request handler in [`std::panic::catch_unwind`] so a panicked
//!   request can never unwind past the loop and take the connection (or, since the
//!   listener thread only spawns, the listener) down.
//!
//! * **MF-C — `allow_mutations` is read FRESH per mutating request.** Never cached at
//!   startup: [`handle_conn`] re-reads [`agent_teams_core::read_mcp_config`] for every
//!   mutating op, so toggling the gate in `mcp-config.json` takes effect on the next
//!   request (mirror of the app's per-request read). Absent/malformed config ⇒ the
//!   safe default (mutations OFF).
//!
//! * **MF-D — per-op gate routing.** Read ops (`ListLive`/`Attach`/`Detach`) are
//!   EUID-gated ONLY (no `allow_mutations`), so a default install can re-attach; the
//!   mutating ops additionally pass the fresh `allow_mutations` gate. The classifier
//!   is the shared [`agent_teams_core::op_requires_mutations`].
//!
//! ## What this slice does NOT do
//!
//! * NO streaming / snapshot / delta / subscription machinery. `Attach`/`Detach` are
//!   answered with [`response_code::STREAMING_UNAVAILABLE`] and the connection is kept
//!   ALIVE (slice 3 replaces these stubs with real subscription handling).
//! * NO spawn / PTY-ownership-transfer: the server is built + tested against a
//!   test-populated [`DaemonSups`] (how panes get INTO the daemon's map is design Q4,
//!   deferred). The handlers are generic over the stored value via [`PaneWrite`], so
//!   the tests drive them with the same `FakePane` double the slice-1 handlers use.
//! * The synthesis/orchestration ops (`Orchestrate`/`Broadcast`/`Handoff`/`Synthesize`/
//!   `Delegate`) WRAP the in-app synthesizer (design §7); the daemon does NOT relocate
//!   it — it answers [`response_code::SERVED_BY_APP`] and the GUI routes them to the app.

use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::TryRecvError;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::frames::{code as frame_code, StreamFrame};
use crate::handlers::{handle_send_input, PaneStream, PaneWrite};
use crate::sups::DaemonSups;
use agent_teams_core::{
    op_requires_mutations, op_served_by_app, read_mcp_config, response_code, SocketData,
    SocketRequest, SocketResponse, FAST_OP_TIMEOUT,
};
use supervisor::subscribers::{subscribe_to, Subscription, DEFAULT_SUB_CAPACITY};

/// Poll cadence for the streaming connection loop (§5 Q2). While a connection has ACTIVE
/// subscriptions the read window is this short interval so the loop wakes promptly to
/// DRAIN each subscription's queue and flush delta frames even when the peer sends no
/// further request. With NO active subscription the loop awaits at [`FAST_OP_TIMEOUT`]
/// (the slice-2 await window). Delta latency is bounded by one interval.
const STREAM_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Idle window while subscriptions are active (§6): if NO request arrives AND NO delta is
/// produced for this long, the connection emits a `keepalive` liveness probe (NOT a close)
/// — a healthy connection to a merely-quiet pane must survive, so only a peer that has
/// GONE (the probe write fails) is reaped. Distinct from [`FAST_OP_TIMEOUT`] (the
/// await-a-request window). A pane that is actively streaming resets the window on every
/// delta, so the probe only fires on a genuinely idle subscription.
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// Cap on a single request line so a same-user peer that never sends a newline can't
/// grow daemon memory unbounded. Mirrors the app's `MAX_REQUEST_BYTES` (64 KiB).
const MAX_REQUEST_BYTES: u64 = 64 * 1024;

/// Bounded cap on concurrently-served connections (MF-A). A same-user connection flood
/// (or a buggy client) must not grow daemon handler threads without limit: once this many
/// are in flight, further connections are refused with [`response_code::BUSY`] WITHOUT
/// spawning a thread. Generous enough that a legitimate GUI (one app + a handful of
/// re-attach probes) never approaches it; revisit alongside slice-3's long-lived `Attach`
/// subscriptions (which PIN a thread for the connection's whole lifetime).
const MAX_INFLIGHT_CONNS: usize = 128;

/// The accept loop. ONE THREAD PER CONNECTION (MF-A) — mirrors the app's
/// `spawn_socket_listener` accept-error handling, but spawns a connection thread per
/// `incoming()` instead of serving serially, so a wedged peer never blocks the listener
/// or another connection.
///
/// `state_root` is threaded so each connection thread can read `mcp-config.json` FRESH
/// per mutating request (MF-C). `sups` is the daemon's live-pane map (a test-populated
/// stand-in in the unit tests; the real `DaemonSups<Supervisor>` in production once
/// PTY-ownership transfer lands — design Q4).
pub fn serve<V>(listener: UnixListener, sups: Arc<DaemonSups<V>>, state_root: PathBuf)
where
    V: PaneWrite + PaneStream + Send + SpawnRouteCap + 'static,
{
    // Q4: one process-global spawn-state (primed set / in-flight counter / child-pid map /
    // worktree registry) shared by every connection thread. Feature-gated — the default
    // build constructs nothing here.
    #[cfg(feature = "daemon-spawn")]
    let spawn_state = Arc::new(crate::spawn::DaemonSpawnState::new());
    // Q4 reaper (D1/D2 + TTL): a background thread sweeps dead/expired daemon-owned panes
    // (the app's frontend `dead_pane_ids` sweep, relocated daemon-side) so an exited/expired
    // agent drops `count_live` and the live registry never returns a corpse. Feature-gated;
    // detached for the daemon's lifetime.
    #[cfg(feature = "daemon-spawn")]
    {
        let reap_sups = Arc::clone(&sups);
        let reap_state = Arc::clone(&spawn_state);
        let reap_root = state_root.clone();
        std::thread::Builder::new()
            .name("daemon-q4-reaper".into())
            .spawn(move || loop {
                std::thread::sleep(Duration::from_secs(30));
                <V as crate::spawn::DaemonSpawnRoutable>::route_reap(
                    &reap_sups,
                    &reap_state,
                    &reap_root,
                );
            })
            .expect("spawn daemon-q4-reaper thread");
    }
    // Q4 AC-4 idle-shutdown (D4): start the idle ticker HERE so the in-flight-spawn hold
    // (`pending`) is actually consumed. The decision count is `count_live() + pending_count()`
    // — the design's `count_live ≥ 1 OR pending > 0 → HoldOpen` — so a first-spawn-after-grace
    // tick that fires BETWEEN the `PendingGuard` increment and the map insert never sees 0 live
    // and SIGHUPs the fresh agent (D4 was built but, until now, wired to NOTHING — the OR-term
    // was unreachable). Held for the daemon's lifetime (the accept loop below never returns).
    // Feature-gated: the DEFAULT build starts no ticker (byte-inert, unchanged lifecycle) and
    // never constructs `spawn_state`.
    #[cfg(feature = "daemon-spawn")]
    let _idle_handle = {
        let idle_sups = Arc::clone(&sups);
        let idle_state = Arc::clone(&spawn_state);
        crate::idle_tick::IdleTicker::default().spawn(
            // The OR-term made reachable: pending in-flight spawns hold the daemon open.
            move || idle_sups.count_live() + idle_state.pending_count(),
            // gui_attached has NO effect on the AC-4 decision (idle = zero live, not "no GUI").
            || false,
            || std::process::exit(0),
        )
    };
    // In-flight handler-thread count for the bounded concurrency cap (MF-A).
    let inflight = Arc::new(AtomicUsize::new(0));
    for conn in listener.incoming() {
        match conn {
            Ok(mut stream) => {
                // EUID GATE BEFORE SPAWN (MF-A): reject a foreign peer WITHOUT paying a
                // thread spawn. `handle_conn` re-checks the euid once more (defense in
                // depth + it stays unit-testable standalone over a socketpair), but a peer
                // that will be rejected must never cost a thread here.
                match socket_peer_euid(&stream) {
                    Some(peer) if peer_allowed(peer, our_euid()) => {}
                    _ => {
                        write_response(
                            &mut stream,
                            &SocketResponse::err(
                                response_code::FORBIDDEN,
                                "peer euid does not match the daemon (same-user only)",
                            ),
                        );
                        continue;
                    }
                }
                // BOUNDED CONCURRENCY CAP (MF-A): a connection flood cannot grow threads
                // without limit. At the cap, refuse with BUSY (no spawn). The count is
                // decremented on ANY thread exit (clean/error/panic) via the RAII guard.
                if inflight.load(Ordering::SeqCst) >= MAX_INFLIGHT_CONNS {
                    write_response(
                        &mut stream,
                        &SocketResponse::err(
                            response_code::BUSY,
                            "daemon connection cap reached; retry shortly",
                        ),
                    );
                    continue;
                }
                inflight.fetch_add(1, Ordering::SeqCst);
                let s = Arc::clone(&sups);
                let sr = state_root.clone();
                let inflight_for_thread = Arc::clone(&inflight);
                #[cfg(feature = "daemon-spawn")]
                let spawn_state_for_thread = Arc::clone(&spawn_state);
                // ONE THREAD PER CONNECTION (MF-A): a slow/wedged peer wedges only its
                // own thread; the listener returns to `accept()` immediately.
                std::thread::spawn(move || {
                    // Decrement the in-flight count on ANY exit (clean, read error, or a
                    // handler panic that escaped the per-request catch_unwind).
                    struct InflightGuard(Arc<AtomicUsize>);
                    impl Drop for InflightGuard {
                        fn drop(&mut self) {
                            self.0.fetch_sub(1, Ordering::SeqCst);
                        }
                    }
                    let _inflight_guard = InflightGuard(inflight_for_thread);
                    #[cfg(not(feature = "daemon-spawn"))]
                    let router = SupRouter { sups: s };
                    #[cfg(feature = "daemon-spawn")]
                    let router = SupRouter {
                        sups: s,
                        state_root: sr.clone(),
                        spawn_state: spawn_state_for_thread,
                    };
                    handle_conn(stream, &sr, router);
                });
            }
            Err(e) => {
                // Mirror the app: an accept error is logged and the loop continues —
                // one bad connection never kills the listener.
                eprintln!("[daemon] mcp socket: accept error: {e} (continuing)");
            }
        }
    }
}

/// A per-connection request router. The streaming connection loop ([`handle_conn`]) is
/// generic over this so the unit tests can drive it with a plain closure (gate/panic
/// coverage) while production uses [`SupRouter`] (real streaming over [`DaemonSups`]).
///
/// `Attach`/`Detach` are NOT routed through [`one_shot`](ConnRouter::one_shot) — the loop
/// intercepts them to manage per-connection subscriptions (§5 Q2, the NON-exclusive
/// loop). Everything else goes through `one_shot` under the loop's panic boundary.
trait ConnRouter {
    /// Route a one-shot op (everything EXCEPT `Attach`/`Detach`) to a response.
    fn one_shot(&self, req: SocketRequest) -> SocketResponse;
    /// `Attach`: atomically snapshot + register pane `id` (the subscription key is minted
    /// process-globally inside the registry, not by the connection). Default `Unavailable`
    /// (closure routers don't stream).
    fn attach(&self, _id: &str) -> AttachResult {
        AttachResult::Unavailable
    }
    /// Is pane `id` still alive? Used to distinguish a `PANE_DIED` close from an
    /// `OVERFLOW` drop when a subscription's channel disconnects. Default `false`.
    fn pane_alive(&self, _id: &str) -> bool {
        false
    }
}

/// The outcome of [`ConnRouter::attach`].
enum AttachResult {
    /// Streaming established: the atomic snapshot + the live subscription to track.
    Ok(Subscription),
    /// The pane was absent or already dead → `PANE_DIED` (no half-upgrade).
    PaneDied,
    /// This router does not stream (closure test routers) → `STREAMING_UNAVAILABLE`.
    Unavailable,
}

/// Closures route one-shot ops only; they neither stream nor know liveness. This blanket
/// impl lets the gate/panic unit tests keep passing a plain `Fn` as the router.
impl<F: Fn(SocketRequest) -> SocketResponse> ConnRouter for F {
    fn one_shot(&self, req: SocketRequest) -> SocketResponse {
        self(req)
    }
}

/// The production router over the daemon's live-pane map. `one_shot` reuses
/// [`route_request`]; `attach` does the lock-shed snapshot + register (§4); `pane_alive`
/// reads liveness under the map lock (the CONNECTION thread MAY take the map lock — only
/// the READER fan-out is forbidden from doing so).
struct SupRouter<V> {
    sups: Arc<DaemonSups<V>>,
    /// Q4 (feature only): the state_root for the FRESH `daemon_spawn_enabled` read + the
    /// registry/audit writes inside the spawn handlers.
    #[cfg(feature = "daemon-spawn")]
    state_root: PathBuf,
    /// Q4 (feature only): the shared spawn-state (primed set / in-flight / child-pid /
    /// worktree registry).
    #[cfg(feature = "daemon-spawn")]
    spawn_state: Arc<crate::spawn::DaemonSpawnState>,
}

/// Feature-gating shim so the single `impl ConnRouter for SupRouter<V>` can require
/// `V: DaemonSpawnRoutable` ONLY under the `daemon-spawn` feature without duplicating the
/// whole impl. WITHOUT the feature it is a blanket no-op bound (all `V` qualify → the
/// existing streaming tests' `SupRouter<StreamFakePane>` compile unchanged); WITH the
/// feature it forces `V` to be spawn-routable (production `Supervisor`).
#[cfg(feature = "daemon-spawn")]
pub trait SpawnRouteCap: crate::spawn::DaemonSpawnRoutable {}
#[cfg(feature = "daemon-spawn")]
impl<T: crate::spawn::DaemonSpawnRoutable> SpawnRouteCap for T {}
#[cfg(not(feature = "daemon-spawn"))]
pub trait SpawnRouteCap {}
#[cfg(not(feature = "daemon-spawn"))]
impl<T> SpawnRouteCap for T {}

impl<V: PaneWrite + PaneStream + SpawnRouteCap> ConnRouter for SupRouter<V> {
    fn one_shot(&self, req: SocketRequest) -> SocketResponse {
        // Q4 interception (feature only): `Spawn`/`Close` are routed to the gated daemon
        // handlers AHEAD of `route_request` (which has no spawn-state and would only return
        // the `SPAWN_UNAVAILABLE` fallback); the FIRST `SendInput` to a claude pane carries
        // the C7 banner-prime. The euid + fresh `allow_mutations` gates have ALREADY passed
        // upstream (`handle_request_line`); `handle_spawn` then checks `daemon_spawn_enabled`.
        // One `let resp = …` per cfg branch is active; the other is compiled out.
        #[cfg(feature = "daemon-spawn")]
        let resp = match req {
            SocketRequest::Spawn { spec } => <V as crate::spawn::DaemonSpawnRoutable>::route_spawn(
                &self.sups,
                &self.spawn_state,
                &self.state_root,
                spec,
            ),
            SocketRequest::Close { id } => <V as crate::spawn::DaemonSpawnRoutable>::route_close(
                &self.sups,
                &self.spawn_state,
                &self.state_root,
                &id,
            ),
            SocketRequest::SendInput { id, text } => {
                crate::handlers::maybe_prime_claude(
                    &self.sups,
                    self.spawn_state.primed_handle(),
                    &id,
                );
                route_request(&self.sups, SocketRequest::SendInput { id, text })
            }
            other => route_request(&self.sups, other),
        };
        #[cfg(not(feature = "daemon-spawn"))]
        let resp = route_request(&self.sups, req);
        resp
    }

    fn attach(&self, id: &str) -> AttachResult {
        // LOCK SHEDDING (mirror `handle_read_output`): under the map lock, gate liveness
        // and clone the two O(1) per-pane handles, then DROP the map lock. The
        // snapshot-copy + register happens off the map lock, under buffer→subscriber only.
        let handles = self.sups.with_mut(id, |sup| {
            if sup.is_alive() {
                Some(sup.stream_handles())
            } else {
                None
            }
        });
        let (buf, subs) = match handles {
            Some(Some(h)) => h,                                 // present + alive
            Some(None) | None => return AttachResult::PaneDied, // present-but-dead, or absent
        };
        // Map lock released. ATOMIC snapshot+register under buffer→subscriber (§4): the
        // first delta is GUARANTEED contiguous with the snapshot baseline. The
        // attach-vs-reader-death race (liveness gated `true` above but the reader thread
        // exited before we register) is closed INSIDE `subscribe_to`: it checks the set's
        // dead flag under the subscriber lock and returns `None` → PANE_DIED, never a
        // silent never-closing zombie subscription.
        match subscribe_to(&buf, &subs, DEFAULT_SUB_CAPACITY) {
            Some(sub) => AttachResult::Ok(sub),
            None => AttachResult::PaneDied,
        }
    }

    fn pane_alive(&self, id: &str) -> bool {
        self.sups
            .with_mut(id, |sup| sup.is_alive())
            .unwrap_or(false)
    }
}

/// One live subscription tracked by a connection (§5 Q1 multiplex: a connection owns a
/// `Vec` of these, one per attached pane). Dropping it DEREGISTERS from the pane's
/// registry, so closing the connection drops every subscription (`Vec` drop → each
/// `ActiveSub` drop).
struct ActiveSub {
    pane_id: String,
    key: u64,
    rx: std::sync::mpsc::Receiver<supervisor::subscribers::OutputDelta>,
    registry: supervisor::subscribers::SubscriberHandle,
}

impl Drop for ActiveSub {
    fn drop(&mut self) {
        // Deregister takes ONLY the subscriber lock (never buffer / map) → no ordering
        // hazard, and must not panic (recover a poisoned lock).
        self.registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .deregister(self.key);
    }
}

/// Serve one connection (§5 Q2 — the NON-exclusive streaming loop): euid-gate ONCE, then
/// loop. Each iteration reads whatever request bytes are available (short poll window
/// while subscriptions are active, [`FAST_OP_TIMEOUT`] while awaiting), routes any
/// complete request lines, THEN drains every active subscription's bounded queue, writing
/// pending delta/error frames. One-shot ops and `Attach`/`Detach` are routed UNIFORMLY in
/// the same loop — there is no exclusive "subscription active" state that rejects a
/// one-shot op while streaming.
///
/// Gate order per request (MF-C/MF-D): parse → SERVED_BY_APP routing → (if mutating)
/// FRESH `allow_mutations` read → dispatch (under the MF-A panic boundary). The euid gate
/// is checked once up front (the peer cannot change uid mid-connection).
fn handle_conn<R: ConnRouter>(mut stream: UnixStream, state_root: &Path, router: R) {
    let _ = stream.set_read_timeout(Some(FAST_OP_TIMEOUT));
    let _ = stream.set_write_timeout(Some(FAST_OP_TIMEOUT));

    // PEER-CRED (euid) — the load-bearing same-user gate, checked ONCE. Fail closed on
    // any libc error. A mismatch ⇒ one FORBIDDEN reply, then the connection is dropped.
    match socket_peer_euid(&stream) {
        Some(peer) if peer_allowed(peer, our_euid()) => {}
        _ => {
            write_response(
                &mut stream,
                &SocketResponse::err(
                    response_code::FORBIDDEN,
                    "peer euid does not match the daemon (same-user only)",
                ),
            );
            return;
        }
    }

    let mut read_stream = stream.try_clone().expect("clone unix stream for read");
    let mut pending: Vec<u8> = Vec::new();
    let mut active: Vec<ActiveSub> = Vec::new();
    let mut last_activity = Instant::now();

    loop {
        let streaming = !active.is_empty();
        let read_to = if streaming {
            STREAM_POLL_INTERVAL
        } else {
            FAST_OP_TIMEOUT
        };
        let _ = read_stream.set_read_timeout(Some(read_to));

        match read_available(&mut read_stream, &mut pending) {
            ReadOutcome::Eof => return,
            ReadOutcome::TooLong => {
                // A single request line exceeded the cap with no newline — cannot safely
                // resync, so reply BAD_REQUEST and close (mirror the app's MAX_REQUEST cap).
                let _ = write_resp(
                    &mut stream,
                    &SocketResponse::err(response_code::BAD_REQUEST, "request line exceeds cap"),
                );
                return;
            }
            ReadOutcome::Idle => {
                if !streaming {
                    // Awaiting a request with no subscriptions and the peer went silent →
                    // close (the slice-2 await-timeout behavior).
                    return;
                }
                // Streaming idle tick → fall through to drain + the idle-deadline check.
            }
            ReadOutcome::Lines(lines) => {
                for line in lines {
                    last_activity = Instant::now();
                    if !handle_request_line(&line, &router, state_root, &mut stream, &mut active) {
                        return; // a write failed (peer gone) → reap connection + all subs
                    }
                }
            }
        }

        // Drain every active subscription → flush pending delta/error frames (MF-B write
        // timeout per frame). A frame write failure reaps the whole connection.
        if !drain_subscriptions(&mut active, &mut stream, &router, &mut last_activity) {
            return;
        }

        // Idle deadline while streaming: no request AND no delta for the window. Do NOT
        // reap a HEALTHY connection whose subscribed panes are merely quiet (agent panes sit
        // idle far longer than the window — reaping them would churn re-attach + waste a
        // snapshot copy every cycle). Instead emit a keepalive probe: a write FAILURE
        // (peer gone, or a non-reading peer whose socket buffer filled → write timeout)
        // reaps the connection; a SUCCESSFUL probe proves liveness and resets the deadline.
        if !active.is_empty() && last_activity.elapsed() >= STREAM_IDLE_TIMEOUT {
            if !write_frame(&mut stream, &StreamFrame::keepalive()) {
                return;
            }
            last_activity = Instant::now();
        }
    }
}

/// The outcome of one [`read_available`] call.
enum ReadOutcome {
    /// One or more COMPLETE request lines (trailing `\n` included) were read.
    Lines(Vec<String>),
    /// The read timed out (no full line yet) — the loop drains subscriptions and retries.
    Idle,
    /// The peer closed the connection, or an unrecoverable read error.
    Eof,
    /// A single unterminated line grew past [`MAX_REQUEST_BYTES`] (a peer that never
    /// sends a newline) — the memory-bound trip.
    TooLong,
}

/// Read whatever bytes are currently available (one `read`, honoring the stream's
/// timeout) into `pending`, then split out every COMPLETE newline-terminated line. A
/// multibyte UTF-8 char split across two reads is reassembled before the line is cut
/// (lines are only formed at a `\n`, which is ASCII). A timeout with no full line is
/// `Idle` (so the loop can still drain subscriptions). The unterminated-tail length is
/// capped (`TooLong`) so a peer that never sends `\n` cannot grow memory unbounded.
fn read_available(stream: &mut UnixStream, pending: &mut Vec<u8>) -> ReadOutcome {
    let mut chunk = [0u8; 4096];
    match stream.read(&mut chunk) {
        Ok(0) => ReadOutcome::Eof,
        Ok(n) => {
            pending.extend_from_slice(&chunk[..n]);
            // Memory bound: an unterminated line past the cap (no newline anywhere).
            if pending.len() as u64 > MAX_REQUEST_BYTES && !pending.contains(&b'\n') {
                return ReadOutcome::TooLong;
            }
            let mut lines = Vec::new();
            while let Some(pos) = pending.iter().position(|&b| b == b'\n') {
                let raw: Vec<u8> = pending.drain(..=pos).collect();
                // A non-UTF-8 line becomes empty → BAD_REQUEST downstream (never panics).
                lines.push(String::from_utf8(raw).unwrap_or_default());
            }
            if lines.is_empty() {
                ReadOutcome::Idle // partial data buffered, no full line yet
            } else {
                ReadOutcome::Lines(lines)
            }
        }
        // A read TIMEOUT (no data within the window) is the normal streaming poll tick.
        Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {
            ReadOutcome::Idle
        }
        // Any other error (broken pipe etc.) closes THIS connection only.
        Err(_) => ReadOutcome::Eof,
    }
}

/// Parse + gate + route ONE request line. Returns `false` only on a fatal WRITE failure
/// (peer gone) so the caller reaps the connection. `Attach`/`Detach` are handled here
/// (they mutate the per-connection subscription set); all other ops go through
/// `router.one_shot` under the MF-A panic boundary.
fn handle_request_line<R: ConnRouter>(
    line: &str,
    router: &R,
    state_root: &Path,
    stream: &mut UnixStream,
    active: &mut Vec<ActiveSub>,
) -> bool {
    let req = match serde_json::from_str::<SocketRequest>(line.trim_end()) {
        Ok(r) => r,
        Err(_) => {
            return write_resp(
                stream,
                &SocketResponse::err(response_code::BAD_REQUEST, "malformed request"),
            )
        }
    };

    // MF-D routing FIRST (ahead of the mutation gate): synthesis ops are SERVED_BY_APP
    // regardless of `allow_mutations`, so the GUI can tell "wrong endpoint" from
    // "capability off" and re-route. (Attach/Detach are not synthesis ops → fall through.)
    if op_served_by_app(&req) {
        return write_resp(
            stream,
            &SocketResponse::err(
                response_code::SERVED_BY_APP,
                "synthesis/orchestration ops are served by the app, not the daemon",
            ),
        );
    }
    // MF-C/MF-D: read the capability gate FRESH per MUTATING request. Read ops
    // (ListLive/Attach/Detach) skip this gate. `SendInput` has its OWN narrow axis
    // `send_input_enabled` (decoupled from the broad `allow_mutations`), kept in lock-step with the
    // app-side handler in app/src-tauri/src/lib.rs so the two fresh-reads can never drift.
    if op_requires_mutations(&req) {
        let cfg = read_mcp_config(state_root);
        if matches!(req, SocketRequest::SendInput { .. }) {
            if !cfg.send_input_enabled {
                return write_resp(
                    stream,
                    &SocketResponse::err(
                        response_code::SEND_INPUT_DISABLED,
                        "send_input_enabled is false in mcp-config.json (arm it in Settings)",
                    ),
                );
            }
        } else if !cfg.allow_mutations {
            return write_resp(
                stream,
                &SocketResponse::err(
                    response_code::MUTATIONS_DISABLED,
                    "allow_mutations is false in mcp-config.json",
                ),
            );
        }
    }

    match req {
        SocketRequest::Attach { id } => handle_attach(&id, router, stream, active),
        SocketRequest::Detach { id } => handle_detach(&id, stream, active),
        other => {
            // MF-A panic boundary: a one-shot handler panic is caught so it can never
            // unwind past the loop and drop the connection.
            let resp = match std::panic::catch_unwind(AssertUnwindSafe(|| router.one_shot(other))) {
                Ok(r) => r,
                Err(payload) => {
                    let msg = panic_message(&payload);
                    eprintln!(
                        "[daemon] mcp socket: handler panic caught (connection survives): {msg}"
                    );
                    SocketResponse::err("HANDLER_PANIC", msg)
                }
            };
            write_resp(stream, &resp)
        }
    }
}

/// `Attach{id}`: atomically snapshot + register, reply `STREAMING`, then send the
/// snapshot frame and start tracking the subscription. A re-`Attach` to an
/// already-subscribed pane drops the old subscription first (fresh snapshot). Pane
/// absent/dead → `PANE_DIED` (no half-upgrade; the connection stays in awaiting state).
fn handle_attach<R: ConnRouter>(
    id: &str,
    router: &R,
    stream: &mut UnixStream,
    active: &mut Vec<ActiveSub>,
) -> bool {
    // Re-attach: drop any existing subscription for this pane (Drop deregisters it) so the
    // client gets a single fresh snapshot rather than two overlapping subscriptions.
    if let Some(pos) = active.iter().position(|a| a.pane_id == id) {
        active.remove(pos);
    }
    match router.attach(id) {
        AttachResult::Ok(sub) => {
            let Subscription {
                baseline,
                snapshot,
                key: sub_key,
                rx,
                registry,
            } = sub;
            // TRACK the subscription FIRST: if a frame write below fails (peer gone), the
            // connection reaps and this `ActiveSub`'s Drop deregisters it — no leaked
            // subscriber left in the pane registry.
            active.push(ActiveSub {
                pane_id: id.to_string(),
                key: sub_key,
                rx,
                registry,
            });
            // Reply STREAMING (request/response correlation), THEN the snapshot frame —
            // always before any delta (the loop's drain runs only after this returns).
            let streaming_ok = SocketResponse {
                ok: true,
                code: response_code::STREAMING.to_string(),
                detail: "streaming".to_string(),
                data: None,
            };
            if !write_resp(stream, &streaming_ok) {
                return false;
            }
            write_frame(stream, &StreamFrame::snapshot(id, baseline, &snapshot))
        }
        AttachResult::PaneDied => write_resp(
            stream,
            &SocketResponse::err(response_code::PANE_DIED, "pane absent or no longer alive"),
        ),
        AttachResult::Unavailable => write_resp(
            stream,
            &SocketResponse::err(
                response_code::STREAMING_UNAVAILABLE,
                "streaming is not available on this endpoint",
            ),
        ),
    }
}

/// `Detach{id}`: drop the subscription for `id` on this connection (its `Drop`
/// deregisters from the pane registry). Other subscriptions persist. No-op OK if absent.
fn handle_detach(id: &str, stream: &mut UnixStream, active: &mut Vec<ActiveSub>) -> bool {
    if let Some(pos) = active.iter().position(|a| a.pane_id == id) {
        active.remove(pos);
    }
    write_resp(stream, &SocketResponse::ok("detached"))
}

/// Drain EVERY active subscription's bounded queue, writing the pending frames (§5 Q1
/// multiplex — each frame tagged with its pane id). A subscription whose channel is
/// `Disconnected` is CLOSED with one error frame and removed: `PANE_DIED` if the pane is
/// gone, `OVERFLOW` if the pane is still alive (the reader dropped this slow subscription
/// to stay unblocked — MF-B). The connection itself survives unless a frame WRITE fails
/// (peer gone), which returns `false` to reap it. `last_activity` advances on every frame.
fn drain_subscriptions<R: ConnRouter>(
    active: &mut Vec<ActiveSub>,
    stream: &mut UnixStream,
    router: &R,
    last_activity: &mut Instant,
) -> bool {
    // ROUND-ROBIN (fairness): each outer pass takes AT MOST ONE frame from every
    // subscription before revisiting the first, then repeats until a whole pass yields
    // nothing. A hot/backlogged sub can no longer monopolize the drain and delay a
    // sibling's pending delta — or, worse, a sibling's OVERFLOW/PANE_DIED error frame —
    // for the duration of its backlog. Terminates: each pass makes bounded progress, and a
    // sub is either emptied (Empty) or removed (Disconnected) eventually.
    loop {
        let mut progressed = false;
        let mut i = 0;
        while i < active.len() {
            match active[i].rx.try_recv() {
                Ok(delta) => {
                    let frame = StreamFrame::delta(
                        &active[i].pane_id,
                        delta.prev_total,
                        delta.new_total,
                        &delta.data,
                    );
                    if !write_frame(stream, &frame) {
                        return false;
                    }
                    *last_activity = Instant::now();
                    progressed = true;
                    i += 1; // one frame this pass → move on to the next sub (round-robin)
                }
                Err(TryRecvError::Empty) => i += 1,
                Err(TryRecvError::Disconnected) => {
                    // The sender(s) dropped: either the reader's close-on-death marked the
                    // registry DEAD (pane gone) or the reader dropped THIS sub on overflow
                    // (pane still alive). The AUTHORITATIVE death signal is the registry's
                    // `dead` flag the reader sets the instant it exits — checked FIRST, since
                    // `pane_alive` (child `try_wait`) LAGS PTY-EOF and would misclassify a
                    // just-died pane as OVERFLOW. Fall back to liveness only when not dead
                    // (the connection MAY take the map lock; only the reader fan-out may not).
                    let reader_gone = active[i]
                        .registry
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .is_dead();
                    let (c, detail) = if !reader_gone && router.pane_alive(&active[i].pane_id) {
                        (
                            frame_code::OVERFLOW,
                            "subscription dropped (slow consumer); re-attach for a fresh snapshot",
                        )
                    } else {
                        (frame_code::PANE_DIED, "pane died")
                    };
                    let frame = StreamFrame::error(&active[i].pane_id, c, detail);
                    active.remove(i); // Drop deregisters (no-op: already gone from the set)
                    *last_activity = Instant::now();
                    if !write_frame(stream, &frame) {
                        return false;
                    }
                    progressed = true;
                    // do NOT increment i: the Vec shifted, so index `i` now holds the next sub.
                }
            }
        }
        if !progressed {
            break;
        }
    }
    true
}

/// Route ONE parsed ONE-SHOT request to its handler (MF-D). The euid + (for mutating ops)
/// fresh `allow_mutations` gate have ALREADY passed before this runs. `Attach`/`Detach`
/// are NOT routed here — the connection loop ([`handle_conn`]) intercepts them to manage
/// per-connection subscriptions; the arms below are an unreachable defensive fallback for
/// a direct call (kept only so the match stays exhaustive).
///
/// * `ListLive` → the live pane ids (+ optional metadata). EUID-gated only (MF-D).
/// * `SendInput` → the slice-1 [`handle_send_input`] (normalize → split-submit).
/// * `Focus` → daemon-local pane existence check.
/// * synthesis ops → [`response_code::SERVED_BY_APP`].
fn route_request<V: PaneWrite>(sups: &DaemonSups<V>, req: SocketRequest) -> SocketResponse {
    match req {
        SocketRequest::ListLive => {
            // EUID-only read op (MF-D): expose the live id set even when mutations are
            // OFF (the AC-6 anti-double-spawn re-attach path). `workspaces` is the
            // optional metadata channel; slice 2 returns the minimal ids-only reply
            // (the per-id LiveWorkspace rows are populated once spawn/registry-writer
            // ownership lands — design Q4).
            SocketResponse::ok("live").with_data(SocketData::LivePanes {
                ids: sups.live_ids(),
                workspaces: None,
            })
        }
        SocketRequest::SendInput { id, text } => handle_send_input(sups, &id, &text),
        SocketRequest::Focus { id } => {
            if sups.contains(&id) {
                SocketResponse::ok("focused")
            } else {
                SocketResponse::err(response_code::UNKNOWN_WORKSPACE, "no such workspace")
            }
        }
        SocketRequest::Orchestrate { .. }
        | SocketRequest::Broadcast { .. }
        | SocketRequest::Handoff { .. }
        | SocketRequest::Synthesize { .. }
        | SocketRequest::Delegate { .. }
        // #262 ext: external visible-grid spawn is APP-served (only the app's webview owns
        // the grid + createWorkspace) — same SERVED_BY_APP reroute as the synthesis ops.
        | SocketRequest::CreateWorkspace { .. }
        | SocketRequest::AddPane { .. }
        // gap-7: the live-scrollback read is APP-served (the app owns both live buffer
        // seams — its PaneBuffer registry AND the attach-stream buffers for daemon panes).
        // `op_served_by_app` already intercepts this in handle_request_line; this arm is
        // the same defensive fallback the other app-served ops keep.
        | SocketRequest::ReadOutput { .. } => SocketResponse::err(
            response_code::SERVED_BY_APP,
            "synthesis/orchestration ops are served by the app, not the daemon",
        ),
        // Loop-intercepted (see the doc above) — defensive fallback only.
        SocketRequest::Attach { .. } | SocketRequest::Detach { .. } => SocketResponse::err(
            response_code::STREAMING_UNAVAILABLE,
            "streaming (Attach/Detach) is handled by the connection loop, not route_request",
        ),
        // Q4 daemon-spawns-on-behalf (§5). With the `daemon-spawn` feature ON these are
        // intercepted by [`SupRouter::one_shot`] AHEAD of this fallback (so it has the
        // spawn-state + state_root the handlers need); this arm is reached only when the
        // feature is COMPILED OUT (the default) → `SPAWN_UNAVAILABLE`, on which the GUI
        // falls back to local spawn. Keeps the match EXHAUSTIVE either way.
        SocketRequest::Spawn { .. } | SocketRequest::Close { .. } => SocketResponse::err(
            response_code::SPAWN_UNAVAILABLE,
            "daemon-spawn is not compiled into this build (feature off) — use local spawn",
        ),
    }
}

/// Serialize a response as one JSON line and write it (best-effort — used for the
/// pre-loop FORBIDDEN/BUSY replies where there is no connection state to reap).
fn write_response(stream: &mut UnixStream, resp: &SocketResponse) {
    if let Ok(mut out) = serde_json::to_string(resp) {
        out.push('\n');
        let _ = stream.write_all(out.as_bytes());
        let _ = stream.flush();
    }
}

/// Write one [`SocketResponse`] line with the MF-B write timeout. Returns `false` on a
/// write failure (peer gone) so the caller reaps the connection.
fn write_resp(stream: &mut UnixStream, resp: &SocketResponse) -> bool {
    match serde_json::to_string(resp) {
        Ok(mut out) => {
            out.push('\n');
            write_all_timed(stream, out.as_bytes())
        }
        // A serialize failure is not the peer's fault → don't reap the connection.
        Err(_) => true,
    }
}

/// Write one [`StreamFrame`] line with the MF-B write timeout. Returns `false` on a write
/// failure (a write timeout reaps the connection and ALL its subscriptions).
fn write_frame(stream: &mut UnixStream, frame: &StreamFrame) -> bool {
    match serde_json::to_string(frame) {
        Ok(mut out) => {
            out.push('\n');
            write_all_timed(stream, out.as_bytes())
        }
        Err(_) => true,
    }
}

/// `write_all` + `flush` under [`FAST_OP_TIMEOUT`] (MF-B). A timeout / broken pipe ⇒
/// `false` (reap the connection); success ⇒ `true`.
fn write_all_timed(stream: &mut UnixStream, bytes: &[u8]) -> bool {
    let _ = stream.set_write_timeout(Some(FAST_OP_TIMEOUT));
    stream
        .write_all(bytes)
        .and_then(|()| stream.flush())
        .is_ok()
}

/// Format a `catch_unwind` payload (a `Box<dyn Any>`, either a `&str` or `String` from
/// `panic!`) into a log/diagnostic string. Mirrors the app's panic-payload formatting.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

// ───────────────────────── peer-cred (euid) — same-user gate ─────────────────────────
//
// The daemon has no AppHandle; these mirror the app's `socket_peer_euid` / `our_euid` /
// `peer_allowed` (`app/src-tauri/src/lib.rs`) so the SAME same-user boundary is enforced
// at the daemon's accept loop. Fail closed on any libc error.

/// Read the connected peer's euid via `getpeereid` (macOS; NOT SO_PEERCRED). `None` on
/// any libc error (the caller treats `None` as a hard reject — fail closed).
fn socket_peer_euid(stream: &UnixStream) -> Option<u32> {
    use std::os::unix::io::AsRawFd;
    let mut euid: libc::uid_t = 0;
    let mut egid: libc::gid_t = 0;
    // SAFETY: fd is a live, connected AF_UNIX stream socket (we just accepted it);
    // getpeereid writes two out-params we own. Non-zero return ⇒ failure ⇒ None.
    let rc = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut euid, &mut egid) };
    if rc == 0 {
        Some(euid as u32)
    } else {
        None
    }
}

/// The daemon's own euid (`geteuid`) — the value the peer must match.
fn our_euid() -> u32 {
    // SAFETY: geteuid always succeeds and takes no args.
    unsafe { libc::geteuid() as u32 }
}

/// Same-user check: the peer is allowed iff its euid matches ours. A root peer is still
/// a DIFFERENT user → rejected (mirror the app's `peer_allowed`).
fn peer_allowed(peer_euid: u32, our_euid: u32) -> bool {
    peer_euid == our_euid
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;
    use std::thread;
    use supervisor::subscribers::{
        close_subscribers, overflow_drop_subscribers, push_and_fanout, SubscriberHandle,
        SubscriberSet,
    };
    use supervisor::PaneBuffer;

    /// A cheap pane stand-in (the slice-1 handlers' `FakePane`): records PTY writes in
    /// order, exposes a wire string + liveness — the exact [`PaneWrite`] surface. A real
    /// `Supervisor` owns a spawned PTY child and can't be built in a unit test.
    #[derive(Debug, Clone)]
    struct FakePane {
        harness: &'static str,
        alive: bool,
        writes: Vec<Vec<u8>>,
    }
    impl FakePane {
        fn new(harness: &'static str) -> Self {
            FakePane {
                harness,
                alive: true,
                writes: Vec::new(),
            }
        }
    }
    impl PaneWrite for FakePane {
        fn is_alive(&mut self) -> bool {
            self.alive
        }
        fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
            self.writes.push(data.to_vec());
            Ok(())
        }
        fn harness_wire(&self) -> &'static str {
            self.harness
        }
    }

    /// Read exactly one newline-delimited JSON response from a stream.
    fn read_response(stream: &UnixStream) -> SocketResponse {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).expect("read response line");
        serde_json::from_str(line.trim_end()).expect("parse response")
    }

    /// Send one request line on a stream (no implicit trailing newline rules — we add it).
    fn send_request(stream: &mut UnixStream, req: &SocketRequest) {
        let mut s = serde_json::to_string(req).unwrap();
        s.push('\n');
        stream.write_all(s.as_bytes()).unwrap();
        stream.flush().unwrap();
    }

    /// Write an `mcp-config.json` SIBLING of `state_root` with the given `allow_mutations`.
    /// Returns the state_root path (a fresh temp dir, with its parent holding the config).
    fn temp_state_root_with_mutations(allow: bool) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "at-daemon-srv-{}-{}",
            std::process::id(),
            // a per-call nonce so concurrent tests never collide on the same config file
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let state_root = base.join("state");
        std::fs::create_dir_all(&state_root).unwrap();
        let cfg = agent_teams_core::mcp_config_path(&state_root).unwrap();
        std::fs::write(&cfg, format!(r#"{{"allow_mutations":{allow}}}"#)).unwrap();
        state_root
    }

    // SendInput is gated by its OWN narrow `send_input_enabled` axis (decoupled from
    // `allow_mutations`). This writes a config arming/disarming ONLY that axis.
    fn temp_state_root_with_send_input(enabled: bool) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "at-daemon-srv-si-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let state_root = base.join("state");
        std::fs::create_dir_all(&state_root).unwrap();
        let cfg = agent_teams_core::mcp_config_path(&state_root).unwrap();
        std::fs::write(&cfg, format!(r#"{{"send_input_enabled":{enabled}}}"#)).unwrap();
        state_root
    }

    /// Spawn a connection-handler thread over a socketpair (same-user by construction →
    /// the euid ACCEPT path), returning the client end + a join handle. `router` routes
    /// requests (a plain closure for the gate/panic tests via the blanket [`ConnRouter`]
    /// impl, or a [`SupRouter`] for streaming); `state_root` feeds the fresh mutation gate.
    fn spawn_handler<R>(state_root: PathBuf, router: R) -> (UnixStream, thread::JoinHandle<()>)
    where
        R: ConnRouter + Send + 'static,
    {
        let (server, client) = UnixStream::pair().expect("socketpair");
        let h = thread::spawn(move || {
            handle_conn(server, &state_root, router);
        });
        (client, h)
    }

    // ───────────────── slice-3 streaming: PaneStream test double + helpers ─────────────────

    /// A streaming-capable pane double built on the REAL [`PaneBuffer`] + [`SubscriberSet`]
    /// substrate, so a test drives the EXACT production push/fanout/subscribe code (not a
    /// re-implementation). Cloning shares all state via `Arc`s: the copy stored in
    /// `DaemonSups` and the test's handle operate on the SAME buffer + registry, so a test
    /// `feed`/`die` is visible to the subscription the connection's `Attach` registered.
    #[derive(Clone)]
    struct StreamFakePane {
        harness: &'static str,
        alive: Arc<AtomicBool>,
        buf: Arc<Mutex<PaneBuffer>>,
        subs: SubscriberHandle,
        writes: Arc<Mutex<Vec<Vec<u8>>>>,
    }
    impl StreamFakePane {
        fn new(harness: &'static str) -> Self {
            StreamFakePane {
                harness,
                alive: Arc::new(AtomicBool::new(true)),
                buf: Arc::new(Mutex::new(PaneBuffer::new(1024))),
                subs: Arc::new(Mutex::new(SubscriberSet::new())),
                writes: Arc::new(Mutex::new(Vec::new())),
            }
        }
        /// Mimic the PTY reader appending a chunk: append + fan out (the exact production
        /// critical section).
        fn feed(&self, chunk: &[u8]) {
            push_and_fanout(&self.buf, &self.subs, chunk);
        }
        /// Mimic pane death: the reader's loop ends → close all senders, and the pane is no
        /// longer alive.
        fn die(&self) {
            self.alive.store(false, Ordering::SeqCst);
            close_subscribers(&self.subs);
        }
        /// Mimic the reader DROPPING this slow subscription on overflow (MF-B): the sender
        /// is dropped while the pane stays ALIVE and the set is NOT marked dead — the
        /// connection should emit OVERFLOW, not PANE_DIED.
        fn overflow_drop(&self) {
            overflow_drop_subscribers(&self.subs);
        }
        /// Mimic the attach-vs-death RACE: the PTY reader thread has exited (close_all → the
        /// registry is marked dead) but the child has not been reaped yet, so `is_alive()`
        /// still races `true`. An `Attach` in this window must NOT register a zombie — it
        /// must observe the dead set and return PANE_DIED.
        fn reader_exited_child_lingers(&self) {
            close_subscribers(&self.subs); // dead set, but `alive` stays true
        }
    }
    impl PaneWrite for StreamFakePane {
        fn is_alive(&mut self) -> bool {
            self.alive.load(Ordering::SeqCst)
        }
        fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
            self.writes.lock().unwrap().push(data.to_vec());
            Ok(())
        }
        fn harness_wire(&self) -> &'static str {
            self.harness
        }
    }
    impl PaneStream for StreamFakePane {
        fn stream_handles(&self) -> (Arc<Mutex<PaneBuffer>>, SubscriberHandle) {
            (self.buf.clone(), self.subs.clone())
        }
    }

    /// Spawn the real [`SupRouter`] connection loop over a streaming `DaemonSups`.
    fn spawn_stream_handler(
        state_root: PathBuf,
        sups: Arc<DaemonSups<StreamFakePane>>,
    ) -> (UnixStream, thread::JoinHandle<()>) {
        #[cfg(not(feature = "daemon-spawn"))]
        let router = SupRouter { sups };
        #[cfg(feature = "daemon-spawn")]
        let router = SupRouter {
            sups,
            state_root: state_root.clone(),
            spawn_state: Arc::new(crate::spawn::DaemonSpawnState::new()),
        };
        spawn_handler(state_root, router)
    }

    // Q4 (feature build only): the streaming test double must satisfy the feature-on
    // `SpawnRouteCap` bound. It never receives a `Spawn`, so its routing impl is a trivial
    // stub — only needed so `cargo test --features daemon-spawn` compiles.
    #[cfg(feature = "daemon-spawn")]
    impl crate::spawn::DaemonPane for StreamFakePane {
        fn kill(&mut self) {
            self.alive.store(false, Ordering::SeqCst);
        }
        fn child_pid(&self) -> Option<u32> {
            None
        }
    }
    #[cfg(feature = "daemon-spawn")]
    impl crate::spawn::DaemonSpawnRoutable for StreamFakePane {
        fn route_spawn(
            _sups: &DaemonSups<Self>,
            _state: &crate::spawn::DaemonSpawnState,
            _state_root: &Path,
            _spec: agent_teams_core::SpawnSpec,
        ) -> SocketResponse {
            SocketResponse::err(response_code::SPAWN_UNAVAILABLE, "test double")
        }
        fn route_close(
            _sups: &DaemonSups<Self>,
            _state: &crate::spawn::DaemonSpawnState,
            _state_root: &Path,
            _id: &str,
        ) -> SocketResponse {
            SocketResponse::ok("closed")
        }
        fn route_reap(
            _sups: &DaemonSups<Self>,
            _state: &crate::spawn::DaemonSpawnState,
            _state_root: &Path,
        ) {
        }
    }

    /// A persistent newline-JSON line reader for the streaming tests — ONE `BufReader` for
    /// the whole connection so buffered bytes (a frame read while looking for a response)
    /// are never lost between reads. A 5s read timeout turns a missing frame into a clear
    /// test failure rather than a hang.
    struct Lines {
        r: BufReader<UnixStream>,
    }
    impl Lines {
        fn new(stream: &UnixStream) -> Self {
            let c = stream.try_clone().unwrap();
            c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            Lines {
                r: BufReader::new(c),
            }
        }
        fn next(&mut self) -> serde_json::Value {
            let mut line = String::new();
            let n = self
                .r
                .read_line(&mut line)
                .expect("read a frame/response line");
            assert!(n > 0, "unexpected EOF waiting for a frame/response");
            serde_json::from_str(line.trim_end()).expect("parse json line")
        }
    }

    /// Decode a frame's base64 `data` field to bytes.
    fn frame_data(v: &serde_json::Value) -> Vec<u8> {
        crate::frames::b64_decode(v["data"].as_str().expect("data field")).expect("valid base64")
    }

    // ── euid gate ──

    #[test]
    fn socketpair_peer_is_same_user_and_allowed() {
        // The security-critical syscall on a REAL connected fd: a same-process socketpair
        // is same-user by construction, so getpeereid returns our euid ⇒ allowed.
        let (a, _b) = UnixStream::pair().expect("socketpair");
        let peer = socket_peer_euid(&a).expect("getpeereid on a live fd");
        assert_eq!(peer, our_euid());
        assert!(peer_allowed(peer, our_euid()));
        // A foreign euid is rejected (the REJECT half — a different uid is unreachable
        // headless, but the pure decision is asserted here).
        assert!(!peer_allowed(peer.wrapping_add(1), our_euid()));
    }

    // ── MF-D: ListLive is EUID-only (works with allow_mutations=false) ──

    #[test]
    fn list_live_returns_live_panes_even_with_mutations_off() {
        // MF-D: ListLive must work on a default install (mutations OFF) — it is the AC-6
        // re-attach path. A test-populated DaemonSups returns its live ids in LivePanes.
        let state_root = temp_state_root_with_mutations(false);
        let sups: Arc<DaemonSups<FakePane>> = Arc::new(DaemonSups::new());
        sups.insert("ws-1", FakePane::new("claude"));
        sups.insert("ws-2", FakePane::new("codex"));
        let s = Arc::clone(&sups);
        let (mut client, h) = spawn_handler(state_root, move |req| route_request(&s, req));

        send_request(&mut client, &SocketRequest::ListLive);
        let resp = read_response(&client);
        assert!(
            resp.ok,
            "ListLive must succeed with mutations off: {resp:?}"
        );
        match resp.data {
            Some(SocketData::LivePanes {
                mut ids,
                workspaces,
            }) => {
                ids.sort();
                assert_eq!(ids, vec!["ws-1".to_string(), "ws-2".to_string()]);
                assert!(
                    workspaces.is_none(),
                    "slice 2 returns the minimal ids-only reply"
                );
            }
            other => panic!("expected LivePanes, got {other:?}"),
        }
        drop(client);
        h.join().unwrap();
    }

    // ── MF-C: SendInput refused when mutations off, accepted+normalized when on ──

    #[test]
    fn send_input_refused_when_send_input_disabled() {
        // The narrow gate: SendInput with send_input_enabled=false ⇒ SEND_INPUT_DISABLED, and the
        // pane receives NO bytes (the gate fires before the handler). Decoupled from allow_mutations.
        let state_root = temp_state_root_with_send_input(false);
        let sups: Arc<DaemonSups<FakePane>> = Arc::new(DaemonSups::new());
        sups.insert("ws", FakePane::new("claude"));
        let s = Arc::clone(&sups);
        let (mut client, h) = spawn_handler(state_root, move |req| route_request(&s, req));

        send_request(
            &mut client,
            &SocketRequest::SendInput {
                id: "ws".into(),
                text: "approve".into(),
            },
        );
        let resp = read_response(&client);
        assert!(!resp.ok);
        assert_eq!(resp.code, response_code::SEND_INPUT_DISABLED);
        assert!(
            sups.with_snapshot("ws", |p| p.writes.clone())
                .unwrap()
                .is_empty(),
            "a refused send-input must write no bytes"
        );
        drop(client);
        h.join().unwrap();
    }

    #[test]
    fn send_input_accepted_and_normalized_when_send_input_enabled() {
        // send_input_enabled=true ⇒ the op routes through handle_send_input (normalize →
        // split-submit), so a claude pane gets the body then the lone \r as SEPARATE writes
        // (the normalize-appended submit). Armed via the narrow axis, NOT allow_mutations.
        let state_root = temp_state_root_with_send_input(true);
        let sups: Arc<DaemonSups<FakePane>> = Arc::new(DaemonSups::new());
        sups.insert("ws", FakePane::new("claude"));
        let s = Arc::clone(&sups);
        let (mut client, h) = spawn_handler(state_root, move |req| route_request(&s, req));

        send_request(
            &mut client,
            &SocketRequest::SendInput {
                id: "ws".into(),
                text: "approve".into(),
            },
        );
        let resp = read_response(&client);
        assert!(
            resp.ok,
            "SendInput must succeed with mutations on: {resp:?}"
        );
        assert_eq!(resp.code, response_code::OK);
        let writes = sups.with_snapshot("ws", |p| p.writes.clone()).unwrap();
        assert_eq!(
            writes,
            vec![b"approve".to_vec(), b"\r".to_vec()],
            "normalize → split-submit: body then the lone \\r"
        );
        drop(client);
        h.join().unwrap();
    }

    // ── MF-D: synthesis ops get the served-by-app code ──

    #[test]
    fn synthesis_op_is_served_by_app() {
        // Orchestrate WRAPS the in-app synthesizer (design §7); the daemon refuses it with
        // SERVED_BY_APP. Mutations are ON so the op clears the gate and reaches the router
        // (proving the SERVED_BY_APP decision is the router's, not the gate's).
        let state_root = temp_state_root_with_mutations(true);
        let sups: Arc<DaemonSups<FakePane>> = Arc::new(DaemonSups::new());
        let s = Arc::clone(&sups);
        let (mut client, h) = spawn_handler(state_root, move |req| route_request(&s, req));

        send_request(
            &mut client,
            &SocketRequest::Orchestrate {
                goal: "ship it".into(),
                dispatch: false,
                target_workspace: None,
            },
        );
        let resp = read_response(&client);
        assert!(!resp.ok);
        assert_eq!(resp.code, response_code::SERVED_BY_APP);
        drop(client);
        h.join().unwrap();
    }

    #[test]
    fn synthesis_op_is_served_by_app_even_with_mutations_off() {
        // MF-D routing on a DEFAULT install (mutations OFF): a synthesis op must STILL get
        // SERVED_BY_APP — NOT MUTATIONS_DISABLED — so the GUI can tell "wrong endpoint" from
        // "capability off" and re-route. This is the case the old mutations-ON test masked.
        let state_root = temp_state_root_with_mutations(false);
        let sups: Arc<DaemonSups<FakePane>> = Arc::new(DaemonSups::new());
        let s = Arc::clone(&sups);
        let (mut client, h) = spawn_handler(state_root, move |req| route_request(&s, req));

        send_request(
            &mut client,
            &SocketRequest::Orchestrate {
                goal: "preview only".into(),
                dispatch: false,
                target_workspace: None,
            },
        );
        let resp = read_response(&client);
        assert!(!resp.ok);
        assert_eq!(
            resp.code,
            response_code::SERVED_BY_APP,
            "synthesis op must route SERVED_BY_APP even with mutations OFF, got {resp:?}"
        );
        drop(client);
        h.join().unwrap();
    }

    // ── slice 3: real streaming — snapshot-on-attach, then deltas ──

    #[test]
    fn attach_returns_snapshot_then_deltas_arrive_as_frames() {
        // Attach (EUID-only) → STREAMING reply + a snapshot frame carrying recent()+baseline;
        // subsequent reader chunks arrive as delta frames with correct prev/new totals and a
        // base64 round-trip.
        let state_root = temp_state_root_with_mutations(false);
        let sups: Arc<DaemonSups<StreamFakePane>> = Arc::new(DaemonSups::new());
        let pane = StreamFakePane::new("claude");
        pane.feed(b"hello "); // pre-existing scrollback before the attach
        sups.insert("ws", pane.clone());
        let (mut client, h) = spawn_stream_handler(state_root, Arc::clone(&sups));
        let mut lines = Lines::new(&client);

        send_request(&mut client, &SocketRequest::Attach { id: "ws".into() });
        let resp = lines.next();
        assert_eq!(resp["ok"], true, "attach reply: {resp}");
        assert_eq!(resp["code"], response_code::STREAMING);

        let snap = lines.next();
        assert_eq!(snap["frame"], "snapshot");
        assert_eq!(snap["id"], "ws");
        assert_eq!(snap["baseline"], 6, "baseline == total_pushed at attach");
        assert_eq!(snap["data_len"], 6);
        assert_eq!(frame_data(&snap), b"hello ", "snapshot == recent() window");

        // a reader chunk after the snapshot → a delta frame contiguous with the baseline.
        pane.feed(b"world");
        let d1 = lines.next();
        assert_eq!(d1["frame"], "delta");
        assert_eq!(d1["id"], "ws");
        assert_eq!(
            d1["prev_total"], 6,
            "first delta is contiguous with the snapshot baseline"
        );
        assert_eq!(d1["new_total"], 11);
        assert_eq!(frame_data(&d1), b"world");

        pane.feed(b"!");
        let d2 = lines.next();
        assert_eq!(
            d2["prev_total"], 11,
            "deltas are contiguous (no gap in a live sub)"
        );
        assert_eq!(d2["new_total"], 12);
        assert_eq!(frame_data(&d2), b"!");

        // Drop the Lines reader's cloned fd too, so the server sees EOF promptly (a
        // lingering dup would otherwise hold the connection open until the idle timeout).
        drop(lines);
        drop(client);
        h.join().unwrap();
    }

    #[test]
    fn attach_to_absent_pane_is_pane_died_and_connection_survives() {
        // A pane absent from the map → PANE_DIED (no half-upgrade), and the connection stays
        // in awaiting-request state — a ListLive on the SAME connection still works.
        let state_root = temp_state_root_with_mutations(false);
        let sups: Arc<DaemonSups<StreamFakePane>> = Arc::new(DaemonSups::new());
        sups.insert("ws", StreamFakePane::new("claude"));
        let (mut client, h) = spawn_stream_handler(state_root, Arc::clone(&sups));
        let mut lines = Lines::new(&client);

        send_request(&mut client, &SocketRequest::Attach { id: "ghost".into() });
        let resp = lines.next();
        assert_eq!(resp["ok"], false);
        assert_eq!(resp["code"], response_code::PANE_DIED);

        send_request(&mut client, &SocketRequest::ListLive);
        let r = lines.next();
        assert_eq!(r["ok"], true, "connection survives a failed attach: {r}");
        drop(lines);
        drop(client);
        h.join().unwrap();
    }

    #[test]
    fn attach_to_dead_pane_is_pane_died() {
        // A pane present but failing is_alive() → PANE_DIED (the D30 gate at attach time).
        let state_root = temp_state_root_with_mutations(false);
        let sups: Arc<DaemonSups<StreamFakePane>> = Arc::new(DaemonSups::new());
        let pane = StreamFakePane::new("claude");
        pane.die(); // mark dead before the attach
        sups.insert("ws", pane);
        let (mut client, h) = spawn_stream_handler(state_root, Arc::clone(&sups));
        let mut lines = Lines::new(&client);

        send_request(&mut client, &SocketRequest::Attach { id: "ws".into() });
        let resp = lines.next();
        assert_eq!(resp["ok"], false);
        assert_eq!(resp["code"], response_code::PANE_DIED);
        drop(lines);
        drop(client);
        h.join().unwrap();
    }

    #[test]
    fn pane_death_midstream_emits_a_pane_died_error_frame() {
        // A pane that dies mid-subscription → the reader closes the senders → the connection
        // drains any buffered delta, then emits a PANE_DIED error frame closing that sub.
        let state_root = temp_state_root_with_mutations(false);
        let sups: Arc<DaemonSups<StreamFakePane>> = Arc::new(DaemonSups::new());
        let pane = StreamFakePane::new("claude");
        sups.insert("ws", pane.clone());
        let (mut client, h) = spawn_stream_handler(state_root, Arc::clone(&sups));
        let mut lines = Lines::new(&client);

        send_request(&mut client, &SocketRequest::Attach { id: "ws".into() });
        assert_eq!(lines.next()["code"], response_code::STREAMING); // streaming reply
        assert_eq!(lines.next()["frame"], "snapshot"); // empty snapshot

        pane.feed(b"x");
        let d = lines.next();
        assert_eq!(d["frame"], "delta");
        assert_eq!(frame_data(&d), b"x");

        pane.die();
        let err = lines.next();
        assert_eq!(err["frame"], "error");
        assert_eq!(err["id"], "ws");
        assert_eq!(err["code"], crate::frames::code::PANE_DIED);
        drop(lines);
        drop(client);
        h.join().unwrap();
    }

    #[test]
    fn overflow_drops_the_subscription_with_an_overflow_frame_and_keeps_connection() {
        // A subscription whose sender is dropped while the pane is STILL ALIVE (the reader's
        // MF-B drop-on-overflow) → the connection emits an OVERFLOW error frame (NOT
        // PANE_DIED) and the connection survives (a ListLive afterward still works).
        let state_root = temp_state_root_with_mutations(false);
        let sups: Arc<DaemonSups<StreamFakePane>> = Arc::new(DaemonSups::new());
        let pane = StreamFakePane::new("claude");
        sups.insert("ws", pane.clone());
        let (mut client, h) = spawn_stream_handler(state_root, Arc::clone(&sups));
        let mut lines = Lines::new(&client);

        send_request(&mut client, &SocketRequest::Attach { id: "ws".into() });
        assert_eq!(lines.next()["code"], response_code::STREAMING);
        assert_eq!(lines.next()["frame"], "snapshot");

        pane.overflow_drop(); // reader drops the slow sub; pane stays alive
        let err = lines.next();
        assert_eq!(err["frame"], "error");
        assert_eq!(
            err["code"],
            crate::frames::code::OVERFLOW,
            "alive pane + dropped sub → OVERFLOW"
        );

        send_request(&mut client, &SocketRequest::ListLive);
        assert_eq!(
            lines.next()["ok"],
            true,
            "connection survives an overflow-dropped sub"
        );
        drop(lines);
        drop(client);
        h.join().unwrap();
    }

    #[test]
    fn midstream_death_classifies_pane_died_even_when_liveness_lags() {
        // The split-brain misclassification fix: the reader exits (registry marked dead) but
        // the child isn't reaped yet so `pane_alive()` still races TRUE. The dropped
        // subscription must still close with PANE_DIED (the authoritative `dead` flag), NOT a
        // spurious OVERFLOW ("re-attach for a fresh snapshot") on a genuinely dead pane.
        let state_root = temp_state_root_with_mutations(false);
        let sups: Arc<DaemonSups<StreamFakePane>> = Arc::new(DaemonSups::new());
        let pane = StreamFakePane::new("claude");
        sups.insert("ws", pane.clone());
        let (mut client, h) = spawn_stream_handler(state_root, Arc::clone(&sups));
        let mut lines = Lines::new(&client);

        send_request(&mut client, &SocketRequest::Attach { id: "ws".into() });
        assert_eq!(lines.next()["code"], response_code::STREAMING);
        assert_eq!(lines.next()["frame"], "snapshot");

        // Reader exits (dead registry) but liveness still reads true (child not reaped).
        pane.reader_exited_child_lingers();
        assert!(
            pane.clone().is_alive(),
            "precondition: liveness lags the reader exit"
        );
        let err = lines.next();
        assert_eq!(err["frame"], "error");
        assert_eq!(
            err["code"],
            crate::frames::code::PANE_DIED,
            "dead registry → PANE_DIED even though pane_alive() still races true"
        );
        drop(lines);
        drop(client);
        h.join().unwrap();
    }

    #[test]
    fn multiplex_detach_removes_one_subscription_others_persist() {
        // Two subscriptions on ONE connection (§5 Q1). Detach ws1 → its feed produces no
        // frame; ws2 keeps streaming. Proven by feeding ws1 THEN ws2 and getting ws2's delta
        // as the next frame (ws1's feed dropped silently).
        let state_root = temp_state_root_with_mutations(false);
        let sups: Arc<DaemonSups<StreamFakePane>> = Arc::new(DaemonSups::new());
        let ws1 = StreamFakePane::new("claude");
        let ws2 = StreamFakePane::new("codex");
        sups.insert("ws1", ws1.clone());
        sups.insert("ws2", ws2.clone());
        let (mut client, h) = spawn_stream_handler(state_root, Arc::clone(&sups));
        let mut lines = Lines::new(&client);

        send_request(&mut client, &SocketRequest::Attach { id: "ws1".into() });
        assert_eq!(lines.next()["code"], response_code::STREAMING);
        assert_eq!(lines.next()["frame"], "snapshot");
        send_request(&mut client, &SocketRequest::Attach { id: "ws2".into() });
        assert_eq!(lines.next()["code"], response_code::STREAMING);
        assert_eq!(lines.next()["frame"], "snapshot");

        send_request(&mut client, &SocketRequest::Detach { id: "ws1".into() });
        let det = lines.next();
        assert_eq!(det["ok"], true, "detach is a no-op-OK reply: {det}");

        ws1.feed(b"gone"); // ws1 detached → fans to nobody (no frame)
        ws2.feed(b"live"); // ws2 still subscribed → a delta
        let d = lines.next();
        assert_eq!(d["frame"], "delta");
        assert_eq!(
            d["id"], "ws2",
            "only the still-subscribed pane streams; ws1's feed dropped"
        );
        assert_eq!(frame_data(&d), b"live");
        drop(lines);
        drop(client);
        h.join().unwrap();
    }

    #[test]
    fn multiplex_isolation_overflow_drops_one_sub_other_keeps_streaming() {
        // Two subs on one connection; one is overflow-dropped (alive) while the other keeps
        // receiving deltas — per-subscription isolation (each owns its OWN bounded queue).
        let state_root = temp_state_root_with_mutations(false);
        let sups: Arc<DaemonSups<StreamFakePane>> = Arc::new(DaemonSups::new());
        let ws1 = StreamFakePane::new("claude");
        let ws2 = StreamFakePane::new("codex");
        sups.insert("ws1", ws1.clone());
        sups.insert("ws2", ws2.clone());
        let (mut client, h) = spawn_stream_handler(state_root, Arc::clone(&sups));
        let mut lines = Lines::new(&client);

        send_request(&mut client, &SocketRequest::Attach { id: "ws1".into() });
        assert_eq!(lines.next()["code"], response_code::STREAMING);
        assert_eq!(lines.next()["frame"], "snapshot");
        send_request(&mut client, &SocketRequest::Attach { id: "ws2".into() });
        assert_eq!(lines.next()["code"], response_code::STREAMING);
        assert_eq!(lines.next()["frame"], "snapshot");

        ws1.overflow_drop(); // drop ws1's slow sub (alive) → an OVERFLOW frame for ws1
        let err = lines.next();
        assert_eq!(err["frame"], "error");
        assert_eq!(err["id"], "ws1");
        assert_eq!(err["code"], crate::frames::code::OVERFLOW);

        ws2.feed(b"still here"); // ws2 unaffected → keeps streaming
        let d = lines.next();
        assert_eq!(d["frame"], "delta");
        assert_eq!(d["id"], "ws2");
        assert_eq!(frame_data(&d), b"still here");
        drop(lines);
        drop(client);
        h.join().unwrap();
    }

    #[test]
    fn two_connections_same_pane_one_closes_other_keeps_streaming() {
        // The cross-connection key-collision regression (the slice's headline isolation
        // guarantee). TWO independent connections attach the SAME pane (the AC-6 app-restart
        // overlap: an old connection lingers while a new one re-attaches). The pane's
        // SubscriberSet is SHARED across both. When connection A closes, its ActiveSub::drop
        // deregisters ONLY A's globally-unique key — connection B's subscriber must survive
        // and keep receiving deltas. (With the old per-connection key=0 both would collide
        // and A's close would tear down B → a spurious OVERFLOW frame on B.)
        let state_root = temp_state_root_with_mutations(false);
        let sups: Arc<DaemonSups<StreamFakePane>> = Arc::new(DaemonSups::new());
        let pane = StreamFakePane::new("claude");
        sups.insert("ws", pane.clone());

        let (mut client_a, ha) = spawn_stream_handler(state_root.clone(), Arc::clone(&sups));
        let (mut client_b, hb) = spawn_stream_handler(state_root, Arc::clone(&sups));
        let mut lines_b = Lines::new(&client_b);

        // Both connections attach the same pane.
        send_request(&mut client_a, &SocketRequest::Attach { id: "ws".into() });
        let mut lines_a = Lines::new(&client_a);
        assert_eq!(lines_a.next()["code"], response_code::STREAMING);
        assert_eq!(lines_a.next()["frame"], "snapshot");
        send_request(&mut client_b, &SocketRequest::Attach { id: "ws".into() });
        assert_eq!(lines_b.next()["code"], response_code::STREAMING);
        assert_eq!(lines_b.next()["frame"], "snapshot");

        // Connection A fully closes; join its handler so the ActiveSub::drop deregister
        // (key A only) has definitely run before we feed the pane.
        drop(lines_a);
        drop(client_a);
        ha.join().unwrap();

        // The pane is alive and B is still subscribed → B receives the delta. If A's close
        // had collaterally torn down B, B would instead get an OVERFLOW error frame here.
        pane.feed(b"to-B");
        let d = lines_b.next();
        assert_eq!(d["frame"], "delta", "B still streams after A closed: {d}");
        assert_eq!(d["id"], "ws");
        assert_eq!(frame_data(&d), b"to-B");

        drop(lines_b);
        drop(client_b);
        hb.join().unwrap();
    }

    #[test]
    fn attach_racing_reader_exit_is_pane_died_not_a_zombie() {
        // The attach-vs-reader-death race: the registry is DEAD (reader thread exited) but
        // the pane's `is_alive()` still races `true` (child not yet reaped). Attach's
        // liveness gate passes, but subscribe_to observes the dead set under the subscriber
        // lock and refuses → PANE_DIED, NOT a silent never-closing zombie subscription.
        let state_root = temp_state_root_with_mutations(false);
        let sups: Arc<DaemonSups<StreamFakePane>> = Arc::new(DaemonSups::new());
        let pane = StreamFakePane::new("claude");
        pane.reader_exited_child_lingers(); // dead registry, alive() still true
        assert!(
            pane.clone().is_alive(),
            "the race precondition: liveness still reads true"
        );
        sups.insert("ws", pane);
        let (mut client, h) = spawn_stream_handler(state_root, Arc::clone(&sups));
        let mut lines = Lines::new(&client);

        send_request(&mut client, &SocketRequest::Attach { id: "ws".into() });
        let resp = lines.next();
        assert_eq!(
            resp["ok"], false,
            "must not half-upgrade onto a dead registry: {resp}"
        );
        assert_eq!(resp["code"], response_code::PANE_DIED);

        // Connection survives — a follow-up request still works.
        send_request(&mut client, &SocketRequest::ListLive);
        assert_eq!(lines.next()["ok"], true);
        drop(lines);
        drop(client);
        h.join().unwrap();
    }

    // ── malformed request → BAD_REQUEST, connection survives ──

    #[test]
    fn malformed_request_is_bad_request_and_connection_survives() {
        let state_root = temp_state_root_with_mutations(false);
        let sups: Arc<DaemonSups<FakePane>> = Arc::new(DaemonSups::new());
        let s = Arc::clone(&sups);
        let (mut client, h) = spawn_handler(state_root, move |req| route_request(&s, req));

        client.write_all(b"not json at all\n").unwrap();
        client.flush().unwrap();
        let r1 = read_response(&client);
        assert!(!r1.ok);
        assert_eq!(r1.code, response_code::BAD_REQUEST);

        // The loop continued — a valid request on the same connection still works.
        send_request(&mut client, &SocketRequest::ListLive);
        let r2 = read_response(&client);
        assert!(r2.ok);
        drop(client);
        h.join().unwrap();
    }

    // ── MF-A: a panicking handler is caught; the connection loop survives ──

    #[test]
    fn handler_panic_is_caught_and_connection_loop_survives() {
        // MF-A panic isolation: a dispatch that panics on the FIRST request must be caught
        // by catch_unwind (→ HANDLER_PANIC), and the loop must keep serving — the SECOND
        // request on the same connection gets a normal response.
        let state_root = temp_state_root_with_mutations(true);
        let seen_second = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&seen_second);
        let dispatch = move |req: SocketRequest| -> SocketResponse {
            match req {
                // First request: a handler that panics (e.g. a poisoned-lock unwrap).
                SocketRequest::Focus { .. } => panic!("boom in handler"),
                // Second request: prove the loop survived.
                _ => {
                    flag.store(true, Ordering::SeqCst);
                    SocketResponse::ok("survived")
                }
            }
        };
        let (mut client, h) = spawn_handler(state_root, dispatch);

        send_request(&mut client, &SocketRequest::Focus { id: "x".into() });
        let r1 = read_response(&client);
        assert!(!r1.ok);
        assert_eq!(
            r1.code, "HANDLER_PANIC",
            "a panic must be caught, not crash the thread"
        );

        send_request(&mut client, &SocketRequest::ListLive);
        let r2 = read_response(&client);
        assert!(
            r2.ok,
            "the connection loop must survive the caught panic: {r2:?}"
        );
        assert!(
            seen_second.load(Ordering::SeqCst),
            "the second request must have been dispatched"
        );
        drop(client);
        h.join().unwrap();
    }

    // ── Focus: daemon-local liveness check ──

    #[test]
    fn focus_validates_pane_existence() {
        let state_root = temp_state_root_with_mutations(true);
        let sups: Arc<DaemonSups<FakePane>> = Arc::new(DaemonSups::new());
        sups.insert("ws", FakePane::new("claude"));
        let s = Arc::clone(&sups);
        let (mut client, h) = spawn_handler(state_root, move |req| route_request(&s, req));

        send_request(&mut client, &SocketRequest::Focus { id: "ws".into() });
        assert!(read_response(&client).ok, "focus a live pane → ok");
        send_request(&mut client, &SocketRequest::Focus { id: "ghost".into() });
        let r = read_response(&client);
        assert_eq!(
            r.code,
            response_code::UNKNOWN_WORKSPACE,
            "focus an absent pane → UNKNOWN_WORKSPACE"
        );
        drop(client);
        h.join().unwrap();
    }

    // ── Q4 Spawn gate ordering + default-build inertness ──

    fn sample_spawn_spec() -> agent_teams_core::SpawnSpec {
        agent_teams_core::SpawnSpec {
            id: "ws-1".into(),
            harness: "claude".into(),
            repo: "/repo".into(),
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
    fn spawn_is_mutation_gated_and_unavailable_in_the_default_build() {
        // Spawn is classified mutating → with allow_mutations=false the UPSTREAM gate
        // (handle_request_line) refuses it with MUTATIONS_DISABLED BEFORE the router runs
        // — the WHO axis. This holds for ANY router (here the closure router → route_request).
        let state_root = temp_state_root_with_mutations(false);
        let sups: Arc<DaemonSups<FakePane>> = Arc::new(DaemonSups::new());
        let s = Arc::clone(&sups);
        let (mut client, h) = spawn_handler(state_root, move |req| route_request(&s, req));
        send_request(
            &mut client,
            &SocketRequest::Spawn {
                spec: sample_spawn_spec(),
            },
        );
        let r = read_response(&client);
        assert_eq!(
            r.code,
            response_code::MUTATIONS_DISABLED,
            "mutations off → refused before routing"
        );
        drop(client);
        h.join().unwrap();

        // With mutations ON but the daemon-spawn feature compiled OUT (this test binary),
        // the request clears the mutation gate and reaches route_request, which returns the
        // SPAWN_UNAVAILABLE fallback so the GUI falls back to local spawn. Nothing spawns.
        let state_root2 = temp_state_root_with_mutations(true);
        let sups2: Arc<DaemonSups<FakePane>> = Arc::new(DaemonSups::new());
        let s2 = Arc::clone(&sups2);
        let (mut client2, h2) = spawn_handler(state_root2, move |req| route_request(&s2, req));
        send_request(
            &mut client2,
            &SocketRequest::Spawn {
                spec: sample_spawn_spec(),
            },
        );
        let r2 = read_response(&client2);
        #[cfg(not(feature = "daemon-spawn"))]
        assert_eq!(
            r2.code,
            response_code::SPAWN_UNAVAILABLE,
            "feature off → SPAWN_UNAVAILABLE fallback"
        );
        assert_eq!(
            sups2.count_live(),
            0,
            "no pane ever spawned through the closure-router fallback"
        );
        let _ = r2;
        drop(client2);
        h2.join().unwrap();
    }
}
