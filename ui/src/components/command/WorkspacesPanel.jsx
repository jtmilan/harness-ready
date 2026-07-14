import React from "react";
import { Plus } from "lucide-react";
import WorkspaceTile from "@/components/command/WorkspaceTile";

export default function WorkspacesPanel({ workspaces, activeId, onSelect, onAdd, onRename }) {
  return (
    <div className="border border-cyan-800/70 bg-[#0A1219] flex flex-col min-h-0">
      <div className="px-3 py-2 border-b border-cyan-800/70 bg-[#0C1720] font-heading font-bold tracking-[0.2em] text-sm text-cyan-300 flex items-center justify-between">
        WORKSPACES
        <button onClick={onAdd} title="New workspace" className="text-cyan-600 hover:text-cyan-300 transition-colors">
          <Plus className="w-4 h-4" />
        </button>
      </div>
      <div className="p-3 grid grid-cols-3 gap-2.5 content-start overflow-y-auto terminal-scroll">
        {workspaces.map((ws) => (
          <WorkspaceTile key={ws.id} ws={ws} active={activeId === ws.id} onSelect={onSelect} onRename={onRename} />
        ))}
      </div>
    </div>
  );
}