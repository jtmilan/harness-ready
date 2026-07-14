// Phase 11 memory-graph view — unit tests for the pure layout core (graph-core.js).
//
// These cover the math + edge-kind handling only; the rendered SVG overlay is
// GUI-verified by the operator. Pure → vitest runs them in Node with no DOM.
import { describe, it, expect } from "vitest";
import { layoutGraph, isEmptyGraph, assignShortLabels, EDGE_LINK, EDGE_SUGGESTED } from "./graph-core.js";

const G = (nodes, edges) => ({ nodes, edges });

describe("isEmptyGraph", () => {
  it("treats the empty projection as empty (not an error)", () => {
    expect(isEmptyGraph({ nodes: [], edges: [] })).toBe(true);
    expect(isEmptyGraph(null)).toBe(true);
    expect(isEmptyGraph(undefined)).toBe(true);
    expect(isEmptyGraph({})).toBe(true);
  });
  it("is non-empty as soon as there is a node", () => {
    expect(isEmptyGraph(G([{ id: "a", title: "A", degree: 0, updated_at: 0 }], []))).toBe(false);
  });
});

describe("layoutGraph — node placement", () => {
  it("returns an empty layout for an empty graph", () => {
    const out = layoutGraph(G([], []));
    expect(out.nodes).toEqual([]);
    expect(out.edges).toEqual([]);
  });

  it("places a single node dead-center", () => {
    const out = layoutGraph(G([{ id: "a", title: "A", degree: 0 }], []), { width: 400, height: 200 });
    expect(out.nodes).toHaveLength(1);
    expect(out.nodes[0].x).toBe(200);
    expect(out.nodes[0].y).toBe(100);
  });

  it("spreads multiple nodes on a ring inside the padded box", () => {
    const nodes = ["a", "b", "c", "d"].map((id) => ({ id, title: id.toUpperCase(), degree: 0 }));
    const out = layoutGraph(G(nodes, []), { width: 400, height: 400, pad: 40 });
    expect(out.nodes).toHaveLength(4);
    // every node within the box bounds
    for (const nd of out.nodes) {
      expect(nd.x).toBeGreaterThanOrEqual(0);
      expect(nd.x).toBeLessThanOrEqual(400);
      expect(nd.y).toBeGreaterThanOrEqual(0);
      expect(nd.y).toBeLessThanOrEqual(400);
    }
    // distinct positions (no two nodes stacked)
    const keys = new Set(out.nodes.map((n) => `${n.x.toFixed(2)},${n.y.toFixed(2)}`));
    expect(keys.size).toBe(4);
  });

  it("orders by descending degree then id (busiest hub first, deterministic)", () => {
    const nodes = [
      { id: "low", title: "Low", degree: 1 },
      { id: "hub", title: "Hub", degree: 9 },
      { id: "mid2", title: "Mid2", degree: 5 },
      { id: "mid1", title: "Mid1", degree: 5 },
    ];
    const out = layoutGraph(G(nodes, []));
    expect(out.nodes.map((n) => n.id)).toEqual(["hub", "mid1", "mid2", "low"]);
  });

  it("passes hover-card fields through (tags/snippet/origin/updated_at) with safe defaults", () => {
    const out = layoutGraph(
      G(
        [
          { id: "a", title: "A", degree: 2, updated_at: 1234, tags: ["rust", "mcp"], snippet: "First line of the note…", origin: "daily/2026-07-04.md" },
          { id: "b", title: "B", degree: 0 }, // older projection — no card fields
        ],
        [],
      ),
    );
    const a = out.nodes.find((n) => n.id === "a");
    expect(a.tags).toEqual(["rust", "mcp"]);
    expect(a.snippet).toBe("First line of the note…");
    expect(a.origin).toBe("daily/2026-07-04.md");
    expect(a.updated_at).toBe(1234);
    const b = out.nodes.find((n) => n.id === "b");
    expect(b.tags).toEqual([]);
    expect(b.snippet).toBe("");
    expect(b.origin).toBe(null);
    expect(b.updated_at).toBe(0);
  });

  it("falls back to id when a title is missing/blank", () => {
    const out = layoutGraph(G([{ id: "note-7", title: "", degree: 0 }], []));
    expect(out.nodes[0].title).toBe("note-7");
    const out2 = layoutGraph(G([{ id: "note-8", degree: 0 }], []));
    expect(out2.nodes[0].title).toBe("note-8");
  });
});

describe("layoutGraph — edge resolution & kind", () => {
  const nodes = [
    { id: "a", title: "A", degree: 2 },
    { id: "b", title: "B", degree: 1 },
    { id: "c", title: "C", degree: 1 },
  ];

  it("resolves edge endpoints to placed node coords", () => {
    const out = layoutGraph(G(nodes, [{ from: "a", to: "b", kind: EDGE_LINK }]));
    const e = out.edges[0];
    const a = out.nodes.find((n) => n.id === "a");
    const b = out.nodes.find((n) => n.id === "b");
    expect(e.x1).toBe(a.x);
    expect(e.y1).toBe(a.y);
    expect(e.x2).toBe(b.x);
    expect(e.y2).toBe(b.y);
  });

  it("flags suggested edges (dashed) vs link edges (solid)", () => {
    const out = layoutGraph(
      G(nodes, [
        { from: "a", to: "b", kind: EDGE_LINK },
        { from: "a", to: "c", kind: EDGE_SUGGESTED },
      ]),
    );
    const link = out.edges.find((e) => e.from === "a" && e.to === "b");
    const sug = out.edges.find((e) => e.from === "a" && e.to === "c");
    expect(link.suggested).toBe(false);
    expect(link.kind).toBe("link");
    expect(sug.suggested).toBe(true);
    expect(sug.kind).toBe("suggested");
  });

  it("drops edges with a dangling endpoint (best-effort, never throws)", () => {
    const out = layoutGraph(G(nodes, [{ from: "a", to: "ghost", kind: EDGE_LINK }]));
    expect(out.edges).toEqual([]);
  });

  it("tolerates a missing/garbage edges array", () => {
    expect(() => layoutGraph({ nodes, edges: null })).not.toThrow();
    expect(layoutGraph({ nodes, edges: undefined }).edges).toEqual([]);
  });
});

// One-word display labels (operator directive: single word, never duplicated in a
// view; data titles stay full — the short form is display-only).
describe("assignShortLabels", () => {
  const recs = (...titles) => titles.map((t, i) => ({ id: "n" + i, title: t }));

  it("derives the first meaningful word, lowercased and punctuation-free", () => {
    const out = assignShortLabels(recs("Voice dictation path", "`count_unique` tests"));
    expect(out[0].label).toBe("voice");
    expect(out[1].label).toBe("count_unique".slice(0, 14));
  });

  it("skips stopwords", () => {
    const out = assignShortLabels(recs("The wrapped git status trap"));
    expect(out[0].label).toBe("wrapped");
  });

  it("never duplicates: later nodes walk to their next free word", () => {
    const out = assignShortLabels(recs(
      "Agent Teams",
      "Agent Teams Pane Communication",
      "Agent Teams Pane Interaction",
    ));
    expect(out[0].label).toBe("agent");
    expect(out[1].label).toBe("teams");
    expect(out[2].label).toBe("pane");
    expect(new Set(out.map((r) => r.label)).size).toBe(3);
  });

  it("identical titles get numeric suffixes, still unique", () => {
    const out = assignShortLabels(recs("deploy", "deploy", "deploy"));
    expect(out.map((r) => r.label)).toEqual(["deploy", "deploy2", "deploy3"]);
  });

  it("empty/missing title falls back to the id, garbage to 'note'", () => {
    const out = assignShortLabels([{ id: "mem_42", title: null }, { id: "x", title: "!!!" }]);
    expect(out[0].label).toBe("mem_42".slice(0, 14));
    expect(out[1].label).toBe("note");
  });

  it("layoutGraph stamps labels on every laid node", () => {
    const out = layoutGraph({
      nodes: [
        { id: "a", title: "Voice dictation path", degree: 2 },
        { id: "b", title: "Voice memo", degree: 1 },
      ],
      edges: [],
    });
    const labels = out.nodes.map((n) => n.label);
    expect(labels).toContain("voice");
    expect(new Set(labels).size).toBe(2);
    for (const l of labels) expect(l).toMatch(/^[a-z0-9_]+$/);
  });
});
