import React from "react";
import { X, Power } from "lucide-react";

export default function ConfirmOverlay({ title, description, confirmLabel, onConfirm, onClose }) {
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 backdrop-blur-sm" onClick={onClose}>
      <div className="w-full max-w-md bg-[#0A1219] border-2 border-red-500/70 shadow-[0_0_30px_rgba(248,113,113,0.2)]" onClick={(e) => e.stopPropagation()}>
        <div className="flex items-center justify-between px-4 py-3 border-b border-red-900/70">
          <span className="font-heading font-bold tracking-[0.25em] text-red-300 text-sm">{title}</span>
          <button onClick={onClose} className="text-red-800 hover:text-red-300"><X className="w-4 h-4" /></button>
        </div>
        <div className="p-4 space-y-4">
          <p className="font-mono text-xs text-red-200/70 leading-relaxed">{description}</p>
          <div className="flex gap-3">
            <button
              onClick={onConfirm}
              className="flex-1 flex items-center justify-center gap-2 px-4 py-2.5 bg-red-500/15 border-2 border-red-500 text-red-300 font-heading tracking-[0.2em] text-sm font-bold hover:bg-red-500/25 transition-colors"
            >
              <Power className="w-4 h-4" /> {confirmLabel}
            </button>
            <button
              onClick={onClose}
              className="px-4 py-2.5 border border-cyan-800 text-cyan-500 font-heading tracking-[0.2em] text-sm font-bold hover:border-cyan-400 hover:text-cyan-300 transition-colors"
            >
              ABORT
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}