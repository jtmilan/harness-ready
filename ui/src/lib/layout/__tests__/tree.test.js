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
  buildGridLayout,
  isGridSpacer,
} from "../tree.js";
import { layoutRects } from "../geometry.js";

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

// buildGridLayout — the tile-mode builder. The whole point of this builder is cross-column row
// alignment: every column must hold the same number of leaves (real panes + spacer padding) so the
// per-column h-chain ratios match and the horizontal seams line up. The legacy buildBalancedTree /
// reconcileTree paths produced ragged columns ([3,2,2] for 7 panes) → misaligned seams.
describe("buildGridLayout — aligned grid", () => {
  // Walk the top-level v-chain into its column subtrees (right-leaning v-chain of h-chains).
  const columnsOf = (node) => {
    const cols = [];
    let n = node;
    while (n && n.t === "split" && n.dir === "v") {
      cols.push(n.a);
      n = n.b;
    }
    if (n) cols.push(n);
    return cols;
  };
  const leavesOf = (node) => {
    if (!node) return [];
    if (node.t === "leaf") return [node.pane];
    return [...leavesOf(node.a), ...leavesOf(node.b)];
  };

  it("pads every column to equal length so row seams align (6 → 2×3)", () => {
    const ids = ["w0", "w1", "w2", "w3", "w4", "w5"];
    const cols = columnsOf(buildGridLayout(ids, null));
    expect(cols).toHaveLength(2);
    const lens = cols.map((c) => leavesOf(c).length);
    expect(lens).toEqual([3, 3]); // equal ⇒ aligned
    // row-major fill: col0 = rows' left cells, col1 = rows' right cells
    expect(leavesOf(cols[0])).toEqual(["w0", "w2", "w4"]);
    expect(leavesOf(cols[1])).toEqual(["w1", "w3", "w5"]);
  });

  it("pads a non-divisible count with spacer leaves (5 → 2×3, one spacer)", () => {
    const cols = columnsOf(buildGridLayout(["w0", "w1", "w2", "w3", "w4"], null));
    const lens = cols.map((c) => leavesOf(c).length);
    expect(lens).toEqual([3, 3]);
    const all = cols.map((c) => leavesOf(c)).flat();
    expect(all).toHaveLength(6); // 5 panes + 1 spacer
    expect(all.filter((p) => isGridSpacer(p))).toHaveLength(1);
    // DFS / column-major order: col0 = [w0,w2,w4], col1 = [w1,w3,spacer]
    expect(all.filter((p) => !isGridSpacer(p))).toEqual(["w0", "w2", "w4", "w1", "w3"]);
  });

  it("pins the coordinator full-height left and grids workers on the right", () => {
    const ids = ["coord", "w0", "w1", "w2", "w3", "w4", "w5"];
    const t = buildGridLayout(ids, "coord");
    expect(t.dir).toBe("v");
    expect(t.a).toEqual({ t: "leaf", pane: "coord" });
    const cols = columnsOf(t.b);
    expect(cols).toHaveLength(2);
    expect(cols.map((c) => leavesOf(c).length)).toEqual([3, 3]);
    expect(cols.flatMap((c) => leavesOf(c))).toEqual(["w0", "w2", "w4", "w1", "w3", "w5"]);
  });

  it("ignores a coordinator id that is not in the pane set", () => {
    const t = buildGridLayout(["w0", "w1", "w2", "w3"], "ghost");
    // no coord pin → root's left child is a column, not a single pinned pane
    expect(t.a.t).not.toBe("leaf");
    // DFS order is column-major (col0 then col1): [w0,w2] then [w1,w3]
    expect(leafPanes(t)).toEqual(["w0", "w2", "w1", "w3"]);
  });

  // The load-bearing invariant: same-row panes get identical y/height (and same-col identical
  // x/width). This is exactly what the broken layout violated (ragged seams in the screenshot).
  it("lays out workers on a pixel-aligned grid (coordinator + 6 workers)", () => {
    const ids = ["coord", "w0", "w1", "w2", "w3", "w4", "w5"];
    const rects = {};
    for (const r of layoutRects(buildGridLayout(ids, "coord"), { x: 0, y: 0, w: 2000, h: 900 })) {
      rects[r.pane] = r;
    }
    // row-major rows: [w0,w1], [w2,w3], [w4,w5]
    for (const [a, b] of [["w0", "w1"], ["w2", "w3"], ["w4", "w5"]]) {
      expect(rects[a].y).toBe(rects[b].y);
      expect(rects[a].h).toBe(rects[b].h);
    }
    for (const col of [["w0", "w2", "w4"], ["w1", "w3", "w5"]]) {
      for (let i = 1; i < col.length; i++) {
        expect(rects[col[i]].x).toBe(rects[col[0]].x);
        expect(rects[col[i]].w).toBe(rects[col[0]].w);
      }
    }
    // coordinator = full-height left column; workers share the narrower right region
    expect(rects.coord.y).toBe(0);
    expect(rects.coord.h).toBe(900);
    expect(rects.w0.x).toBeGreaterThan(rects.coord.x);
    const workerW = rects.w0.w;
    expect(workerW).toBeLessThan(rects.coord.w);
    // Worker columns are equal to within 1px: splitAxis uses Math.round per binary split, so a
    // right-leaning v-chain can drift by 1px on odd available widths (pre-existing geometry
    // behaviour, invisible to the eye; a pixel-exact grid would need an N-way grid primitive).
    for (const w of ["w1", "w2", "w3", "w4", "w5"]) {
      expect(Math.abs(rects[w].w - workerW)).toBeLessThanOrEqual(1);
    }
  });
});
