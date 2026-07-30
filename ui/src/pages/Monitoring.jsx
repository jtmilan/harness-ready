import React, { useState, useEffect, useMemo } from "react";
import { Cpu, AlertTriangle, CircleDot } from "lucide-react";
import { bridge } from "@/lib/agentBridge";
import { buildMonitorRows, fleetCounts, METRIC_COLS, LANE, fmtWaited } from "@/lib/monitorRows";
import TitleBar from "@/components/command/TitleBar";
import ResourceChart from "@/components/monitor/ResourceChart";
import SuccessRateChart from "@/components/monitor/SuccessRateChart";

// F-OBS-1 (R-OBS): Monitoring is the OBSERVE surface — a per-pane real-signal health TABLE as
// the hero, sourced from bridge.subscribe. EXISTS-signal columns render live today; the five
// metric columns (CPU/MEM/GIT/LAST FAIL/GATE Q) are NEEDS-BACKEND and show an honest no-data
// cell until R-OBS sampling + outcome events land (see monitorRows.js + SYNTHESIS §2/§3). The
// simulated resource/success charts are demoted to a clearly-labelled SECONDARY block.
// Lane semantics (LANE) + time formatting (fmtWaited) live in monitorRows.js — single source,
// unit-tested, no string-split hacks (review fix).

// Honest live count tile (subscribe-derived). Replaces the old fake FLEET CPU/MEM/SUCCESS/
// TASKS vanity cards. The number + label carry the meaning; the lane color reinforces (R8).
function CountTile({ icon: Icon, label, value, lane }) {
  const l = LANE[lane] || LANE.muted;
  return (
    <div className="border border-cyan-900/60 bg-[#0A1219] px-4 py-3 flex items-center gap-3">
      <Icon className={`w-4 h-4 ${l.text}`} />
      <div className="leading-tight">
        <div className={`font-heading text-2xl font-bold tabular-nums ${l.text}`}>{value}</div>
        <div className="font-heading text-[10px] tracking-[0.25em] text-cyan-600 font-bold">{label}</div>
      </div>
    </div>
  );
}

export default function Monitoring() {
  const [agents, setAgents] = useState([]);

  // Real fleet state from the single contract. No bridge.start() here — that drives the mock
  // simulator; in the web preview an empty fleet correctly shows the honest empty state, and
  // in the Tauri shell subscribe reflects the live panes.
  useEffect(() => {
    const unsubscribe = bridge.subscribe(setAgents);
    return unsubscribe;
  }, []);

  const rows = useMemo(() => buildMonitorRows(agents), [agents]);
  const counts = useMemo(() => fleetCounts(agents), [agents]);

  return (
    <div className="h-screen flex flex-col bg-[#0D1117] scanlines overflow-hidden">
      <TitleBar />
      {/* F-OBS-4 (R4): page-wide honesty banner — nothing below the table is live yet. */}
      <div className="px-5 py-2 border-b border-amber-500/40 bg-amber-500/10 font-mono text-[11px] leading-relaxed text-amber-300/90">
        ⚠ The charts at the bottom are SIMULATED placeholders pending R-OBS. The health table
        above is live from the bridge; its CPU/MEM/GIT/FAIL/GATE-Q columns are honest
        <span className="text-amber-200"> no-data </span> until the real signal feed lands.
      </div>

      <div className="flex-1 overflow-y-auto terminal-scroll p-5 space-y-5">
        {/* Live, subscribe-derived counts — the only honest aggregates. */}
        <div className="grid grid-cols-2 lg:grid-cols-4 gap-4">
          <CountTile icon={CircleDot} label="LIVE PANES" value={counts.live} lane="info" />
          <CountTile icon={AlertTriangle} label="NEED YOU" value={counts.needsYou} lane="need" />
          <CountTile icon={Cpu} label="WORKING" value={counts.working} lane="success" />
          <CountTile icon={AlertTriangle} label="ERROR" value={counts.error} lane="danger" />
        </div>

        {/* HERO: per-pane real-signal health table. */}
        <div className="border border-cyan-800/70 bg-[#0A1219]">
          <div className="px-4 py-2.5 border-b border-cyan-800/70 bg-[#0C1720] flex items-center justify-between">
            <span className="font-heading font-bold tracking-[0.2em] text-sm text-cyan-300">
              FLEET HEALTH — PER PANE
            </span>
            <span className="font-mono text-[10px] text-cyan-700">
              live · from bridge.subscribe · {rows.length} pane{rows.length === 1 ? "" : "s"}
            </span>
          </div>
          <div className="overflow-x-auto">
            <table className="w-full text-left font-mono text-[11px] border-collapse">
              <thead>
                <tr className="text-cyan-600">
                  <th className="px-3 py-2 font-bold tracking-[0.15em]">PANE</th>
                  <th className="px-3 py-2 font-bold tracking-[0.15em]">HARNESS</th>
                  <th className="px-3 py-2 font-bold tracking-[0.15em]">ROLE</th>
                  <th className="px-3 py-2 font-bold tracking-[0.15em]">STATUS</th>
                  <th className="px-3 py-2 font-bold tracking-[0.15em]">ATTENTION</th>
                  <th className="px-3 py-2 font-bold tracking-[0.15em]">BRANCH</th>
                  {METRIC_COLS.map((c) => (
                    <th key={c.key} className="px-3 py-2 font-bold tracking-[0.1em] text-right" title="metric pending R-OBS backend">
                      {c.label}<span className="text-cyan-800">*</span>
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {rows.length === 0 ? (
                  <tr>
                    <td colSpan={6 + METRIC_COLS.length} className="px-3 py-6 text-center text-cyan-700">
                      no fleet — spawn an agent to populate this table (empty state, not an error)
                    </td>
                  </tr>
                ) : rows.map((r) => {
                  const l = LANE[r.lane] || LANE.muted;
                  return (
                    // Display-only rows: no hover affordance (a hover implies clickability — R8
                    // adjacency; rows carry no handler/tabIndex, so they must not look clickable).
                    <tr key={r.id} className="border-t border-cyan-900/40">
                      <td className="px-3 py-1.5 text-cyan-200 whitespace-nowrap">{r.name}</td>
                      <td className="px-3 py-1.5 text-cyan-400 uppercase">{r.kind ?? "—"}</td>
                      <td className="px-3 py-1.5 text-cyan-500">{r.role ?? "—"}</td>
                      <td className="px-3 py-1.5 whitespace-nowrap">
                        <span className={`inline-flex items-center gap-1 border px-1.5 py-0.5 ${l.text} ${l.border}`}>
                          <span className={`w-1.5 h-1.5 ${l.dot}`} />
                          {(r.status ?? "unknown").toUpperCase()}
                        </span>
                      </td>
                      <td className="px-3 py-1.5 text-amber-300/90 whitespace-nowrap">
                        {r.attention ? `${r.attention.reason}${r.attention.waitedSec != null ? " · " + fmtWaited(r.attention.waitedSec) : ""}` : <span className="text-cyan-800">—</span>}
                      </td>
                      <td className="px-3 py-1.5 text-cyan-600 whitespace-nowrap max-w-[14rem] truncate" title={r.branch ?? ""}>{r.branch ?? "—"}</td>
                      {METRIC_COLS.map((c) => {
                        const cell = r.metrics[c.key];
                        return (
                          <td key={c.key} className="px-3 py-1.5 text-right tabular-nums">
                            {cell.kind === "nodata"
                              ? <span className="text-cyan-800" title="no backend signal yet (R-OBS)">—</span>
                              : <span className="text-cyan-200">{String(cell.value)}</span>}
                          </td>
                        );
                      })}
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
          <div className="px-4 py-2 border-t border-cyan-900/40 font-mono text-[10px] text-cyan-700">
            * CPU / MEM / GIT / LAST FAIL / GATE Q are NEEDS-BACKEND — rendered as honest no-data,
            never zero or interpolated, until the R-OBS feed exists.
          </div>
        </div>

        {/*
         * PER-AGENT OUTCOMES — placeholder pending R-OBS outcome events (NEEDS-BACKEND);
         * nothing below is live.
         *
         * F-OBS-2 — NEEDS-BACKEND (proposed, NOT wired): when core emits per-agent task
         * outcomes, either (a) extend the subscribe() agent shape with
         *   outcomes: { success, blocked, error, total, lastOutcomeAt }
         * or (b) add bridge.getOutcomeStats(): Promise<Array<{ id, success, blocked,
         *   error, lastOutcomeAt }>>.
         * Until then this section stays an empty placeholder (R4). Do NOT add either to
         * agentBridge.js here (R10: contract changes are NOTE-only, backend-gated).
         */}
        <div className="space-y-2">
          <div className="font-heading text-[11px] tracking-[0.3em] text-amber-400/80 font-bold">
            PER-AGENT OUTCOMES — PLACEHOLDER (pending R-OBS outcome events)
          </div>
          <div className="px-4 py-2 border border-amber-500/30 bg-amber-500/5 font-mono text-[10px] text-amber-400/80">
            ⚠ No outcome data yet — success / blocked / error / total counts require
            per-agent task-outcome events from the backend (NEEDS-BACKEND, F-OBS-2). The
            empty chart below is honest; do not read numbers into it.
          </div>
          <SuccessRateChart data={[]} />
        </div>

        {/* SECONDARY, SIMULATED — CPU/MEM sampling placeholder (R4 / V-B5). Separate from
         * outcomes: R-OBS sampling is a different concern from task-outcome attribution. */}
        <div className="space-y-2">
          <div className="font-heading text-[11px] tracking-[0.3em] text-amber-400/80 font-bold">
            SIMULATED SERIES — PLACEHOLDER ONLY (pending R-OBS sampling)
          </div>
          <ResourceChart data={[]} />
        </div>
      </div>
    </div>
  );
}
