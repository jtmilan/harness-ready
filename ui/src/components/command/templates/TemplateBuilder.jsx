import React, { useState } from "react";
import { Plus, X, Save } from "lucide-react";
import { AGENT_KINDS, KIND_IDS } from "@/lib/agentTypes";
import { coerceTemplateAgents, DEFAULT_TEMPLATE_KIND } from "@/components/command/templates/templateAgents";

const emptyRow = () => ({ role: "", kind: DEFAULT_TEMPLATE_KIND, priority: "normal", autonomy: "semi" });

const inputCls = "bg-[#081019] border border-cyan-900 text-cyan-200 font-mono text-xs px-2 py-1.5 focus:border-cyan-400 focus:outline-none w-full";

export default function TemplateBuilder({ onSave, saving }) {
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [rows, setRows] = useState([emptyRow()]);
  // F-ONB-1 recipe fields: playbook = displayed guidance; recommended = advisory metadata.
  const [playbook, setPlaybook] = useState("");
  const [recPriority, setRecPriority] = useState("");
  const [recAutonomy, setRecAutonomy] = useState("");

  const setRow = (i, patch) => setRows((prev) => prev.map((r, idx) => (idx === i ? { ...r, ...patch } : r)));
  const valid = name.trim() && rows.length > 0 && rows.every((r) => r.role.trim());
  const buildPayload = () => {
    const recommended = {};
    if (recPriority) recommended.priority = recPriority;
    if (recAutonomy) recommended.autonomy = recAutonomy;
    const data = { name: name.trim(), description: description.trim(), agents: coerceTemplateAgents(rows) };
    if (playbook.trim()) data.playbook = playbook.trim();
    if (recommended.priority || recommended.autonomy) data.recommended = recommended;
    return data;
  };

  return (
    <div className="p-4 space-y-3 overflow-y-auto terminal-scroll max-h-[55vh]">
      <input value={name} onChange={(e) => setName(e.target.value)} placeholder="TEMPLATE NAME" className={inputCls} />
      <input value={description} onChange={(e) => setDescription(e.target.value)} placeholder="DESCRIPTION (optional)" className={inputCls} />
      {/* F-ONB-1: optional recipe layer. Playbook is shown to the operator on launch (never
          auto-applied); recommended autonomy/priority are ADVISORY metadata, labelled as such —
          the backend does not enforce them (NEEDS-BACKEND), so we never claim it does. */}
      <div className="space-y-1.5">
        <div className="font-mono text-[11px] text-cyan-600">// recipe (optional) — a playbook turns this template into onboarding</div>
        <textarea
          value={playbook}
          onChange={(e) => setPlaybook(e.target.value)}
          rows={3}
          placeholder={"FIRST-RUN STEPS / AUTONOMY GUIDANCE\nshown to the operator on launch · NOT auto-applied"}
          className={inputCls + " resize-y"}
        />
        <div className="grid grid-cols-2 gap-2">
          <div className="space-y-1">
            <div className="font-mono text-[9px] text-cyan-700">RECOMMENDED PRIORITY · <span className="text-amber-400/80">advisory only</span></div>
            <select value={recPriority} onChange={(e) => setRecPriority(e.target.value)} title="Stored as operator guidance; the backend does not enforce it (NEEDS-BACKEND)" className={inputCls}>
              <option value="">— none —</option>
              <option value="low">LOW</option>
              <option value="normal">NORMAL</option>
              <option value="high">HIGH</option>
            </select>
          </div>
          <div className="space-y-1">
            <div className="font-mono text-[9px] text-cyan-700">RECOMMENDED AUTONOMY · <span className="text-amber-400/80">advisory only</span></div>
            <select value={recAutonomy} onChange={(e) => setRecAutonomy(e.target.value)} title="Stored as operator guidance; the backend does not enforce it (NEEDS-BACKEND)" className={inputCls}>
              <option value="">— none —</option>
              <option value="supervised">SUPERVISED</option>
              <option value="semi">SEMI-AUTO</option>
              <option value="full">FULL AUTO</option>
            </select>
          </div>
        </div>
      </div>
      <div className="font-mono text-[11px] text-cyan-600">// agents in this team</div>
      {rows.map((row, i) => (
        <div key={i} className="flex gap-2 items-center">
          <input value={row.role} onChange={(e) => setRow(i, { role: e.target.value })} placeholder="ROLE (e.g. reviewer)" className={inputCls} />
          <select value={row.kind} onChange={(e) => setRow(i, { kind: e.target.value })} className={inputCls}>
            {KIND_IDS.map((k) => (
              <option key={k} value={k}>{AGENT_KINDS[k].label}</option>
            ))}
          </select>
          {/* F-OBS-4 (M1 / R3): priority/autonomy have no backend effect — disabled until
              spawn honors them (NEEDS-BACKEND). Defaults still ride the schema. */}
          <select value={row.priority} onChange={(e) => setRow(i, { priority: e.target.value })} disabled title="Pending backend — not honored by spawn yet" className={inputCls + " opacity-50 cursor-not-allowed"}>
            <option value="low">LOW</option>
            <option value="normal">NORMAL</option>
            <option value="high">HIGH</option>
          </select>
          <select value={row.autonomy} onChange={(e) => setRow(i, { autonomy: e.target.value })} disabled title="Pending backend — not honored by spawn yet" className={inputCls + " opacity-50 cursor-not-allowed"}>
            <option value="supervised">SUPERVISED</option>
            <option value="semi">SEMI-AUTO</option>
            <option value="full">FULL AUTO</option>
          </select>
          <button
            onClick={() => setRows((prev) => prev.filter((_, idx) => idx !== i))}
            disabled={rows.length === 1}
            className="p-1.5 text-cyan-700 hover:text-red-400 disabled:opacity-30"
          >
            <X className="w-3.5 h-3.5" />
          </button>
        </div>
      ))}
      <div className="flex gap-2 pt-1">
        <button
          onClick={() => setRows((prev) => [...prev, emptyRow()])}
          className="flex items-center gap-1.5 px-3 py-2 border border-cyan-800 text-cyan-500 font-heading font-bold tracking-[0.15em] text-xs hover:border-cyan-400 hover:text-cyan-300"
        >
          <Plus className="w-3.5 h-3.5" /> ADD AGENT
        </button>
        <button
          onClick={() => onSave(buildPayload())}
          disabled={!valid || saving}
          className="ml-auto flex items-center gap-1.5 px-5 py-2 bg-cyan-400/15 border border-cyan-400 text-cyan-300 font-heading font-bold tracking-[0.15em] text-xs hover:bg-cyan-400/25 disabled:opacity-40 disabled:cursor-not-allowed"
        >
          <Save className="w-3.5 h-3.5" /> {saving ? "SAVING..." : "SAVE TEMPLATE"}
        </button>
      </div>
    </div>
  );
}