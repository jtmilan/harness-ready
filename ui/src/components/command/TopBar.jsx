import React from "react";
import { Megaphone, Network, Radio, LayoutTemplate, Plus, Power } from "lucide-react";

// `broadcastActive` / `onBroadcastToggle` drive the ⌘⇧I broadcast TOGGLE (every keystroke
// mirrors live into all panes) — state lives with the caller, not here. Distinct from
// `onBroadcast`, which opens the one-shot "send this text once" prompt.
// Fleet Pause/Stop/Skip (playmode cluster) removed: Pause was local-only state, Stop was
// an unconfirmed closeWorkspace duplicate of CLOSE WORKSPACE, Skip was a no-op stub.
export default function TopBar({ activeCount, localWorking = 0, capMax = null, atCap = false, broadcastActive, onBroadcastToggle, onNewAgent, onBroadcast, onDelegate, onTemplates, onCloseWorkspace }) {
  return (
    <div className="flex items-center gap-6 px-5 py-4 border-b border-cyan-900/60 bg-[#0A0E13]">
      <button
        onClick={onNewAgent}
        disabled={atCap}
        title={atCap ? `At the agent cap (${capMax}). Close a pane or raise the cap to add more.` : "New agent"}
        className={`flex items-center gap-2 px-5 py-2.5 border-2 font-heading tracking-[0.2em] text-sm font-bold transition-colors ${
          atCap
            ? "border-cyan-900 text-cyan-700 bg-cyan-400/5 opacity-50 cursor-not-allowed"
            : "bg-cyan-400/15 border-cyan-400 text-cyan-300 shadow-[0_0_14px_rgba(0,229,255,0.3)] hover:bg-cyan-400/25"
        }`}
      >
        <Plus className="w-4 h-4" /> NEW AGENT
      </button>
      <button
        onClick={onBroadcast}
        title="Broadcast (⌘⇧I)"
        className="flex items-center gap-2 px-5 py-2.5 border border-cyan-800 text-cyan-500 font-heading tracking-[0.2em] text-sm font-bold hover:border-cyan-400 hover:text-cyan-300 transition-colors"
      >
        <Megaphone className="w-4 h-4" /> BROADCAST
      </button>
      <button
        onClick={onBroadcastToggle}
        title="Broadcast typing to all panes (⌘⇧I)"
        aria-label="Broadcast typing to all panes"
        aria-pressed={!!broadcastActive}
        className={`flex items-center gap-2 px-3 py-2.5 border transition-colors ${
          broadcastActive
            ? "border-cyan-400 text-cyan-300 bg-cyan-400/15 shadow-[0_0_14px_rgba(0,229,255,0.3)]"
            : "border-cyan-800 text-cyan-600 hover:border-cyan-400 hover:text-cyan-300"
        }`}
      >
        <Radio className="w-4 h-4" />
        <kbd className="px-1.5 py-0.5 border border-cyan-800/60 text-[10px] font-mono leading-none">⌘⇧I</kbd>
      </button>
      <button
        onClick={onDelegate}
        className="flex items-center gap-2 px-5 py-2.5 border border-cyan-800 text-cyan-500 font-heading tracking-[0.2em] text-sm font-bold hover:border-cyan-400 hover:text-cyan-300 transition-colors"
      >
        <Network className="w-4 h-4" /> DELEGATE
      </button>
      <button
        onClick={onTemplates}
        className="flex items-center gap-2 px-5 py-2.5 border border-cyan-800 text-cyan-500 font-heading tracking-[0.2em] text-sm font-bold hover:border-cyan-400 hover:text-cyan-300 transition-colors"
      >
        <LayoutTemplate className="w-4 h-4" /> TEMPLATES
      </button>
      <div className="ml-auto flex items-center gap-5">
        <div className="font-heading tracking-[0.2em] text-lg font-bold text-cyan-300">
          ACTIVE AGENTS: <span className="text-cyan-200">{activeCount}</span>
        </div>
        <button
          onClick={onCloseWorkspace}
          className="flex items-center gap-2 px-4 py-2 border border-red-900 text-red-400/80 font-heading tracking-[0.2em] text-xs font-bold hover:border-red-500 hover:text-red-300 transition-colors"
        >
          <Power className="w-4 h-4" /> CLOSE WORKSPACE
        </button>
      </div>
    </div>
  );
}
