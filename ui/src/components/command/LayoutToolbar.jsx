import React, { useState } from "react";
import { Square, LayoutGrid, Columns3, Maximize2 } from "lucide-react";

// Layout modes. Keys match what Lane 1 / the coordinator switch on in Home.
const MODES = [
  { key: "single", icon: Square, title: "Single pane (⌘G)" },
  { key: "tile", icon: LayoutGrid, title: "Tile grid" },
  { key: "columns", icon: Columns3, title: "Columns" },
  { key: "focus", icon: Maximize2, title: "Focus" },
];

const ModeButton = ({ icon: Icon, active, title, onClick }) => (
  <button
    type="button"
    onClick={onClick}
    title={title}
    aria-pressed={active}
    className={`w-9 h-9 flex items-center justify-center border transition-all duration-150 ${
      active
        ? "border-cyan-400 text-cyan-300 bg-cyan-400/10 shadow-[0_0_10px_rgba(0,229,255,0.35)]"
        : "border-cyan-900 text-cyan-700 hover:border-cyan-500 hover:text-cyan-300"
    }`}
  >
    <Icon className="w-4 h-4" />
  </button>
);

/**
 * Layout-mode switch + workspace tabs that double as drop targets for a pane
 * dragged from the grid (Lane 1 owns the pane drag; this only renders the
 * drop affordance + highlight and reports the drop).
 *
 * Drag wiring is pointer-based, NOT html5 draggable (Tauri intercepts native
 * DnD). Lane 1's drag controller drives the global pointermove/up and hit-tests
 * with `document.elementFromPoint`; each workspace tab carries a
 * `data-ws-drop="<wsId>"` attribute so that hit-test can resolve the target and
 * call `onDropPaneOnWorkspace`. `onPointerUp` on the tab is a fallback for the
 * non-pointer-capture case. Local pointer-enter/leave drives the highlight, and
 * the optional controlled `dropTargetWs` prop lets Lane 1 force the highlight
 * from its elementFromPoint result while the pointer is captured.
 *
 * @param {object} props
 * @param {"single"|"tile"|"columns"|"focus"} props.mode        active layout mode
 * @param {(mode: string) => void} props.onModeChange           mode button click
 * @param {{id: string, name: string}[]} props.workspaces       workspace tabs
 * @param {string} props.activeWs                               selected workspace id
 * @param {(wsId: string) => void} props.onSelectWorkspace      tab click
 * @param {string|null} [props.draggingPaneId]                  set by Lane 1 while a pane drag is active
 * @param {string|null} [props.dropTargetWs]                    optional: ws under the pointer (forces highlight)
 * @param {(paneId: string, wsId: string) => void} [props.onDropPaneOnWorkspace] drop handler (-> moveAgentToWorkspace)
 */
export default function LayoutToolbar({
  mode,
  onModeChange,
  workspaces,
  activeWs,
  onSelectWorkspace,
  draggingPaneId = null,
  dropTargetWs = null,
  onDropPaneOnWorkspace,
}) {
  const [hoverWs, setHoverWs] = useState(null);
  const dragging = draggingPaneId != null;

  const handleDrop = (wsId) => {
    if (dragging && onDropPaneOnWorkspace) onDropPaneOnWorkspace(draggingPaneId, wsId);
    setHoverWs(null);
  };

  return (
    <div className="flex items-center gap-4 px-5 py-2.5 border-b border-cyan-900/60 bg-[#0A0E13]">
      <div className="flex items-center gap-2">
        <span className="text-[10px] font-heading tracking-[0.3em] text-cyan-600 font-bold mr-1">LAYOUT</span>
        {MODES.map((m) => (
          <ModeButton
            key={m.key}
            icon={m.icon}
            title={m.title}
            active={mode === m.key}
            onClick={() => onModeChange(m.key)}
          />
        ))}
      </div>

      <div className="h-6 w-px bg-cyan-900/60" />

      <div className="flex items-center gap-2 overflow-x-auto terminal-scroll">
        {workspaces.map((ws) => {
          const active = activeWs === ws.id;
          const dropActive = dragging && (dropTargetWs === ws.id || hoverWs === ws.id);
          return (
            <button
              key={ws.id}
              type="button"
              data-ws-drop={ws.id}
              onClick={() => onSelectWorkspace(ws.id)}
              onPointerEnter={() => dragging && setHoverWs(ws.id)}
              onPointerLeave={() => setHoverWs((cur) => (cur === ws.id ? null : cur))}
              onPointerUp={() => handleDrop(ws.id)}
              title={dragging ? `Move pane to ${ws.name}` : ws.name}
              className={`px-4 py-1.5 border font-heading text-[11px] font-bold tracking-[0.2em] whitespace-nowrap transition-colors ${
                dropActive
                  ? "border-cyan-300 bg-cyan-300/25 text-cyan-100 shadow-[0_0_12px_rgba(0,229,255,0.5)]"
                  : active
                    ? "border-cyan-400 bg-cyan-400/15 text-cyan-200 shadow-[0_0_8px_rgba(0,229,255,0.3)]"
                    : dragging
                      ? "border-cyan-700 border-dashed text-cyan-500 hover:border-cyan-400 hover:text-cyan-300"
                      : "border-cyan-900 text-cyan-700 hover:border-cyan-500 hover:text-cyan-300"
              }`}
            >
              {ws.name}
            </button>
          );
        })}
      </div>
    </div>
  );
}
