//! Bounded byte ring buffer — Phase 08 Sub-build 2 substrate.
//!
//! Today each pane's PTY output lives in an unbounded `Arc<Mutex<Vec<u8>>>`
//! (`core/supervisor`), fed by a reader thread that only ever appends. In a
//! long-lived daemon that is a leak. [`ByteRing`] is the bounded replacement:
//! it retains at most `capacity()` bytes, dropping the oldest as new bytes
//! arrive, so memory is flat regardless of how much output a pane produces.
//!
//! Scope is deliberately the buffer ONLY. The delta / cursor / `since(position)`
//! / subscription / streaming API is **deferred to Sub-build 3**, whose
//! streaming protocol is still undesigned — committing to a read-cursor shape
//! now would almost certainly be the wrong shape later. [`ByteRing::recent`]
//! is therefore a plain whole-window read (named `recent`, not `snapshot`, to
//! avoid implying the deferred streaming surface), not a cursor. This crate is
//! std-only, pure, and holds no `Arc`/`Mutex` — the caller wraps it.

use std::collections::VecDeque;

/// A fixed-capacity byte buffer that retains only the most recent
/// `capacity()` bytes. Appends never grow it past the bound: once full, the
/// oldest bytes are dropped to make room. Load-bearing invariant: after any
/// [`push`](ByteRing::push), `len() <= capacity()` always holds.
#[derive(Debug, Clone)]
pub struct ByteRing {
    /// Retained bytes, oldest at the front, newest at the back.
    buf: VecDeque<u8>,
    /// Max retained bytes — the bound. May be 0 (retains nothing).
    cap: usize,
    /// Monotonic count of every byte ever pushed (not just retained).
    total: u64,
}

impl ByteRing {
    /// Create a ring that retains at most `cap` bytes.
    ///
    /// `cap == 0` is legal: the ring retains nothing — every [`push`](ByteRing::push)
    /// still advances [`total_pushed`](ByteRing::total_pushed) but `len()` stays 0
    /// and [`recent`](ByteRing::recent) is always empty.
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            buf: VecDeque::with_capacity(cap),
            cap,
            total: 0,
        }
    }

    /// Append `bytes`, dropping the oldest retained bytes as needed so that
    /// `len() <= capacity()` still holds afterwards.
    ///
    /// If `bytes` is larger than `capacity()`, only its last `capacity()` bytes
    /// are retained (everything before them would be evicted immediately anyway).
    pub fn push(&mut self, bytes: &[u8]) {
        self.total += bytes.len() as u64;

        if self.cap == 0 {
            return;
        }

        // A single push bigger than the bound: keep only its tail.
        if bytes.len() >= self.cap {
            self.buf.clear();
            self.buf.extend(&bytes[bytes.len() - self.cap..]);
            return;
        }

        // Evict just enough of the oldest to fit the incoming bytes.
        let needed = self.buf.len() + bytes.len();
        if needed > self.cap {
            self.buf.drain(..needed - self.cap);
        }
        self.buf.extend(bytes);
    }

    /// Copy the retained window out, oldest byte first.
    ///
    /// This is the "recent scrollback" a future re-attach reads — a plain,
    /// whole-window read, NOT a cursor or delta (see the crate-level note on
    /// the deferred streaming API).
    pub fn recent(&self) -> Vec<u8> {
        self.buf.iter().copied().collect()
    }

    /// Number of bytes currently retained. Always `<= capacity()`.
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// `true` when nothing is retained.
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// The configured bound — the max bytes this ring will ever retain.
    pub fn capacity(&self) -> usize {
        self.cap
    }

    /// Monotonic count of every byte ever pushed, including bytes that were
    /// later evicted (or never retained, when `capacity() == 0`). Provided as
    /// a cheap liveness/throughput signal — NOT a read cursor; no delta or
    /// `since(position)` API is built on it (deferred to Sub-build 3).
    pub fn total_pushed(&self) -> u64 {
        self.total
    }

    /// Count of bytes that were pushed but are no longer retained — i.e. how
    /// many the bound has dropped (`total_pushed() - len()`). A `> 0` result
    /// means the buffer overflowed its bound at least once and the oldest
    /// output was lost; a re-attach reading [`recent`](ByteRing::recent) is
    /// seeing a truncated tail, not the full history.
    ///
    /// Like [`total_pushed`](ByteRing::total_pushed) this is a diagnostic
    /// COUNT, NOT a read cursor — it deliberately does not expose the evicted
    /// bytes or any `since(position)` surface (deferred to Sub-build 3). The
    /// subtraction never underflows: `len()` is the retained byte count, which
    /// is always `<= total` (every retained byte was pushed).
    pub fn evicted(&self) -> u64 {
        self.total - self.buf.len() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn under_cap_retains_all() {
        let mut r = ByteRing::with_capacity(100);
        r.push(&[1u8; 30]);
        r.push(&[2u8; 40]);
        assert_eq!(r.len(), 70);
        let mut expected = vec![1u8; 30];
        expected.extend(vec![2u8; 40]);
        assert_eq!(r.recent(), expected);
    }

    #[test]
    fn over_cap_drops_oldest() {
        let mut r = ByteRing::with_capacity(10);
        r.push(b"0123456789");
        r.push(b"ABCDE");
        assert_eq!(r.len(), 10);
        assert_eq!(r.recent(), b"56789ABCDE");
    }

    #[test]
    fn push_larger_than_cap_keeps_last_cap() {
        let mut r = ByteRing::with_capacity(4);
        r.push(b"abcdefgh");
        assert_eq!(r.len(), 4);
        assert_eq!(r.recent(), b"efgh");
    }

    #[test]
    fn cap_boundary_exact() {
        let mut r = ByteRing::with_capacity(5);
        r.push(b"12345");
        assert_eq!(r.len(), 5);
        r.push(b"6");
        assert_eq!(r.len(), 5);
        assert_eq!(r.recent(), b"23456");
    }

    #[test]
    fn empty_and_zero_cap() {
        let fresh = ByteRing::with_capacity(8);
        assert!(fresh.is_empty());
        assert_eq!(fresh.len(), 0);

        let mut zero = ByteRing::with_capacity(0);
        zero.push(b"anything");
        assert_eq!(zero.len(), 0);
        assert!(zero.recent().is_empty());
        // total still counts bytes that were never retained.
        assert_eq!(zero.total_pushed(), 8);
    }

    #[test]
    fn evicted_is_zero_while_under_cap() {
        let mut r = ByteRing::with_capacity(100);
        assert_eq!(r.evicted(), 0); // fresh: nothing pushed, nothing evicted
        r.push(&[1u8; 30]);
        r.push(&[2u8; 40]);
        // 70 pushed, all retained → nothing dropped.
        assert_eq!(r.len(), 70);
        assert_eq!(r.evicted(), 0);
    }

    #[test]
    fn evicted_counts_dropped_oldest() {
        let mut r = ByteRing::with_capacity(10);
        r.push(b"0123456789"); // fills to the bound, 0 dropped
        assert_eq!(r.evicted(), 0);
        r.push(b"ABCDE"); // 15 pushed total, 10 retained → 5 dropped
        assert_eq!(r.total_pushed(), 15);
        assert_eq!(r.len(), 10);
        assert_eq!(r.evicted(), 5);
    }

    #[test]
    fn evicted_counts_tail_only_push_larger_than_cap() {
        let mut r = ByteRing::with_capacity(4);
        r.push(b"abcdefgh"); // 8 pushed, only last 4 retained → 4 dropped
        assert_eq!(r.evicted(), 4);
    }

    #[test]
    fn evicted_counts_all_when_zero_cap() {
        let mut r = ByteRing::with_capacity(0);
        r.push(b"anything"); // retains nothing → every pushed byte is evicted
        assert_eq!(r.len(), 0);
        assert_eq!(r.total_pushed(), 8);
        assert_eq!(r.evicted(), 8);
    }

    #[test]
    fn evicted_plus_len_equals_total_pushed_invariant() {
        let mut r = ByteRing::with_capacity(7);
        for n in [1usize, 3, 7, 2, 11, 0, 5, 6, 13, 4] {
            r.push(&vec![b'x'; n]);
            // the defining identity: nothing pushed is ever unaccounted for.
            assert_eq!(r.evicted() + r.len() as u64, r.total_pushed());
        }
    }

    #[test]
    fn len_never_exceeds_cap() {
        let mut r = ByteRing::with_capacity(7);
        // Varied push sizes, including ones larger than the cap.
        for n in [1usize, 3, 7, 2, 11, 0, 5, 6, 13, 4] {
            let chunk = vec![b'x'; n];
            r.push(&chunk);
            assert!(
                r.len() <= r.capacity(),
                "len {} exceeded cap {} after pushing {} bytes",
                r.len(),
                r.capacity(),
                n
            );
        }
    }
}
