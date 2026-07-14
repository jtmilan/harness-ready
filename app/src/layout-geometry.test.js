import { describe, it, expect } from "vitest";
import {
  TILE_GAP,
  MIN_V,
  MIN_H,
  splitAxis,
  layoutRects,
  collectSplits,
  seamOf,
} from "./layout-geometry.js";

const leaf = (pane) => ({ t: "leaf", pane });
const vsplit = (a, b, ratio = 0.5) => ({ t: "split", dir: "v", ratio, a, b });
const hsplit = (a, b, ratio = 0.5) => ({ t: "split", dir: "h", ratio, a, b });
const box = (x, y, w, h) => ({ x, y, w, h });

describe("splitAxis", () => {
  it("carves the gutter out first, then splits by ratio", () => {
    // 1000 total → 994 available; 0.5 → 497 / 497
    expect(splitAxis(1000, 0.5, MIN_V)).toEqual([497, 497]);
  });

  it("floors each side at min when there's room for two", () => {
    // avail 994 >= 2*160 → clamp a into [160, 994-160]
    expect(splitAxis(1000, 0.01, MIN_V)).toEqual([160, 834]);
    expect(splitAxis(1000, 0.99, MIN_V)).toEqual([834, 160]);
  });

  it("degrades gracefully when there isn't room for two minimums", () => {
    // avail = 200 < 2*160 → no min floor, just clamp into [0, avail]
    const [a, b] = splitAxis(206, 0.5, MIN_V);
    expect(a + b).toBe(200);
    expect(a).toBeGreaterThanOrEqual(0);
    expect(a).toBeLessThanOrEqual(200);
  });

  it("returns [0,0] when the box is smaller than the gutter", () => {
    expect(splitAxis(TILE_GAP, 0.5, MIN_V)).toEqual([0, 0]);
    expect(splitAxis(0, 0.5, MIN_V)).toEqual([0, 0]);
  });
});

describe("layoutRects", () => {
  it("returns empty for a null tree", () => {
    expect(layoutRects(null, box(0, 0, 100, 100))).toEqual([]);
  });

  it("gives a single leaf the whole box", () => {
    expect(layoutRects(leaf("p0"), box(0, 0, 800, 600))).toEqual([
      { pane: "p0", x: 0, y: 0, w: 800, h: 600 },
    ]);
  });

  it("slices WIDTH for a vertical split, offsetting the 2nd pane past the gutter", () => {
    const rects = layoutRects(vsplit(leaf("a"), leaf("b")), box(0, 0, 1000, 600));
    const [aw, bw] = splitAxis(1000, 0.5, MIN_V);
    expect(rects[0]).toEqual({ pane: "a", x: 0, y: 0, w: aw, h: 600 });
    expect(rects[1]).toEqual({ pane: "b", x: aw + TILE_GAP, y: 0, w: bw, h: 600 });
  });

  it("slices HEIGHT for a horizontal split", () => {
    const rects = layoutRects(hsplit(leaf("a"), leaf("b")), box(0, 0, 800, 1000));
    const [ah, bh] = splitAxis(1000, 0.5, MIN_H);
    expect(rects[0]).toEqual({ pane: "a", x: 0, y: 0, w: 800, h: ah });
    expect(rects[1]).toEqual({ pane: "b", x: 0, y: ah + TILE_GAP, w: 800, h: bh });
  });

  it("recurses into nested splits (one rect per leaf, all panes present)", () => {
    const tree = vsplit(leaf("a"), hsplit(leaf("b"), leaf("c")));
    const rects = layoutRects(tree, box(0, 0, 1200, 800));
    expect(rects.map((r) => r.pane).sort()).toEqual(["a", "b", "c"]);
    // no rect exceeds the host box
    for (const r of rects) {
      expect(r.x).toBeGreaterThanOrEqual(0);
      expect(r.x + r.w).toBeLessThanOrEqual(1200 + 1);
      expect(r.y + r.h).toBeLessThanOrEqual(800 + 1);
    }
  });
});

describe("collectSplits", () => {
  it("returns no records for a bare leaf", () => {
    const out = [];
    expect(collectSplits(leaf("p0"), box(0, 0, 100, 100), out)).toEqual([]);
  });

  it("collects one record per internal split node with resolved boxes", () => {
    const tree = vsplit(leaf("a"), hsplit(leaf("b"), leaf("c")));
    const out = collectSplits(tree, box(0, 0, 1200, 800), []);
    expect(out).toHaveLength(2);
    expect(out[0].dir).toBe("v");
    expect(out[1].dir).toBe("h");
    // the nested h-split's box is the RIGHT column (offset past the v-split's gutter)
    const [aw] = splitAxis(1200, 0.5, MIN_V);
    expect(out[1].box.x).toBe(aw + TILE_GAP);
  });
});

describe("seamOf", () => {
  it("centers a vertical seam in the gutter for a v-split", () => {
    const rec = { node: vsplit(leaf("a"), leaf("b")), dir: "v", box: box(0, 0, 1000, 600) };
    const [aw] = splitAxis(1000, 0.5, MIN_V);
    expect(seamOf(rec)).toEqual({ left: aw + TILE_GAP / 2, top: 0, height: 600 });
  });

  it("centers a horizontal seam in the gutter for an h-split", () => {
    const rec = { node: hsplit(leaf("a"), leaf("b")), dir: "h", box: box(0, 0, 800, 1000) };
    const [ah] = splitAxis(1000, 0.5, MIN_H);
    expect(seamOf(rec)).toEqual({ top: ah + TILE_GAP / 2, left: 0, width: 800 });
  });
});
