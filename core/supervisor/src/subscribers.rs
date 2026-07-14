//! Per-pane output subscriber registry — the PUSH-from-PTY-reader delta substrate
//! (Phase 08 Sub-build 3 / slice 3, design §4 "THE CRUX").
//!
//! The PTY reader thread ([`crate::Supervisor::spawn`]) appends each chunk to the
//! pane's [`crate::PaneBuffer`] and THEN fans the SAME chunk out to every registered
//! subscriber via a NON-BLOCKING `try_send`. This is the "never a new `ByteRing`
//! cursor" rule: the snapshot taken at `Attach` time is the ONLY read from the buffer;
//! every byte after that is PUSHED. The delta frame's data is exactly the chunk the
//! reader just appended — it is NEVER re-read from `recent()` (which evicts).
//!
//! ## Lock ordering (HARD CONSTRAINT — design §4 build note)
//!
//! Both the reader fan-out ([`push_and_fanout`]) and a new `Attach`
//! ([`subscribe_to`]) take the pane BUFFER lock FIRST, then this SUBSCRIBER lock —
//! the SAME order — so snapshot+register is ATOMIC with respect to push+fanout (no
//! duplicate and no gap on the first delta: the first fanout after a register has
//! `prev_total == baseline`). NEITHER path EVER takes the daemon `DaemonSups` map
//! lock — the per-pane registry exists precisely so the reader never needs it. A
//! map-lock-under-buffer-lock would be a deadlock hazard; it is forbidden here.
//!
//! ## MF-B — bounded queue + drop-on-overflow
//!
//! Each subscription owns a BOUNDED `sync_channel` ([`DEFAULT_SUB_CAPACITY`] frames).
//! The reader `try_send`s; a FULL queue (a slow client) or a GONE receiver ⇒ that
//! subscriber is DROPPED ([`SubscriberSet::fanout`] returns it via `retain`), which
//! UNBLOCKS the reader. The reader NEVER blocks on a slow subscriber. The dropped
//! subscriber's receiver then sees `Disconnected`; the connection turns that into an
//! error frame and the client re-`Attach`es for a fresh snapshot if it wants more.

use crate::PaneBuffer;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};

/// PROCESS-GLOBAL monotonic source of subscription keys.
///
/// A pane's [`SubscriberSet`] is SHARED across every daemon connection that attaches the
/// same pane (the reader and each connection hold clones of the SAME `Arc<Mutex<_>>`). A
/// key that is merely unique *within one connection* therefore COLLIDES across
/// connections — two connections attaching the same pane would both mint `0`, and a
/// `deregister(0)` by one ([`SubscriberSet::deregister`] removes EVERY entry with that
/// key) would silently tear down the OTHER connection's still-live subscriber. Minting
/// every key from this global counter makes each `Attach` across ALL connections unique,
/// so a deregister removes exactly its own subscriber. Starts at 1 (0 is never a live key).
static NEXT_SUB_KEY: AtomicU64 = AtomicU64::new(1);

/// Allocate the next globally-unique subscription key (see [`NEXT_SUB_KEY`]).
fn next_sub_key() -> u64 {
    NEXT_SUB_KEY.fetch_add(1, Ordering::Relaxed)
}

/// One push the reader fans out: the chunk just appended to the buffer, plus the
/// buffer's absolute byte total BEFORE and AFTER the push. `new_total - prev_total ==
/// data.len()` always. A subscriber whose recorded last total != an incoming
/// `prev_total` has a GAP (output it never saw) and should re-`Attach` — but within a
/// LIVE subscription there is never a gap (overflow drops the WHOLE subscription).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputDelta {
    /// `total_pushed` (== [`PaneBuffer::end`]) BEFORE this chunk was appended.
    pub prev_total: u64,
    /// `total_pushed` AFTER this chunk was appended (`prev_total + data.len()`).
    pub new_total: u64,
    /// The exact chunk the reader appended — never re-read from the (evicting) ring.
    pub data: Vec<u8>,
}

/// Default bounded queue depth (frames) per subscription (MF-B). A slow client that
/// lets this fill loses its subscription (drop-on-overflow) rather than ever blocking
/// the perf-critical reader thread.
pub const DEFAULT_SUB_CAPACITY: usize = 256;

/// One registered subscription's sender end (lives inside the [`SubscriberSet`]).
struct Subscriber {
    key: u64,
    tx: SyncSender<OutputDelta>,
}

/// Per-pane subscriber registry. Owned per-[`crate::Supervisor`] behind an
/// `Arc<Mutex<_>>`; the reader thread holds its OWN clone of the same `Arc` so it can
/// fan out WITHOUT the daemon map lock.
#[derive(Default)]
pub struct SubscriberSet {
    subs: Vec<Subscriber>,
    /// Set ONCE when the PTY reader thread exits ([`close_all`](SubscriberSet::close_all))
    /// — the authoritative "the reader is gone" signal. A late [`subscribe_to`] that races
    /// the reader's exit (the child may not be reaped yet, so liveness still reads `true`)
    /// checks this UNDER the subscriber lock and refuses to register a subscriber that
    /// nothing would ever push to or close again (the zombie-subscription race).
    dead: bool,
}

impl SubscriberSet {
    /// A new, empty registry.
    pub fn new() -> Self {
        Self {
            subs: Vec::new(),
            dead: false,
        }
    }

    /// Register a subscription's bounded sender under the globally-unique `key`
    /// ([`next_sub_key`]). `pub(crate)` — only [`subscribe_to`] registers, and it does so
    /// UNDER the buffer lock (atomicity) after checking [`is_dead`](SubscriberSet::is_dead).
    pub(crate) fn register(&mut self, key: u64, tx: SyncSender<OutputDelta>) {
        self.subs.push(Subscriber { key, tx });
    }

    /// Remove a subscription by `key` (Detach / connection drop). No-op if absent. Because
    /// keys are process-globally unique ([`NEXT_SUB_KEY`]), this removes EXACTLY the one
    /// subscriber that owns `key` — never a sibling connection's subscriber on the same pane.
    pub fn deregister(&mut self, key: u64) {
        self.subs.retain(|s| s.key != key);
    }

    /// Drop EVERY subscriber's sender AND mark the set DEAD (pane death): the reader calls
    /// this when its PTY read loop ends, so every subscription's receiver promptly sees
    /// `Disconnected` and the connection can emit a `PANE_DIED` frame. Setting `dead` also
    /// fences off a racing [`subscribe_to`] from registering a zombie. Idempotent.
    pub fn close_all(&mut self) {
        self.subs.clear();
        self.dead = true;
    }

    /// Drop every subscriber's sender WITHOUT marking the set dead — models the reader
    /// dropping all current subscribers on overflow (the per-sub [`fanout`] retain applied
    /// to every entry at once). The pane is still ALIVE, so new `Attach`es may register and
    /// each dropped receiver classifies as `OVERFLOW` (not `PANE_DIED`). Distinct from
    /// [`close_all`](SubscriberSet::close_all), which signals death.
    pub fn drop_all_overflow(&mut self) {
        self.subs.clear();
    }

    /// `true` once the reader thread has exited ([`close_all`](SubscriberSet::close_all)).
    /// Read under the subscriber lock by [`subscribe_to`] to fence off the zombie race.
    pub fn is_dead(&self) -> bool {
        self.dead
    }

    /// Current subscriber count (tests / diagnostics).
    pub fn len(&self) -> usize {
        self.subs.len()
    }

    /// `true` when there are no subscribers — the zero-cost common case the reader
    /// fan-out short-circuits on.
    pub fn is_empty(&self) -> bool {
        self.subs.is_empty()
    }

    /// Fan ONE just-pushed chunk out to every subscriber via NON-BLOCKING `try_send`
    /// (MF-B). A subscriber whose bounded queue is FULL (slow client) or whose receiver
    /// is GONE is DROPPED (the design's drop-on-overflow policy) — the reader NEVER
    /// blocks on a slow peer. `data` is cloned per subscriber (chunks are small, N is
    /// tiny); the no-subscriber path allocates nothing.
    pub fn fanout(&mut self, prev_total: u64, new_total: u64, data: &[u8]) {
        if self.subs.is_empty() {
            return; // zero-cost on the common (no-subscriber) path
        }
        self.subs.retain(|s| {
            let msg = OutputDelta {
                prev_total,
                new_total,
                data: data.to_vec(),
            };
            match s.tx.try_send(msg) {
                Ok(()) => true,
                // FULL (slow client) or DISCONNECTED (receiver dropped) → drop the sub.
                // try_send NEVER blocks, so the reader keeps reading regardless.
                Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => false,
            }
        });
    }
}

/// A shareable handle to a pane's subscriber registry (cloned for the reader thread
/// and for each connection's [`Subscription`]).
pub type SubscriberHandle = Arc<Mutex<SubscriberSet>>;

/// The result of [`subscribe_to`]: the atomic snapshot baseline + retained window, the
/// receiver the connection drains for delta frames, and the registry handle it
/// deregisters from on Detach / close.
pub struct Subscription {
    /// `total_pushed` (== [`PaneBuffer::end`]) at snapshot time — the baseline the
    /// first delta's `prev_total` is GUARANTEED to equal (atomic register under the
    /// buffer lock).
    pub baseline: u64,
    /// The retained scrollback window at snapshot time (== [`PaneBuffer::retained`]).
    pub snapshot: Vec<u8>,
    /// The subscription's unique key (for [`SubscriberSet::deregister`]).
    pub key: u64,
    /// Bounded receiver the connection drains (`try_recv`) to emit delta frames.
    pub rx: Receiver<OutputDelta>,
    /// The pane's registry — `registry.lock().deregister(key)` on Detach / close.
    pub registry: SubscriberHandle,
}

/// Lock a poisoned-recoverable mutex (mirror the rest of the crate: a prior panic while
/// the lock was held leaves the protected data usable, so recover rather than panic a
/// reader/connection thread).
fn lock_recover<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// `Attach`: ATOMICALLY snapshot the retained window + baseline and register a bounded
/// subscriber under a process-globally-unique key ([`next_sub_key`]). Takes the BUFFER
/// lock first, then the SUBSCRIBER lock (the canonical order — §4) so no reader push can
/// interleave between reading `baseline` and registering: the first delta the new
/// subscriber sees has `prev_total == baseline`.
///
/// Returns `None` when the set is already DEAD (the reader thread exited via
/// [`close_subscribers`] between the caller's liveness gate and here — the child may not
/// be reaped yet, so liveness still reads `true`). The dead-check and the register happen
/// under the SAME subscriber lock, so this is race-free: a `None` means "nothing would
/// ever push to or close this subscriber" and the caller must report `PANE_DIED` instead
/// of leaking a silent, never-closing zombie subscription.
///
/// `capacity` is clamped to `>= 1` (MF-B): a `sync_channel(0)` is a rendezvous channel
/// on which `try_send` only succeeds when a receiver is blocked in `recv` — but the
/// connection drains with `try_recv`, so a 0-cap queue would silently drop EVERY delta.
pub fn subscribe_to(
    buf: &Arc<Mutex<PaneBuffer>>,
    subs: &SubscriberHandle,
    capacity: usize,
) -> Option<Subscription> {
    let capacity = capacity.max(1);
    let key = next_sub_key();
    // BUFFER lock FIRST (held across the register below → atomic vs push_and_fanout).
    let o = lock_recover(buf);
    let baseline = o.end();
    let snapshot = o.retained().to_vec();
    let (tx, rx) = sync_channel(capacity);
    // SUBSCRIBER lock SECOND, still UNDER the buffer lock — the §4 lock order. The
    // dead-check is under THIS lock too, so it cannot race the reader's close_all.
    {
        let mut set = lock_recover(subs);
        if set.is_dead() {
            return None; // reader already exited → don't register a zombie (→ PANE_DIED)
        }
        set.register(key, tx);
    }
    Some(Subscription {
        baseline,
        snapshot,
        key,
        rx,
        registry: Arc::clone(subs),
    })
    // both guards drop here
}

/// The PTY reader thread's per-chunk critical section: append `chunk` to the buffer,
/// THEN fan the SAME `chunk` out to subscribers — both UNDER the BUFFER lock (atomic vs
/// [`subscribe_to`]), fan-out NON-BLOCKING. Shared by the live reader and the daemon's
/// test double so a test exercises the EXACT production push+fanout+lock-order. The
/// buffer's `end()` advances by exactly `chunk.len()` per push (compaction only drains
/// the front, never changes `end()`), so `prev`/`new` bracket this chunk precisely.
pub fn push_and_fanout(buf: &Mutex<PaneBuffer>, subs: &Mutex<SubscriberSet>, chunk: &[u8]) {
    let mut o = lock_recover(buf);
    let prev = o.end();
    o.push(chunk);
    let new = o.end();
    // SUBSCRIBER lock UNDER the buffer lock (the §4 order). NEVER the daemon map lock.
    lock_recover(subs).fanout(prev, new, chunk);
}

/// Drop every subscriber's sender for a pane whose PTY read loop has ended (death), so
/// each subscription's receiver sees `Disconnected` and the connection can emit a
/// `PANE_DIED` frame. Idempotent — safe to call once on reader exit.
pub fn close_subscribers(subs: &Mutex<SubscriberSet>) {
    lock_recover(subs).close_all();
}

/// Drop every subscriber's sender WITHOUT marking the pane dead (the reader overflowed and
/// dropped all current subscribers, but the pane is still live). Each dropped receiver then
/// classifies as `OVERFLOW` rather than `PANE_DIED`, and a fresh `Attach` may re-register.
pub fn overflow_drop_subscribers(subs: &Mutex<SubscriberSet>) {
    lock_recover(subs).drop_all_overflow();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_buf(cap: usize) -> Arc<Mutex<PaneBuffer>> {
        Arc::new(Mutex::new(PaneBuffer::new(cap)))
    }
    fn new_subs() -> SubscriberHandle {
        Arc::new(Mutex::new(SubscriberSet::new()))
    }

    #[test]
    fn snapshot_on_attach_returns_recent_and_baseline_then_deltas_arrive() {
        let buf = new_buf(64);
        let subs = new_subs();
        // pre-existing scrollback before the attach.
        push_and_fanout(&buf, &subs, b"hello ");
        let sub = subscribe_to(&buf, &subs, DEFAULT_SUB_CAPACITY).expect("live pane");
        assert_eq!(
            sub.snapshot, b"hello ",
            "snapshot == retained window at attach"
        );
        assert_eq!(sub.baseline, 6, "baseline == total_pushed at attach");

        // subsequent reader chunks arrive as deltas with correct prev/new totals.
        push_and_fanout(&buf, &subs, b"world");
        let d1 = sub.rx.try_recv().expect("first delta");
        assert_eq!(
            (d1.prev_total, d1.new_total),
            (6, 11),
            "delta brackets the chunk"
        );
        assert_eq!(d1.data, b"world");
        assert_eq!(
            d1.prev_total, sub.baseline,
            "first delta is contiguous with the snapshot"
        );

        push_and_fanout(&buf, &subs, b"!");
        let d2 = sub.rx.try_recv().expect("second delta");
        assert_eq!((d2.prev_total, d2.new_total), (11, 12));
        assert_eq!(d2.data, b"!");
        assert_eq!(
            d2.prev_total, d1.new_total,
            "deltas are contiguous (no gap in a live sub)"
        );
    }

    #[test]
    fn overflow_drops_the_subscription_and_never_blocks_the_reader() {
        // A bounded cap of 2; the consumer never drains. The 3rd push overflows the
        // queue → the subscriber is DROPPED. The reader (push_and_fanout) returns every
        // time — it never blocks on the full queue.
        let buf = new_buf(1024);
        let subs = new_subs();
        let sub = subscribe_to(&buf, &subs, 2).expect("live pane");
        assert_eq!(subs.lock().unwrap().len(), 1, "registered");

        push_and_fanout(&buf, &subs, b"a"); // queued (1/2)
        push_and_fanout(&buf, &subs, b"b"); // queued (2/2 full)
                                            // This push would block a naive sender; try_send drops the sub instead and the
                                            // call RETURNS (proven by reaching the assert below).
        push_and_fanout(&buf, &subs, b"c"); // overflow → sub dropped
        assert_eq!(
            subs.lock().unwrap().len(),
            0,
            "overflowing sub is dropped from the set"
        );

        // The reader keeps making progress on further pushes (no deadlock / no block).
        push_and_fanout(&buf, &subs, b"d");
        push_and_fanout(&buf, &subs, b"e");

        // The dropped subscriber drains its 2 buffered frames, then sees Disconnected
        // (its sender was removed from the set) — the connection's re-attach signal.
        assert_eq!(sub.rx.try_recv().unwrap().data, b"a");
        assert_eq!(sub.rx.try_recv().unwrap().data, b"b");
        assert!(matches!(
            sub.rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn multiplex_isolation_slow_sub_dropped_other_keeps_receiving() {
        // Two subscriptions on the SAME pane. One (cap 1) overflows and is dropped; the
        // other (large cap) keeps receiving every delta — isolation per subscription.
        let buf = new_buf(1024);
        let subs = new_subs();
        let slow = subscribe_to(&buf, &subs, 1).expect("live pane");
        let fast = subscribe_to(&buf, &subs, 256).expect("live pane");
        assert_eq!(subs.lock().unwrap().len(), 2);

        push_and_fanout(&buf, &subs, b"1"); // slow: queued(1/1 full); fast: queued
        push_and_fanout(&buf, &subs, b"2"); // slow: OVERFLOW→dropped; fast: queued
        assert_eq!(
            subs.lock().unwrap().len(),
            1,
            "only the slow sub was dropped"
        );

        push_and_fanout(&buf, &subs, b"3"); // fast still receives
                                            // fast got all three deltas in order; slow only its first buffered frame.
        assert_eq!(fast.rx.try_recv().unwrap().data, b"1");
        assert_eq!(fast.rx.try_recv().unwrap().data, b"2");
        assert_eq!(fast.rx.try_recv().unwrap().data, b"3");
        assert_eq!(slow.rx.try_recv().unwrap().data, b"1");
        assert!(matches!(
            slow.rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn detach_removes_one_subscription_others_persist() {
        let buf = new_buf(1024);
        let subs = new_subs();
        let a = subscribe_to(&buf, &subs, 256).expect("live pane");
        let _b = subscribe_to(&buf, &subs, 256).expect("live pane");
        assert_eq!(subs.lock().unwrap().len(), 2);
        assert_ne!(
            a.key, _b.key,
            "every subscription gets a globally-unique key"
        );

        // Detach a (its key) — b persists.
        subs.lock().unwrap().deregister(a.key);
        assert_eq!(subs.lock().unwrap().len(), 1);

        push_and_fanout(&buf, &subs, b"x");
        // a is gone → its receiver sees Disconnected; b still receives.
        assert!(matches!(
            a.rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        ));
        assert_eq!(_b.rx.try_recv().unwrap().data, b"x");
    }

    #[test]
    fn two_subscriptions_same_pane_get_distinct_keys_and_deregister_in_isolation() {
        // The cross-connection collision regression: two subscriptions on the SAME shared
        // registry (as two daemon connections attaching one pane produce) must get DISTINCT
        // keys, so deregistering one (Detach / connection close) leaves the other's live
        // subscriber intact — NOT torn down by a shared key-0 retain.
        let buf = new_buf(1024);
        let subs = new_subs();
        let a = subscribe_to(&buf, &subs, 256).expect("live pane"); // "connection A"
        let b = subscribe_to(&buf, &subs, 256).expect("live pane"); // "connection B"
        assert_ne!(
            a.key, b.key,
            "distinct keys across attaches on the same shared registry"
        );
        assert_eq!(subs.lock().unwrap().len(), 2);

        // Connection A detaches/closes (deregister by A's key only).
        subs.lock().unwrap().deregister(a.key);
        assert_eq!(
            subs.lock().unwrap().len(),
            1,
            "only A removed; B's subscriber survives"
        );

        // B still receives — it was NOT collaterally torn down.
        push_and_fanout(&buf, &subs, b"to-B");
        assert_eq!(b.rx.try_recv().unwrap().data, b"to-B");
        assert!(matches!(
            a.rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn subscribe_to_a_dead_set_returns_none_no_zombie() {
        // The attach-vs-reader-death race: the reader exited (close_all → dead) but the
        // caller's liveness gate still raced `true`. subscribe_to must REFUSE to register
        // (→ None) so the connection reports PANE_DIED instead of a silent never-closing
        // subscriber that nothing will ever push to or disconnect.
        let buf = new_buf(1024);
        let subs = new_subs();
        close_subscribers(&subs); // reader exited
        assert!(
            subscribe_to(&buf, &subs, 256).is_none(),
            "no zombie on a dead set"
        );
        assert_eq!(subs.lock().unwrap().len(), 0, "nothing registered");
    }

    #[test]
    fn overflow_drop_keeps_the_set_alive_for_re_subscription() {
        // Overflow-dropping all current subscribers (reader stayed alive) must NOT mark the
        // set dead — a fresh Attach can still register, unlike after close_all (death).
        let buf = new_buf(1024);
        let subs = new_subs();
        let _gone = subscribe_to(&buf, &subs, 1).expect("live pane");
        overflow_drop_subscribers(&subs);
        assert!(
            !subs.lock().unwrap().is_dead(),
            "overflow drop is not death"
        );
        assert!(
            subscribe_to(&buf, &subs, 256).is_some(),
            "re-attach allowed after overflow"
        );
    }

    #[test]
    fn close_all_signals_pane_death_to_every_subscriber() {
        let buf = new_buf(1024);
        let subs = new_subs();
        let a = subscribe_to(&buf, &subs, 256).expect("live pane");
        let b = subscribe_to(&buf, &subs, 256).expect("live pane");
        push_and_fanout(&buf, &subs, b"tail");

        // Reader exit: close all senders.
        close_subscribers(&subs);

        // Each subscriber drains its buffered tail, THEN sees Disconnected (pane died).
        assert_eq!(a.rx.try_recv().unwrap().data, b"tail");
        assert!(matches!(
            a.rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        ));
        assert_eq!(b.rx.try_recv().unwrap().data, b"tail");
        assert!(matches!(
            b.rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn zero_capacity_is_clamped_so_deltas_are_not_silently_dropped() {
        // MF-B clamp: cap 0 would be a rendezvous channel → try_send always Full →
        // every delta dropped. subscribe_to clamps to >= 1, so one delta survives.
        let buf = new_buf(1024);
        let subs = new_subs();
        let sub = subscribe_to(&buf, &subs, 0).expect("live pane");
        push_and_fanout(&buf, &subs, b"z");
        assert_eq!(sub.rx.try_recv().unwrap().data, b"z", "cap clamped to >=1");
    }

    #[test]
    fn no_subscriber_fanout_is_a_noop() {
        // The common path: a pane with no subscribers. push_and_fanout must not error
        // and the buffer still advances (the GUI read path is unaffected).
        let buf = new_buf(1024);
        let subs = new_subs();
        push_and_fanout(&buf, &subs, b"abc");
        push_and_fanout(&buf, &subs, b"de");
        assert_eq!(buf.lock().unwrap().end(), 5);
        assert_eq!(buf.lock().unwrap().retained(), b"abcde");
        assert!(subs.lock().unwrap().is_empty());
    }

    #[test]
    fn concurrent_reader_fanout_never_deadlocks_under_attach_churn() {
        // Stress: one "reader" thread pushes+fans out continuously while another thread
        // churns subscribe/deregister. The buffer→subscriber lock order is consistent in
        // BOTH paths (no map lock anywhere), so this must complete without deadlock.
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::thread;
        let buf = new_buf(4096);
        let subs = new_subs();
        let stop = Arc::new(AtomicBool::new(false));

        let reader = {
            let (buf, subs, stop) = (Arc::clone(&buf), Arc::clone(&subs), Arc::clone(&stop));
            thread::spawn(move || {
                let mut n = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    push_and_fanout(&buf, &subs, b"chunk");
                    n += 1;
                }
                n // proves the reader made progress (never wedged)
            })
        };
        let churner = {
            let (buf, subs, stop) = (Arc::clone(&buf), Arc::clone(&subs), Arc::clone(&stop));
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    // tiny cap → frequent overflow drops; key is globally minted internally.
                    let s = subscribe_to(&buf, &subs, 4).expect("live pane");
                    // drain a little then drop/deregister
                    let _ = s.rx.try_recv();
                    subs.lock().unwrap().deregister(s.key);
                }
            })
        };

        thread::sleep(std::time::Duration::from_millis(150));
        stop.store(true, Ordering::Relaxed);
        let pushes = reader.join().expect("reader thread did not wedge");
        churner.join().expect("churner thread did not wedge");
        assert!(pushes > 0, "the reader fan-out kept making progress");
    }
}
