import React, { useState, useEffect } from "react";
import { X } from "lucide-react";
import { base44 } from "@/api/base44Client";
import TemplateList from "@/components/command/templates/TemplateList";
import TemplateBuilder from "@/components/command/templates/TemplateBuilder";

export default function TemplatesOverlay({ onLaunch, onClose }) {
  const [templates, setTemplates] = useState([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [view, setView] = useState("list"); // 'list' | 'new'

  const load = async () => {
    setLoading(true);
    const data = await base44.entities.AgentTemplate.list("-created_date");
    setTemplates(data);
    setLoading(false);
  };

  useEffect(() => { load(); }, []);

  const handleSave = async (data) => {
    setSaving(true);
    await base44.entities.AgentTemplate.create(data);
    setSaving(false);
    setView("list");
    load();
  };

  const handleDelete = async (id) => {
    setTemplates((prev) => prev.filter((t) => t.id !== id));
    await base44.entities.AgentTemplate.delete(id);
  };

  const tabCls = (active) =>
    `px-4 py-1.5 font-heading font-bold tracking-[0.2em] text-xs transition-colors ${
      active ? "bg-cyan-400/15 text-cyan-200 border border-cyan-400/60" : "text-cyan-700 border border-transparent hover:text-cyan-300"
    }`;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 backdrop-blur-sm" onClick={onClose}>
      <div
        className="w-full max-w-3xl mx-4 border-2 border-cyan-400/60 bg-[#0A1219] shadow-[0_0_30px_rgba(0,229,255,0.25)]"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center gap-3 px-4 py-3 border-b border-cyan-800/70 bg-[#0C1720]">
          <span className="font-heading font-bold tracking-[0.25em] text-cyan-300 text-sm">TEAM TEMPLATES</span>
          <div className="ml-4 flex gap-1">
            <button className={tabCls(view === "list")} onClick={() => setView("list")}>LAUNCH</button>
            <button className={tabCls(view === "new")} onClick={() => setView("new")}>SAVE NEW</button>
          </div>
          <button onClick={onClose} className="ml-auto text-cyan-600 hover:text-cyan-300">
            <X className="w-4 h-4" />
          </button>
        </div>
        {view === "list" ? (
          <TemplateList templates={templates} loading={loading} onLaunch={(t) => { onLaunch(t); onClose(); }} onDelete={handleDelete} />
        ) : (
          <TemplateBuilder onSave={handleSave} saving={saving} />
        )}
      </div>
    </div>
  );
}