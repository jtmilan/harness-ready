//! AC-1 (per-state normalization) + AC-4 (ranking) coverage, incl. the
//! no-false-positive case and the two explicit non-cases. Dependency-free.

use state_adapter::*;

fn ev(h: Harness, e: &str, d: Option<Decision>) -> RawEvent {
    RawEvent {
        harness: h,
        event: e.to_string(),
        decision: d,
        at: 0,
    }
}

#[test]
fn cursor_defer_is_a_block_allow_is_not() {
    // defer ⇒ genuine block
    let s = normalize(&ev(
        Harness::Cursor,
        "beforeShellExecution",
        Some(Decision::Defer),
    ));
    assert_eq!(s.state, State::Waiting);
    assert_eq!(s.waiting_reason, Some(WaitingReason::Approval));
    assert!(s.needs_human);

    // allow ⇒ routine, NOT a block (no false-positive flicker — D9)
    let s = normalize(&ev(
        Harness::Cursor,
        "beforeShellExecution",
        Some(Decision::Allow),
    ));
    assert_eq!(s.state, State::Working);
    assert!(!s.needs_human);

    // preToolUse behaves the same as beforeShellExecution
    let s = normalize(&ev(Harness::Cursor, "preToolUse", Some(Decision::Defer)));
    assert!(s.needs_human);
}

// PROPERTY test (crate is dependency-free → hand-rolled deterministic LCG): over many
// generated item-sets, rank must (1) be a permutation — never drop or duplicate an
// item, and (2) partition needs_human=true strictly before needs_human=false (the core
// who-needs-me invariant). The example tests pin ONE input; this exercises the
// comparator across arbitrary mixes the examples never hit.
#[test]
fn rank_is_a_permutation_and_partitions_needs_human() {
    let reasons = [
        None,
        Some(WaitingReason::Approval),
        Some(WaitingReason::Question),
        Some(WaitingReason::TurnEnd),
        Some(WaitingReason::RateLimit),
    ];
    let states = [
        State::Idle,
        State::Working,
        State::Waiting,
        State::Done,
        State::Error,
    ];

    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let rng = |s: &mut u64| -> u32 {
        *s = s
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (*s >> 33) as u32
    };

    for _trial in 0..256 {
        let n = (rng(&mut seed) % 9) as usize; // 0..=8 items, incl. the empty set
        let items: Vec<(usize, AgentState)> = (0..n)
            .map(|i| {
                let st = AgentState {
                    state: states[(rng(&mut seed) as usize) % states.len()],
                    waiting_reason: reasons[(rng(&mut seed) as usize) % reasons.len()],
                    needs_human: rng(&mut seed) % 2 == 0,
                    since: (rng(&mut seed) % 1000) as u64,
                };
                (i, st)
            })
            .collect();

        let ranked = rank(&items);

        // (1) permutation — same key multiset, nothing dropped or duplicated.
        let mut in_keys: Vec<usize> = items.iter().map(|(k, _)| *k).collect();
        let mut out_keys: Vec<usize> = ranked.iter().map(|(k, _)| *k).collect();
        in_keys.sort_unstable();
        out_keys.sort_unstable();
        assert_eq!(
            in_keys, out_keys,
            "rank must be a permutation (trial n={n})"
        );

        // (2) partition — once a non-needs_human row appears, no needs_human row follows.
        let mut seen_non_needs = false;
        for (_, st) in &ranked {
            if st.needs_human {
                assert!(
                    !seen_non_needs,
                    "a needs_human row ranked AFTER a non-needs_human one"
                );
            } else {
                seen_non_needs = true;
            }
        }
    }
}

// PROPERTY/fuzz: arbitrary event strings × harness × decision must never panic, must
// keep needs_human consistent with the reason, and an UNRECOGNIZED event must NEVER
// produce a needs_human block (no false-positive queue signal — D9). The matrix test
// pins a fixed alphabet; this fuzzes random strings.
#[test]
fn normalize_never_panics_and_never_false_blocks_on_arbitrary_events() {
    // every recognized event token (any harness) — a generated string equal to one of
    // these is exempt from the "must not block" assertion (it's a real event).
    let known = [
        "Notification",
        "PermissionRequest",
        "Stop",
        "StopFailure",
        "SubagentStop",
        "PermissionDenied",
        "beforeShellExecution",
        "beforeMCPExecution",
        "preToolUse",
        "stop",
        "sessionStart",
        "resource_exhausted",
        "afterShellExecution",
    ];
    let harnesses = [Harness::Claude, Harness::Cursor];
    let decisions = [None, Some(Decision::Allow), Some(Decision::Defer)];

    let mut seed: u64 = 0xD1B5_4A32_D192_ED03;
    let rng = |s: &mut u64| -> u32 {
        *s = s
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (*s >> 33) as u32
    };

    for _ in 0..512 {
        let len = (rng(&mut seed) % 12) as usize;
        let ev_str: String = (0..len)
            .map(|_| (b'a' + (rng(&mut seed) % 26) as u8) as char)
            .collect();
        let h = harnesses[(rng(&mut seed) as usize) % harnesses.len()];
        let d = decisions[(rng(&mut seed) as usize) % decisions.len()];

        let s = normalize(&ev(h, &ev_str, d)); // must not panic
                                               // internal consistency: needs_human is exactly needs_human(reason).
        assert_eq!(
            s.needs_human,
            needs_human(s.waiting_reason),
            "inconsistent needs_human for {ev_str:?}"
        );
        // an unrecognized event must never light the who-needs-you queue.
        if !known.contains(&ev_str.as_str()) {
            assert!(
                !s.needs_human,
                "unrecognized event {ev_str:?} must not false-block"
            );
        }
    }
}

#[test]
fn cursor_lifecycle() {
    assert_eq!(
        normalize(&ev(Harness::Cursor, "sessionStart", None)).state,
        State::Working
    );
    assert_eq!(
        normalize(&ev(Harness::Cursor, "afterShellExecution", None)).state,
        State::Working
    );
    let stop = normalize(&ev(Harness::Cursor, "stop", None));
    assert_eq!(stop.state, State::Done);
    assert_eq!(stop.waiting_reason, Some(WaitingReason::TurnEnd));
    assert!(!stop.needs_human, "clean stop is done, not a block");
}

#[test]
fn rate_limit_is_not_needs_human() {
    let s = normalize(&ev(Harness::Cursor, "resource_exhausted", None));
    assert_eq!(s.state, State::Waiting);
    assert_eq!(s.waiting_reason, Some(WaitingReason::RateLimit));
    assert!(
        !s.needs_human,
        "rate limit routes to the scheduler, not you"
    );
}

#[test]
fn claude_permission_request_is_the_block() {
    let s = normalize(&ev(Harness::Claude, "PermissionRequest", None));
    assert_eq!(s.state, State::Waiting);
    assert_eq!(s.waiting_reason, Some(WaitingReason::Approval));
    assert!(
        s.needs_human,
        "PermissionRequest = claude awaiting your approval"
    );
}

#[test]
fn claude_stop_is_never_a_block() {
    let s = normalize(&ev(Harness::Claude, "Stop", None));
    assert_eq!(s.state, State::Done);
    assert_eq!(s.waiting_reason, Some(WaitingReason::TurnEnd));
    assert!(!s.needs_human, "Stop = turn-done, never a block (D4)");
}

#[test]
fn claude_routine_events_are_working() {
    for e in [
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
    ] {
        let s = normalize(&ev(Harness::Claude, e, None));
        assert_eq!(
            s.state,
            State::Working,
            "{e} should be working, not a block"
        );
        assert!(!s.needs_human);
    }
}

#[test]
fn ranking_needs_human_then_reason_then_wait() {
    let approval_new = AgentState {
        state: State::Waiting,
        waiting_reason: Some(WaitingReason::Approval),
        needs_human: true,
        since: 100,
    };
    let approval_old = AgentState {
        since: 50,
        ..approval_new
    };
    let question = AgentState {
        state: State::Waiting,
        waiting_reason: Some(WaitingReason::Question),
        needs_human: true,
        since: 10, // small wait, but still below approvals (reason breaks the tie first)
    };
    let turn_end = AgentState {
        state: State::Done,
        waiting_reason: Some(WaitingReason::TurnEnd),
        needs_human: false,
        since: 1,
    };
    let working = AgentState {
        state: State::Working,
        waiting_reason: None,
        needs_human: false,
        since: 1,
    };

    let items = vec![
        ("working", working),
        ("turn_end", turn_end),
        ("question", question),
        ("approval_new", approval_new),
        ("approval_old", approval_old),
    ];
    let order: Vec<&str> = rank(&items).into_iter().map(|(k, _)| k).collect();
    assert_eq!(
        order,
        vec![
            "approval_old",
            "approval_new",
            "question",
            "turn_end",
            "working"
        ]
    );
}

// Pin each behaviorally-meaningful claude event→state arm directly (the in-crate
// matrix test only asserts the needs_human==needs_human(reason) tautology, which
// holds for ANY mapping — so a remap of these arms would silently corrupt the queue).

#[test]
fn claude_notification_is_a_question_block() {
    // the SOLE producer of WaitingReason::Question — a needs_human signal.
    let s = normalize(&ev(Harness::Claude, "Notification", None));
    assert_eq!(s.state, State::Waiting);
    assert_eq!(s.waiting_reason, Some(WaitingReason::Question));
    assert!(
        s.needs_human,
        "Notification = claude soft needs-input signal"
    );
}

#[test]
fn claude_stop_failure_is_error_not_done() {
    // the SOLE producer of State::Error — distinct from a clean Stop→Done.
    let s = normalize(&ev(Harness::Claude, "StopFailure", None));
    assert_eq!(s.state, State::Error);
    assert_eq!(s.waiting_reason, None);
    assert!(!s.needs_human, "an error is not a human block");
}

#[test]
fn claude_subagent_stop_stays_working_not_turn_end() {
    // a distinct arm — must NOT be mis-mapped to Done/TurnEnd like the sibling "Stop".
    let s = normalize(&ev(Harness::Claude, "SubagentStop", None));
    assert_eq!(s.state, State::Working);
    assert_eq!(s.waiting_reason, None);
    assert!(!s.needs_human);
}

#[test]
fn claude_permission_denied_resumes_working_not_a_block() {
    // 'denied' explicitly does NOT block (distinct from PermissionRequest→Approval).
    let s = normalize(&ev(Harness::Claude, "PermissionDenied", None));
    assert_eq!(s.state, State::Working);
    assert!(
        !s.needs_human,
        "a denied permission resumes work, it does not block"
    );
}
