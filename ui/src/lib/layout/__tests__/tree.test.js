// Pure-tree tests: reconcileTree self-heals against the live pane set (append new, prune dead,
// preserve structure), and moveLeaf reorders a pane beside a target. Framework-free — no DOM.
import { describe, it, expect } from "vitest";
import {
  leafPanes,
  hasPane,
  buildDefaultTree,
  reconcileTree,
  moveLeaf,
  serializeTree,
  deserializeTree,
} from "../tree.js";

describe("reconcileTree — add / remove", () => {
  it("appends a newly-live pane missing from the tree", () => {
    const t0 = buildDefaultTree(["w-p0", "w-p1"]);
    const { tree } = reconcileTree(t0, ["w-p0", "w-p1", "w-p2"], "w-p1");
    expect(new Set(leafPanes(tree))).toEqual(new Set(["w-p0", "w-p1", "w-p2"]));
  });

  it("prunes a dead pane, promoting its sibling", () => {
    const t0 = buildDefaultTree(["w-p0", "w-p1", "w-p2"]);
    const { tree } = reconcileTree(t0, ["w-p0", "w-p2"], "w-p0");
    expect(leafPanes(tree).sort()).toEqual(["w-p0", "w-p2"]);
    expect(hasPane(tree, "w-p1")).toBe(false);
  });

  it("is idempotent when the tree already matches the live set", () => {
    const t0 = buildDefaultTree(["w-p0", "w-p1"]);
    const a = reconcileTree(t0, ["w-p0", "w-p1"], null).tree;
    const b = reconcileTree(a, ["w-p0", "w-p1"], null).tree;
    expect(leafPanes(b)).toEqual(leafPanes(a));
  });

  it("prune-all → null tree", () => {
    const t0 = buildDefaultTree(["w-p0", "w-p1"]);
    const { tree } = reconcileTree(t0, [], null);
    expect(tree).toBeNull();
  });
});

describe("moveLeaf — reorder", () => {
  it("moves src beside target without changing the pane set", () => {
    const t0 = buildDefaultTree(["w-p0", "w-p1", "w-p2"]);
    const moved = moveLeaf(t0, "w-p0", "w-p2", "h", "after");
    expect(new Set(leafPanes(moved))).toEqual(new Set(["w-p0", "w-p1", "w-p2"]));
    // p0 now trails p2 in DFS order (was the head before the move)
    expect(leafPanes(moved).indexOf("w-p0")).toBeGreaterThan(leafPanes(moved).indexOf("w-p2"));
  });

  it("moving a pane onto itself is a no-op", () => {
    const t0 = buildDefaultTree(["w-p0", "w-p1"]);
    expect(moveLeaf(t0, "w-p0", "w-p0", "v", "after")).toBe(t0);
  });
});

describe("serialize / deserialize round-trip", () => {
  it("survives an id rebuild via the pane index", () => {
    const t0 = buildDefaultTree(["w-p0", "w-p1"]);
    const ser = serializeTree(t0, (pane) => Number(/-p(\d+)$/.exec(pane)[1]));
    const back = deserializeTree(ser, "w");
    expect(leafPanes(back)).toEqual(["w-p0", "w-p1"]);
  });
});
