// Pane -> workspace assignment map. Persisted in localStorage, pure (no React).
//
// Panes (agents) are not born with a workspace; every pane renders in one grid
// until it is explicitly *moved*. This layer records which workspace a pane has
// been moved to, which is exactly what makes cross-workspace move possible:
// reassigning a pane's wsId relocates it.
//
// DEFAULT-BUCKET RULE: a pane with NO entry here belongs to the "default bucket"
// -- the first/active workspace the caller names via `defaultWsId`. That way a
// fresh fleet (nothing assigned yet) stays fully visible under the active tab
// and no pane can silently disappear. See `paneIdsForWorkspace`.
const KEY = "acc-workspace-assign";

/**
 * Current pane->workspace map.
 * @returns {Record<string, string>} paneId -> wsId ({} if empty or corrupt).
 */
export function getAssignment() {
  try {
    const saved = JSON.parse(localStorage.getItem(KEY));
    if (saved && typeof saved === "object" && !Array.isArray(saved)) return saved;
  } catch {
    /* fall through to empty map */
  }
  return {};
}

function persist(map) {
  localStorage.setItem(KEY, JSON.stringify(map));
}

/**
 * Assign (or re-assign) a pane to a workspace. Idempotent.
 * @param {string} paneId
 * @param {string} wsId
 */
export function assign(paneId, wsId) {
  const map = getAssignment();
  map[paneId] = wsId;
  persist(map);
}

/**
 * Drop a pane's explicit assignment; it falls back to the default bucket.
 * @param {string} paneId
 */
export function unassign(paneId) {
  const map = getAssignment();
  if (paneId in map) {
    delete map[paneId];
    persist(map);
  }
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
  const map = getAssignment();
  return allIds.filter((id) => {
    const assigned = map[id];
    return assigned ? assigned === wsId : wsId === defaultWsId;
  });
}
