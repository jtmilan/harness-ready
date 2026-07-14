// Memory-graph lightning overlay — PURE math core (no DOM, no three.js, no globals).
//
// The Three.js animation layer (memory-lightning.js) draws each memory-graph edge
// as a procedurally jagged "lightning bolt". This module owns only the math:
//   1. jaggedPath      — classic fractal-lightning midpoint displacement, producing
//                        a polyline between two layoutGraph endpoints, and
//   2. packSegments    — packing many such polylines into one preallocated
//                        Float32Array of line-SEGMENT pairs for a GPU buffer.
// Keeping it pure means vitest can exercise it in Node with zero setup (mirrors
// graph-core.js / kanban-core.js).
//
// Coordinate space is the same pixel space graph-core's layoutGraph emits
// (x right, y down); z is a constant plane supplied by the caller.

// Number of points a jaggedPath polyline has after `generations` rounds of
// midpoint displacement: each generation doubles the segment count, so the
// polyline ends at 2^generations segments = 2^generations + 1 points.
export function pathPointCount(generations) {
  return 2 ** generations + 1;
}

// Floats needed to pack `edgeCount` paths (all built with the same `generations`)
// as line-segment PAIRS: each path has 2^generations segments, each segment is
// 2 points × 3 floats (x, y, z). The caller sizes its Float32Array with this and
// hands it to packSegments.
export function segmentFloats(edgeCount, generations) {
  return edgeCount * (2 ** generations) * 6;
}

// Procedural jagged lightning polyline between (x1,y1) and (x2,y2) — recursive
// perpendicular midpoint displacement, implemented iteratively (array doubling
// per generation) which is simpler to reason about than recursion.
//
// opts: {
//   generations = 3     — rounds of subdivision (result: 2^generations + 1 points)
//   offsetRatio = 0.18  — initial displacement amplitude as a fraction of the
//                         straight endpoint-to-endpoint length
//   maxOffset   = Infinity — absolute cap (same pixel units) on the initial
//                         displacement amplitude, so long edges don't wander
//   decay       = 0.55  — amplitude multiplier applied after every generation
//   rng         = Math.random — injectable [0,1) source for deterministic tests
// }
//
// Each generation displaces every current segment's midpoint along that
// segment's 2D perpendicular by rand(-offset, +offset). The FIRST and LAST
// points are always the exact untouched endpoints. A zero-length input edge
// (offset 0, no perpendicular) yields straight repeated points — never NaN.
//
// Returns [{ x, y }, ...] with exactly pathPointCount(generations) points.
export function jaggedPath(x1, y1, x2, y2, opts = {}) {
  const generations = opts.generations == null ? 3 : opts.generations;
  const offsetRatio = opts.offsetRatio == null ? 0.18 : opts.offsetRatio;
  const maxOffset = opts.maxOffset == null ? Infinity : opts.maxOffset;
  const decay = opts.decay == null ? 0.55 : opts.decay;
  const rng = opts.rng || Math.random;

  let pts = [{ x: x1, y: y1 }, { x: x2, y: y2 }];
  let offset = Math.min(offsetRatio * Math.hypot(x2 - x1, y2 - y1), maxOffset);

  for (let g = 0; g < generations; g++) {
    const next = new Array(pts.length * 2 - 1);
    next[0] = pts[0];
    for (let i = 0; i < pts.length - 1; i++) {
      const a = pts[i];
      const b = pts[i + 1];
      const dx = b.x - a.x;
      const dy = b.y - a.y;
      const len = Math.hypot(dx, dy);
      // rand(-offset, +offset) along the segment's unit perpendicular. A
      // degenerate (zero-length) segment has no perpendicular — leave the
      // midpoint in place rather than divide by zero (NaN guard). rng is
      // still consumed so the stream stays aligned across segments.
      const d = (rng() * 2 - 1) * offset;
      const px = len === 0 ? 0 : -dy / len;
      const py = len === 0 ? 0 : dx / len;
      next[2 * i + 1] = {
        x: (a.x + b.x) / 2 + px * d,
        y: (a.y + b.y) / 2 + py * d,
      };
      next[2 * i + 2] = b;
    }
    pts = next;
    offset *= decay;
  }
  return pts;
}

// ── Parametric jag: 1D midpoint-displacement offsets ─────────────────────────
// For a MOVING edge (force-layout nodes drift every frame) the absolute-coords
// jaggedPath can't be cached — its endpoints go stale. Instead we precompute a
// polyline as PERPENDICULAR offsets from the straight chord at fixed parametric
// stations t_j = j / 2^generations, and reconstruct positions per frame from the
// LIVE endpoints:  P_j = lerp(A, B, t_j) + perp(A→B) * off[j] * scale.
// off[] is unitless (amplitude 1 at generation 0, decaying), endpoints pinned at
// exactly 0 so the bolt always meets its nodes. Same recursion as jaggedPath,
// collapsed to the offset axis. Writes into a caller-provided Float32Array slab
// (length 2^generations + 1) — zero allocation on the animation path.
export function jagOffsets1D(generations, out, opts = {}) {
  const decay = opts.decay == null ? 0.55 : opts.decay;
  const rng = opts.rng || Math.random;
  const count = 2 ** generations + 1;
  if (out.length < count) {
    throw new RangeError(`jagOffsets1D: out holds ${out.length}, need ${count}`);
  }
  out[0] = 0;
  out[count - 1] = 0;
  // Iterative midpoint displacement over the index range, generation by
  // generation: stride halves each round, amplitude decays.
  let amp = 1;
  let stride = count - 1;
  for (let g = 0; g < generations; g++) {
    const half = stride >> 1;
    for (let i = 0; i + stride <= count - 1; i += stride) {
      out[i + half] = (out[i] + out[i + stride]) / 2 + (rng() * 2 - 1) * amp;
    }
    stride = half;
    amp *= decay;
  }
  return count;
}

// ── Strike lifecycle: cinematic fade envelope ────────────────────────────────
// Intensity of a thunder strike at normalized lifetime k (0 = birth, 1 =
// retirement): full blaze (exactly 1) through the hold window, then a CUBIC
// EASE-OUT fade — (1-u)³ over the hold→end span — hitting exactly 0 at `end`
// with zero slope, so the light dies softly instead of popping off. k past
// `end` clamps to 0. Two passes over the same bolt use two envelopes: the CORE
// with an early `end`, its GLOW with end = 1 — the glow outlives the core by
// (endGlow − endCore) · duration (the lingering afterlight).
export function strikeEnvelope(k, hold, end) {
  if (k <= hold) return 1;
  if (k >= end) return 0;
  const u = 1 - (k - hold) / (end - hold);
  return u * u * u;
}

// ── Strike overshoot: camera-flash pops ──────────────────────────────────────
// Multiplicative intensity factor (≥ 1) that gives the strike light a brief
// overshoot flash at each event time (birth, re-strikes): at k = e the factor
// jumps to exactly 1 + boost and eases back to exactly 1 by k = e + span on a
// cubic (1-u)³ — a camera-flash pop, not a step. Overlapping windows take the
// STRONGEST pop (max, never sum — flashes don't stack); k outside every
// window returns exactly 1. `events` is a caller-hoisted array (ascending not
// required); zero allocation — safe on the per-frame path.
export function strikeOvershoot(k, events, span, boost) {
  let best = 0;
  for (let i = 0; i < events.length; i++) {
    const u = (k - events[i]) / span;
    if (u >= 0 && u < 1) {
      const inv = 1 - u;
      const p = inv * inv * inv;
      if (p > best) best = p;
    }
  }
  return 1 + boost * best;
}

// ── Planetary yaw: projection + depth (ORTHOGRAPHIC camera) ──────────────────
// The Second Brain cloud rotates slowly about the VERTICAL axis through the
// view center (x = cx). The force sim stays 2D; each node carries a stable
// z-offset. Under the renderer's orthographic camera a yaw θ affects ONLY the
// projected x and the depth — y never moves:
//   projected x:  cx + dx·cosθ + z·sinθ        (yawX)
//   depth:        −dx·sinθ + z·cosθ            (yawDepth; + = toward camera)
// These two are the SINGLE source of truth for everything CPU-side that must
// agree with the GPU's group rotation: DOM label chips, hover hit-testing,
// hover-card anchors, and the depth-cue attribute. Pure scalars — zero
// allocation on the per-frame path.
export function yawX(x, cx, z, cos, sin) {
  return cx + (x - cx) * cos + z * sin;
}
export function yawDepth(x, cx, z, cos, sin) {
  return -(x - cx) * sin + z * cos;
}

// Stable per-node z-offset: category SHELLS (each category group sits at its
// own depth band — the "orbital shells" of the planetary read; x/y clustering
// already groups them laterally) plus a per-node jitter inside the shell so a
// category never reads as a coplanar sheet. groupCount ≤ 1 centers the shell.
// jitter01 is a stable [0,1) hash — same input, same depth, forever.
export function nodeZOffset(group, groupCount, jitter01, spread, jitterAmp) {
  const shell = groupCount > 1 ? group / (groupCount - 1) - 0.5 : 0;
  return shell * spread + (jitter01 - 0.5) * jitterAmp;
}

// ── Branch (fork) parameter rolling ──────────────────────────────────────────
// Roll one fork of a strike into out[base..base+2] as [tStation, angle, lenFrac]:
//   tStation — parametric station on the parent bolt the fork buds from,
//              uniform in [tMin, tMax] (interior only — forks never bud off the
//              endpoints, which sit under the node discs)
//   angle    — radians off the parent CHORD direction; magnitude uniform in
//              [angMin, angMax], sign a fair coin
//   lenFrac  — fork reach as a fraction of the PARENT span, uniform in
//              [lenMin, lenMax]
// Consumes exactly FOUR rng draws (t, sign, magnitude, reach) so deterministic
// streams stay aligned across calls. Out-param write (jagOffsets1D style) —
// zero allocation. Returns out.
export function rollBranch(rng, out, base = 0, opts = {}) {
  const tMin = opts.tMin == null ? 0.25 : opts.tMin;
  const tMax = opts.tMax == null ? 0.75 : opts.tMax;
  const angMin = opts.angMin == null ? 0.35 : opts.angMin;
  const angMax = opts.angMax == null ? 0.95 : opts.angMax;
  const lenMin = opts.lenMin == null ? 0.25 : opts.lenMin;
  const lenMax = opts.lenMax == null ? 0.45 : opts.lenMax;
  out[base] = tMin + rng() * (tMax - tMin);
  const sign = rng() < 0.5 ? -1 : 1;
  out[base + 1] = sign * (angMin + rng() * (angMax - angMin));
  out[base + 2] = lenMin + rng() * (lenMax - lenMin);
  return out;
}

// ── Category palette — the SINGLE source of truth shared by the renderer (orb +
// edge colors) and the edit UI (dropdown). Pure data + pure functions, so it
// stays in this DOM-free module and vitest can exercise the resolver. Colors are
// neon-on-dark; `key` is what the backend stores in Note.category (lowercased).
// Hexes lifted verbatim from the Base44 "Synapse" reference app (see
// .claude/context brief + scratchpad/base44/SPEC.md §B) — our keys, their colors.
export const CATEGORY_META = [
  { key: "idea", label: "Idea", emoji: "💡", color: 0x63b3ff }, // blue (ref: idea)
  { key: "note", label: "Note", emoji: "📝", color: 0xa882ff }, // purple (ref: note)
  { key: "question", label: "Question", emoji: "❓", color: 0xff63b1 }, // pink (ref: question)
  { key: "reference", label: "Reference", emoji: "📚", color: 0x63ffc4 }, // emerald (ref: reference)
  { key: "project", label: "Project", emoji: "🚀", color: 0xffa763 }, // orange (ref: task)
  { key: "person", label: "Person", emoji: "👤", color: 0xc4ff63 }, // lime (ours — no ref sibling)
  { key: "topic", label: "Topic", emoji: "🗂️", color: 0xffec63 }, // yellow (ref: insight)
];
// Palette used to give UNCATEGORIZED nodes a stable, distinct color (the reference
// colors every orb, never grey) — hashed from a stable key so a node keeps its hue
// across reloads.
const FALLBACK_PALETTE = [0x63b3ff, 0xffa763, 0xa882ff, 0x63ffc4, 0xff63b1, 0xffec63, 0xc4ff63];
const CATEGORY_BY_KEY = new Map(CATEGORY_META.map((c) => [c.key, c]));

// Stable non-negative 32-bit string hash (djb2) — deterministic across reloads and
// Node/browser, so a node's fallback color never flickers between sessions.
function hashStr(s) {
  let h = 5381;
  for (let i = 0; i < s.length; i++) h = ((h << 5) + h + s.charCodeAt(i)) | 0;
  return h >>> 0;
}

// Node SHAPES — the silhouette encodes the thought's context (category), so
// different kinds of notes read apart at a glance. Indices are consumed by the
// renderer's SDF fragment shader; keep this list in sync with its branch chain.
export const SHAPE_NAMES = ["circle", "square", "diamond", "hexagon", "triangle", "pentagon", "ring"];
const SHAPE_BY_CATEGORY = new Map([
  ["idea", 0], // circle — the classic thought orb
  ["note", 1], // rounded square — a card
  ["question", 2], // diamond — decision/unknown
  ["reference", 3], // hexagon — structured source
  ["project", 4], // triangle — direction/movement
  ["person", 5], // pentagon — distinct silhouette for people
  ["topic", 6], // ring — a container of things
]);

// Resolve a graph node to a shape index (0..SHAPE_NAMES.length-1). Known
// categories map fixedly; unknown/uncategorized hash to a stable shape from the
// same seed chain categoryColor uses, so a node keeps its silhouette forever.
export function categoryShape(node) {
  const cat = node && typeof node.category === "string" ? node.category.trim().toLowerCase() : "";
  const known = cat ? SHAPE_BY_CATEGORY.get(cat) : undefined; // NOTE: index 0 is valid — no && chain
  if (known != null) return known;
  const seed =
    cat ||
    (node && Array.isArray(node.tags) && node.tags.length ? String(node.tags[0]) : "") ||
    (node && node.origin ? String(node.origin) : "") ||
    (node && node.id ? String(node.id) : "x");
  return hashStr(seed) % SHAPE_NAMES.length;
}

// Resolve a graph node to an orb/edge color (hex int). A known `category` wins;
// otherwise fall back to the first tag / origin / id, hashed into FALLBACK_PALETTE
// so every node reads as a distinct hue (matching the reference's colored orbs).
export function categoryColor(node) {
  const cat = node && typeof node.category === "string" ? node.category.trim().toLowerCase() : "";
  const known = cat && CATEGORY_BY_KEY.get(cat);
  if (known) return known.color;
  const seed =
    cat ||
    (node && Array.isArray(node.tags) && node.tags.length ? String(node.tags[0]) : "") ||
    (node && node.origin ? String(node.origin) : "") ||
    (node && node.id ? String(node.id) : "x");
  return FALLBACK_PALETTE[hashStr(seed) % FALLBACK_PALETTE.length];
}

// Hex int → [r, g, b] floats in 0..1, for per-vertex color attributes (the fat-line
// color buffer and the node aColor attribute both want linear 0..1 triplets).
export function hexToRgb01(hex) {
  return [((hex >> 16) & 0xff) / 255, ((hex >> 8) & 0xff) / 255, (hex & 0xff) / 255];
}

// ── Electric palette — the FIXED color ramp of a thunder strike ──────────────
// Lightning reads as electricity regardless of the edge's category hue. Ramp
// position t is the strike's intensity: 1 = white-hot flash, down through
// blue-white body and deep electric blue, to the violet fringe of the
// afterglow at 0. Stops are [t, hex], ascending; tuned against the renderer's
// near-black field (CLEAR_COLOR 0x08080f).
export const ELECTRIC_STOPS = [
  [0.0, 0x8b5cf6], // violet fringe — the afterglow's last light
  [0.45, 0x7aa2ff], // deep electric blue
  [0.75, 0x9db8ff], // blue-white body
  [1.0, 0xffffff], // white-hot core
];
const ELECTRIC_RGB = ELECTRIC_STOPS.map(([t, hex]) => [t, ...hexToRgb01(hex)]);

// Sample the ramp at t (clamped to [0,1]) into out[base..base+2] — an
// out-param write so the render loop can sample every frame with zero
// allocation. Exact stop colors at exact stop positions; linear lerp between.
// Returns out.
export function electricColor(t, out, base = 0) {
  const tc = t < 0 ? 0 : t > 1 ? 1 : t;
  let i = 1;
  while (i < ELECTRIC_RGB.length - 1 && tc > ELECTRIC_RGB[i][0]) i++;
  const lo = ELECTRIC_RGB[i - 1];
  const hi = ELECTRIC_RGB[i];
  const span = hi[0] - lo[0];
  const u = span === 0 ? 0 : (tc - lo[0]) / span;
  out[base] = lo[1] + (hi[1] - lo[1]) * u;
  out[base + 1] = lo[2] + (hi[2] - lo[2]) * u;
  out[base + 2] = lo[3] + (hi[3] - lo[3]) * u;
  return out;
}

// Pack polylines into a preallocated Float32Array as consecutive line-segment
// pairs [ax, ay, z, bx, by, z] — one pair per adjacent point pair of every path,
// paths in order. Never allocates; returns the number of floats written. If
// `out` cannot hold every segment the call throws RangeError up front (fail
// loud, graph-core style) rather than truncating silently.
export function packSegments(paths, out, z = 0) {
  let needed = 0;
  for (let p = 0; p < paths.length; p++) {
    needed += Math.max(0, paths[p].length - 1) * 6;
  }
  if (out.length < needed) {
    throw new RangeError(
      `packSegments: out holds ${out.length} floats, need ${needed}`,
    );
  }
  let w = 0;
  for (let p = 0; p < paths.length; p++) {
    const pts = paths[p];
    for (let i = 0; i < pts.length - 1; i++) {
      const a = pts[i];
      const b = pts[i + 1];
      out[w++] = a.x;
      out[w++] = a.y;
      out[w++] = z;
      out[w++] = b.x;
      out[w++] = b.y;
      out[w++] = z;
    }
  }
  return w;
}
