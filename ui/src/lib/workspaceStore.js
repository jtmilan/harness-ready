// Local workspace registry — persisted in localStorage.
// Real backend: each workspace maps to a repo root directory that agents
// create their git worktrees under.
import { assign, getAssignment, unassign, unassignWorkspace } from "@/lib/workspaceAssign";

const KEY = "acc-workspaces";

// Launch-pad seed: the single default workspace the fleet returns to after a
// full CLOSE WORKSPACE. Exported as a FACTORY (not a shared constant) so every
// caller gets a fresh array — mutating the return value of loadWorkspaces() or
// resetWorkspaces() can never poison a later call's seed. The array shape
// ({id,name}) matches what the rest of the app expects for workspace entries.
export function launchPadWorkspaces() {
  return [{ id: "ws-1", name: "MY WORKSPACE" }];
}

export function loadWorkspaces() {
  try {
    const saved = JSON.parse(localStorage.getItem(KEY));
    if (Array.isArray(saved) && saved.length) return saved;
  } catch { /* fall through to seed */ }
  return launchPadWorkspaces();
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
// Delete `wsId` from the registry — unless it is the last remaining workspace
// (never leave the fleet with zero). Also drops every pane assignment pointing
// at the deleted workspace so the map holds no stale entries; the caller is
// responsible for closing (or re-homing) those panes BEFORE deleting.
//
// Returns the resulting workspace list so React callers can `setWorkspaces(...)`
// in one step (same command+query shape as mergeWorkspaces).
export function deleteWorkspace(wsId) {
  const list = loadWorkspaces();
  if (list.length <= 1) return list;
  const next = list.filter((w) => w.id !== wsId);
  if (next.length === list.length) return list; // wsId wasn't in the registry
  unassignWorkspace(wsId);
  saveWorkspaces(next);
  return next;
}

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

// Reset the registry to the launch-pad seed. Used by the CLOSE WORKSPACE
// confirm path (Home.handleCloseWorkspace) after bridge.closeWorkspace() has
// terminated every pane: without this reset, the localStorage workspace list
// survives with zero panes per tab and rehydrates as empty ghost cards on
// relaunch (the bug this fixes). We also scrub every pane→workspace
// assignment so the map holds no stale entries for the dead fleet.
//
// Same command+query shape as deleteWorkspace / mergeWorkspaces: persist the
// new state AND return it, so the React caller can setWorkspaces(next) in one
// step. Idempotent — calling twice yields the same seed with empty assignments.
export function resetWorkspaces() {
  // 1. clear every pane assignment (the fleet is dead; no stale entries).
  //    Loop + unassign mirrors mergeWorkspaces's per-pane assign loop and
  //    notifies subscribers per write so any live assignment-aware UI re-reads.
  for (const paneId of Object.keys(getAssignment())) {
    unassign(paneId);
  }
  // 2. persist the launch-pad seed and hand it back to the caller.
  const seed = launchPadWorkspaces();
  saveWorkspaces(seed);
  return seed;
}