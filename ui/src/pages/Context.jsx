/**
 * CONTEXT — read-only memory-graph surface (F-MEM-1).
 *
 * PROPOSED (NOT wired) bridge contract — R10: do NOT add this to agentBridge.js here.
 * Documenting the shape so the UI side is ready when the backend exposes it:
 *
 *   bridge.getMemoryGraph(): Promise<{
 *     nodes: Array<{ id: string, label: string, kind: string }>,
 *     edges: Array<{ source: string, target: string, relation: string, confidence?: number }>
 *   }>
 *
 * Read-only projection of the agent-teams memory store. Never writes. The UI consumes
 * this via the `summarizeGraph` / `isEmptyGraph` helpers in @/lib/contextGraph.
 *
 * Until the bridge exposes a read-only memory projection this page renders an honest
 * UNAVAILABLE / NEEDS-BACKEND state. No fake nodes or edges are fabricated (R3/R4).
 */
import React from "react";
import TitleBar from "@/components/command/TitleBar";
import { isEmptyGraph, summarizeGraph } from "@/lib/contextGraph";

// The empty graph is data-driven through the helpers so the summary strip stays honest
// about what the backend actually surfaces — never a fabricated non-zero count.
const EMPTY_GRAPH = { nodes: [], edges: [] };

export default function Context() {
  const empty = isEmptyGraph(EMPTY_GRAPH);
  const summary = summarizeGraph(EMPTY_GRAPH);

  return (
    <div className="h-screen flex flex-col bg-[#0D1117] scanlines overflow-hidden">
      <TitleBar />

      {/* Header strip — matches Monitoring/TitleBar terminal aesthetic. */}
      <div className="px-5 py-2 border-b border-cyan-900/60 bg-[#0A1219] flex items-baseline gap-3">
        <span className="font-heading font-bold tracking-[0.35em] text-cyan-300 text-sm">
          CONTEXT
        </span>
        <span className="font-mono text-[11px] tracking-[0.2em] text-cyan-700">
          MEMORY GRAPH (read-only)
        </span>
      </div>

      {/* NEEDS-BACKEND amber banner (R3/R4 — honest about unavailable state). */}
      <div className="px-5 py-2 border-b border-amber-500/40 bg-amber-500/10 font-mono text-[11px] leading-relaxed text-amber-300/90">
        ⚠ The memory graph is not yet exposed to the UI (NEEDS-BACKEND). Until the bridge
        exposes a read-only memory projection there is nothing to render here — this is not
        an error.
      </div>

      {/* Summary strip — exercises the helpers on the empty state so the page is
          data-driven rather than a static placeholder. */}
      <div className="px-5 py-3 border-b border-cyan-900/40 bg-[#0A0E13] font-mono text-[11px] tracking-wide text-cyan-700 flex items-center gap-4">
        <span className="text-cyan-500">
          {summary.nodeCount} notes
        </span>
        <span>·</span>
        <span>
          {summary.linkEdges} links
        </span>
        <span>·</span>
        <span>
          {summary.suggestedEdges} suggested
        </span>
        {empty && (
          <>
            <span>·</span>
            <span className="text-amber-400/80">EMPTY — backend projection unavailable</span>
          </>
        )}
      </div>

      <div className="flex-1 overflow-y-auto terminal-scroll p-5 space-y-6">
        {/* Intended rendering legend — descriptive only; NO fake nodes/edges (R3/R4). */}
        <section className="border border-cyan-900/50 bg-[#0A1219] p-4 space-y-3">
          <h2 className="font-heading font-bold tracking-[0.25em] text-cyan-300 text-xs">
            LEGEND (intended — not yet rendered)
          </h2>
          <p className="font-mono text-[11px] leading-relaxed text-cyan-700">
            When the bridge exposes the memory projection, this surface will render the
            agent-teams memory store as a graph. The intended visual vocabulary:
          </p>
          <ul className="font-mono text-[11px] leading-relaxed text-cyan-600 space-y-1.5 pl-4">
            <li>
              <span className="text-cyan-400">● node</span> — a note in the memory store
              (label from the note title; color grouped by lane tokens, never brand-as-state).
            </li>
            <li>
              <span className="text-cyan-400">— solid edge</span> — a hard author-made link
              (relation <code className="text-cyan-300">"link"</code>).
            </li>
            <li>
              <span className="text-cyan-400">- - dashed edge</span> — a soft heuristic
              suggestion (relation <code className="text-cyan-300">"suggested"</code>);
              lower contrast, never presented as authoritative.
            </li>
            <li>
              <span className="text-cyan-400">◉ selected ring</span> — uses
              <code className="text-cyan-300"> --info</code> / neutral, not the attention
              lane (V-N1 from SYNTHESIS §3).
            </li>
          </ul>
        </section>

        {/* Contract documentation — captured in the page so the design intent survives
            until the backend lands. */}
        <section className="border border-cyan-900/50 bg-[#0A1219] p-4 space-y-2">
          <h2 className="font-heading font-bold tracking-[0.25em] text-cyan-300 text-xs">
            PROPOSED CONTRACT (not wired)
          </h2>
          <pre className="font-mono text-[11px] leading-relaxed text-cyan-600 whitespace-pre-wrap">
{`bridge.getMemoryGraph(): Promise<{
  nodes: Array<{ id: string, label: string, kind: string }>,
  edges: Array<{
    source: string,
    target: string,
    relation: "link" | "suggested" | string,
    confidence?: number
  }>
}>`}
          </pre>
          <p className="font-mono text-[11px] leading-relaxed text-cyan-700">
            Read-only. Never writes to the memory store. R10: adding this method to
            agentBridge.js is out of scope for this phase.
          </p>
        </section>
      </div>
    </div>
  );
}
