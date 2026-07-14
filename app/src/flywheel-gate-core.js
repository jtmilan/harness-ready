// flywheel-gate-core — pure copy/decision helpers for the Flywheel modal's gate readout.
// No DOM, no Tauri. Fed the `delegate_gate_status` payload (fields: allow_mutations,
// delegate_live, flywheel_apply, flywheel_ship, autonomy_ceiling, in_flight, loop_autonomy).
// The apply/ship fields are OPTIONAL — an older backend omits them, and the copy must degrade
// gracefully to the report-only message rather than over-promising the autonomous loop.

// The subtitle under the Flywheel modal title. Repainted every gate refresh so it tracks the
// live build capability + arm state instead of the hard-coded "Phase 0" string it replaces.
export function flywheelPhaseCopy(gate) {
  // No gate, or an observe-only build → the live cycle can't run at all.
  if (!gate || !gate.delegate_live) {
    return "This build only observes — it can't run the live cycle. Install the delegate-live build to fix and open PRs.";
  }
  const REPORT_ONLY =
    "Read-only cycle: workers audit → synthesizer ground-truths → the merged tree is tested. " +
    "No code is changed; open the PR yourself from the result card.";
  // Live build, but the apply/ship flags are absent (older backend can't tell us) → report-only.
  if (gate.flywheel_apply === undefined) {
    return REPORT_ONLY;
  }
  if (!gate.flywheel_apply) {
    // Live + apply explicitly off → report-only.
    return REPORT_ONLY;
  }
  // apply is on from here.
  if (!gate.flywheel_ship) {
    return "Runs the full cycle and applies the fix to the merged tree, but stops before pushing — " +
      "open the PR yourself from the result card.";
  }
  // apply + ship both on → the full autonomous loop (still human-merge at the PR).
  return "Full loop: audit → fix → verify → auto-opens a PR for your review. Each cycle stops at the PR; you merge.";
}

// Trailing chips for the #fw-gate strip: apply/ship flags, only when the backend sends them.
// Presence-gated — undefined field renders nothing (backward-compat with older backends).
export function fwGateChips(g) {
  if (!g) return "";
  const ok = (b) => (b ? "✓" : "✗");
  const parts = [];
  if (g.flywheel_apply !== undefined) parts.push(`apply ${ok(!!g.flywheel_apply)}`);
  if (g.flywheel_ship !== undefined) parts.push(`ship ${ok(!!g.flywheel_ship)}`);
  return parts.length ? "   ·   " + parts.join(" · ") : "";
}
