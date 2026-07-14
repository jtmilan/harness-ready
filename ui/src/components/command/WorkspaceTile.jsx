import React, { useState } from "react";
import { Pencil } from "lucide-react";

// Selectable + renameable workspace. `chip` renders the compact landing-page style.
export default function WorkspaceTile({ ws, active, chip, onSelect, onRename }) {
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState(ws.name);

  const commit = () => {
    const v = name.trim();
    if (v) onRename(ws.id, v.toUpperCase());
    setEditing(false);
  };

  const base = chip
    ? "px-4 py-2 border font-heading text-[11px] font-bold tracking-[0.2em] transition-colors"
    : "w-full px-3 py-4 font-heading font-bold tracking-[0.1em] text-xs text-left transition-all";
  const activeCls = chip
    ? "border-cyan-400 bg-cyan-400/15 text-cyan-200 shadow-[0_0_8px_rgba(0,229,255,0.3)]"
    : "bg-cyan-300 text-[#0A1219] shadow-[0_0_14px_rgba(0,229,255,0.5)]";
  const idleCls = chip
    ? "border-cyan-900 text-cyan-700 hover:border-cyan-500 hover:text-cyan-300"
    : "border border-cyan-800/70 text-cyan-500 hover:border-cyan-400 hover:text-cyan-300";

  if (editing) {
    return (
      <input
        autoFocus
        value={name}
        onChange={(e) => setName(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") commit();
          if (e.key === "Escape") { setName(ws.name); setEditing(false); }
        }}
        className={`${base} bg-[#0C1720] border border-cyan-400 text-cyan-200 outline-none uppercase`}
      />
    );
  }

  return (
    <div className="relative group">
      <button onClick={() => onSelect(ws.id)} className={`${base} ${active ? activeCls : idleCls} pr-7 truncate`}>
        {ws.name}
      </button>
      <button
        onClick={() => { setName(ws.name); setEditing(true); }}
        title="Rename workspace"
        className={`absolute top-1/2 -translate-y-1/2 right-1.5 opacity-0 group-hover:opacity-100 transition-opacity ${
          active && !chip ? "text-[#0A1219]" : "text-cyan-600 hover:text-cyan-300"
        }`}
      >
        <Pencil className="w-3 h-3" />
      </button>
    </div>
  );
}