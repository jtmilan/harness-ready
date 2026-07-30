import React, { useRef } from "react";
import { Rocket, Trash2, Download, Upload } from "lucide-react";
import { toast } from "@/components/ui/use-toast";

export default function TemplateList({ templates, loading, onLaunch, onDelete, onExport, onImportText, emptyMessage }) {
  const fileRef = useRef(null);
  const onPickFile = (e) => {
    const file = e.target.files && e.target.files[0];
    if (!file) return;
    // file.text() is a modern Promise-based read; no new dep. This catch covers ONLY the
    // file-read failure (a non-text/unreadable file); handleImportText handles its own
    // parse/write errors and never rejects, so there is no double-toast.
    file
      .text()
      .then((text) => onImportText && onImportText(text))
      .catch(() => toast({ title: "Import error", description: "could not read the selected file", variant: "destructive" }));
    e.target.value = ""; // reset so the same file can be re-selected (onChange won't fire otherwise)
  };

  // F-TPL-2: import/export toolbar. Import is always available (append-without-clobber);
  // Export is disabled on an empty library. Both are honest local operations (R6).
  const toolbar = (
    <div className="flex items-center gap-2 px-3 pt-3">
      <span className="font-mono text-[10px] text-cyan-700">{loading ? "…" : `${templates.length} saved`}</span>
      <div className="ml-auto flex items-center gap-2">
        <input ref={fileRef} type="file" accept="application/json,.json" className="hidden" onChange={onPickFile} />
        <button
          onClick={() => fileRef.current && fileRef.current.click()}
          title="Import templates from a versioned JSON file — appended locally; existing templates are never overwritten"
          className="flex items-center gap-1.5 px-3 py-1.5 border border-cyan-800 text-cyan-500 font-heading font-bold tracking-[0.15em] text-[11px] hover:border-cyan-400 hover:text-cyan-300 transition-colors"
        >
          <Upload className="w-3.5 h-3.5" /> IMPORT
        </button>
        <button
          onClick={() => onExport && onExport()}
          disabled={!templates.length}
          title="Export all templates as versioned JSON"
          className="flex items-center gap-1.5 px-3 py-1.5 border border-cyan-800 text-cyan-500 font-heading font-bold tracking-[0.15em] text-[11px] hover:border-cyan-400 hover:text-cyan-300 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
        >
          <Download className="w-3.5 h-3.5" /> EXPORT
        </button>
      </div>
    </div>
  );

  if (loading) {
    return (
      <div>
        {toolbar}
        <div className="p-6 font-mono text-xs text-cyan-600 animate-pulse">// loading templates ...</div>
      </div>
    );
  }
  if (templates.length === 0) {
    return (
      <div>
        {toolbar}
        <div className="p-6 font-mono text-xs text-cyan-700">{emptyMessage || "// no templates saved yet — IMPORT a bundle or switch to SAVE NEW"}</div>
      </div>
    );
  }
  return (
    <div>
      {toolbar}
      <div className="p-3 space-y-2 overflow-y-auto terminal-scroll max-h-[50vh]">
        {templates.map((t) => (
          <div key={t.id} className="border border-cyan-800/70 bg-[#0C1720] p-3 flex items-center gap-3">
            <div className="flex-1 min-w-0">
              <div className="font-heading font-bold tracking-[0.15em] text-sm text-cyan-200">{t.name}</div>
              {t.description && <div className="font-mono text-[11px] text-cyan-600 truncate">{t.description}</div>}
              <div className="font-mono text-[11px] text-cyan-500 mt-1">
                {t.agents.length} agent(s): {t.agents.map((a) => a.role).join(", ")}
              </div>
              {t.playbook && (
                <>
                  <div className="mt-1 inline-flex items-center px-1.5 py-0.5 border border-amber-500/50 text-[9px] font-mono tracking-[0.15em] text-amber-300/90">RECIPE</div>
                  <div className="font-mono text-[11px] text-amber-200/70 mt-1">// playbook: {t.playbook.slice(0, 120)}{t.playbook.length > 120 ? "…" : ""}</div>
                </>
              )}
              {t.recommended && (t.recommended.autonomy || t.recommended.priority) && (
                <div className="font-mono text-[10px] text-cyan-600 mt-0.5">
                  recommended (advisory):{t.recommended.autonomy ? ` autonomy=${t.recommended.autonomy}` : ""}{t.recommended.priority ? ` priority=${t.recommended.priority}` : ""}
                </div>
              )}
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
    </div>
  );
}
