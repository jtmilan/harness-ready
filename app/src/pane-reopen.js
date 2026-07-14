// Agent Teams — pane reopen: PURE counter-idx selection (close/reopen fix, D63).
//
// Pane state arrays (sessionIds/harnesses/roles) are keyed by the monotonic spawn
// counter (`spawnPane`: idx = ws.counter++; id = `${wsId}-p${idx}`). closeWorkspace
// compacts paneIds but leaves those arrays keyed by the ORIGINAL idx (sparse). So a
// reopen must re-spawn each surviving pane at ITS OWN idx — never positional 0..count-1,
// which would map a survivor onto a closed neighbor's slot. These helpers compute that
// idx list; resume correctness itself is GUI-verified.

// Parse the spawn-counter idx baked into a pane id (`${wsId}-p${idx}`). -1 if absent.
export function paneIdx(id) {
  const m = /-p(\d+)$/.exec(String(id == null ? "" : id));
  return m ? Number(m[1]) : -1;
}

// The ordered counter-idxs to re-spawn on reopen: each SURVIVING pane's own idx (so it
// reads its own sparse-array slot + keeps its id/worktree/conversation). Falls back to
// 0..count-1 for legacy persisted data that has no surviving ids.
export function survivorIdxList(paneIds, count) {
  const ids = Array.isArray(paneIds) ? paneIds : [];
  const idxs = ids.map(paneIdx).filter((i) => i >= 0);
  if (idxs.length) return idxs;
  const n = Math.max(0, count | 0);
  return Array.from({ length: n }, (_, i) => i);
}
