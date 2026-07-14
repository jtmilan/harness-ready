import React from "react";
import { AreaChart, Area, XAxis, YAxis, ResponsiveContainer, Tooltip, PieChart, Pie, Cell, Legend } from "recharts";

const STATUS_COLORS = { working: "#00E5FF", needs_input: "#fbbf24", starting: "#fde047", blocked: "#fb923c", error: "#f87171", idle: "#475569" };
const tooltipStyle = {
  backgroundColor: "#0A1219",
  border: "1px solid #164e63",
  fontFamily: "JetBrains Mono, monospace",
  fontSize: 11,
  color: "#67e8f9",
};

export default function PerformanceWidget({ trend, agents }) {
  const distribution = Object.keys(STATUS_COLORS)
    .map((status) => ({
      name: status.replace("_", " ").toUpperCase(),
      value: agents.filter((a) => a.status === status).length,
      color: STATUS_COLORS[status],
    }))
    .filter((d) => d.value > 0);

  return (
    <div className="border border-cyan-800/70 bg-[#0A1219] flex flex-col min-h-0">
      <div className="px-3 py-2 border-b border-cyan-800/70 bg-[#0C1720] font-heading font-bold tracking-[0.2em] text-sm text-cyan-300">
        FLEET PERFORMANCE
      </div>
      <div className="flex-1 grid grid-cols-2 gap-2 p-2 min-h-0">
        <div className="flex flex-col min-h-0">
          <span className="font-mono text-[10px] text-cyan-600 px-1 pb-1">ACTIVITY TREND</span>
          <ResponsiveContainer width="100%" height="100%">
            <AreaChart data={trend} margin={{ top: 2, right: 2, left: 2, bottom: 2 }}>
              <defs>
                <linearGradient id="trendGrad" x1="0" y1="0" x2="0" y2="1">
                  <stop offset="0%" stopColor="#00E5FF" stopOpacity={0.4} />
                  <stop offset="100%" stopColor="#00E5FF" stopOpacity={0} />
                </linearGradient>
              </defs>
              <XAxis dataKey="time" hide />
              <YAxis domain={[0, agents.length]} hide />
              <Tooltip contentStyle={tooltipStyle} />
              <Area type="monotone" dataKey="active" name="ACTIVE" stroke="#00E5FF" strokeWidth={2} fill="url(#trendGrad)" isAnimationActive={false} />
            </AreaChart>
          </ResponsiveContainer>
        </div>
        <div className="flex flex-col min-h-0">
          <span className="font-mono text-[10px] text-cyan-600 px-1 pb-1">STATUS DISTRIBUTION</span>
          <ResponsiveContainer width="100%" height="100%">
            <PieChart>
              <Pie data={distribution} dataKey="value" innerRadius="55%" outerRadius="85%" stroke="#0A1219" isAnimationActive={false}>
                {distribution.map((d) => (
                  <Cell key={d.name} fill={d.color} />
                ))}
              </Pie>
              <Tooltip contentStyle={tooltipStyle} />
              <Legend
                iconType="square"
                iconSize={7}
                wrapperStyle={{ fontFamily: "monospace", fontSize: 9, color: "#67e8f9" }}
              />
            </PieChart>
          </ResponsiveContainer>
        </div>
      </div>
    </div>
  );
}