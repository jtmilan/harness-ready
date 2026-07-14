//! Idle-tick driver — the real-time side of the AC-4 idle-shutdown loop.
//!
//! [`lifecycle::idle_shutdown_decision`] is a **pure function** that takes the
//! current live-pane count and elapsed idle time and returns a verdict.  That
//! separation keeps the decision logic unit-testable without a clock or threads.
//! This module provides the **driver** that owns the clock and the tick interval
//! and calls the pure decision function on a regular schedule.
//!
//! ## Threading model
//!
//! [`IdleTicker::spawn`] starts a single background thread that ticks every
//! [`IdleTicker::tick_interval`]. On each tick it:
//!
//! 1. Reads the current live-pane count via the [`LivePaneCounter`] callback.
//! 2. Resets or advances the idle timer based on whether any pane is live.
//! 3. Calls [`lifecycle::idle_shutdown_decision`] with the elapsed idle time.
//! 4. If the verdict is [`ShutdownDecision::Shutdown`], invokes the
//!    `on_shutdown` callback (which in production calls `std::process::exit`).
//!
//! The driver thread exits cleanly when [`IdleTickHandle`] is dropped (via the
//! `stop` flag), making it safe to use in tests without OS-level cleanup.
//!
//! ## Testability
//!
//! Callers inject a `live_pane_count: impl Fn() -> usize + Send + 'static`
//! closure and an `on_shutdown: impl FnOnce() + Send + 'static` callback.
//! Tests substitute these with simple closures over shared atomics/channels,
//! so no real PTY or socket is needed.

use crate::lifecycle::{idle_shutdown_decision, ShutdownDecision};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Handle to the background idle-tick thread.  The thread is automatically
/// requested to stop when this handle is dropped.
pub struct IdleTickHandle {
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl IdleTickHandle {
    /// Signal the idle-tick thread to stop and wait for it to exit.  After
    /// this returns the `on_shutdown` callback will NOT be invoked (even if the
    /// daemon was about to shut down on the next tick).
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.join.take() {
            // Give the thread a moment to observe the flag; it checks once per
            // tick so we wait up to (tick_interval + ε).  In production this is
            // fine (shutdown is clean); in tests tick_interval is short.
            let _ = h.join();
        }
    }
}

impl Drop for IdleTickHandle {
    fn drop(&mut self) {
        // Signal the thread to stop.  If the handle is just dropped (not
        // explicitly `stop()`-ed), we signal but do not join — avoids blocking
        // the caller's destructor.
        self.stop.store(true, Ordering::Relaxed);
        // The join handle is intentionally NOT awaited here; the thread will
        // notice the flag on its next tick (≤ tick_interval) and exit.
    }
}

/// Configuration for the idle-tick driver.
#[derive(Debug, Clone)]
pub struct IdleTicker {
    /// How often to evaluate the idle-shutdown decision.  Shorter = more
    /// responsive shutdown; longer = lower CPU overhead.  In production the
    /// daemon uses ~5 seconds; tests use milliseconds.
    pub tick_interval: Duration,

    /// Once zero panes are live for this long, fire the `on_shutdown` callback.
    /// This is passed directly to [`idle_shutdown_decision`] as `grace`.
    pub grace: Duration,
}

impl Default for IdleTicker {
    /// Sensible production defaults: check every 5 s, shut down after 60 s idle.
    fn default() -> Self {
        Self {
            tick_interval: Duration::from_secs(5),
            grace: Duration::from_secs(60),
        }
    }
}

impl IdleTicker {
    /// Spawn the background idle-tick thread.
    ///
    /// # Arguments
    ///
    /// * `live_pane_count` — called on every tick to get the current live-pane
    ///   count.  In production this reads [`crate::sups::DaemonSups::count_live`].
    ///   Must be cheap and infallible.
    ///
    /// * `gui_attached` — called on every tick to determine whether a GUI
    ///   process is currently connected.  Per AC-4, this does NOT influence the
    ///   shutdown decision; it is passed through purely for diagnostic logging
    ///   and to make the invariant explicit in the call site.
    ///
    /// * `on_shutdown` — called ONCE when the idle-shutdown verdict fires.
    ///   In production: `|| std::process::exit(0)`.  In tests: signal a
    ///   channel / set an atomic.  Called from the tick thread, so it must not
    ///   block indefinitely.
    ///
    /// # Returns
    ///
    /// An [`IdleTickHandle`] whose `drop` (or explicit `stop`) signals the
    /// background thread to exit without firing `on_shutdown`.
    pub fn spawn<C, G, S>(
        self,
        live_pane_count: C,
        gui_attached: G,
        on_shutdown: S,
    ) -> IdleTickHandle
    where
        C: Fn() -> usize + Send + 'static,
        G: Fn() -> bool + Send + 'static,
        S: FnOnce() + Send + 'static,
    {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = Arc::clone(&stop);
        let tick_interval = self.tick_interval;
        let grace = self.grace;

        let join = std::thread::Builder::new()
            .name("daemon-idle-tick".into())
            .spawn(move || {
                // `idle_since` is Some(instant) when zero panes are live; reset to
                // None the moment any pane is registered.  We track it here rather
                // than using a Mutex<Instant> visible to tests so the pure decision
                // function never holds state.
                let mut idle_since: Option<Instant> = None;
                // Wrap on_shutdown in an Option so we can consume it exactly once.
                let mut on_shutdown = Some(on_shutdown);

                loop {
                    if stop_clone.load(Ordering::Relaxed) {
                        break;
                    }

                    let live = live_pane_count();
                    let attached = gui_attached();

                    // Advance or reset the idle timer.
                    if live == 0 {
                        idle_since.get_or_insert_with(Instant::now);
                    } else {
                        idle_since = None;
                    }

                    let elapsed_idle = idle_since.map(|t| t.elapsed()).unwrap_or(Duration::ZERO);

                    let verdict = idle_shutdown_decision(live, elapsed_idle, grace, attached);
                    if verdict == ShutdownDecision::Shutdown {
                        if let Some(cb) = on_shutdown.take() {
                            cb();
                        }
                        // After calling on_shutdown, exit the thread regardless of
                        // whether the callback is blocking or asynchronous.
                        break;
                    }

                    std::thread::sleep(tick_interval);
                }
            })
            .expect("failed to spawn daemon-idle-tick thread");

        IdleTickHandle {
            stop,
            join: Some(join),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;

    // Shared test tick configuration: very short intervals so tests are fast.
    fn test_ticker() -> IdleTicker {
        IdleTicker {
            tick_interval: Duration::from_millis(10),
            grace: Duration::from_millis(50),
        }
    }

    /// When zero panes are live and the grace elapses, `on_shutdown` is called.
    #[test]
    fn fires_shutdown_after_grace_with_zero_panes() {
        let (tx, rx) = mpsc::channel::<()>();
        let handle = test_ticker().spawn(
            || 0,     // always zero live panes
            || false, // GUI not attached (irrelevant per AC-4)
            move || {
                let _ = tx.send(());
            },
        );
        // Wait up to 2 s for the shutdown signal (tick=10ms + grace=50ms → fires ~60ms).
        rx.recv_timeout(Duration::from_secs(2))
            .expect("on_shutdown was never called within 2 s");
        handle.stop();
    }

    /// When a live pane is present the daemon MUST NOT shut down, even past the grace.
    #[test]
    fn does_not_fire_when_live_pane_present() {
        let shutdown_count = Arc::new(AtomicUsize::new(0));
        let count_clone = Arc::clone(&shutdown_count);

        let handle = test_ticker().spawn(
            || 1, // always one live pane → should never shut down
            || false,
            move || {
                count_clone.fetch_add(1, Ordering::SeqCst);
            },
        );

        // Wait long enough that the driver would have fired if the logic were wrong
        // (grace=50ms; wait 4× that).
        std::thread::sleep(Duration::from_millis(200));
        handle.stop();

        assert_eq!(
            shutdown_count.load(Ordering::SeqCst),
            0,
            "on_shutdown must NOT fire while a live pane is registered"
        );
    }

    /// `gui_attached` has NO effect on the shutdown decision (AC-4 invariant).
    /// The same configuration with gui_attached=true fires at the same time as false.
    #[test]
    fn gui_attached_does_not_delay_shutdown() {
        let (tx, rx) = mpsc::channel::<()>();
        // gui_attached = true — must still shut down at grace (zero panes).
        let handle = test_ticker().spawn(
            || 0,
            || true, // GUI is "attached" — must NOT prevent shutdown
            move || {
                let _ = tx.send(());
            },
        );
        rx.recv_timeout(Duration::from_secs(2))
            .expect("on_shutdown should fire even with GUI attached");
        handle.stop();
    }

    /// The tick thread exits cleanly when `stop()` is called before the grace elapses,
    /// without invoking `on_shutdown`.
    #[test]
    fn handle_stop_suppresses_shutdown() {
        let shutdown_fired = Arc::new(AtomicBool::new(false));
        let fired_clone = Arc::clone(&shutdown_fired);

        let handle = test_ticker().spawn(
            || 0,
            || false,
            move || {
                fired_clone.store(true, Ordering::SeqCst);
            },
        );

        // Stop immediately — before the grace could elapse.
        handle.stop();

        assert!(
            !shutdown_fired.load(Ordering::SeqCst),
            "on_shutdown must not fire when the handle is stopped before grace"
        );
    }

    /// Live panes → zero panes → grace elapses → shutdown fires.
    /// Proves the idle timer resets correctly when panes come and go.
    #[test]
    fn shutdown_fires_after_pane_leaves() {
        // Start with 1 pane, then switch to 0.
        let pane_count = Arc::new(AtomicUsize::new(1));
        let count_clone = Arc::clone(&pane_count);
        let (tx, rx) = mpsc::channel::<()>();

        let handle = test_ticker().spawn(
            move || count_clone.load(Ordering::SeqCst),
            || false,
            move || {
                let _ = tx.send(());
            },
        );

        // Let the driver tick a few times with a live pane (no shutdown).
        std::thread::sleep(Duration::from_millis(100));
        // Drop the pane — the idle timer starts now.
        pane_count.store(0, Ordering::SeqCst);

        // Should fire within grace+tick_interval after the pane drops.
        rx.recv_timeout(Duration::from_secs(2))
            .expect("on_shutdown must fire after pane count drops to zero and grace elapses");
        handle.stop();
    }

    /// `on_shutdown` is called AT MOST ONCE even if the ticker thread could fire again.
    /// (This is guaranteed by the `Option<S>` wrapper, but we test the observable
    /// behaviour from the outside to guard against future regressions.)
    #[test]
    fn on_shutdown_called_at_most_once() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_clone = Arc::clone(&calls);

        let ticker = IdleTicker {
            tick_interval: Duration::from_millis(5),
            grace: Duration::from_millis(20),
        };
        let (tx, rx) = mpsc::channel::<()>();

        let handle = ticker.spawn(
            || 0,
            || false,
            move || {
                calls_clone.fetch_add(1, Ordering::SeqCst);
                let _ = tx.send(());
            },
        );
        rx.recv_timeout(Duration::from_secs(2)).expect("shutdown");
        // Wait a bit more to check no second call arrives.
        std::thread::sleep(Duration::from_millis(50));
        handle.stop();

        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "on_shutdown must be called exactly once"
        );
    }
}
