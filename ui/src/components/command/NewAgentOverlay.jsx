import React, { useState } from "react";
import { X, Rocket } from "lucide-react";
import { AGENT_KINDS, KIND_IDS } from "@/lib/agentTypes";

const inputCls = "w-full bg-[#081019] border border-cyan-800 text-cyan-200 font-mono text-sm px-3 py-2 focus:outline-none focus:border-cyan-400";
const labelCls = "font-heading text-[10px] tracking-[0.3em] text-cyan-600 font-bold";

// F-ORCH-1: closed role-cast presets. Selecting one sets the SAME `role` value the free-text
// field edits, so it is a quick-set, not a second source of truth. The value is the pane's
// registry role stamp (set_pane_roles) — what the queue / orchestrator / broker see — so it has
// a REAL coordination effect (advisory persona, not a fake toggle). Custom roles still allowed.
const ROLE_CAST = ["coordinator", "builder", "scout", "reviewer"];

export default function NewAgentOverlay({ onLaunch, onClose }) {
  const [role, setRole] = useState("");
  const [kind, setKind] = useState("claude-code");
  const [priority, setPriority] = useState("normal");
  const [autonomy, setAutonomy] = useState("semi");

  const launch = () => {
    if (!role.trim()) return;
    onLaunch({ role: role.trim(), kind, priority, autonomy });
    onClose();
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 backdrop-blur-sm" onClick={onClose}>
      <div className="w-full max-w-lg bg-[#0A1219] border-2 border-cyan-400/70 shadow-[0_0_30px_rgba(0,229,255,0.25)]" onClick={(e) => e.stopPropagation()}>
        <div className="flex items-center justify-between px-4 py-3 border-b border-cyan-800">
          <span className="font-heading font-bold tracking-[0.25em] text-cyan-300 text-sm">SPAWN NEW AGENT</span>
          <button type="button" onClick={onClose} className="text-cyan-700 hover:text-cyan-300"><X className="w-4 h-4" /></button>
        </div>
        <div className="p-4 space-y-4">
          <div className="space-y-1.5">
            <div className={labelCls}>ROLE / MISSION · <span className="text-cyan-800">free override</span></div>
            <input
              autoFocus
              value={role}
              onChange={(e) => setRole(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && launch()}
              placeholder="pick a cast below, or type a custom role"
              className={inputCls}
            />
          </div>
          <div className="space-y-1.5">
            <div className={labelCls}>ROLE CAST · <span className="text-cyan-800">advisory persona stamp</span></div>
            <div className="grid grid-cols-2 sm:grid-cols-4 gap-1.5">
              {ROLE_CAST.map((c) => (
                <button
                  key={c}
                  type="button"
                  onClick={() => setRole(c)}
                  aria-pressed={role.toLowerCase() === c}
                  title={`Set role to ${c} — stamped on the pane and read by the queue / orchestrator (advisory)`}
                  className={`px-2 py-2 border font-heading text-[10px] font-bold tracking-[0.15em] uppercase transition-colors ${
                    role.toLowerCase() === c
                      ? "border-cyan-400 bg-cyan-400/15 text-cyan-200 shadow-[0_0_8px_rgba(0,229,255,0.3)]"
                      : "border-cyan-900 text-cyan-700 hover:border-cyan-500 hover:text-cyan-300"
                  }`}
                >
                  {c}
                </button>
              ))}
            </div>
          </div>
          <div className="space-y-1.5">
            <div className={labelCls}>HARNESS</div>
            <div className="grid grid-cols-2 sm:grid-cols-4 gap-1.5">
              {KIND_IDS.map((k) => (
                <button
                  key={k}
                  type="button"
                  onClick={() => setKind(k)}
                  className={`px-2 py-2 border font-heading text-[10px] font-bold tracking-[0.15em] transition-colors ${
                    kind === k
                      ? "border-cyan-400 bg-cyan-400/15 text-cyan-200 shadow-[0_0_8px_rgba(0,229,255,0.3)]"
                      : "border-cyan-900 text-cyan-700 hover:border-cyan-500 hover:text-cyan-300"
                  }`}
                >
                  {AGENT_KINDS[k].label}
                  <div className="font-mono text-[9px] text-cyan-700 mt-0.5 normal-case tracking-normal">$ {AGENT_KINDS[k].cmd}</div>
                </button>
              ))}
            </div>
          </div>
          <div className="grid grid-cols-2 gap-3">
            {/* F-OBS-4 (M1 / R3): PRIORITY + AUTONOMY have NO backend effect — Tauri
                spawn_workspace accepts id/harness/repo/role only. Shown disabled with a
                PENDING BACKEND tag rather than removed, so the fields exist the day the
                contract grows them (NEEDS-BACKEND; SYNTHESIS §2 V-B1). */}
            <div className="space-y-1.5" title="No backend effect yet — spawn_workspace takes id / harness / repo / role only (NEEDS-BACKEND)">
              <div className={labelCls}>PRIORITY · <span className="text-cyan-800">PENDING BACKEND</span></div>
              <select value={priority} onChange={(e) => setPriority(e.target.value)} disabled className={inputCls + " opacity-50 cursor-not-allowed"}>
                <option value="low">LOW</option>
                <option value="normal">NORMAL</option>
                <option value="high">HIGH</option>
              </select>
            </div>
            <div className="space-y-1.5" title="No backend effect yet — spawn_workspace takes id / harness / repo / role only (NEEDS-BACKEND)">
              <div className={labelCls}>AUTONOMY · <span className="text-cyan-800">PENDING BACKEND</span></div>
              <select value={autonomy} onChange={(e) => setAutonomy(e.target.value)} disabled className={inputCls + " opacity-50 cursor-not-allowed"}>
                <option value="supervised">SUPERVISED</option>
                <option value="semi">SEMI-AUTONOMOUS</option>
                <option value="full">FULL AUTONOMY</option>
              </select>
            </div>
          </div>
          <button
            type="button"
            onClick={launch}
            disabled={!role.trim()}
            className="w-full flex items-center justify-center gap-2 px-5 py-2.5 bg-cyan-400/15 border-2 border-cyan-400 text-cyan-300 font-heading tracking-[0.2em] text-sm font-bold shadow-[0_0_14px_rgba(0,229,255,0.3)] hover:bg-cyan-400/25 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
          >
            <Rocket className="w-4 h-4" /> SPAWN IN NEW WORKTREE
          </button>
        </div>
      </div>
    </div>
  );
}