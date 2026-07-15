import React, { useState, useEffect, useRef, useReducer, useCallback } from "react";
import { loadWorkspaces, saveWorkspaces, moveAgentToWorkspace } from "@/lib/workspaceStore";
import { paneIdsForWorkspace, assign } from "@/lib/workspaceAssign";
import { useTiling } from "@/lib/layout/useTiling";
import { bridge } from "@/lib/agentBridge";
import { isReplyTraffic } from "@/lib/tauriAgentBridge";
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
  const [overlay, setOverlay] = useState(null); // 'broadcast' | 'delegate' | 'bulk-broadcast' | 'templates'
  const [checkedIds, setCheckedIds] = useState([]);
  const [running, setRunning] = useState(true);
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

  useEffect(() => { saveWorkspaces(workspaces); }, [workspaces]);

  const handleAddWorkspace = () => {
    const ws = { id: `ws-${Date.now()}`, name: `WORKSPACE ${workspaces.length + 1}` };
    setWorkspaces([...workspaces, ws]);
    setActiveWorkspace(ws.id);
  };
  const handleRenameWorkspace = (id, name) =>
    setWorkspaces((prev) => prev.map((w) => (w.id === id ? { ...w, name } : w)));

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
  const handlePause = () => setRunning(false);
  const handleStop = () => { setRunning(false); bridge.stopAll(); };

  const toggleCheck = (id) =>
    setCheckedIds((prev) => (prev.includes(id) ? prev.filter((x) => x !== id) : [...prev, id]));

  const handleBulkPause = () => bridge.pauseAgents(checkedIds);
  const handleBulkRestart = () => bridge.restartAgents(checkedIds);
  const handleBulkBroadcast = (msg) => bridge.broadcastTo(checkedIds, msg);
  const handleLaunchTemplate = (template) => bridge.spawnAgents(template.agents, template.name);
  const handleSpawnAgent = (cfg) => bridge.spawnAgents([cfg], "MANUAL LAUNCH");
  const handleCloseWorkspace = () => {
    bridge.closeWorkspace();
    setCheckedIds([]);
    setSelectedId(null);
    setOverlay(null);
  };

  const handleSelect = (id) => {
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

  // Close a single pane. The bridge exposes only whole-fleet closeWorkspace(); the per-pane
  // primitive is the `close_workspace` tauri invoke (BRIEF C4 / PaneMenu comment assign it to the
  // container). Adding a bridge method is out of this lane, so call the invoke directly. In the
  // web-preview mock (no window.__TAURI__) this is a no-op — the real kill happens in the Tauri shell.
  const handleClosePane = (id) => {
    if (typeof window !== "undefined" && window.__TAURI__?.core) {
      window.__TAURI__.core.invoke("close_workspace", { id });
    }
    if (selectedId === id) setSelectedId(null);
  };

  // Only the active workspace's panes are visible. Unassigned panes fall into the first workspace
  // (default bucket) so a fresh fleet renders entirely under the active tab. forceRerender re-reads
  // the assignment after a cross-workspace move.
  const visibleIds = paneIdsForWorkspace(
    activeWorkspace,
    agents.map((a) => a.id),
    workspaces[0]?.id,
  );
  const visibleAgents = agents.filter((a) => visibleIds.includes(a.id));

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
    containerRef: tilingHostRef,
    wsId: activeWorkspace,
    onDragFrame: applyDragFrame,
  });

  // Global shortcuts (L3/BRIEF C3): ⌘⇧I toggles broadcast mode, ⌘⇧G maximizes the highlighted pane.
  // onMaximizeToggle acts on the currently selected pane only; if none is selected, it does nothing.
  const handleMaximizeToggle = () => { if (selectedId) toggleZoom(selectedId); };
  useKeyboardShortcuts({
    onBroadcastToggle: () => setBroadcast((b) => !b),
    onMaximizeToggle: handleMaximizeToggle,
  });

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

  return (
    <div className="h-screen flex flex-col bg-[#0D1117] scanlines overflow-hidden">
      <TitleBar />
      <TopBar
        activeCount={activeCount}
        running={running}
        broadcastActive={broadcast}
        onBroadcastToggle={() => setBroadcast((b) => !b)}
        onNewAgent={() => setOverlay("new-agent")}
        onBroadcast={() => setOverlay("broadcast")}
        onDelegate={() => setOverlay("delegate")}
        onTemplates={() => setOverlay("templates")}
        onCloseWorkspace={() => setOverlay("close-workspace")}
        onPause={handlePause}
        onStop={handleStop}
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
        </div>
      </div>
      <div className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-[1fr_1.3fr_1.3fr_1fr] gap-4 p-4 pt-0 h-64 shrink-0">
        <AgentDirectory agents={agents} selectedId={selectedId} onSelect={handleSelect} />
        <PerformanceWidget trend={trend} agents={agents} />
        <WorkspacesPanel workspaces={workspaces} activeId={activeWorkspace} onSelect={setActiveWorkspace} onAdd={handleAddWorkspace} onRename={handleRenameWorkspace} />
        <SessionInfo sessionId={SESSION_ID} startTime={SESSION_START} running={running} />
      </div>
      </>
      )}
      {checkedIds.length > 0 && (
        <BulkActionBar
          count={checkedIds.length}
          onPause={handleBulkPause}
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