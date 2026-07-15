import React from "react";
import { Megaphone, Network, Play, Pause, Square, FastForward, LayoutTemplate, Plus, Power } from "lucide-react";

const PlayBtn = ({ icon: Icon, onClick, active, title }) => (
  <button
    onClick={onClick}
    title={title}
    className={`w-9 h-9 flex items-center justify-center border transition-all duration-150 ${
      active
        ? "border-cyan-400 text-cyan-300 bg-cyan-400/10 shadow-[0_0_10px_rgba(0,229,255,0.35)]"
        : "border-cyan-900 text-cyan-700 hover:border-cyan-500 hover:text-cyan-300"
    }`}
  >
    <Icon className="w-4 h-4" fill={active ? "currentColor" : "none"} />
  </button>
);

export default function TopBar({ activeCount, running, onNewAgent, onBroadcast, onDelegate, onTemplates, onCloseWorkspace, onPlay, onPause, onStop, onSkip }) {
  return (
    <div className="flex items-center gap-6 px-5 py-4 border-b border-cyan-900/60 bg-[#0A0E13]">
      <button
        onClick={onNewAgent}
        className="flex items-center gap-2 px-5 py-2.5 bg-cyan-400/15 border-2 border-cyan-400 text-cyan-300 font-heading tracking-[0.2em] text-sm font-bold shadow-[0_0_14px_rgba(0,229,255,0.3)] hover:bg-cyan-400/25 transition-colors"
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
      <div className="h-8 w-px bg-cyan-900/60" />
      <div className="flex flex-col items-center gap-1.5">
        <span className="text-[10px] font-heading tracking-[0.3em] text-cyan-600 font-bold">PLAYMODE CONTROLS</span>
        <div className="flex gap-2">
          <PlayBtn icon={Play} onClick={onPlay} active={running} title="Resume all" />
          <PlayBtn icon={Pause} onClick={onPause} title="Pause all" />
          <PlayBtn icon={Square} onClick={onStop} title="Stop all" />
          <PlayBtn icon={FastForward} onClick={onSkip} title="Advance starting agents" />
        </div>
      </div>
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