import React from "react";

export default function StatCard({ icon: Icon, label, value, sub, tone = "cyan" }) {
  const tones = {
    cyan: "text-cyan-300 border-cyan-800/70",
    yellow: "text-yellow-300 border-yellow-800/50",
    green: "text-emerald-300 border-emerald-800/50",
  };
  return (
    <div className={`border bg-[#0A1219] p-4 ${tones[tone]}`}>
      <div className="flex items-center gap-2 text-cyan-600">
        <Icon className="w-4 h-4" />
        <span className="font-heading font-bold tracking-[0.2em] text-[11px]">{label}</span>
      </div>
      <div className={`mt-2 font-mono text-3xl font-bold ${tones[tone].split(" ")[0]}`}>{value}</div>
      {sub && <div className="mt-1 font-mono text-[11px] text-cyan-700">{sub}</div>}
    </div>
  );
}