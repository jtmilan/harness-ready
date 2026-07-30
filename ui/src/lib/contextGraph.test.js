// Pure helpers — no DOM, no Tauri, no bridge. Defensive on odd shapes.
import { describe, it, expect } from "vitest";
import { isEmptyGraph, summarizeGraph } from "@/lib/contextGraph";

describe("isEmptyGraph", () => {
  it("treats nullish inputs as empty", () => {
    expect(isEmptyGraph(null)).toBe(true);
    expect(isEmptyGraph(undefined)).toBe(true);
  });

  it("treats a graph with no nodes field as empty", () => {
    expect(isEmptyGraph({})).toBe(true);
    expect(isEmptyGraph({ edges: [] })).toBe(true);
  });

  it("treats a graph with an empty nodes array as empty", () => {
    expect(isEmptyGraph({ nodes: [], edges: [] })).toBe(true);
  });

  it("returns false when at least one node is present", () => {
    expect(isEmptyGraph({ nodes: [{ id: "n1" }] })).toBe(false);
  });

  it("treats a non-array nodes as empty (defensive)", () => {
    expect(isEmptyGraph({ nodes: "oops" })).toBe(true);
    expect(isEmptyGraph({ nodes: null })).toBe(true);
  });
});

describe("summarizeGraph", () => {
  it("returns zero summary for an empty graph", () => {
    const s = summarizeGraph({ nodes: [], edges: [] });
    expect(s).toEqual({
      nodeCount: 0,
      edgeCount: 0,
      linkEdges: 0,
      suggestedEdges: 0,
      byKind: {},
    });
  });

  it("treats nullish input as zero summary", () => {
    const s = summarizeGraph(null);
    expect(s.nodeCount).toBe(0);
    expect(s.edgeCount).toBe(0);
    expect(s.byKind).toEqual({});
  });

  it("treats missing nodes/edges fields as empty arrays", () => {
    const s = summarizeGraph({});
    expect(s.nodeCount).toBe(0);
    expect(s.edgeCount).toBe(0);
  });

  it("counts mixed edge relations correctly", () => {
    const g = {
      nodes: [
        { id: "a", label: "A", kind: "topic" },
        { id: "b", label: "B", kind: "entity" },
        { id: "c", label: "C", kind: "topic" },
      ],
      edges: [
        { source: "a", target: "b", relation: "link" },
        { source: "b", target: "c", relation: "suggested" },
        { source: "a", target: "c", relation: "link" },
        { source: "a", target: "c", relation: "other" },
      ],
    };
    const s = summarizeGraph(g);
    expect(s.nodeCount).toBe(3);
    expect(s.edgeCount).toBe(4);
    expect(s.linkEdges).toBe(2);
    expect(s.suggestedEdges).toBe(1);
    expect(s.byKind).toEqual({ topic: 2, entity: 1 });
  });

  it("falls back to file_type when kind is absent", () => {
    const g = {
      nodes: [
        { id: "n1", file_type: "md" },
        { id: "n2", file_type: "pdf" },
      ],
      edges: [],
    };
    expect(summarizeGraph(g).byKind).toEqual({ md: 1, pdf: 1 });
  });

  it("falls back to category when kind and file_type are absent", () => {
    const g = {
      nodes: [{ id: "n1", category: "shared" }],
      edges: [],
    };
    expect(summarizeGraph(g).byKind).toEqual({ shared: 1 });
  });

  it("classifies nodes with no classification field as unknown", () => {
    const g = {
      nodes: [{ id: "n1" }, { id: "n2", kind: "topic" }],
      edges: [],
    };
    expect(summarizeGraph(g).byKind).toEqual({ unknown: 1, topic: 1 });
  });

  it("does not throw on malformed inputs", () => {
    expect(() => summarizeGraph({ nodes: [null, "bad", 42, {}] })).not.toThrow();
    expect(() => summarizeGraph({ nodes: [], edges: [null, "bad", 42] })).not.toThrow();
    // Malformed nodes: non-objects go to unknown; objects with no kind field go to unknown.
    const s = summarizeGraph({ nodes: [null, "bad", 42, { id: "ok", kind: "topic" }], edges: [] });
    expect(s.nodeCount).toBe(4);
    expect(s.byKind.unknown).toBe(3);
    expect(s.byKind.topic).toBe(1);
  });

  it("counts edges even when they are malformed non-objects", () => {
    const s = summarizeGraph({
      nodes: [],
      edges: [{ source: "a", target: "b", relation: "link" }, null, "bad", 42],
    });
    expect(s.edgeCount).toBe(4);
    expect(s.linkEdges).toBe(1);
  });
});
