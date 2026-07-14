//! Agent Teams — cross-harness state adapter core (Plan 01-01).
//!
//! Encodes the `core/hooks/SIGNALS.md` contract: a tagged raw hook event →
//! a normalized [`AgentState`] `{state, waiting_reason, needs_human}` (D4), plus
//! the queue [`rank`]ing ("who needs me").
//!
//! Core principle (D9): a `beforeShellExecution` / `PreToolUse` interception is
//! NOT a block on its own — it fires before every command. Whether it is a
//! "needs-you" wait is a function of the **policy decision** the injected writer
//! returned (`Allow` ⇒ routine; `Defer` ⇒ handed to the human via the native
//! prompt). The writer tags the event with that [`Decision`]; this core reads it.
//!
//! The core has no clock and does no I/O — event time is passed in — so it is
//! fully unit-testable offline (no external dependencies).

/// Per-workspace hook injection (Task 3).
pub mod inject;

/// events.jsonl reader + workspace discovery for the CLI (Task 4).
pub mod watch;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Harness {
    Claude,
    Cursor,
    /// State-blind agent harnesses — no native `SessionStart`/`Stop` hooks. The
    /// supervisor writes a SYNTHETIC `SessionStart` event at spawn (see supervisor
    /// `write_spawn_ready_event`) so the adapter sees them as `Working` the instant
    /// they spawn, instead of producing ZERO events and being invisible to the queue.
    Codex,
    CommandCode,
    OpenCode,
    Cline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Idle,
    Working,
    Waiting,
    Done,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitingReason {
    Approval,
    Question,
    TurnEnd,
    RateLimit,
}

/// The policy decision the injected writer made at an interception event (D9).
/// `Allow` = auto-allowed (allowlisted/safe) → routine. `Defer` = handed to the
/// human → the harness shows its native prompt (Model A).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Defer,
}

/// A raw hook event after the writer tagged it. `event` is the harness's own
/// event name, verbatim from SIGNALS.md. `decision` is present only for
/// interception events (`beforeShellExecution` / `preToolUse` / `PreToolUse`);
/// the writer always tags those.
#[derive(Debug, Clone)]
pub struct RawEvent {
    pub harness: Harness,
    pub event: String,
    pub decision: Option<Decision>,
    /// event time in unix millis (passed in by the caller — keeps the core testable)
    pub at: u64,
}

/// Normalized state of one workspace's agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentState {
    pub state: State,
    pub waiting_reason: Option<WaitingReason>,
    pub needs_human: bool,
    /// when this state began (unix millis) — drives the wait-time tie-break
    pub since: u64,
}

impl AgentState {
    fn make(state: State, reason: Option<WaitingReason>, since: u64) -> Self {
        AgentState {
            state,
            waiting_reason: reason,
            needs_human: needs_human(reason),
            since,
        }
    }
}

/// `needs_human` is true ONLY for a genuine human-blocking wait (approval or
/// question). `rate_limit` routes to the scheduler, not you; done / working /
/// error never block on you. (D4 / D9)
pub fn needs_human(reason: Option<WaitingReason>) -> bool {
    matches!(
        reason,
        Some(WaitingReason::Approval) | Some(WaitingReason::Question)
    )
}

/// Map a tagged raw event → normalized [`AgentState`], per SIGNALS.md.
pub fn normalize(ev: &RawEvent) -> AgentState {
    use Harness::*;
    use State::*;
    use WaitingReason::*;

    let (state, reason) = match (ev.harness, ev.event.as_str()) {
        // ---- claude (events confirmed against installed ~/.claude/settings.json + Vibeyard) ----
        (Claude, "SessionStart") => (Working, None),
        (Claude, "UserPromptSubmit") => (Working, None),
        // PreToolUse is an interception; only a `Defer` (permissionDecision: ask) is a block.
        (Claude, "PreToolUse") => match ev.decision {
            Some(Decision::Defer) => (Waiting, Some(Approval)),
            _ => (Working, None),
        },
        (Claude, "PostToolUse") => (Working, None),
        // THE BLOCK — Vibeyard maps PermissionRequest → `input` (needs you).
        (Claude, "PermissionRequest") => (Waiting, Some(Approval)),
        (Claude, "PermissionDenied") => (Working, None),
        // Soft needs-input signal; secondary to PermissionRequest.
        (Claude, "Notification") => (Waiting, Some(Question)),
        // Stop = turn-done, NOT blocked (D4).
        (Claude, "Stop") => (Done, Some(TurnEnd)),
        (Claude, "StopFailure") => (Error, None),
        (Claude, "SubagentStop") => (Working, None),

        // ---- cursor (spike-observed events + by-design flag semantics) ----
        (Cursor, "sessionStart") => (Working, None),
        // Interception: Defer ⇒ native prompt = the block; Allow ⇒ routine (no false positive).
        // None defaults to routine (the writer always tags; this guards against flicker).
        (Cursor, "beforeShellExecution") | (Cursor, "preToolUse") => match ev.decision {
            Some(Decision::Defer) => (Waiting, Some(Approval)),
            _ => (Working, None),
        },
        (Cursor, "afterShellExecution") => (Working, None),
        (Cursor, "stop") => (Done, Some(TurnEnd)),
        // Not a hook — surfaced via stderr by a Phase-02 supervisor; needs_human=false.
        (Cursor, "resource_exhausted") => (Waiting, Some(RateLimit)),

        // ---- codex / commandcode / opencode / cline: state-blind harnesses ----
        // No native SessionStart hook, so the supervisor writes a SYNTHETIC one at
        // spawn (write_spawn_ready_event) → Working, the SAME baseline claude/cursor
        // get from their real SessionStart. codex's only turn-end signal is `notify`
        // (its global config hook) → Done; commandcode has no turn-end hook yet, so it
        // stays Working until one lands (the catch-all keeps any other event Working —
        // never a false block).
        (Codex, "SessionStart")
        | (CommandCode, "SessionStart")
        | (OpenCode, "SessionStart")
        | (Cline, "SessionStart") => (Working, None),
        (Codex, "notify") => (Done, Some(TurnEnd)),
        // opencode's turn-end: its plugin (opencode-state-plugin.js) writes `stop` on
        // `session.idle`. cline has NO hook/plugin surface today (inject:None, no emitter),
        // so in production it behaves like commandcode — it stays Working after its
        // synthetic SessionStart and never reaches Done (its headless turn-end is
        // UNVERIFIED). The `(Cline, "stop")` arm is FORWARD-DEFENSE only: if cline ever
        // grows a turn-end emitter, it maps correctly here without a code change.
        (OpenCode, "stop") | (Cline, "stop") => (Done, Some(TurnEnd)),

        // ---- unknown event: assume the agent is working, never a false block ----
        _ => (Working, None),
    };

    AgentState::make(state, reason, ev.at)
}

/// Priority for the ranked queue: approval > question > turn_end > rate_limit > none.
fn reason_priority(reason: Option<WaitingReason>) -> u8 {
    match reason {
        Some(WaitingReason::Approval) => 0,
        Some(WaitingReason::Question) => 1,
        Some(WaitingReason::TurnEnd) => 2,
        Some(WaitingReason::RateLimit) => 3,
        None => 4,
    }
}

/// Rank workspaces for "who needs me" (AC-4): `needs_human` first, then
/// approval > question > turn_end, tie-broken by longest wait (earliest `since`).
/// Stable; returns a new ordered Vec.
pub fn rank<T: Clone>(items: &[(T, AgentState)]) -> Vec<(T, AgentState)> {
    let mut v: Vec<(T, AgentState)> = items.to_vec();
    v.sort_by(|a, b| {
        let (sa, sb) = (a.1, b.1);
        sb.needs_human
            .cmp(&sa.needs_human) // true first
            .then(reason_priority(sa.waiting_reason).cmp(&reason_priority(sb.waiting_reason)))
            .then(sa.since.cmp(&sb.since)) // earliest start = longest wait first
    });
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    // ----- deterministic, dependency-free fuzzer (hand-rolled xorshift) -----
    // No rand/proptest crate (repo convention). Same seed every run ⇒ failures
    // are reproducible.
    struct Lcg(u64);
    impl Lcg {
        fn new() -> Self {
            Lcg(0x2545_F491_4F6C_DD1D)
        }
        // Named `next_u64`, not `next`, to dodge clippy::should_implement_trait.
        fn next_u64(&mut self) -> u64 {
            let mut s = self.0;
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            self.0 = s;
            s
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next_u64() % n
        }
    }

    const REASONS: [Option<WaitingReason>; 5] = [
        None,
        Some(WaitingReason::Approval),
        Some(WaitingReason::Question),
        Some(WaitingReason::TurnEnd),
        Some(WaitingReason::RateLimit),
    ];
    const STATES: [State; 5] = [
        State::Idle,
        State::Working,
        State::Waiting,
        State::Done,
        State::Error,
    ];

    /// The ranking-priority spec, re-encoded independently of the (private)
    /// `reason_priority`: approval < question < turn_end < rate_limit < none.
    /// The whole point is to pin the spec from the test side, so we never call
    /// the production fn here.
    fn prio(r: Option<WaitingReason>) -> u8 {
        match r {
            Some(WaitingReason::Approval) => 0,
            Some(WaitingReason::Question) => 1,
            Some(WaitingReason::TurnEnd) => 2,
            Some(WaitingReason::RateLimit) => 3,
            None => 4,
        }
    }

    fn skey(s: State) -> u8 {
        match s {
            State::Idle => 0,
            State::Working => 1,
            State::Waiting => 2,
            State::Done => 3,
            State::Error => 4,
        }
    }

    /// Total order over an element, used for multiset (permutation) comparison.
    fn elem_key(item: &(usize, AgentState)) -> (usize, u8, u8, u8, u64) {
        let s = item.1;
        (
            item.0,
            skey(s.state),
            prio(s.waiting_reason),
            s.needs_human as u8,
            s.since,
        )
    }

    /// The full rank sort key, re-encoded test-side: needs_human first (true
    /// before false), then priority ascending, then `since` ascending.
    fn rank_key(s: AgentState) -> (u8, u8, u64) {
        ((!s.needs_human) as u8, prio(s.waiting_reason), s.since)
    }

    fn gen_state(rng: &mut Lcg) -> AgentState {
        let state = STATES[rng.below(STATES.len() as u64) as usize];
        let reason = REASONS[rng.below(REASONS.len() as u64) as usize];
        let since = rng.below(8); // small range ⇒ frequent `since` ties
        AgentState {
            state,
            waiting_reason: reason,
            needs_human: needs_human(reason),
            since,
        }
    }

    fn gen_items(rng: &mut Lcg, n: usize) -> Vec<(usize, AgentState)> {
        (0..n).map(|i| (i, gen_state(rng))).collect()
    }

    fn ev(harness: Harness, event: &str, decision: Option<Decision>) -> RawEvent {
        RawEvent {
            harness,
            event: event.to_string(),
            decision,
            at: 0,
        }
    }

    // ------------------------------- rank -----------------------------------

    #[test]
    fn rank_is_a_permutation() {
        let mut rng = Lcg::new();
        for _ in 0..256 {
            let n = rng.below(12) as usize;
            let items = gen_items(&mut rng, n);
            let ranked = rank(&items);
            let mut before: Vec<_> = items.iter().map(elem_key).collect();
            let mut after: Vec<_> = ranked.iter().map(elem_key).collect();
            before.sort_unstable();
            after.sort_unstable();
            assert_eq!(before, after, "rank changed the multiset");
        }
    }

    #[test]
    fn rank_needs_human_strictly_first() {
        let mut rng = Lcg::new();
        for _ in 0..256 {
            let n = rng.below(16) as usize;
            let items = gen_items(&mut rng, n);
            let ranked = rank(&items);
            for w in ranked.windows(2) {
                // A non-needs row must never be followed by a needs row.
                assert!(
                    w[0].1.needs_human || !w[1].1.needs_human,
                    "needs_human=true followed a false: {:?} -> {:?}",
                    w[0].1,
                    w[1].1
                );
            }
        }
    }

    #[test]
    fn rank_priority_then_since_monotonic() {
        let mut rng = Lcg::new();
        for _ in 0..256 {
            let n = rng.below(16) as usize;
            let items = gen_items(&mut rng, n);
            let ranked = rank(&items);
            for w in ranked.windows(2) {
                // Adjacent rows are non-decreasing on (needs_human desc,
                // priority asc, since asc) — the encoded spec.
                assert!(
                    rank_key(w[0].1) <= rank_key(w[1].1),
                    "non-monotonic order: {:?} then {:?}",
                    w[0].1,
                    w[1].1
                );
            }
        }
    }

    #[test]
    fn rank_idempotent() {
        let mut rng = Lcg::new();
        for _ in 0..256 {
            let n = rng.below(16) as usize;
            let items = gen_items(&mut rng, n);
            let once = rank(&items);
            let twice = rank(&once);
            let a: Vec<_> = once.iter().map(elem_key).collect();
            let b: Vec<_> = twice.iter().map(elem_key).collect();
            assert_eq!(a, b, "rank(rank(x)) reordered rank(x)");
        }
    }

    #[test]
    fn rank_stable_on_equal_keys() {
        // Tiny key space (5 reasons × 4 `since`) over many items ⇒ heavy
        // collisions, so equal-key runs are common. Payload = input index;
        // a stable sort must keep each run in ascending index order.
        let mut rng = Lcg::new();
        let items: Vec<(usize, AgentState)> = (0..400)
            .map(|i| {
                let reason = REASONS[rng.below(REASONS.len() as u64) as usize];
                let since = rng.below(4);
                (
                    i,
                    AgentState {
                        state: State::Working,
                        waiting_reason: reason,
                        needs_human: needs_human(reason),
                        since,
                    },
                )
            })
            .collect();
        let ranked = rank(&items);
        for w in ranked.windows(2) {
            if rank_key(w[0].1) == rank_key(w[1].1) {
                assert!(
                    w[0].0 < w[1].0,
                    "stability broken: index {} preceded {} on an equal key",
                    w[0].0,
                    w[1].0
                );
            }
        }
    }

    // ---------------------------- needs_human -------------------------------

    #[test]
    fn needs_human_truth_table() {
        assert!(needs_human(Some(WaitingReason::Approval)));
        assert!(needs_human(Some(WaitingReason::Question)));
        assert!(!needs_human(Some(WaitingReason::TurnEnd)));
        assert!(!needs_human(Some(WaitingReason::RateLimit)));
        assert!(!needs_human(None));
    }

    // ----------------------------- normalize --------------------------------

    #[test]
    fn normalize_never_panics_over_matrix() {
        let harnesses = [
            Harness::Claude,
            Harness::Cursor,
            Harness::Codex,
            Harness::CommandCode,
            Harness::OpenCode,
            Harness::Cline,
        ];
        let events = [
            // claude
            "SessionStart",
            "UserPromptSubmit",
            "PreToolUse",
            "PostToolUse",
            "PermissionRequest",
            "PermissionDenied",
            "Notification",
            "Stop",
            "StopFailure",
            "SubagentStop",
            // cursor
            "sessionStart",
            "beforeShellExecution",
            "preToolUse",
            "afterShellExecution",
            "stop",
            "resource_exhausted",
            // unknown / cross-harness / edge strings
            "",
            "unknown_event",
            "pretooluse",
            "PRETOOLUSE",
            "Stop ",
        ];
        let decisions = [None, Some(Decision::Allow), Some(Decision::Defer)];
        for &h in &harnesses {
            for &e in &events {
                for &d in &decisions {
                    let st = normalize(&ev(h, e, d));
                    // Core invariant: the cached `needs_human` flag always
                    // agrees with the pure fn over the resolved reason.
                    assert_eq!(
                        st.needs_human,
                        needs_human(st.waiting_reason),
                        "needs_human/reason disagree for {:?} {:?} {:?}",
                        h,
                        e,
                        d
                    );
                }
            }
        }
    }

    #[test]
    fn normalize_unknown_event_is_working_never_blocks() {
        // D9: an unrecognized event assumes the agent is working and never a
        // false block — for either harness and any decision tag.
        let unknowns = [
            "",
            "totally_unknown",
            "pretooluse",
            "PRETOOLUSE",
            "permissionrequest",
            "Stop ",
        ];
        for &h in &[
            Harness::Claude,
            Harness::Cursor,
            Harness::Codex,
            Harness::CommandCode,
            Harness::OpenCode,
            Harness::Cline,
        ] {
            for &e in &unknowns {
                for &d in &[None, Some(Decision::Allow), Some(Decision::Defer)] {
                    let st = normalize(&ev(h, e, d));
                    assert_eq!(st.state, State::Working, "{:?} {:?} {:?}", h, e, d);
                    assert_eq!(st.waiting_reason, None, "{:?} {:?} {:?}", h, e, d);
                    assert!(!st.needs_human, "{:?} {:?} {:?}", h, e, d);
                }
            }
        }
    }

    #[test]
    fn normalize_ratelimit_not_needs_human() {
        // D33: Cursor resource exhaustion is a wait that routes to the
        // scheduler, NOT to the human.
        let st = normalize(&ev(Harness::Cursor, "resource_exhausted", None));
        assert_eq!(st.state, State::Waiting);
        assert_eq!(st.waiting_reason, Some(WaitingReason::RateLimit));
        assert!(!st.needs_human);
    }

    #[test]
    fn state_blind_harness_synthetic_sessionstart_is_working_visible() {
        // The dogfood fix: codex/commandcode/opencode have no native SessionStart hook,
        // so the supervisor writes a SYNTHETIC one at spawn (write_spawn_ready_event).
        // The adapter must recognize these harnesses and surface them as Working — the
        // same baseline claude/cursor get — instead of dropping them / blocking.
        for h in [
            Harness::Codex,
            Harness::CommandCode,
            Harness::OpenCode,
            Harness::Cline,
        ] {
            let st = normalize(&ev(h, "SessionStart", None));
            assert_eq!(st.state, State::Working, "{h:?} SessionStart → Working");
            assert_eq!(st.waiting_reason, None, "{h:?} SessionStart has no reason");
            assert!(!st.needs_human, "{h:?} SessionStart never needs human");
        }
        // codex's only turn-end signal (its `notify` hook) → Done/TurnEnd, not a block.
        let done = normalize(&ev(Harness::Codex, "notify", None));
        assert_eq!(done.state, State::Done);
        assert_eq!(done.waiting_reason, Some(WaitingReason::TurnEnd));
        assert!(!done.needs_human);
        // opencode turn-end: its plugin writes `stop` on session.idle → Done/TurnEnd.
        let oc = normalize(&ev(Harness::OpenCode, "stop", None));
        assert_eq!(oc.state, State::Done);
        assert_eq!(oc.waiting_reason, Some(WaitingReason::TurnEnd));
        assert!(!oc.needs_human);
        // cline turn-end: same synthetic `stop` → Done/TurnEnd treatment as opencode.
        let cl = normalize(&ev(Harness::Cline, "stop", None));
        assert_eq!(cl.state, State::Done);
        assert_eq!(cl.waiting_reason, Some(WaitingReason::TurnEnd));
        assert!(!cl.needs_human);
    }

    #[test]
    fn normalize_interception_defer_vs_allow() {
        // D9: an interception is a block only when the writer's policy DEFERS;
        // Allow (or an untagged None) is routine.
        let interceptions = [
            (Harness::Claude, "PreToolUse"),
            (Harness::Cursor, "beforeShellExecution"),
            (Harness::Cursor, "preToolUse"),
        ];
        for &(h, e) in &interceptions {
            let deferred = normalize(&ev(h, e, Some(Decision::Defer)));
            assert_eq!(deferred.state, State::Waiting, "{:?} {:?}", h, e);
            assert_eq!(deferred.waiting_reason, Some(WaitingReason::Approval));
            assert!(
                deferred.needs_human,
                "defer must need human: {:?} {:?}",
                h, e
            );

            for d in [Some(Decision::Allow), None] {
                let routine = normalize(&ev(h, e, d));
                assert_eq!(routine.state, State::Working, "{:?} {:?} {:?}", h, e, d);
                assert_eq!(routine.waiting_reason, None, "{:?} {:?} {:?}", h, e, d);
                assert!(!routine.needs_human, "{:?} {:?} {:?}", h, e, d);
            }
        }
    }
}
