// Force-directed layout core for the memory "Second Brain" graph — PURE math
// (no DOM, no three.js, no globals), mirroring graph-core.js / lightning-core.js
// so vitest exercises it in Node with zero setup.
//
// This is a dt-NORMALIZED port of the Base44 "Synapse" reference physics
// (extracted verbatim — see .claude/context/2026-07-05-force-directed-brain-graph.md
// and scratchpad/base44/SPEC.md §D). Their model, per 60Hz frame:
//   • pairwise repulsion ONLY within radius 120: f = (120 - d) / d * 0.3
//   • spring per connection: rest 200, k 0.01, symmetric
//   • forces add STRAIGHT INTO VELOCITY, then v *= 0.92 damping, Euler x += v
//   • NO center gravity, NO walls, NO velocity clamp, NO wander — the layout
//     settles to a static equilibrium (the app's "life" is pulse/bolts, not drift)
// Deviation (named in the brief): we scale by dt/16.67 so 120Hz displays don't
// run the sim 2× fast like the original does, and clamp dt so a hidden-tab
// return integrates as one small step.
//
// The sim OWNS velocities in typed arrays but writes positions back into the
// caller's node objects (node.x / node.y) every step. A dragged node is
// "pinned": its position is pointer-driven, its velocity zeroed, and
// integration skips it while everything else still reacts around it (matches
// the original's drag: direct set + zeroed velocity).

// Reference constants (SPEC.md §D/§F) — per 60Hz-frame units.
const REPULSE_RADIUS = 120;
const REPULSE_K = 0.3;
const SPRING_REST = 200;
const SPRING_K = 0.01;
const DAMPING_PER_FRAME = 0.92;
// Category clustering (ours): each group gets an anchor on a ring around the
// canvas center; members feel a weak linear pull toward it. Weak vs the springs
// on purpose — link structure still dominates, categories bias the neighborhoods.
const GROUP_PULL = 0.0045; // px/frame² per px toward the group anchor
const GROUP_RING_RATIO = 0.3; // anchor ring radius = min(w,h) × this
const FRAME_MS = 50 / 3; // 16.6667 — the 60Hz frame the reference constants assume

// Deterministic 0..1 hash (same trick as memory-lightning's hash01) — used for
// the deterministic placement jitter; keeps the sim rng-free and testable.
function hash01(x) {
  const s = Math.sin(x * 127.1 + 311.7) * 43758.5453123;
  return s - Math.floor(s);
}

// Build sim state over the caller's laid `nodes` (objects with x/y — mutated in
// place each step) and index-based edges (edgeA[i] ↔ edgeB[i] into `nodes`).
// `seedPositions` (optional { [id]: {x,y} }) restores a previous session's
// layout so a re-render (save/reload) doesn't re-scramble the map; nodes not in
// the seed keep their layoutGraph coords plus a deterministic jitter so
// perfectly-stacked seeds separate.
// `groups` (optional array, one int per node, -1 = ungrouped): members of the
// same group share an anchor on a ring around the canvas center and drift into
// a shared neighborhood (category clustering).
export function createForceSim(nodes, edgeA, edgeB, opts = {}) {
  const n = nodes.length;
  const width = Math.max(1, opts.width || 800);
  const height = Math.max(1, opts.height || 600);
  const seed = opts.seedPositions || null;
  const groups = Array.isArray(opts.groups) && opts.groups.length === n ? opts.groups : null;

  const vx = new Float64Array(n);
  const vy = new Float64Array(n);
  const pinned = new Uint8Array(n);
  for (let i = 0; i < n; i++) {
    const s = seed && nodes[i].id != null ? seed[nodes[i].id] : null;
    if (s && Number.isFinite(s.x) && Number.isFinite(s.y)) {
      nodes[i].x = s.x;
      nodes[i].y = s.y;
    } else {
      nodes[i].x += (hash01(i * 7.31) - 0.5) * 48;
      nodes[i].y += (hash01(i * 3.77 + 9.1) - 0.5) * 48;
    }
  }

  // Group anchors: evenly spaced on a ring, starting at the top. One anchor per
  // distinct group index (0..maxGroup); ungrouped (-1) nodes feel no pull.
  let anchorX = null;
  let anchorY = null;
  if (groups) {
    const g = Math.max(...groups) + 1;
    if (g > 0) {
      anchorX = new Float64Array(g);
      anchorY = new Float64Array(g);
      const R = Math.min(width, height) * GROUP_RING_RATIO;
      for (let k = 0; k < g; k++) {
        const a = -Math.PI / 2 + (Math.PI * 2 * k) / g;
        anchorX[k] = width / 2 + R * Math.cos(a);
        anchorY[k] = height / 2 + R * Math.sin(a);
      }
    }
  }
  return { nodes, edgeA, edgeB, n, width, height, vx, vy, pinned, groups, anchorX, anchorY };
}

// Advance the sim by `dtMs` (t kept in the signature for API stability; the
// reference model is time-free — pure relaxation). Call once per frame.
export function stepForceSim(state, t, dtMs) {
  const { nodes, edgeA, edgeB, n, vx, vy, pinned, groups, anchorX, anchorY } = state;
  if (n === 0) return;
  // At most ONE frame's worth per call — the reference steps once per rAF with
  // no catch-up (a hidden tab resumes with a single normal frame, never a
  // slingshot). 120Hz displays get half-frames (f=0.5) → real-time parity.
  const f = Math.min(Math.max(dtMs || FRAME_MS, 1), FRAME_MS) / FRAME_MS;

  // Pairwise repulsion — ONLY within REPULSE_RADIUS (reference cutoff).
  for (let i = 0; i < n; i++) {
    for (let j = i + 1; j < n; j++) {
      let dx = nodes[j].x - nodes[i].x;
      let dy = nodes[j].y - nodes[i].y;
      let d = Math.hypot(dx, dy);
      if (d === 0) {
        // Perfect overlap: separate along a deterministic pseudo-random axis.
        const a = hash01(i * 31.7 + j * 17.3) * Math.PI * 2;
        dx = Math.cos(a);
        dy = Math.sin(a);
        d = 1;
      }
      if (d < REPULSE_RADIUS) {
        const push = ((REPULSE_RADIUS - d) / d) * REPULSE_K * f;
        vx[i] -= dx * push;
        vy[i] -= dy * push;
        vx[j] += dx * push;
        vy[j] += dy * push;
      }
    }
  }

  // Spring per connection — rest SPRING_REST, stiffness SPRING_K, symmetric.
  for (let e = 0; e < edgeA.length; e++) {
    const a = edgeA[e];
    const b = edgeB[e];
    if (a < 0 || b < 0 || a === b) continue;
    const dx = nodes[b].x - nodes[a].x;
    const dy = nodes[b].y - nodes[a].y;
    const d = Math.hypot(dx, dy) || 1;
    const pull = ((d - SPRING_REST) / d) * SPRING_K * f;
    vx[a] += dx * pull;
    vy[a] += dy * pull;
    vx[b] -= dx * pull;
    vy[b] -= dy * pull;
  }

  // Category clustering: weak linear pull toward the group anchor.
  if (groups && anchorX) {
    for (let i = 0; i < n; i++) {
      const g = groups[i];
      if (g < 0) continue;
      vx[i] += (anchorX[g] - nodes[i].x) * GROUP_PULL * f;
      vy[i] += (anchorY[g] - nodes[i].y) * GROUP_PULL * f;
    }
  }

  // Damping + Euler integration. Pinned (dragged) nodes are pointer-driven.
  const damp = DAMPING_PER_FRAME ** f;
  for (let i = 0; i < n; i++) {
    if (pinned[i]) {
      vx[i] = 0;
      vy[i] = 0;
      continue;
    }
    vx[i] *= damp;
    vy[i] *= damp;
    nodes[i].x += vx[i] * f;
    nodes[i].y += vy[i] * f;
  }
}

// Drag support: pin node i to the pointer (drives position, zeroes velocity;
// neighbors keep reacting), release to hand it back to the sim — the reference
// does NOT re-pin after release, springs/repulsion resume immediately.
export function pinNode(state, i, x, y) {
  if (i < 0 || i >= state.n) return;
  state.pinned[i] = 1;
  state.nodes[i].x = x;
  state.nodes[i].y = y;
}
export function releaseNode(state, i) {
  if (i < 0 || i >= state.n) return;
  state.pinned[i] = 0;
}

// Reduced-motion path: relax to (near-)equilibrium offline and return — the
// caller renders ONE static frame. The reference model has no perpetual forces,
// so it genuinely converges.
export function settleForceSim(state, iterations = 240) {
  for (let i = 0; i < iterations; i++) stepForceSim(state, 0, FRAME_MS);
}
