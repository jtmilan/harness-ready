import React from "react";
import { Plus, LayoutTemplate, Zap } from "lucide-react";
import { AGENT_KINDS, KIND_IDS } from "@/lib/agentTypes";
import WorkspaceTile from "@/components/command/WorkspaceTile";

export default function EmptyState({ onNewAgent, onTemplates, onLoadDemo, workspaces = [], activeId, onSelectWorkspace, onAddWorkspace, onRenameWorkspace, onDeleteWorkspace }) {
  // Never offer delete on the last remaining workspace.
  const deletable = workspaces.length > 1 ? onDeleteWorkspace : undefined;
  return (
    <div className="flex-1 flex flex-col items-center justify-center overflow-y-auto terminal-scroll p-8">
      <div className="font-mono text-xs text-cyan-700 mb-5">$ agent fleet status — 0 active</div>
      <h1 className="font-heading font-bold tracking-[0.3em] text-4xl md:text-5xl text-cyan-200 text-center drop-shadow-[0_0_18px_rgba(0,229,255,0.35)]">
        AGENT COMMAND CENTER<span className="inline-block w-3 h-9 bg-cyan-300 animate-pulse align-baseline ml-2" />
      </h1>
      <p className="mt-4 font-mono text-sm text-cyan-500 text-center max-w-xl leading-relaxed">
        Spin up interactive coding agents in isolated git worktrees, watch their
        terminals live, and answer whoever needs you first.
      </p>
      <div className="mt-9 flex flex-wrap justify-center gap-4">
        <button
          onClick={onNewAgent}
          className="flex items-center gap-2 px-6 py-3 bg-cyan-400/15 border-2 border-cyan-400 text-cyan-300 font-heading tracking-[0.2em] text-sm font-bold shadow-[0_0_14px_rgba(0,229,255,0.3)] hover:bg-cyan-400/25 transition-colors"
        >
          <Plus className="w-4 h-4" /> SPAWN FIRST AGENT
        </button>
        <button
          onClick={onTemplates}
          className="flex items-center gap-2 px-6 py-3 border border-cyan-800 text-cyan-500 font-heading tracking-[0.2em] text-sm font-bold hover:border-cyan-400 hover:text-cyan-300 transition-colors"
        >
          <LayoutTemplate className="w-4 h-4" /> LAUNCH A TEMPLATE
        </button>
        <button
          onClick={onLoadDemo}
          className="flex items-center gap-2 px-6 py-3 border border-cyan-900 text-cyan-700 font-heading tracking-[0.2em] text-sm font-bold hover:border-cyan-500 hover:text-cyan-300 transition-colors"
        >
          <Zap className="w-4 h-4" /> LOAD DEMO FLEET
        </button>
      </div>
      <div className="mt-11 w-full max-w-3xl">
        <div className="font-heading text-xs tracking-[0.35em] text-cyan-600 font-bold text-center mb-3">SELECT WORKSPACE</div>
        <div className="flex flex-wrap justify-center gap-2">
          {workspaces.map((w) => (
            <WorkspaceTile key={w.id} ws={w} chip active={activeId === w.id} onSelect={onSelectWorkspace} onRename={onRenameWorkspace} onDelete={deletable} />
          ))}
          <button
            onClick={onAddWorkspace}
            className="flex items-center gap-1.5 px-4 py-2 border border-dashed border-cyan-800 text-cyan-600 font-heading text-[11px] font-bold tracking-[0.2em] hover:border-cyan-400 hover:text-cyan-300 transition-colors"
          >
            <Plus className="w-3 h-3" /> NEW WORKSPACE
          </button>
        </div>
      </div>
      <div className="mt-11 w-full max-w-3xl">
        <div className="font-heading text-xs tracking-[0.35em] text-cyan-600 font-bold text-center mb-3">SUPPORTED HARNESSES</div>
        <div className="grid grid-cols-2 sm:grid-cols-4 gap-2">
          {KIND_IDS.map((k) => (
            <div key={k} className="border border-cyan-900/70 bg-[#0A1219] px-3 py-2.5 text-center hover:border-cyan-600 transition-colors">
              <div className="font-heading text-[11px] font-bold tracking-[0.2em] text-cyan-300">{AGENT_KINDS[k].label}</div>
              <div className="font-mono text-[10px] text-cyan-700 mt-0.5">$ {AGENT_KINDS[k].cmd}</div>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}