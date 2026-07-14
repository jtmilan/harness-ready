import React, { useState, useEffect, useMemo } from "react";
import { Cpu, MemoryStick, CheckCircle2, ListChecks } from "lucide-react";
import { initialSeries, nextPoint, createSuccessRates, driftSuccessRates } from "@/lib/monitorData";
import TitleBar from "@/components/command/TitleBar";
import StatCard from "@/components/monitor/StatCard";
import ResourceChart from "@/components/monitor/ResourceChart";
import SuccessRateChart from "@/components/monitor/SuccessRateChart";

const AGENT_IDS = Array.from({ length: 12 }, (_, i) => `AGENT-${String(i + 1).padStart(3, "0")}`);

export default function Monitoring() {
  const [series, setSeries] = useState(() => initialSeries(30));
  const [rates, setRates] = useState(() => createSuccessRates(AGENT_IDS));

  useEffect(() => {
    const t = setInterval(() => {
      setSeries((prev) => [...prev.slice(-29), nextPoint(prev[prev.length - 1])]);
    }, 2000);
    const r = setInterval(() => setRates(driftSuccessRates), 5000);
    return () => { clearInterval(t); clearInterval(r); };
  }, []);

  const latest = series[series.length - 1];
  const stats = useMemo(() => {
    const avgSuccess = Math.round(rates.reduce((s, r) => s + r.success, 0) / rates.length);
    const totalTasks = rates.reduce((s, r) => s + r.tasks, 0);
    return { avgSuccess, totalTasks };
  }, [rates]);

  return (
    <div className="h-screen flex flex-col bg-[#0D1117] scanlines overflow-hidden">
      <TitleBar />
      <div className="flex-1 overflow-y-auto terminal-scroll p-5 space-y-5">
        <div className="grid grid-cols-2 lg:grid-cols-4 gap-4">
          <StatCard icon={Cpu} label="FLEET CPU" value={`${Math.round(latest.cpu)}%`} sub="avg across agents" />
          <StatCard icon={MemoryStick} label="FLEET MEMORY" value={`${Math.round(latest.mem)}%`} sub="avg across agents" tone="yellow" />
          <StatCard icon={CheckCircle2} label="SUCCESS RATE" value={`${stats.avgSuccess}%`} sub="fleet average" tone="green" />
          <StatCard icon={ListChecks} label="TASKS COMPLETED" value={stats.totalTasks} sub="this session" />
        </div>
        <div className="grid grid-cols-1 xl:grid-cols-2 gap-5">
          <ResourceChart data={series} />
          <SuccessRateChart data={rates} />
        </div>
      </div>
    </div>
  );
}