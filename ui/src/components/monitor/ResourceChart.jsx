import React from "react";
import { AreaChart, Area, XAxis, YAxis, CartesianGrid, Tooltip, ResponsiveContainer, Legend } from "recharts";

const tooltipStyle = {
  backgroundColor: "#0A1219",
  border: "1px solid #164e63",
  fontFamily: "JetBrains Mono, monospace",
  fontSize: 11,
  color: "#67e8f9",
};

export default function ResourceChart({ data }) {
  return (
    <div className="border border-cyan-800/70 bg-[#0A1219]">
      <div className="px-4 py-2.5 border-b border-cyan-800/70 bg-[#0C1720] font-heading font-bold tracking-[0.2em] text-sm text-cyan-300">
        FLEET RESOURCE USAGE — LIVE
      </div>
      <div className="p-4 h-72">
        <ResponsiveContainer width="100%" height="100%">
          <AreaChart data={data}>
            <defs>
              <linearGradient id="cpuGrad" x1="0" y1="0" x2="0" y2="1">
                <stop offset="0%" stopColor="#00E5FF" stopOpacity={0.35} />
                <stop offset="100%" stopColor="#00E5FF" stopOpacity={0} />
              </linearGradient>
              <linearGradient id="memGrad" x1="0" y1="0" x2="0" y2="1">
                <stop offset="0%" stopColor="#fde047" stopOpacity={0.3} />
                <stop offset="100%" stopColor="#fde047" stopOpacity={0} />
              </linearGradient>
            </defs>
            <CartesianGrid strokeDasharray="3 3" stroke="#12303d" />
            <XAxis dataKey="time" tick={{ fill: "#155e75", fontSize: 10, fontFamily: "monospace" }} interval="preserveStartEnd" minTickGap={40} />
            <YAxis domain={[0, 100]} unit="%" tick={{ fill: "#155e75", fontSize: 10, fontFamily: "monospace" }} width={45} />
            <Tooltip contentStyle={tooltipStyle} />
            <Legend wrapperStyle={{ fontFamily: "monospace", fontSize: 11, color: "#67e8f9" }} />
            <Area type="monotone" dataKey="cpu" name="CPU" stroke="#00E5FF" strokeWidth={2} fill="url(#cpuGrad)" isAnimationActive={false} />
            <Area type="monotone" dataKey="mem" name="MEMORY" stroke="#fde047" strokeWidth={2} fill="url(#memGrad)" isAnimationActive={false} />
            <Area type="monotone" dataKey="net" name="NETWORK" stroke="#34d399" strokeWidth={1.5} fill="none" isAnimationActive={false} />
          </AreaChart>
        </ResponsiveContainer>
      </div>
    </div>
  );
}