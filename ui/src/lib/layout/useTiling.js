// useTiling — the ONLY React integration surface for the BSP tiling engine. Wraps the pure
// tree.js (structure) + geometry.js (pixel rects) modules ported from the Agent Teams prod app,
// and exposes a headless hook the coordinator drops into Home.jsx to replace the static CSS grid.
//
// Headless by design: it computes absolutely-positioned {x,y,w,h} rects for each pane + the
// draggable seams between them, but owns NO DOM. The consumer renders panes at `rects[paneId]`
// and seam handles at `seams[i].rect`, wiring `onSeamPointerDown` to each handle's pointerdown.
//
// The seam drag mirrors prod's POINTER approach (setPointerCapture + mutate ratio + relayout),
// NOT html5 draggable — Tauri's webview intercepts native drag-and-drop.

import * as React from "react";
import {
  leaf,
  moveLeaf,
  buildDefaultTree,
  buildBalancedTree,
  reconcileTree,
  serializeTree,
  deserializeTree,
} from "./tree.js";
import {
  TILE_GAP,
  MIN_V,
  MIN_H,
  layoutRects,
  collectSplits,
  seamOf,
} from "./geometry.js";

/**
 * @typedef {"tile"|"columns"|"single"|"focus"} TilingMode
 * @typedef {{ x:number, y:number, w:number, h:number }} Rect
 * @typedef {{ left?:number, top?:number, width?:number, height?:number }} SeamRect
 * @typedef {{ t:"leaf", pane:string }|{ t:"split", dir:"v"|"h", ratio:number, a:object, b:object }} TreeNode
 * @typedef {{ id:string, node:TreeNode, dir:"v"|"h", box:Rect, rect:SeamRect }} Seam
 * @typedef {"left"|"right"|"up"|"down"} MoveDir
 */

/** @type {TilingMode[]} */
const MODES = ["tile", "columns", "single", "focus"];

// dir → [tree split dir, insert side]. left/right = vertical divider (side-by-side);
// up/down = horizontal divider (stacked). before → new pane is child `a` (left/top).
/** @type {Record<MoveDir, ["v"|"h", "before"|"after"]>} */
const DIR_MAP = {
  left: ["v", "before"],
  right: ["v", "after"],
  up: ["h", "before"],
  down: ["h", "after"],
};

// paneId → integer index. Panes are named `<wsId>-p<idx>` (mirrors deserializeTree), so the
// index is the trailing `-p<n>` suffix. Returns null (drop the leaf) when it doesn't parse.
function paneIdxOf(paneId) {
  const m = /-p(\d+)$/.exec(String(paneId));
  return m ? Number(m[1]) : null;
}

// localStorage is best-effort: a Tauri webview / private mode can throw on access. Never let a
// storage failure break layout — degrade to in-memory-only.
function lsGet(key) {
  try {
    return window.localStorage.getItem(key);
  } catch {
    return null;
  }
}
function lsSet(key, val) {
  try {
    window.localStorage.setItem(key, val);
  } catch {
    /* storage unavailable — layout still works, just doesn't persist */
  }
}

/**
 * Headless tiling controller for one workspace.
 *
 * @param {object} opts
 * @param {string[]} opts.paneIds       Authoritative live pane ids for the workspace.
 * @param {string|null} [opts.focusedId] Currently focused pane (anchor for single/focus + reconcile).
 * @param {React.RefObject<HTMLElement>} opts.containerRef Ref to the tiling host element (measured box).
 * @param {string} opts.wsId            Workspace id — namespaces persisted mode + tree.
 * @returns {{
 *   rects: Record<string, Rect>,
 *   mode: TilingMode,
 *   setMode: (m: TilingMode) => void,
 *   seams: Seam[],
 *   onSeamPointerDown: (seam: Seam, e: PointerEvent) => void,
 *   movePane: (srcId: string, targetId: string, dir: MoveDir) => void,
 *   zoomId: string|null,
 *   toggleZoom: (id: string) => void,
 *   tree: TreeNode|null,
 *   serialize: () => object|null,
 *   restore: (serial: string|object) => boolean,
 * }}
 */
export function useTiling({ paneIds: rawPaneIds, focusedId = null, containerRef, wsId, onDragFrame = null }) {
  const paneIds = React.useMemo(
    () => (Array.isArray(rawPaneIds) ? rawPaneIds : []),
    [rawPaneIds],
  );
  const paneKey = paneIds.join("|"); // stable dep for effects/memo (array identity churns)

  const modeKey = `hr:tiling:mode:${wsId}`;
  const treeKey = `hr:tiling:tree:${wsId}`;

  // Restore the persisted STRUCTURAL tree (tile/columns) for this ws, reconciled to live panes;
  // fall back to a fresh balanced tree. Structure only — single/focus are derived, not stored.
  const restoreStructTree = () => {
    const raw = lsGet(treeKey);
    if (raw) {
      try {
        const t = deserializeTree(JSON.parse(raw), wsId);
        if (t) return reconcileTree(t, paneIds, focusedId).tree;
      } catch {
        /* malformed persisted tree — rebuild fresh below */
      }
    }
    return buildBalancedTree(paneIds);
  };

  /** @type {[TilingMode, React.Dispatch<React.SetStateAction<TilingMode>>]} */
  const [mode, setModeState] = React.useState(() => {
    const saved = lsGet(modeKey);
    return MODES.includes(/** @type {TilingMode} */ (saved)) ? /** @type {TilingMode} */ (saved) : "tile";
  });

  // Latest-value refs so effects/handlers that must NOT re-subscribe on every prop change can
  // still read current values (standard React ref-mirror pattern).
  const paneIdsRef = React.useRef(paneIds);
  paneIdsRef.current = paneIds;
  const focusedIdRef = React.useRef(focusedId);
  focusedIdRef.current = focusedId;
  const modeRef = React.useRef(mode);
  modeRef.current = mode;

  // The authoritative, mutable STRUCTURAL tree (tile/columns). Mutated in place during a seam
  // drag (prod parity — zero alloc per pointermove), rebuilt/reconciled by the effects below.
  const structRef = React.useRef(/** @type {TreeNode|null|undefined} */ (undefined));
  if (structRef.current === undefined) {
    structRef.current = restoreStructTree(); // lazy init on first render → tree exists for first paint
  }

  // Measured container box via ResizeObserver (mirrors src/hooks/use-size.jsx). useLayoutEffect
  // so the first measurement lands before paint → rects are real on the first painted frame.
  // Keyed on paneKey TOO: the host is conditionally rendered (EmptyState at 0 panes), so on the
  // app's first mount containerRef.current is null and no observer attaches — the effect must
  // re-run when panes appear (host now mounted) or size stays null and every rect collapses to 0×0.
  const [size, setSize] = React.useState(/** @type {{width:number,height:number}|null} */ (null));
  React.useLayoutEffect(() => {
    const el = containerRef && containerRef.current;
    if (!el) return undefined;
    const rect = el.getBoundingClientRect();
    setSize({ width: rect.width, height: rect.height });
    const observer = new ResizeObserver(([entry]) => {
      const { width, height } = entry.contentRect;
      setSize({ width, height });
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, [containerRef, paneKey]);

  // Mutation → re-render pump. Rects/seams are DERIVED from structRef; bumping recomputes them
  // after an in-place mutation (seam drag, move) without cloning the tree on the hot path.
  const [version, bump] = React.useReducer((n) => n + 1, 0);

  const persistStruct = React.useCallback(() => {
    const ser = serializeTree(structRef.current, paneIdxOf);
    lsSet(treeKey, ser ? JSON.stringify(ser) : "");
  }, [treeKey]);

  // Reconcile the structural tree to the live pane set on add/remove (prune dead leaves, append
  // new panes) — preserving existing seam ratios. Skipped in single/focus (derived views). Runs
  // on pane-set / focus change; reconcileTree is idempotent so re-runs are safe.
  React.useEffect(() => {
    if (modeRef.current === "single" || modeRef.current === "focus") return;
    structRef.current =
      reconcileTree(structRef.current, paneIds, focusedId).tree || buildDefaultTree(paneIds);
    persistStruct();
    bump();
    // paneKey stands in for the paneIds array (identity churns each render); reconcile is idempotent.
  }, [paneKey, focusedId, persistStruct]);

  // Workspace switch: reload the persisted mode + structural tree for the new ws. Skipped on
  // mount (the lazy init above already built the tree for the first ws).
  const firstWs = React.useRef(true);
  React.useEffect(() => {
    if (firstWs.current) {
      firstWs.current = false;
      return;
    }
    structRef.current = restoreStructTree();
    const savedMode = lsGet(modeKey);
    if (MODES.includes(/** @type {TilingMode} */ (savedMode))) {
      setModeState(/** @type {TilingMode} */ (savedMode));
    }
    bump();
    // keyed on wsId only; restoreStructTree reads latest paneIds/focus from the render closure.
  }, [wsId]);

  // Mode switch. tile/columns REBUILD the structural tree (an explicit re-tile that resets seams);
  // single/focus leave the structural tree intact (they only change the derived render view, so
  // returning to tile/columns resumes the prior arrangement). Persisted per ws.
  const setMode = React.useCallback(
    (m) => {
      if (!MODES.includes(m)) return;
      setModeState(m);
      lsSet(modeKey, m);
      if (m === "tile") {
        structRef.current = buildBalancedTree(paneIdsRef.current);
        persistStruct();
      } else if (m === "columns") {
        structRef.current = buildDefaultTree(paneIdsRef.current);
        persistStruct();
      }
      bump();
    },
    [modeKey, persistStruct],
  );

  // ── Seam drag (pointer-based, prod parity) ──────────────────────────────────────────────────
  const dragRef = React.useRef(
    /** @type {{node:TreeNode, horiz:boolean, start:number, len:number, min:number, raf:number}|null} */ (null),
  );
  // Consumer-provided direct-DOM frame sink (prod relayout parity, main.js:886-935): during a
  // seam drag we mutate the ratio and hand fresh rects/seams to the consumer, which writes
  // `el.style.*` directly — ZERO React renders per pointermove; panes/xterm are never recreated.
  // One state bump lands on drag END. Without a sink, falls back to per-move bump().
  const onDragFrameRef = React.useRef(onDragFrame);
  onDragFrameRef.current = onDragFrame;

  const layoutSnapshot = React.useCallback(() => {
    const host = containerRef && containerRef.current;
    if (!host || !structRef.current) return null;
    const b = host.getBoundingClientRect();
    const box = { x: 0, y: 0, w: b.width, h: b.height };
    /** @type {Record<string, Rect>} */
    const rectsMap = {};
    for (const r of layoutRects(structRef.current, box)) {
      rectsMap[r.pane] = { x: r.x, y: r.y, w: r.w, h: r.h };
    }
    const seamList = collectSplits(structRef.current, box, []).map((rec, i) => ({
      id: `seam-${i}`,
      node: rec.node,
      dir: rec.dir,
      box: rec.box,
      rect: seamOf(rec),
    }));
    return { rects: rectsMap, seams: seamList };
  }, [containerRef]);

  const onSeamMove = React.useCallback(
    (e) => {
      const d = dragRef.current;
      if (!d) return;
      const host = containerRef && containerRef.current;
      if (!host || d.len <= 0) return;
      const hr = host.getBoundingClientRect();
      const pointer = d.horiz ? e.clientX - hr.left : e.clientY - hr.top;
      let r = (pointer - d.start) / d.len;
      const minR = d.min / d.len;
      r = Math.max(minR, Math.min(1 - minR, r)); // honor MIN_V / MIN_H floors on both sides
      if (!Number.isFinite(r)) return;
      d.node.ratio = r; // mutate the structural node in place
      if (onDragFrameRef.current) {
        // rAF-coalesce a burst of pointermoves into one direct-DOM frame (prod main.js:932).
        if (!d.raf) {
          d.raf = requestAnimationFrame(() => {
            d.raf = 0;
            const snap = layoutSnapshot();
            if (snap && onDragFrameRef.current) onDragFrameRef.current(snap.rects, snap.seams);
          });
        }
      } else {
        bump(); // no sink registered — legacy per-move re-render
      }
    },
    [containerRef, layoutSnapshot],
  );

  const endSeam = React.useCallback(
    (e) => {
      const d = dragRef.current;
      if (!d) return;
      if (d.raf) cancelAnimationFrame(d.raf);
      const target = e.currentTarget;
      target.removeEventListener("pointermove", onSeamMove);
      target.removeEventListener("pointerup", endSeam);
      target.removeEventListener("lostpointercapture", endSeam);
      try {
        target.releasePointerCapture(e.pointerId);
      } catch {
        /* capture already released */
      }
      dragRef.current = null;
      persistStruct(); // commit the new ratio to storage once, on drag end
      bump(); // single React sync for the whole drag
    },
    [onSeamMove, persistStruct],
  );

  const onSeamPointerDown = React.useCallback(
    (seam, e) => {
      if (!seam || !seam.node) return;
      if (e.preventDefault) e.preventDefault();
      if (e.stopPropagation) e.stopPropagation();
      const horiz = seam.dir === "v"; // a vertical divider slides horizontally
      const len = (horiz ? seam.box.w : seam.box.h) - TILE_GAP;
      if (len <= 0) return;
      dragRef.current = {
        node: seam.node,
        horiz,
        start: horiz ? seam.box.x : seam.box.y,
        len,
        min: horiz ? MIN_V : MIN_H,
        raf: 0,
      };
      const target = e.currentTarget;
      try {
        target.setPointerCapture(e.pointerId); // route move/up to the handle even over an xterm
      } catch {
        /* pointer capture unsupported — listeners below still fire on the element */
      }
      target.addEventListener("pointermove", onSeamMove);
      target.addEventListener("pointerup", endSeam);
      target.addEventListener("lostpointercapture", endSeam);
    },
    [onSeamMove, endSeam],
  );

  // ── Zoom (per-pane maximize) ────────────────────────────────────────────────────────────────
  // An explicit per-pane pin, NOT a MODE: single/focus anchor on selectedId and derive their view,
  // zoom names one pane. They coexist — zoom takes precedence while set (prod checks ws.zoom before
  // the tree at main.js:904) and restoring it drops straight back to the mode's view.
  //
  // Keyed by wsId so a zoom survives a workspace switch, and IN-MEMORY ONLY: prod omits zoom from
  // persistWorkspaces (main.js:4090), so it dies on reload. Deliberately NOT written to `hr:tiling:*`.
  /** @type {[Record<string,string>, React.Dispatch<React.SetStateAction<Record<string,string>>>]} */
  const [zoomByWs, setZoomByWs] = React.useState({});

  // DERIVED, not stored: a zoomed pane that left paneIds (closed, or moved to another ws) would make
  // `rects` a lone rect for a pane that no longer renders — i.e. a blank grid. Prod guards this at
  // relayout (main.js:902); deriving it means the dangling id can never reach a paint, which an
  // after-paint effect could not promise.
  const rawZoom = zoomByWs[wsId] || null;
  const zoomId = rawZoom && paneIds.includes(rawZoom) ? rawZoom : null;

  // ...then drop the dead entry, so a pane returning to this ws can't resurrect a stale zoom.
  React.useEffect(() => {
    if (!rawZoom || zoomId) return;
    setZoomByWs((prev) => {
      if (!prev[wsId]) return prev;
      const next = { ...prev };
      delete next[wsId];
      return next;
    });
  }, [rawZoom, zoomId, wsId]);

  const toggleZoom = React.useCallback(
    (id) => {
      if (!id || !paneIdsRef.current.includes(id)) return; // prod: maximizePane no-ops on an unknown pane
      setZoomByWs((prev) => {
        const next = { ...prev };
        if (next[wsId] === id) delete next[wsId]; // same pane → restore
        else next[wsId] = id;
        return next;
      });
    },
    [wsId],
  );

  // Intra-workspace reorder: prune src, re-insert it beside target on the given side.
  const movePane = React.useCallback(
    (srcId, targetId, dir) => {
      const [splitDir, where] = DIR_MAP[dir] || DIR_MAP.right;
      structRef.current = moveLeaf(structRef.current, srcId, targetId, splitDir, where);
      persistStruct();
      bump();
    },
    [persistStruct],
  );

  // Stable serialize/restore so layout survives reloads (index-keyed, ws-agnostic payload).
  const serialize = React.useCallback(() => serializeTree(structRef.current, paneIdxOf), []);
  const restore = React.useCallback(
    (serial) => {
      try {
        const parsed = typeof serial === "string" ? JSON.parse(serial) : serial;
        const t = deserializeTree(parsed, wsId);
        if (!t) return false;
        structRef.current = reconcileTree(t, paneIdsRef.current, focusedIdRef.current).tree;
        persistStruct();
        bump();
        return true;
      } catch {
        return false;
      }
    },
    [wsId, persistStruct],
  );

  // The tree actually laid out this render. single/focus → the focused pane full-box (others
  // absent from `rects` → the consumer hides them). tile/columns → the structural tree.
  const derivedFocus =
    focusedId && paneIds.includes(focusedId) ? focusedId : paneIds[0] || null;
  const renderTree =
    mode === "single" || mode === "focus"
      ? derivedFocus
        ? leaf(derivedFocus)
        : null
      : structRef.current || buildDefaultTree(paneIds);

  const { rects, seams } = React.useMemo(() => {
    const w = size ? size.width : 0;
    const h = size ? size.height : 0;
    const box = { x: 0, y: 0, w, h };
    // Zoom short-circuits the tree walk (prod parity, main.js:904): ONE full-host rect, every
    // sibling ABSENT from `rects`. The absence IS the hide mechanism — Home.jsx already maps a
    // missing rect to display:none + visible={false}, which detaches WebGL and skips the re-fit.
    // No seams under zoom: the dividers belong to splits that aren't on screen (main.js:943).
    if (zoomId) return { rects: { [zoomId]: { x: 0, y: 0, w, h } }, seams: [] };
    /** @type {Record<string, Rect>} */
    const rectsMap = {};
    for (const r of layoutRects(renderTree, box)) {
      rectsMap[r.pane] = { x: r.x, y: r.y, w: r.w, h: r.h };
    }
    /** @type {Seam[]} */
    const seamList = collectSplits(renderTree, box, []).map((rec, i) => ({
      id: `seam-${i}`,
      node: rec.node,
      dir: rec.dir,
      box: rec.box,
      rect: seamOf(rec),
    }));
    return { rects: rectsMap, seams: seamList };
    // version/size/mode/focus/paneKey capture every relayout input (renderTree is derived from them).
  }, [version, size, mode, focusedId, paneKey, zoomId]);

  return {
    rects,
    mode,
    setMode,
    seams,
    onSeamPointerDown,
    movePane,
    zoomId,
    toggleZoom,
    tree: renderTree,
    serialize,
    restore,
  };
}
