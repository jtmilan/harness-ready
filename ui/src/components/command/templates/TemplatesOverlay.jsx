import React, { useState, useEffect } from "react";
import { X } from "lucide-react";
import { base44 } from "@/api/base44Client";
import TemplateList from "@/components/command/templates/TemplateList";
import TemplateBuilder from "@/components/command/templates/TemplateBuilder";
import { coerceTemplateAgents } from "@/components/command/templates/templateAgents";
import { buildExportBundle, parseImportPayload, downloadJSON } from "@/components/command/templates/templateIO";
import { toast } from "@/components/ui/use-toast";

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

  // F-TPL-2: export the current library as a versioned bundle; import appends validated
  // templates with FRESH ids (create regenerates id/timestamps) so an import never clobbers
  // an existing local template — portable across machines, honest about what it skipped.
  const handleExport = () => {
    if (!templates.length) return;
    const stamp = new Date().toISOString().slice(0, 10);
    downloadJSON(
      `harness-ready-templates-${stamp}.json`,
      JSON.stringify(buildExportBundle(templates), null, 2)
    );
  };
  const handleImportText = async (text) => {
    // Parse is wrapped so a malformed payload never rejects unhandled; create() failures are
    // caught per-row so a partial import is reported (not silently lost) and the list always
    // refreshes to show whatever landed. Destructive = total failure; default = success (with
    // an optional "skipped invalid rows" note, which is a warning, not an error).
    let r;
    try {
      r = parseImportPayload(text);
    } catch (e) {
      toast({ title: "Import failed", description: e?.message || "could not parse file", variant: "destructive" });
      return;
    }
    if (!r.ok) {
      toast({ title: "Import failed", description: r.errors.join(" · "), variant: "destructive" });
      return;
    }
    let imported = 0;
    let writeError = null;
    for (const t of r.templates) {
      try {
        await base44.entities.AgentTemplate.create(t);
        imported += 1;
      } catch (e) {
        writeError = e;
        break;
      }
    }
    await load();
    if (writeError) {
      toast({ title: `Imported ${imported}, then stopped`, description: `write failed: ${writeError?.message || writeError}`, variant: "destructive" });
      return;
    }
    if (imported === 0) {
      toast({
        title: "Import failed — no valid templates",
        description: r.skipped.length ? r.skipped.map((s) => s.error).join(" · ") : "nothing to import",
        variant: "destructive",
      });
      return;
    }
    toast({
      title: `Imported ${imported} template(s)`,
      description: r.skipped.length
        ? `${r.skipped.length} invalid row(s) skipped: ${r.skipped.map((s) => s.error).join(" · ")}`
        : "stored locally",
    });
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
          {/* F-TPL-2 / R6: storage is local (localStorage) — say so; no cloud dependency. */}
          <span className="px-2 py-0.5 border border-cyan-800/70 text-[9px] font-mono tracking-[0.15em] text-cyan-600">LOCAL · localStorage</span>
          <div className="ml-4 flex gap-1">
            <button className={tabCls(view === "list")} onClick={() => setView("list")}>LAUNCH</button>
            <button className={tabCls(view === "recipes")} onClick={() => setView("recipes")}>RECIPES</button>
            <button className={tabCls(view === "new")} onClick={() => setView("new")}>SAVE NEW</button>
          </div>
          <button onClick={onClose} className="ml-auto text-cyan-600 hover:text-cyan-300">
            <X className="w-4 h-4" />
          </button>
        </div>
        {view === "new" ? (
          <TemplateBuilder onSave={handleSave} saving={saving} />
        ) : (
          <TemplateList
            templates={view === "recipes" ? templates.filter((t) => t.playbook) : templates}
            loading={loading}
            emptyMessage={
              view === "recipes"
                ? "// no recipes yet — add a PLAYBOOK in SAVE NEW to turn a template into a recipe"
                : undefined
            }
            onExport={handleExport}
            onImportText={handleImportText}
            onLaunch={(t) => {
              // Home → bridge.spawnAgents(template.agents) assumes [{kind, role, ...}].
              // Coerce so legacy/unmapped kinds never hit K4 refuse-as-bash path.
              onLaunch({ ...t, agents: coerceTemplateAgents(t.agents) });
              onClose();
            }}
            onDelete={handleDelete}
          />
        )}
      </div>
    </div>
  );
}