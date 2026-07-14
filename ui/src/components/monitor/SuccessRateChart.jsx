import React from "react";
import { BarChart, Bar, XAxis, YAxis, CartesianGrid, Tooltip, ResponsiveContainer, Cell } from "recharts";

const tooltipStyle = {
  backgroundColor: "#0A1219",
  border: "1px solid #164e63",
  fontFamily: "JetBrains Mono, monospace",
  fontSize: 11,
  color: "#67e8f9",
};

const barColor = (v) => (v >= 90 ? "#34d399" : v >= 75 ? "#00E5FF" : "#fde047");

export default function SuccessRateChart({ data }) {
  return (
    <div className="border border-cyan-800/70 bg-[#0A1219]">
      <div className="px-4 py-2.5 border-b border-cyan-800/70 bg-[#0C1720] font-heading font-bold tracking-[0.2em] text-sm text-cyan-300">
        TASK SUCCESS RATE — PER AGENT
      </div>
      <div className="p-4 h-72">
        <ResponsiveContainer width="100%" height="100%">
          <BarChart data={data}>
            <CartesianGrid strokeDasharray="3 3" stroke="#12303d" vertical={false} />
            <XAxis dataKey="id" tick={{ fill: "#155e75", fontSize: 9, fontFamily: "monospace" }} tickFormatter={(id) => id.replace("AGENT-", "")} />
            <YAxis domain={[0, 100]} unit="%" tick={{ fill: "#155e75", fontSize: 10, fontFamily: "monospace" }} width={45} />
            <Tooltip contentStyle={tooltipStyle} cursor={{ fill: "rgba(0,229,255,0.06)" }} formatter={(v) => [`${v}%`, "SUCCESS"]} />
            <Bar dataKey="success" isAnimationActive={false}>
              {data.map((d) => (
                <Cell key={d.id} fill={barColor(d.success)} />
              ))}
            </Bar>
          </BarChart>
        </ResponsiveContainer>
      </div>
    </div>
  );
}