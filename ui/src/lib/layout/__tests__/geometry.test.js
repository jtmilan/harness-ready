// Pure-geometry tests: layoutRects must tile the whole box, carving exactly TILE_GAP between
// adjacent tiles (so N tiles along an axis sum to axisLength − (N−1)·gaps). Framework-free math,
// runnable under vitest (`vitest run`) — no DOM needed. See report: repo has no runner installed,
// so these also back the node smoke in scratchpad/WM-1-smoke.mjs.
import { describe, it, expect } from "vitest";
import { buildDefaultTree, buildBalancedTree } from "../tree.js";
import { layoutRects, TILE_GAP } from "../geometry.js";

const BOX = { x: 0, y: 0, w: 1200, h: 800 };
const rectsByPane = (tree, box = BOX) => {
  const map = {};
  for (const r of layoutRects(tree, box)) map[r.pane] = r;
  return map;
};

describe("layoutRects — columns (buildDefaultTree)", () => {
  it("2 panes: widths sum to box.w − 1 gap, full height, no overlap", () => {
    const m = rectsByPane(buildDefaultTree(["w-p0", "w-p1"]));
    const a = m["w-p0"];
    const b = m["w-p1"];
    expect(a.h).toBe(BOX.h);
    expect(b.h).toBe(BOX.h);
    expect(a.w + b.w).toBe(BOX.w - TILE_GAP);
    expect(b.x).toBe(a.x + a.w + TILE_GAP); // exactly one gutter between them
  });

  it("3 panes: total occupied width = box.w − 2 gaps", () => {
    const ids = ["w-p0", "w-p1", "w-p2"];
    const rects = layoutRects(buildDefaultTree(ids), BOX);
    const totalW = rects.reduce((s, r) => s + r.w, 0);
    expect(totalW).toBe(BOX.w - 2 * TILE_GAP);
    expect(rects.length).toBe(3);
  });

  it("4 panes: total occupied width = box.w − 3 gaps", () => {
    const ids = ["w-p0", "w-p1", "w-p2", "w-p3"];
    const rects = layoutRects(buildDefaultTree(ids), BOX);
    const totalW = rects.reduce((s, r) => s + r.w, 0);
    expect(totalW).toBe(BOX.w - 3 * TILE_GAP);
  });
});

describe("layoutRects — balanced grid (buildBalancedTree)", () => {
  it("4 panes → 2×2: every rect inside the box, no negative sizes", () => {
    const rects = layoutRects(buildBalancedTree(["w-p0", "w-p1", "w-p2", "w-p3"]), BOX);
    expect(rects.length).toBe(4);
    for (const r of rects) {
      expect(r.w).toBeGreaterThan(0);
      expect(r.h).toBeGreaterThan(0);
      expect(r.x).toBeGreaterThanOrEqual(BOX.x);
      expect(r.y).toBeGreaterThanOrEqual(BOX.y);
      expect(r.x + r.w).toBeLessThanOrEqual(BOX.x + BOX.w);
      expect(r.y + r.h).toBeLessThanOrEqual(BOX.y + BOX.h);
    }
  });

  it("single pane fills the whole box", () => {
    const m = rectsByPane(buildBalancedTree(["w-p0"]));
    expect(m["w-p0"]).toMatchObject({ x: 0, y: 0, w: 1200, h: 800 });
  });
});
