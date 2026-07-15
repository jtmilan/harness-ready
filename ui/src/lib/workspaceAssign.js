// Pane -> workspace assignment map. Persisted in localStorage, pure (no React).
//
// Panes (agents) are not born with a workspace; every pane renders in one grid
// until it is explicitly *moved*. This layer records which workspace a pane has
// been moved to, which is exactly what makes cross-workspace move possible:
// reassigning a pane's wsId relocates it.
//
// DEFAULT-BUCKET RULE (load-bearing — BUG-3 / Home spawn path + paneIdsForWorkspace):
// a pane with NO entry here belongs to the "default bucket" — the first/active
// workspace the caller names via `defaultWsId`. Spawned panes stay unassigned
// until Home pins them with assignMany; until then they still render under
// defaultWsId so nothing silently disappears from the active tab. Do not "fix"
// unassigned panes by auto-assigning at write time; absence IS the default bucket.
//
// Subscription + cross-window sync mirror paneLabels.js: in-memory map replaced
// (never mutated) on write, one notify per mutation, `storage` re-reads peers.
const KEY = "acc-workspace-assign";

/** @type {Record<string, string>} paneId -> wsId. Replaced (never mutated) on write. */
let assignment = load();
const listeners = new Set();

function load() {
  try {
    if (typeof localStorage === "undefined") return {};
    const saved = JSON.parse(localStorage.getItem(KEY));
    if (saved && typeof saved === "object" && !Array.isArray(saved)) return saved;
  } catch {
    /* unreadable / corrupt / private mode / node — fall through to empty map */
  }
  return {};
}

function persist() {
  try {
    if (typeof localStorage === "undefined") return;
    localStorage.setItem(KEY, JSON.stringify(assignment));
  } catch {
    /* quota or private mode: the in-memory map still holds for this session */
  }
}

function notify() {
  for (const fn of [...listeners]) fn();
}

/**
 * Subscribe to assignment-map changes (this window's writes + peer-window storage).
 * @param {() => void} fn
 * @returns {() => void} unsubscribe
 */
export function subscribe(fn) {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

// Cross-window sync: `storage` fires in *other* documents that share this origin when
// localStorage changes. The writing window already has the new map in memory and has
// notified its own listeners; this path only reloads + wakes peers so two windows do
// not silently diverge. Guarded for vitest/node (no window / no localStorage).
if (typeof window !== "undefined") {
  window.addEventListener("storage", (e) => {
    if (e.key !== KEY && e.key !== null) return;
    assignment = load();
    notify();
  });
}

/**
 * Current pane->workspace map.
 * Returns a shallow copy so callers cannot mutate module state by accident.
 * @returns {Record<string, string>} paneId -> wsId ({} if empty or corrupt).
 */
export function getAssignment() {
  return { ...assignment };
}

/**
 * Assign (or re-assign) a pane to a workspace. Idempotent write (always persists + notifies).
 * @param {string} paneId
 * @param {string} wsId
 */
export function assign(paneId, wsId) {
  assignment = { ...assignment, [paneId]: wsId };
  persist();
  notify();
}

/**
 * Assign many panes to one workspace in a single storage write and a single notify.
 * Semantics match N× assign(paneId, wsId) for the map contents; empty/null paneIds is a
 * no-op (no write, no notify).
 * @param {string[]|null|undefined} paneIds
 * @param {string} wsId
 */
export function assignMany(paneIds, wsId) {
  if (!paneIds || paneIds.length === 0) return;
  const map = { ...assignment };
  for (const id of paneIds) map[id] = wsId;
  assignment = map;
  persist();
  notify();
}

/**
 * Drop a pane's explicit assignment; it falls back to the default bucket.
 * @param {string} paneId
 */
export function unassign(paneId) {
  if (!(paneId in assignment)) return;
  const map = { ...assignment };
  delete map[paneId];
  assignment = map;
  persist();
  notify();
}

/**
 * Pane ids that render under `wsId`.
 *
 * Explicitly-assigned panes go to their assigned workspace. Panes with NO
 * assignment fall into `defaultWsId` (the first/active workspace) so nothing
 * disappears. Callers should pass the active/first workspace id as
 * `defaultWsId` for every tab query; omit it (null) only to deliberately hide
 * unassigned panes.
 *
 * @param {string} wsId          workspace being rendered
 * @param {string[]} allIds      every live pane id
 * @param {string|null} [defaultWsId=null] workspace that owns unassigned panes
 * @returns {string[]} pane ids belonging to `wsId`
 */
export function paneIdsForWorkspace(wsId, allIds, defaultWsId = null) {
  const map = assignment;
  return allIds.filter((id) => {
    const assigned = map[id];
    return assigned ? assigned === wsId : wsId === defaultWsId;
  });
}
