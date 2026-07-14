// Phase 11 memory-graph view — PURE layout core (no DOM, no IPC, no globals).
//
// The frontend graph overlay (main.js) renders the read-only `{nodes, edges}`
// projection returned by the `memory_graph` Tauri command. The Rust shapes are:
//   GraphNode = { id, title, degree, updated_at, tags, snippet, origin }
//                (tags: string[] / snippet: string / origin: string|null are
//                 ADDITIVE — older projections omit them; layoutGraph defaults)
//   GraphEdge = { from, to, kind }   // kind: "link" (hard) | "suggested" (soft)
//
// This module owns only the math: given a graph, place every node on a circle and
// resolve each edge to a pair of endpoint coordinates. Keeping it pure means vitest
// can exercise it in Node with zero setup (mirrors kanban-core.js).

// A node is "suggested-only" vs "link" is decided per EDGE, not per node. We expose
// the two edge kinds as constants so main.js and the tests agree on the spelling
// (must match the Rust `GraphEdge.kind` string exactly).
export const EDGE_LINK = "link";
export const EDGE_SUGGESTED = "suggested";

// ---- one-word display labels ---------------------------------------------------
//
// Operator directive (2026-07-07): graph nodes read as ONE word, never duplicated.
// Titles stay contentful in the DATA (harvest dedups by exact title; the hover card
// and SVG tooltip show the full title) — the short form is DISPLAY-ONLY, derived
// here so both renderers (WebGL labels, SVG fallback) inherit the same label.
//
// Derivation is deterministic in node order (layoutGraph's degree-then-id order):
// first non-stopword word of the title not already claimed by an earlier node;
// falls back to any unclaimed word, then to a numeric suffix ("word2") so labels
// are unique within a view even for identical titles.
const LABEL_STOPWORDS = new Set([
  "the", "a", "an", "this", "that", "these", "those", "of", "in", "on", "for",
  "to", "and", "or", "is", "are", "was", "be", "so", "its", "it", "as", "with",
  "can", "if", "not", "no", "use", "make", "how", "why", "what", "when",
]);
const LABEL_MAX_CHARS = 14;

// Title → candidate words: lowercase, punctuation/backticks stripped, split on
// whitespace/dashes/slashes, short fragments dropped.
function labelWords(title) {
  return String(title)
    .toLowerCase()
    .replace(/[`'".,:;!?()[\]{}<>*#=+|~^$%&@\\]/g, " ")
    .split(/[\s/—–-]+/) // NOT underscore: identifiers (count_unique, mem_42) stay whole words
    .filter((w) => w.length >= 2 && /[a-z0-9]/.test(w));
}

// Stamp a unique one-word `label` on every rec (mutates + returns the array).
// Exported for tests; layoutGraph applies it to each view's node set.
export function assignShortLabels(recs) {
  const used = new Set();
  for (const r of recs) {
    const words = labelWords(r.title == null ? r.id : r.title);
    let pick =
      words.find((w) => !LABEL_STOPWORDS.has(w) && !used.has(w)) ||
      words.find((w) => !used.has(w)) ||
      words.find((w) => !LABEL_STOPWORDS.has(w)) ||
      words[0] ||
      "note";
    let label = pick.slice(0, LABEL_MAX_CHARS);
    if (used.has(label)) {
      let i = 2;
      while (used.has(label + i)) i += 1;
      label = label + i;
    }
    used.add(label);
    r.label = label;
  }
  return recs;
}

// Deterministic ring layout. Nodes are placed on a single circle, ordered by
// DESCENDING degree (the most-connected notes first) then id, so the busiest hubs
// land in stable, spread-out slots and the layout does not jump between polls. The
// circle is centered in a `width`×`height` box with `pad` px of breathing room.
//
// Returns { width, height,
//           nodes: [{ id, title, degree, tags, snippet, origin, updated_at, x, y }],
//           edges: [...] }
// where each laid-out edge carries its endpoint coords + kind + a `suggested` bool
// so the renderer can pick solid vs dashed without re-parsing the kind string.
// The hover-card fields pass through from the source node with safe defaults
// (tags=[], snippet="", origin=null, updated_at=0) so the card renderer never
// branches on undefined when reading an older projection.
// Unknown edge endpoints (an edge naming a missing node) are dropped — the view is
// best-effort and never throws on a malformed projection.
export function layoutGraph(graph, opts = {}) {
  const width = opts.width || 800;
  const height = opts.height || 600;
  const pad = opts.pad == null ? 60 : opts.pad;

  const rawNodes = (graph && Array.isArray(graph.nodes)) ? graph.nodes : [];
  const rawEdges = (graph && Array.isArray(graph.edges)) ? graph.edges : [];

  // Stable order: most-connected first, ties broken by id (string compare) so the
  // ring is deterministic across renders.
  const ordered = rawNodes.slice().sort((a, b) => {
    const da = (a && a.degree) || 0;
    const db = (b && b.degree) || 0;
    if (db !== da) return db - da;
    return String(a && a.id).localeCompare(String(b && b.id));
  });

  const cx = width / 2;
  const cy = height / 2;
  const radius = Math.max(0, Math.min(width, height) / 2 - pad);
  const n = ordered.length;

  const placed = new Map(); // id -> { id, title, degree, tags, snippet, origin, updated_at, x, y }
  const nodes = ordered.map((node, i) => {
    // A single node sits dead-center; otherwise spread evenly around the ring,
    // starting at the top (−90°) and going clockwise.
    const angle = n <= 1 ? 0 : (-Math.PI / 2) + (2 * Math.PI * i) / n;
    const x = n <= 1 ? cx : cx + radius * Math.cos(angle);
    const y = n <= 1 ? cy : cy + radius * Math.sin(angle);
    const rec = {
      id: String(node && node.id),
      title: (node && node.title != null && node.title !== "") ? String(node.title) : String(node && node.id),
      degree: (node && node.degree) || 0,
      // Hover-card passthrough (additive Rust fields) — defaulted, never undefined.
      tags: (node && Array.isArray(node.tags)) ? node.tags : [],
      snippet: (node && typeof node.snippet === "string") ? node.snippet : "",
      origin: (node && node.origin != null) ? String(node.origin) : null,
      // Category (additive Rust field, Note.category) — drives orb/edge color and
      // the edit dropdown. Null when absent (legacy notes) → renderer hashes a color.
      category: (node && node.category != null) ? String(node.category) : null,
      updated_at: (node && node.updated_at) || 0,
      x,
      y,
    };
    placed.set(rec.id, rec);
    return rec;
  });

  // Display labels: one word, unique within the view (see assignShortLabels).
  assignShortLabels(nodes);

  const edges = [];
  for (const e of rawEdges) {
    if (!e) continue;
    const a = placed.get(String(e.from));
    const b = placed.get(String(e.to));
    if (!a || !b) continue; // dangling endpoint → skip (best-effort)
    const suggested = e.kind === EDGE_SUGGESTED;
    edges.push({
      from: a.id,
      to: b.id,
      kind: e.kind,
      suggested,
      x1: a.x,
      y1: a.y,
      x2: b.x,
      y2: b.y,
    });
  }

  return { width, height, nodes, edges };
}

// True when the projection has no notes at all → the view should show its empty
// state rather than a blank ring. (An empty graph is a valid, non-error result.)
export function isEmptyGraph(graph) {
  return !graph || !Array.isArray(graph.nodes) || graph.nodes.length === 0;
}
