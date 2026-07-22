// layout-geometry — pure split-tree → pixel-rect math. Ported verbatim from the Agent Teams prod
// app (app/src/layout-geometry.js), lifted out of main.js's tile renderer.
//
// tree.js owns the STRUCTURE (split/remove/move/reconcile a tree of leaves); this owns the
// GEOMETRY (slice a host box recursively per the tree's ratios into absolute pane rects + the
// divider seams between them). No DOM, no globals — the caller keeps the host box (which reads
// container.clientWidth) and the divider DOM builders, and feeds a plain {x,y,w,h} box in here.
//
// Node shape (same as tree.js): { t:"leaf", pane } | { t:"split", dir:"v"|"h", ratio, a, b }.
// A "v" split slices WIDTH (side-by-side columns); an "h" split slices HEIGHT (stacked rows).

export const TILE_GAP = 6; // gutter between tiles, == the old grid gap
export const MIN_V = 160;  // min pane width floor (~enough for ≥20 cols @ typical mono)
// Min full-pane height must leave room for AgentPane chrome (header ~36 + branch ~28 +
// padding) PLUS ≥5–6 xterm rows @ fontSize 11 (~13px/row) so the sized-gate can open.
// 90px was below chrome+6rows → legal tiles could leave OpenCode/Pi blank forever
// while agent.raw was full (writes held until cols≥20/rows≥5).
export const MIN_H = 160;

// Split `total` px along an axis at `ratio`, flooring each side at `min` when there's room.
// Returns [a, b] where a + b == total − TILE_GAP (the gutter is carved out first).
export function splitAxis(total, ratio, min) {
  const avail = total - TILE_GAP;
  if (avail <= 0) return [0, 0];
  let a = Math.round(avail * ratio);
  if (avail >= 2 * min) a = Math.max(min, Math.min(avail - min, a));
  else a = Math.max(0, Math.min(avail, a));
  return [a, avail - a];
}

// Recursively slice `box` per the split tree → a flat list of { pane, x, y, w, h } leaf rects.
export function layoutRects(node, box) {
  if (!node) return [];
  if (node.t === "leaf") return [{ pane: node.pane, x: box.x, y: box.y, w: box.w, h: box.h }];
  if (node.dir === "v") {
    const [aw, bw] = splitAxis(box.w, node.ratio, MIN_V);
    return [
      ...layoutRects(node.a, { x: box.x, y: box.y, w: aw, h: box.h }),
      ...layoutRects(node.b, { x: box.x + aw + TILE_GAP, y: box.y, w: bw, h: box.h }),
    ];
  }
  const [ah, bh] = splitAxis(box.h, node.ratio, MIN_H);
  return [
    ...layoutRects(node.a, { x: box.x, y: box.y, w: box.w, h: ah }),
    ...layoutRects(node.b, { x: box.x, y: box.y + ah + TILE_GAP, w: box.w, h: bh }),
  ];
}

// Collect every internal split node WITH its resolved box (for divider placement + resize),
// appending { node, dir, box } records into `out`. Returns `out` for convenience.
export function collectSplits(node, box, out) {
  if (!node || node.t !== "split") return out;
  out.push({ node, dir: node.dir, box: { ...box } });
  if (node.dir === "v") {
    const [aw, bw] = splitAxis(box.w, node.ratio, MIN_V);
    collectSplits(node.a, { x: box.x, y: box.y, w: aw, h: box.h }, out);
    collectSplits(node.b, { x: box.x + aw + TILE_GAP, y: box.y, w: bw, h: box.h }, out);
  } else {
    const [ah, bh] = splitAxis(box.h, node.ratio, MIN_H);
    collectSplits(node.a, { x: box.x, y: box.y, w: box.w, h: ah }, out);
    collectSplits(node.b, { x: box.x, y: box.y + ah + TILE_GAP, w: box.w, h: bh }, out);
  }
  return out;
}

// The seam (divider) rect for a split record. A "v" split → a vertical bar (left/top/height);
// an "h" split → a horizontal bar (top/left/width). Centered in the TILE_GAP gutter.
export function seamOf(rec) {
  const { node, box } = rec;
  if (node.dir === "v") { const [aw] = splitAxis(box.w, node.ratio, MIN_V); return { left: box.x + aw + TILE_GAP / 2, top: box.y, height: box.h }; }
  const [ah] = splitAxis(box.h, node.ratio, MIN_H); return { top: box.y + ah + TILE_GAP / 2, left: box.x, width: box.w };
}
