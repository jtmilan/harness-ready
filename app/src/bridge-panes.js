// Pure pane-aggregation for the Bridge→Flywheel UNIFY flow. Kept dependency-free (no DOM, no
// globals) so it's unit-testable; main.js's bridgeAllLivePanes() delegates here.
//
// Returns the live panes across ALL non-dormant workspaces, RESTRICTED to the active
// workspace's repo. The unified Bridge distributes an idea to every connected harness, but the
// backend `orchestrate` takes ONE repo — a cross-repo pane would recon the wrong tree — so we
// group by the active repo and drop mismatches. A live pane = present in `sessions` AND not in
// `deadPanes` (a corpse lingers in sessions until close). Per-pane harness/role are
// counter-indexed (`${wsId}-p${idx}`), matching bridgeLivePanes().

const idxOf = (id) => {
  const m = /-p(\d+)$/.exec(id);
  return m ? Number(m[1]) : -1;
};

// has(x): deadPanes may be a Set (main.js) or any object exposing .has — normalize.
function isDead(deadPanes, id) {
  if (!deadPanes) return false;
  if (typeof deadPanes.has === "function") return deadPanes.has(id);
  return !!deadPanes[id];
}

export function allLivePanes({ workspaces, sessions, deadPanes, activeWs }) {
  const ws = workspaces || {};
  const active = activeWs ? ws[activeWs] : null;
  if (!active) return [];
  const repo = active.repo || null;
  const out = [];
  for (const w of Object.values(ws)) {
    if (!w || w.dormant) continue;
    if ((w.repo || null) !== repo) continue; // single-repo orchestrate guard
    const harnesses = w.harnesses || [];
    const roles = w.roles || [];
    const models = w.models || [];
    for (const id of (w.paneIds || [])) {
      if (!(sessions && sessions[id]) || isDead(deadPanes, id)) continue;
      const i = idxOf(id);
      out.push({
        id,
        harness: (i >= 0 && harnesses[i]) || w.harness,
        role: (i >= 0 ? roles[i] : null) || null,
        model: (i >= 0 ? models[i] : null) || null, // model-at-spawn (display)
      });
    }
  }
  return out;
}
