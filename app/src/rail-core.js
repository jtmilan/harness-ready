// Agent Teams — ranked "who needs you" rail: PURE reconcile core (10-01).
//
// The DOM + WAAPI motion live in main.js (`renderRail`); this is the PURE set-math
// that drives it, keyed by pane id, so it is unit-testable in node with no DOM —
// the same pure-module split as wizard-core.js / presets.js.
//
// NOTE (honest scope): these functions prove WHICH rows enter/exit/move + the meta
// string. They say NOTHING about the motion itself (FLIP geometry, fades, the amber
// pulse, the reduced-motion gate) — that correctness is GUI-verified only.

// The meta line under a pane id: "<harness> <state>" + " · <reason>" (reason "-" or
// empty is omitted). Verbatim from renderRail's prior inline string — extracted so it
// is testable. Pure + total.
export function railMeta(r) {
  const base = `${r.harness} ${r.state}`;
  const withReason = r.reason && r.reason !== "-" ? `${base} · ${r.reason}` : base;
  return withReason.trim();
}

// Diff the previous rail order (array of pane ids) against the next ranked rows (row
// objects carrying `.id`, already in BACKEND rank order — this NEVER re-ranks: ranking
// is Rust-owned). Returns the sets the DOM reconcile acts on:
//   enter = ids new this render        exit = ids gone this render
//   reuse = ids that persist           move = reused ids whose index changed
// `move` is informational; the DOM FLIP is driven by MEASURED geometry, not this.
export function reconcilePlan(prevIds, nextRows) {
  const prev = Array.isArray(prevIds) ? prevIds : [];
  const rows = Array.isArray(nextRows) ? nextRows : [];
  const prevSet = new Set(prev);
  const nextIds = rows.map((r) => r.id);
  const nextSet = new Set(nextIds);
  const enter = nextIds.filter((id) => !prevSet.has(id));
  const exit = prev.filter((id) => !nextSet.has(id));
  const reuse = nextIds.filter((id) => prevSet.has(id));
  const move = reuse.filter((id) => prev.indexOf(id) !== nextIds.indexOf(id));
  return { enter, exit, reuse, move, nextIds };
}
