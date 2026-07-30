/**
 * contextGraph — pure helpers for the CONTEXT memory-graph surface (F-MEM-1).
 *
 * These operate on the read-only projection shape proposed for the bridge:
 *   { nodes: Array<{ id, label, kind }>, edges: Array<{ source, target, relation, confidence }> }
 *
 * Defensive: never throw on odd shapes (null, undefined, missing fields, non-array
 * nodes/edges, nodes without a kind field). The UI must stay honest about what the
 * backend actually surfaces — helpers return safe zeros, not fabricated signal.
 *
 * No imports from agentBridge.js. The bridge currently exposes NO memory-read method;
 * these helpers are wired against the empty state until NEEDS-BACKEND lands.
 */

/**
 * True when the graph is nullish or has no nodes.
 * @param {{ nodes?: unknown[] } | null | undefined} g
 * @returns {boolean}
 */
export function isEmptyGraph(g) {
  if (!g || typeof g !== "object") return true;
  const nodes = Array.isArray(g.nodes) ? g.nodes : [];
  return nodes.length === 0;
}

/**
 * Count nodes grouped by a classification field. Tries `kind` first, falls back to
 * `file_type`, then `category`. Nodes missing the chosen field land under "unknown".
 * @param {unknown[]} nodes
 * @returns {Record<string, number>}
 */
function countByKind(nodes) {
  const byKind = {};
  for (const n of nodes) {
    if (!n || typeof n !== "object") {
      byKind["unknown"] = (byKind["unknown"] || 0) + 1;
      continue;
    }
    const key =
      (typeof n.kind === "string" && n.kind) ||
      (typeof n.file_type === "string" && n.file_type) ||
      (typeof n.category === "string" && n.category) ||
      "unknown";
    byKind[key] = (byKind[key] || 0) + 1;
  }
  return byKind;
}

/**
 * Summary projection of a memory graph.
 *
 * Returns `{ nodeCount, edgeCount, linkEdges, suggestedEdges, byKind }`.
 *   - linkEdges: edges whose `relation` is "link" (hard, author-made).
 *   - suggestedEdges: edges whose `relation` is "suggested" (soft, heuristic).
 *   - Edges with any other relation value are counted in edgeCount only.
 *
 * Defensive: absent nodes/edges treated as []; malformed entries skipped for
 * relation counts (non-object edges counted toward edgeCount but not link/suggested).
 *
 * @param {{ nodes?: unknown[], edges?: unknown[] } | null | undefined} g
 * @returns {{ nodeCount: number, edgeCount: number, linkEdges: number, suggestedEdges: number, byKind: Record<string, number> }}
 */
export function summarizeGraph(g) {
  const nodes = g && typeof g === "object" && Array.isArray(g.nodes) ? g.nodes : [];
  const edges = g && typeof g === "object" && Array.isArray(g.edges) ? g.edges : [];

  let linkEdges = 0;
  let suggestedEdges = 0;
  for (const e of edges) {
    if (!e || typeof e !== "object") continue;
    if (e.relation === "link") linkEdges += 1;
    else if (e.relation === "suggested") suggestedEdges += 1;
  }

  return {
    nodeCount: nodes.length,
    edgeCount: edges.length,
    linkEdges,
    suggestedEdges,
    byKind: countByKind(nodes),
  };
}
