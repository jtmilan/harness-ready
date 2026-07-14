// Memory-graph lightning overlay — unit tests for the pure math core (lightning-core.js).
//
// These cover the fractal midpoint-displacement math and the GPU segment packing
// only; the rendered Three.js overlay is GUI-verified by the operator. Pure →
// vitest runs them in Node with no DOM (mirrors graph-core.test.js).
import { describe, it, expect } from "vitest";
import {
  jaggedPath,
  pathPointCount,
  segmentFloats,
  packSegments,
  categoryColor,
  hexToRgb01,
  CATEGORY_META,
  categoryShape,
  SHAPE_NAMES,
  strikeEnvelope,
  strikeOvershoot,
  electricColor,
  ELECTRIC_STOPS,
  rollBranch,
  yawX,
  yawDepth,
  nodeZOffset,
} from "./lightning-core.js";

// Cycling deterministic rng stub: walks `values` forever. Values must be [0,1).
const cyclingRng = (values) => {
  let i = 0;
  return () => values[i++ % values.length];
};

// Perpendicular distance from point p to the infinite line through (x1,y1)→(x2,y2).
const perpDist = (p, x1, y1, x2, y2) => {
  const dx = x2 - x1;
  const dy = y2 - y1;
  const len = Math.hypot(dx, dy);
  return Math.abs((p.x - x1) * dy - (p.y - y1) * dx) / len;
};

describe("pathPointCount / segmentFloats", () => {
  it("pathPointCount is 2^generations + 1", () => {
    expect(pathPointCount(0)).toBe(2);
    expect(pathPointCount(1)).toBe(3);
    expect(pathPointCount(2)).toBe(5);
    expect(pathPointCount(3)).toBe(9);
    expect(pathPointCount(4)).toBe(17);
  });

  it("segmentFloats is edgeCount * 2^generations segments * 6 floats", () => {
    expect(segmentFloats(1, 0)).toBe(6);
    expect(segmentFloats(1, 3)).toBe(48);
    expect(segmentFloats(3, 2)).toBe(72);
    expect(segmentFloats(0, 4)).toBe(0);
  });
});

describe("jaggedPath — endpoints & point count", () => {
  it("keeps the exact untouched endpoints and hits pathPointCount for generations 0..4", () => {
    for (let g = 0; g <= 4; g++) {
      const pts = jaggedPath(10, 20, 110, -40, {
        generations: g,
        rng: cyclingRng([0.13, 0.87, 0.5, 0.02, 0.71]),
      });
      expect(pts).toHaveLength(pathPointCount(g));
      expect(pts[0].x).toBe(10);
      expect(pts[0].y).toBe(20);
      expect(pts[pts.length - 1].x).toBe(110);
      expect(pts[pts.length - 1].y).toBe(-40);
    }
  });
});

describe("jaggedPath — determinism via injectable rng", () => {
  it("identical seeded rng → identical polyline", () => {
    const seed = [0.9, 0.1, 0.4, 0.6, 0.25];
    const a = jaggedPath(0, 0, 300, 150, { generations: 3, rng: cyclingRng(seed) });
    const b = jaggedPath(0, 0, 300, 150, { generations: 3, rng: cyclingRng(seed) });
    expect(a).toEqual(b);
  });

  it("different rng → different midpoints (endpoints still exact)", () => {
    const a = jaggedPath(0, 0, 300, 150, { generations: 3, rng: cyclingRng([0.9, 0.1]) });
    const b = jaggedPath(0, 0, 300, 150, { generations: 3, rng: cyclingRng([0.2, 0.7]) });
    expect(a).not.toEqual(b);
    expect(a[0]).toEqual(b[0]);
    expect(a[a.length - 1]).toEqual(b[b.length - 1]);
  });
});

describe("jaggedPath — displacement envelope & decay", () => {
  it("every point stays within the summed decayed-offset envelope of the straight line", () => {
    const generations = 4;
    const offsetRatio = 0.18;
    const decay = 0.55;
    // Worst-case bound: a point placed at generation g starts within the current
    // envelope (midpoint of two in-envelope points) and moves at most the current
    // offset, so max distance ≤ offset0 * (1 + decay + decay^2 + ...).
    const cases = [
      [0, 0, 200, 0],
      [10, 20, 110, -40],
      [-50, 80, 30, 300],
    ];
    for (const [x1, y1, x2, y2] of cases) {
      const offset0 = offsetRatio * Math.hypot(x2 - x1, y2 - y1);
      let envelope = 0;
      for (let g = 0; g < generations; g++) envelope += offset0 * decay ** g;
      const pts = jaggedPath(x1, y1, x2, y2, {
        generations,
        offsetRatio,
        decay,
        rng: cyclingRng([0.99, 0.01, 0.5, 0.83, 0.17, 0.66]),
      });
      for (const p of pts) {
        expect(perpDist(p, x1, y1, x2, y2)).toBeLessThanOrEqual(envelope + 1e-9);
      }
    }
  });

  it("generation-1 midpoint displacement is exactly offsetRatio*length when rng always returns 1", () => {
    const pts = jaggedPath(0, 0, 100, 0, {
      generations: 1,
      offsetRatio: 0.2,
      rng: () => 1, // rand(-offset, +offset) hits +offset exactly
    });
    expect(pts).toHaveLength(3);
    const mid = pts[1];
    expect(mid.x).toBe(50); // displaced only along the perpendicular
    expect(Math.abs(mid.y)).toBeCloseTo(0.2 * 100, 12);
  });

  it("maxOffset caps the initial displacement on long edges; default stays uncapped", () => {
    // Long edge (length 1000): offsetRatio*len = 180, but maxOffset 20 wins.
    const capped = jaggedPath(0, 0, 1000, 0, {
      generations: 1,
      offsetRatio: 0.18,
      maxOffset: 20,
      rng: () => 1, // rand(-offset, +offset) hits +offset exactly
    });
    expect(capped).toHaveLength(3);
    expect(capped[1].x).toBe(500); // displaced only along the perpendicular
    expect(Math.abs(capped[1].y)).toBeCloseTo(20, 12); // exactly maxOffset, not 180
    // Default (no maxOffset): same edge keeps the full offsetRatio*len wander.
    const uncapped = jaggedPath(0, 0, 1000, 0, {
      generations: 1,
      offsetRatio: 0.18,
      rng: () => 1,
    });
    expect(Math.abs(uncapped[1].y)).toBeCloseTo(0.18 * 1000, 12);
  });

  it("decay 0 kills displacement after generation 1 (later midpoints are exact midpoints)", () => {
    const pts = jaggedPath(0, 0, 100, 0, {
      generations: 2,
      offsetRatio: 0.2,
      decay: 0,
      rng: () => 1,
    });
    expect(pts).toHaveLength(5);
    // gen-2 midpoints sit exactly between their (gen-1) neighbours
    expect(pts[1].x).toBeCloseTo((pts[0].x + pts[2].x) / 2, 12);
    expect(pts[1].y).toBeCloseTo((pts[0].y + pts[2].y) / 2, 12);
    expect(pts[3].x).toBeCloseTo((pts[2].x + pts[4].x) / 2, 12);
    expect(pts[3].y).toBeCloseTo((pts[2].y + pts[4].y) / 2, 12);
  });
});

describe("jaggedPath — degenerate input", () => {
  it("zero-length edge yields straight repeated points, never NaN", () => {
    const pts = jaggedPath(5, 5, 5, 5, { generations: 3 });
    expect(pts).toHaveLength(pathPointCount(3));
    for (const p of pts) {
      expect(p.x).toBe(5);
      expect(p.y).toBe(5);
      expect(Number.isNaN(p.x)).toBe(false);
      expect(Number.isNaN(p.y)).toBe(false);
    }
  });
});

describe("packSegments", () => {
  it("round-trips one path into consecutive [ax,ay,z, bx,by,z] segment pairs", () => {
    const path = jaggedPath(0, 0, 64, 32, {
      generations: 2,
      rng: cyclingRng([0.25, 0.75, 0.5]),
    });
    const out = new Float32Array(segmentFloats(1, 2));
    const written = packSegments([path], out, 7);
    expect(written).toBe(segmentFloats(1, 2));
    for (let i = 0; i < path.length - 1; i++) {
      const base = i * 6;
      expect(out[base + 0]).toBe(Math.fround(path[i].x));
      expect(out[base + 1]).toBe(Math.fround(path[i].y));
      expect(out[base + 2]).toBe(7);
      expect(out[base + 3]).toBe(Math.fround(path[i + 1].x));
      expect(out[base + 4]).toBe(Math.fround(path[i + 1].y));
      expect(out[base + 5]).toBe(7);
    }
  });

  it("packs multiple paths in order, each at its own offset, and defaults z to 0", () => {
    const g = 1;
    const a = jaggedPath(0, 0, 10, 0, { generations: g, rng: cyclingRng([0.4]) });
    const b = jaggedPath(0, 20, 10, 20, { generations: g, rng: cyclingRng([0.6]) });
    const out = new Float32Array(segmentFloats(2, g));
    const written = packSegments([a, b], out);
    expect(written).toBe(segmentFloats(2, g));
    const perPath = segmentFloats(1, g);
    // first segment of path b lands right after path a's block
    expect(out[perPath + 0]).toBe(Math.fround(b[0].x));
    expect(out[perPath + 1]).toBe(Math.fround(b[0].y));
    expect(out[perPath + 2]).toBe(0); // default z
    expect(out[2]).toBe(0); // default z on path a too
  });

  it("throws RangeError (fail loud) when out is too small", () => {
    const path = jaggedPath(0, 0, 10, 10, { generations: 2, rng: cyclingRng([0.5]) });
    const tooSmall = new Float32Array(segmentFloats(1, 2) - 1);
    expect(() => packSegments([path], tooSmall)).toThrow(RangeError);
  });
});

describe("categoryColor / hexToRgb01 (orb + edge colors)", () => {
  it("resolves a known category to its palette color", () => {
    const idea = CATEGORY_META.find((c) => c.key === "idea");
    expect(categoryColor({ category: "Idea" })).toBe(idea.color); // case-insensitive
    expect(categoryColor({ category: "idea" })).toBe(idea.color);
  });

  it("is deterministic for uncategorized nodes and stable across the fallback seeds", () => {
    const a = categoryColor({ id: "n1", tags: ["ml"] });
    const b = categoryColor({ id: "n1", tags: ["ml"] });
    expect(a).toBe(b); // same input → same hue (no per-call flicker)
    expect(typeof a).toBe("number");
    // falls back through tags → origin → id without throwing on sparse nodes
    expect(typeof categoryColor({})).toBe("number");
    expect(typeof categoryColor({ origin: "personal" })).toBe("number");
  });

  it("unknown category strings still hash to a color (never undefined)", () => {
    expect(typeof categoryColor({ category: "totally-made-up" })).toBe("number");
  });

  it("hexToRgb01 splits a hex int into 0..1 rgb", () => {
    expect(hexToRgb01(0xff8000)).toEqual([1, 128 / 255, 0]);
    expect(hexToRgb01(0x000000)).toEqual([0, 0, 0]);
    expect(hexToRgb01(0xffffff)).toEqual([1, 1, 1]);
  });
});

describe("strikeEnvelope (cinematic cubic ease-out strike fade)", () => {
  it("blazes at exactly 1 through the whole hold window", () => {
    expect(strikeEnvelope(0, 0.1, 0.85)).toBe(1);
    expect(strikeEnvelope(0.05, 0.1, 0.85)).toBe(1);
    expect(strikeEnvelope(0.1, 0.1, 0.85)).toBe(1);
  });

  it("dies to exactly 0 at end and clamps to 0 after", () => {
    expect(strikeEnvelope(0.85, 0.1, 0.85)).toBe(0);
    expect(strikeEnvelope(0.99, 0.1, 0.85)).toBe(0);
    expect(strikeEnvelope(5, 0.1, 0.85)).toBe(0);
  });

  it("fades as a cubic ease-out: (1-u)^3 over the hold→end span", () => {
    // halfway through the fade → (0.5)^3; quarter through → (0.75)^3
    expect(strikeEnvelope(0.1 + 0.375, 0.1, 0.85)).toBeCloseTo(0.125, 12);
    expect(strikeEnvelope(0.1 + 0.1875, 0.1, 0.85)).toBeCloseTo(0.421875, 12);
  });

  it("is monotonically non-increasing across the whole lifetime", () => {
    let prev = Infinity;
    for (let k = 0; k <= 1.2; k += 0.01) {
      const v = strikeEnvelope(k, 0.1, 0.85);
      expect(v).toBeLessThanOrEqual(prev + 1e-12);
      prev = v;
    }
  });

  it("a glow envelope with a later end outlives its core (the lingering light)", () => {
    const k = 0.9; // past the core's end, before the glow's
    expect(strikeEnvelope(k, 0.1, 0.85)).toBe(0);
    expect(strikeEnvelope(k, 0.1, 1)).toBeGreaterThan(0);
  });

  it("supports ends past 1 (the aura outliving the normalized core lifetime)", () => {
    expect(strikeEnvelope(1.05, 0.1, 1.15)).toBeGreaterThan(0);
    expect(strikeEnvelope(1.15, 0.1, 1.15)).toBe(0);
  });
});

describe("yawX / yawDepth (planetary yaw projection under ortho)", () => {
  it("zero yaw is the identity: x projects to x, depth = the node's own z", () => {
    expect(yawX(340, 400, 120, 1, 0)).toBe(340);
    expect(yawDepth(340, 400, 120, 1, 0)).toBe(120);
  });

  it("quarter turn swaps the axes: z becomes lateral offset, −dx becomes depth", () => {
    expect(yawX(340, 400, 120, 0, 1)).toBe(400 + 120); // cx + z
    expect(yawDepth(340, 400, 120, 0, 1)).toBe(60); // −(340−400)
  });

  it("is a rigid rotation: dx² + z² is preserved for any yaw", () => {
    const x = 512, cx = 400, z = -75;
    const r2 = (x - cx) ** 2 + z ** 2;
    for (const th of [0.3, 1.2, 2.5, 4.0]) {
      const c = Math.cos(th);
      const s = Math.sin(th);
      const px = yawX(x, cx, z, c, s) - cx;
      const pd = yawDepth(x, cx, z, c, s);
      expect(px * px + pd * pd).toBeCloseTo(r2, 9);
    }
  });

  it("the pivot axis is fixed: a node at (cx, z=0) never moves", () => {
    for (const th of [0, 0.7, 2.1]) {
      const c = Math.cos(th);
      const s = Math.sin(th);
      expect(yawX(400, 400, 0, c, s)).toBe(400);
      expect(yawDepth(400, 400, 0, c, s)).toBeCloseTo(0, 12); // −0·sin → negative zero; toBe would trip on Object.is
    }
  });
});

describe("nodeZOffset (category shells + per-node jitter)", () => {
  it("a single category centers the shell — depth is jitter-only", () => {
    expect(nodeZOffset(0, 1, 0.5, 220, 90)).toBe(0);
    expect(nodeZOffset(0, 1, 1, 220, 90)).toBeCloseTo(45, 12); // +jitterAmp/2
    expect(nodeZOffset(0, 0, 0, 220, 90)).toBeCloseTo(-45, 12); // −jitterAmp/2
  });

  it("shells span ±spread/2 symmetrically across the category range", () => {
    expect(nodeZOffset(0, 3, 0.5, 200, 0)).toBeCloseTo(-100, 12);
    expect(nodeZOffset(1, 3, 0.5, 200, 0)).toBeCloseTo(0, 12);
    expect(nodeZOffset(2, 3, 0.5, 200, 0)).toBeCloseTo(100, 12);
  });

  it("total depth is bounded by ±(spread + jitterAmp)/2", () => {
    const bound = (220 + 90) / 2;
    for (const g of [0, 1, 2, 3]) {
      for (const j of [0, 0.5, 0.999]) {
        expect(Math.abs(nodeZOffset(g, 4, j, 220, 90))).toBeLessThanOrEqual(bound);
      }
    }
  });
});

describe("strikeOvershoot (camera-flash pops at birth + re-strikes)", () => {
  const EVENTS = [0, 0.1, 0.22]; // birth + the re-strike moments

  it("peaks at exactly 1 + boost at each event instant", () => {
    for (const e of EVENTS) {
      expect(strikeOvershoot(e, EVENTS, 0.15, 0.4)).toBeCloseTo(1.4, 12);
    }
  });

  it("returns exactly 1 outside every flash window (before, between, after)", () => {
    expect(strikeOvershoot(-0.01, EVENTS, 0.02, 0.4)).toBe(1); // before birth
    expect(strikeOvershoot(0.05, EVENTS, 0.02, 0.4)).toBe(1); // between pops
    expect(strikeOvershoot(0.9, EVENTS, 0.02, 0.4)).toBe(1); // long after
    expect(strikeOvershoot(0.22 + 0.02, EVENTS, 0.02, 0.4)).toBe(1); // window end is exclusive
  });

  it("eases back on a cubic: partway through a window the pop is boost·(1-u)³", () => {
    expect(strikeOvershoot(0.075, [0], 0.15, 0.4)).toBeCloseTo(1 + 0.4 * 0.125, 12); // u = 1/2
    expect(strikeOvershoot(0.0375, [0], 0.15, 0.4)).toBeCloseTo(1 + 0.4 * 0.421875, 12); // u = 1/4
  });

  it("is monotonically non-increasing within a single window", () => {
    let prev = Infinity;
    for (let k = 0; k < 0.15; k += 0.005) {
      const v = strikeOvershoot(k, [0], 0.15, 0.4);
      expect(v).toBeLessThanOrEqual(prev + 1e-12);
      prev = v;
    }
  });

  it("overlapping windows take the STRONGEST pop, never the sum", () => {
    // At k = 0.1 both the birth window (decayed) and the re-strike window
    // (fresh) are live — the fresh one wins outright and the result never
    // exceeds 1 + boost (no stacking).
    const v = strikeOvershoot(0.1, [0, 0.1], 0.15, 0.4);
    expect(v).toBeCloseTo(1.4, 12);
    expect(v).toBeLessThanOrEqual(1.4);
  });
});

describe("electricColor (fixed electric palette ramp)", () => {
  it("hits the exact stop colors at their stop positions", () => {
    const out = new Float32Array(3);
    for (const [t, hex] of ELECTRIC_STOPS) {
      electricColor(t, out);
      const [r, g, b] = hexToRgb01(hex);
      expect(out[0]).toBeCloseTo(r, 6);
      expect(out[1]).toBeCloseTo(g, 6);
      expect(out[2]).toBeCloseTo(b, 6);
    }
  });

  it("clamps t outside [0,1] to the end stops (violet fringe / white-hot)", () => {
    const out = new Float32Array(3);
    const violet = hexToRgb01(ELECTRIC_STOPS[0][1]);
    electricColor(-3, out);
    for (let c = 0; c < 3; c++) expect(out[c]).toBeCloseTo(violet[c], 6);
    electricColor(7, out);
    expect([...out]).toEqual([1, 1, 1]);
  });

  it("lerps linearly midway between adjacent stops", () => {
    const out = new Float32Array(3);
    const [t0, h0] = ELECTRIC_STOPS[2];
    const [t1, h1] = ELECTRIC_STOPS[3];
    electricColor((t0 + t1) / 2, out);
    const a = hexToRgb01(h0);
    const b = hexToRgb01(h1);
    for (let c = 0; c < 3; c++) expect(out[c]).toBeCloseTo((a[c] + b[c]) / 2, 6);
  });

  it("writes at the given base offset and returns out (render-loop out-param style)", () => {
    const slab = new Float32Array(9).fill(-1);
    const ret = electricColor(1, slab, 3);
    expect(ret).toBe(slab);
    expect([...slab.subarray(3, 6)]).toEqual([1, 1, 1]);
    expect(slab[0]).toBe(-1); // neighbours untouched
    expect(slab[6]).toBe(-1);
  });
});

describe("rollBranch (strike fork parameters)", () => {
  it("writes [t, angle, len] inside the default ranges for any rng value", () => {
    // Bounds go through Float32Array storage — compare against f32-rounded
    // range edges (Math.fround) or exact boundary values fail by ~1e-8.
    const out = new Float32Array(3);
    for (const r of [0, 0.25, 0.5, 0.75, 0.999]) {
      rollBranch(() => r, out);
      expect(out[0]).toBeGreaterThanOrEqual(Math.fround(0.25)); // bud station: interior only
      expect(out[0]).toBeLessThanOrEqual(Math.fround(0.75));
      expect(Math.abs(out[1])).toBeGreaterThanOrEqual(Math.fround(0.35));
      expect(Math.abs(out[1])).toBeLessThanOrEqual(Math.fround(0.95));
      expect(out[2]).toBeGreaterThanOrEqual(Math.fround(0.25)); // reach: 25–45% of the parent span
      expect(out[2]).toBeLessThanOrEqual(Math.fround(0.45));
    }
  });

  it("consumes exactly four rng draws (t, sign, magnitude, reach)", () => {
    let calls = 0;
    const rng = () => { calls++; return 0.5; };
    rollBranch(rng, new Float32Array(3));
    expect(calls).toBe(4);
  });

  it("signs the fork angle from the second draw (fair coin)", () => {
    const out = new Float32Array(3);
    rollBranch(cyclingRng([0.5, 0.1, 0.5, 0.5]), out); // 2nd draw < 0.5 → negative
    expect(out[1]).toBeLessThan(0);
    rollBranch(cyclingRng([0.5, 0.9, 0.5, 0.5]), out); // 2nd draw ≥ 0.5 → positive
    expect(out[1]).toBeGreaterThan(0);
  });

  it("respects range overrides and writes at the base offset only", () => {
    const slab = new Float32Array(6).fill(-1);
    rollBranch(() => 0, slab, 3, { tMin: 0.4, tMax: 0.6, angMin: 0.2, angMax: 0.3, lenMin: 0.1, lenMax: 0.2 });
    expect(slab[3]).toBeCloseTo(0.4, 6);
    expect(Math.abs(slab[4])).toBeCloseTo(0.2, 6);
    expect(slab[5]).toBeCloseTo(0.1, 6);
    expect(slab[0]).toBe(-1); // neighbours untouched
    expect(slab[1]).toBe(-1);
    expect(slab[2]).toBe(-1);
  });
});

describe("categoryShape (silhouette encodes context)", () => {
  it("maps known categories to fixed, distinct shape indices", () => {
    const idx = CATEGORY_META.map((c) => categoryShape({ category: c.key }));
    expect(new Set(idx).size).toBe(CATEGORY_META.length); // all distinct
    for (const i of idx) {
      expect(i).toBeGreaterThanOrEqual(0);
      expect(i).toBeLessThan(SHAPE_NAMES.length);
    }
    expect(categoryShape({ category: "Idea" })).toBe(categoryShape({ category: "idea" })); // case-insensitive
  });

  it("is deterministic for unknown/uncategorized nodes (never undefined)", () => {
    const a = categoryShape({ id: "n1", tags: ["ml"] });
    expect(a).toBe(categoryShape({ id: "n1", tags: ["ml"] }));
    expect(typeof categoryShape({})).toBe("number");
  });
});
