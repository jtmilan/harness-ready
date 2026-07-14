// Force-directed layout core — unit tests (pure math, Node, no DOM), mirroring
// lightning-core.test.js. The sim is a dt-normalized port of the Base44
// reference physics (repulsion r=120 k=0.3, spring rest=200 k=0.01, damping
// 0.92/frame, no walls/gravity/wander) — these pin its invariants.
import { describe, it, expect } from "vitest";
import { createForceSim, stepForceSim, pinNode, releaseNode, settleForceSim } from "./force-core.js";
import { jagOffsets1D } from "./lightning-core.js";

const mkNodes = (coords) => coords.map(([x, y], i) => ({ id: "n" + i, x, y }));

describe("createForceSim", () => {
  it("restores seeded positions by note id and jitters unseeded nodes", () => {
    const nodes = mkNodes([[100, 100], [200, 200]]);
    createForceSim(nodes, [], [], {
      width: 800,
      height: 600,
      seedPositions: { n0: { x: 321, y: 123 } },
    });
    expect(nodes[0].x).toBe(321);
    expect(nodes[0].y).toBe(123);
    // Unseeded node moved off its exact input coords (deterministic jitter) but nearby.
    expect(nodes[1].x).not.toBe(200);
    expect(Math.abs(nodes[1].x - 200)).toBeLessThanOrEqual(24);
    expect(Math.abs(nodes[1].y - 200)).toBeLessThanOrEqual(24);
  });
});

describe("stepForceSim (reference model)", () => {
  it("separates overlapping nodes toward the 120px repulsion radius, all finite", () => {
    const nodes = mkNodes([[400, 300], [400, 300], [400, 300]]);
    const sim = createForceSim(nodes, [], [], { width: 800, height: 600 });
    settleForceSim(sim, 400);
    const d01 = Math.hypot(nodes[0].x - nodes[1].x, nodes[0].y - nodes[1].y);
    const d02 = Math.hypot(nodes[0].x - nodes[2].x, nodes[0].y - nodes[2].y);
    expect(d01).toBeGreaterThan(60); // pushed well apart (repulsion dies at 120)
    expect(d02).toBeGreaterThan(60);
    for (const nd of nodes) {
      expect(Number.isFinite(nd.x)).toBe(true);
      expect(Number.isFinite(nd.y)).toBe(true);
    }
  });

  it("a linked pair converges to the 200px spring rest length", () => {
    // Start at 600 apart (outside the 120 repulsion cutoff): the ONLY force is
    // the spring, so equilibrium is exactly rest=200 (repulsion stays off ≥120).
    const nodes = mkNodes([[100, 300], [700, 300]]);
    const sim = createForceSim(nodes, [0], [1], { width: 800, height: 600 });
    settleForceSim(sim, 900);
    const d = Math.hypot(nodes[0].x - nodes[1].x, nodes[0].y - nodes[1].y);
    expect(d).toBeGreaterThan(150);
    expect(d).toBeLessThan(260);
  });

  it("settles to a genuinely static equilibrium (no wander/gravity forces)", () => {
    const nodes = mkNodes([[300, 300], [520, 340], [400, 500]]);
    const sim = createForceSim(nodes, [0, 1], [1, 2], { width: 800, height: 600 });
    settleForceSim(sim, 1200);
    const snap = nodes.map((nd) => ({ x: nd.x, y: nd.y }));
    for (let i = 0; i < 30; i++) stepForceSim(sim, 0, 50 / 3);
    for (let i = 0; i < nodes.length; i++) {
      expect(Math.abs(nodes[i].x - snap[i].x)).toBeLessThan(0.5);
      expect(Math.abs(nodes[i].y - snap[i].y)).toBeLessThan(0.5);
    }
  });

  it("a pinned node stays exactly where the pointer put it; release hands it back", () => {
    const nodes = mkNodes([[300, 300], [340, 300]]);
    const sim = createForceSim(nodes, [0], [1], { width: 800, height: 600 });
    pinNode(sim, 0, 111, 222);
    for (let i = 0; i < 60; i++) stepForceSim(sim, i * 16, 16);
    expect(nodes[0].x).toBe(111);
    expect(nodes[0].y).toBe(222);
    releaseNode(sim, 0);
    for (let i = 61; i < 160; i++) stepForceSim(sim, i * 16, 16);
    // back under sim control — the spring (rest 200, current ~230+) moves it
    expect(nodes[0].x === 111 && nodes[0].y === 222).toBe(false);
  });

  it("dt is capped at one frame: a huge clock jump never slingshots", () => {
    const nodes = mkNodes([[400, 300], [420, 300]]);
    const sim = createForceSim(nodes, [], [], { width: 800, height: 600 });
    stepForceSim(sim, 0, 16);
    const before = { x: nodes[0].x, y: nodes[0].y };
    stepForceSim(sim, 60000, 60000); // returned from a hidden tab
    const moved = Math.hypot(nodes[0].x - before.x, nodes[0].y - before.y);
    // ≤ one ordinary frame of this (violent, close-range) configuration — the
    // same displacement a normal 16ms step would produce, never 60s worth.
    const beforeNormal = { x: nodes[0].x, y: nodes[0].y };
    stepForceSim(sim, 60016, 16);
    const normalStep = Math.hypot(nodes[0].x - beforeNormal.x, nodes[0].y - beforeNormal.y);
    expect(moved).toBeLessThan(Math.max(normalStep * 3, 120));
  });

  it("empty graph is a no-op", () => {
    const sim = createForceSim([], [], [], { width: 800, height: 600 });
    expect(() => stepForceSim(sim, 0, 16)).not.toThrow();
  });

  it("groups cluster: same-group nodes end nearer each other than cross-group", () => {
    // Two groups of 3, no edges, scattered start. After settling, mean
    // intra-group distance must be well under mean inter-group distance.
    const nodes = mkNodes([[100, 100], [700, 500], [400, 300], [150, 500], [650, 120], [420, 320]]);
    const groups = [0, 0, 0, 1, 1, 1];
    const sim = createForceSim(nodes, [], [], { width: 800, height: 600, groups });
    settleForceSim(sim, 900);
    const d = (a, b) => Math.hypot(nodes[a].x - nodes[b].x, nodes[a].y - nodes[b].y);
    const intra = (d(0, 1) + d(0, 2) + d(1, 2) + d(3, 4) + d(3, 5) + d(4, 5)) / 6;
    let inter = 0;
    for (const a of [0, 1, 2]) for (const b of [3, 4, 5]) inter += d(a, b);
    inter /= 9;
    expect(intra).toBeLessThan(inter * 0.75);
  });
});

describe("jagOffsets1D (parametric bolt offsets)", () => {
  it("pins both endpoints at exactly 0 and fills 2^g+1 stations", () => {
    const out = new Float32Array(9);
    const n = jagOffsets1D(3, out, { rng: () => 0.9 });
    expect(n).toBe(9);
    expect(out[0]).toBe(0);
    expect(out[8]).toBe(0);
    expect(Math.abs(out[4])).toBeGreaterThan(0);
  });

  it("is deterministic under an injected rng and bounded by the decay envelope", () => {
    const mk = () => {
      const out = new Float32Array(9);
      let i = 0;
      const seq = [0.9, 0.1, 0.6, 0.4, 0.75, 0.25, 0.5];
      jagOffsets1D(3, out, { decay: 0.55, rng: () => seq[i++ % seq.length] });
      return out;
    };
    const a = mk();
    const b = mk();
    expect(Array.from(a)).toEqual(Array.from(b));
    const envelope = 1 + 0.55 + 0.55 * 0.55;
    for (const v of a) expect(Math.abs(v)).toBeLessThanOrEqual(envelope + 1e-6);
  });

  it("throws RangeError when the slab is too small (fail loud)", () => {
    expect(() => jagOffsets1D(3, new Float32Array(8))).toThrow(RangeError);
  });
});
