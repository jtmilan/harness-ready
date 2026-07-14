import React, { useState } from "react";
import { X } from "lucide-react";

export default function CommandOverlay({ title, description, agents, requireAgent, onSubmit, onClose }) {
  const [message, setMessage] = useState("");
  const [agentId, setAgentId] = useState(agents?.[0]?.id || "");

  const submit = () => {
    if (!message.trim()) return;
    onSubmit(message.trim(), agentId);
    onClose();
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 backdrop-blur-sm" onClick={onClose}>
      <div
        className="w-full max-w-lg border-2 border-cyan-400 bg-[#0A1219] shadow-[0_0_30px_rgba(0,229,255,0.35)]"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between px-4 py-3 border-b border-cyan-800/70 bg-[#0C1720]">
          <span className="font-heading font-bold tracking-[0.25em] text-cyan-300">{title}</span>
          <button onClick={onClose} className="text-cyan-600 hover:text-cyan-300"><X className="w-4 h-4" /></button>
        </div>
        <div className="p-5 space-y-4">
          <p className="font-mono text-xs text-cyan-600">{description}</p>
          {requireAgent && (
            <select
              value={agentId}
              onChange={(e) => setAgentId(e.target.value)}
              className="w-full bg-[#0C1720] border border-cyan-800 text-cyan-300 font-mono text-sm px-3 py-2 focus:border-cyan-400 outline-none"
            >
              {agents.map((a) => <option key={a.id} value={a.id}>{a.id}</option>)}
            </select>
          )}
          <textarea
            autoFocus
            value={message}
            onChange={(e) => setMessage(e.target.value)}
            onKeyDown={(e) => { if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); submit(); } }}
            placeholder="> enter command..."
            rows={3}
            className="w-full bg-[#0C1720] border border-cyan-800 text-cyan-200 font-mono text-sm px-3 py-2 focus:border-cyan-400 outline-none resize-none placeholder:text-cyan-800"
          />
          <button
            onClick={submit}
            disabled={!message.trim()}
            className="w-full py-2.5 bg-cyan-400/15 border border-cyan-400 text-cyan-300 font-heading font-bold tracking-[0.2em] text-sm hover:bg-cyan-400/25 disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
          >
            EXECUTE
          </button>
        </div>
      </div>
    </div>
  );
}