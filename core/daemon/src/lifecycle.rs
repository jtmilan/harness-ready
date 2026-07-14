//! Idle-shutdown decision — the AC-4 core, as a PURE function.
//!
//! The single load-bearing invariant of the whole daemon design lives here:
//!
//! > **Idle = ZERO LIVE PANES, NOT "no GUI attached."**
//!
//! Quitting the GUI while an agent still runs must NOT shut the daemon down
//! (`08-01-PLAN` hard-problem #2 / AC-4) — that is the entire point of moving PTY
//! ownership out of the GUI. So the trigger is `live_panes == 0` sustained past a
//! grace window; `gui_attached` is threaded through ONLY to prove, in the type
//! signature and the tests, that it is *not* a trigger.
//!
//! No internal clock: every input is an argument, so the decision is deterministic
//! and unit-testable. A real driver computes `elapsed_idle` from a monotonic timer
//! and `live_panes` from [`crate::sups::DaemonSups::count_live`], then acts on the
//! returned [`ShutdownDecision`].

use std::time::Duration;

/// What the daemon should do at an idle-check tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownDecision {
    /// Stay running. Either a pane is live, or the zero-pane grace has not elapsed.
    HoldOpen,
    /// Self-exit (clean idle shutdown): zero live panes for at least the grace.
    Shutdown,
}

/// Decide whether the daemon should idle-shut-down right now.
///
/// Rules (AC-4):
/// * `live_panes >= 1` → [`ShutdownDecision::HoldOpen`] ALWAYS — regardless of
///   `elapsed_idle` and regardless of `gui_attached`. A live pane (even with the
///   GUI quit) keeps the daemon up; this is the "survive app quit while agents
///   run" guarantee.
/// * `live_panes == 0` → [`ShutdownDecision::Shutdown`] iff `elapsed_idle >=
///   grace`, else [`ShutdownDecision::HoldOpen`]. The boundary is inclusive:
///   `elapsed_idle == grace` shuts down.
///
/// `gui_attached` is intentionally NOT consulted: it exists in this signature to
/// document — and let the tests assert — that GUI presence is irrelevant to idle
/// shutdown. Idle is about live panes, never about whether a GUI is attached.
pub fn idle_shutdown_decision(
    live_panes: usize,
    elapsed_idle: Duration,
    grace: Duration,
    gui_attached: bool,
) -> ShutdownDecision {
    // `gui_attached` is deliberately unused: GUI presence must not influence the
    // decision (idle = zero live panes, NOT no-GUI). Naming it (rather than `_`)
    // keeps that invariant legible at the call site.
    let _ = gui_attached;

    if live_panes >= 1 {
        // Any live pane holds the daemon open, whatever the elapsed/GUI state.
        return ShutdownDecision::HoldOpen;
    }
    if elapsed_idle >= grace {
        ShutdownDecision::Shutdown
    } else {
        ShutdownDecision::HoldOpen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRACE: Duration = Duration::from_secs(60);

    #[test]
    fn live_panes_block_shutdown_regardless_of_gui() {
        // Way past the grace, but a pane is live → HOLD, both GUI states.
        let long = Duration::from_secs(10_000);
        assert_eq!(
            idle_shutdown_decision(1, long, GRACE, true),
            ShutdownDecision::HoldOpen
        );
        assert_eq!(
            idle_shutdown_decision(3, long, GRACE, false),
            ShutdownDecision::HoldOpen
        );
    }

    #[test]
    fn zero_panes_after_grace_shuts_down() {
        // Boundary: elapsed == grace → Shutdown (inclusive), and strictly past too.
        assert_eq!(
            idle_shutdown_decision(0, GRACE, GRACE, false),
            ShutdownDecision::Shutdown
        );
        assert_eq!(
            idle_shutdown_decision(0, GRACE + Duration::from_secs(1), GRACE, false),
            ShutdownDecision::Shutdown
        );
    }

    #[test]
    fn zero_panes_within_grace_holds() {
        // Boundary: one tick under the grace → still HoldOpen.
        assert_eq!(
            idle_shutdown_decision(0, GRACE - Duration::from_nanos(1), GRACE, false),
            ShutdownDecision::HoldOpen
        );
        assert_eq!(
            idle_shutdown_decision(0, Duration::ZERO, GRACE, false),
            ShutdownDecision::HoldOpen
        );
    }

    #[test]
    fn gui_attached_is_ignored_when_a_pane_is_live() {
        // The same (live pane, past grace) inputs give the SAME decision whether or
        // not a GUI is attached — proving gui_attached is not a trigger.
        let past = GRACE + Duration::from_secs(5);
        assert_eq!(
            idle_shutdown_decision(1, past, GRACE, true),
            idle_shutdown_decision(1, past, GRACE, false),
        );
        // And with zero panes past grace, GUI state still does not change it.
        assert_eq!(
            idle_shutdown_decision(0, past, GRACE, true),
            idle_shutdown_decision(0, past, GRACE, false),
        );
    }
}
