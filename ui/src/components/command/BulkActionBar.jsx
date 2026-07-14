import React from "react";
import { Pause, RotateCcw, Megaphone, X } from "lucide-react";

const ActionBtn = ({ icon: Icon, label, onClick }) => (
  <button
    onClick={onClick}
    className="flex items-center gap-2 px-4 py-2 border border-cyan-800 text-cyan-400 font-heading font-bold tracking-[0.15em] text-xs hover:border-cyan-400 hover:text-cyan-200 hover:bg-cyan-400/10 transition-colors"
  >
    <Icon className="w-3.5 h-3.5" /> {label}
  </button>
);

export default function BulkActionBar({ count, onPause, onRestart, onBroadcast, onClear }) {
  return (
    <div className="fixed bottom-72 left-1/2 -translate-x-1/2 z-40 flex items-center gap-3 px-4 py-3 bg-[#0A1219] border-2 border-cyan-400 shadow-[0_0_24px_rgba(0,229,255,0.4)]">
      <span className="font-mono text-xs text-cyan-300 font-bold pr-2 border-r border-cyan-800">
        {count} SELECTED
      </span>
      <ActionBtn icon={Pause} label="PAUSE" onClick={onPause} />
      <ActionBtn icon={RotateCcw} label="RESTART" onClick={onRestart} />
      <ActionBtn icon={Megaphone} label="BROADCAST" onClick={onBroadcast} />
      <button onClick={onClear} title="Clear selection" className="text-cyan-700 hover:text-cyan-300 pl-1">
        <X className="w-4 h-4" />
      </button>
    </div>
  );
}