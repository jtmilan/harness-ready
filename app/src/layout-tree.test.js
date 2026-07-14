import { test, expect } from "vitest";
import {
  leaf, leafPanes, hasPane, findParentOf, splitLeaf, removeLeaf, moveLeaf,
  buildDefaultTree, buildBalancedTree, reconcileTree, serializeTree, deserializeTree,
} from "./layout-tree.js";

test("buildDefaultTree: empty / single / chain", () => {
  expect(buildDefaultTree([])).toBe(null);
  expect(buildDefaultTree(["p0"])).toEqual({ t: "leaf", pane: "p0" });
  const t = buildDefaultTree(["p0", "p1", "p2"]);
  expect(leafPanes(t)).toEqual(["p0", "p1", "p2"]);
  // right-leaning v-chain: a=p0, b=split(p1,p2)
  expect(t.t).toBe("split");
  expect(t.dir).toBe("v");
  expect(t.a).toEqual({ t: "leaf", pane: "p0" });
  expect(t.b.t).toBe("split");
});

test("splitLeaf: replaces a leaf with a split, where decides side", () => {
  const t0 = buildDefaultTree(["p0"]);
  const after = splitLeaf(t0, "p0", "v", "p1", "after");
  expect(after).toEqual({ t: "split", dir: "v", ratio: 0.5, a: { t: "leaf", pane: "p0" }, b: { t: "leaf", pane: "p1" } });
  const before = splitLeaf(t0, "p0", "h", "p1", "before");
  expect(before.dir).toBe("h");
  expect(before.a).toEqual({ t: "leaf", pane: "p1" });
  expect(before.b).toEqual({ t: "leaf", pane: "p0" });
  // unknown leaf → unchanged
  expect(splitLeaf(t0, "zzz", "v", "p9", "after")).toEqual(t0);
});

test("splitLeaf does not mutate the input tree", () => {
  const t0 = buildDefaultTree(["p0", "p1"]);
  const snapshot = JSON.stringify(t0);
  splitLeaf(t0, "p1", "h", "p2", "after");
  expect(JSON.stringify(t0)).toBe(snapshot);
});

test("removeLeaf: sibling is promoted; root leaf → null", () => {
  const t = buildDefaultTree(["p0", "p1", "p2"]); // v(p0, v(p1,p2))
  const r = removeLeaf(t, "p0");
  expect(leafPanes(r)).toEqual(["p1", "p2"]); // sibling subtree promoted to root
  const r2 = removeLeaf(t, "p1");
  expect(leafPanes(r2)).toEqual(["p0", "p2"]);
  expect(removeLeaf(buildDefaultTree(["only"]), "only")).toBe(null);
  // unknown pane → unchanged set
  expect(leafPanes(removeLeaf(t, "ghost"))).toEqual(["p0", "p1", "p2"]);
});

test("findParentOf: root leaf, direct child, deep child", () => {
  const t = buildDefaultTree(["p0", "p1", "p2"]);
  expect(findParentOf(t, "p0")).toEqual({ parent: t, key: "a" });
  const root = buildDefaultTree(["solo"]);
  expect(findParentOf(root, "solo")).toEqual({ parent: null, key: null });
  expect(findParentOf(t, "p2")).not.toBe(null); // deep leaf still found
  expect(findParentOf(t, "nope")).toBe(null);
});

test("moveLeaf: prune src then split target", () => {
  const t = buildDefaultTree(["p0", "p1", "p2"]);
  const m = moveLeaf(t, "p0", "p2", "h", "after");
  expect(new Set(leafPanes(m))).toEqual(new Set(["p0", "p1", "p2"])); // no pane lost
  // p0 now sits next to p2 under an "h" split
  const pp = findParentOf(m, "p0");
  expect(pp.parent.dir).toBe("h");
  expect(hasPane(pp.parent, "p2")).toBe(true);
  // moving onto self → unchanged
  expect(moveLeaf(t, "p1", "p1", "v", "after")).toEqual(t);
});

test("reconcileTree: prunes dead, appends missing, idempotent", () => {
  const t = buildDefaultTree(["p0", "p1", "p2"]);
  // p1 died → pruned, sibling promoted
  const a = reconcileTree(t, ["p0", "p2"], "p0").tree;
  expect(new Set(leafPanes(a))).toEqual(new Set(["p0", "p2"]));
  // a new live pane p3 appears → appended (split off the focused leaf)
  const b = reconcileTree(a, ["p0", "p2", "p3"], "p0").tree;
  expect(new Set(leafPanes(b))).toEqual(new Set(["p0", "p2", "p3"]));
  // idempotent: same live set → same pane set
  const c = reconcileTree(b, ["p0", "p2", "p3"], "p0").tree;
  expect(new Set(leafPanes(c))).toEqual(new Set(["p0", "p2", "p3"]));
  // null tree + live panes → builds from scratch
  const d = reconcileTree(null, ["x", "y"], null).tree;
  expect(new Set(leafPanes(d))).toEqual(new Set(["x", "y"]));
  // all dead → null
  expect(reconcileTree(t, [], null).tree).toBe(null);
});

test("serialize/deserialize round-trips on pane index", () => {
  const wsId = "ws9";
  const t = buildDefaultTree([`${wsId}-p0`, `${wsId}-p1`, `${wsId}-p2`]);
  const idxOf = (pane) => Number(/-p(\d+)$/.exec(pane)[1]);
  const ser = serializeTree(t, idxOf);
  expect(JSON.stringify(ser)).not.toContain(wsId); // keyed by index, not full id
  const back = deserializeTree(ser, wsId);
  expect(leafPanes(back)).toEqual(leafPanes(t));
  expect(back.dir).toBe(t.dir);
});

test("buildBalancedTree covers all panes", () => {
  for (const n of [1, 2, 3, 4, 5, 7, 9]) {
    const ids = Array.from({ length: n }, (_, i) => `p${i}`);
    expect(new Set(leafPanes(buildBalancedTree(ids)))).toEqual(new Set(ids));
  }
});
