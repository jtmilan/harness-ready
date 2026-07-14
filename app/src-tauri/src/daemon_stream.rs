//! Q4 attach-streaming: the app-side CLIENT of a daemon-owned pane's `Attach` stream
//! (Phase 08 Sub-build 3 / Q4 Stage-4, `08-05-Q4-PTY-OWNERSHIP-DESIGN.md`).
//!
//! ## Role
//!
//! A daemon-owned pane (one tracked in `AppState.daemon_panes`, spawned by the
//! `daemon_spawn`-ON routing path) renders its output via this module. The daemon owns the
//! PTY master fd; the app holds NO buffer onto it. To render such a pane the app opens a
//! LONG-LIVED connection to the daemon's socket, dials `Attach{id}`, reads the one-shot
//! `snapshot` frame (current scrollback) then the live `delta` frames, base64-decodes each,
//! and APPENDS them into a per-id [`PaneBuffer`]. `read_output_delta` then serves a cursor
//! delta from THAT buffer exactly as it does for an app-resident pane — so the frontend
//! poller renders a daemon pane with zero frontend change.
//!
//! This is NOT the one-shot [`crate::daemon_client::daemon_spawn`] dial (round-trip one
//! line, drop the connection). It is a PERSISTENT reader thread per attached daemon pane.
//!
//! ## Lock discipline (slice-3 rule: never hold the buffer lock across the map lock or a
//! blocking socket read)
//!
//! The reader thread holds ONLY its own [`Arc<Mutex<StreamBuf>>`] clone. It base64-decodes a
//! frame FIRST (no lock), then takes a short `lock → push/reset → drop` and goes back to the
//! socket read with the lock released. `read_output_delta` clones the per-id buffer handle
//! UNDER the `daemon_streams` map lock, DROPS the map lock, then locks the buffer alone for
//! the µs delta copy. No path holds the buffer lock across the map lock or a socket read.
//!
//! ## Re-attach is a RESET, not an append (no dup, no loss, monotonic cursor)
//!
//! On a connection drop the reader reconnects and re-`Attach`es; the daemon answers with a
//! FRESH `snapshot` (a full re-capture of the retained window). The app buffer is RESET to
//! that snapshot and `gen_base` (the app-absolute offset of the new generation's byte 0) is
//! advanced strictly past every byte the frontend already consumed — so the very next
//! `read_output_delta` reports `truncated` and the frontend self-heals (RIS reset + replay,
//! `main.js applyPaneDelta`). The on-wire daemon `prev_total/new_total` are used ONLY as a
//! contiguity check; the app's own monotonic `gen_base + buffer offset` is the cursor SSOT.
//!
//! ## Default-OFF inertness
//!
//! With `daemon_spawn` OFF there are NO daemon-owned panes, so [`start`] is never called, NO
//! reader thread exists, and `read_output_delta` for app-resident ids never enters the daemon
//! branch — byte-identical to today. Every symbol here is reachable ONLY for an id in
//! `daemon_panes`, which only fills when the routing flag is ON.

#![allow(dead_code)] // reachable only on the `daemon_spawn`-ON routing path.

use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use agent_teams_core::{response_code, socket_path, SocketRequest, SocketResponse};
use agent_teams_daemon::frames::{b64_decode, code, StreamFrame};
use supervisor::{PaneBuffer, RETAIN_CAP};

/// How long a reader `read` blocks before returning so the loop can re-check the stop flag
/// (also the keepalive cadence the daemon emits every 30s sits comfortably inside this).
const READ_POLL: Duration = Duration::from_millis(200);
/// Write bound for the `Attach`/`Detach` request lines (a wedged daemon never blocks forever).
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
/// Reconnect backoff bounds (a dropped connection is retried, never spun on).
const BACKOFF_START: Duration = Duration::from_millis(200);
const BACKOFF_MAX: Duration = Duration::from_secs(5);
/// A single frame line past this is pathological — drop the connection (a ~4 MiB snapshot
/// base64-inflates to ~5.5 MiB, so this is comfortably above the legitimate maximum).
const MAX_LINE_BYTES: usize = 64 * 1024 * 1024;

/// Reader attach status (for tests + DEFERRED frontend death/disconnect surfacing).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachState {
    /// `Attach` acked `STREAMING`; reading snapshot/delta frames.
    Attached,
    /// Connection dropped / OVERFLOW / contiguity gap → reader is retrying with backoff
    /// (the pane is RE-ATTACHABLE; the next reconnect's snapshot re-syncs).
    Disconnected,
    /// The daemon reported the pane died (`PANE_DIED`) → reader stopped, no reconnect.
    Dead,
}

/// The per-id streamed output substrate: the bounded byte buffer (snapshot + accumulated
/// deltas) plus the cursor/contiguity bookkeeping. Guarded by ONE mutex so the reader's
/// frame-apply and `read_output_delta`'s cursor read never need two locks.
pub struct StreamBuf {
    /// The CURRENT attach generation's bytes. `PaneBuffer::new` resets `base` to 0 each
    /// generation; the app-absolute offset is recovered by adding `gen_base`.
    buf: PaneBuffer,
    /// App-absolute offset of `buf`'s byte 0 for this generation. 0 for the first attach;
    /// advanced strictly past the prior generation's end on every re-attach so a stale
    /// frontend cursor always reports `truncated` (forces exactly one full replay).
    gen_base: u64,
    /// The daemon-side total at the last applied frame (snapshot `baseline` or delta
    /// `new_total`) — a CONTIGUITY reference only (a delta whose `prev_total` mismatches is a
    /// gap → reconnect). Never the app cursor base. `None` until the first snapshot.
    last_total: Option<u64>,
    /// Whether any snapshot has ever been applied (distinguishes the first attach — which
    /// starts at `gen_base == 0` like a fresh app-resident `PaneBuffer` — from a re-attach).
    attached_once: bool,
}

/// What applying one frame did (drives the reader loop's reconnect/stop decision).
#[derive(Debug, PartialEq, Eq)]
pub enum FrameStep {
    /// Frame applied (or a keepalive ignored) — keep streaming.
    Ok,
    /// The daemon reported the pane died → stop the reader (no reconnect).
    PaneDied,
    /// Decode failure / contiguity gap / overflow → reconnect (a fresh snapshot re-syncs).
    Reconnect,
}

impl StreamBuf {
    fn new() -> Self {
        StreamBuf {
            buf: PaneBuffer::new(RETAIN_CAP),
            gen_base: 0,
            last_total: None,
            attached_once: false,
        }
    }

    /// Apply one decoded [`StreamFrame`] to the buffer. Pure (no I/O) so the snapshot/delta/
    /// reset/gap logic is unit-tested without a socket.
    pub fn apply(&mut self, frame: &StreamFrame) -> FrameStep {
        match frame {
            StreamFrame::Snapshot { baseline, data, .. } => {
                let Some(bytes) = b64_decode(data) else { return FrameStep::Reconnect };
                if self.attached_once {
                    // Re-attach: advance past everything the frontend could have consumed in
                    // the prior generation (+1 closes the fully-caught-up case) so the next
                    // read reports `truncated` → frontend does a full RIS-reset + replay.
                    self.gen_base = self.gen_base + self.buf.end() + 1;
                } else {
                    // First attach: app-local offsets start at 0, exactly like a fresh
                    // app-resident PaneBuffer (the first non-empty delta is a clean append).
                    self.gen_base = 0;
                }
                self.buf = PaneBuffer::new(RETAIN_CAP);
                self.buf.push(&bytes);
                self.last_total = Some(*baseline);
                self.attached_once = true;
                FrameStep::Ok
            }
            StreamFrame::Delta { prev_total, new_total, data, .. } => {
                // Contiguity: a delta whose prev_total != our last applied total has a gap →
                // reconnect (the fresh snapshot resyncs). The first delta after a snapshot has
                // prev_total == baseline == last_total.
                if let Some(t) = self.last_total {
                    if *prev_total != t {
                        return FrameStep::Reconnect;
                    }
                }
                let Some(bytes) = b64_decode(data) else { return FrameStep::Reconnect };
                self.buf.push(&bytes);
                self.last_total = Some(*new_total);
                FrameStep::Ok
            }
            StreamFrame::Error { code: c, .. } => {
                if c == code::PANE_DIED {
                    FrameStep::PaneDied
                } else {
                    // OVERFLOW (slow-subscriber drop) or any unknown error → reconnect for a
                    // fresh snapshot.
                    FrameStep::Reconnect
                }
            }
            StreamFrame::Keepalive => FrameStep::Ok,
        }
    }

    /// App-absolute cursor delta for `since`, mirroring [`PaneBuffer::delta`] but across
    /// re-attach generations. Returns `(start, bytes)` where `start` is the app-absolute
    /// offset of `bytes[0]`; the caller feeds it to `delta_payload` (which flags
    /// `truncated` whenever `start != since`).
    pub fn delta(&self, since: u64) -> (u64, Vec<u8>) {
        if since < self.gen_base {
            // The cursor predates this generation (a re-attach advanced gen_base past it) →
            // serve the whole current window; `start (gen_base) != since` flags truncation.
            return (self.gen_base, self.buf.retained().to_vec());
        }
        let (bstart, bytes) = self.buf.delta(since - self.gen_base);
        (self.gen_base + bstart, bytes)
    }

    /// Test/diagnostic: the whole current-generation window as bytes.
    #[cfg(test)]
    fn window(&self) -> Vec<u8> {
        self.buf.retained().to_vec()
    }
}

/// One attached daemon pane: the shared streamed buffer (reader writes, `read_output_delta`
/// reads), the reader's stop flag + status, and its join handle. Dropping it STOPS the reader
/// (sets the flag; the reader wakes within `READ_POLL`, sends a best-effort `Detach`, exits)
/// WITHOUT joining — Drop must never block a command thread.
pub struct DaemonStream {
    inner: Arc<Mutex<StreamBuf>>,
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<AttachState>>,
    join: Option<JoinHandle<()>>,
}

impl DaemonStream {
    /// Cheap clone of the per-id buffer handle so `read_output_delta` reads OUTSIDE the
    /// `daemon_streams` map lock (slice-3 lock-shedding, like `Supervisor::output_handle`).
    pub fn handle(&self) -> Arc<Mutex<StreamBuf>> {
        self.inner.clone()
    }

    /// Current attach status (for tests + DEFERRED UI surfacing).
    pub fn status(&self) -> AttachState {
        *self.status.lock().unwrap()
    }

    /// Signal the reader to stop (idempotent). Drop does this too.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    /// Test-only: stop and JOIN the reader, asserting it terminates (no thread leak).
    #[cfg(test)]
    fn stop_and_join(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            j.join().expect("reader thread joined cleanly");
        }
    }
}

impl Drop for DaemonStream {
    fn drop(&mut self) {
        // Stop the reader; do NOT join (Drop must not block — the reader wakes within
        // READ_POLL, best-effort Detaches, and exits → no leak, no blocking).
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Start the per-id Attach reader for a daemon-owned pane and return its handle. The reader
/// runs on a dedicated `std` thread (all app socket I/O is blocking `std`); it opens a
/// long-lived connection, dials `Attach{id}`, and streams snapshot+delta frames into the
/// per-id buffer, reconnecting on drop. Inert unless called (only the `daemon_spawn`-ON
/// routing path calls it).
pub fn start(state_root: PathBuf, id: String) -> DaemonStream {
    let inner = Arc::new(Mutex::new(StreamBuf::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let status = Arc::new(Mutex::new(AttachState::Disconnected));
    let (i2, s2, st2) = (inner.clone(), stop.clone(), status.clone());
    let join = std::thread::Builder::new()
        .name(format!("daemon-attach-{id}"))
        .spawn(move || attach_reader_loop(state_root, id, i2, s2, st2))
        .ok();
    DaemonStream { inner, stop, status, join }
}

/// The outcome of one connection attempt (`stream_once`).
enum Outcome {
    /// The stop flag was observed → a best-effort `Detach` was sent; exit the loop.
    Stopped,
    /// The daemon reported the pane died → stop (no reconnect).
    PaneDied,
    /// EOF / error / overflow / gap → reconnect with backoff (re-attachable).
    Disconnected,
}

/// The reconnect-resilient reader loop: attach, stream, and on a drop retry with bounded
/// backoff until stopped or the pane dies. NEVER panics, NEVER spins (every retry sleeps).
fn attach_reader_loop(
    state_root: PathBuf,
    id: String,
    inner: Arc<Mutex<StreamBuf>>,
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<AttachState>>,
) {
    let mut backoff = BACKOFF_START;
    while !stop.load(Ordering::SeqCst) {
        match stream_once(&state_root, &id, &inner, &stop, &status) {
            Outcome::Stopped => break,
            Outcome::PaneDied => {
                *status.lock().unwrap() = AttachState::Dead;
                break;
            }
            Outcome::Disconnected => {
                *status.lock().unwrap() = AttachState::Disconnected;
                if sleep_interruptible(backoff, &stop) {
                    break; // stop observed during backoff
                }
                backoff = (backoff * 2).min(BACKOFF_MAX);
            }
        }
    }
}

/// One connection lifetime: connect → `Attach{id}` → read the response + snapshot/delta
/// frames → return why the connection ended.
fn stream_once(
    state_root: &Path,
    id: &str,
    inner: &Arc<Mutex<StreamBuf>>,
    stop: &Arc<AtomicBool>,
    status: &Arc<Mutex<AttachState>>,
) -> Outcome {
    let Some(sock) = socket_path(state_root) else { return Outcome::Disconnected };
    let mut stream = match UnixStream::connect(&sock) {
        Ok(s) => s,
        Err(_) => return Outcome::Disconnected,
    };
    let _ = stream.set_read_timeout(Some(READ_POLL));
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));

    if !write_request(&mut stream, &SocketRequest::Attach { id: id.to_string() }) {
        return Outcome::Disconnected;
    }

    let mut pending: Vec<u8> = Vec::new();
    let mut got_response = false;

    loop {
        if stop.load(Ordering::SeqCst) {
            // Best-effort Detach so the daemon deregisters promptly (it would also notice EOF
            // on the dropped connection).
            let _ = write_request(&mut stream, &SocketRequest::Detach { id: id.to_string() });
            return Outcome::Stopped;
        }
        match read_lines(&mut stream, &mut pending) {
            ReadStep::Eof => return Outcome::Disconnected,
            ReadStep::Idle => continue, // read timeout → re-check the stop flag
            ReadStep::Lines(lines) => {
                for raw in lines {
                    let line = raw.trim_end();
                    if line.is_empty() {
                        continue;
                    }
                    if !got_response {
                        got_response = true;
                        match parse_attach_response(line) {
                            AttachAck::Streaming => {
                                *status.lock().unwrap() = AttachState::Attached;
                                continue;
                            }
                            AttachAck::PaneDied => return Outcome::PaneDied,
                            AttachAck::Other => return Outcome::Disconnected,
                        }
                    }
                    // A frame line. Decode happens INSIDE apply; the buffer lock is taken only
                    // for the brief push/reset, never across the next socket read.
                    let frame: StreamFrame = match serde_json::from_str(line) {
                        Ok(f) => f,
                        // An unrecognized line (a future frame type) is ignored, not fatal.
                        Err(_) => continue,
                    };
                    match inner.lock().unwrap().apply(&frame) {
                        FrameStep::Ok => {}
                        FrameStep::PaneDied => return Outcome::PaneDied,
                        FrameStep::Reconnect => return Outcome::Disconnected,
                    }
                }
            }
        }
    }
}

/// Classification of the first reply line on an `Attach` connection.
enum AttachAck {
    Streaming,
    PaneDied,
    Other,
}

fn parse_attach_response(line: &str) -> AttachAck {
    match serde_json::from_str::<SocketResponse>(line) {
        Ok(r) if r.ok && r.code == response_code::STREAMING => AttachAck::Streaming,
        Ok(r) if r.code == response_code::PANE_DIED => AttachAck::PaneDied,
        _ => AttachAck::Other,
    }
}

/// One `read` worth of complete newline-terminated lines (mirrors the daemon server's
/// `read_available`): accumulate bytes into `pending`, split out every complete line, keep
/// any partial tail for the next call. A timeout with no full line is `Idle` (so the loop
/// re-checks the stop flag); EOF/error is `Eof`.
enum ReadStep {
    Lines(Vec<String>),
    Idle,
    Eof,
}

fn read_lines(stream: &mut UnixStream, pending: &mut Vec<u8>) -> ReadStep {
    let mut chunk = [0u8; 64 * 1024];
    match stream.read(&mut chunk) {
        Ok(0) => ReadStep::Eof,
        Ok(n) => {
            pending.extend_from_slice(&chunk[..n]);
            // Memory bound: a peer that never sends `\n` (or a pathological frame) must not
            // grow memory unbounded — treat it as a broken connection.
            if pending.len() > MAX_LINE_BYTES && !pending.contains(&b'\n') {
                pending.clear();
                return ReadStep::Eof;
            }
            let mut lines = Vec::new();
            while let Some(pos) = pending.iter().position(|&b| b == b'\n') {
                let raw: Vec<u8> = pending.drain(..=pos).collect();
                // A non-UTF-8 line becomes lossy → unparseable downstream (ignored), never panics.
                lines.push(String::from_utf8_lossy(&raw).into_owned());
            }
            if lines.is_empty() {
                ReadStep::Idle
            } else {
                ReadStep::Lines(lines)
            }
        }
        Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {
            ReadStep::Idle
        }
        Err(_) => ReadStep::Eof,
    }
}

/// Serialize + newline-terminate + write one request line. `false` on any write error.
fn write_request(stream: &mut UnixStream, req: &SocketRequest) -> bool {
    let Ok(mut line) = serde_json::to_string(req) else { return false };
    line.push('\n');
    if stream.write_all(line.as_bytes()).is_err() {
        return false;
    }
    stream.flush().is_ok()
}

/// Sleep `dur` in small slices, returning early `true` if the stop flag is set meanwhile (so
/// a Close/drop during reconnect backoff is observed within ~50ms, not after the full delay).
fn sleep_interruptible(dur: Duration, stop: &AtomicBool) -> bool {
    let step = Duration::from_millis(50);
    let mut slept = Duration::ZERO;
    while slept < dur {
        if stop.load(Ordering::SeqCst) {
            return true;
        }
        let nap = step.min(dur - slept);
        std::thread::sleep(nap);
        slept += nap;
    }
    stop.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::AtomicU32;
    use std::sync::mpsc;
    use std::time::Instant;

    // ───────────────────────── pure StreamBuf tests (no socket) ─────────────────────────

    fn snap(baseline: u64, data: &[u8]) -> StreamFrame {
        StreamFrame::snapshot("p", baseline, data)
    }
    fn delta(prev: u64, new: u64, data: &[u8]) -> StreamFrame {
        StreamFrame::delta("p", prev, new, data)
    }

    #[test]
    fn first_snapshot_starts_at_zero_and_is_clean_append() {
        let mut s = StreamBuf::new();
        assert_eq!(s.apply(&snap(6, b"hello ")), FrameStep::Ok);
        // gen_base 0 → a cursor at 0 reads the whole window, NOT truncated (clean first paint).
        let (start, bytes) = s.delta(0);
        assert_eq!(start, 0);
        assert_eq!(bytes, b"hello ");
    }

    #[test]
    fn accumulates_snapshot_then_deltas() {
        let mut s = StreamBuf::new();
        assert_eq!(s.apply(&snap(6, b"hello ")), FrameStep::Ok);
        assert_eq!(s.apply(&delta(6, 11, b"world")), FrameStep::Ok);
        assert_eq!(s.apply(&delta(11, 12, b"!")), FrameStep::Ok);
        assert_eq!(s.delta(0).1, b"hello world!");
    }

    #[test]
    fn cursor_delta_is_exact_and_flags_truncation() {
        let mut s = StreamBuf::new();
        s.apply(&snap(0, b"hello "));
        s.apply(&delta(0, 6, b"world!"));
        // exact mid-stream cursor → no truncation, exactly the tail.
        let (start, bytes) = s.delta(6);
        assert_eq!(start, 6);
        assert_eq!(bytes, b"world!");
        // a cursor past the end → whole window, start != since (caller flags truncated).
        let (start, bytes) = s.delta(9999);
        assert_ne!(start, 9999);
        assert_eq!(bytes, b"hello world!");
    }

    #[test]
    fn binary_and_newline_bytes_round_trip_through_the_buffer() {
        let mut s = StreamBuf::new();
        s.apply(&snap(0, b""));
        // a delta carrying NUL, 0xFF, and a raw newline (the base64 framing's whole point).
        let payload = [0x00u8, 0xFF, b'\n', b'a'];
        assert_eq!(s.apply(&delta(0, 4, &payload)), FrameStep::Ok);
        assert_eq!(s.delta(0).1, payload);
    }

    #[test]
    fn reattach_resets_window_and_forces_replay() {
        let mut s = StreamBuf::new();
        s.apply(&snap(4, b"AAAA")); // gen 1: window "AAAA", app offsets 0..4
        assert_eq!(s.delta(0).1, b"AAAA");
        // A second snapshot is a RE-ATTACH → reset to "BB", NOT "AAAABB".
        s.apply(&snap(2, b"BB"));
        assert_eq!(s.window(), b"BB", "re-attach resets the window, never appends");
        // A stale cursor from gen 1 (e.g. since=4) MUST report truncated (start != since).
        let (start, bytes) = s.delta(4);
        assert_ne!(start, 4, "stale gen-1 cursor → truncated → frontend full replay");
        assert_eq!(bytes, b"BB");
        // Even a fully-caught-up gen-1 cursor (since == gen-1 end) is forced to replay.
        // gen-1 end was 4; gen_base advanced to 4+1=5, so since<=4 is always < gen_base.
    }

    #[test]
    fn reattach_cursor_is_monotonic() {
        let mut s = StreamBuf::new();
        s.apply(&snap(0, b"first"));
        let n1 = { let (start, b) = s.delta(0); start + b.len() as u64 };
        s.apply(&snap(0, b"second-gen"));
        // The post-reattach replay cursor must be strictly ahead of the prior generation's.
        let (start2, _b2) = s.delta(0);
        assert!(start2 >= n1, "gen_base monotonically advances across re-attach: {start2} >= {n1}");
    }

    #[test]
    fn delta_contiguity_gap_triggers_reconnect() {
        let mut s = StreamBuf::new();
        s.apply(&snap(6, b"hello "));
        // a delta whose prev_total (99) != last applied total (6) is a GAP → reconnect.
        assert_eq!(s.apply(&delta(99, 104, b"world")), FrameStep::Reconnect);
    }

    #[test]
    fn pane_died_error_frame_stops() {
        let mut s = StreamBuf::new();
        s.apply(&snap(0, b"x"));
        assert_eq!(
            s.apply(&StreamFrame::error("p", code::PANE_DIED, "gone")),
            FrameStep::PaneDied
        );
    }

    #[test]
    fn overflow_error_frame_reconnects() {
        let mut s = StreamBuf::new();
        s.apply(&snap(0, b"x"));
        assert_eq!(
            s.apply(&StreamFrame::error("p", code::OVERFLOW, "slow")),
            FrameStep::Reconnect
        );
    }

    #[test]
    fn keepalive_is_ignored() {
        let mut s = StreamBuf::new();
        s.apply(&snap(3, b"abc"));
        assert_eq!(s.apply(&StreamFrame::keepalive()), FrameStep::Ok);
        assert_eq!(s.delta(0).1, b"abc", "keepalive does not touch the buffer");
    }

    // ───────────────────── socket integration tests (real reader thread) ─────────────────────

    static N: AtomicU32 = AtomicU32::new(0);

    /// A temp dir whose CHILD is the state_root, so `socket_path` = `<dir>/agent-teams-mcp.sock`
    /// stays under the macOS AF_UNIX ~104-byte path limit and is cleaned on drop.
    struct Scratch {
        dir: PathBuf,
    }
    impl Scratch {
        fn new() -> Self {
            let n = N.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("q4st{:x}_{n:x}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self { dir }
        }
        fn state_root(&self) -> PathBuf {
            self.dir.join("s")
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// The script one accepted connection follows.
    #[derive(Clone)]
    struct ConnScript {
        /// Reply `PANE_DIED` to the `Attach` instead of `STREAMING` (then close).
        pane_died_resp: bool,
        /// Frames to emit after `STREAMING`.
        frames: Vec<StreamFrame>,
        /// Keep the connection open after the frames (else close → client reconnects).
        hold: bool,
    }
    impl ConnScript {
        fn streaming(frames: Vec<StreamFrame>, hold: bool) -> Self {
            ConnScript { pane_died_resp: false, frames, hold }
        }
    }

    /// A scripted streaming `Attach` daemon stand-in: binds the REAL `socket_path`, accepts
    /// connections in a loop, and per connection follows `scripts[min(n, last)]`. Captures
    /// every request line (so a test asserts the client dialed `Attach`/`Detach`).
    struct FakeDaemon {
        reqs: mpsc::Receiver<String>,
        conns: Arc<AtomicU32>,
        stop: Arc<AtomicBool>,
        _join: JoinHandle<()>,
    }
    impl FakeDaemon {
        fn start(state_root: &Path, scripts: Vec<ConnScript>) -> Self {
            let sock = socket_path(state_root).unwrap();
            let _ = std::fs::remove_file(&sock);
            let listener = UnixListener::bind(&sock).unwrap();
            listener.set_nonblocking(true).unwrap();
            let (tx, rx) = mpsc::channel();
            let stop = Arc::new(AtomicBool::new(false));
            let conns = Arc::new(AtomicU32::new(0));
            let (stop2, conns2) = (stop.clone(), conns.clone());
            let scripts = Arc::new(scripts);
            let join = std::thread::spawn(move || {
                loop {
                    if stop2.load(Ordering::SeqCst) {
                        return;
                    }
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let n = conns2.fetch_add(1, Ordering::SeqCst) as usize;
                            let idx = n.min(scripts.len().saturating_sub(1));
                            let script = scripts[idx].clone();
                            let tx = tx.clone();
                            let st = stop2.clone();
                            std::thread::spawn(move || run_conn(stream, script, tx, st));
                        }
                        Err(e) if e.kind() == ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(15));
                        }
                        Err(_) => return,
                    }
                }
            });
            std::thread::sleep(Duration::from_millis(30));
            FakeDaemon { reqs: rx, conns, stop, _join: join }
        }
        /// Drain the request lines seen so far.
        fn drain_reqs(&self) -> Vec<String> {
            let mut v = Vec::new();
            while let Ok(l) = self.reqs.try_recv() {
                v.push(l);
            }
            v
        }
        fn conn_count(&self) -> u32 {
            self.conns.load(Ordering::SeqCst)
        }
    }
    impl Drop for FakeDaemon {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
        }
    }

    fn write_json<T: serde::Serialize>(stream: &mut UnixStream, v: &T) {
        let mut line = serde_json::to_string(v).unwrap();
        line.push('\n');
        let _ = stream.write_all(line.as_bytes());
        let _ = stream.flush();
    }

    fn run_conn(mut stream: UnixStream, script: ConnScript, tx: mpsc::Sender<String>, stop: Arc<AtomicBool>) {
        stream.set_read_timeout(Some(Duration::from_millis(60))).ok();
        let mut pending = Vec::new();
        // 1. read the Attach request line.
        let attach = loop {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            match read_lines(&mut stream, &mut pending) {
                ReadStep::Lines(mut ls) => break ls.remove(0),
                ReadStep::Idle => continue,
                ReadStep::Eof => return,
            }
        };
        let _ = tx.send(attach);
        // 2. respond.
        if script.pane_died_resp {
            write_json(&mut stream, &SocketResponse::err(response_code::PANE_DIED, "died"));
            return;
        }
        write_json(
            &mut stream,
            &SocketResponse {
                ok: true,
                code: response_code::STREAMING.to_string(),
                detail: "streaming".to_string(),
                data: None,
            },
        );
        for f in &script.frames {
            write_json(&mut stream, f);
        }
        if !script.hold {
            return; // close → client sees EOF → reconnects
        }
        // 3. hold open: relay any further request lines (Detach) until stop or client EOF.
        loop {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            match read_lines(&mut stream, &mut pending) {
                ReadStep::Lines(ls) => {
                    for l in ls {
                        let _ = tx.send(l);
                    }
                }
                ReadStep::Idle => continue,
                ReadStep::Eof => return,
            }
        }
    }

    /// Poll `cond` up to `dur`, returning whether it became true (test convergence helper).
    fn wait_until(dur: Duration, mut cond: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + dur;
        while Instant::now() < deadline {
            if cond() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        cond()
    }

    fn req_is(line: &str, want: &SocketRequest) -> bool {
        serde_json::from_str::<SocketRequest>(line.trim_end()).map(|r| &r == want).unwrap_or(false)
    }

    #[test]
    fn reader_accumulates_snapshot_then_deltas() {
        let s = Scratch::new();
        let fake = FakeDaemon::start(
            &s.state_root(),
            vec![ConnScript::streaming(
                vec![snap(6, b"hello "), delta(6, 11, b"world"), delta(11, 12, b"!")],
                true,
            )],
        );
        let ds = start(s.state_root(), "p".to_string());
        let h = ds.handle();
        let ok = wait_until(Duration::from_secs(3), || h.lock().unwrap().delta(0).1 == b"hello world!");
        assert!(ok, "streamed snapshot+deltas accumulated: got {:?}", h.lock().unwrap().delta(0).1);
        assert_eq!(ds.status(), AttachState::Attached);
        // the client dialed Attach{p}.
        assert!(
            fake.drain_reqs().iter().any(|l| req_is(l, &SocketRequest::Attach { id: "p".into() })),
            "client dialed Attach"
        );
        ds.stop_and_join();
    }

    #[test]
    fn reader_serves_cursor_delta() {
        let s = Scratch::new();
        let _fake = FakeDaemon::start(
            &s.state_root(),
            vec![ConnScript::streaming(vec![snap(0, b"hello "), delta(0, 6, b"world!")], true)],
        );
        let ds = start(s.state_root(), "p".to_string());
        let h = ds.handle();
        assert!(wait_until(Duration::from_secs(3), || h.lock().unwrap().delta(0).1 == b"hello world!"));
        let (start_off, bytes) = h.lock().unwrap().delta(6);
        assert_eq!((start_off, bytes), (6, b"world!".to_vec()));
        ds.stop_and_join();
    }

    #[test]
    fn drop_stops_reader_and_sends_detach() {
        let s = Scratch::new();
        let fake = FakeDaemon::start(
            &s.state_root(),
            vec![ConnScript::streaming(vec![snap(2, b"hi")], true)],
        );
        let ds = start(s.state_root(), "p".to_string());
        let h = ds.handle();
        assert!(wait_until(Duration::from_secs(3), || !h.lock().unwrap().window().is_empty()));
        let started = Instant::now();
        ds.stop_and_join(); // sets stop + joins → asserts the thread actually exits (no leak)
        assert!(started.elapsed() < Duration::from_secs(2), "reader exits promptly on stop");
        // the daemon stand-in observed a Detach{p} line on stop.
        assert!(
            wait_until(Duration::from_secs(2), || fake
                .drain_reqs()
                .iter()
                .any(|l| req_is(l, &SocketRequest::Detach { id: "p".into() }))),
            "server saw Detach on stop"
        );
    }

    #[test]
    fn reattach_after_drop_resets_buffer() {
        let s = Scratch::new();
        // conn 1: snapshot "AAAA" then CLOSE (hold=false) → client reconnects.
        // conn 2: snapshot "BB" then hold.
        let _fake = FakeDaemon::start(
            &s.state_root(),
            vec![
                ConnScript::streaming(vec![snap(4, b"AAAA")], false),
                ConnScript::streaming(vec![snap(2, b"BB")], true),
            ],
        );
        let ds = start(s.state_root(), "p".to_string());
        let h = ds.handle();
        // eventually the second-generation snapshot replaces (does not append to) the first.
        let ok = wait_until(Duration::from_secs(4), || h.lock().unwrap().window() == b"BB");
        assert!(ok, "re-attach reset to BB, got {:?}", h.lock().unwrap().window());
        // a stale gen-1 cursor reports truncated (start != since).
        let (start_off, _bytes) = h.lock().unwrap().delta(4);
        assert_ne!(start_off, 4);
        ds.stop_and_join();
    }

    #[test]
    fn pane_died_response_stops_reader_no_reconnect() {
        let s = Scratch::new();
        let fake = FakeDaemon::start(
            &s.state_root(),
            vec![ConnScript { pane_died_resp: true, frames: vec![], hold: false }],
        );
        let ds = start(s.state_root(), "p".to_string());
        assert!(
            wait_until(Duration::from_secs(3), || ds.status() == AttachState::Dead),
            "PANE_DIED → status Dead"
        );
        // only ONE connection — no reconnect after PANE_DIED.
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(fake.conn_count(), 1, "no reconnect after PANE_DIED");
        ds.stop_and_join();
    }

    #[test]
    fn reconnects_after_server_drop() {
        let s = Scratch::new();
        // First server: holds the connection, then we drop it → client must reconnect.
        let fake1 = FakeDaemon::start(
            &s.state_root(),
            vec![ConnScript::streaming(vec![snap(2, b"hi")], true)],
        );
        let ds = start(s.state_root(), "p".to_string());
        let h = ds.handle();
        assert!(wait_until(Duration::from_secs(3), || ds.status() == AttachState::Attached));
        assert!(h.lock().unwrap().window() == b"hi");
        // Drop server 1 → connection EOF → reader marks Disconnected + retries.
        drop(fake1);
        assert!(
            wait_until(Duration::from_secs(3), || ds.status() == AttachState::Disconnected),
            "server drop → Disconnected (re-attachable)"
        );
        // Bind a fresh server → the reader reconnects + re-Attaches + re-snapshots.
        let fake2 = FakeDaemon::start(
            &s.state_root(),
            vec![ConnScript::streaming(vec![snap(3, b"new")], true)],
        );
        assert!(
            wait_until(Duration::from_secs(5), || ds.status() == AttachState::Attached
                && h.lock().unwrap().window() == b"new"),
            "reader re-attached to the fresh server and re-synced the snapshot"
        );
        assert!(
            fake2.drain_reqs().iter().any(|l| req_is(l, &SocketRequest::Attach { id: "p".into() })),
            "fresh server saw a re-Attach"
        );
        ds.stop_and_join();
    }

    #[test]
    fn unreachable_daemon_does_not_panic_and_is_re_attachable() {
        let s = Scratch::new();
        // No server bound → connect refused → Disconnected, retry (never panics, never spins).
        let ds = start(s.state_root(), "p".to_string());
        assert!(
            wait_until(Duration::from_secs(2), || ds.status() == AttachState::Disconnected),
            "no daemon → Disconnected (re-attachable), no panic"
        );
        ds.stop_and_join();
    }
}
