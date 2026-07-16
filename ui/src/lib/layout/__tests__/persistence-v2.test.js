// T1/T2 contract tests — tree persistence v2 + reconcile survivor ratios.
// Pure layer only (no useTiling / React). p1 lands T1/T2 in parallel; probe for the
// v2 surface and (ready ? it : it.todo) so this lane's gate stays green on base
// without soft assertions (wave-6 idiom: workspaceAssign.test.js:69-76).
import { describe, it, expect } from "vitest";
import * as treeMod from "../tree.js";
import {
  leaf,
  leafPanes,
  buildDefaultTree,
  reconcileTree,
  serializeTree,
  deserializeTree,
} from "../tree.js";

// ─── probes (static export / shape presence — not runtime soft-pass) ─────────

/** True when deserialize accepts T1 leaf shape `{t:"leaf", id:"<fullPaneId>"}`. */
function probeDeserializeV2() {
  try {
    const n = deserializeTree({ t: "leaf", id: "ws48213x0-p0" }, "ws-1");
    return !!(n && n.t === "leaf" && n.pane === "ws48213x0-p0");
  } catch {
    return false;
  }
}

/**
 * True when serialize emits T1 leaves with full ids (mapper may be ignored or absent).
 * v1 returns null when paneIdxOf yields a non-number — that is the negative probe.
 */
function probeSerializeV2() {
  const node = leaf("ws48213x0-p0");
  try {
    // Prefer 1-arg form if present.
    if (serializeTree.length <= 1) {
      const ser = serializeTree(node);
      return leafIdOf(ser) === "ws48213x0-p0" || leafIdOf(unwrapEnvelope(ser)) === "ws48213x0-p0";
    }
    // Mapper that always fails v1's typeof-number check.
    const ser = serializeTree(node, () => null);
    if (!ser) return false;
    return leafIdOf(ser) === "ws48213x0-p0" || leafIdOf(unwrapEnvelope(ser)) === "ws48213x0-p0";
  } catch {
    return false;
  }
}

/** Optional pure helper for storage envelopes `{v:2, tree}` (name not pinned — probe common ones). */
function findStoredParser() {
  const names = [
    "parseStoredTree",
    "decodeStoredTree",
    "parsePersistedTree",
    "loadStoredTree",
    "unwrapStoredTree",
    "readStoredTree",
  ];
  for (const n of names) {
    if (typeof treeMod[n] === "function") return treeMod[n];
  }
  return null;
}

function unwrapEnvelope(ser) {
  if (ser && typeof ser === "object" && ser.v === 2 && "tree" in ser) return ser.tree;
  return ser;
}

function leafIdOf(node) {
  if (!node || node.t !== "leaf") return null;
  // T1 wire uses `id`; runtime tree leaves still use `pane`.
  return node.id ?? node.pane ?? null;
}

function serializeV2(node) {
  if (serializeTree.length <= 1) return serializeTree(node);
  return serializeTree(node, () => null);
}

function deepRatios(node) {
  if (!node) return [];
  if (node.t === "leaf") return [];
  return [node.ratio, ...deepRatios(node.a), ...deepRatios(node.b)];
}

const v2Ready = probeDeserializeV2() && probeSerializeV2();
const storedParser = findStoredParser();
const envelopeReady = typeof storedParser === "function";

// Real spawn-shaped ids (T1 collision case: two groups both have p0).
const ID_A0 = "ws48213x0-p0";
const ID_A1 = "ws48213x0-p1";
const ID_B0 = "ws51772x0-p0";

/** Hand-built non-default tree used by round-trip + ratio pins. */
function collisionTree() {
  return {
    t: "split",
    dir: "v",
    ratio: 0.42,
    a: leaf(ID_A0),
    b: {
      t: "split",
      dir: "h",
      ratio: 0.618,
      a: leaf(ID_A1),
      b: leaf(ID_B0),
    },
  };
}

// ─── T1: serialize → deserialize round-trip ──────────────────────────────────

describe("T1 persistence v2 — full-id round-trip", () => {
  (v2Ready ? it : it.todo)(
    "preserves structure, ratios, and exact leaf ids across spawn-group collision ids",
    () => {
      const original = collisionTree();
      const ser = serializeV2(original);
      const wire = unwrapEnvelope(ser);
      // Wire leaves must carry FULL ids, not -pN indices.
      expect(wire.t).toBe("split");
      expect(wire.ratio).toBe(0.42);
      expect(leafIdOf(wire.a)).toBe(ID_A0);
      expect(wire.b.t).toBe("split");
      expect(wire.b.ratio).toBe(0.618);
      expect(leafIdOf(wire.b.a)).toBe(ID_A1);
      expect(leafIdOf(wire.b.b)).toBe(ID_B0);

      // deserializeTree second arg (legacy wsId) must not rewrite ids under v2.
      const back = deserializeTree(wire, "ws-1");
      expect(leafPanes(back)).toEqual([ID_A0, ID_A1, ID_B0]);
      expect(back.ratio).toBe(0.42);
      expect(back.dir).toBe("v");
      expect(back.b.ratio).toBe(0.618);
      expect(back.b.dir).toBe("h");
      expect(deepRatios(back)).toEqual([0.42, 0.618]);
    },
  );

  (v2Ready ? it : it.todo)(
    "mock-id regression: ids with NO -pN suffix (agent-001) round-trip",
    () => {
      // Preview bug: paneIdxOf null for every mock id → serialize emptied the tree.
      const original = {
        t: "split",
        dir: "v",
        ratio: 0.55,
        a: leaf("agent-001"),
        b: leaf("agent-002"),
      };
      const ser = serializeV2(original);
      const wire = unwrapEnvelope(ser);
      expect(leafIdOf(wire.a)).toBe("agent-001");
      expect(leafIdOf(wire.b)).toBe("agent-002");
      const back = deserializeTree(wire, "ws-1");
      expect(leafPanes(back)).toEqual(["agent-001", "agent-002"]);
      expect(back.ratio).toBe(0.55);
    },
  );

  (v2Ready ? it : it.todo)(
    "buildDefaultTree of real-shaped ids survives serialize→deserialize",
    () => {
      const ids = [ID_A0, ID_A1, ID_B0];
      const t0 = buildDefaultTree(ids);
      const back = deserializeTree(unwrapEnvelope(serializeV2(t0)), "ws-1");
      expect(leafPanes(back)).toEqual(leafPanes(t0));
      expect(deepRatios(back)).toEqual(deepRatios(t0));
    },
  );
});

// ─── T1: rejection of non-v2 stored shapes ───────────────────────────────────

describe("T1 persistence v2 — rejection of legacy / malformed input", () => {
  // When only deserializeTree is the pure surface, old index leaves must not
  // resurrect as `${wsId}-p${i}` under v2 (that was the id-space mismatch).
  (v2Ready ? it : it.todo)(
    "old index-format leaf {t:leaf,i:0} is rejected (null / empty)",
    () => {
      const n = deserializeTree({ t: "leaf", i: 0 }, "ws-1");
      // Must not produce a live leaf keyed by UI ws id.
      expect(n == null || leafPanes(n).length === 0).toBe(true);
    },
  );

  (v2Ready ? it : it.todo)(
    "old index-format nested split is rejected or yields no index-rewritten leaves",
    () => {
      const old = {
        t: "split",
        dir: "v",
        ratio: 0.5,
        a: { t: "leaf", i: 0 },
        b: { t: "leaf", i: 1 },
      };
      const n = deserializeTree(old, "ws-1");
      if (n) {
        for (const id of leafPanes(n)) {
          expect(id.startsWith("ws-1-p")).toBe(false);
        }
      } else {
        expect(n).toBeNull();
      }
    },
  );

  // Envelope parse (if p1 exposes a pure helper). Otherwise todo — rejection may
  // live only inside useTiling's lsGet path (hook, not pure).
  (envelopeReady ? it : it.todo)(
    'stored "" is discarded',
    () => {
      expect(storedParser("")).toBeNull();
    },
  );

  (envelopeReady ? it : it.todo)(
    'stored "null" is discarded',
    () => {
      expect(storedParser("null")).toBeNull();
    },
  );

  (envelopeReady ? it : it.todo)(
    "malformed JSON is discarded",
    () => {
      expect(storedParser("{not-json")).toBeNull();
    },
  );

  (envelopeReady ? it : it.todo)(
    "{v:1,...} is discarded",
    () => {
      expect(
        storedParser(JSON.stringify({ v: 1, tree: { t: "leaf", id: ID_A0 } })),
      ).toBeNull();
    },
  );

  (envelopeReady ? it : it.todo)(
    "bare {t:...} without version is discarded",
    () => {
      expect(
        storedParser(JSON.stringify({ t: "leaf", id: ID_A0 })),
      ).toBeNull();
    },
  );

  (envelopeReady ? it : it.todo)(
    "old index root without version is discarded",
    () => {
      expect(
        storedParser(JSON.stringify({ t: "leaf", i: 0 })),
      ).toBeNull();
    },
  );

  (envelopeReady ? it : it.todo)(
    "{v:2, tree} with full-id leaf is accepted",
    () => {
      const out = storedParser(
        JSON.stringify({ v: 2, tree: { t: "leaf", id: ID_A0 } }),
      );
      // Helper may return the tree node or a leaf runtime node.
      const node = out && out.t ? out : out?.tree ?? out;
      expect(node).toBeTruthy();
      if (node.t === "leaf") {
        expect(leafIdOf(node)).toBe(ID_A0);
      } else {
        expect(leafPanes(node)).toContain(ID_A0);
      }
    },
  );
});

// ─── T2: reconcileTree preserves surviving split ratios (base behavior — pin) ─

describe("T2 reconcileTree — surviving split ratios preserved", () => {
  it("pruning one leaf keeps the remaining root ratio untouched", () => {
    // v(0.3, A0 | h(0.7, A1 | B0)) → remove B0 → v(0.3, A0 | A1)
    const t0 = collisionTree();
    const { tree } = reconcileTree(t0, [ID_A0, ID_A1], null);
    expect(new Set(leafPanes(tree))).toEqual(new Set([ID_A0, ID_A1]));
    expect(tree.t).toBe("split");
    expect(tree.dir).toBe("v");
    expect(tree.ratio).toBe(0.42);
    expect(tree.a.t).toBe("leaf");
    expect(tree.a.pane).toBe(ID_A0);
    expect(tree.b.t).toBe("leaf");
    expect(tree.b.pane).toBe(ID_A1);
  });

  it("pruning the left child promotes the right split with its ratio intact", () => {
    // remove A0 → promote h(0.7, A1 | B0)
    const t0 = collisionTree();
    const { tree } = reconcileTree(t0, [ID_A1, ID_B0], null);
    expect(new Set(leafPanes(tree))).toEqual(new Set([ID_A1, ID_B0]));
    expect(tree.t).toBe("split");
    expect(tree.dir).toBe("h");
    expect(tree.ratio).toBe(0.618);
  });

  it("idempotent reconcile does not alter ratios when the live set matches", () => {
    const t0 = collisionTree();
    const a = reconcileTree(t0, [ID_A0, ID_A1, ID_B0], null).tree;
    const b = reconcileTree(a, [ID_A0, ID_A1, ID_B0], null).tree;
    expect(deepRatios(b)).toEqual(deepRatios(a));
    expect(leafPanes(b)).toEqual(leafPanes(a));
  });
});
