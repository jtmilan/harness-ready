import React from "react";
import { Rocket, Trash2 } from "lucide-react";

export default function TemplateList({ templates, loading, onLaunch, onDelete }) {
  if (loading) {
    return <div className="p-6 font-mono text-xs text-cyan-600 animate-pulse">// loading templates ...</div>;
  }
  if (templates.length === 0) {
    return <div className="p-6 font-mono text-xs text-cyan-700">// no templates saved yet — switch to SAVE NEW to create one</div>;
  }
  return (
    <div className="p-3 space-y-2 overflow-y-auto terminal-scroll max-h-[50vh]">
      {templates.map((t) => (
        <div key={t.id} className="border border-cyan-800/70 bg-[#0C1720] p-3 flex items-center gap-3">
          <div className="flex-1 min-w-0">
            <div className="font-heading font-bold tracking-[0.15em] text-sm text-cyan-200">{t.name}</div>
            {t.description && <div className="font-mono text-[11px] text-cyan-600 truncate">{t.description}</div>}
            <div className="font-mono text-[11px] text-cyan-500 mt-1">
              {t.agents.length} agent(s): {t.agents.map((a) => a.role).join(", ")}
            </div>
          </div>
          <button
            onClick={() => onLaunch(t)}
            className="flex items-center gap-1.5 px-4 py-2 bg-cyan-400/15 border border-cyan-400 text-cyan-300 font-heading font-bold tracking-[0.15em] text-xs hover:bg-cyan-400/25 transition-colors"
          >
            <Rocket className="w-3.5 h-3.5" /> LAUNCH
          </button>
          <button
            onClick={() => onDelete(t.id)}
            title="Delete template"
            className="p-2 border border-cyan-900 text-cyan-700 hover:border-red-500 hover:text-red-400 transition-colors"
          >
            <Trash2 className="w-3.5 h-3.5" />
          </button>
        </div>
      ))}
    </div>
  );
}