import React from "react";
import { STATUS_META } from "@/lib/agentData";

export default function AgentDirectory({ agents, selectedId, onSelect }) {
  return (
    <div className="border border-cyan-800/70 bg-[#0A1219] flex flex-col min-h-0">
      <div className="px-3 py-2 border-b border-cyan-800/70 bg-[#0C1720] font-heading font-bold tracking-[0.2em] text-sm text-cyan-300">
        AGENT DIRECTORY
      </div>
      <div className="overflow-y-auto terminal-scroll flex-1 p-1.5">
        {agents.map((agent) => {
          const meta = STATUS_META[agent.status];
          return (
            <button
              key={agent.id}
              onClick={() => onSelect(agent.id)}
              className={`w-full flex items-center justify-between px-2.5 py-1.5 font-mono text-xs transition-colors ${
                selectedId === agent.id
                  ? "bg-cyan-400/15 text-cyan-200 border border-cyan-400/60"
                  : "text-cyan-500 border border-transparent hover:bg-cyan-400/5 hover:text-cyan-300"
              }`}
            >
              <span className="truncate">{agent.name || agent.id}</span>
              <span className={`font-bold ${meta.color}`}>({meta.badge})</span>
            </button>
          );
        })}
      </div>
    </div>
  );
}