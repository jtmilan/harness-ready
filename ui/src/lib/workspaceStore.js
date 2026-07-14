// Local workspace registry — persisted in localStorage.
// Real backend: each workspace maps to a repo root directory that agents
// create their git worktrees under.
import { assign, getAssignment } from "@/lib/workspaceAssign";

const KEY = "acc-workspaces";

export function loadWorkspaces() {
  try {
    const saved = JSON.parse(localStorage.getItem(KEY));
    if (Array.isArray(saved) && saved.length) return saved;
  } catch { /* fall through to seed */ }
  return [{ id: "ws-1", name: "MY WORKSPACE" }];
}

export function saveWorkspaces(list) {
  localStorage.setItem(KEY, JSON.stringify(list));
}

// Move a single pane (agent) to another workspace. Pure command — records the
// new assignment via the pane->workspace map; the grid re-buckets on next read
// (see workspaceAssign.paneIdsForWorkspace). This is the NEW capability prod
// lacks ("cross-workspace combine isn't supported").
export function moveAgentToWorkspace(paneId, toWsId) {
  assign(paneId, toWsId);
}

// Merge `fromWsId` into `intoWsId`: reassign every pane explicitly assigned to
// fromWs over to intoWs, then remove the emptied fromWs from the registry —
// unless it is the last remaining workspace (never leave the fleet with zero).
//
// Returns the resulting workspace list so React callers can `setWorkspaces(...)`
// in one step (reducer-style command+query — escape hatch: the store must both
// persist the registry and hand the caller the value to sync its state; Home's
// existing saveWorkspaces effect then re-persists the identical list, a no-op).
//
// Note: panes with NO explicit assignment (default-bucket panes shown under the
// first/active workspace) carry no entry, so they are untouched here and simply
// follow whatever workspace is the default bucket after the merge.
export function mergeWorkspaces(fromWsId, intoWsId) {
  const list = loadWorkspaces();
  if (fromWsId === intoWsId) return list;

  // 1. reassign fromWs's explicitly-assigned panes to intoWs
  const map = getAssignment();
  for (const [paneId, wsId] of Object.entries(map)) {
    if (wsId === fromWsId) assign(paneId, intoWsId);
  }

  // 2. remove the emptied fromWs (never the last workspace)
  if (list.length <= 1) return list;
  const next = list.filter((w) => w.id !== fromWsId);
  if (next.length === list.length) return list; // fromWs wasn't in the registry
  saveWorkspaces(next);
  return next;
}