import React, { useState, useEffect, useRef, useReducer, useCallback } from "react";
import { loadWorkspaces, saveWorkspaces, moveAgentToWorkspace, deleteWorkspace, resetWorkspaces } from "@/lib/workspaceStore";
import { paneIdsForWorkspace, assign } from "@/lib/workspaceAssign";
import { toast } from "@/components/ui/use-toast";
import { useTiling } from "@/lib/layout/useTiling";
import { bridge } from "@/lib/agentBridge";
import { isReplyTraffic, isTauri } from "@/lib/tauriAgentBridge";
import { useKeyboardShortcuts } from "@/lib/useKeyboardShortcuts";
import TopBar from "@/components/command/TopBar";
import LayoutToolbar from "@/components/command/LayoutToolbar";
import AgentPane from "@/components/command/AgentPane";
import AgentDirectory from "@/components/command/AgentDirectory";
import WorkspacesPanel from "@/components/command/WorkspacesPanel";
import SessionInfo from "@/components/command/SessionInfo";
import CommandOverlay from "@/components/command/CommandOverlay";
import BulkActionBar from "@/components/command/BulkActionBar";
import TitleBar from "@/components/command/TitleBar";
import PerformanceWidget from "@/components/command/PerformanceWidget";
import TemplatesOverlay from "@/components/command/templates/TemplatesOverlay";
import NewAgentOverlay from "@/components/command/NewAgentOverlay";
import EmptyState from "@/components/command/EmptyState";
import ConfirmOverlay from "@/components/command/ConfirmOverlay";

const SESSION_ID = "00612425-38791089839";
const SESSION_START = Date.now();

export default function Home() {
  const [agents, setAgents] = useState([]);
  const [selectedId, setSelectedId] = useState(null);
  const [workspaces, setWorkspaces] = useState(loadWorkspaces);
  const [activeWorkspace, setActiveWorkspace] = useState(() => loadWorkspaces()[0].id);
  const [overlay, setOverlay] = useState(null); // 'broadcast' | 'delegate' | 'bulk-broadcast' | 'templates' | 'delete-workspace' | ...
  const [wsToDelete, setWsToDelete] = useState(null); // ws id pending delete confirmation
  const [wsPendingSharing, setWsPendingSharing] = useState(null); // ws id pending enable-sharing confirm
  const [checkedIds, setCheckedIds] = useState([]);
  const [trend, setTrend] = useState([]);
  // Broadcast-toggle mode (⌘⇧I): every keystroke mirrors live into all panes, except terminal
  // reply traffic (isReplyTraffic). State lives here, not in TopBar/AgentPane.
  const [broadcast, setBroadcast] = useState(false);
  // Pane-drag controller state (Lane 3): which pane is mid-drag + the ws tab under the pointer.
  const [dragPaneId, setDragPaneId] = useState(null);
  const [dragDropTargetWs, setDragDropTargetWs] = useState(null);
  // moveAgentToWorkspace writes localStorage only; bump this to re-read the assignment + re-bucket.
  const [, forceRerender] = useReducer((n) => n + 1, 0);
  const tilingHostRef = useRef(null);

  // All agent state flows through the AgentBridge (see src/lib/agentBridge.js)
  useEffect(() => {
    const unsubscribe = bridge.subscribe(setAgents);
    bridge.start();
    return unsubscribe;
  }, []);

  // Merge backend sharing state into workspaces on mount + on a 1.5s poll.
  // Backend is authoritative for allow_sharing — never persisted in localStorage
  // (workspaceStore.js writes every other ws field but explicitly skips this one).
  // The bridge's own 1.5s timer also folds sharing into its _emit, but we read
  // on our own interval so the workspaces[] shape stays in sync with the ws tab
  // row even when no agent event tickled the bridge's listener set.
  useEffect(() => {
    let cancelled = false;
    const merge = async () => {
      if (cancelled) return;
      try {
        const fetchSharing =
          (bridge.fetchSharingStates && bridge.fetchSharingStates.bind(bridge)) ||
          null;
        // Prefer the bridge's own fetch when available; fall back to a direct invoke
        // is intentionally NOT provided here — the mock bridge lacks the command and
        // must stay isolated, so a missing fetchSharing is a silent no-op.
        const m = fetchSharing ? await fetchSharing() : (bridge.getSharing ? bridge.getSharing() : {});
        if (cancelled) return;
        setWorkspaces((prev) => {
          let changed = false;
          const next = prev.map((w) => {
            const want = !!(m && m[w.id]);
            if (!!w.allow_sharing === want) return w;
            changed = true;
            return { ...w, allow_sharing: want };
          });
          return changed ? next : prev;
        });
      } catch {
        /* old backend / transient — safe default: every ws stays isolated */
      }
    };
    merge();
    const t = setInterval(merge, 1500);
    return () => { cancelled = true; clearInterval(t); };
  }, [activeWorkspace]);

  // Strip allow_sharing before persisting — backend is the sharing source of truth;
  // localStorage must never carry it (a stale persisted flag would outlive a backend
  // kill-switch flip and render the wrong badge on next load).
  useEffect(() => {
    saveWorkspaces(workspaces.map((w) => {
      if (!("allow_sharing" in w)) return w;
      const { allow_sharing: _drop, ...rest } = w;
      return rest;
    }));
  }, [workspaces]);

  const handleAddWorkspace = () => {
    const ws = { id: `ws-${Date.now()}`, name: `WORKSPACE ${workspaces.length + 1}` };
    setWorkspaces([...workspaces, ws]);
    setActiveWorkspace(ws.id);
  };
  const handleRenameWorkspace = (id, name) =>
    setWorkspaces((prev) => prev.map((w) => (w.id === id ? { ...w, name } : w)));

  // Delete flow: tile trash icon → confirm overlay → handleDeleteWorkspace.
  // Deleting a workspace terminates every pane visible under its tab (explicitly
  // assigned panes, plus default-bucket panes when deleting the first workspace),
  // then removes the tab. Never offered on the last remaining workspace.
  const requestDeleteWorkspace = (id) => {
    setWsToDelete(id);
    setOverlay("delete-workspace");
  };
  const handleDeleteWorkspace = async () => {
    const id = wsToDelete;
    setOverlay(null);
    setWsToDelete(null);
    if (!id) return;
    const paneIds = paneIdsForWorkspace(id, agents.map((a) => a.id), workspaces[0]?.id);
    if (paneIds.length) {
      await bridge.closeAgents(paneIds);
      setCheckedIds((prev) => prev.filter((x) => !paneIds.includes(x)));
      if (paneIds.includes(selectedId)) setSelectedId(null);
    }
    const next = deleteWorkspace(id);
    setWorkspaces(next);
    if (activeWorkspace === id) setActiveWorkspace(next[0]?.id);
  };

  // Sharing toggle (phase 1): enabling RELAXES the isolation boundary (agents in
  // other workspaces that also enable sharing can interact here) — gated behind a
  // ConfirmOverlay. Disabling TIGHTENS it and is immediate. Backend is authoritative;
  // optimistically flip the local flag then roll back on reject so a failed toggle
  // never leaves the badge lying about the real state.
  const applySharing = async (wsId, enabled) => {
    setWsPendingSharing(null);
    const prev = workspaces.find((w) => w.id === wsId)?.allow_sharing;
    setWorkspaces((p) => p.map((w) => (w.id === wsId ? { ...w, allow_sharing: enabled } : w)));
    try {
      await bridge.setWorkspaceSharing(wsId, enabled);
    } catch (e) {
      setWorkspaces((p) =>
        p.map((w) => (w.id === wsId ? { ...w, allow_sharing: !!prev } : w)),
      );
      toast({
        title: "Sharing toggle failed",
        description: String(e && e.message ? e.message : e).slice(0, 200),
        variant: "destructive",
      });
    }
  };
  const requestToggleSharing = (wsId, next) => {
    if (next) {
      // Enable is a boundary relaxation — confirm.
      setWsPendingSharing(wsId);
      setOverlay("enable-sharing");
    } else {
      // Disable is tightening — no confirm.
      void applySharing(wsId, false);
    }
  };

  // Sample fleet activity for the performance trend
  const agentsRef = useRef(agents);
  agentsRef.current = agents;
  useEffect(() => {
    const t = setInterval(() => {
      const active = agentsRef.current.filter((a) => a.status === "working").length;
      setTrend((prev) => [
        ...prev.slice(-29),
        { time: new Date().toLocaleTimeString("en-US", { hour12: false, minute: "2-digit", second: "2-digit" }), active },
      ]);
    }, 2000);
    return () => clearInterval(t);
  }, []);

  const handleBroadcast = (msg) => bridge.broadcast(msg);
  const handleDelegate = (msg, agentId) => bridge.delegate(agentId, msg);
  const handleRespond = (agentId, text) => bridge.sendInput(agentId, text);
  // The broadcast seam (L4/BRIEF C4): when broadcast mode is on, fan keystrokes to every live
  // pane via broadcastRaw — but keep terminal reply traffic (mouse reports, focus, OSC replies)
  // on the focused pane only, or it gets typed as garbage into every sibling.
  const handleInput = (agentId, data) =>
    broadcast && !isReplyTraffic(data)
      ? bridge.broadcastRaw(data)
      : bridge.sendRaw(agentId, data);
  const handleResize = (agentId, rows, cols) => bridge.resizePane(agentId, rows, cols);

  const toggleCheck = (id) =>
    setCheckedIds((prev) => (prev.includes(id) ? prev.filter((x) => x !== id) : [...prev, id]));

  const handleBulkPause = () => bridge.pauseAgents(checkedIds);
  // The only way out of handleBulkPause's SIGSTOP. Always available — see BulkActionBar.
  const handleBulkResume = () => bridge.resumeAgents(checkedIds);
  const handleBulkRestart = () => bridge.restartAgents(checkedIds);
  const handleBulkBroadcast = (msg) => bridge.broadcastTo(checkedIds, msg);
// Template launch: pre-mint backend-shaped ws id, create UI tab + activate FIRST, then
  // spawn with assignTo so optimistic panes never land in the default bucket mid-sequence
  // (spawn-window squeeze → 1–2 col PTYs). Manual spawn uses assignTo when the operator
  // is on a non-default workspace for the same reason.
  const defaultWsId = workspaces[0]?.id;
  const spawnAssignOpts =
    activeWorkspace && activeWorkspace !== defaultWsId
      ? { assignTo: activeWorkspace }
      : {};
  const handleLaunchTemplate = async (template) => {
    // Same id scheme as TauriAgentBridge.spawnAgents — bridge accepts optional wsId.
    const wsId = "ws" + String(Math.floor(10000 + Math.random() * 90000)) + "x0";
    setWorkspaces((prev) => [...prev, { id: wsId, name: template.name }]);
    setActiveWorkspace(wsId);
    await bridge.spawnAgents(template.agents, template.name, { assignTo: wsId, wsId });
    forceRerender();
  };
  const handleSpawnAgent = async (cfg) => {
    const { paneIds } = await bridge.spawnAgents([cfg], "MANUAL LAUNCH", spawnAssignOpts);
    // When assignTo was not used (default bucket), still pin to the active workspace.
    if (!spawnAssignOpts.assignTo) {
      const wsId = activeWorkspace;
      for (const paneId of paneIds) assign(paneId, wsId);
    }
    forceRerender(); // assignment is localStorage-only; re-bucket without a tab switch
  };
  // CLOSE WORKSPACE confirm path: terminate the whole fleet AND return to a
  // clean launch pad (matches the ConfirmOverlay copy). bridge.closeWorkspace()
  // kills every pane on the backend side; resetWorkspaces() then wipes the
  // localStorage workspace registry back to the single seed tab and scrubs
  // every pane→workspace assignment. Without the registry reset, the per-tab
  // entries survive with zero panes and rehydrate as empty ghost cards on
  // relaunch (the bug this fixes). forceRerender() mirrors the other handlers
  // that mutate the assignment store (handleSpawnAgent, handleDropOnWorkspace)
  // so the grid re-reads the now-empty assignment map on the next paint.
  const handleCloseWorkspace = () => {
    bridge.closeWorkspace();
    const next = resetWorkspaces();
    setWorkspaces(next);
    setActiveWorkspace(next[0].id);
    forceRerender();
    setCheckedIds([]);
    setSelectedId(null);
    setOverlay(null);
  };

  // K2 may fire onSelect on every terminal focus via onFocusCapture — keep this cheap and
  // idempotent (no work when the id is already selected; scroll only on a real change).
  const handleSelect = (id) => {
    if (id === selectedId) return;
    setSelectedId(id);
    document.getElementById(`pane-${id}`)?.scrollIntoView({ behavior: "smooth", block: "center" });
  };

  // Pane-head kebab actions (L2/BRIEF C2). AgentPane stays presentational: it emits the action and
  // Home performs the side effect (clipboard, re-assign, close, zoom). Rename is the one action
  // AgentPane persists itself (paneLabels.setPaneLabel) before notifying us — here it's a no-op.
  const handleMenuAction = (action, id, payload) => {
    switch (action) {
      case "rename":
        break; // label already committed by AgentPane via paneLabels.setPaneLabel
      case "maximize":
        toggleZoom(id);
        break;
      case "copy-id":
        navigator.clipboard?.writeText(id);
        break;
      case "copy-branch": {
        const a = agents.find((x) => x.id === id);
        if (a?.branch) navigator.clipboard?.writeText(a.branch);
        break;
      }
      case "move-to-ws":
        if (payload?.wsId) {
          assign(id, payload.wsId);
          // Re-bucket the grid now that the assignment changed (mirrors handleDropOnWorkspace).
          forceRerender();
          if (selectedId === id && payload.wsId !== activeWorkspace) setSelectedId(null);
        }
        break;
      case "close":
        handleClosePane(id);
        break;
      default:
        break;
    }
  };

  // Close a single pane via the bridge contract (closeAgents → close_workspace + _forget).
  // Never raw-invoke: mock path must drop the agent, and Tauri must clear spawned/offsets.
  const handleClosePane = (id) => {
    void bridge.closeAgents([id]);
    setCheckedIds((prev) => prev.filter((x) => x !== id));
    if (selectedId === id) setSelectedId(null);
  };

  // Only the active workspace's panes are visible. Unassigned panes fall into the first workspace
  // (default bucket) so a fresh fleet renders entirely under the active tab. forceRerender re-reads
  // the assignment after a cross-workspace move.
  const allPaneIds = agents.map((a) => a.id);
  const visibleIds = paneIdsForWorkspace(
    activeWorkspace,
    allPaneIds,
    workspaces[0]?.id,
  );
  // Per-workspace member count (every ws, not just the active one) — feeds the
  // WorkspacesPanel tile footer so the operator can see fleet distribution at a
  // glance without tab-hopping. Cheap: one pass per render over a small agent list.
  const memberCounts = Object.fromEntries(
    workspaces.map((w) => [
      w.id,
      paneIdsForWorkspace(w.id, allPaneIds, workspaces[0]?.id).length,
    ]),
  );
  const visibleAgents = agents.filter((a) => visibleIds.includes(a.id));
  // Tile mode pins a coordinator-role pane (free-text role containing "coordinator"/"orchestrator",
  // case-insensitive — the header uppercases it to [COORDINATOR]) as a full-height left column and
  // grids the remaining workers on the right. Roles that don't match ⇒ no pin, the whole fleet
  // becomes one uniform aligned grid.
  const coordinatorId =
    visibleAgents.find(
      (a) => typeof a.role === "string" && /(coordinator|orchestrator)/i.test(a.role),
    )?.id ?? null;

  // Headless BSP tiling for the visible panes → absolute {x,y,w,h} rects + draggable seams.
  // Direct-DOM frame applier for seam drags (prod relayout parity, main.js:886-935): the hook
  // hands us fresh rects/seams per rAF and we write el.style.* straight onto the pane + handle
  // elements — no React render until drag end. Panes are matched by data-pane-id, handles by index.
  const applyDragFrame = useCallback((frameRects, frameSeams) => {
    const host = tilingHostRef.current;
    if (!host) return;
    for (const [id, r] of Object.entries(frameRects)) {
      const el = host.querySelector(`[data-pane-id="${CSS.escape(id)}"]`);
      if (!el) continue;
      el.style.left = r.x + "px";
      el.style.top = r.y + "px";
      el.style.width = r.w + "px";
      el.style.height = r.h + "px";
    }
    frameSeams.forEach((s, i) => {
      const el = host.querySelector(`[data-seam-idx="${i}"]`);
      if (!el) return;
      const HANDLE = 8;
      if (s.dir === "v") {
        el.style.left = s.rect.left - HANDLE / 2 + "px";
        el.style.top = s.rect.top + "px";
        el.style.height = s.rect.height + "px";
      } else {
        el.style.top = s.rect.top - HANDLE / 2 + "px";
        el.style.left = s.rect.left + "px";
        el.style.width = s.rect.width + "px";
      }
    });
  }, []);

  const { rects, mode, setMode, seams, onSeamPointerDown, movePane, zoomId, toggleZoom } = useTiling({
    paneIds: visibleIds,
    focusedId: selectedId,
    coordinatorId,
    containerRef: tilingHostRef,
    wsId: activeWorkspace,
    onDragFrame: applyDragFrame,
  });

  // Global shortcuts (L3/BRIEF C3): ⌘⇧I toggles broadcast mode, ⌘⇧G maximizes the highlighted pane.
  // BUG-1 / K3: selection-first, else the currently zoomed pane so restore stays reachable when
  // selection was lost while zoomed. Nothing selected and nothing zoomed → no-op.
  const handleMaximizeToggle = () => {
    const target = selectedId ?? zoomId;
    if (target) toggleZoom(target);
  };
  useKeyboardShortcuts({
    onBroadcastToggle: () => setBroadcast((b) => !b),
    onMaximizeToggle: handleMaximizeToggle,
  });

  // T3: native app-menu "Toggle Pane Zoom" (Cmd+Shift+G) emits "maximize-pane" from
  // lib.rs. Same consumer as the JS keydown arm. Latest-ref so the once-bound listen
  // closure never freezes the first render's selectedId/zoomId (BUG-2 class trap).
  // Browser preview has no Tauri inject → no-op; the keydown arm still serves Chrome.
  // No @tauri-apps/api npm dep in this app — withGlobalTauri exposes window.__TAURI__.
  const maximizeToggleRef = useRef(handleMaximizeToggle);
  useEffect(() => {
    maximizeToggleRef.current = handleMaximizeToggle;
  });
  useEffect(() => {
    if (!isTauri()) return undefined;
    const listen = window.__TAURI__?.event?.listen;
    if (typeof listen !== "function") return undefined;
    let unlisten = null;
    let cancelled = false;
    listen("maximize-pane", () => {
      maximizeToggleRef.current?.();
    }).then((fn) => {
      if (cancelled) {
        try { fn(); } catch { /* ignore */ }
        return;
      }
      unlisten = fn;
    }).catch(() => { /* event API absent / reject — browser-safe */ });
    return () => {
      cancelled = true;
      if (typeof unlisten === "function") {
        try { unlisten(); } catch { /* ignore */ }
      }
    };
  }, []);

  // OpenCode/Bun can segfault once at startup; backend respawns the same pane id with a
  // fresh PTY. Reset bridge cursor/scrollback so the terminal rewrites (legacy main.js
  // already handled this; React path was missing it → silent blank after respawn).
  // Does NOT touch the sized-gate / wrap path.
  useEffect(() => {
    if (!isTauri()) return undefined;
    const listen = window.__TAURI__?.event?.listen;
    if (typeof listen !== "function") return undefined;
    let unlisten = null;
    let cancelled = false;
    listen("pane-early-respawn", (e) => {
      const p = (e && e.payload) || {};
      if (!p.id) return;
      // ok:true → backend respawned a fresh PTY; reset delta cursor + scrollback.
      // ok:false → budget spent (OpenCode/Bun crash loop); still reset so the pane is
      // not a silent blank corpse holding stale scrollback (nexus-agents has no handler;
      // main.js toasts — we at least re-arm the terminal for the failure surface).
      if (typeof bridge.onEarlyRespawn === "function") bridge.onEarlyRespawn(p.id);
    }).then((fn) => {
      if (cancelled) {
        try { fn(); } catch { /* ignore */ }
        return;
      }
      unlisten = fn;
    }).catch(() => {});
    return () => {
      cancelled = true;
      if (typeof unlisten === "function") {
        try { unlisten(); } catch { /* ignore */ }
      }
    };
  }, []);

  // Backend emits "spawn-cap-warning" when a spawn is parked at the global concurrent-pane
  // cap (the queue path that used to be totally silent). Belt-and-suspenders with the bridge's
  // own queued-toast: this also fires for spawn paths that don't go through spawnAgents
  // (external/delegate). Coalesced — one toast per burst, not one per queued pane.
  useEffect(() => {
    if (!isTauri()) return undefined;
    const listen = window.__TAURI__?.event?.listen;
    if (typeof listen !== "function") return undefined;
    let unlisten = null;
    let cancelled = false;
    let timer = null;
    let last = null;
    listen("spawn-cap-warning", (e) => {
      const p = (e && e.payload) || {};
      last = p;
      if (timer) return; // coalesce a burst of queued spawns into a single toast
      timer = setTimeout(() => {
        timer = null;
        const q = last || {};
        toast({
          title: "Agent cap reached — spawns are queued",
          description:
            `${q.working ?? "?"}/${q.max ?? "?"} panes running. New panes wait in line ` +
            "(#" + (q.position ?? "?") + "). Close idle panes or raise the cap to run more.",
          variant: "destructive",
        });
      }, 400);
    }).then((fn) => {
      if (cancelled) {
        try { fn(); } catch { /* ignore */ }
        return;
      }
      unlisten = fn;
    }).catch(() => { /* event API absent / reject — browser-safe */ });
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
      if (typeof unlisten === "function") {
        try { unlisten(); } catch { /* ignore */ }
      }
    };
  }, []);

  // Cross-workspace drop: reassign the pane, then re-render so the grid re-buckets.
  const handleDropOnWorkspace = (paneId, wsId) => {
    if (!paneId || !wsId) return;
    moveAgentToWorkspace(paneId, wsId);
    forceRerender();
  };

  // Drop quadrant → intra-workspace move direction (relative to the target pane's box).
  const dropDir = (rect, x, y) => {
    const rx = (x - rect.left) / rect.width - 0.5;
    const ry = (y - rect.top) / rect.height - 0.5;
    if (Math.abs(rx) > Math.abs(ry)) return rx < 0 ? "left" : "right";
    return ry < 0 ? "up" : "down";
  };

  // Drop-zone quadrant overlay (prod showDropZone parity, main.js:394-421): an imperative DOM
  // element appended to the tiling host, positioned per pointermove — never React state (a
  // per-frame setState would re-render the whole tree mid-drag). Removed on drag end.
  const dropZoneElRef = useRef(null);
  const hideDropZone = () => {
    if (dropZoneElRef.current) {
      dropZoneElRef.current.remove();
      dropZoneElRef.current = null;
    }
  };
  const showDropZone = (paneEl, dir) => {
    const host = tilingHostRef.current;
    if (!host) return;
    const hostR = host.getBoundingClientRect();
    const r = paneEl.getBoundingClientRect();
    // quadrant → the half of the target pane the dragged pane would land in
    let x = r.left - hostR.left;
    let y = r.top - hostR.top;
    let w = r.width;
    let h = r.height;
    if (dir === "left") w = r.width / 2;
    else if (dir === "right") { x += r.width / 2; w = r.width / 2; }
    else if (dir === "up") h = r.height / 2;
    else { y += r.height / 2; h = r.height / 2; }
    let z = dropZoneElRef.current;
    if (!z) {
      z = document.createElement("div");
      z.style.cssText =
        "position:absolute;pointer-events:none;z-index:30;background:rgba(34,211,238,0.15);border:1px solid rgba(34,211,238,0.7);";
      host.appendChild(z);
      dropZoneElRef.current = z;
    }
    z.style.left = x + "px";
    z.style.top = y + "px";
    z.style.width = w + "px";
    z.style.height = h + "px";
  };

  // Pointer-based pane drag (NOT html5 draggable — Tauri intercepts native DnD). Fired from an
  // AgentPane header pointerdown. A 6px threshold separates a click (select) from a drag. During the
  // drag we hit-test elementFromPoint for a ws tab ([data-ws-drop]) → highlight; on release we drop
  // onto a ws tab (move to workspace) or another pane ([data-pane-id]) (intra-ws reorder via movePane).
  const handlePaneDragStart = (id, e) => {
    if (e.button != null && e.button !== 0) return; // left button only
    const drag = { id, sx: e.clientX, sy: e.clientY, active: false, lastWs: null };
    const onMove = (ev) => {
      if (!drag.active) {
        if (Math.hypot(ev.clientX - drag.sx, ev.clientY - drag.sy) < 6) return;
        drag.active = true;
        setDragPaneId(id);
      }
      ev.preventDefault();
      const el = document.elementFromPoint(ev.clientX, ev.clientY);
      const wsEl = el && el.closest("[data-ws-drop]");
      const ws = wsEl ? wsEl.getAttribute("data-ws-drop") : null;
      if (ws !== drag.lastWs) {
        drag.lastWs = ws;
        setDragDropTargetWs(ws);
      }
      // Quadrant preview over a sibling pane (prod drop-zone): shows WHERE the pane will land.
      const paneEl = el && el.closest("[data-pane-id]");
      if (paneEl && paneEl.getAttribute("data-pane-id") !== id) {
        showDropZone(paneEl, dropDir(paneEl.getBoundingClientRect(), ev.clientX, ev.clientY));
      } else {
        hideDropZone();
      }
    };
    const onUp = (ev) => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      hideDropZone();
      setDragPaneId(null);
      setDragDropTargetWs(null);
      if (!drag.active) return; // never crossed the threshold — it was a click, not a drag
      const el = document.elementFromPoint(ev.clientX, ev.clientY);
      if (!el) return;
      const wsEl = el.closest("[data-ws-drop]");
      if (wsEl) {
        const wsId = wsEl.getAttribute("data-ws-drop");
        if (wsId && wsId !== activeWorkspace) handleDropOnWorkspace(id, wsId);
        return;
      }
      const paneEl = el.closest("[data-pane-id]");
      if (paneEl) {
        const targetId = paneEl.getAttribute("data-pane-id");
        if (targetId && targetId !== id) {
          movePane(id, targetId, dropDir(paneEl.getBoundingClientRect(), ev.clientX, ev.clientY));
        }
      }
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  };

  // Seam handle box: a thin grabbable bar centered on the divider gutter (seam.rect gives the center).
  const seamStyle = (seam) => {
    const r = seam.rect;
    const HANDLE = 8;
    return seam.dir === "v"
      ? { position: "absolute", left: r.left - HANDLE / 2, top: r.top, width: HANDLE, height: r.height, cursor: "col-resize", touchAction: "none", zIndex: 20 }
      : { position: "absolute", top: r.top - HANDLE / 2, left: r.left, width: r.width, height: HANDLE, cursor: "row-resize", touchAction: "none", zIndex: 20 };
  };

  const activeCount = agents.filter((a) => a.status === "working").length;
  // B: per-workspace working count + the global admission cap (from the bridge's 1s
  // get_capacity poll). atCap gates NEW AGENT / TEMPLATES; nearCap shows a HUD warning.
  const localWorking = visibleAgents.filter((a) => a.status === "working").length;
  const capacity = bridge.getCapacity ? bridge.getCapacity() : null;
  const capMax = capacity?.max ?? null;
  const capWorking = capacity?.working ?? activeCount;
  const atCap = capMax != null && capWorking >= capMax;
  const nearCap = capMax != null && !atCap && capWorking >= capMax - 2;
  const guardedNewAgent = () => {
    if (atCap) {
      toast({
        title: "At the agent cap",
        description: `${capWorking}/${capMax} panes running. Close an idle pane or raise the cap (Scheduler) to add more.`,
        variant: "destructive",
      });
      return;
    }
    if (nearCap) {
      toast({ title: "Near the agent cap", description: `${capWorking}/${capMax} panes running.` });
    }
    setOverlay("new-agent");
  };
  const guardedTemplates = () => {
    if (atCap) {
      toast({
        title: "At the agent cap",
        description: `${capWorking}/${capMax} panes running — a template's spawns would queue. Free slots first.`,
        variant: "destructive",
      });
      return;
    }
    if (nearCap) {
      toast({ title: "Near the agent cap", description: `${capWorking}/${capMax} panes running — a big template may queue.` });
    }
    setOverlay("templates");
  };

  return (
    <div className="h-screen flex flex-col bg-[#0D1117] scanlines overflow-hidden">
      <TitleBar />
      <TopBar
        activeCount={activeCount}
        localWorking={localWorking}
        capMax={capMax}
        atCap={atCap}
        broadcastActive={broadcast}
        onBroadcastToggle={() => setBroadcast((b) => !b)}
        onNewAgent={guardedNewAgent}
        onBroadcast={() => setOverlay("broadcast")}
        onDelegate={() => setOverlay("delegate")}
        onTemplates={guardedTemplates}
        onCloseWorkspace={() => setOverlay("close-workspace")}
      />
      {agents.length === 0 ? (
        <EmptyState
          onNewAgent={() => setOverlay("new-agent")}
          onTemplates={() => setOverlay("templates")}
          workspaces={workspaces}
          activeId={activeWorkspace}
          onSelectWorkspace={setActiveWorkspace}
          onAddWorkspace={handleAddWorkspace}
          onRenameWorkspace={handleRenameWorkspace}
          onDeleteWorkspace={requestDeleteWorkspace}
        />
      ) : (
      <>
      <LayoutToolbar
        mode={mode}
        onModeChange={setMode}
        workspaces={workspaces}
        activeWs={activeWorkspace}
        onSelectWorkspace={setActiveWorkspace}
        draggingPaneId={dragPaneId}
        dropTargetWs={dragDropTargetWs}
        onDropPaneOnWorkspace={handleDropOnWorkspace}
      />
      <div className="flex-1 overflow-hidden p-4">
        <div ref={tilingHostRef} className="relative w-full h-full">
          {visibleAgents.map((agent) => {
            const r = rects[agent.id];
            return (
              <React.Fragment key={agent.id}>
                <AgentPane
                  agent={agent}
                  style={{
                    position: "absolute",
                    left: r ? r.x : 0,
                    top: r ? r.y : 0,
                    width: r ? r.w : 0,
                    height: r ? r.h : 0,
                    display: r ? "flex" : "none", // KEEP MOUNTED when hidden — unmount kills the xterm/PTY
                    opacity: dragPaneId === agent.id ? 0.6 : 1,
                  }}
                  onDragStart={handlePaneDragStart}
                  visible={!!r}
                  selected={selectedId === agent.id}
                  checked={checkedIds.includes(agent.id)}
                  onToggleCheck={toggleCheck}
                  onSelect={handleSelect}
                  onRespond={handleRespond}
                  onInput={handleInput}
                  onResize={handleResize}
                  zoomed={zoomId === agent.id}
                  onMaximize={toggleZoom}
                  onMenuAction={handleMenuAction}
                  workspaces={workspaces.filter((w) => w.id !== activeWorkspace)}
                />
                {/* Broadcast-on cue: a cyan ring matched to the repo's selected-pane glow idiom,
                    overlaying every live pane border while broadcast mode is active. Non-interactive. */}
                {broadcast && r && (
                  <div
                    aria-hidden="true"
                    className="pointer-events-none absolute z-10 rounded-[1px] ring-2 ring-cyan-300/80 shadow-[0_0_16px_rgba(0,229,255,0.45)]"
                    style={{ left: r.x, top: r.y, width: r.w, height: r.h }}
                  />
                )}
              </React.Fragment>
            );
          })}
          {seams.map((seam, i) => (
            <div
              key={seam.id}
              data-seam-idx={i}
              onPointerDown={(e) => onSeamPointerDown(seam, e)}
              style={seamStyle(seam)}
              className="bg-transparent hover:bg-cyan-400/30 transition-colors"
            />
          ))}
          {visibleAgents.length === 0 && workspaces.length > 1 && activeWorkspace !== workspaces[0]?.id && (
            <div className="absolute inset-0 flex items-center justify-center p-8 pointer-events-none">
              <div className="max-w-md text-center border border-cyan-900/70 bg-[#0A1219]/80 rounded-sm px-6 py-5">
                <div className="font-heading text-[11px] tracking-[0.3em] text-cyan-400 font-bold mb-2">
                  NO AGENTS IN THIS WORKSPACE
                </div>
                <div className="font-mono text-[11px] text-cyan-700/90 leading-relaxed">
                  The fleet is at its concurrent-pane cap, so this workspace's spawns are queued
                  or failed — watch for the toast. Close idle panes or raise the cap (Scheduler),
                  or launch a template here.
                </div>
              </div>
            </div>
          )}
        </div>
      </div>
      <div className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-[1fr_1.3fr_1.3fr_1fr] gap-4 p-4 pt-0 h-64 shrink-0">
        <AgentDirectory agents={agents} selectedId={selectedId} onSelect={handleSelect} />
        <PerformanceWidget trend={trend} agents={agents} />
        <WorkspacesPanel workspaces={workspaces} activeId={activeWorkspace} onSelect={setActiveWorkspace} onAdd={handleAddWorkspace} onRename={handleRenameWorkspace} onDelete={requestDeleteWorkspace} memberCounts={memberCounts} onToggleSharing={requestToggleSharing} />
        {/* running is always true: fleet Pause was local-only and is gone; SessionInfo still
            expects the prop (that file is out of this lane). */}
        <SessionInfo sessionId={SESSION_ID} startTime={SESSION_START} running />
      </div>
      </>
      )}
      {checkedIds.length > 0 && (
        <BulkActionBar
          count={checkedIds.length}
          onPause={handleBulkPause}
          onResume={handleBulkResume}
          onRestart={handleBulkRestart}
          onBroadcast={() => setOverlay("bulk-broadcast")}
          onClear={() => setCheckedIds([])}
        />
      )}
      {overlay === "bulk-broadcast" && (
        <CommandOverlay
          title="BROADCAST TO SELECTED"
          description={`// transmit command to ${checkedIds.length} selected agent(s)`}
          onSubmit={handleBulkBroadcast}
          onClose={() => setOverlay(null)}
        />
      )}
      {overlay === "close-workspace" && (
        <ConfirmOverlay
          title="CLOSE WORKSPACE"
          description={`// terminate all ${agents.length} agent(s), remove their worktrees, and return to the launch pad`}
          confirmLabel="TERMINATE & CLOSE"
          onConfirm={handleCloseWorkspace}
          onClose={() => setOverlay(null)}
        />
      )}
      {overlay === "delete-workspace" && (() => {
        const ws = workspaces.find((w) => w.id === wsToDelete);
        const count = paneIdsForWorkspace(wsToDelete, agents.map((a) => a.id), workspaces[0]?.id).length;
        return (
          <ConfirmOverlay
            title="DELETE WORKSPACE"
            description={
              count > 0
                ? `// delete "${ws?.name ?? wsToDelete}" — terminate its ${count} agent(s), remove their worktrees, and drop the tab`
                : `// delete "${ws?.name ?? wsToDelete}" — the workspace is empty; the tab is removed`
            }
            confirmLabel="DELETE"
            onConfirm={handleDeleteWorkspace}
            onClose={() => { setOverlay(null); setWsToDelete(null); }}
          />
        );
      })()}
      {overlay === "enable-sharing" && (() => {
        const ws = workspaces.find((w) => w.id === wsPendingSharing);
        return (
          <ConfirmOverlay
            title="ENABLE CROSS-WORKSPACE SHARING"
            description={
              `// relax workspace isolation — agents in other workspaces that also enable sharing ` +
              `will be able to interact with panes and read data here (${ws?.name ?? wsPendingSharing})`
            }
            confirmLabel="ENABLE SHARING"
            onConfirm={() => applySharing(wsPendingSharing, true)}
            onClose={() => { setOverlay(null); setWsPendingSharing(null); }}
          />
        );
      })()}
      {overlay === "new-agent" && (
        <NewAgentOverlay onLaunch={handleSpawnAgent} onClose={() => setOverlay(null)} />
      )}
      {overlay === "templates" && (
        <TemplatesOverlay onLaunch={handleLaunchTemplate} onClose={() => setOverlay(null)} />
      )}
      {overlay === "broadcast" && (
        <CommandOverlay
          title="BROADCAST"
          description="// transmit command to all agents in the fleet"
          onSubmit={handleBroadcast}
          onClose={() => setOverlay(null)}
        />
      )}
      {overlay === "delegate" && (
        <CommandOverlay
          title="DELEGATE"
          description="// assign a task to a specific agent"
          agents={agents}
          requireAgent
          onSubmit={handleDelegate}
          onClose={() => setOverlay(null)}
        />
      )}
    </div>
  );
}