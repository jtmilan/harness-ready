import React, { useState } from "react";
import { Pencil, Trash2, Shield, ShieldOff } from "lucide-react";

// Selectable + renameable + deletable workspace. `chip` renders the compact
// landing-page style. Delete button only renders when `onDelete` is provided —
// callers omit it for the last remaining workspace.
// Phase 1 sharing: always-visible footer strip with Shield toggle + badge. The
// strip is NEVER hover-gated — workspace isolation is a security boundary and
// the operator must see its state at rest, not discover it on hover.
export default function WorkspaceTile({ ws, active, chip, onSelect, onRename, onDelete, memberCount, onToggleSharing }) {
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState(ws.name);
  const sharing = !!ws.allow_sharing;

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
    : sharing
      ? "bg-amber-400 text-[#0A1219] shadow-[0_0_14px_rgba(251,191,36,0.45)]"
      : "bg-cyan-300 text-[#0A1219] shadow-[0_0_14px_rgba(0,229,255,0.5)]";
  const idleCls = chip
    ? "border-cyan-900 text-cyan-700 hover:border-cyan-500 hover:text-cyan-300"
    : sharing
      ? "border border-amber-500/70 text-cyan-500 hover:border-amber-400 hover:text-cyan-300"
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

  const iconCls = active && !chip ? (sharing ? "text-[#0A1219]" : "text-[#0A1219]") : "text-cyan-600 hover:text-cyan-300";

  // Non-chip layout: tile button + ALWAYS-VISIBLE sharing footer. The footer
  // wraps: Shield/ShieldOff toggle button, a fixed "SHARING" label, the live
  // member count, and an ISOLATED/SHARED badge. NOT hover-gated — the isolation
  // boundary is a security surface and its state must be visible at rest.
  if (!chip) {
    return (
      <div className="relative group flex flex-col">
        <button onClick={() => onSelect(ws.id)} className={`${base} ${active ? activeCls : idleCls} ${onDelete ? "pr-12" : "pr-7"} truncate`}>
          {ws.name}
        </button>
        <div className="absolute top-1/2 -translate-y-1/2 right-1.5 flex items-center gap-1.5 opacity-0 group-hover:opacity-100 transition-opacity">
          <button
            onClick={() => { setName(ws.name); setEditing(true); }}
            title="Rename workspace"
            className={iconCls}
          >
            <Pencil className="w-3 h-3" />
          </button>
          {onDelete && (
            <button
              onClick={() => onDelete(ws.id)}
              title="Delete workspace"
              className={active ? (sharing ? "text-[#0A1219] hover:text-red-700" : "text-[#0A1219] hover:text-red-700") : "text-cyan-600 hover:text-red-400"}
            >
              <Trash2 className="w-3 h-3" />
            </button>
          )}
        </div>
        {/* Sharing footer — always visible. Border-top matches the tile's border
            (amber for shared, cyan-800 for isolated) so the strip reads as a
            continuous surface, not a tacked-on extra. */}
        {onToggleSharing && (
          <div
            className={
              "flex items-center justify-between gap-2 px-2 py-1 border-t " +
              (sharing ? "border-amber-500/70 bg-amber-500/5" : "border-cyan-800/70 bg-[#0A1219]")
            }
          >
            <button
              type="button"
              onClick={(e) => { e.stopPropagation(); onToggleSharing(ws.id, !sharing); }}
              title={sharing ? "Disable cross-workspace sharing" : "Enable cross-workspace sharing"}
              className={
                "flex items-center gap-1 font-heading text-[9px] tracking-[0.2em] font-bold transition-colors " +
                (sharing ? "text-amber-400 hover:text-amber-300" : "text-cyan-600 hover:text-cyan-300")
              }
            >
              {sharing ? <ShieldOff className="w-3 h-3" /> : <Shield className="w-3 h-3" />}
              <span className="uppercase">sharing</span>
            </button>
            <span className="font-mono text-[9px] text-cyan-700/80">
              {memberCount ?? 0} agent{memberCount === 1 ? "" : "s"}
            </span>
            <span
              className={
                "font-heading text-[9px] tracking-[0.2em] font-bold " +
                (sharing ? "text-amber-400" : "text-cyan-700/80")
              }
            >
              {sharing ? "SHARED" : "ISOLATED"}
            </span>
          </div>
        )}
      </div>
    );
  }

  // Chip variant (landing-page compact style) — no sharing footer; the chip is
  // a selection affordance, not a workspace management surface.
  return (
    <div className="relative group">
      <button onClick={() => onSelect(ws.id)} className={`${base} ${active ? activeCls : idleCls} ${onDelete ? "pr-12" : "pr-7"} truncate`}>
        {ws.name}
      </button>
    </div>
  );
}