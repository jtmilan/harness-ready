// Harness Ready — split-tree layout model (PURE, no DOM, no globals). Ported verbatim from the
// Agent Teams prod app (app/src/layout-tree.js). The visual layout for a workspace is a binary
// tree: leaves carry a paneId, split nodes hold a direction + ratio. `paneIds` stays the
// AUTHORITATIVE live-pane set — this tree is only a VIEW over it (useTiling reconciles the tree
// against paneIds). Keeping the model pure means it's unit-testable without xterm/Tauri.
//
// Node shapes (tagged union):
//   Leaf:  { t:"leaf", pane:"<paneId>" }
//   Split: { t:"split", dir:"v"|"h", ratio:<0..1>, a:<node>, b:<node> }
// dir "v" = a VERTICAL divider, children side-by-side (columns). dir "h" = a HORIZONTAL
// divider, children stacked (rows). ratio = child `a`'s fraction of the parent box along the
// split axis; `b` gets (1 - ratio). Binary only — an N-way layout is a right-leaning chain,
// so every split owns exactly one divider (keeps the rect walk + divider math trivial).
//
// All exported functions are pure: they take a tree and return a NEW tree (structural copy
// along the mutated path), never mutating the input.

export function leaf(pane) { return { t: "leaf", pane }; }

// In-order DFS list of pane ids. INVARIANT (enforced by reconcileTree): equals the set of
// the workspace's live paneIds.
export function leafPanes(node) {
  if (!node) return [];
  if (node.t === "leaf") return [node.pane];
  return [...leafPanes(node.a), ...leafPanes(node.b)];
}

export function hasPane(tree, paneId) {
  return leafPanes(tree).indexOf(paneId) !== -1;
}

// Locate the leaf carrying paneId: returns { parent, key:'a'|'b' } (parent === null + key null
// when the leaf is the whole tree), or null when the pane isn't present.
export function findParentOf(tree, paneId) {
  if (!tree) return null;
  if (tree.t === "leaf") return tree.pane === paneId ? { parent: null, key: null } : null;
  if (tree.a && tree.a.t === "leaf" && tree.a.pane === paneId) return { parent: tree, key: "a" };
  if (tree.b && tree.b.t === "leaf" && tree.b.pane === paneId) return { parent: tree, key: "b" };
  return findParentOf(tree.a, paneId) || findParentOf(tree.b, paneId);
}

// Replace the leaf carrying paneId with a split of { old-leaf, new-leaf }. `where` "before"/
// "top" → the new pane becomes child `a`; "after"/"bottom" → child `b`. Returns a new tree;
// if the leaf isn't found, returns the tree unchanged.
export function splitLeaf(tree, paneId, dir, newPaneId, where) {
  const nl = leaf(newPaneId);
  const before = where === "before" || where === "top";
  const recur = (node) => {
    if (!node) return node;
    if (node.t === "leaf") {
      if (node.pane !== paneId) return node;
      return { t: "split", dir, ratio: 0.5, a: before ? nl : node, b: before ? node : nl };
    }
    return { ...node, a: recur(node.a), b: recur(node.b) };
  };
  return recur(tree);
}

// Remove the leaf carrying paneId; its SIBLING takes the parent split's slot. A root leaf
// removed → null. Splits always keep two children, so a deeper removal can never strand a
// null child (the direct-child collapse handles it).
export function removeLeaf(tree, paneId) {
  if (!tree) return null;
  if (tree.t === "leaf") return tree.pane === paneId ? null : tree;
  if (tree.a && tree.a.t === "leaf" && tree.a.pane === paneId) return tree.b;
  if (tree.b && tree.b.t === "leaf" && tree.b.pane === paneId) return tree.a;
  return { ...tree, a: removeLeaf(tree.a, paneId), b: removeLeaf(tree.b, paneId) };
}

// Move src next to target: prune src, then split target with src in the given dir/where.
export function moveLeaf(tree, srcId, targetId, dir, where) {
  if (srcId === targetId) return tree;
  const pruned = removeLeaf(tree, srcId);
  if (!pruned) return tree;            // src was the only pane — nothing to move
  if (!hasPane(pruned, targetId)) return tree; // target gone (shouldn't happen) — no-op
  return splitLeaf(pruned, targetId, dir, srcId, where);
}

// Right-leaning vertical chain: [p0,p1,p2] → v(p0, v(p1,p2)) — equal-width side-by-side
// columns, matching what a user sees today before touching arrangement.
export function buildDefaultTree(paneIds) {
  return chain((paneIds || []).slice(), "v");
}
function chain(ids, dir) {
  if (!ids.length) return null;
  let node = leaf(ids[ids.length - 1]);
  for (let i = ids.length - 2; i >= 0; i--) {
    node = { t: "split", dir, ratio: 1 / (ids.length - i), a: leaf(ids[i]), b: node };
  }
  return node;
}

// Optional visually-balanced migration: ceil(sqrt(n)) columns, each a vertical stack — closer
// to the legacy auto-grid look. v1 uses buildDefaultTree; this is available if migration look matters.
export function buildBalancedTree(paneIds) {
  const ids = (paneIds || []).slice();
  if (!ids.length) return null;
  if (ids.length === 1) return leaf(ids[0]);
  const cols = Math.ceil(Math.sqrt(ids.length));
  const buckets = Array.from({ length: cols }, () => []);
  ids.forEach((id, i) => buckets[i % cols].push(id));
  const colTrees = buckets.filter((b) => b.length).map((b) => chainNodes(b.map(leaf), "h"));
  return chainNodes(colTrees, "v");
}
function chainNodes(nodes, dir) {
  if (!nodes.length) return null;
  let node = nodes[nodes.length - 1];
  for (let i = nodes.length - 2; i >= 0; i--) {
    node = { t: "split", dir, ratio: 1 / (nodes.length - i), a: nodes[i], b: node };
  }
  return node;
}

// Self-heal the tree against the authoritative live set: prune leaves whose pane is no longer
// live (sibling promoted), then append any live pane missing from the tree by splitting the
// focused leaf (or the last leaf) with an alternating direction (tmux-style balance). Returns
// { tree }. Idempotent — safe to call on every relayout. This is also the MIGRATION path: a
// null tree + live panes → buildDefaultTree fallback happens at the call site.
export function reconcileTree(tree, livePaneIds, focusedPaneId) {
  const live = new Set(livePaneIds || []);
  let t = pruneDead(tree, live);
  const present = new Set(leafPanes(t));
  const missing = (livePaneIds || []).filter((id) => !present.has(id));
  for (const id of missing) {
    if (!t) { t = leaf(id); present.add(id); continue; }
    const panes = leafPanes(t);
    const anchor = focusedPaneId && present.has(focusedPaneId) ? focusedPaneId : panes[panes.length - 1];
    const pp = findParentOf(t, anchor);
    const dir = pp && pp.parent ? (pp.parent.dir === "v" ? "h" : "v") : "v";
    t = splitLeaf(t, anchor, dir, id, "after");
    present.add(id);
  }
  return { tree: t };
}
function pruneDead(node, live) {
  if (!node) return null;
  if (node.t === "leaf") return live.has(node.pane) ? node : null;
  const a = pruneDead(node.a, live);
  const b = pruneDead(node.b, live);
  if (a && b) return { ...node, a, b };
  return a || b || null; // one child gone → sibling promoted; both gone → null
}

// Persistence: serialize with the pane INDEX (the -pN suffix) so the tree is keyed in the same
// idx-space as idxList/harnesses/roles and survives the id-rebuild on reopen. paneIdxOf maps a
// paneId → integer index (or null/undefined to drop the leaf).
export function serializeTree(node, paneIdxOf) {
  if (!node) return null;
  if (node.t === "leaf") {
    const idx = paneIdxOf(node.pane);
    return typeof idx === "number" ? { t: "leaf", i: idx } : null;
  }
  const a = serializeTree(node.a, paneIdxOf);
  const b = serializeTree(node.b, paneIdxOf);
  if (a && b) return { t: "split", dir: node.dir, ratio: node.ratio, a, b };
  return a || b || null;
}
export function deserializeTree(node, wsId) {
  if (!node) return null;
  if (node.t === "leaf") return typeof node.i === "number" ? leaf(`${wsId}-p${node.i}`) : null;
  const a = deserializeTree(node.a, wsId);
  const b = deserializeTree(node.b, wsId);
  if (a && b) return { t: "split", dir: node.dir, ratio: node.ratio, a, b };
  return a || b || null;
}
