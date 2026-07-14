import React, { useState, useEffect } from "react";

function formatDuration(start) {
  const s = Math.floor((Date.now() - start) / 1000);
  const h = String(Math.floor(s / 3600)).padStart(2, "0");
  const m = String(Math.floor((s % 3600) / 60)).padStart(2, "0");
  const sec = String(s % 60).padStart(2, "0");
  return `${h}:${m}:${sec}`;
}

export default function SessionInfo({ sessionId, startTime, running }) {
  const [duration, setDuration] = useState(() => formatDuration(startTime));

  useEffect(() => {
    const t = setInterval(() => setDuration(formatDuration(startTime)), 1000);
    return () => clearInterval(t);
  }, [startTime]);

  return (
    <div className="border border-cyan-800/70 bg-[#0A1219] flex flex-col min-h-0">
      <div className="px-3 py-2 border-b border-cyan-800/70 bg-[#0C1720] font-heading font-bold tracking-[0.2em] text-sm text-cyan-300">
        SESSION INFO
      </div>
      <div className="p-4 font-mono text-xs space-y-3 text-cyan-400">
        <div>
          <div className="text-cyan-600">SESSION ID:</div>
          <div className="text-cyan-200">{sessionId}</div>
        </div>
        <div>
          <div className="text-cyan-600">DURATION:</div>
          <div className="text-cyan-200">{duration}</div>
        </div>
        <div>
          <div className="text-cyan-600">ORCHESTRATOR:</div>
          <div className={running ? "text-cyan-300" : "text-yellow-300"}>{running ? "RUNNING" : "PAUSED"}</div>
        </div>
      </div>
    </div>
  );
}