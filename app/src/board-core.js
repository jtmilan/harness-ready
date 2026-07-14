// board-core — the pure Kanban-board logic extracted from main.js (pattern:
// svg-icon-core.js / diff-view-core.js — pure module, imported back, unit-tested).
// No DOM, no module globals: everything a function needs (workspaces map, paneOwner
// lookup) is passed in, so vitest exercises the real shipped code in a node env.
//
// Left→right by attention: blocked first, then live, scheduled, idle, terminal, unknown.
// "Backlog" is the home for MCP task-model cards (lifecycle `created`) — workspace
// state rows never map here (columnFor returns only the state cols), so it's a
// tasks-only lane on the left. bucketByColumn's unknown-fallback stays the LAST col.
export const BOARD_COLS = ["Backlog", "Needs you", "Working", "Scheduled", "Idle", "Done", "Error", "Starting"];

// Total mapping QueueRow → column. needs_human is checked BEFORE state; rate_limit is
// "Scheduled" (scheduler-owned, NOT "Needs you" — D33); anything else (incl. the
// synthetic "starting" extras and any unknown state) falls to "Starting". Never throws.
export function columnFor(row) {
  if (row && row.needs_human === true) return "Needs you";
  switch (row && row.state) {
    case "working": return "Working";
    case "idle": return "Idle";
    case "done": return "Done";
    case "error": return "Error";
  }
  if (row && row.reason === "rate_limit") return "Scheduled";
  return "Starting";
}

// The FULL per-workspace row set, exactly like renderRail: ranked list_queue rows ∪
// list_workspaces ids the queue didn't carry (idle/done/just-spawned), as synthetic
// "starting" rows. PRESERVES list_queue order (the rows come first, in rank order), so
// bucketing in array order yields in-column order == list_queue order for free.
// `workspaces` (wsId → def) and `paneOwner` (paneId → wsId|null) are parameters — the
// caller owns that state; this stays pure.
export function boardRows(queue, all, workspaces, paneOwner) {
  const seen = new Set((queue || []).map((r) => r.id));
  const extra = (all || [])
    .filter((id) => !seen.has(id))
    .map((id) => ({ id, harness: "", state: "starting", reason: "-", needs_human: false }));
  // drop orphan rows (no owning workspace) — same phantom guard as renderRail.
  return [...(queue || []), ...extra].filter((r) => (workspaces || {})[paneOwner(r.id)]);
}

// Bucket rows into columns, PRESERVING array order per bucket (⇒ in-column order ==
// list_queue order). NO .sort() — single-source rank is already baked into the input
// order. A column name columnFor() returns that isn't in `cols` falls to the last col.
export function bucketByColumn(rows, cols) {
  const buckets = {};
  for (const c of cols) buckets[c] = [];
  for (const r of rows || []) {
    const name = columnFor(r);
    const c = buckets[name] ? name : cols[cols.length - 1];
    buckets[c].push(r.id);
  }
  return buckets;
}

// Apply a session-local manual order on top of the rank-ordered bucket. Rebuilt via
// filter/indexOf — NOT a comparator .sort() (the "no second ranking" invariant is
// literal). Ids in `overrideIds` (still present in the bucket) lead in override order;
// the rest follow in their original rank order. Empty/absent override ⇒ rank order.
export function applyOrder(bucketIds, overrideIds) {
  if (!overrideIds || overrideIds.length === 0) return (bucketIds || []).slice();
  const inBucket = new Set(bucketIds);
  const ordered = overrideIds.filter((id) => inBucket.has(id));
  const orderedSet = new Set(ordered);
  const rest = (bucketIds || []).filter((id) => !orderedSet.has(id));
  return [...ordered, ...rest];
}

// Compute a column's new id order after a drop. INTRA-column only: a cross-column drop
// (sourceCol !== targetCol) returns null — a NO-OP, no reorder, no state change (Model
// A; durable cross-column placement is the Task-model line, NOT crossed in Tier A).
// Pure: returns the new array, never touches the backend.
export function boardReorder(colIds, sourceCol, targetCol, draggedId, beforeId) {
  if (sourceCol !== targetCol) return null; // cross-column = no-op
  const without = (colIds || []).filter((id) => id !== draggedId);
  if (beforeId == null) { without.push(draggedId); return without; }
  const idx = without.indexOf(beforeId);
  if (idx < 0) { without.push(draggedId); return without; }
  without.splice(idx, 0, draggedId);
  return without;
}
