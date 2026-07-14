import React, { useState } from "react";
import { Plus, X, Save } from "lucide-react";
import { AGENT_KINDS, KIND_IDS } from "@/lib/agentTypes";

const emptyRow = () => ({ role: "", kind: "claude-code", priority: "normal", autonomy: "semi" });

const inputCls = "bg-[#081019] border border-cyan-900 text-cyan-200 font-mono text-xs px-2 py-1.5 focus:border-cyan-400 focus:outline-none w-full";

export default function TemplateBuilder({ onSave, saving }) {
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [rows, setRows] = useState([emptyRow()]);

  const setRow = (i, patch) => setRows((prev) => prev.map((r, idx) => (idx === i ? { ...r, ...patch } : r)));
  const valid = name.trim() && rows.length > 0 && rows.every((r) => r.role.trim());

  return (
    <div className="p-4 space-y-3 overflow-y-auto terminal-scroll max-h-[55vh]">
      <input value={name} onChange={(e) => setName(e.target.value)} placeholder="TEMPLATE NAME" className={inputCls} />
      <input value={description} onChange={(e) => setDescription(e.target.value)} placeholder="DESCRIPTION (optional)" className={inputCls} />
      <div className="font-mono text-[11px] text-cyan-600">// agents in this team</div>
      {rows.map((row, i) => (
        <div key={i} className="flex gap-2 items-center">
          <input value={row.role} onChange={(e) => setRow(i, { role: e.target.value })} placeholder="ROLE (e.g. reviewer)" className={inputCls} />
          <select value={row.kind} onChange={(e) => setRow(i, { kind: e.target.value })} className={inputCls}>
            {KIND_IDS.map((k) => (
              <option key={k} value={k}>{AGENT_KINDS[k].label}</option>
            ))}
          </select>
          <select value={row.priority} onChange={(e) => setRow(i, { priority: e.target.value })} className={inputCls}>
            <option value="low">LOW</option>
            <option value="normal">NORMAL</option>
            <option value="high">HIGH</option>
          </select>
          <select value={row.autonomy} onChange={(e) => setRow(i, { autonomy: e.target.value })} className={inputCls}>
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
          onClick={() => onSave({ name: name.trim(), description: description.trim(), agents: rows })}
          disabled={!valid || saving}
          className="ml-auto flex items-center gap-1.5 px-5 py-2 bg-cyan-400/15 border border-cyan-400 text-cyan-300 font-heading font-bold tracking-[0.15em] text-xs hover:bg-cyan-400/25 disabled:opacity-40 disabled:cursor-not-allowed"
        >
          <Save className="w-3.5 h-3.5" /> {saving ? "SAVING..." : "SAVE TEMPLATE"}
        </button>
      </div>
    </div>
  );
}