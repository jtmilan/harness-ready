//! Daemon-side request handlers (Phase 08 Sub-build 2 / 08-T4).
//!
//! Pure functions over a [`DaemonSups`] that mirror the GUI's existing gated
//! pane-resolution sites: write (the D30 `is_alive` gate), read (a bounded recent
//! snapshot), resize, and list. They are NOT wired to a socket server here — the
//! daemon's socket server is Sub-build 3 (08-T5/T6), which will call these from its
//! connection loop. Keeping them pure lets the unit tests below exercise the exact
//! gate/branch logic with no socket and no PTY.
//!
//! ## The read substrate: `ByteRing::recent`
//!
//! [`handle_read_output`] returns "recent scrollback" via [`Supervisor::snapshot`],
//! which is the bounded whole-window read (equivalent to [`ringbuf::ByteRing::recent`]
//! — a tail-bounded copy, NOT the delta cursor). The GUI's `read_output_delta`
//! byte-cursor protocol (incomplete-trailing-codepoint held back, truncation signaled)
//! stays on the GUI's `output_handle()` + `PaneBuffer::delta(since)` path — that
//! cursor surface is preserved untouched (08-T4 invariant); the daemon read here is
//! the simple whole-window snapshot a future cold re-attach repaints from.

use crate::sups::DaemonSups;
use agent_teams_core::{
    harness_needs_split_submit, normalize_input, response_code, split_settle_ms, SocketResponse,
};

/// The minimal capability the gated split-write path needs from a daemon-owned pane
/// value. Implemented for the real [`supervisor::Supervisor`] (production) and for a
/// cheap test double (so the two-phase ordering is unit-testable without a live PTY —
/// a real `Supervisor` owns a spawned child and cannot be constructed in a test). The
/// `harness_wire` is the stable WIRE STRING (`descriptor().wire`) so the split-submit /
/// settle decision routes through the shared `agent_teams_core` helpers (MF-E).
pub trait PaneWrite {
    /// D30 liveness — `false` ⇒ the PTY is a corpse and the write must be rejected.
    fn is_alive(&mut self) -> bool;
    /// Write raw bytes to the PTY master.
    fn write(&mut self, data: &[u8]) -> std::io::Result<()>;
    /// The harness's stable wire id (`"claude"` / `"bash"` / …).
    fn harness_wire(&self) -> &'static str;
}

impl PaneWrite for supervisor::Supervisor {
    fn is_alive(&mut self) -> bool {
        supervisor::Supervisor::is_alive(self)
    }
    fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        supervisor::Supervisor::write(self, data)
    }
    fn harness_wire(&self) -> &'static str {
        self.harness.descriptor().wire
    }
}

/// The streaming capability the daemon's `Attach` (08 Sub-build 3 / slice 3) needs from
/// a daemon-owned pane: O(1) clones of its (buffer, subscriber-registry) handles so the
/// snapshot+register runs OFF the map lock (the lock-shedding `output_handle()` pattern).
/// Implemented for the real [`supervisor::Supervisor`] (production) and a streaming test
/// double. Kept SEPARATE from [`PaneWrite`] so the slice-1/2 write handlers stay generic
/// over `V: PaneWrite` alone; only the streaming path requires `V: PaneWrite + PaneStream`.
pub trait PaneStream {
    /// O(1) clones of the pane's output buffer + subscriber registry. The caller drops
    /// the map lock, then `supervisor::subscribers::subscribe_to(&buf, &subs, ..)`
    /// atomically snapshots+registers under the buffer→subscriber lock (NEVER the map
    /// lock — the reader fan-out depends on that invariant).
    #[allow(clippy::type_complexity)]
    fn stream_handles(
        &self,
    ) -> (
        std::sync::Arc<std::sync::Mutex<supervisor::PaneBuffer>>,
        supervisor::subscribers::SubscriberHandle,
    );
}

impl PaneStream for supervisor::Supervisor {
    fn stream_handles(
        &self,
    ) -> (
        std::sync::Arc<std::sync::Mutex<supervisor::PaneBuffer>>,
        supervisor::subscribers::SubscriberHandle,
    ) {
        supervisor::Supervisor::stream_handles(self)
    }
}

/// The daemon-side `SendInput` entry (08 Sub-build 3 / MF-E) and the ONLY public write
/// path: NORMALIZE the text first (reject interior newline / control bytes — so a
/// same-user socket peer can neither inject a second TUI line nor drive the TUI via
/// ESC/Ctrl-C/tab) and ONLY then two-phase split-write it. The raw [`handle_split_write`]
/// is `pub(crate)`, so it is UNREACHABLE from outside this crate without the payload
/// defense in front of it — "unreachable without normalize" is enforced by VISIBILITY,
/// not just prose. A non-normalizable payload → `BAD_REQUEST`, never written. This is the
/// composition the slice-2/3 accept loop wires `SendInput` to (it is NOT a socket server,
/// just the normalize→split wiring). The old raw `handle_write_to_pane` (no normalize, no
/// split) is DELETED — it was zero-caller dead code and a second reachable raw path.
///
/// SUB-BUILD 3 TRANSFER OBLIGATION — claude banner-prime (NOT done here):
/// the GUI's `write_to_pane` (`app/src-tauri/src/lib.rs:1340-1364`) sends ONE Esc before
/// the FIRST write to a claude pane to dismiss claude Code v2.1.181's "N setup issues:
/// MCP" startup banner that INTERCEPTS Enter — without it the first dispatched line
/// buffers and never submits (re-sends only glue more text onto the unsent line). This
/// path mirrors only the split-SUBMIT half of the GUI shape, NOT the one-time prime. In
/// the role-inversion end state the daemon OWNS the claude PTY, so the accept loop
/// (08-T5/T6) that drives the FIRST `SendInput` to a freshly-spawned claude pane MUST
/// replicate the prime (one-time Esc + 250ms settle, keyed by a per-pane "primed" set)
/// BEFORE this call, or the first daemon submit silently fails. Tracked in the §9 build
/// checklist of `08-04-SUBBUILD3-HARDENED-DESIGN.md`.
pub fn handle_send_input<V: PaneWrite>(
    sups: &DaemonSups<V>,
    id: &str,
    text: &str,
) -> SocketResponse {
    match normalize_input(text) {
        Ok(normalized) => handle_split_write(sups, id, normalized.as_bytes()),
        Err(e) => SocketResponse::err(response_code::BAD_REQUEST, e),
    }
}

/// Handle a write through the gated split-submit two-phase path (08 Sub-build 3 /
/// MF-E). `pub(crate)` — NOT a public entry: the only way in is [`handle_send_input`],
/// which has ALREADY passed the bytes through [`normalize_input`]. This function owns the
/// D30 gate, the per-pane write serialization, and the split-submit two PTY writes.
///
/// PER-PANE SERIALIZATION (MF-A): the daemon runs per-connection threads, so two
/// same-user socket peers can issue concurrent `SendInput` to the SAME pane. The body and
/// the deferred lone `\r` are SEPARATE PTY writes with an UNLOCKED settle between them;
/// without serialization the two writers' bodies/`\r`s can interleave and splice (A-body,
/// B-body, A-`\r` submitting B's half-written line). A per-pane write lock
/// (`DaemonSups::write_lock`) is held across BOTH phases so body+`\r` submit as an ATOMIC
/// unit per pane — while the global `DaemonSups` map lock is STILL never held across the
/// settle sleep (only the per-pane `()` lock is).
///
/// Mirrors the split-SUBMIT half of the GUI `write_to_pane` shape
/// (`app/src-tauri/src/lib.rs:1365-1419`); the GUI's one-time claude banner-prime is a
/// transfer obligation documented on [`handle_send_input`], not replicated here.
///  - Phase 1 under ONE `with_mut` (map) lock: D30 `is_alive` gate; for a
///    `harness_needs_split_submit` harness whose data ends in `\r`, write the BODY and
///    record `(wire, body_len)` to defer the lone `\r`; otherwise write the data in one
///    shot (bash, or a payload with no trailing `\r`).
///  - The map lock is RELEASED before the settle `sleep` (never held across the sleep).
///  - Phase 2 (only when deferred): sleep `split_settle_ms(wire, body_len)` OUTSIDE the
///    map lock so the harness's paste/Ink coalescer flushes the body, then re-acquire and
///    write the lone `\r` as its OWN PTY read event (else it folds into the composer and
///    the task never submits). A pane that vanished during the settle is a no-op.
///
/// - `Some(Ok)`  → `OK` (`"written"`)
/// - `Some(Err)` → `DEAD_PANE`
/// - `None`      → `UNKNOWN_WORKSPACE`
pub(crate) fn handle_split_write<V: PaneWrite>(
    sups: &DaemonSups<V>,
    id: &str,
    data: &[u8],
) -> SocketResponse {
    // MEMORY BOUND (MF-A): mint the per-pane write lock ONLY for a KNOWN pane. The slice-2
    // socket router (`route_request`) feeds ARBITRARY peer-supplied ids straight in; without
    // this gate, `write_lock` would lazily insert a permanent `write_locks` entry for every
    // distinct id BEFORE the `with_mut` existence check, so a same-user peer (mutations ON)
    // streaming distinct nonexistent ids would grow the long-lived daemon's memory unbounded.
    // A small TOCTOU (pane removed between this check and the lock) only yields a harmless
    // transient entry keyed by a real id — bounded by distinct REAL panes, never by attacker
    // input.
    if !sups.contains(id) {
        return SocketResponse::err(response_code::UNKNOWN_WORKSPACE, "no such workspace");
    }
    // Per-pane write lock spanning BOTH phases (MF-A): serialize concurrent SendInput to
    // the SAME pane so two socket peers' body/\r writes can never interleave. The map lock
    // is still taken only transiently inside each phase, never across the settle sleep.
    let pane_lock = sups.write_lock(id);
    let _wguard = pane_lock.lock().unwrap_or_else(|e| e.into_inner());
    // Phase 1: D30 gate + body write under ONE map lock; yields whether to defer the \r.
    let defer = sups.with_mut(id, |sup| -> Result<Option<(&'static str, usize)>, String> {
        // D30: reject writes to a dead PTY instead of silently swallowing them.
        if !sup.is_alive() {
            return Err("workspace is no longer alive".to_string());
        }
        match data.strip_suffix(b"\r") {
            // split-submit harness with a trailing \r AND a non-empty body → write the body
            // now, defer the \r. A bare `\r` (empty body) has nothing to coalesce, so it falls
            // to the fast `_` arm: a single direct write, no settle sleep (mirrors the GUI
            // `write_to_pane` fast path — avoids a pointless per-Enter stall).
            Some(body) if !body.is_empty() && harness_needs_split_submit(sup.harness_wire()) => {
                sup.write(body).map_err(|e| e.to_string())?;
                Ok(Some((sup.harness_wire(), body.len())))
            }
            // bash, no trailing \r, or a bare \r → single write, no defer.
            _ => {
                sup.write(data).map_err(|e| e.to_string())?;
                Ok(None)
            }
        }
    });
    // `with_mut` released the lock on return — the settle sleep below stays OUTSIDE it.
    let defer = match defer {
        None => return SocketResponse::err(response_code::UNKNOWN_WORKSPACE, "no such workspace"),
        Some(Err(e)) => return SocketResponse::err(response_code::DEAD_PANE, e),
        Some(Ok(d)) => d,
    };
    if let Some((wire, body_len)) = defer {
        // Per-harness, payload-scaled settle before the lone submit \r (OUTSIDE the lock).
        std::thread::sleep(std::time::Duration::from_millis(split_settle_ms(
            wire, body_len,
        )));
        // Absent id here is a no-op (the pane vanished during the settle); only a
        // live-pane write error propagates as DEAD_PANE.
        let outcome = sups
            .with_mut(id, |sup| {
                if sup.is_alive() {
                    sup.write(b"\r").map_err(|e| e.to_string())
                } else {
                    Ok(())
                }
            })
            .unwrap_or(Ok(()));
        if let Err(e) = outcome {
            return SocketResponse::err(response_code::DEAD_PANE, e);
        }
    }
    SocketResponse::ok("written")
}

/// Q4 claude banner-prime (must-fix C7) — the daemon-side replica of the GUI's
/// `write_to_pane` one-time Esc + settle. Because the Q4 daemon OWNS the claude PTY, the
/// FIRST `SendInput` to a freshly-spawned claude pane must dismiss claude Code's "N setup
/// issues: MCP" startup banner (which INTERCEPTS Enter) — else the first dispatched line
/// buffers and never submits. Keyed by a per-pane `primed` set (reset on (re)spawn).
///
/// C7 CONSTRAINT: the prime is a HARDCODED Esc (the ONLY internal byte) written through
/// the one normalize-bypassing path, per-pane, and NEVER carries peer bytes — so MF-E's
/// "only literal text reaches the PTY" holds for everything except this single fixed
/// internal prime. A non-claude pane needs no prime (marked primed once, then skipped).
/// Compiled only under `cfg(any(test, feature = "daemon-spawn"))` (the default build never
/// owns a claude PTY, so it never primes).
#[cfg(any(test, feature = "daemon-spawn"))]
pub fn maybe_prime_claude<V: PaneWrite>(
    sups: &DaemonSups<V>,
    primed: &std::sync::Mutex<std::collections::HashSet<String>>,
    id: &str,
) {
    // Check-and-set under the primed lock ONLY (released before any map lock — no nesting):
    // claim the prime exactly once per pane. A racing second writer sees it already claimed.
    let claimed = {
        let mut g = primed.lock().unwrap_or_else(|e| e.into_inner());
        if g.contains(id) {
            false
        } else {
            g.insert(id.to_string());
            true
        }
    };
    if !claimed {
        return;
    }
    // Only a claude pane needs the banner-prime. (A non-claude pane stays "claimed" — a
    // harmless marker; it just means we never reconsider it.)
    let is_claude = sups
        .with_mut(id, |s| s.harness_wire() == "claude")
        .unwrap_or(false);
    if !is_claude {
        return;
    }
    // The single hardcoded Esc, then a settle so claude dismisses the banner BEFORE the
    // first real submit arrives. Never peer bytes.
    let _ = sups.with_mut(id, |s| s.write(b"\x1b"));
    std::thread::sleep(std::time::Duration::from_millis(250));
}

/// Handle a read-output request: return the pane's recent bounded scrollback
/// (the `ByteRing::recent`-style whole-window read).
///
/// LOCK SHEDDING (the load-bearing invariant): the `DaemonSups` map lock is
/// released BEFORE this pane's output buffer is locked — exactly the
/// `output_handle()`-clone-then-read pattern the GUI's `read_output_delta` uses.
/// `with_snapshot` clones the `Arc<Mutex<PaneBuffer>>` handle (O(1), no buffer
/// lock) and the map guard drops on return; only THEN is the per-pane buffer
/// locked for the bounded-window copy. This prevents a write (which takes the
/// map lock via `with_mut`) from queuing behind a snapshot read — and it never
/// nests the two locks (map → buffer), so no lock-ordering hazard exists for a
/// future Sub-build 3 reader-thread callback that holds the buffer lock first.
pub fn handle_read_output(sups: &DaemonSups, id: &str) -> SocketResponse {
    let Some(handle) = sups.with_snapshot(id, |sup| sup.output_handle()) else {
        return SocketResponse::err(response_code::UNKNOWN_WORKSPACE, "no such workspace");
    };
    // The map lock is released here — only this pane's buffer is locked below.
    let output = handle
        .lock()
        .map(|o| String::from_utf8_lossy(&o.recent_ring().recent()).to_string())
        .unwrap_or_default();
    SocketResponse::ok(output)
}

/// Handle a resize-pty request (rows/cols → SIGWINCH). `resize` takes `&self`, so an
/// immutable snapshot borrow suffices.
pub fn handle_resize_pty(sups: &DaemonSups, id: &str, rows: u16, cols: u16) -> SocketResponse {
    match sups.with_snapshot(id, |sup| sup.resize(rows, cols)) {
        Some(()) => SocketResponse::ok("resized"),
        None => SocketResponse::err(response_code::UNKNOWN_WORKSPACE, "no such workspace"),
    }
}

/// Handle a list-workspaces request: the live pane ids as a JSON `{ "ids": [...] }`
/// body on the response detail.
pub fn handle_list_workspaces(sups: &DaemonSups) -> SocketResponse {
    let ids = sups.live_ids();
    let body = serde_json::json!({ "ids": ids }).to_string();
    SocketResponse::ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_teams_core::response_code;

    /// A cheap pane stand-in for the split-write tests. A real `Supervisor` owns a
    /// spawned PTY child and cannot be constructed in a unit test, so this records each
    /// PTY write in ORDER (proving the two-phase body-then-`\r` ordering) and exposes a
    /// harness wire string + a liveness flag — the exact surface [`PaneWrite`] needs.
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

    #[test]
    fn split_write_for_split_harness_does_two_writes_body_then_cr() {
        // A split-submit harness (codex) with a trailing \r → body and the lone \r must
        // be SEPARATE PTY writes, in that order (the load-bearing MF-E split-submit ACs).
        let sups: DaemonSups<FakePane> = DaemonSups::new();
        sups.insert("ws", FakePane::new("codex"));
        let r = handle_split_write(&sups, "ws", b"do the thing\r");
        assert!(r.ok, "live split-write must succeed");
        let writes = sups.with_snapshot("ws", |p| p.writes.clone()).unwrap();
        assert_eq!(
            writes,
            vec![b"do the thing".to_vec(), b"\r".to_vec()],
            "split harness: body first, then the lone \\r as its own write"
        );
    }

    #[test]
    fn split_write_for_bash_does_single_write() {
        // bash never coalesces a glued \r → it submits on the SINGLE concat write.
        let sups: DaemonSups<FakePane> = DaemonSups::new();
        sups.insert("sh", FakePane::new("bash"));
        let r = handle_split_write(&sups, "sh", b"ls\r");
        assert!(r.ok);
        let writes = sups.with_snapshot("sh", |p| p.writes.clone()).unwrap();
        assert_eq!(
            writes,
            vec![b"ls\r".to_vec()],
            "bash: one shot, no deferred \\r"
        );
    }

    #[test]
    fn split_write_to_absent_pane_is_unknown_workspace() {
        let sups: DaemonSups<FakePane> = DaemonSups::new();
        let r = handle_split_write(&sups, "ghost", b"hi\r");
        assert!(!r.ok);
        assert_eq!(r.code, response_code::UNKNOWN_WORKSPACE);
    }

    #[test]
    fn unknown_pane_send_input_mints_no_write_lock() {
        // MF-A memory bound: a SendInput against an arbitrary nonexistent id must return
        // UNKNOWN_WORKSPACE WITHOUT leaving a permanent write_locks entry — else a peer
        // streaming distinct unknown ids (the socket router feeds them straight in) grows
        // the long-lived daemon's memory unbounded.
        let sups: DaemonSups<FakePane> = DaemonSups::new();
        for id in ["ghost-1", "ghost-2", "ghost-3"] {
            let r = handle_send_input(&sups, id, "x");
            assert!(!r.ok);
            assert_eq!(r.code, response_code::UNKNOWN_WORKSPACE);
        }
        assert_eq!(
            sups.write_lock_count(),
            0,
            "unknown ids must never mint a write lock (unbounded-growth defense)"
        );
        // A KNOWN pane DOES mint exactly one stable lock (the serialization seam still works).
        sups.insert("ws", FakePane::new("claude"));
        assert!(handle_send_input(&sups, "ws", "approve").ok);
        assert_eq!(
            sups.write_lock_count(),
            1,
            "a known pane mints exactly one write lock"
        );
    }

    #[test]
    fn split_write_to_dead_pane_is_dead_pane() {
        // D30 gate: a write to a dead PTY is rejected (never silently swallowed), and
        // nothing is written to the corpse.
        let sups: DaemonSups<FakePane> = DaemonSups::new();
        sups.insert(
            "ws",
            FakePane {
                harness: "codex",
                alive: false,
                writes: Vec::new(),
            },
        );
        let r = handle_split_write(&sups, "ws", b"do the thing\r");
        assert!(!r.ok);
        assert_eq!(r.code, response_code::DEAD_PANE);
        let writes = sups.with_snapshot("ws", |p| p.writes.clone()).unwrap();
        assert!(writes.is_empty(), "dead pane must receive no bytes");
    }

    #[test]
    fn send_input_normalizes_before_split_write() {
        // The composed entry: a single line is normalized (+\r appended) then split-written.
        let sups: DaemonSups<FakePane> = DaemonSups::new();
        sups.insert("ws", FakePane::new("claude"));
        let r = handle_send_input(&sups, "ws", "approve");
        assert!(r.ok);
        let writes = sups.with_snapshot("ws", |p| p.writes.clone()).unwrap();
        // claude splits → body "approve" then the lone \r normalize appended.
        assert_eq!(writes, vec![b"approve".to_vec(), b"\r".to_vec()]);
    }

    #[test]
    fn send_input_rejects_interior_newline_without_writing() {
        // MF-E: a multi-line payload is rejected at normalize — never reaches the PTY,
        // so a socket peer cannot sneak a second TUI submission ("yes\nyes").
        let sups: DaemonSups<FakePane> = DaemonSups::new();
        sups.insert("ws", FakePane::new("claude"));
        let r = handle_send_input(&sups, "ws", "yes\nyes");
        assert!(!r.ok);
        assert_eq!(r.code, response_code::BAD_REQUEST);
        let writes = sups.with_snapshot("ws", |p| p.writes.clone()).unwrap();
        assert!(
            writes.is_empty(),
            "a rejected payload must not write any bytes"
        );
    }

    #[test]
    fn send_input_rejects_control_byte_without_writing() {
        // MF-E: a control byte (ESC) is rejected — can't drive the TUI history/signals.
        let sups: DaemonSups<FakePane> = DaemonSups::new();
        sups.insert("ws", FakePane::new("codex"));
        let r = handle_send_input(&sups, "ws", "\x1b[A");
        assert!(!r.ok);
        assert_eq!(r.code, response_code::BAD_REQUEST);
        assert!(sups
            .with_snapshot("ws", |p| p.writes.clone())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn concurrent_send_input_to_same_pane_never_splices_body_and_cr() {
        // MF-A per-pane serialization: two concurrent SendInput peers to the SAME pane
        // must not interleave. The split-submit write is body → (UNLOCKED settle) → \r;
        // the per-pane write lock holds across BOTH phases so the recorded PTY writes are
        // two CONTIGUOUS (body, \r) pairs — never the spliced [bodyA, bodyB, \rA, \rB]
        // that a missing lock would produce (B grabs the map lock during A's settle).
        use std::sync::Arc;
        use std::thread;
        let sups: Arc<DaemonSups<FakePane>> = Arc::new(DaemonSups::new());
        sups.insert("ws", FakePane::new("codex"));
        let a = {
            let s = Arc::clone(&sups);
            thread::spawn(move || handle_send_input(&*s, "ws", "aaa"))
        };
        let b = {
            let s = Arc::clone(&sups);
            thread::spawn(move || handle_send_input(&*s, "ws", "bbb"))
        };
        assert!(a.join().unwrap().ok, "writer A must succeed");
        assert!(b.join().unwrap().ok, "writer B must succeed");
        let writes = sups.with_snapshot("ws", |p| p.writes.clone()).unwrap();
        assert_eq!(
            writes.len(),
            4,
            "two split-submit writes = 4 PTY writes, got {writes:?}"
        );
        let valid = writes
            == vec![
                b"aaa".to_vec(),
                b"\r".to_vec(),
                b"bbb".to_vec(),
                b"\r".to_vec(),
            ]
            || writes
                == vec![
                    b"bbb".to_vec(),
                    b"\r".to_vec(),
                    b"aaa".to_vec(),
                    b"\r".to_vec(),
                ];
        assert!(
            valid,
            "body+\\r must submit atomically per pane (no splice), got {writes:?}"
        );
    }

    // The handlers are GENERIC over the stored value only through DaemonSups<V>; a real
    // Supervisor can't be constructed in a unit test (it owns a spawned PTY). The
    // UNKNOWN_WORKSPACE branch is driven with an empty DaemonSups. The split-write
    // Some(Ok)/Some(Err) arms (the D30 dead-pane gate's DEAD_PANE response in particular)
    // are covered WITHOUT a live PTY via the `FakePane` PaneWrite double in the
    // split_write_* tests above. The read/resize Some(...) arms still require a live PTY
    // and are covered by the supervisor crate's own PTY tests + the operator GUI-verify (AC-1).

    #[test]
    fn maybe_prime_claude_sends_one_esc_to_claude_once_and_never_to_bash() {
        // C7: a claude pane gets exactly ONE hardcoded Esc on the first call, nothing on
        // the second (per-pane primed set); a bash pane never gets an Esc.
        use std::collections::HashSet;
        use std::sync::Mutex;
        let sups: DaemonSups<FakePane> = DaemonSups::new();
        sups.insert("c", FakePane::new("claude"));
        sups.insert("sh", FakePane::new("bash"));
        let primed = Mutex::new(HashSet::new());

        maybe_prime_claude(&sups, &primed, "c");
        assert_eq!(
            sups.with_snapshot("c", |p| p.writes.clone()).unwrap(),
            vec![b"\x1b".to_vec()],
            "claude pane primed with exactly one Esc"
        );
        // second call → no second Esc (already primed).
        maybe_prime_claude(&sups, &primed, "c");
        assert_eq!(
            sups.with_snapshot("c", |p| p.writes.clone()).unwrap().len(),
            1,
            "no re-prime on a primed pane"
        );
        // bash pane → never primed (no Esc).
        maybe_prime_claude(&sups, &primed, "sh");
        assert!(
            sups.with_snapshot("sh", |p| p.writes.clone())
                .unwrap()
                .is_empty(),
            "bash never gets a banner-prime"
        );
    }

    #[test]
    fn read_absent_pane_is_unknown_workspace() {
        let sups = DaemonSups::new();
        let r = handle_read_output(&sups, "ghost");
        assert!(!r.ok);
        assert_eq!(r.code, response_code::UNKNOWN_WORKSPACE);
    }

    #[test]
    fn resize_absent_pane_is_unknown_workspace() {
        let sups = DaemonSups::new();
        let r = handle_resize_pty(&sups, "ghost", 40, 120);
        assert!(!r.ok);
        assert_eq!(r.code, response_code::UNKNOWN_WORKSPACE);
    }

    #[test]
    fn list_empty_is_ok_with_empty_ids() {
        let sups = DaemonSups::new();
        let r = handle_list_workspaces(&sups);
        assert!(r.ok);
        let v: serde_json::Value = serde_json::from_str(&r.detail).unwrap();
        assert_eq!(v["ids"].as_array().unwrap().len(), 0);
    }
}
