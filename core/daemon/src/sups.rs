//! The daemon-owned live-pane map — the authoritative master-fd registry
//! (Phase 08 Sub-build 2 / 08-T4).
//!
//! This is `AppState.sups` relocated out of the GUI: a `Mutex<HashMap<id,
//! Supervisor>>` where each `Supervisor` owns one PTY master fd. Sub-build 1
//! shipped only a zero-field `SupsPlaceholder` whose `len()` fed the
//! idle-shutdown decision; Sub-build 2 replaces it with this real owner.
//!
//! ## Why generic over `V`
//!
//! A real [`supervisor::Supervisor`] cannot be constructed without spawning a PTY
//! child, so a unit test can't insert one. [`DaemonSups`] is therefore generic over
//! the stored value `V` (defaulting to `Supervisor`), letting the tests below
//! exercise insert / remove / with_mut / with_snapshot / live_ids / with_map against
//! a cheap stand-in while production code uses `DaemonSups` (== `DaemonSups<Supervisor>`).
//! The map mechanics are identical for any `V`, so the test double proves them.
//!
//! ## Lock discipline (the load-bearing invariant)
//!
//! Every accessor takes the single inner `Mutex`, does its O(1)/O(n) work, and
//! RELEASES it before returning — no guard escapes, so a caller can never hold this
//! lock across an `.await`, a subprocess spawn, or a `sleep`. The two closure
//! accessors ([`with_mut`](DaemonSups::with_mut) / [`with_snapshot`](DaemonSups::with_snapshot))
//! and the two whole-map escape hatches ([`with_map`](DaemonSups::with_map) /
//! [`with_map_mut`](DaemonSups::with_map_mut)) run the closure WHILE the lock is held,
//! so the GUI's existing lock-shedding patterns (clone a handle under the lock, drop
//! the lock, then do the slow work) are expressed by returning the handle FROM the
//! closure — exactly as `read_output_delta` already does. A poisoned lock is
//! recovered (`into_inner`) rather than panicking a command thread.
//!
//! ## Closures MUST NOT panic under the guard
//!
//! Because [`with_mut`](DaemonSups::with_mut) / [`with_map_mut`](DaemonSups::with_map_mut)
//! run the closure WHILE the inner `Mutex` guard is alive, a closure that PANICS
//! mid-mutation poisons the lock and can leave the map partially updated (e.g. a batch
//! role-set that set some panes and not others). `guard()` recovers the poison
//! (`into_inner`) so the next caller still gets the data, but a half-applied batch is
//! silent. Therefore closures passed to these accessors MUST be panic-free: prefer
//! returning a `Result` for any fallible step (as the write path does — `write_all`
//! errors map to `Err`, never an unwind). If a future closure needs a heavy allocation
//! (which can OOM-panic), build the data OUTSIDE the closure and pass it in.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use supervisor::Supervisor;

/// The daemon's live-pane map: workspace id → its live session value (a real
/// [`Supervisor`] owning the PTY master fd in production). Generic over `V` only so
/// the map mechanics are unit-testable with a stand-in (see the module note); the
/// production type alias is [`DaemonSups`] over `Supervisor`.
pub struct DaemonSups<V = Supervisor> {
    sups: Mutex<HashMap<String, V>>,
    /// Per-pane write-serialization locks (08 Sub-build 3 / MF-A + MF-E). The daemon's
    /// per-connection threads mean two same-user socket peers can issue concurrent
    /// `SendInput` to the SAME pane; the split-submit write is two PTY writes with an
    /// UNLOCKED settle between them, so without a per-pane guard the bodies/`\r`s of two
    /// writers can interleave and splice a task (A-body, B-body, A-`\r` submitting B's
    /// half-written line). [`write_lock`](DaemonSups::write_lock) hands out a STABLE
    /// per-id `Mutex` that `handle_split_write` holds across BOTH phases — while the
    /// `sups` map lock is STILL never held across the settle sleep. Keyed by id, lazily
    /// minted; a removed pane's stale entry is harmless (bounded by distinct ids seen).
    write_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl<V> Default for DaemonSups<V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<V> DaemonSups<V> {
    /// A new, empty live-pane map.
    pub fn new() -> Self {
        Self {
            sups: Mutex::new(HashMap::new()),
            write_locks: Mutex::new(HashMap::new()),
        }
    }

    /// The per-pane write-serialization lock for `id` (08 Sub-build 3 / MF-A). Lazily
    /// minted and STABLE per id, so every `SendInput` to the same pane contends on the
    /// SAME `Mutex<()>` — `handle_split_write` holds it across both phases of the
    /// split-submit write so two concurrent socket peers' body/`\r` writes can never
    /// interleave. The lock taken HERE is only the brief `write_locks` map lock; the
    /// returned `Arc<Mutex<()>>` is what the caller holds across the settle sleep (the
    /// per-pane `Supervisor` map lock is taken only transiently inside each phase, never
    /// across the sleep), so the map lock is never pinned by a sleeping writer.
    ///
    /// MEMORY BOUND (MF-A): this lazily INSERTS an entry for `id`, so the caller
    /// (`handle_split_write`) gates it behind a `contains(id)` existence check — an
    /// arbitrary peer-supplied unknown id must never leave a permanent entry. The map is
    /// therefore bounded by distinct REAL pane ids, not by attacker input.
    pub fn write_lock(&self, id: &str) -> Arc<Mutex<()>> {
        self.write_locks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(id.to_string())
            .or_default()
            .clone()
    }

    /// The number of per-pane write locks currently minted (MF-A memory-bound gauge).
    /// Used by the handlers tests to assert an unknown id never leaves a permanent
    /// `write_locks` entry; also a cheap operational count of distinct panes ever written.
    pub fn write_lock_count(&self) -> usize {
        self.write_locks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    /// Lock the inner map, recovering a poisoned lock rather than propagating the
    /// panic into a command thread (the data is plain `Supervisor`s — a prior panic
    /// while one was held leaves the MAP itself consistent).
    fn guard(&self) -> std::sync::MutexGuard<'_, HashMap<String, V>> {
        self.sups.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Insert a value at `id` (spawn). Returns the PREVIOUS value if the id was
    /// already present — the caller (kill-on-replace in `do_spawn`) must `kill()`
    /// the returned old `Supervisor` so a same-id respawn never orphans a live child.
    pub fn insert(&self, id: impl Into<String>, value: V) -> Option<V> {
        self.guard().insert(id.into(), value)
    }

    /// Remove and return the value at `id` (close). The caller `kill()`s it.
    pub fn remove(&self, id: &str) -> Option<V> {
        self.guard().remove(id)
    }

    /// `true` iff `id` is currently live (a cheap membership probe — `contains_key`
    /// at the call sites: admission `already_live`, focus/handoff parent checks, the
    /// loop-host reuse guard).
    pub fn contains(&self, id: &str) -> bool {
        self.guard().contains_key(id)
    }

    /// Run `f` against an IMMUTABLE borrow of the value at `id`, returning its result
    /// (or `None` if absent). The closure runs under the lock; to SHED the lock for
    /// slow work, return a cheap handle from the closure and operate on it after this
    /// call returns (the `output_handle()` clone-then-read pattern in
    /// `read_output_delta`).
    pub fn with_snapshot<F, R>(&self, id: &str, f: F) -> Option<R>
    where
        F: FnOnce(&V) -> R,
    {
        self.guard().get(id).map(f)
    }

    /// Run `f` against a MUTABLE borrow of the value at `id` (write / resize /
    /// is_alive / role set), returning its result (or `None` if absent). The closure
    /// runs under the lock and the lock is released when this returns — never hold it
    /// across the subsequent settle `sleep` (the caller does the sleep AFTER this).
    pub fn with_mut<F, R>(&self, id: &str, f: F) -> Option<R>
    where
        F: FnOnce(&mut V) -> R,
    {
        self.guard().get_mut(id).map(f)
    }

    /// Escape hatch for the iter-based read sites (`live_pane_ctxs`,
    /// `pane_contributors`, the orchestrate enrich): run `f` against the WHOLE map
    /// under ONE lock acquisition, exactly as those sites took `sups.lock()` once and
    /// iterated. Preserves their single-lock discipline without re-locking per id.
    pub fn with_map<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&HashMap<String, V>) -> R,
    {
        f(&self.guard())
    }

    /// Mutable whole-map escape hatch for the iter-MUT sites (`dead_pane_ids`'
    /// `iter_mut().filter_map(is_alive)`, the dead-sweep snapshot): one lock, then a
    /// `&mut HashMap` to the closure. The closure must NOT sleep/await while it holds
    /// the borrow — it returns owned data and the lock drops here.
    pub fn with_map_mut<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut HashMap<String, V>) -> R,
    {
        f(&mut self.guard())
    }

    /// Every live pane id (the registry keys) — the `keys().cloned().collect()`
    /// callers: admission's `working_count` live set, `list_workspaces`, `list_queue`,
    /// broadcast targets, the budget computation, the HUD timer poll.
    pub fn live_ids(&self) -> Vec<String> {
        self.guard().keys().cloned().collect()
    }

    /// The number of live panes — the count [`crate::lifecycle::idle_shutdown_decision`]
    /// consumes (0 ⇒ idle-shutdown-eligible after the grace; non-zero ⇒ hold open).
    /// This is the one contract the Sub-build-1 placeholder already honored.
    pub fn count_live(&self) -> usize {
        self.guard().len()
    }

    /// `true` when there are no live panes (idle).
    pub fn is_empty(&self) -> bool {
        self.guard().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lifecycle::{idle_shutdown_decision, ShutdownDecision};
    use std::time::Duration;

    /// A cheap stand-in for a live pane's per-session state. A real `Supervisor` owns
    /// a spawned PTY child and cannot be constructed in a unit test; the map mechanics
    /// (`DaemonSups`'s whole job) are identical for any `V`, so this double proves them.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct FakePane {
        harness: &'static str,
        alive: bool,
    }

    fn pane(h: &'static str) -> FakePane {
        FakePane {
            harness: h,
            alive: true,
        }
    }

    #[test]
    fn insert_returns_prior_and_never_double_counts() {
        let s: DaemonSups<FakePane> = DaemonSups::new();
        assert!(s.is_empty());
        assert_eq!(s.count_live(), 0);

        // first insert of a new id → no prior value
        assert_eq!(s.insert("ws-1", pane("claude")), None);
        assert_eq!(s.count_live(), 1);
        assert!(!s.is_empty());
        assert!(s.contains("ws-1"));

        // re-insert at the SAME id → returns the prior value, count stays 1 (the
        // kill-on-replace seam: the caller kills the returned old Supervisor).
        assert_eq!(s.insert("ws-1", pane("cursor")), Some(pane("claude")));
        assert_eq!(s.count_live(), 1, "re-insert must never double-count an id");

        // a distinct id is counted separately
        assert_eq!(s.insert("ws-2", pane("bash")), None);
        assert_eq!(s.count_live(), 2);
    }

    #[test]
    fn remove_returns_value_and_drops_count() {
        let s: DaemonSups<FakePane> = DaemonSups::new();
        s.insert("ws-1", pane("claude"));
        s.insert("ws-2", pane("cursor"));

        assert_eq!(s.remove("ws-1"), Some(pane("claude")));
        assert_eq!(s.count_live(), 1);
        assert!(!s.contains("ws-1"));
        assert!(s.contains("ws-2"));

        // removing an absent id is None and a no-op on the count
        assert_eq!(s.remove("nope"), None);
        assert_eq!(s.count_live(), 1);

        assert_eq!(s.remove("ws-2"), Some(pane("cursor")));
        assert!(s.is_empty());
    }

    #[test]
    fn with_snapshot_reads_present_and_misses_absent() {
        let s: DaemonSups<FakePane> = DaemonSups::new();
        s.insert("ws-1", pane("claude"));

        // present → the closure runs and its result is returned in Some
        assert_eq!(s.with_snapshot("ws-1", |p| p.harness), Some("claude"));
        // absent → closure NEVER runs, None returned
        assert_eq!(s.with_snapshot("ghost", |p| p.harness), None);
    }

    #[test]
    fn with_mut_mutates_in_place() {
        let s: DaemonSups<FakePane> = DaemonSups::new();
        s.insert("ws-1", pane("claude"));

        // mutate through the &mut borrow (mirrors set_pane_roles / write's &mut sup)
        let touched = s.with_mut("ws-1", |p| {
            p.alive = false;
            p.harness
        });
        assert_eq!(touched, Some("claude"));
        // the mutation persisted in the map
        assert_eq!(s.with_snapshot("ws-1", |p| p.alive), Some(false));
        // absent id → None, closure not run
        assert_eq!(s.with_mut("ghost", |p| p.alive = true), None);
    }

    #[test]
    fn with_map_and_map_mut_run_once_over_the_whole_map() {
        let s: DaemonSups<FakePane> = DaemonSups::new();
        s.insert(
            "ws-1",
            FakePane {
                harness: "claude",
                alive: true,
            },
        );
        s.insert(
            "ws-2",
            FakePane {
                harness: "cursor",
                alive: false,
            },
        );
        s.insert(
            "ws-3",
            FakePane {
                harness: "bash",
                alive: true,
            },
        );

        // read escape hatch: filter the whole map under one lock (live_pane_ctxs shape)
        let mut alive_ids: Vec<String> = s.with_map(|m| {
            m.iter()
                .filter(|(_, p)| p.alive)
                .map(|(id, _)| id.clone())
                .collect()
        });
        alive_ids.sort();
        assert_eq!(alive_ids, vec!["ws-1".to_string(), "ws-3".to_string()]);

        // mut escape hatch: the dead_pane_ids shape — iter_mut + collect the dead ids
        let mut dead: Vec<String> = s.with_map_mut(|m| {
            m.iter_mut()
                .filter_map(|(id, p)| (!p.alive).then(|| id.clone()))
                .collect()
        });
        dead.sort();
        assert_eq!(dead, vec!["ws-2".to_string()]);

        // mutations made THROUGH the &mut handle must persist past the closure. Flip a
        // field (the set_pane_roles shape) and remove an entry (the dead-pane retain
        // sweep) and assert the map state survives.
        s.with_map_mut(|m| {
            m.get_mut("ws-2").unwrap().alive = true;
        });
        assert_eq!(
            s.with_snapshot("ws-2", |p| p.alive),
            Some(true),
            "field mutation persisted"
        );

        // retain only live panes (now all three are alive) → no removals
        s.with_map_mut(|m| m.retain(|_, p| p.alive));
        assert_eq!(s.count_live(), 3, "all live → retain keeps all");

        // mark ws-1 dead, then retain → ws-1 is dropped through the &mut handle
        s.with_map_mut(|m| {
            m.get_mut("ws-1").unwrap().alive = false;
        });
        s.with_map_mut(|m| m.retain(|_, p| p.alive));
        assert_eq!(s.count_live(), 2, "dead-pane retain removed ws-1");
        assert!(
            !s.contains("ws-1"),
            "ws-1 removal persisted past the closure"
        );
    }

    #[test]
    fn live_ids_returns_every_key() {
        let s: DaemonSups<FakePane> = DaemonSups::new();
        s.insert("ws-1", pane("claude"));
        s.insert("ws-2", pane("cursor"));
        let mut ids = s.live_ids();
        ids.sort();
        assert_eq!(ids, vec!["ws-1".to_string(), "ws-2".to_string()]);
    }

    /// The Sub-build-1 contract carried forward: `count_live()` is the live-pane count
    /// the idle-shutdown decision consumes (empty ⇒ shutdown-eligible after the grace;
    /// non-empty ⇒ hold open), and it never double-counts a re-registered id.
    #[test]
    fn count_live_feeds_idle_shutdown_decision() {
        let grace = Duration::from_secs(60);
        let s: DaemonSups<FakePane> = DaemonSups::new();

        // empty ⇒ count 0 ⇒ at/after the grace the daemon is eligible to shut down
        assert_eq!(
            idle_shutdown_decision(s.count_live(), grace, grace, false),
            ShutdownDecision::Shutdown
        );

        // one live pane ⇒ count 1 ⇒ hold open regardless of elapsed / GUI attach
        s.insert("ws-1", pane("claude"));
        assert_eq!(
            idle_shutdown_decision(s.count_live(), grace, grace, false),
            ShutdownDecision::HoldOpen
        );

        // re-register the same id ⇒ count MUST stay 1 (else the daemon never reaches
        // 0 live panes → never shutdown-eligible)
        s.insert("ws-1", pane("cursor"));
        assert_eq!(s.count_live(), 1);
        assert_eq!(
            idle_shutdown_decision(s.count_live(), grace, grace, false),
            ShutdownDecision::HoldOpen
        );

        // close it ⇒ back to 0 ⇒ shutdown-eligible
        s.remove("ws-1");
        assert_eq!(
            idle_shutdown_decision(s.count_live(), grace, grace, false),
            ShutdownDecision::Shutdown
        );
    }
}
