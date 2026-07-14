// state-blind-core — pure decision for the pane-header "status not reported" badge.
//
// Some harnesses are STATE-BLIND: their headless/TUI panes emit no machine-readable state
// signal (inject:None), so the poller can never learn whether they're working, waiting, or
// idle — the pane header just shows "Working" forever. That's misleading: it reads as a live
// status when it's really "unknown / unreported". This module decides, purely from the pane's
// harness, whether to paint a small header badge that says so. No DOM, no globals — main.js
// owns the rendering; this owns the decision so it can be unit-tested.
//
// Keep this set in sync with the backend harness descriptors' inject policy: codex /
// commandcode / opencode / cline are inject:None (state-blind); claude / cursor / gemini
// report state and are NOT listed here.
export const STATE_BLIND = new Set(["codex", "commandcode", "opencode", "cline"]);

export const STATE_BLIND_BADGE_LABEL = "status not reported";

// True when this harness cannot report pane state (case-insensitive, tolerant of null/blank).
export function isStateBlind(harness) {
  return STATE_BLIND.has(String(harness || "").trim().toLowerCase());
}

// Badge descriptor for a pane's harness, or null when the harness DOES report state (no badge).
//   { label, title } — label = the short chip text; title = the fuller hover/aria explanation.
export function stateBlindBadge(harness) {
  if (!isStateBlind(harness)) return null;
  const h = String(harness || "").trim();
  return {
    label: STATE_BLIND_BADGE_LABEL,
    title:
      "This harness" + (h ? " (" + h + ")" : "") +
      " does not report its state — the pane may read “Working” even when it is idle or waiting.",
  };
}
