// Agent Teams — frontend. One xterm Terminal PER workspace (no reset/replay on
// switch); PTY size synced to xterm only when it actually changes (no SIGWINCH
// feedback loop). No bundler: ESM-vendored xterm + withGlobalTauri.
import { Terminal } from "/vendor/xterm.mjs";
import { FitAddon } from "/vendor/addon-fit.mjs";
// WebglAddon is LAZY-loaded (see attachWebgl): its ~124KB module is parsed only on the first
// terminal that attaches WebGL, not at launcher boot (the launcher opens zero terminals).
import { openWizard } from "./wizard.js";
import { trapModalFocus, releaseModalFocus } from "./focus-trap-core.js";
import * as ltree from "./layout-tree.js";
import { reconcilePlan, railMeta } from "./rail-core.js";
import { paneIdx, survivorIdxList } from "./pane-reopen.js";
import { expandHarnesses } from "./presets.js";
import { benchmarkRows, fmtDur as benchDur, fmtCost as benchCost } from "./bench-core.js";
import { allLivePanes } from "./bridge-panes.js";
import { initialChat, chatReduce, classifyInput, formatRoleBadge } from "./bridge-chat-core.js";
import { applyDelta, paneDue, queueSignature, POLL_TICK_MS } from "./poll-core.js";
import { isPaneSettled } from "./bridge-settle.js";
import { layoutGraph, isEmptyGraph } from "./graph-core.js";
import { CATEGORY_META } from "./lightning-core.js"; // category palette — SSOT shared with the renderer's orbs
import { svgIcon } from "./svg-icon-core.js";
import { diffLineClass, renderUnifiedDiff } from "./diff-view-core.js";
import { mcpEffectiveCol, displayColToWire, buildCreatePayload, buildUpdatePayload } from "./kanban-core.js";
import { BOARD_COLS, columnFor, boardRows, bucketByColumn, applyOrder, boardReorder } from "./board-core.js";
import { SKIN_KEY, isValidSkin, normalizeSkin, resolveBootSkin, skinAttrFor, skinStorageOp } from "./skin-core.js";
import { flywheelPhaseCopy, fwGateChips } from "./flywheel-gate-core.js";
import { trustedAddDecision, renderTrustedReposList } from "./settings-core.js";
import { stateBlindBadge } from "./state-blind-core.js";
import { dgRunTs, dgRelTime, dgDurationMs, dgFmtDur, dgVerdictUx, dgReviewData, dgIngestLine } from "./delegation-log-core.js";
import { TILE_GAP, MIN_V, MIN_H, layoutRects, collectSplits, seamOf } from "./layout-geometry.js";
import { externalSpawnCap, expandExternalPanes } from "./external-spawn-core.js";

function tauriInvoke() {
  const t = window.__TAURI__;
  return t && t.core && t.core.invoke;
}
function hasTauri() { return !!tauriInvoke(); }
function invoke(cmd, args) {
  const f = tauriInvoke();
  return f ? f(cmd, args) : Promise.reject(new Error("Tauri API unavailable"));
}

// Record the active workspace into the live registry so external tooling (GlikaAgents)
// can name "the workspace we're on". Best-effort, fire-and-forget; debounced by identity.
let _lastPersistedActiveWs;
function persistActiveWs() {
  if (typeof activeWs === "undefined") return;
  if (activeWs === _lastPersistedActiveWs) return;
  _lastPersistedActiveWs = activeWs;
  try { invoke("set_active_workspace", { id: activeWs || null }).catch(() => {}); } catch (_) {}
}

// svgIcon (inline Lucide <svg class="icon"><use href="#id"/></svg> builder) lives in
// ./svg-icon-core.js (imported above) so it can be DOM-tested in isolation.

const host = document.getElementById("terminal");
const sessions = {}; // paneId -> { term, fit, el, consumed, pendingWrites, polling, webgl, webglBlocked, lastRows, lastCols }
let activeId = null; // active pane id

// ---- workspaces: a frontend grouping of PTY panes (backend stays per-pane) ----
const workspaces = {}; // wsId -> { name, color, repo, harness, paneIds:[], counter, count, dormant }
let activeWs = null;
let wsSeq = 0;
// ---- Scheduler (Plan 05-02, D33): panes the backend QUEUED instead of spawning
// (over the max_concurrent cap). No PTY/xterm yet; they live here until the
// `workspace-admitted` event attaches them. Keyed by pane id → { id, wsId, harness,
// prompt, enqueuedAt, position }. These never appear in the "who needs you" queue.
// PERSISTED to localStorage (at_pending): a webview reload keeps the Rust backend (and
// its queue) ALIVE but wipes this JS map, so without persistence a queued pane would
// vanish from the Scheduler (row + "run now") and its admit event would land with no
// record. Persist + restore so the row survives the reload, and reconcilePending()
// adopts any pane the backend admitted while we were gone. (A FULL app restart kills
// the backend queue; leftover entries can't re-admit — see reconcilePending note.) ----
const pending = {};
// Panes the frontend KNOWS are dead — a send_input rejected with "workspace is no
// longer alive" / "no such workspace" (lib.rs send_input). The backend may still list
// a dead pane as needs_human (it lingers in `sups`), so we track death frontend-side to
// light the error dot (D30) and stop re-targeting the corpse. DEAD_RE is the single
// source of truth for that death-error match — shared by Speak&Send, broadcast, and
// dispatch so the three callers can't drift.
const deadPanes = new Set();
const DEAD_RE = /no longer alive|no such workspace|not alive/i;
// Dead-pane typing hint: term.onData short-circuits input to a known-dead pane (typing
// into a corpse just errors backend-side), and this one-shot set makes sure the operator
// is TOLD once per pane instead of typing into a silent black hole. Cleared wherever the
// death flag itself clears (reopen reuses the id / close).
const _deadHintShown = new Set();
function hintDeadPane(id) {
  if (_deadHintShown.has(id)) return;
  _deadHintShown.add(id);
  try { showToast(`${id} is dead — typing is ignored (close or reopen the pane)`); } catch (_) {}
}
// Single source of truth for every localStorage key used ≥3 times (the 1-2-use keys
// stay inline next to their feature). Prevents a typo'd literal silently forking state.
const LS_KEYS = {
  bridgeRun: "at_bridge_run",
  bridgePrd: "at_bridge_prd",
  bridgeGoalDraft: "at_bridge_goal_draft",
  bridgeUnify: "at_bridge_unify",
  bridgeDockOpen: "at_bridge_dock_open",
  orchRunner: "at_orch_runner",
  orchLoop: "at_orch_loop",
  lastCount: "at_last_count",
  lastFolder: "at_last_folder",
  lastHarness: "at_last_harness",
  fwHarness: "at_fw_harness",
  maxConcurrent: "at_max_concurrent2",
};
// last list_queue poll, cached so we can re-render the rail synchronously (e.g. to
// repaint a dot the instant a send fails, without waiting for the next 1s tick).
let _lastQueue = [];
// last list_workspaces poll, cached alongside _lastQueue so the alt-view board (09-03
// Tier A) can re-render synchronously on toggle-open from the same data the rail uses
// (queue rows ∪ workspace extras) without an extra backend round-trip.
let _lastAll = [];
// Identity-only ramp (muted, status-free): never collides with status colors
// (--success green / --need amber / --danger red), so a workspace's identity dot
// can't be mistaken for a live status signal. Status dots use --need/--muted in renderWorkspaces.
const WS_PALETTE = ["#E0E0E0", "#C8C8C8", "#A8A8A8", "#909090", "#787878", "#646464"];

// xterm surface sourced from the CSS design tokens so the terminal stays in sync
// with --bg/--fg/--font-mono (no hardcoded-hex drift). Read once at module load
// (styles.css is in <head>, so computed values are available).
const _cssVar = (name, fallback) => {
  try { return getComputedStyle(document.documentElement).getPropertyValue(name).trim() || fallback; }
  catch (_) { return fallback; }
};
// xterm theme is no longer a frozen const — a skin change rewrites :root tokens, so the
// theme must be recomputed FRESH from the current computed --bg/--fg on every (re)theme.
// computeTermTheme() is the single source of truth: used at terminal-creation time (so a
// new pane honors the active skin) AND on a live skin switch (re-themes every session).
// Always returns a COMPLETE theme (background + foreground guaranteed) so xterm can never
// be handed a half-object and blank the pane.
function computeTermTheme() {
  return {
    background: _cssVar("--bg", "#000000"),
    foreground: _cssVar("--fg", "#E8E8E8"),
  };
}

// ---- Themeable skins (data-skin token sets) -------------------------------------------
// The whole look is driven by :root CSS custom properties; styles.css ships per-skin
// overrides under html[data-skin="…"]. Switching skins = swapping that attribute. Nothing
// is the default and is expressed as the BARE :root (no data-skin attribute at all).
// The pure decisions (allowlist, boot precedence, delete-vs-set, persist-vs-remove) live
// in ./skin-core.js (imported above); only the DOM/localStorage/xterm side effects stay here.
function currentSkin() {
  const d = document.documentElement.dataset.skin;
  return (d && isValidSkin(d)) ? d : "nothing";
}
// Apply WITHOUT persisting — used at boot. nothing → remove the attribute (bare :root).
function applySkinAttr(name) {
  const attr = skinAttrFor(name);
  if (attr === null) delete document.documentElement.dataset.skin;
  else document.documentElement.dataset.skin = attr;
}
// Boot apply: resolve + set the attribute BEFORE the first render so the initial paint
// (and the first terminal's theme) already reflects the active skin.
applySkinAttr(resolveBootSkin(location.search, (() => {
  try { return localStorage.getItem(SKIN_KEY); } catch (_) { return null; }
})()));

const TERM_THEME = computeTermTheme();
// Terminal scrollback uses a SYSTEM monospace (not the Space Mono webfont): a font-display:swap
// webfont swaps in AFTER first paint, forcing xterm to remeasure + repaint every pane (an N-pane
// reflow storm at boot). System fonts resolve instantly → zero FOUT. The Nothing-OS look lives in
// the UI chrome (--font-mono, untouched), not the agent scrollback. Override via --font-term.
const TERM_FONT = _cssVar("--font-term", 'ui-monospace, "SF Mono", Menlo, Monaco, monospace');

// Live skin switch: swap the data-skin attribute, persist, then RE-THEME every live
// terminal. TERM_THEME was computed once at boot from the OLD tokens, so after a skin
// change it's stale — each terminal must be handed a freshly-computed theme and forced to
// repaint (DOM renderer via term.refresh; WebGL renderer via its own forced repaint), then
// refit so nothing clips against the new metrics.
function setSkin(name) {
  const skin = normalizeSkin(name);
  applySkinAttr(skin);
  try {
    const op = skinStorageOp(skin);
    if (op.action === "remove") localStorage.removeItem(SKIN_KEY);
    else localStorage.setItem(SKIN_KEY, op.value);
  } catch (_) {}
  // The :root tokens have changed — recompute ONCE and apply to every session's terminal.
  const theme = computeTermTheme();
  for (const id of Object.keys(sessions)) {
    const s = sessions[id];
    if (!s || !s.term) continue;
    try {
      s.term.options.theme = theme;
      // DOM renderer: force a full repaint of every visible row with the new palette.
      s.term.refresh(0, Math.max(0, s.term.rows - 1));
      // WebGL renderer: the addon caches the theme on its GL atlas; clearTextureAtlas()
      // (when present) invalidates it so the next frame re-rasterizes glyphs with the new
      // colors. Fall back to a plain refresh otherwise — the refresh above already ran.
      if (s.webgl && typeof s.webgl.clearTextureAtlas === "function") {
        try { s.webgl.clearTextureAtlas(); } catch (_) {}
      }
    } catch (_) { /* one stubborn pane must never abort the rest of the re-theme */ }
  }
  // Token swaps can change font metrics / chrome sizing — refit + relayout so no pane clips.
  try { relayout(); } catch (_) {}
  for (const id of Object.keys(sessions)) {
    try { fitSession(sessions[id], id); } catch (_) {}
  }
  return skin;
}

function newWsId() { return "ws" + (Date.now() % 100000) + "x" + wsSeq++; }
// reverse lookup: which workspace owns a pane id
function paneOwner(paneId) {
  for (const wsId of Object.keys(workspaces)) {
    if (workspaces[wsId].paneIds.includes(paneId)) return wsId;
  }
  return null;
}

// ---- Per-pane DISPLAY label (rename, 2026-06-18) ----------------------------------------
// Display-only. NEVER the machine pane/branch id (which stays the git-safe `ws…-pN` used for
// worktrees). Session-scoped + best-effort localStorage; absent → show the id. Fully isolated
// from the PTY/session lifecycle, so a rename interaction can never wedge pane creation.
const PANE_LABELS_KEY = "at_pane_labels";
let paneLabels = (() => { try { return JSON.parse(localStorage.getItem(PANE_LABELS_KEY) || "{}") || {}; } catch (_) { return {}; } })();
function paneLabel(id) { return (paneLabels && typeof paneLabels[id] === "string" && paneLabels[id]) ? paneLabels[id] : id; }
function setPaneLabel(id, label) {
  const v = (label || "").trim();
  if (v && v !== id) paneLabels[id] = v; else delete paneLabels[id];
  try { localStorage.setItem(PANE_LABELS_KEY, JSON.stringify(paneLabels)); } catch (_) {}
}
// Inline rename: dblclick the pane-head id → swap for an input; Enter/blur commits, Esc cancels.
function beginPaneRename(id, phId, head) {
  if (head.querySelector("input.ph-rename")) return;
  const input = document.createElement("input");
  input.className = "ph-rename";
  input.value = paneLabel(id);
  input.spellcheck = false;
  head.classList.add("renaming");
  phId.textContent = "";
  phId.appendChild(input);
  input.focus();
  input.select();
  let done = false;
  const finish = (commit) => {
    if (done) return;
    done = true;
    head.classList.remove("renaming");
    if (commit) setPaneLabel(id, input.value);
    phId.textContent = paneLabel(id);
    phId.title = "Double-click to rename · " + id;
  };
  input.addEventListener("keydown", (e) => {
    e.stopPropagation();
    if (e.key === "Enter") { e.preventDefault(); finish(true); }
    else if (e.key === "Escape") { e.preventDefault(); finish(false); }
  });
  input.addEventListener("blur", () => finish(true));
  input.addEventListener("click", (e) => e.stopPropagation());
  input.addEventListener("mousedown", (e) => e.stopPropagation());
}

// Pane-head kebab menu (display-only popover; every action routes to an EXISTING handler so
// it can never wedge a pane; the ⤢/✕ buttons remain the source of truth). Mounted on
// document.body (fixed layer) so grid overflow can't clip it; rebuilt each open.
// a11y: shared roving focus for the role="menu" popovers (pane kebab + workspace
// context menu) — focus the first menuitem on open; ArrowDown/ArrowUp cycle (wrapping),
// Home/End jump. Escape stays with each menu's own capture handler.
function menuFocusFirst(menu) {
  const it = menu.querySelector('[role="menuitem"]:not([disabled])');
  if (it) { try { it.focus(); } catch (_) {} }
}
function menuArrowNav(e, menu) {
  if (e.key !== "ArrowDown" && e.key !== "ArrowUp" && e.key !== "Home" && e.key !== "End") return;
  const items = [...menu.querySelectorAll('[role="menuitem"]:not([disabled])')];
  if (!items.length) return;
  e.preventDefault();
  e.stopPropagation();
  let i = items.indexOf(document.activeElement);
  if (e.key === "Home") i = 0;
  else if (e.key === "End") i = items.length - 1;
  else if (e.key === "ArrowDown") i = (i + 1 + items.length) % items.length; // unfocused (-1) → first
  else i = i <= 0 ? items.length - 1 : i - 1;                                // unfocused (-1) → last
  try { items[i].focus(); } catch (_) {}
}

let paneMenuEl = null;
function closePaneMenu() {
  if (!paneMenuEl) return;
  try { paneMenuEl.remove(); } catch (_) {}
  paneMenuEl = null;
  document.removeEventListener("mousedown", onPaneMenuOutside, true);
  document.removeEventListener("keydown", onPaneMenuKey, true);
  window.removeEventListener("scroll", closePaneMenu, true);
  window.removeEventListener("resize", closePaneMenu, true);
}
function onPaneMenuOutside(e) { if (paneMenuEl && !paneMenuEl.contains(e.target)) closePaneMenu(); }
function onPaneMenuKey(e) {
  if (e.key === "Escape") { e.stopPropagation(); closePaneMenu(); }
  else if (paneMenuEl) menuArrowNav(e, paneMenuEl);
}
function openPaneMenu(id, anchorEl, phId, head) {
  if (paneMenuEl) { closePaneMenu(); return; }
  const menu = document.createElement("div");
  menu.className = "ph-menu";
  menu.setAttribute("role", "menu");
  const addItem = (label, glyph, onClick, opts) => {
    const it = document.createElement("button");
    it.className = "ph-menu-item" + (opts && opts.danger ? " danger" : "") + (opts && opts.disabled ? " disabled" : "");
    it.setAttribute("role", "menuitem");
    if (opts && opts.disabled) it.disabled = true;
    const g = document.createElement("span"); g.className = "ph-menu-ico"; g.textContent = glyph;
    const t = document.createElement("span"); t.className = "ph-menu-label"; t.textContent = label;
    it.append(g, t);
    it.onclick = (opts && opts.disabled) ? (e) => e.stopPropagation()
      : (e) => { e.stopPropagation(); closePaneMenu(); onClick(); };
    menu.appendChild(it);
  };
  const sep = () => { const s = document.createElement("div"); s.className = "ph-menu-sep"; menu.appendChild(s); };
  addItem("Rename", "✎", () => beginPaneRename(id, phId, head));
  addItem("Maximize", "⤢", () => maximizePane(id));
  addItem("Split right", "⬌", () => { setActivePane(id); splitPane("v"); });
  addItem("Split down", "⬍", () => { setActivePane(id); splitPane("h"); });
  addItem("View diff", "⌗", () => { setActivePane(id); openDiff(); });
  addItem("Copy id", "⧉", () => {
    try { if (navigator.clipboard && navigator.clipboard.writeText) navigator.clipboard.writeText(id).then(() => { try { showToast("Copied " + id); } catch (_) {} }).catch(() => {}); } catch (_) {}
  });
  {
    // "Copy branch" — only when this pane resolved a git branch (chip visible). The branch
    // is the genuinely-useful head value (the id is the internal worktree root); mirror the
    // "Copy id" clipboard path. Reads the raw label off the .ph-branch chip in this head.
    const bEl = head.querySelector(".ph-branch");
    const branch = bEl && bEl.style.display !== "none" ? (bEl.dataset.branch || "") : "";
    if (branch) addItem("Copy branch", "⎇", () => {
      try { if (navigator.clipboard && navigator.clipboard.writeText) navigator.clipboard.writeText(branch).then(() => { try { showToast("Copied " + branch); } catch (_) {} }).catch(() => {}); } catch (_) {}
    });
  }
  {
    // "Move to workspace" — migrate THIS pane to another live workspace (#2). One item per target.
    const targets = Object.keys(workspaces).filter((w) => w !== paneOwner(id) && !workspaces[w].dormant);
    if (targets.length) {
      sep();
      const lab = document.createElement("div"); lab.className = "ph-menu-collabel"; lab.textContent = "Move to workspace";
      menu.appendChild(lab);
      for (const w of targets) addItem(workspaces[w].name, "⇄", () => movePaneToWorkspace(id, w));
    }
  }
  sep();
  addItem("Open in editor", "↗", () => {}, { disabled: true });
  addItem("Generate handoff", "⎘", () => {}, { disabled: true });
  sep();
  addItem("Close", "✕", () => closeWorkspace(id), { danger: true });
  document.body.appendChild(menu);
  paneMenuEl = menu;
  const r = anchorEl.getBoundingClientRect();
  const mw = menu.offsetWidth || 200, mh = menu.offsetHeight || 180;
  let left = Math.min(r.right - mw, window.innerWidth - mw - 8);
  if (left < 8) left = 8;
  let top = r.bottom + 4;
  if (top + mh > window.innerHeight - 8) top = Math.max(8, r.top - mh - 4);
  menu.style.left = left + "px";
  menu.style.top = top + "px";
  document.addEventListener("mousedown", onPaneMenuOutside, true);
  document.addEventListener("keydown", onPaneMenuKey, true);
  window.addEventListener("scroll", closePaneMenu, true);
  window.addEventListener("resize", closePaneMenu, true);
  menuFocusFirst(menu); // a11y: keyboard users land on the first item (Arrows cycle)
}

// ── pane drag-to-reorder (#6) ──────────────────────────────────────────────
// Grab a pane header and drop it on another pane in the SAME workspace to move it
// there. Pointer-based, NOT HTML5 `draggable` — Tauri's webview intercepts native
// drag-drop (see the tauri://drag-* file-drop handler), so element DnD is unreliable;
// a mouse-tracked drag sidesteps that entirely. Cross-workspace combine isn't supported.
let paneDrag = null;       // { id, sx, sy, active }
let dropZoneEl = null;     // the live drop-zone overlay element
function paneIdAtPoint(x, y) {
  const el = document.elementFromPoint(x, y);
  const paneEl = el && el.closest && el.closest(".term-pane");
  if (!paneEl) return null;
  for (const pid of Object.keys(sessions)) if (sessions[pid].el === paneEl) return pid;
  return null;
}
// which edge-zone of the target pane the cursor is over (left/right/top/bottom thirds → split
// in that direction; center → no split).
function paneZoneAt(targetId, x, y) {
  const el = sessions[targetId] && sessions[targetId].el; if (!el) return null;
  const r = el.getBoundingClientRect();
  const fx = (x - r.left) / r.width, fy = (y - r.top) / r.height;
  const edge = 0.34;
  if (fx < edge) return "left";
  if (fx > 1 - edge) return "right";
  if (fy < edge) return "top";
  if (fy > 1 - edge) return "bottom";
  return "center";
}
function hideDropZone() { if (dropZoneEl) { dropZoneEl.remove(); dropZoneEl = null; } }
function showDropZone(targetId, zone) {
  hideDropZone();
  const el = sessions[targetId] && sessions[targetId].el;
  if (!el || !zone || zone === "center") return;
  let x = el.offsetLeft, y = el.offsetTop, w = el.offsetWidth, h = el.offsetHeight;
  if (zone === "left") w = w / 2;
  else if (zone === "right") { x += w / 2; w = w / 2; }
  else if (zone === "top") h = h / 2;
  else if (zone === "bottom") { y += h / 2; h = h / 2; }
  const z = document.createElement("div");
  z.className = "drop-zone";
  z.style.left = x + "px"; z.style.top = y + "px"; z.style.width = w + "px"; z.style.height = h + "px";
  host.appendChild(z); dropZoneEl = z;
}
function onPaneDragMove(e) {
  if (!paneDrag) return;
  if (!paneDrag.active) {
    if (Math.abs(e.clientX - paneDrag.sx) + Math.abs(e.clientY - paneDrag.sy) < 6) return; // click, not drag
    paneDrag.active = true;
    document.body.classList.add("pane-dragging");
    const s = sessions[paneDrag.id]; if (s) s.el.classList.add("pane-drag-src");
  }
  const target = paneIdAtPoint(e.clientX, e.clientY);
  if (target && target !== paneDrag.id && paneOwner(target) === paneOwner(paneDrag.id)) {
    showDropZone(target, paneZoneAt(target, e.clientX, e.clientY));
  } else hideDropZone();
}
function onPaneDragUp(e) {
  document.removeEventListener("mousemove", onPaneDragMove, true);
  document.removeEventListener("mouseup", onPaneDragUp, true);
  const drag = paneDrag; paneDrag = null;
  document.body.classList.remove("pane-dragging");
  hideDropZone();
  if (drag) { const s = sessions[drag.id]; if (s) s.el.classList.remove("pane-drag-src"); }
  if (!drag || !drag.active) return;
  const ws = workspaces[paneOwner(drag.id)];
  if (!ws || paneOwner(drag.id) !== activeWs) return;
  const live = ws.paneIds.filter((id) => sessions[id]);
  const target = paneIdAtPoint(e.clientX, e.clientY);
  if (target && target !== drag.id && sessions[target] && paneOwner(target) === activeWs) {
    const zone = paneZoneAt(target, e.clientX, e.clientY);
    if (zone && zone !== "center") {
      const dir = (zone === "left" || zone === "right") ? "v" : "h";
      const where = (zone === "left" || zone === "top") ? "before" : "after";
      ws.layout = ltree.moveLeaf(ws.layout, drag.id, target, dir, where);
      ws.layout = ltree.reconcileTree(ws.layout, live, activeId).tree;
      persistLayout(ws); relayout();
    }
  } else if (!target && live.length > 1) {
    // dropped on empty host area → detach into its own root sibling (split the root, v).
    const others = ltree.removeLeaf(ws.layout, drag.id);
    const firstLeaf = ltree.leafPanes(others)[0];
    if (firstLeaf) { ws.layout = ltree.splitLeaf(others, firstLeaf, "v", drag.id, "before"); persistLayout(ws); relayout(); }
  }
}
function startPaneDrag(id, e) {
  if (e.button !== 0) return;               // left button only
  if (e.target.closest(".ph-btn")) return;  // header buttons keep their own clicks
  if (e.target.closest("input")) return;    // don't hijack the inline rename input
  paneDrag = { id, sx: e.clientX, sy: e.clientY, active: false };
  document.addEventListener("mousemove", onPaneDragMove, true);
  document.addEventListener("mouseup", onPaneDragUp, true);
}

// Per-harness Shift+Enter newline sequence — the bytes that insert a newline WITHOUT
// submitting the prompt. Agent TUIs (Claude Code, codex, cursor, opencode, …) accept ESC+CR
// (= Option/Alt+Enter) as "newline, keep editing"; a raw shell uses a backslash continuation
// (\\ + Enter). Keyed by harness so each pane gets the right one — add/adjust an entry here if
// a harness needs a different sequence.
// The reliable cross-TUI newline is Ctrl+J (LF, byte 0x0a): plain Enter sends CR (\r) → submit,
// Ctrl+J sends LF (\n) → insert newline. Confirmed for codex / opencode / cursor / gemini CLIs
// (all document Ctrl+J as the newline key). Claude Code instead uses backslash-continuation
// (\\ + Enter), VERIFIED working. A raw shell submits on Ctrl+J, so shells use \\ + CR.
const HARNESS_NEWLINE = {
  claude: "\\\r",       // backslash-continuation — verified in Claude Code
  codex: "\n",          // Ctrl+J (LF)
  opencode: "\n",       // Ctrl+J
  cursor: "\n",         // Ctrl+J
  gemini: "\n",         // Ctrl+J
  commandcode: "\n",    // Ctrl+J (assumed — adjust if it differs)
  pi: "\n",          // default TUI newline (UNVERIFIED — Ink-based; splits like the other non-bash TUIs, settle=200ms)
  bash: "\\\r", sh: "\\\r", zsh: "\\\r", // shell line continuation (Ctrl+J would submit)
};
const DEFAULT_NEWLINE = "\n"; // Ctrl+J — the most common TUI newline
// Pane harness/model keyed BY ID (recorded at spawn) — id-keyed so it survives a cross-workspace
// migrate (the per-idx ws.harnesses arrays would mis-key after a move). Falls back to the idx
// arrays for any pane spawned before this map existed.
const paneMetaById = {};
function paneInfo(id) {
  if (paneMetaById[id]) return paneMetaById[id];
  const ws = workspaces[paneOwner(id)];
  const m = /-p(\d+)$/.exec(id);
  const idx = m ? Number(m[1]) : -1;
  const harness = (ws && idx >= 0 && (ws.harnesses || [])[idx]) || (ws && ws.harness) || "";
  const model = (ws && idx >= 0 && (ws.models || [])[idx]) || "";
  return { harness, model };
}
function harnessOf(id) { return paneInfo(id).harness; }
function newlineSeqFor(id) { return HARNESS_NEWLINE[harnessOf(id)] || DEFAULT_NEWLINE; }

function ensureSession(id) {
  if (sessions[id]) return sessions[id];
  const el = document.createElement("div");
  el.className = "term-pane";
  el.style.display = "none";
  const head = document.createElement("div");
  head.className = "pane-head";
  head.dataset.state = "idle"; // Nothing-OS status tick — seeded idle; pollQueue drives live state
  head.onclick = () => setActivePane(id); // click header → select this tile (stay in grid)
  head.addEventListener("mousedown", (e) => startPaneDrag(id, e)); // grab header → drag-to-reorder (#6)
  const phId = document.createElement("span");
  phId.className = "ph-id";
  phId.textContent = paneLabel(id);
  phId.title = "Double-click to rename · " + id;
  phId.addEventListener("dblclick", (e) => { e.stopPropagation(); beginPaneRename(id, phId, head); });
  // model-at-spawn: show WHAT this pane runs (harness · model) beside the id — the
  // operator had no way to see a pane's model anywhere. idx-keyed like spawnPane.
  const phMeta = document.createElement("span");
  phMeta.className = "ph-meta";
  // state-blind badge (#D5): codex/commandcode/opencode/pi emit no state signal, so this
  // pane's "Working" tick is unreported — not live. Flag it so the operator isn't misled.
  const phBlind = document.createElement("span");
  phBlind.className = "ph-blind";
  phBlind.style.display = "none";
  {
    const { harness, model } = paneInfo(id); // id-keyed (survives a workspace migrate)
    phMeta.textContent = harness ? (model ? `${harness} · ${model}` : harness) : "";
    phMeta.title = model ? `${harness} running ${model}` : (harness ? `${harness} on its account-default model` : "");
    const blind = stateBlindBadge(harness);
    if (blind) {
      phBlind.textContent = blind.label;
      phBlind.title = blind.title;
      phBlind.setAttribute("aria-label", blind.title); // a11y: the short chip text alone is cryptic
      phBlind.style.display = "";
    }
  }
  // git branch chip (#3): read-only `pane_branch` (git rev-parse --abbrev-ref HEAD on the
  // pane's worktree). Display-only — NEVER the machine ws…-pN id. Hidden when the pane has
  // no worktree (ran in folder) or the lookup errs. One delayed retry covers the restart-
  // attach race where the worktree meta isn't registered yet when this head is first built.
  const phBranch = document.createElement("span");
  phBranch.className = "ph-branch";
  phBranch.style.display = "none";
  const populateBranch = (attempt) => {
    // bail if this pane was torn down (closeWorkspace deletes sessions[id]) — the deferred
    // retry must not invoke against a removed worktree or write to a detached node.
    if (!hasTauri() || !sessions[id]) return;
    invoke("pane_branch", { id }).then((b) => {
      const name = (b || "").trim();
      if (!name) return;
      const label = name === "HEAD" ? "detached" : name;
      phBranch.dataset.branch = label;          // raw label for the kebab "Copy branch" item
      phBranch.textContent = "⎇ " + label;
      phBranch.title = "git branch · " + label;
      phBranch.style.display = "";
    }).catch(() => {
      // no worktree / not a repo yet → leave the chip hidden; retry once for the
      // restart-attach race, then give up silently (a missing chip is graceful).
      if (attempt < 1) setTimeout(() => populateBranch(attempt + 1), 1500);
    });
  };
  const phMax = document.createElement("button");
  phMax.className = "ph-btn ph-max";
  phMax.textContent = "⤢";
  phMax.title = "Maximize / restore";
  phMax.setAttribute("aria-label", "Maximize or restore pane"); // a11y: glyph-only button
  phMax.onclick = (e) => { e.stopPropagation(); maximizePane(id); };
  const phClose = document.createElement("button");
  phClose.className = "ph-btn ph-close";
  phClose.textContent = "✕";
  phClose.title = "Close";
  phClose.setAttribute("aria-label", "Close pane"); // a11y: glyph-only button
  phClose.onclick = (e) => { e.stopPropagation(); closeWorkspace(id); };
  const phMenu = document.createElement("button");
  phMenu.className = "ph-btn ph-kebab";
  phMenu.textContent = "⋯";
  phMenu.title = "More…";
  phMenu.setAttribute("aria-label", "More pane actions"); // a11y: glyph-only button
  phMenu.setAttribute("aria-haspopup", "true");
  phMenu.onclick = (e) => { e.stopPropagation(); openPaneMenu(id, phMenu, phId, head); };
  head.append(phId, phMeta, phBlind, phBranch, phMenu, phMax, phClose);
  const mount = document.createElement("div");
  mount.className = "pane-term";
  // click anywhere in the tile body → select it as the active pane (the active
  // outline follows the click; xterm still gets the event for its own focus).
  mount.addEventListener("mousedown", () => setActivePane(id));
  el.append(head, mount);
  host.appendChild(el);
  const term = new Terminal({
    fontSize: 13,
    fontFamily: TERM_FONT,
    cursorBlink: true,
    scrollback: 5000,
    // Fresh per-creation read (not the stale boot-time TERM_THEME const): a pane created
    // AFTER a skin switch must honor the active skin's --bg/--fg.
    theme: computeTermTheme(),
  });
  let fit = null;
  try { fit = new FitAddon(); term.loadAddon(fit); } catch (_) { fit = null; }
  term.open(mount);
  // Shift+Enter → insert a NEWLINE, don't submit. In a raw PTY, Enter and Shift+Enter both
  // emit a bare \r, so the agent CLI can't tell them apart and treats both as submit. We
  // intercept the keydown and instead send ESC+CR (= Alt/Option+Enter) — the sequence the
  // harness TUIs (Claude Code's documented Option+Enter, etc.) map to "newline, no submit".
  // Returning false suppresses xterm's default \r so the prompt isn't sent.
  // xterm 6's API is attachCustomKeyEventHandler (NOT attachCustomKeyHandler — that earlier
  // typo threw a TypeError here, aborting ensureSession before sessions[id]=s, which left the
  // backend pane orphaned → the "phantom" queue row). typeof-guard + try/catch so a key-handler
  // problem can NEVER abort pane registration. Returning false suppresses xterm's default \r.
  try {
    if (typeof term.attachCustomKeyEventHandler === "function") {
      term.attachCustomKeyEventHandler((e) => {
        if (e.key === "Enter" && e.shiftKey && !e.metaKey && !e.ctrlKey && !e.altKey) {
          // Send the per-harness newline ONCE (on keydown). Return false on EVERY event type
          // (keydown/keypress/keyup) to fully suppress this key — otherwise the Enter *keypress*
          // still reaches xterm and emits the submitting \r, which cancelled the newline (the
          // bug behind "Shift+Enter still doesn't work"). ESC+CR for agents, \\+CR for claude/shells.
          if (e.type === "keydown" && hasTauri()) {
            if (broadcast) { for (const pid of broadcastTargets()) invoke("send_input", { id: pid, data: newlineSeqFor(pid) }).catch(() => {}); }
            else invoke("send_input", { id, data: newlineSeqFor(id) }).catch((e) => { if (DEAD_RE.test(String(e))) noteDeadPanes([id]); });
          }
          return false;
        }
        // ⌘G maximize/restore — handle HERE so WKWebView "Find Next" (native) can't steal it
        // when the xterm helper <textarea> is focused. Use maximizePane(this id) so we never
        // depend on the activeId guard that toggleGrid() used to hit on a silent no-op.
        // stopPropagation: xterm only skips its own input when we return false — the DOM
        // event still bubbles, and the document keydown handler would toggleGrid() again
        // (double-toggle → no visible change). Stop bubble so only this path runs.
        if (e.type === "keydown" && e.metaKey && !e.altKey && !e.ctrlKey && !e.shiftKey
            && (e.key === "g" || e.key === "G")) {
          e.preventDefault();
          e.stopPropagation();
          maximizePane(id);
          return false;
        }
        return true;
      });
    }
  } catch (_) { /* Shift+Enter newline degrades gracefully; the pane still registers */ }
  term.onData((d) => {
    if (!hasTauri()) return;
    // KNOWN-dead pane: don't type into the void — send_input would just reject backend-side
    // and the keystrokes vanish silently. Skip the send and hint ONCE per pane (D30's red
    // dot already marks the corpse; this covers the operator who types before looking).
    if (deadPanes.has(id)) { hintDeadPane(id); return; }
    // broadcast mode: one keystroke → every pane in the active workspace
    // (tmux synchronize-panes / iTerm2 broadcast). Otherwise just this pane.
    // Fan out KEYBOARD input only: xterm emits terminal REPLY traffic through this same
    // channel — SGR mouse reports (\x1b[<…M/m), focus in/out (\x1b[I / \x1b[O), and OSC
    // query replies (\x1b]11;rgb:… background-color probe) — and broadcasting those types
    // them as garbage into every other pane's input line (live-fired on a commandcode
    // pane: "[<35;28;24M…]11;rgb:0a0a/…"). Replies belong ONLY to the pane whose app
    // asked. Arrow/function keys (\x1b[A, \x1bOP, …) don't match and still fan out.
    if (broadcast && /^(?:\x1b\[<|\x1b\[[IO]$|\x1b\])/.test(d)) {
      // reply traffic → the focused pane only. Catch a dead-pane reject so a keystroke to a
      // just-died PTY marks it dead (D30) now instead of surfacing as an unhandled rejection
      // and lagging until the next poll (mirrors the broadcast branch below).
      invoke("send_input", { id, data: d }).catch((e) => { if (DEAD_RE.test(String(e))) noteDeadPanes([id]); });
      return;
    }
    if (broadcast) {
      // a pane that died since the last keystroke rejects → mark it dead (D30) and
      // surface ONE coalesced "N dead, skipped" toast; broadcastTargets drops it next.
      for (const pid of broadcastTargets()) {
        invoke("send_input", { id: pid, data: d }).catch((e) => { if (DEAD_RE.test(String(e))) noteDeadPanes([pid]); });
      }
    } else {
      // Single-pane keystroke. Catch a dead-pane reject → mark dead (D30) immediately rather
      // than lose the keystroke to an unhandled rejection and lag the red dot until the poll.
      invoke("send_input", { id, data: d }).catch((e) => { if (DEAD_RE.test(String(e))) noteDeadPanes([id]); });
    }
  });
  // consumed = absolute BYTE cursor for read_output_delta (CONTRACT seam 1) — seeded 0
  // on every fresh attach (the protocol's defined fresh-consumer path: backend returns
  // the retained tail). pendingWrites/polling = poller backpressure + in-flight latch.
  // webgl/webglBlocked = renderer policy state (syncRenderers owns them).
  const s = { term, fit, el, consumed: 0, pendingWrites: 0, polling: false, webgl: null, webglBlocked: false, lastRows: 0, lastCols: 0 };
  sessions[id] = s;
  populateBranch(0); // sessions[id] now set → the liveness guard is valid for the read + retry
  return s;
}

// Fit the visible terminal; only tell the PTY to resize when rows/cols actually
// changed — prevents a resize→SIGWINCH→repaint→resize feedback loop.
function fitSession(s, id) {
  if (!s || !s.fit) return;
  try { s.fit.fit(); } catch (_) {}
  if (hasTauri() && (s.term.rows !== s.lastRows || s.term.cols !== s.lastCols)) {
    s.lastRows = s.term.rows;
    s.lastCols = s.term.cols;
    invoke("resize_pty", { id, rows: s.term.rows, cols: s.term.cols }).catch(() => {});
  }
}

// ---- renderer policy: WebGL2 on VISIBLE panes only (perf-2026-06-10, C-plan C1) ----
// The DOM renderer rebuilds each dirty row's span-DOM per frame — N streaming TUIs ≈
// 10-30k node churn/s. WebGL moves that to GPU draws. But WKWebView caps live WebGL
// contexts per page (~16; the OLDEST context gets context-lost when exceeded), so we
// attach ONLY to visible panes and dispose on hide: live contexts == visible ≤ MAX_WEBGL,
// deterministic, and another canvas can never evict a visible terminal. Hidden panes
// fall back to the DOM renderer, which xterm 6 already render-pauses (IntersectionObserver)
// — they cost nothing until revealed. FitAddon behavior is untouched.
const MAX_WEBGL = 8;
// Field kill-switch (rollback without a rebuild): localStorage at_no_webgl="1" → never attach.
let webglDisabled = (() => { try { return localStorage.getItem("at_no_webgl") === "1"; } catch (_) { return false; } })();
// Lazily-loaded WebglAddon constructor, cached after the first dynamic import() so the ~124KB
// module is parsed exactly ONCE — on the first terminal that attaches WebGL, never at launcher
// boot. _WebglCtor holds the resolved class; _WebglLoad de-dupes concurrent first-attach loads.
let _WebglCtor = null;
let _WebglLoad = null;
function loadWebglCtor() {
  if (_WebglCtor) return Promise.resolve(_WebglCtor);
  if (!_WebglLoad) {
    _WebglLoad = import("/vendor/addon-webgl.mjs")
      .then((m) => { _WebglCtor = m.WebglAddon; return _WebglCtor; })
      .catch((e) => { _WebglLoad = null; throw e; }); // allow a retry on a transient load failure
  }
  return _WebglLoad;
}
// async because the addon module loads lazily on first use. The sole caller (syncRenderers) is
// fire-and-forget — nothing awaits the return — so async is safe and WebGL still attaches on the
// first terminal exactly as before. Guards are RE-CHECKED after the await: the session may have
// hidden/disposed (s.webgl set by a racing call, s.term gone) while the module was loading.
async function attachWebgl(s) {
  if (webglDisabled || !s || s.webgl || s.webglBlocked || !s.term) return;
  let WebglAddon;
  try {
    WebglAddon = await loadWebglCtor();
  } catch (_) {
    // Module failed to load (e.g. offline vendor asset) — leave the DOM renderer in place for
    // this pane WITHOUT latching the global disable: a later attach can retry the import.
    s.webglBlocked = true;
    return;
  }
  // The world may have moved during the await — re-assert every guard before touching the term.
  if (webglDisabled || s.webgl || s.webglBlocked || !s.term) return;
  try {
    const addon = new WebglAddon();
    addon.onContextLoss(() => {
      // GPU context lost (OOM / suspend / cap eviction): dispose() restores the DOM
      // renderer (README-blessed). PERMANENT fallback for THIS pane — re-attaching on
      // a memory-pressured GPU just flaps attach→lose→attach, worse than DOM for good.
      try { addon.dispose(); } catch (_) {}
      s.webgl = null;
      s.webglBlocked = true;
    });
    s.term.loadAddon(addon); // throws when WebGL2 is unavailable in this webview
    s.webgl = addon;
  } catch (_) {
    // Per-pane block first (review F1): getContext("webgl2") returns null TRANSIENTLY
    // under GPU pressure / context churn, not just on missing WebGL2 — one flaky pane
    // must not downgrade every pane for the whole session. Latch globally only when a
    // bare capability probe agrees WebGL2 is truly absent from this webview.
    s.webglBlocked = true;
    try {
      if (!document.createElement("canvas").getContext("webgl2")) webglDisabled = true;
    } catch (_) { webglDisabled = true; }
  }
}
function detachWebgl(s) {
  if (!s || !s.webgl) return;
  // Free the GPU context NOW, not at GC (review F2): the addon's dispose() only removes
  // its canvas — the GL context survives until GC, and WKWebView evicts the OLDEST live
  // context when over the cap, which can be a VISIBLE pane (→ context-loss → permanent
  // DOM fallback). Dispose FIRST (unhooks the addon's own context-loss listener so the
  // forced loss can't latch webglBlocked), then explicitly lose the orphaned context.
  const addon = s.webgl;
  s.webgl = null;
  let gl = null;
  try { gl = (addon._renderer && addon._renderer._gl) || addon._gl || null; } catch (_) {}
  try { addon.dispose(); } catch (_) {}
  try { if (gl) { const x = gl.getExtension("WEBGL_lose_context"); if (x) x.loseContext(); } } catch (_) {}
}
// Reconcile EVERY session's renderer against the visible set: visible panes (active
// pane first, so it always wins a slot when a >MAX_WEBGL grid hits the cap) attach;
// everything else disposes (frees its GPU context). Single call site: relayout()'s rAF
// AFTER the fit pass — loadAddon against a hidden/0×0 element is the classic webgl throw.
function syncRenderers(visibleIds) {
  const ordered = [...visibleIds].sort((a, b) => (a === activeId ? -1 : b === activeId ? 1 : 0));
  const want = new Set(ordered.slice(0, MAX_WEBGL));
  for (const id of Object.keys(sessions)) {
    const s = sessions[id];
    if (want.has(id)) attachWebgl(s);
    else detachWebgl(s);
  }
}

let gridMode = false;
let dragState = null;
let broadcast = false; // type once → every pane in the active workspace

// ── split-tree layout (#6 flexible panes) ───────────────────────────────────
// Each workspace gets ws.layout = a binary split tree (layout-tree.js) — a VIEW over the
// authoritative ws.paneIds. ws.zoom = a paneId maximized full-area (null = tile all). Both
// live OFF persistWorkspaces' emitted def — the tree persists to its own
// "at_layouts" key, keyed by pane INDEX so it survives the reopen id-rebuild (idxList).
const LAYOUTS_KEY = "at_layouts";
let layoutsCache = null;
function loadLayouts() {
  if (layoutsCache) return layoutsCache;
  try { layoutsCache = JSON.parse(localStorage.getItem(LAYOUTS_KEY) || "{}") || {}; } catch (_) { layoutsCache = {}; }
  return layoutsCache;
}
function persistLayout(ws) {
  if (!ws || !ws.wsId) return;
  const obj = loadLayouts();
  if (ws.layout) obj[ws.wsId] = ltree.serializeTree(ws.layout, (pid) => paneIdx(pid));
  else delete obj[ws.wsId];
  try { localStorage.setItem(LAYOUTS_KEY, JSON.stringify(obj)); } catch (_) {}
}
function dropLayout(wsId) {
  const obj = loadLayouts();
  if (obj[wsId]) { delete obj[wsId]; try { localStorage.setItem(LAYOUTS_KEY, JSON.stringify(obj)); } catch (_) {} }
}
// Reconcile ws.layout against the live pane set (the single authority): first run materializes
// the tree (saved → migrate from paneIds), then self-heals (prune dead leaves, append new live
// panes). Idempotent — safe on every relayout. Also asserts the tree's leaves == live set.
function ensureLayout(ws, activePanes) {
  if (!ws) return null;
  let tree = ws.layout;
  if (!tree) {
    const saved = loadLayouts()[ws.wsId];
    tree = (saved && ltree.deserializeTree(saved, ws.wsId)) || ltree.buildDefaultTree(activePanes);
  }
  tree = ltree.reconcileTree(tree, activePanes, activeId).tree;
  ws.layout = tree;
  return tree;
}

// panes that receive broadcast input: the active workspace's live panes, MINUS any
// known-dead corpse (D30) — typing into a dead PTY just errors, and its red dot
// already signals death. A pane that dies mid-broadcast is caught by the send .catch
// (noteDeadPanes) and drops out of this set on the next keystroke.
function broadcastTargets() {
  const ws = activeWs ? workspaces[activeWs] : null;
  return ws ? ws.paneIds.filter((id) => sessions[id] && !deadPanes.has(id)) : [];
}

// ONE writer for the topbar active-session label AND #main's `.no-session` class —
// styles.css keys the label's live tick on `#main:not(.no-session) #active-label::before`,
// so the class must flip in lockstep with the text (a stale class = a dot pulsing over
// "no session", or a frozen dot over a live pane).
function setActiveLabel(id) {
  const lab = document.getElementById("active-label");
  if (lab) lab.textContent = id ? `▸ ${id}` : "no session";
  document.getElementById("main")?.classList.toggle("no-session", !id);
}

// Select a tile as the active pane WITHOUT leaving grid — the active outline
// follows the click. Use setActive (or ⤢) to maximize/focus the active pane.
function setActivePane(id) {
  if (!id || !sessions[id]) return;
  const owner = paneOwner(id);
  if (owner && owner !== activeWs) { activeWs = owner; relayout(); renderWorkspaces(); }
  activeId = id;
  setActiveLabel(id);
  for (const pid of Object.keys(sessions)) {
    sessions[pid].el.classList.toggle("active", pid === activeId);
  }
  const s = sessions[id];
  if (s) requestAnimationFrame(() => s.term.focus());
  resetQueueSig();  // user-driven selection: force the next queue tick to repaint
  pollRevealNow();  // a cross-workspace click reveals hidden panes — paint them NOW
}

function setActive(id) {
  // switching to a pane in another workspace must make that workspace active,
  // or relayout() (scoped to activeWs) won't render it.
  if (id) {
    const owner = paneOwner(id);
    if (owner && owner !== activeWs) activeWs = owner;
  }
  activeId = id;
  const aws = activeWs ? workspaces[activeWs] : null;
  if (aws) aws.zoom = null; // a plain "go to this pane" shows the tiled view (maximize = maximizePane)
  updateGridBtn();
  setActiveLabel(id);
  if (id) ensureSession(id);
  relayout();
  renderWorkspaces();
  if (id) {
    const s = sessions[id];
    requestAnimationFrame(() => { if (s) { fitSession(s, id); s.term.focus(); } });
  }
  resetQueueSig();  // belt+braces: active row highlight repaints next tick even if a path forgot
  pollRevealNow();  // the focused pane may have been in the hidden poll lane — catch it up instantly
  persistActiveWs();
}

// Lay out the terminal host for the ACTIVE workspace only: grid (its panes
// tiled) or focus (active pane). Panes of other workspaces are hidden but stay
// alive (PTY + xterm) — within-session retention (D7).
// BridgeMind launcher empty-state: shown only when NO panes exist (sessions is the
// authoritative paneId map). Additive + display-only; legacy #placeholder is the graceful
// fallback (hidden when the rich card shows, shown if #launcher is absent).
function syncEmptyState() {
  const empty = Object.keys(sessions).length === 0;
  const lc = document.getElementById("launcher");
  const ph = document.getElementById("placeholder");
  if (lc) lc.classList.toggle("hidden", !empty);
  if (ph) ph.classList.toggle("hidden", empty);
}

function relayout() {
  const ids = Object.keys(sessions);
  syncEmptyState();
  const ws = activeWs ? workspaces[activeWs] : null;
  const activePanes = ws ? ws.paneIds.filter((id) => sessions[id]) : [];
  const inActive = new Set(activePanes);
  // TILE MODE (#6): tiling is always on when ≥1 pane. 1 pane = a single leaf filling the host;
  // N panes tile via ws.layout (split tree). ws.zoom maximizes one pane full-area. Replaces the
  // old Grid/Columns/Rows. Panes are absolutely positioned (inline left/top/w/h) — NEVER
  // reparented/recreated, so xterm + its WebGL context stay put.
  if (activePanes.length > 0) {
    host.classList.add("tile");
    host.classList.remove("grid");
    host.style.gridTemplateColumns = "";
    host.style.gridTemplateRows = "";
    if (!inActive.has(activeId)) activeId = activePanes[0];
    if (ws.zoom && !inActive.has(ws.zoom)) ws.zoom = null;
    const tree = ensureLayout(ws, activePanes); // migrate / self-heal the tree vs the live set
    const rects = ws.zoom ? [{ pane: ws.zoom, ...hostBox() }] : layoutRects(tree, hostBox());
    const rmap = new Map(rects.map((r) => [r.pane, r]));
    for (const id of ids) {
      const r = rmap.get(id);
      const el = sessions[id].el;
      if (r) {
        el.style.display = "flex";
        el.style.left = r.x + "px"; el.style.top = r.y + "px";
        el.style.width = r.w + "px"; el.style.height = r.h + "px";
        el.style.order = "";
        el.classList.toggle("active", id === activeId);
        el.classList.toggle("zoomed", !!ws.zoom && id === ws.zoom);
      } else {
        el.style.display = "none";
        el.style.left = el.style.top = el.style.width = el.style.height = "";
        el.classList.remove("active", "zoomed");
      }
    }
  } else {
    host.classList.remove("tile", "grid");
    host.style.gridTemplateColumns = "";
    host.style.gridTemplateRows = "";
    clearHandles();
    for (const id of ids) {
      sessions[id].el.style.display = "none";
      sessions[id].el.classList.remove("active", "zoomed");
    }
  }
  requestAnimationFrame(() => {
    const visible = activePanes.length > 0 ? (ws.zoom ? [ws.zoom] : activePanes) : [];
    for (const id of visible) {
      const s = sessions[id];
      if (s) fitSession(s, id);
    }
    // renderer policy AFTER the fit pass: visible panes get WebGL, hidden ones drop it
    // (GPU contexts == visible set). Unchanged from the grid era.
    syncRenderers(visible);
    // dividers AFTER the fit pass; rebuilt every relayout so they always match the tree.
    // (under zoom there are no seams.)
    if (activePanes.length > 0 && !ws.zoom) buildDividers(ws);
    else clearHandles();
  });
}

// ── split-tree tile renderer + dividers (#6) ─────────────────────────────────
// Rects are computed by recursively slicing the host box per the split tree (absolute-rects
// design). Panes get inline left/top/w/h; one .grid-handle divider per internal split node
// (reuses the existing overlay CSS + the mutate-only-on-move / one-refit-on-up discipline).
// TILE_GAP / MIN_V / MIN_H + splitAxis / layoutRects / collectSplits / seamOf → ./layout-geometry.js
// (pure split-tree→rect math, unit-tested). hostBox() stays here — it reads the live host DOM box.
function hostBox() { return { x: 0, y: 0, w: host.clientWidth, h: host.clientHeight }; }
// re-place pane rects ONLY (no divider rebuild) — used during a resize drag (zero IPC).
function applyPaneRects(ws) {
  if (!ws || !ws.layout) return;
  const rects = ws.zoom ? [{ pane: ws.zoom, ...hostBox() }] : layoutRects(ws.layout, hostBox());
  const rmap = new Map(rects.map((r) => [r.pane, r]));
  for (const id of Object.keys(sessions)) {
    const r = rmap.get(id); if (!r) continue;
    const el = sessions[id].el;
    el.style.left = r.x + "px"; el.style.top = r.y + "px"; el.style.width = r.w + "px"; el.style.height = r.h + "px";
  }
}
function buildDividers(ws) {
  clearHandles();
  if (!ws || !ws.layout || ws.zoom) return;
  const splits = []; collectSplits(ws.layout, hostBox(), splits);
  for (const rec of splits) {
    const h = document.createElement("div");
    h.className = "grid-handle " + (rec.dir === "v" ? "col" : "row");
    h._node = rec.node;
    h.setAttribute("role", "separator");
    h.setAttribute("aria-orientation", rec.dir === "v" ? "vertical" : "horizontal");
    // a11y: keyboard-focusable slider — arrow keys nudge the split ratio (see onSplitResizeKey).
    h.tabIndex = 0;
    h.setAttribute("aria-label", rec.dir === "v" ? "Resize columns" : "Resize rows");
    h.setAttribute("aria-valuemin", "0");
    h.setAttribute("aria-valuemax", "100");
    h.setAttribute("aria-valuenow", String(Math.round((rec.node.ratio ?? 0.5) * 100)));
    h.title = "Drag or arrow keys to resize · double-click to reset";
    h.addEventListener("pointerdown", startSplitResize);
    h.addEventListener("keydown", onSplitResizeKey);
    h.addEventListener("dblclick", resetSplit);
    host.appendChild(h);
  }
  positionDividers(ws);
}
function positionDividers(ws) {
  if (!ws || !ws.layout) return;
  const splits = []; collectSplits(ws.layout, hostBox(), splits);
  const handles = [...host.querySelectorAll(".grid-handle")];
  for (const rec of splits) {
    const h = handles.find((el) => el._node === rec.node); if (!h) continue;
    const s = seamOf(rec);
    if (rec.dir === "v") { h.style.left = s.left + "px"; h.style.top = s.top + "px"; h.style.height = s.height + "px"; h.style.width = ""; }
    else { h.style.top = s.top + "px"; h.style.left = s.left + "px"; h.style.width = s.width + "px"; h.style.height = ""; }
    h.setAttribute("aria-valuenow", String(Math.round((rec.node.ratio ?? 0.5) * 100))); // keep SR value in sync with drags
  }
}
function startSplitResize(e) {
  e.preventDefault(); e.stopPropagation();
  const node = e.currentTarget._node;
  const ws = activeWs ? workspaces[activeWs] : null;
  if (!node || !ws || !ws.layout) return;
  const splits = []; collectSplits(ws.layout, hostBox(), splits);
  const rec = splits.find((r) => r.node === node);
  if (!rec) return;
  const horiz = node.dir === "v";
  dragState = {
    node, ws, handle: e.currentTarget, horiz,
    start: horiz ? rec.box.x : rec.box.y,
    len: (horiz ? rec.box.w : rec.box.h) - TILE_GAP,
    min: horiz ? MIN_V : MIN_H,
  };
  e.currentTarget.classList.add("dragging");
  e.currentTarget.setPointerCapture(e.pointerId); // route move/up here even over an xterm
  e.currentTarget.addEventListener("pointermove", onSplitResizeMove);
  e.currentTarget.addEventListener("pointerup", onSplitResizeUp);
  e.currentTarget.addEventListener("lostpointercapture", onSplitResizeUp);
  document.body.classList.add("at-resizing");
  if (!horiz) document.body.classList.add("row-resize");
}
function onSplitResizeMove(e) {
  const d = dragState; if (!d) return;
  const hr = host.getBoundingClientRect();
  const pointer = d.horiz ? e.clientX - hr.left : e.clientY - hr.top;
  if (d.len <= 0) return;
  let r = (pointer - d.start) / d.len;
  const minR = d.min / d.len;
  r = Math.max(minR, Math.min(1 - minR, r));
  if (!Number.isFinite(r)) return;
  d.node.ratio = r;
  applyPaneRects(d.ws);     // panes only — zero IPC, no SIGWINCH storm
  positionDividers(d.ws);   // move existing handles, NEVER rebuild mid-drag (keeps pointer capture)
}
function onSplitResizeUp(e) {
  const d = dragState; if (!d) return;
  d.handle.removeEventListener("pointermove", onSplitResizeMove);
  d.handle.removeEventListener("pointerup", onSplitResizeUp);
  d.handle.removeEventListener("lostpointercapture", onSplitResizeUp);
  try { d.handle.releasePointerCapture(e.pointerId); } catch (_) {}
  d.handle.classList.remove("dragging");
  document.body.classList.remove("at-resizing", "row-resize");
  dragState = null;
  refitVisible();      // exactly ONE refit → fitSession's lastRows/lastCols guard dedups
  persistLayout(d.ws);
}
function resetSplit(e) {
  const node = e.currentTarget._node;
  const ws = activeWs ? workspaces[activeWs] : null;
  if (!node || !ws) return;
  node.ratio = 0.5;
  applyPaneRects(ws); positionDividers(ws); refitVisible(); persistLayout(ws);
}
// a11y: keyboard resize for a focused split divider. A col (vertical) divider responds to
// ←/→, a row (horizontal) one to ↑/↓; Home/End jump to the min/max clamp. Reuses the exact
// ratio-clamp + apply path the pointer drag uses (applyPaneRects → positionDividers → refit).
const SPLIT_KEY_STEP = 0.03; // ~3% per key press
function onSplitResizeKey(e) {
  const key = e.key;
  const node = e.currentTarget._node;
  const ws = activeWs ? workspaces[activeWs] : null;
  if (!node || !ws || !ws.layout) return;
  const horiz = node.dir === "v"; // col divider slides horizontally
  const along = horiz ? ["ArrowLeft", "ArrowRight"] : ["ArrowUp", "ArrowDown"];
  if (!along.includes(key) && key !== "Home" && key !== "End") return;
  e.preventDefault();
  const splits = []; collectSplits(ws.layout, hostBox(), splits);
  const rec = splits.find((r) => r.node === node);
  if (!rec) return;
  const len = (horiz ? rec.box.w : rec.box.h) - TILE_GAP;
  if (len <= 0) return;
  const minR = (horiz ? MIN_V : MIN_H) / len;
  let r = node.ratio;
  if (key === "Home") r = minR;
  else if (key === "End") r = 1 - minR;
  else r += (key === "ArrowRight" || key === "ArrowDown" ? 1 : -1) * SPLIT_KEY_STEP;
  r = Math.max(minR, Math.min(1 - minR, r));
  if (!Number.isFinite(r)) return;
  node.ratio = r;
  applyPaneRects(ws); positionDividers(ws); refitVisible(); persistLayout(ws);
  e.currentTarget.setAttribute("aria-valuenow", String(Math.round(r * 100)));
}
// Migrate a single pane to another workspace (live reorg). The pane keeps its id + running PTY;
// only the owning ws.paneIds + split layout change. harness/model travel via paneMetaById (id-
// keyed), so phMeta + the Shift+Enter newline stay correct after the move.
function movePaneToWorkspace(paneId, targetWsId) {
  const srcWs = workspaces[paneOwner(paneId)];
  const tgt = workspaces[targetWsId];
  if (!srcWs || !tgt || srcWs === tgt || tgt.dormant) return;
  srcWs.paneIds = srcWs.paneIds.filter((p) => p !== paneId);
  srcWs.layout = ltree.removeLeaf(srcWs.layout, paneId);
  srcWs.count = srcWs.paneIds.length;
  if (srcWs.zoom === paneId) srcWs.zoom = null;
  tgt.paneIds.push(paneId);
  tgt.count = tgt.paneIds.length;
  tgt.layout = ltree.reconcileTree(tgt.layout, tgt.paneIds.filter((id) => sessions[id]), paneId).tree;
  try { persistWorkspaces(); persistLayout(srcWs); persistLayout(tgt); } catch (_) {}
  activeWs = targetWsId; activeId = paneId;
  setActiveLabel(paneId);
  renderWorkspaces();
  relayout();
  try { showToast(`Moved ${paneLabel(paneId)} → ${tgt.name}`); } catch (_) {}
}
// Move ALL of a workspace's live panes into another, then drop the now-empty source. (Covers
// the "multiple panes" migrate — merge one workspace into another.)
function mergeWorkspaceInto(srcWsId, targetWsId) {
  const srcWs = workspaces[srcWsId];
  if (!srcWs || !workspaces[targetWsId] || srcWsId === targetWsId) return;
  const panes = srcWs.paneIds.filter((id) => sessions[id]).slice();
  if (!panes.length) { try { showToast("No live panes to move"); } catch (_) {} return; }
  for (const pid of panes) movePaneToWorkspace(pid, targetWsId);
  if (workspaces[srcWsId] && workspaces[srcWsId].paneIds.filter((id) => sessions[id]).length === 0) {
    delete workspaces[srcWsId];
    try { dropLayout(srcWsId); persistWorkspaces(); } catch (_) {}
    renderWorkspaces();
  }
}

// Auto-tile: re-balance ALL panes of the active workspace into an even (≈grid) split tree.
function autoTile() {
  const ws = activeWs ? workspaces[activeWs] : null;
  if (!ws) return;
  const live = ws.paneIds.filter((id) => sessions[id]);
  if (!live.length) return;
  ws.zoom = null;
  ws.layout = ltree.buildBalancedTree(live);
  persistLayout(ws);
  relayout();
}

// Split the focused pane, spawning a new one beside/below it (#6 add). dir "v"=right, "h"=down.
async function splitPane(dir) {
  const ws = activeWs ? workspaces[activeWs] : null;
  if (!ws || ws.dormant) { launchWizard(); return; } // no live workspace → new one
  const live = ws.paneIds.filter((id) => sessions[id]);
  const anchor = (activeId && sessions[activeId]) ? activeId : live[live.length - 1];
  ws.zoom = null;
  const newId = await spawnPane(activeWs, null, ws.harness, false, undefined, undefined, undefined);
  if (!newId) return;
  if (anchor && sessions[newId]) {
    // explicit placement: drop any reconcile auto-placement, then split the anchor leaf.
    ws.layout = ltree.removeLeaf(ws.layout, newId);
    ws.layout = ltree.splitLeaf(ws.layout || ltree.leaf(anchor), anchor, dir, newId, "after");
  } else if (anchor && pending[newId]) {
    // over the concurrency cap → the pane is QUEUED (no session yet). Stash the intended
    // placement on the pending record so admitPending splits the anchor when a slot frees,
    // instead of reconcile auto-placing it with an alternating direction.
    pending[newId].splitHint = { anchor, dir, where: "after" };
    try { persistPending(); } catch (_) {} // survive a reload while queued
    try { showToast("Split queued — the new pane lands beside this one when a slot frees"); } catch (_) {}
  }
  ws.count = ws.paneIds.length;
  persistWorkspaces();
  persistLayout(ws);
  if (sessions[newId]) setActivePane(newId); // a queued pane has no session to focus yet
  relayout();
}

// ── layout helpers shared by the split-tree divider system ────────────────────
// (The legacy grid-track drag-resize subsystem — buildHandles/positionHandles/
// startCol|RowResize + the gridFr/gridRowFr maps — was deleted 2026-07: the
// split-tree dividers above own resize now. clearHandles stays: relayout and
// buildDividers reuse the same .grid-handle overlay.)

function gridActivePanes() {
  const ws = activeWs ? workspaces[activeWs] : null;
  return ws ? ws.paneIds.filter((id) => sessions[id]) : [];
}

function clearHandles() {
  host.querySelectorAll(".grid-handle").forEach((h) => h.remove());
}

function refitVisible() {
  const ws = activeWs ? workspaces[activeWs] : null;
  const vis = ws && ws.zoom ? (sessions[ws.zoom] ? [ws.zoom] : gridActivePanes()) : gridActivePanes();
  for (const id of vis) {
    const s = sessions[id];
    if (s) fitSession(s, id);
  }
}

// The topbar "Focus" button + ⌘G: zoom the active pane to full-area (toggle).
function updateGridBtn() {
  const b = document.getElementById("grid-btn");
  if (!b) return;
  const ws = activeWs ? workspaces[activeWs] : null;
  const zoomed = !!(ws && ws.zoom);
  // icon is an SVG <use> (don't overwrite it) — convey state via aria-pressed + title only.
  b.setAttribute("aria-pressed", zoomed ? "true" : "false");
  b.title = zoomed ? "Restore — exit maximize (⌘G)" : "Focus — maximize the active pane (⌘G)";
}

function toggleGrid() {
  const ws = activeWs ? workspaces[activeWs] : null;
  if (!ws) return;
  // Prefer activeId when still a live session; otherwise recover a sensible target so ⌘G
  // doesn't silently no-op when focus landed without the mousedown → setActivePane path.
  let id = activeId && sessions[activeId] ? activeId : null;
  if (!id) {
    if (ws.zoom && sessions[ws.zoom]) {
      id = ws.zoom; // restore the currently maximized pane
    } else {
      const live = ws.paneIds.filter((p) => sessions[p]);
      if (live.length === 1) {
        id = live[0];
      } else {
        for (const pid of live) {
          const s = sessions[pid];
          if (s && s.el && s.el.contains(document.activeElement)) { id = pid; break; }
        }
      }
    }
    if (!id) {
      console.debug("[toggleGrid] no-op: activeId null/stale and no recoverable focused/zoomed pane");
      return;
    }
    activeId = id;
  }
  ws.zoom = ws.zoom === id ? null : id;
  updateGridBtn();
  relayout();
}

// Maximize/unmaximize a specific pane (⤢ pane button + kebab) — full-area zoom toggle.
function maximizePane(id) {
  if (!sessions[id]) return;
  const owner = paneOwner(id);
  const ws = workspaces[owner];
  if (!ws) return;
  if (owner !== activeWs) activeWs = owner;
  ws.zoom = ws.zoom === id ? null : id;
  activeId = id;
  setActiveLabel(id);
  updateGridBtn();
  relayout();
  renderWorkspaces();
  const s = sessions[id];
  if (s) requestAnimationFrame(() => s.term.focus());
}

// ── sidebar toggle: hide/show the left rail to widen the workspace area ──
const RAIL_COLLAPSED_KEY = "at_rail_collapsed";
function applyRailCollapsed(collapsed) {
  const app = document.getElementById("app");
  const btn = document.getElementById("rail-toggle");
  if (app) app.classList.toggle("rail-collapsed", collapsed);
  if (btn) btn.setAttribute("aria-pressed", collapsed ? "true" : "false");
}
function toggleRail() {
  const app = document.getElementById("app");
  if (!app) return;
  const collapsed = !app.classList.contains("rail-collapsed");
  applyRailCollapsed(collapsed);
  try { localStorage.setItem(RAIL_COLLAPSED_KEY, collapsed ? "1" : "0"); } catch (_) {}
  // #main (flex:1) just grew/shrank → refit the tiled panes to the new host width.
  requestAnimationFrame(() => relayout());
}

// Broadcast mode: keystrokes in the focused pane fan out to every pane in the
// active workspace. Tile outlines turn accent so it's obvious it's armed.
function toggleBroadcast() {
  broadcast = !broadcast;
  const b = document.getElementById("broadcast-btn");
  if (b) b.classList.toggle("active", broadcast);
  host.classList.toggle("broadcast", broadcast);
  const n = broadcastTargets().length;
  showToast(broadcast ? `Broadcast ON → typing goes to all ${n} pane${n === 1 ? "" : "s"}` : "Broadcast off");
  if (activeId && sessions[activeId]) sessions[activeId].term.focus();
}

// Stream each session's output incrementally via the read_output_delta byte-cursor
// protocol (CONTRACT seam 1, perf-2026-06-10). Replaces the old full-snapshot
// read_output: payload per tick is now the actual PTY delta, not O(total scrollback),
// and the cursor is an absolute BYTE offset owned by the backend — NEVER derived from
// JS string lengths (UTF-16 units ≠ bytes; the old `out.length` cursor was both).

// visible panes = the active workspace's panes (grid) or just the focused one — the
// same definition relayout's rAF uses. Shared by the poller cadence + reveal hook.
function visiblePaneIds() {
  const ws = activeWs ? workspaces[activeWs] : null;
  const panes = ws ? ws.paneIds.filter((id) => sessions[id]) : [];
  // tile mode: all active panes are visible, unless one is zoomed full-area.
  return ws && ws.zoom && sessions[ws.zoom] ? [ws.zoom] : panes;
}

// Apply one pane's delta reply to its terminal. Shared by the out-of-band reveal poll
// (pollPane) and the batched scheduler poll (pollOutput), so the reset + backpressure
// policy lives in exactly one place. applyDelta (pure, poll-core.js) decides; this glue
// applies: reset BEFORE write (truncation = bytes we never saw were evicted, or the pane
// respawned under our id — either way the parser state is stale).
function applyPaneDelta(s, delta) {
  const r = applyDelta(s.consumed, delta);
  // reset rides the WRITE QUEUE as RIS (ESC c), not term.reset() (review F3): xterm
  // writes are async-queued, so an immediate reset() would run BEFORE chunks still
  // queued from prior ticks parse — they'd repaint stale bytes after the "clean"
  // reset. Through the queue, reset and replay land in order.
  const payload = r.reset ? "\x1bc" + (r.write || "") : r.write;
  if (payload) {
    // backpressure accounting: count unflushed term.write callbacks; paneDue skips
    // this pane's reads while too many are outstanding (never queue unboundedly into
    // xterm during an output storm — the backend ring retains, the gap protocol covers).
    s.pendingWrites++;
    s.term.write(payload, () => { s.pendingWrites = Math.max(0, s.pendingWrites - 1); });
    s.lastOut = Date.now(); // pane liveness signal — the Bridge stall-detector keys on this
  }
  s.consumed = r.consumed;
}

// One pane, one delta read — the OUT-OF-BAND reveal path (workspace/pane switch). The
// steady scheduler tick batches instead (pollOutput). `polling` is the per-pane in-flight
// latch shared by both paths: a reveal and a batch must never read the same cursor twice
// (that would double-write the same bytes).
async function pollPane(id, s) {
  if (s.polling) return;
  s.polling = true;
  try {
    const delta = await invoke("read_output_delta", { id, since: s.consumed });
    applyPaneDelta(s, delta);
  } finally {
    s.polling = false;
  }
}

let _pollTick = 0; // monotonic scheduler tick; hidden panes ride every HIDDEN_EVERY-th
function pollOutput() {
  if (hasTauri()) {
    _pollTick++;
    const vis = new Set(visiblePaneIds());
    // BATCH every due, not-already-in-flight pane into ONE invoke (was one per pane —
    // ~66/s at 8 visible panes; each invoke is a macOS main-thread JS eval). The per-pane
    // `polling` latch is set here and cleared in finally, so a concurrent reveal poll never
    // re-reads the same cursor. paneDue still gates PER pane (visible cadence + pendingWrites
    // backpressure), so a back-pressured pane is simply left out of THIS tick's batch.
    const batch = [];
    for (const id of Object.keys(sessions)) {
      const s = sessions[id];
      if (!s || s.polling) continue;
      if (!paneDue({ visible: vis.has(id), tick: _pollTick, pendingWrites: s.pendingWrites })) continue;
      s.polling = true;
      batch.push({ id, since: s.consumed });
    }
    if (batch.length) {
      // FIRE-AND-FORGET (review F5): don't await the fleet — pollOutput reschedules
      // immediately. Per-pane failure isolation is preserved by the backend: it snapshots
      // handles under one brief lock then try_locks each buffer, SKIPPING (omitting) any
      // busy pane — so the batch does bounded, non-blocking work and always settles. A
      // skipped pane is just absent from `entries`, keeps its cursor, and retries next tick.
      // finally clears every latch even on an error reply (no pane can wedge latched-forever).
      invoke("read_output_delta_batch", { reqs: batch })
        .then((entries) => {
          for (const e of entries || []) { const s = sessions[e.id]; if (s) applyPaneDelta(s, e); }
        })
        .catch(() => {})
        .finally(() => { for (const r of batch) { const s = sessions[r.id]; if (s) s.polling = false; } });
    }
  }
  setTimeout(pollOutput, POLL_TICK_MS);
}

// Reveal hook (workspace switch / setActive): newly-visible panes must repaint NOW,
// not up to ~720ms later when the hidden lane's tick comes around. Out-of-band read;
// the `polling` latch makes overlap with the scheduler tick harmless.
function pollRevealNow() {
  if (!hasTauri()) return;
  for (const id of visiblePaneIds()) {
    const s = sessions[id];
    if (s && paneDue({ visible: true, tick: _pollTick, pendingWrites: s.pendingWrites })) {
      pollPane(id, s).catch(() => {});
    }
  }
}

// ---- ranked "who needs you" rail ----
// Render diet (perf-2026-06-10, B-plan finding 3): rail/workspaces/board are PURE
// functions of the structural tuple queueSignature captures — skip their DOM work
// (forced layouts + full rebuilds, every 1s, even idle) when nothing changed. The
// sweep + reconcile side effects above the gate still run EVERY tick. _qSig is reset
// at user-driven mutation sites (setActive/setActivePane/noteDeadPanes) as belt+braces:
// even if some path forgot to repaint imperatively, the next tick renders.
let _qSig = "";
function resetQueueSig() { _qSig = ""; }
async function pollQueue() {
  if (hasTauri()) {
    try {
      const [queue, all, dead] = await Promise.all([
        invoke("list_queue"),
        invoke("list_workspaces"),
        invoke("dead_pane_ids"),
      ]);
      _lastQueue = queue;
      _lastAll = all;
      // D30 deterministic death: the backend sweep returns panes whose PTY child EXITED
      // (is_alive()==false) but still linger in the registry. Mark them dead NOW so the
      // error dot lights + the Bridge drops them — instead of only learning on a failed
      // send. deadPanes is idempotent + cleared on close/reopen; renderWorkspaces below
      // repaints the dot the same tick (it consults deadPanes).
      for (const id of dead) deadPanes.add(id);
      // if the Bridge modal is open, live-prune rows for panes that just died — the list
      // is rendered once at openBridge (bridgeLivePanes excludes deadPanes on NEXT open),
      // so without this a pane that dies WHILE the modal sits over it would linger in the
      // shown team until a close+reopen. Removing the dead row preserves the others' typed
      // focus (no full re-render). Pane ids are [\w-] only → safe in the attribute selector.
      if (dead.length && bridgeEl && !bridgeEl.classList.contains("hidden")) {
        for (const id of dead) {
          const inp = document.querySelector(`#br-panes .br-focus[data-id="${id}"]`);
          if (inp) { const row = inp.closest(".br-pane-row"); if (row) row.remove(); }
        }
      }
      reconcilePending(all); // reload-reconcile: adopt panes admitted while we were gone
      const sig = queueSignature({
        queue, all, dead, activeId, activeWs, workspaces,
        deadPanes, pendingKeys: Object.keys(pending),
      });
      const changed = sig !== _qSig;
      if (changed) {
        renderRail(queue, all);
        renderWorkspaces(queue);
        renderBoard(queue, all); // 09-03 Tier A: no-op while the board is closed; auto-moves cards on state change (≤1s tick → AC-3 ≤2s)
        _qSig = sig; // commit AFTER the renders — a throwing render must retry next tick, not freeze stale UI
      }
      // Scheduler is the ONE time-varying surface (elapsedLabel ticks while queued) —
      // keep it repainting while anything is pending; otherwise only on real change.
      if (changed || Object.keys(pending).length) renderScheduler(queue);
      // Live session cost readout: a small reduce over dgRuns each tick (cheap). Catches the case
      // where a run completes via an event path that didn't already call renderSessionCost.
      renderSessionCost();
      // While the board overlay is open, keep its MCP task lane fresh — agents/operators
      // may transition tasks out-of-band. Change-gated (`_mcpSig`) so an unchanged poll
      // does NOT rebuild the board (respects the renderBoard-on-change convention).
      if (boardOpen) {
        await refreshMcpTasks();
        const sig = _mcpTasks.map((t) => t.id + ":" + t.lifecycle).join(",");
        if (sig !== _mcpSig) { _mcpSig = sig; renderBoard(_lastQueue, _lastAll); }
      }
    } catch (_) {}
  }
  setTimeout(pollQueue, 1000);
}

// ---- rail motion (10-01): keyed reconcile + WAAPI FLIP / enter / exit + amber pulse ----
// The rail re-renders every ~1s (pollQueue). Instead of replaceChildren() snapping the
// whole list, we keep DOM nodes keyed by pane id and only enter/exit/move what changed,
// so survivors FLIP smoothly. Motion fires ONLY when geometry actually moves (the dy
// gate) → a no-change tick or an active-only change produces ZERO motion. Ranking stays
// Rust-owned: we render list_queue's order verbatim (rail-core never re-ranks).
const railNodes = new Map();   // pane id -> <li>          (the keys renderRail lacked)
const railClosing = new Map(); // pane id -> { li, anim }  (resurrect guard: re-enter mid-exit)
const _railReduceMQ =
  typeof window !== "undefined" && window.matchMedia
    ? window.matchMedia("(prefers-reduced-motion: reduce)")
    : { matches: false };
function railReduced() { return _railReduceMQ.matches; }

// Motion tokens read once from :root (the design-token SSOT); fall back if absent.
let _motionTokens = null;
function motionTokens() {
  if (_motionTokens) return _motionTokens;
  const cs = getComputedStyle(document.documentElement);
  const ms = (v, d) => { const n = parseFloat(cs.getPropertyValue(v)); return Number.isFinite(n) ? n : d; };
  const ez = (v, d) => cs.getPropertyValue(v).trim() || d;
  _motionTokens = {
    durFast: ms("--dur-fast", 120), durBase: ms("--dur-base", 180), durSlow: ms("--dur-slow", 240),
    easeStandard: ez("--ease-standard", "ease"),
    easeEmphasis: ez("--ease-emphasis", "ease"),
    easeExit: ez("--ease-exit", "ease"),
  };
  return _motionTokens;
}

function buildRailRow(r) {
  const li = document.createElement("li");
  li.className = "qrow";
  const mark = document.createElement("span"); mark.className = "mark";
  const qid = document.createElement("span"); qid.className = "qid"; qid.textContent = r.id;
  const qmeta = document.createElement("span"); qmeta.className = "qmeta";
  li.append(mark, qid, qmeta);
  li.onclick = () => setActive(r.id);
  // Keyboard-operable: the "who needs you" rows carry onclick but were mouse-only. Make each an
  // Enter/Space-activatable button so a keyboard user can select any queue entry (not just the
  // top pick via Cmd+Shift+J).
  li.tabIndex = 0;
  li.setAttribute("role", "button");
  li.addEventListener("keydown", (e) => {
    if (e.key === "Enter" || e.key === " ") { e.preventDefault(); setActive(r.id); }
  });
  updateRailRow(li, r);
  return li;
}

// Re-apply per-tick row state IN PLACE (classList.toggle, so .pulse/.closing survive).
// Returns gainedNeeds — `wasNeeds` is read BEFORE toggling `.needs` so a steadily-needing
// row never re-pulses every tick (the amber-strobe trap).
function updateRailRow(li, r) {
  const wasNeeds = li.classList.contains("needs");
  const nowNeeds = !!r.needs_human;
  li.classList.toggle("needs", nowNeeds);
  li.classList.toggle("active", r.id === activeId);
  li.querySelector(".mark").textContent = nowNeeds ? "►" : "";
  li.querySelector(".qmeta").textContent = railMeta(r);
  return !wasNeeds && nowNeeds;
}

// Two-phase exit: pin the row out of flow at its current rect (.closing → position:absolute
// against the now-position:relative #queue), fade it, THEN remove — because `.hidden` is
// `display:none !important`, you can't fade a removed node. Reduced motion → remove instantly.
function exitRailRow(ul, id, li) {
  if (railReduced()) { li.remove(); return; }
  const rect = li.getBoundingClientRect();
  const urect = ul.getBoundingClientRect();
  li.classList.add("closing");
  li.style.top = `${rect.top - urect.top + ul.scrollTop}px`;
  li.style.left = `${rect.left - urect.left}px`;
  li.style.width = `${rect.width}px`;
  const t = motionTokens();
  const anim = li.animate(
    [{ opacity: 1, transform: "translateX(0)" }, { opacity: 0, transform: "translateX(10px)" }],
    { duration: t.durFast, easing: t.easeExit, fill: "forwards" }, // fill ok: node removed onfinish
  );
  railClosing.set(id, { li, anim });
  anim.onfinish = () => { li.remove(); railClosing.delete(id); };
  anim.oncancel = () => { railClosing.delete(id); };
}

function renderRail(queue, all) {
  const ul = document.getElementById("queue");
  if (!ul) return;
  const seen = new Set(queue.map((r) => r.id));
  const extra = (all || [])
    .filter((id) => !seen.has(id))
    .map((id) => ({ id, harness: "", state: "starting", reason: "-", needs_human: false }));
  // Drop ORPHAN rows — a backend pane that belongs to no workspace (paneOwner null). These are
  // phantoms (e.g. a spawn that failed to register on the frontend); showing them in "WHO NEEDS
  // YOU" is misleading + unactionable (there's no pane to open). A legit pane is always in its
  // ws.paneIds before it reaches list_queue (createWorkspace registers the ws first; admit pushes).
  const rows = [...queue, ...extra].filter((r) => workspaces[paneOwner(r.id)]);
  const reduced = railReduced();

  // EMPTY: exit every live row, then show the placeholder.
  if (rows.length === 0) {
    for (const [id, li] of [...railNodes]) { railNodes.delete(id); exitRailRow(ul, id, li); }
    if (!ul.querySelector(".queue-empty")) {
      const e = document.createElement("li");
      e.className = "queue-empty";
      e.textContent = "No agents yet — ⌘N to start one.";
      ul.appendChild(e);
    }
    return;
  }

  const nextSet = new Set(rows.map((r) => r.id));

  // (1) MEASURE first tops of persisting nodes (before any mutation).
  const firstTop = new Map();
  if (!reduced) {
    for (const [id, li] of railNodes) {
      if (!li.classList.contains("closing")) firstTop.set(id, li.getBoundingClientRect().top);
    }
  }

  const emptyEl = ul.querySelector(".queue-empty");
  if (emptyEl) emptyEl.remove();

  // (2) EXITS — ids we hold that the backend dropped this tick.
  for (const [id, li] of [...railNodes]) {
    if (!nextSet.has(id)) { railNodes.delete(id); exitRailRow(ul, id, li); }
  }

  // (2b) REUSE / RESURRECT / BUILD.
  const entering = [];
  const pulses = [];
  for (const r of rows) {
    let li = railNodes.get(r.id);
    if (li) { if (updateRailRow(li, r)) pulses.push(li); continue; }
    const closing = railClosing.get(r.id);
    if (closing) { // re-entered while still fading out → reclaim its node
      if (closing.anim) { try { closing.anim.cancel(); } catch (_) {} }
      li = closing.li;
      li.classList.remove("closing");
      li.style.position = ""; li.style.top = ""; li.style.left = "";
      li.style.width = ""; li.style.opacity = ""; li.style.transform = "";
      railClosing.delete(r.id);
      if (updateRailRow(li, r)) pulses.push(li);
    } else {
      li = buildRailRow(r);
      if (!reduced) li.style.opacity = "0"; // avoid a 1-frame full-opacity flash before enter
      entering.push(li);
    }
    railNodes.set(r.id, li);
  }

  // (2c) ORDER — appendChild in rank order moves each node into place; closing ghosts
  // stay put (out of flow). list_queue order is honored verbatim (no client re-rank).
  for (const r of rows) ul.appendChild(railNodes.get(r.id));

  if (reduced) return; // correct final DOM, zero motion

  // (3) MEASURE last + (4) INVERT+PLAY (FLIP). No fill → releases to layout for next tick.
  const t = motionTokens();
  for (const [id, top0] of firstTop) {
    const li = railNodes.get(id);
    if (!li) continue; // exited this tick → it fades, doesn't FLIP
    const dy = top0 - li.getBoundingClientRect().top;
    if (Math.abs(dy) < 0.5) continue; // only animate rows that actually moved
    li.animate(
      [{ transform: `translateY(${dy}px)` }, { transform: "translateY(0)" }],
      { duration: t.durBase, easing: t.easeStandard },
    );
  }

  // ENTER — staggered fade + slide-in; clear the inline opacity guard on finish.
  entering.forEach((li, i) => {
    const a = li.animate(
      [{ opacity: 0, transform: "translateX(14px)" }, { opacity: 1, transform: "translateX(0)" }],
      { duration: t.durBase, delay: i * 45, easing: t.easeEmphasis },
    );
    a.onfinish = () => { li.style.opacity = ""; };
  });

  // PULSE — a row that JUST gained needs_human flashes the load-bearing amber once.
  for (const li of pulses) {
    li.classList.remove("pulse");
    void li.offsetWidth; // restart the CSS keyframe
    li.classList.add("pulse");
    li.addEventListener("animationend", () => li.classList.remove("pulse"), { once: true });
  }

  // a11y: the amber pulse is silent to screen readers. When ≥1 row NEWLY needs a human,
  // announce the current total via the SR-only polite live region (not #queue itself — the
  // FLIP re-render churn would make it read the whole list every tick).
  if (pulses.length) announceQueueNeeds(rows);
}

// SR-only "N agents need you" announcement — fired only on a needs edge (see renderRail).
function announceQueueNeeds(rows) {
  const el = document.getElementById("queue-sr-status");
  if (!el) return;
  const n = rows.filter((r) => r.needs_human).length;
  if (n <= 0) return;
  el.textContent = `${n} agent${n === 1 ? "" : "s"} need${n === 1 ? "s" : ""} you.`;
}

async function focusTop() {
  if (!hasTauri()) return;
  try {
    const queue = await invoke("list_queue");
    const top = queue.find((r) => r.needs_human) || queue[0];
    if (top) setActive(top.id);
  } catch (_) {}
}

// ──────────────────── 09-03 Tier A alt-view board (AC-2/AC-3/AC-6) ───────────────
//
// Model A: the board REFLECTS agent state, it never SETS it. Columns are the adapter
// state machine; cards are auto-placed from the SAME ranked `list_queue` the Queue
// uses (no second ranking — AC-2). The Queue rail stays the default/primary surface;
// the board is an opt-in overlay over #main reached from the rail-head toggle (AC-6).
// ⌘⇧J is untouched. Cross-column drag is a NO-OP; only intra-column reorder (a
// session-only priority nudge) is allowed (AC-3).
//
// The board's pure logic (BOARD_COLS, columnFor, boardRows, bucketByColumn, applyOrder,
// boardReorder) lives in ./board-core.js (imported above) so vitest exercises it in
// isolation; boardRows takes `workspaces` + `paneOwner` as parameters at the call sites.

// ---- BRIDGEBOARD user tasks (#2, 2026-06-18) — a USER-TASK layer ON TOP of the read-only
// pane mirror. Plain {id,title,status,category} objects persisted per-workspace to
// localStorage; render as a second card flavor in the same status columns, draggable
// cross-column (drop updates status). Isolated from PTY/session + the machine ws…-pN id
// (task ids live in their own "t…" namespace). ----
const BOARD_TASKS_KEY = "at_board_tasks";
let boardTasks = (() => { try { return JSON.parse(localStorage.getItem(BOARD_TASKS_KEY) || "{}") || {}; } catch (_) { return {}; } })();
let taskSeq = 0;
function saveBoardTasks() { try { localStorage.setItem(BOARD_TASKS_KEY, JSON.stringify(boardTasks)); } catch (_) {} }
function taskScope() { return activeWs || "_"; }
function tasksForScope() { const k = taskScope(); return Array.isArray(boardTasks[k]) ? boardTasks[k] : []; }
function createBoardTask(status) {
  const col = BOARD_COLS.indexOf(status) >= 0 ? status : BOARD_COLS[0];
  const t = { id: "t" + (Date.now() % 1000000) + "x" + taskSeq++, title: "", status: col, category: "" };
  const k = taskScope();
  if (!Array.isArray(boardTasks[k])) boardTasks[k] = [];
  boardTasks[k].push(t);
  saveBoardTasks();
  return t;
}
function findBoardTask(id) { return tasksForScope().find((t) => t && t.id === id) || null; }
function updateBoardTask(id, patch) { const t = findBoardTask(id); if (!t) return; Object.assign(t, patch); saveBoardTasks(); }
function deleteBoardTask(id) { const k = taskScope(); if (Array.isArray(boardTasks[k])) boardTasks[k] = boardTasks[k].filter((t) => t.id !== id); saveBoardTasks(); }

// ---- MCP task-model projection (Phase-14 durable log/store) — board cards ----
// Distinct from the localStorage `boardTasks` above: these come from the backend
// `list_mcp_tasks` (folds the append-only `agent-teams-tasks-log.jsonl`), are GLOBAL
// (no workspace scope), and are RENDERED here. Lifecycle-based column placement is the
// default; an operator can drag a card to a new column, which calls `update_task_kanban`
// to persist the column override in the durable operator store (Tier B true-kanban).
let _mcpTasks = [];
let _mcpSig = ""; // change-gate so an unchanged poll doesn't rebuild the board

// ---- Tier B operator task store (Phase-14-T2 durable kanban) ----
// `_operatorTasks` holds the result of `list_tasks_kanban`: tasks whose columns the
// human has explicitly set via drag. Index by id for O(1) column lookup.
// A task in _operatorTasksById overrides the lifecycle-derived column for the MCP card.
let _operatorTasks = [];
let _operatorTasksById = {}; // id → TaskRow (from list_tasks_kanban)

// Column mapping (lifecycle → column, operator-override resolution, wire↔display) lives
// in ./kanban-core.js (imported above) so vitest exercises the pure logic in isolation.
// Effective column for an MCP task: operator-store column wins; lifecycle is the fallback.
function mcpTaskEffectiveCol(t) {
  return mcpEffectiveCol(t.lifecycle, _operatorTasksById[t.id]);
}

async function refreshMcpTasks() {
  if (!hasTauri()) return;
  try { _mcpTasks = (await invoke("list_mcp_tasks")) || []; } catch (_) { _mcpTasks = []; }
  // Also refresh the operator task store so column overrides are current.
  try {
    _operatorTasks = (await invoke("list_tasks_kanban")) || [];
    _operatorTasksById = {};
    for (const t of _operatorTasks) { if (t && t.id) _operatorTasksById[t.id] = t; }
  } catch (_) {
    _operatorTasks = [];
    _operatorTasksById = {};
  }
}

// Persist a durable column move for an MCP task via the Tauri CRUD commands.
// If the task already exists in the operator store, update its column; otherwise
// create it (minting an operator-store entry from the MCP log task). Fire-and-forget:
// the board re-renders optimistically from the local cache and the next refreshMcpTasks
// poll confirms the persisted state. Never throws (fail-soft on IPC error).
async function persistMcpTaskColumnMove(mcpTask, newColumn) {
  if (!hasTauri()) return;
  // Wire-form mapping + payload shapes live in kanban-core.js (displayColToWire /
  // buildCreatePayload / buildUpdatePayload) — the same code the vitest suite exercises.
  const colWire = displayColToWire(newColumn);
  const existing = _operatorTasksById[mcpTask.id];
  try {
    if (existing) {
      await invoke("update_task_kanban", { payload: buildUpdatePayload(mcpTask.id, newColumn) });
    } else {
      await invoke("create_task_kanban", { payload: buildCreatePayload(mcpTask, newColumn) });
    }
    // Update local cache immediately so the next renderBoard reflects the change
    // before the next full poll tick.
    _operatorTasksById[mcpTask.id] = { ...existing, id: mcpTask.id, column: colWire };
  } catch (_) {
    // Fail-soft: the column move was optimistic; the next poll will show the true state.
  }
}

let boardOpen = false;
// Session-only intra-column reorder overrides: column → manually-ordered ids. NON-
// DURABLE (lives only in memory; durable order would need the persisted Task model —
// the Tier B line we do NOT cross here). RESETS a row's slot when its column changes.
const boardOrder = {};
// Per-id last column, to detect a state change → drop that id's manual override so the
// machine reclaims its rank position (AC-3 auto-move).
const boardStateSig = {};

// On a state change, evict the moved id from every manual order so it falls back to its
// rank position in the new column (the override is non-durable, by design).
function reconcileBoardOverrides(rows) {
  for (const r of rows) {
    const col = columnFor(r);
    const prev = boardStateSig[r.id];
    if (prev !== undefined && prev !== col) {
      for (const c of Object.keys(boardOrder)) {
        const i = boardOrder[c].indexOf(r.id);
        if (i >= 0) boardOrder[c].splice(i, 1);
      }
    }
    boardStateSig[r.id] = col;
  }
}

// Identity color from the muted WS_PALETTE (status-free), keyed by the pane's owning
// workspace (`${wsId}-p${idx}` → wsId) so every pane of a workspace shares its color.
function paletteColor(id) {
  const base = String(id).replace(/-p\d+$/, "");
  let h = 0;
  for (let i = 0; i < base.length; i++) h = (h * 31 + base.charCodeAt(i)) | 0;
  return WS_PALETTE[Math.abs(h) % WS_PALETTE.length];
}

// Build the board overlay once (absolute over #main; the Queue rail stays visible →
// Queue remains primary). Lazily created so it costs nothing until first opened.
function ensureBoardOverlay() {
  let overlay = document.getElementById("board-overlay");
  if (overlay) return overlay;
  overlay = document.createElement("div");
  overlay.id = "board-overlay";
  overlay.className = "hidden";
  const head = document.createElement("div");
  head.className = "board-head";
  const title = document.createElement("span");
  title.className = "board-title";
  title.textContent = "Board — state machine (read-only · reflects state)";
  const close = document.createElement("button");
  close.className = "board-close";
  close.title = "Back to Queue";
  close.setAttribute("aria-label", "Close board");
  close.appendChild(svgIcon("i-x"));
  close.onclick = () => closeBoard();
  head.append(title, close);
  const cols = document.createElement("div");
  cols.className = "board-cols";
  overlay.append(head, cols);
  // pointer-based card drag (WKWebView breaks native HTML5 dnd) — one delegated listener
  // on the stable overlay covers every card across keyed-reconcile rebuilds.
  overlay.addEventListener("mousedown", onBoardCardDown);
  const main = document.getElementById("main");
  if (main) main.appendChild(overlay);
  return overlay;
}

function openBoard() {
  ensureBoardOverlay().classList.remove("hidden");
  boardOpen = true;
  syncBoardToggle();
  renderBoard(_lastQueue, _lastAll);
  // pull the MCP task log NOW so the Backlog lane is populated on first paint, not
  // ~1s later when the next pollQueue tick refreshes it.
  refreshMcpTasks().then(() => { _mcpSig = ""; renderBoard(_lastQueue, _lastAll); });
}
function closeBoard() {
  const o = document.getElementById("board-overlay");
  if (o) o.classList.add("hidden");
  boardOpen = false;
  syncBoardToggle();
}

// Render the columns + cards. Called every 1s poll tick (no-op while closed): on a real
// state change the card auto-moves to its new column within ≤2s (AC-3). Drag wiring is
// intra-column reorder only; cross-column drops are dropped by boardReorder (no-op).
// ---- board keyed reconcile (perf) ----
// The board re-renders on every queue-signature change (~1Hz) while open. The old
// wrap.replaceChildren() rebuilt every column + card + re-bound every DnD listener each
// tick — and, worse, silently DESTROYED an in-progress inline task-title edit whenever any
// agent changed state mid-edit. The overlay (and thus .board-cols) is built once, so we
// build column shells once and reuse them; cards live in a Map keyed by id and mutate in
// place. A card mid-edit is left ENTIRELY untouched (no mutate, no reparent) so its focused
// input + typed text survive a background poll; finish() re-renders cleanly afterwards.
const boardColShells = new Map(); // colName -> { list, count }
const boardCardNodes = new Map(); // id -> { el, type: 'row' | 'task' }

function ensureBoardShells(wrap) {
  if (boardColShells.size === BOARD_COLS.length && wrap.querySelector(".board-col")) return;
  boardColShells.clear();
  boardCardNodes.clear();
  wrap.replaceChildren();
  for (const colName of BOARD_COLS) {
    const col = document.createElement("div");
    col.className = "board-col" + (colName === "Needs you" ? " board-col-need" : "");
    col.dataset.col = colName;
    const ch = document.createElement("div");
    ch.className = "board-col-head";
    const cl = document.createElement("span");
    cl.className = "board-col-name";
    cl.textContent = colName;
    const cc = document.createElement("span");
    cc.className = "board-col-count";
    ch.append(cl, cc);
    const addBtn = document.createElement("button");
    addBtn.className = "board-add-task";
    addBtn.type = "button";
    addBtn.title = "Add a task to " + colName;
    addBtn.setAttribute("aria-label", "Add task to " + colName);
    addBtn.appendChild(svgIcon("i-plus"));
    addBtn.onclick = (e) => { e.stopPropagation(); const t = createBoardTask(colName); renderBoard(_lastQueue, _lastAll); const el = document.querySelector('.board-card.task[data-id="' + t.id + '"]'); if (el) beginTaskTitleEdit(t.id, el); };
    ch.appendChild(addBtn);
    col.appendChild(ch);
    const list = document.createElement("div");
    list.className = "board-card-list";
    // drop highlighting + the move are handled by the pointer-drag (boardColAtPoint / onBoardDragUp).
    col.appendChild(list);
    wrap.appendChild(col);
    boardColShells.set(colName, { list, count: cc });
  }
}

function renderBoard(queue, all) {
  const overlay = document.getElementById("board-overlay");
  if (!overlay || !boardOpen) return;
  const rows = boardRows(queue || _lastQueue, all || _lastAll, workspaces, paneOwner);
  reconcileBoardOverrides(rows);
  const buckets = bucketByColumn(rows, BOARD_COLS);
  const byId = {};
  for (const r of rows) byId[r.id] = r;
  const wrap = overlay.querySelector(".board-cols");
  if (!wrap) return;
  ensureBoardShells(wrap);
  const tasks = tasksForScope();
  const want = new Set();
  for (const colName of BOARD_COLS) {
    const shell = boardColShells.get(colName);
    const ids = applyOrder(buckets[colName], boardOrder[colName]);
    shell.count.textContent = String(ids.length); // count = queue rows only (tasks excluded, as before)
    const ordered = [];
    for (const id of ids) ordered.push({ id, type: "row" });
    for (const t of tasks) { if (t && t.status === colName) ordered.push({ id: t.id, type: "task", t }); }
    // MCP task-model cards (global): placed by effective column — operator-store override
    // wins over the lifecycle-derived default (Tier B durable kanban, Phase-14-T2).
    for (const t of _mcpTasks) { if (t && mcpTaskEffectiveCol(t) === colName) ordered.push({ id: t.id, type: "mcp", t }); }
    for (const item of ordered) {
      want.add(item.id);
      let rec = boardCardNodes.get(item.id);
      // mid-edit card: leave it exactly as-is — touching it (mutate OR appendChild move)
      // would blur the input. Order may drift for a few seconds; finish() corrects it.
      if (rec && rec.el.classList.contains("editing")) continue;
      if (!rec || rec.type !== item.type) {
        if (rec) rec.el.remove();
        const el = item.type === "row" ? renderBoardCard(byId[item.id], colName)
          : item.type === "mcp" ? renderMcpTaskCard(item.t)
          : renderTaskCard(item.t);
        rec = { el, type: item.type };
        boardCardNodes.set(item.id, rec);
      } else if (item.type === "row") {
        updateBoardCard(rec.el, byId[item.id], colName);
      } else if (item.type === "mcp") {
        updateMcpTaskCard(rec.el, item.t);
      } else {
        updateTaskCard(rec.el, item.t);
      }
      shell.list.appendChild(rec.el); // moves the existing node into place / new column
    }
  }
  // EXIT — drop cards that left the board (never yank one mid-edit; finish() cleans up).
  for (const [id, rec] of [...boardCardNodes]) {
    if (!want.has(id) && !rec.el.classList.contains("editing")) { rec.el.remove(); boardCardNodes.delete(id); }
  }
}

function renderBoardCard(row, colName) {
  const card = document.createElement("div");
  card.className = "board-card" + (row.id === activeId ? " active" : "") + (colName === "Needs you" ? " need" : "");
  card.dataset.id = row.id;
  card.dataset.col = colName;
  const dot = document.createElement("span");
  dot.className = "board-card-dot";
  dot.style.background = paletteColor(row.id);
  const id = document.createElement("span");
  id.className = "board-card-id";
  id.textContent = row.id;
  const meta = document.createElement("span");
  meta.className = "board-card-meta";
  const m = `${row.harness || ""} ${row.state || ""}`.trim() + (row.reason && row.reason !== "-" ? ` · ${row.reason}` : "");
  meta.textContent = m;
  card.append(dot, id, meta);
  card.onclick = () => setActive(card.dataset.id);
  // Keyboard-operable (a11y): the card was mouse-only. Enter/Space activates it,
  // mirroring the wsrow pattern. No column-move keys: queue-row cards reflect machine
  // state (Model A — cross-column is a no-op even for the mouse drag).
  card.tabIndex = 0;
  card.setAttribute("role", "button");
  card.setAttribute("aria-label", `${row.id} — select pane`);
  card.addEventListener("keydown", (e) => {
    if (e.key === "Enter" || e.key === " ") { e.preventDefault(); setActive(card.dataset.id); }
  });
  // drag handled by the delegated pointer listener (onBoardCardDown); id/col read LIVE
  // from the dataset at drag time so a reconciled column move never goes stale.
  return card;
}

// Mutate a queue-row card IN PLACE (reconcile path). Only the active/need class, column,
// and meta line vary per tick; the dot color + id are id-stable (set once at build).
function updateBoardCard(el, row, colName) {
  const dragging = el.classList.contains("dragging");
  el.className = "board-card" + (row.id === activeId ? " active" : "") + (colName === "Needs you" ? " need" : "") + (dragging ? " dragging" : "");
  el.dataset.col = colName;
  const meta = el.querySelector(".board-card-meta");
  if (meta) {
    const m = `${row.harness || ""} ${row.state || ""}`.trim() + (row.reason && row.reason !== "-" ? ` · ${row.reason}` : "");
    if (meta.textContent !== m) meta.textContent = m;
  }
}

// Render a USER task card (.board-card.task) — draggable cross-column; carries a "t…" id.
function renderTaskCard(t) {
  const card = document.createElement("div");
  card.className = "board-card task";
  card.dataset.id = t.id;
  card.dataset.col = t.status;
  card.dataset.task = "1";
  const dot = document.createElement("span");
  dot.className = "board-card-dot task-dot";
  const title = document.createElement("span");
  title.className = "board-card-id task-title";
  title.textContent = t.title || "Untitled task";
  title.title = "Double-click to edit";
  const meta = document.createElement("span");
  meta.className = "board-card-meta";
  if (t.category) { const chip = document.createElement("span"); chip.className = "task-cat"; chip.textContent = t.category; meta.appendChild(chip); }
  const del = document.createElement("button");
  del.className = "task-del"; del.type = "button"; del.title = "Delete task"; del.textContent = "×";
  del.setAttribute("aria-label", "Delete task");
  del.onclick = (e) => { e.stopPropagation(); deleteBoardTask(t.id); renderBoard(_lastQueue, _lastAll); };
  card.append(dot, title, meta, del);
  title.addEventListener("dblclick", (e) => { e.stopPropagation(); beginTaskTitleEdit(t.id, card); });
  // Keyboard-operable (a11y): Enter/Space edits (the dblclick affordance), Delete/Backspace
  // removes, and [ / ] (or Alt+←/→) move the card a column — the same boardMove path the
  // pointer drag uses, so cross-column persistence rules stay single-sourced.
  card.tabIndex = 0;
  card.setAttribute("role", "button");
  card.setAttribute("aria-label",
    `Task: ${t.title || "Untitled task"} — Enter to edit, Delete to remove, [ and ] to move column`);
  card.addEventListener("keydown", (e) => {
    if (e.key === "Enter" || e.key === " ") { e.preventDefault(); beginTaskTitleEdit(card.dataset.id, card); }
    else if (e.key === "Delete" || e.key === "Backspace") { e.preventDefault(); deleteBoardTask(card.dataset.id); renderBoard(_lastQueue, _lastAll); }
    else boardCardMoveKey(e, card);
  });
  // drag handled by the delegated pointer listener (onBoardCardDown).
  return card;
}

// Keyboard column-move for board task cards: [ / ] (or Alt+ArrowLeft / Alt+ArrowRight)
// move the focused card one column left/right through the SAME boardMove path the
// pointer drag lands in (user task → localStorage status; MCP task → durable
// update_task_kanban; queue rows never call this — cross-column is a Model A no-op).
// Re-focuses the moved card after the re-render so repeated presses walk the board.
function boardCardMoveKey(e, card) {
  let dir = 0;
  if (e.key === "[" || (e.altKey && e.key === "ArrowLeft")) dir = -1;
  else if (e.key === "]" || (e.altKey && e.key === "ArrowRight")) dir = 1;
  if (!dir) return;
  e.preventDefault();
  e.stopPropagation();
  const id = card.dataset.id;
  const sourceCol = card.dataset.col;
  const i = BOARD_COLS.indexOf(sourceCol);
  const targetCol = BOARD_COLS[i + dir];
  if (i < 0 || !targetCol) return;
  boardMove(id, sourceCol, targetCol, card.dataset.mcp === "1", null);
  requestAnimationFrame(() => {
    const el = document.querySelector(`.board-card[data-id="${id}"]`);
    if (el) { try { el.focus(); } catch (_) {} }
  });
}

// Mutate a user-task card IN PLACE (reconcile path). Title + category + column vary; never
// called while the card is mid-edit (renderBoard skips editing cards before reaching here).
function updateTaskCard(el, t) {
  const dragging = el.classList.contains("dragging");
  el.className = "board-card task" + (dragging ? " dragging" : "");
  el.dataset.col = t.status;
  const title = el.querySelector(".task-title");
  if (title) { const txt = t.title || "Untitled task"; if (title.textContent !== txt) title.textContent = txt; }
  el.setAttribute("aria-label",
    `Task: ${t.title || "Untitled task"} — Enter to edit, Delete to remove, [ and ] to move column`);
  const meta = el.querySelector(".board-card-meta");
  if (meta) {
    const want = t.category || "";
    const chip = meta.querySelector(".task-cat");
    if (want !== (chip ? chip.textContent : "")) {
      meta.replaceChildren();
      if (want) { const c = document.createElement("span"); c.className = "task-cat"; c.textContent = want; meta.appendChild(c); }
    }
  }
}

// Render an MCP task-model card (.board-card.task.mcp) — the durable log drives
// lifecycle, but the operator can drag cross-column to set a durable column override
// via the Tier B kanban CRUD commands (Phase-14-T2). The lifecycle chip shows the
// agent-owned lifecycle stage; the column placement reflects any operator override.
// No inline title edit / delete: the MCP log is append-only (title set at genesis).
function renderMcpTaskCard(t) {
  const card = document.createElement("div");
  card.className = "board-card task mcp";
  card.dataset.id = t.id;
  card.dataset.task = "1";
  card.dataset.mcp = "1";
  card.dataset.col = mcpTaskEffectiveCol(t);
  const dot = document.createElement("span");
  dot.className = "board-card-dot task-dot mcp-dot";
  const title = document.createElement("span");
  title.className = "board-card-id task-title";
  title.textContent = t.title || "Untitled task";
  title.title = t.title || "";
  const meta = document.createElement("span");
  meta.className = "board-card-meta";
  const chip = document.createElement("span");
  chip.className = "task-cat";
  chip.textContent = t.lifecycle || "created";
  meta.appendChild(chip);
  card.append(dot, title, meta);
  // Keyboard-operable (a11y): MCP cards have no click action (append-only log), but the
  // operator CAN move them cross-column — [ / ] (or Alt+←/→) mirror the durable drag
  // (persistMcpTaskColumnMove via boardMove).
  card.tabIndex = 0;
  card.setAttribute("role", "button");
  card.setAttribute("aria-label",
    `Task: ${t.title || "Untitled task"} (${t.lifecycle || "created"}) — [ and ] to move column`);
  card.addEventListener("keydown", (e) => boardCardMoveKey(e, card));
  // Durable cross-column drag handled by the delegated pointer listener; isMcp read from
  // dataset.mcp, id/col read LIVE at drag time (node persists across reconcile ticks).
  return card;
}

// Mutate an MCP task card IN PLACE (reconcile path). Title + lifecycle chip + column vary.
function updateMcpTaskCard(el, t) {
  const dragging = el.classList.contains("dragging");
  el.className = "board-card task mcp" + (dragging ? " dragging" : "");
  // Keep dataset.col in sync so the next dragstart reads the current column.
  el.dataset.col = mcpTaskEffectiveCol(t);
  const title = el.querySelector(".task-title");
  if (title) { const txt = t.title || "Untitled task"; if (title.textContent !== txt) { title.textContent = txt; title.title = t.title || ""; } }
  const chip = el.querySelector(".task-cat");
  if (chip) { const lc = t.lifecycle || "created"; if (chip.textContent !== lc) chip.textContent = lc; }
  el.setAttribute("aria-label",
    `Task: ${t.title || "Untitled task"} (${t.lifecycle || "created"}) — [ and ] to move column`);
}

// Inline edit of a task title + category. Enter/blur commits, Esc cancels.
function beginTaskTitleEdit(id, card) {
  if (card.querySelector("input.task-edit")) return;
  const t = findBoardTask(id); if (!t) return;
  const titleEl = card.querySelector(".task-title");
  const input = document.createElement("input");
  input.className = "task-edit ph-rename"; input.value = t.title || ""; input.placeholder = "Task title"; input.spellcheck = false;
  const cat = document.createElement("input");
  cat.className = "task-edit task-edit-cat"; cat.value = t.category || ""; cat.placeholder = "category"; cat.spellcheck = false;
  card.classList.add("editing");
  if (titleEl) { titleEl.textContent = ""; titleEl.appendChild(input); titleEl.appendChild(cat); }
  input.focus(); input.select();
  let done = false;
  const finish = (commit) => {
    if (done) return; done = true;
    card.classList.remove("editing");
    if (commit) updateBoardTask(id, { title: input.value.trim(), category: cat.value.trim() });
    renderBoard(_lastQueue, _lastAll);
  };
  const onKey = (e) => { e.stopPropagation(); if (e.key === "Enter") { e.preventDefault(); finish(true); } else if (e.key === "Escape") { e.preventDefault(); finish(false); } };
  input.addEventListener("keydown", onKey); cat.addEventListener("keydown", onKey);
  const onBlur = () => setTimeout(() => { const a = document.activeElement; if (a !== input && a !== cat) finish(true); }, 0);
  input.addEventListener("blur", onBlur); cat.addEventListener("blur", onBlur);
  [input, cat].forEach((el) => { el.addEventListener("click", (e) => e.stopPropagation()); el.addEventListener("mousedown", (e) => e.stopPropagation()); });
}

// Handle a drop on a column: routes by card type.
//
// • Queue-row cards (pane ids): intra-column reorder = session-only override;
//   cross-column = NO-OP (Model A — the board reflects machine state, never sets it).
// • localStorage user-task cards (ids starting "t"): cross-column = localStorage update.
// • MCP task-model cards (text/at-mcp == "1"): cross-column = durable column move via
//   `update_task_kanban` / `create_task_kanban` (Tier B true-kanban, Phase-14-T2).
function boardMove(draggedId, sourceCol, targetCol, isMcp, overCardEl) {
  if (!draggedId || !targetCol) return;

  // MCP task card: durable cross-column move (Tier B kanban).
  if (isMcp) {
    if (sourceCol !== targetCol) {
      const mcpTask = _mcpTasks.find((t) => t && t.id === draggedId);
      if (mcpTask) {
        // Optimistic local update so the board reflects the new column immediately,
        // before the async IPC call completes (avoids a visible flash-back on fast polls).
        _operatorTasksById[draggedId] = {
          ...(_operatorTasksById[draggedId] || {}),
          id: draggedId,
          column: { "Backlog": "backlog", "Working": "doing", "Needs you": "review", "Done": "done" }[targetCol] || "backlog",
        };
        persistMcpTaskColumnMove(mcpTask, targetCol);
      }
      renderBoard(_lastQueue, _lastAll);
    }
    return;
  }

  // localStorage user task (id starts "t"): cross-column allowed → update status.
  if (draggedId.charAt(0) === "t" && findBoardTask(draggedId)) {
    if (sourceCol !== targetCol) { updateBoardTask(draggedId, { status: targetCol }); renderBoard(_lastQueue, _lastAll); }
    return;
  }

  // Queue-row card: intra-column reorder (session-only); cross-column = NO-OP (Model A).
  const beforeId = overCardEl && overCardEl.dataset.id !== draggedId ? overCardEl.dataset.id : null;
  const current = applyOrder(bucketByColumn(boardRows(_lastQueue, _lastAll, workspaces, paneOwner), BOARD_COLS)[targetCol], boardOrder[targetCol]);
  const next = boardReorder(current, sourceCol, targetCol, draggedId, beforeId);
  if (next == null) return; // cross-column drop: no-op (Model A)
  boardOrder[targetCol] = next;
  renderBoard(_lastQueue, _lastAll);
}

// ── board card drag (pointer-based) ───────────────────────────────────────────
// WKWebView intercepts native HTML5 drag-drop (see the pane-drag note ~L313), so the
// board uses the same mouse-tracked drag: mousedown on a card → 6px threshold → release
// over a column. One delegated listener on #board-overlay (onBoardCardDown) covers every
// card across keyed-reconcile rebuilds; id/col/mcp are read LIVE from the dataset.
let boardDrag = null; // { id, sourceCol, isMcp, sx, sy, active, card }
function boardColAtPoint(x, y) {
  const el = document.elementFromPoint(x, y);
  const c = el && el.closest ? el.closest(".board-col") : null;
  return c ? c.dataset.col : null;
}
function boardCardAtPoint(x, y) {
  const el = document.elementFromPoint(x, y);
  return el && el.closest ? el.closest(".board-card") : null;
}
function onBoardDragMove(e) {
  if (!boardDrag) return;
  if (!boardDrag.active) {
    if (Math.abs(e.clientX - boardDrag.sx) + Math.abs(e.clientY - boardDrag.sy) < 6) return; // a click, not a drag
    boardDrag.active = true;
    document.body.classList.add("board-dragging");
    if (boardDrag.card) boardDrag.card.classList.add("dragging");
  }
  const col = boardColAtPoint(e.clientX, e.clientY);
  document.querySelectorAll("#board-overlay .board-col.drag-over").forEach((c) => { if (c.dataset.col !== col) c.classList.remove("drag-over"); });
  if (col) {
    const el = document.elementFromPoint(e.clientX, e.clientY);
    const colEl = el && el.closest ? el.closest(".board-col") : null;
    if (colEl) colEl.classList.add("drag-over");
  }
}
function onBoardDragUp(e) {
  document.removeEventListener("mousemove", onBoardDragMove, true);
  document.removeEventListener("mouseup", onBoardDragUp, true);
  const drag = boardDrag; boardDrag = null;
  document.body.classList.remove("board-dragging");
  document.querySelectorAll("#board-overlay .board-col.drag-over").forEach((c) => c.classList.remove("drag-over"));
  if (drag && drag.card) drag.card.classList.remove("dragging");
  if (!drag || !drag.active) return; // a click, not a drag — let the card's onclick fire
  const targetCol = boardColAtPoint(e.clientX, e.clientY);
  if (!targetCol) return;
  boardMove(drag.id, drag.sourceCol, targetCol, drag.isMcp, boardCardAtPoint(e.clientX, e.clientY));
}
function onBoardCardDown(e) {
  if (e.button !== 0) return;
  const card = e.target.closest && e.target.closest(".board-card");
  if (!card) return;
  if (e.target.closest("button, input, .task-edit")) return; // delete / inline-edit, not a drag
  boardDrag = { id: card.dataset.id, sourceCol: card.dataset.col, isMcp: card.dataset.mcp === "1", sx: e.clientX, sy: e.clientY, active: false, card };
  document.addEventListener("mousemove", onBoardDragMove, true);
  document.addEventListener("mouseup", onBoardDragUp, true);
}

// Queue/Board segmented toggle in the rail head. Queue is the DEFAULT active state;
// "Board" opens the overlay, "Queue" closes it. ⌘⇧J is NOT shadowed (no keybind here).
function syncBoardToggle() {
  const q = document.getElementById("view-queue");
  const b = document.getElementById("view-board");
  if (q) { q.classList.toggle("active", !boardOpen); q.setAttribute("aria-pressed", String(!boardOpen)); }
  if (b) { b.classList.toggle("active", boardOpen); b.setAttribute("aria-pressed", String(boardOpen)); }
}

(function mountBoardToggle() {
  const rail = document.getElementById("rail");
  const queue = document.getElementById("queue");
  if (!rail || !queue) return;
  const strip = document.createElement("div");
  strip.className = "view-toggle-strip";
  const seg = document.createElement("span");
  seg.className = "view-toggle";
  seg.setAttribute("role", "group");
  seg.setAttribute("aria-label", "Queue or Board view");
  const q = document.createElement("button");
  q.id = "view-queue";
  q.className = "view-seg active";
  q.textContent = "Queue";
  q.title = "Ranked queue (default)";
  q.setAttribute("aria-pressed", "true");
  q.onclick = () => closeBoard();
  const b = document.createElement("button");
  b.id = "view-board";
  b.className = "view-seg";
  b.textContent = "Board";
  b.title = "Alt-view board — agents by state (read-only)";
  b.setAttribute("aria-pressed", "false");
  b.onclick = () => openBoard();
  seg.append(q, b);
  strip.appendChild(seg);
  // own row between the rail head and the queue list (not squeezed into rail-head).
  rail.insertBefore(strip, queue);
})();

// ──────────────────── Phase 11 (L6) memory-graph view — Second Brain ─────────────
//
// A third "Graph" segment in the Queue/Board toggle strip opens an overlay over
// #main rendering the memory note-link graph (the `memory_graph` Tauri command
// → core/memory's build_graph projection). Mirrors the Board overlay pattern: built
// once (lazily), toggled hidden, Queue stays the DEFAULT view (graph is opt-in).
// No longer read-only: the Second Brain editor (further below) layers create /
// edit / delete / connect flows over the same projection — every mutation goes
// through the memory_* Tauri commands and re-pulls the graph, so the canvas is
// always the store's truth, never an optimistic local copy.
//
// The projection is `{ nodes:[{id,title,degree,updated_at}], edges:[{from,to,kind}] }`
// where edge `kind` is "link" (hard backlink, drawn SOLID) or "suggested" (heuristic,
// drawn DASHED). An empty store returns `{nodes:[],edges:[]}` (NOT an error) → empty
// state. Layout math lives in the pure, tested graph-core.js (layoutGraph). Node titles
// are written with textContent only — never innerHTML.

let graphOpen = false;
let _graphLoading = false;

// Live handle (`{ dispose }` — nothing else) to the Three.js lightning renderer
// (memory-lightning.js) when the WebGL path is active; null while the SVG fallback
// (or no view) is showing. Resizes go through a full re-render, not the handle. ONE
// handle at a time — every re-render/hide disposes first, so a resize storm can
// never stack live WebGL contexts (same WKWebView context-budget concern as the
// terminal renderers above).
let lightningHandle = null;
// Monotonic render generation. renderGraph awaits a dynamic import, so a newer
// render can start while an older one is parked on the await; the stale render
// re-checks its generation after the await and backs off instead of attaching a
// context the newer render would never dispose (mirrors attachWebgl's re-checks).
let _graphRenderGen = 0;
// Warn ONCE (not per re-render) when the lightning module is missing or broken.
let _lightningWarned = false;

// ── Second Brain editor state ─────────────────────────────────────────────────
// The graph is an editor now, not just a viewer: clicking a node opens it in the
// right-side panel, double-clicking empty canvas creates, and a connect flow links
// two thoughts. State is module-level (like the board/diff overlays) because the
// panel element survives inside .graph-stage across renders.
let brainPanelOpen = false;
let _brainMode = null; // "create" | "edit" while the panel is open, else null
let _brainNote = null; // the FULL note (memory_get_note shape) backing EDIT mode — carries body/links
let _brainConnectMode = false; // armed by "Connect to another thought": the NEXT node click links, not opens
let _brainDeleteArmed = false; // two-click delete arm (window.confirm is a WKWebView no-op — same as ensureRepoArmed)
let _brainDeleteTimer = null;
// id → projection node from the last renderGraph — resolves link ids to titles in
// the connections list without a per-row fetch. Refreshed on EVERY render.
let _brainNodesById = new Map();
// Last known force-sim coordinates ({ [noteId]: {x,y} }), banked by disposeLightning
// and seeded into the next createLightningGraph — a save/reload keeps the
// constellation where the user left it instead of re-rolling the layout.
let _brainPositions = null;

// Tear down the live lightning renderer, if any. Called before every re-render,
// before the SVG fallback paints, and whenever the overlay hides — the WebGL
// context must die with the view, never linger behind a detached canvas.
function disposeLightning() {
  if (!lightningHandle) return;
  // Bank the sim's live coordinates BEFORE the teardown — nodes move continuously,
  // so this snapshot (not layoutGraph's fresh roll) is what the next render seeds
  // from. Guarded: the handle may predate the getPositions contract.
  try {
    if (typeof lightningHandle.getPositions === "function") {
      const pos = lightningHandle.getPositions();
      if (pos && typeof pos === "object") _brainPositions = pos;
    }
  } catch (_) {}
  try { lightningHandle.dispose(); } catch (_) {}
  lightningHandle = null;
  // The hover card belongs to the lightning view — never leave it showing after
  // the renderer dies (overlay close, re-render, SVG fallback all land here).
  hideMemoryCard();
}

const GRAPH_SVG_NS = "http://www.w3.org/2000/svg";

// Build the graph overlay once (absolute over #main; the Queue rail stays visible →
// Queue remains primary). Lazily created so it costs nothing until first opened.
// The head is the Second Brain toolbar (brand · live tally · legend · Add Thought ·
// close); the canvas sits inside .graph-stage so the editor panel can float over it
// WITHOUT living in #graph-body — every render replaceChildren()s the body, and the
// panel must survive re-renders with a half-typed note intact.
function ensureGraphOverlay() {
  let overlay = document.getElementById("graph-overlay");
  if (overlay) return overlay;
  overlay = document.createElement("div");
  overlay.id = "graph-overlay";
  overlay.className = "hidden";
  const head = document.createElement("div");
  head.className = "graph-head";
  const title = document.createElement("span");
  title.className = "graph-title";
  title.textContent = "🧠 Second Brain";
  // Live "N nodes · M links" tally — renderGraph rewrites it on every render
  // (including the empty state), so it always mirrors what's on the canvas.
  const count = document.createElement("span");
  count.className = "graph-count";
  count.id = "graph-count";
  count.textContent = "…";
  // Legend so the solid/dashed distinction is legible without hovering.
  const legend = document.createElement("span");
  legend.className = "graph-legend";
  const lLink = document.createElement("span");
  lLink.className = "graph-legend-item";
  const lLinkSwatch = document.createElement("i");
  lLinkSwatch.className = "graph-legend-swatch graph-legend-link";
  lLinkSwatch.setAttribute("aria-hidden", "true"); // decorative — the text label carries the meaning
  lLink.append(lLinkSwatch, document.createTextNode("link"));
  const lSug = document.createElement("span");
  lSug.className = "graph-legend-item";
  const lSugSwatch = document.createElement("i");
  lSugSwatch.className = "graph-legend-swatch graph-legend-suggested";
  lSugSwatch.setAttribute("aria-hidden", "true");
  lSug.append(lSugSwatch, document.createTextNode("suggested"));
  legend.append(lLink, lSug);
  // Primary create entry — the empty-canvas double-click is the power-user path;
  // this button is the discoverable one (and the ONLY one while the store is empty,
  // since an empty graph renders no canvas to double-click).
  const add = document.createElement("button");
  add.className = "graph-add";
  add.id = "graph-add";
  add.textContent = "+ Add Thought";
  add.setAttribute("aria-label", "Add a new thought");
  add.onclick = () => openBrainPanel("create");
  const close = document.createElement("button");
  close.className = "graph-close";
  close.title = "Back to Queue";
  close.setAttribute("aria-label", "Close graph");
  close.appendChild(svgIcon("i-x"));
  close.onclick = () => closeGraph();
  head.append(title, count, legend, add, close);
  const stage = document.createElement("div");
  stage.className = "graph-stage"; // relative box the editor panel floats over
  const body = document.createElement("div");
  body.className = "graph-body";
  body.id = "graph-body";
  stage.appendChild(body);
  overlay.append(head, stage);
  const main = document.getElementById("main");
  if (main) main.appendChild(overlay);
  return overlay;
}

function openGraph() {
  // One overlay shows at a time — opening Graph closes the Board (and vice versa).
  if (boardOpen) closeBoard();
  const overlay = ensureGraphOverlay();
  overlay.classList.remove("hidden");
  graphOpen = true;
  syncGraphToggle();
  // Land keyboard focus inside the overlay. Light dialog semantics — layered
  // Escape (connect flow → panel → overlay) instead of a full focus trap; the
  // editor panel moves focus to its title field when it opens.
  const close = overlay.querySelector(".graph-close");
  if (close) close.focus();
  loadGraph();
}
function closeGraph() {
  const o = document.getElementById("graph-overlay");
  // Capture BEFORE hiding — display:none drops focus to <body>, losing the signal.
  const focusWasInside = !!(o && o.contains(document.activeElement));
  if (o) o.classList.add("hidden");
  graphOpen = false;
  disposeLightning(); // every hide path (close button, Queue/Board toggles) lands here
  // Reset the editor with the overlay (connect mode, delete arm, note ref) so a
  // reopen starts clean. graphOpen is already false → closeBrainPanel skips its
  // focus hand-off, and the focusWasInside restore below still wins.
  if (brainPanelOpen) closeBrainPanel();
  syncGraphToggle();
  // Hand focus back to the Graph segment ONLY when the overlay held it (close button,
  // Escape) — never steal from a Queue/Board segment the user just clicked.
  if (focusWasInside) {
    const g = document.getElementById("view-graph");
    if (g) g.focus();
  }
}

// Queue / Graph segmented-toggle sync. Queue is active whenever NEITHER overlay is
// open; Graph is active only while the graph overlay is showing. (We do NOT touch the
// Board segment here — that is syncBoardToggle's job.)
function syncGraphToggle() {
  const g = document.getElementById("view-graph");
  if (g) { g.classList.toggle("active", graphOpen); g.setAttribute("aria-pressed", String(graphOpen)); }
  // Keep the Queue segment lit only when no overlay is open. Reading the board/graph
  // flags here (not writing board state) is safe and avoids a stale "Queue" highlight
  // when the graph is the active view.
  const q = document.getElementById("view-queue");
  if (q) { const queueActive = !boardOpen && !graphOpen; q.classList.toggle("active", queueActive); q.setAttribute("aria-pressed", String(queueActive)); }
}

// Pull the projection and (re)render. Tolerant of a missing Tauri API (dev webview)
// and of the empty-graph result, which is a valid non-error state. RETURNS the
// chain's promise (resolving after renderGraph completes) so mutation flows —
// save / connect / delete — can sequence "after the re-render lands" work, e.g.
// re-selecting the edited node's orb via setSelected.
function loadGraph() {
  const body = document.getElementById("graph-body");
  if (!body) return Promise.resolve();
  _graphLoading = true;
  disposeLightning(); // the loading message replaces the canvas — free its context too
  renderGraphMessage(body, "Loading graph…");
  return invoke("memory_graph")
    // renderGraph is async (lazy renderer import) — return its promise so an
    // unexpected rejection lands in the catch below, never as an unhandled one.
    .then((g) => { _graphLoading = false; if (graphOpen) return renderGraph(g || { nodes: [], edges: [] }); })
    .catch((e) => { _graphLoading = false; if (graphOpen) renderGraphMessage(body, "Graph unavailable — " + String(e && e.message ? e.message : e)); });
}

// Centered status / empty message inside the graph body (textContent only).
function renderGraphMessage(body, msg) {
  body.replaceChildren();
  // The body is a message region now, not a graph image — drop the stale name
  // renderGraph stamped; the wrap announces politely (loading / empty / error).
  body.removeAttribute("role");
  body.removeAttribute("aria-label");
  const wrap = document.createElement("div");
  wrap.className = "graph-empty";
  wrap.setAttribute("role", "status");
  wrap.textContent = msg;
  body.appendChild(wrap);
}

// Render the `{nodes,edges}` projection: the Three.js lightning view when WebGL is
// allowed and the renderer module loads, else the SVG node-link fallback (below,
// unchanged). Empty store → empty state (handled before layout). async only for
// the renderer's lazy dynamic import — the sole caller (loadGraph) chains it.
async function renderGraph(graph) {
  const body = document.getElementById("graph-body");
  if (!body) return;
  // Bump the generation FIRST — before the empty branch — so a stale in-flight
  // lightning render parked on its await can never clobber a newer empty state.
  const gen = ++_graphRenderGen;
  disposeLightning(); // never re-render over a live WebGL context
  // Toolbar tally + the id→title map the editor's connections list resolves
  // against — refreshed on EVERY render (including empty) so neither goes stale.
  const nodes = Array.isArray(graph.nodes) ? graph.nodes : [];
  const edges = Array.isArray(graph.edges) ? graph.edges : [];
  _brainNodesById = new Map(nodes.map((n) => [n.id, n]));
  const countEl = document.getElementById("graph-count");
  if (countEl) countEl.textContent = `${nodes.length} node${nodes.length === 1 ? "" : "s"} · ${edges.length} link${edges.length === 1 ? "" : "s"}`;
  if (isEmptyGraph(graph)) {
    renderGraphMessage(body, "No thoughts yet — press “+ Add Thought” to plant the first one.");
    return;
  }

  // Size the viewBox to the body; clamp so a single note still gets a sane canvas.
  const rect = body.getBoundingClientRect();
  const width = Math.max(360, Math.round(rect.width) || 800);
  const height = Math.max(280, Math.round(rect.height) || 600);
  const laid = layoutGraph(graph, { width, height, pad: 56 });

  const handled = await tryRenderLightning(body, laid, width, height, gen);
  // Superseded or closed mid-await — the newer render (or nobody) owns the body now.
  if (!graphOpen || gen !== _graphRenderGen) return;
  if (!handled) renderGraphSvg(body, laid, width, height);
  // Accessible name for the graph surface (WebGL and SVG paths alike): announce the
  // graph's size; refreshed on every render, cleared by renderGraphMessage.
  body.setAttribute("role", "img");
  body.setAttribute("aria-label", `Memory graph: ${graph.nodes.length} notes, ${graph.edges.length} links`);
}

// WebGL lightning branch. Dynamically imports the Three.js renderer (mirrors the
// lazy addon-webgl import in loadWebglCtor — the bundle is parsed only on first
// use; the SVG path never pays for it). Honors the SAME webglDisabled policy as
// the terminals (at_no_webgl kill-switch + the WebGL2-absent latch). Returns true
// when the caller must NOT paint the SVG fallback: either the lightning view took
// the body, or this render went stale mid-await (overlay closed / a newer render
// superseded it and now owns the body). Returns false → paint the SVG. Never
// throws — any failure warns once and falls back.
async function tryRenderLightning(body, layout, width, height, gen) {
  if (webglDisabled) return false;
  let mod;
  try {
    mod = await import("/memory-lightning.js");
  } catch (e) {
    warnLightningOnce(e);
    return false;
  }
  // The world may have moved during the await — re-check before touching the DOM.
  if (!graphOpen || gen !== _graphRenderGen) return true;
  try {
    body.replaceChildren(); // the renderer appends its own canvas + label layer
    const handle = mod.createLightningGraph({
      container: body, layout, width, height,
      onHover: onGraphNodeHover,
      // Second Brain editor wiring: node click → edit panel / empty-space click →
      // dismiss (or link, in connect mode); empty-canvas double-click → create.
      onSelect: onGraphNodeSelect,
      onCreateAt: onGraphCreateAt,
      // Seed the force sim with the previous run's banked coordinates (see
      // disposeLightning) so a save/reload keeps the constellation in place.
      seedPositions: _brainPositions,
    });
    if (!handle) return false; // renderer declined (e.g. no WebGL context) → SVG
    lightningHandle = handle;
    return true;
  } catch (e) {
    warnLightningOnce(e);
    disposeLightning(); // defensive: nothing should be live after a throw
    return false;
  }
}

function warnLightningOnce(e) {
  if (_lightningWarned) return;
  _lightningWarned = true;
  console.warn("memory-lightning unavailable — falling back to the SVG graph:", e);
}

// ── Memory hover card (lightning/WebGL path ONLY — the SVG fallback keeps its
// native <title> tooltips). ONE reusable element inside #graph-body, fed by the
// renderer's onHover(node, anchor) callback: `node` is the LAID node (carries the
// tags/snippet/origin/updated_at passthrough from layoutGraph), `anchor` is {x,y}
// in #graph-body-local px. Populated with textContent only — never innerHTML.
//
// Lifecycle: #graph-body is replaceChildren()'d on every re-render, which silently
// DETACHES the card — so ensureMemoryCard re-creates it whenever the module-level
// ref no longer sits inside the current body. Hiding goes through disposeLightning
// (every close/re-render/fallback path), so the card can never outlive its renderer.
let _memoryCard = null;

function ensureMemoryCard(body) {
  if (_memoryCard && _memoryCard.parentElement === body) return _memoryCard;
  const card = document.createElement("div");
  card.className = "memory-card";
  card.setAttribute("role", "tooltip");
  card.setAttribute("aria-hidden", "true");
  body.appendChild(card);
  _memoryCard = card;
  return card;
}

// Hide = visually fade (opacity via .show) + aria-hidden. The node stays in the DOM
// (pointer-events:none) so the next hover reuses it without a re-append.
function hideMemoryCard() {
  if (!_memoryCard) return;
  _memoryCard.classList.remove("show");
  _memoryCard.setAttribute("aria-hidden", "true");
}

// onHover callback handed to createLightningGraph. Fires with (node, anchor) on
// hover change and (null) on clear.
function onGraphNodeHover(node, anchor) {
  if (!node || !anchor) { hideMemoryCard(); return; }
  const body = document.getElementById("graph-body");
  if (!body) return;
  const card = ensureMemoryCard(body);
  card.replaceChildren();

  // Title row.
  const title = document.createElement("div");
  title.className = "memory-card-title";
  title.textContent = node.title;
  card.appendChild(title);

  // Tag chips (+ origin pill, when the note carries one).
  const tags = Array.isArray(node.tags) ? node.tags : [];
  if (tags.length || node.origin) {
    const row = document.createElement("div");
    row.className = "memory-card-tags";
    for (const t of tags) {
      const chip = document.createElement("span");
      chip.className = "memory-card-tag";
      chip.textContent = String(t);
      row.appendChild(chip);
    }
    if (node.origin) {
      const o = document.createElement("span");
      o.className = "memory-card-origin";
      o.textContent = String(node.origin);
      row.appendChild(o);
    }
    card.appendChild(row);
  }

  // Snippet paragraph — skipped entirely when empty.
  if (node.snippet) {
    const p = document.createElement("p");
    p.className = "memory-card-snippet";
    p.textContent = node.snippet;
    card.appendChild(p);
  }

  // Meta row: "N links · updated <relative>". dgRelTime (delegation-log-core) is the
  // repo's relative-time helper — "" for a missing timestamp drops the updated half.
  const meta = document.createElement("div");
  meta.className = "memory-card-meta";
  const deg = node.degree || 0;
  const rel = dgRelTime(node.updated_at);
  meta.textContent = `${deg} link${deg === 1 ? "" : "s"}` + (rel ? ` · updated ${rel}` : "");
  card.appendChild(meta);

  // Show FIRST (the hidden state is opacity-only, so offsetWidth/Height are live
  // either way), then measure the populated card and place it +14px right/down of
  // the anchor — flipping to the left/above when that side would overflow, and
  // clamping to the body box as the final guard (tiny bodies, huge cards).
  card.setAttribute("aria-hidden", "false");
  card.classList.add("show");
  const bw = body.clientWidth;
  const bh = body.clientHeight;
  const cw = card.offsetWidth;
  const ch = card.offsetHeight;
  const OFF = 14, PAD = 8;
  let x = anchor.x + OFF;
  let y = anchor.y + OFF;
  if (x + cw > bw - PAD) x = anchor.x - OFF - cw; // overflow right → flip left
  if (y + ch > bh - PAD) y = anchor.y - OFF - ch; // overflow bottom → flip above
  x = Math.max(PAD, Math.min(x, bw - cw - PAD));
  y = Math.max(PAD, Math.min(y, bh - ch - PAD));
  card.style.left = `${Math.round(x)}px`;
  card.style.top = `${Math.round(y)}px`;
}

// ──────────────────── Second Brain editor (graph mutations) ─────────────────────
//
// Right-side panel over the graph canvas: click a node to edit it, double-click
// empty canvas (or "+ Add Thought") to create, "Connect to another thought" arms a
// one-shot click-to-link flow. The panel lives in .graph-stage — NOT #graph-body,
// which every render replaceChildren()s away — so a half-typed note survives the
// re-render each mutation triggers. Populated with textContent only — never
// innerHTML. Save sends title/body/category ONLY; links belong exclusively to the
// connect/remove flows, so a stale form can never clobber a fresh link write.

// Build the panel once (lazily, like ensureGraphOverlay) and park it hidden.
function ensureBrainPanel() {
  let panel = document.getElementById("brain-panel");
  if (panel) return panel;
  const overlay = ensureGraphOverlay();
  const stage = overlay.querySelector(".graph-stage");
  panel = document.createElement("div");
  panel.id = "brain-panel";
  panel.className = "brain-panel hidden";
  panel.setAttribute("role", "dialog");
  panel.setAttribute("aria-label", "Thought editor");

  const head = document.createElement("div");
  head.className = "brain-panel-head";
  const title = document.createElement("span");
  title.id = "brain-panel-title";
  title.textContent = "Edit Thought";
  const close = document.createElement("button");
  close.className = "brain-panel-close";
  close.title = "Close editor";
  close.setAttribute("aria-label", "Close thought editor");
  close.appendChild(svgIcon("i-x"));
  close.onclick = () => closeBrainPanel();
  head.append(title, close);

  const bodyWrap = document.createElement("div");
  bodyWrap.className = "brain-panel-body";

  const titleLabel = document.createElement("label");
  titleLabel.className = "brain-label";
  titleLabel.htmlFor = "brain-title";
  titleLabel.textContent = "Title";
  const titleInput = document.createElement("input");
  titleInput.type = "text";
  titleInput.id = "brain-title";
  titleInput.placeholder = "What's the thought?";

  // One option per CATEGORY_META entry (the SSOT shared with the renderer's orb
  // palette — lightning-core.js) plus a leading "uncategorized" blank. Option text
  // is data-driven but written via textContent, same as everything else here.
  const catLabel = document.createElement("label");
  catLabel.className = "brain-label";
  catLabel.htmlFor = "brain-category";
  catLabel.textContent = "Category";
  const catSel = document.createElement("select");
  catSel.id = "brain-category";
  const none = document.createElement("option");
  none.value = "";
  none.textContent = "— uncategorized —";
  catSel.appendChild(none);
  for (const c of CATEGORY_META) {
    const opt = document.createElement("option");
    opt.value = c.key;
    opt.textContent = `${c.emoji} ${c.label}`;
    catSel.appendChild(opt);
  }

  const bodyLabel = document.createElement("label");
  bodyLabel.className = "brain-label";
  bodyLabel.htmlFor = "brain-content";
  bodyLabel.textContent = "Content";
  const bodyTa = document.createElement("textarea");
  bodyTa.id = "brain-content";
  bodyTa.placeholder = "Write the note body…";

  // Connections section — EDIT mode only (a new note has no id to link from yet);
  // openBrainPanel toggles it via the global .hidden utility.
  const connWrap = document.createElement("div");
  connWrap.id = "brain-connections";
  connWrap.className = "brain-connections";
  const connLabel = document.createElement("div");
  connLabel.className = "brain-label";
  connLabel.textContent = "Connections";
  const links = document.createElement("div");
  links.id = "brain-links";
  const connectBtn = document.createElement("button");
  connectBtn.id = "brain-connect";
  connectBtn.className = "brain-connect";
  connectBtn.textContent = "🔗 Connect to another thought";
  connectBtn.onclick = () => brainSetConnectMode(!_brainConnectMode);
  const hint = document.createElement("div");
  hint.id = "brain-connect-hint";
  hint.className = "brain-connect-hint hidden";
  hint.setAttribute("role", "status"); // announce the armed flow to AT, politely
  hint.textContent = "Click a node in the graph to link it.";
  connWrap.append(connLabel, links, connectBtn, hint);

  // Inline error line (mirrors #dl-error) — mutation failures land here, in the
  // panel the user is looking at, not in a toast behind it.
  const err = document.createElement("div");
  err.id = "brain-error";
  err.className = "brain-error";
  err.setAttribute("role", "alert");

  bodyWrap.append(titleLabel, titleInput, catLabel, catSel, bodyLabel, bodyTa, connWrap, err);

  const foot = document.createElement("div");
  foot.className = "brain-panel-actions";
  const del = document.createElement("button");
  del.id = "brain-delete";
  del.className = "brain-delete";
  del.textContent = "Delete";
  del.setAttribute("aria-label", "Delete this thought");
  del.onclick = () => brainDelete();
  const save = document.createElement("button");
  save.id = "brain-save";
  save.className = "brain-save";
  save.textContent = "Save";
  save.setAttribute("aria-label", "Save thought");
  save.onclick = () => brainSave();
  foot.append(del, save);

  panel.append(head, bodyWrap, foot);
  if (stage) stage.appendChild(panel);
  return panel;
}

// Open the panel in "create" (blank fields) or "edit" (populated from the FULL
// note — memory_get_note's shape, incl. body/links/category). Re-entrant: opening
// over an already-open panel just repopulates it (clicking node B while editing
// node A switches to B; unsaved edits to A are dropped, same as empty-space click).
function openBrainPanel(mode, note) {
  const panel = ensureBrainPanel();
  brainSetConnectMode(false);
  brainDisarmDelete();
  _brainMode = mode;
  _brainNote = mode === "edit" && note ? note : null;
  brainPanelOpen = true;
  const err = document.getElementById("brain-error");
  if (err) err.textContent = "";
  const head = document.getElementById("brain-panel-title");
  if (head) head.textContent = mode === "edit" ? "Edit Thought" : "New Thought";
  const titleInput = document.getElementById("brain-title");
  const catSel = document.getElementById("brain-category");
  const bodyTa = document.getElementById("brain-content");
  if (titleInput) titleInput.value = _brainNote ? String(_brainNote.title || "") : "";
  if (catSel) {
    // Unknown/legacy category values fall back to "uncategorized" — assigning a
    // select a value with no matching option silently no-ops (stale selection).
    const cat = _brainNote && typeof _brainNote.category === "string" ? _brainNote.category : "";
    catSel.value = CATEGORY_META.some((c) => c.key === cat) ? cat : "";
  }
  if (bodyTa) bodyTa.value = _brainNote ? String(_brainNote.body || "") : "";
  const conn = document.getElementById("brain-connections");
  if (conn) conn.classList.toggle("hidden", mode !== "edit");
  const del = document.getElementById("brain-delete");
  if (del) del.classList.toggle("hidden", mode !== "edit");
  if (mode === "edit") renderBrainLinks();
  panel.classList.remove("hidden");
  if (titleInput) titleInput.focus(); // dialog focus lands on the first field
}

// Close + fully reset the editor (connect mode, delete arm, note ref). Hands focus
// back to the toolbar's Add button ONLY while the overlay itself stays open —
// closeGraph flips graphOpen off before calling here, so an overlay close never
// has its focus stolen back into a hidden toolbar.
function closeBrainPanel() {
  brainSetConnectMode(false);
  brainDisarmDelete();
  const panel = document.getElementById("brain-panel");
  const hadFocus = !!(panel && panel.contains(document.activeElement));
  if (panel) panel.classList.add("hidden");
  brainPanelOpen = false;
  _brainMode = null;
  _brainNote = null;
  brainSetSelected(null); // the renderer's selection highlight dies with the panel
  if (hadFocus && graphOpen) {
    const add = document.getElementById("graph-add");
    if (add) add.focus();
  }
}

// Best-effort renderer selection sync (the visual ring on the selected orb). The
// handle is null on the SVG fallback and setSelected is part of the newer renderer
// contract — guard both, never throw.
function brainSetSelected(id) {
  if (lightningHandle && typeof lightningHandle.setSelected === "function") {
    try { lightningHandle.setSelected(id); } catch (_) {}
  }
}

// CONNECT MODE: while armed, the next node click links source→target instead of
// opening the target; the panel stays on the source note the whole time. Exits on
// link-add, Escape, panel close, or clicking the (now "Cancel") button again.
function brainSetConnectMode(on) {
  _brainConnectMode = !!on;
  const btn = document.getElementById("brain-connect");
  const hint = document.getElementById("brain-connect-hint");
  if (btn) {
    btn.classList.toggle("active", _brainConnectMode);
    btn.textContent = _brainConnectMode ? "Cancel connecting" : "🔗 Connect to another thought";
  }
  if (hint) hint.classList.toggle("hidden", !_brainConnectMode);
}

// Rebuild the outbound-links list from the backing note. Titles resolve through
// the last projection's node map (no per-row fetch); an id missing from the map
// (e.g. a link to a since-deleted note) falls back to the raw id.
function renderBrainLinks() {
  const list = document.getElementById("brain-links");
  if (!list) return;
  list.replaceChildren();
  const links = _brainNote && Array.isArray(_brainNote.links) ? _brainNote.links : [];
  if (!links.length) {
    const empty = document.createElement("div");
    empty.className = "brain-links-empty";
    empty.textContent = "No connections yet.";
    list.appendChild(empty);
    return;
  }
  for (const id of links) {
    const row = document.createElement("div");
    row.className = "brain-link-row";
    const t = document.createElement("span");
    t.className = "brain-link-title";
    const node = _brainNodesById.get(id);
    const label = node && node.title ? String(node.title) : String(id);
    t.textContent = label;
    t.title = label; // rows ellipsize — the full title survives on hover
    const rm = document.createElement("button");
    rm.className = "brain-link-remove";
    rm.setAttribute("aria-label", `Remove link to ${label}`);
    rm.appendChild(svgIcon("i-x"));
    rm.onclick = () => brainRemoveLink(id);
    row.append(t, rm);
    list.appendChild(row);
  }
}

// Drop ONE outbound link (the row's ✕).
async function brainRemoveLink(linkId) {
  if (!_brainNote || !hasTauri()) return;
  const links = (Array.isArray(_brainNote.links) ? _brainNote.links : []).filter((l) => l !== linkId);
  await brainWriteLinks(links);
}

// CONNECT MODE tail: append the clicked target to the source's links. Dedupe +
// no self-link; a self-click keeps the mode armed (mis-click, pick again), an
// already-linked target just disarms (the edge already exists).
async function brainAddLink(targetId) {
  if (!_brainNote || !hasTauri()) { brainSetConnectMode(false); return; }
  if (targetId === _brainNote.id) return;
  const links = Array.isArray(_brainNote.links) ? _brainNote.links.slice() : [];
  if (links.includes(targetId)) { brainSetConnectMode(false); return; }
  links.push(targetId);
  brainSetConnectMode(false);
  await brainWriteLinks(links);
}

// Shared links write: persist (links ONLY — title/body/category omitted mean
// "leave unchanged"), resync the backing note, re-render the list, re-pull the
// graph so the edge change shows, and re-select the source orb after the render.
async function brainWriteLinks(links) {
  const err = document.getElementById("brain-error");
  if (err) err.textContent = "";
  const id = _brainNote.id;
  try {
    const updated = await invoke("memory_update_note", { id, links });
    if (updated) _brainNote = updated; else _brainNote.links = links;
    renderBrainLinks();
    await loadGraph();
    brainSetSelected(id);
  } catch (e) {
    if (err) err.textContent = "Couldn't update links — " + String(e && e.message ? e.message : e);
  }
}

// Save = CREATE (memory_create_note) or EDIT (memory_update_note; title/body/
// category only — links are the connect/remove flows' job). A create flips the
// panel into EDIT mode on the fresh note and selects its orb once the re-render
// lands, so "add → connect" is one continuous flow with no reopen between.
async function brainSave() {
  const err = document.getElementById("brain-error");
  if (err) err.textContent = "";
  if (!hasTauri()) { if (err) err.textContent = "Tauri API unavailable."; return; }
  const titleInput = document.getElementById("brain-title");
  const title = ((titleInput && titleInput.value) || "").trim();
  if (!title) {
    if (err) err.textContent = "Give the thought a title first.";
    if (titleInput) titleInput.focus();
    return;
  }
  const body = document.getElementById("brain-content")?.value || "";
  // "" (uncategorized) is sent AS-IS on edit — null means "leave unchanged" to
  // memory_update_note, so the empty string is the only way to CLEAR a category.
  const catVal = document.getElementById("brain-category")?.value || "";
  const save = document.getElementById("brain-save");
  if (save) { save.disabled = true; save.textContent = "Saving…"; }
  try {
    if (_brainMode === "create") {
      const note = await invoke("memory_create_note", { title, body, category: catVal || null, tags: null, links: null });
      _brainNote = note;
      _brainMode = "edit";
      const head = document.getElementById("brain-panel-title");
      if (head) head.textContent = "Edit Thought";
      const conn = document.getElementById("brain-connections");
      if (conn) conn.classList.remove("hidden");
      const del = document.getElementById("brain-delete");
      if (del) del.classList.remove("hidden");
      renderBrainLinks();
      await loadGraph();
      if (note && note.id) brainSetSelected(note.id);
    } else if (_brainNote) {
      const updated = await invoke("memory_update_note", { id: _brainNote.id, title, body, category: catVal });
      if (updated) _brainNote = updated;
      await loadGraph();
      brainSetSelected(_brainNote.id);
    }
  } catch (e) {
    if (err) err.textContent = "Couldn't save — " + String(e && e.message ? e.message : e);
  } finally {
    if (save) { save.disabled = false; save.textContent = "Save"; }
  }
}

// Two-click delete arm (window.confirm is a WKWebView no-op — same pattern as
// ensureRepoArmed / the spawn force path): the first click arms for 6s and
// relabels the button; a second click inside the window deletes for real.
function brainDisarmDelete() {
  _brainDeleteArmed = false;
  if (_brainDeleteTimer) { clearTimeout(_brainDeleteTimer); _brainDeleteTimer = null; }
  const btn = document.getElementById("brain-delete");
  if (btn) { btn.classList.remove("armed"); btn.textContent = "Delete"; }
}

async function brainDelete() {
  if (!_brainNote || !hasTauri()) return;
  const btn = document.getElementById("brain-delete");
  if (!_brainDeleteArmed) {
    _brainDeleteArmed = true;
    if (btn) { btn.classList.add("armed"); btn.textContent = "Click again to delete"; }
    _brainDeleteTimer = setTimeout(() => brainDisarmDelete(), 6000);
    return;
  }
  const id = _brainNote.id;
  brainDisarmDelete();
  const err = document.getElementById("brain-error");
  try {
    await invoke("memory_delete_note", { id });
    closeBrainPanel(); // the note is gone — nothing left to edit
    await loadGraph();
  } catch (e) {
    if (err) err.textContent = "Couldn't delete — " + String(e && e.message ? e.message : e);
  }
}

// onSelect callback handed to createLightningGraph: `node` is the LAID node
// (projection fields + x/y), null = empty-space click. Routing: connect mode
// consumes node clicks as link targets (empty space never cancels it — Escape
// does); otherwise a node opens the editor and empty space dismisses it.
function onGraphNodeSelect(node) {
  if (_brainConnectMode) {
    if (!node) return;
    brainAddLink(node.id);
    return;
  }
  if (!node) { if (brainPanelOpen) closeBrainPanel(); return; }
  brainOpenNote(node.id);
}

// Laid nodes carry only the projection fields — fetch the FULL note (body, links,
// category) before editing. No generation token: last click wins is correct here,
// and openBrainPanel is re-entrant.
async function brainOpenNote(id) {
  if (!hasTauri()) return;
  try {
    const note = await invoke("memory_get_note", { id });
    if (!graphOpen) return; // overlay closed mid-fetch — don't resurrect the panel
    if (!note) { showToast("That thought no longer exists."); return; }
    openBrainPanel("edit", note);
  } catch (e) {
    showToast("Couldn't load note — " + String(e && e.message ? e.message : e));
  }
}

// onCreateAt callback (empty-canvas double-click). The click point is NOT
// persisted — the force sim owns positions — so it just means "new thought".
function onGraphCreateAt() {
  openBrainPanel("create");
}

// SVG node-link fallback. Edges first (under the nodes), then node dots + labels.
// Solid for "link", dashed for the "suggested" heuristic.
function renderGraphSvg(body, laid, width, height) {
  const svg = document.createElementNS(GRAPH_SVG_NS, "svg");
  svg.setAttribute("class", "graph-svg");
  svg.setAttribute("width", String(width));
  svg.setAttribute("height", String(height));
  svg.setAttribute("viewBox", `0 0 ${width} ${height}`);
  svg.setAttribute("preserveAspectRatio", "xMidYMid meet");

  // Edge layer.
  const edgeLayer = document.createElementNS(GRAPH_SVG_NS, "g");
  edgeLayer.setAttribute("class", "graph-edges");
  for (const e of laid.edges) {
    const line = document.createElementNS(GRAPH_SVG_NS, "line");
    line.setAttribute("x1", String(e.x1));
    line.setAttribute("y1", String(e.y1));
    line.setAttribute("x2", String(e.x2));
    line.setAttribute("y2", String(e.y2));
    line.setAttribute("class", "graph-edge " + (e.suggested ? "graph-edge-suggested" : "graph-edge-link"));
    edgeLayer.appendChild(line);
  }
  svg.appendChild(edgeLayer);

  // Node layer (dots + labels). Label is the note title via textContent (never
  // innerHTML); dot radius scales gently with degree so hubs read as larger.
  const nodeLayer = document.createElementNS(GRAPH_SVG_NS, "g");
  nodeLayer.setAttribute("class", "graph-nodes");
  for (const nd of laid.nodes) {
    const grp = document.createElementNS(GRAPH_SVG_NS, "g");
    grp.setAttribute("class", "graph-node");
    grp.setAttribute("transform", `translate(${nd.x},${nd.y})`);

    const r = 4 + Math.min(8, nd.degree);
    const dot = document.createElementNS(GRAPH_SVG_NS, "circle");
    dot.setAttribute("r", String(r));
    dot.setAttribute("class", "graph-node-dot");
    grp.appendChild(dot);

    const label = document.createElementNS(GRAPH_SVG_NS, "text");
    label.setAttribute("class", "graph-node-label");
    label.setAttribute("x", String(r + 4));
    label.setAttribute("y", "4");
    label.textContent = nd.label || nd.title; // one-word display label; textContent only — never innerHTML
    grp.appendChild(label);

    // Native tooltip with the full title (helps when labels overlap on a busy ring).
    const tip = document.createElementNS(GRAPH_SVG_NS, "title");
    tip.textContent = nd.title;
    grp.appendChild(tip);

    nodeLayer.appendChild(grp);
  }
  svg.appendChild(nodeLayer);

  body.replaceChildren(svg);
}

// Append a "Graph" segment to the existing Queue/Board toggle strip (additive; does
// not modify the board's own segments or syncBoardToggle).
(function mountGraphToggle() {
  const seg = document.querySelector(".view-toggle");
  if (!seg) return;
  if (document.getElementById("view-graph")) return;
  // The group grows a third segment here — keep its accessible name truthful.
  seg.setAttribute("aria-label", "Queue, Board, or Graph view");
  const g = document.createElement("button");
  g.id = "view-graph";
  g.className = "view-seg";
  g.textContent = "Graph";
  g.title = "Second Brain — memory graph (edit, connect, create)";
  g.setAttribute("aria-pressed", "false");
  g.onclick = () => { if (graphOpen) closeGraph(); else openGraph(); };
  seg.appendChild(g);
  // Switching to Queue or Board must also close the Graph overlay (one view at a
  // time). We ATTACH a listener (additive) rather than editing the board's handlers.
  const q = document.getElementById("view-queue");
  const b = document.getElementById("view-board");
  if (q) q.addEventListener("click", () => { if (graphOpen) closeGraph(); });
  if (b) b.addEventListener("click", () => { if (graphOpen) closeGraph(); });
})();

// Re-render the open graph on window resize so the ring re-fits the body. Debounced
// via rAF so a resize-drag does not thrash. No-op while closed.
let _graphResizeRaf = 0;
window.addEventListener("resize", () => {
  if (!graphOpen || _graphLoading) return;
  if (_graphResizeRaf) cancelAnimationFrame(_graphResizeRaf);
  _graphResizeRaf = requestAnimationFrame(() => {
    _graphResizeRaf = 0;
    // Re-pull is cheap and keeps the view fresh; the command re-projects on each call.
    loadGraph();
  });
});

// ──────────────────── 09-04 Slice 1 read-only diff viewer (AC-5) ─────────────────
//
// A "Diff" button in the #main topbar opens a read-only unified-diff overlay for the
// ACTIVE pane, via the three read-only Tauri commands (diff_changed_files /
// diff_unified / diff_read_file). Display only — no editor, no save, no write path.

// diffLineClass + renderUnifiedDiff (unified-diff → DOM builders) live in
// ./diff-view-core.js (imported at top) so they can be DOM-tested in isolation.

function ensureDiffOverlay() {
  let overlay = document.getElementById("diff-overlay");
  if (overlay) return overlay;
  overlay = document.createElement("div");
  overlay.id = "diff-overlay";
  overlay.className = "hidden";
  const head = document.createElement("div");
  head.className = "diff-head";
  const title = document.createElement("span");
  title.className = "diff-title";
  title.id = "diff-title";
  title.textContent = "Diff";
  const close = document.createElement("button");
  close.className = "diff-close";
  close.title = "Close diff";
  close.setAttribute("aria-label", "Close diff");
  close.appendChild(svgIcon("i-x"));
  close.onclick = () => closeDiff();
  head.append(title, close);
  const files = document.createElement("div");
  files.className = "diff-files";
  files.id = "diff-files";
  const body = document.createElement("div");
  body.className = "diff-scroll";
  body.id = "diff-scroll";
  overlay.append(head, files, body);
  const main = document.getElementById("main");
  if (main) main.appendChild(overlay);
  return overlay;
}

// Current diff session: the pane id + the unified-diff text, so the file strip can
// toggle between the unified diff and a single file's full contents (the latter via
// the guarded diff_read_file — which also makes UNTRACKED new files, absent from
// `git diff`, viewable in full).
let _diffSession = { id: null, diffText: "" };

// Generation token for openDiff: switching panes and re-opening while a slow
// diff_changed_files/diff_unified pair is still in flight used to let the STALE
// reply land last and paint pane A's diff under pane B's title. Each open bumps
// the token; a reply whose token no longer matches is discarded.
let _diffGen = 0;

async function openDiff() {
  if (!hasTauri()) { showToast("Tauri API unavailable"); return; }
  const id = activeId;
  if (!id) { showToast("No active session — select an agent first."); return; }
  const gen = ++_diffGen;
  const overlay = ensureDiffOverlay();
  document.getElementById("diff-title").textContent = `Diff · ${id}`;
  const filesEl = document.getElementById("diff-files");
  const scrollEl = document.getElementById("diff-scroll");
  filesEl.replaceChildren();
  scrollEl.replaceChildren();
  overlay.classList.remove("hidden");
  try {
    const [files, diff] = await Promise.all([
      invoke("diff_changed_files", { id }),
      invoke("diff_unified", { id }),
    ]);
    if (gen !== _diffGen) return; // a newer openDiff superseded this reply — discard it
    _diffSession = { id, diffText: diff };
    // "Diff" reset chip → the unified diff (default view); file chips → that file's
    // full contents. Untracked (`?`) files aren't in `git diff`, so viewing them is
    // the only way to see their content — the guarded read makes that safe.
    const diffChip = document.createElement("button");
    diffChip.className = "diff-file-chip diff-reset active";
    diffChip.textContent = "Unified diff";
    diffChip.onclick = () => { selectChip(diffChip); showUnifiedDiff(); };
    filesEl.appendChild(diffChip);
    const count = document.createElement("span");
    count.className = "diff-files-count";
    count.textContent = files.length === 0 ? "no changed files" : `${files.length} changed · click to view`;
    filesEl.appendChild(count);
    for (const f of files) {
      const chip = document.createElement("button");
      chip.className = "diff-file-chip";
      const untracked = (f.status || "").includes("?");
      const st = document.createElement("span");
      st.className = "diff-file-st st-" + (f.status || "").replace(/[^A-Za-z?]/g, "").toLowerCase();
      st.textContent = f.status || "·";
      const nm = document.createElement("span");
      nm.className = "diff-file-name";
      nm.textContent = f.path;
      chip.append(st, nm);
      chip.onclick = () => { selectChip(chip); showFileContents(f.path, untracked); };
      filesEl.appendChild(chip);
    }
    showUnifiedDiff();
  } catch (e) {
    if (gen !== _diffGen) return; // stale failure — a newer open owns the overlay now
    const err = document.createElement("div");
    err.className = "diff-empty";
    err.textContent = "Diff unavailable: " + String(e);
    scrollEl.appendChild(err);
  }
}

// single-select the active chip in the file strip
function selectChip(active) {
  const filesEl = document.getElementById("diff-files");
  if (!filesEl) return;
  for (const c of filesEl.querySelectorAll(".diff-file-chip")) c.classList.toggle("active", c === active);
}

function showUnifiedDiff() {
  const scrollEl = document.getElementById("diff-scroll");
  scrollEl.replaceChildren(renderUnifiedDiff(_diffSession.diffText));
}

// Read ONE file's full contents (read-only, containment-guarded) and show it — for an
// untracked file the whole thing is new, so render it added; otherwise as context.
async function showFileContents(relPath, untracked) {
  const scrollEl = document.getElementById("diff-scroll");
  scrollEl.replaceChildren();
  try {
    const text = await invoke("diff_read_file", { id: _diffSession.id, relPath });
    const body = document.createElement("div");
    body.className = "diff-body";
    const head = document.createElement("div");
    head.className = "diff-line diff-meta";
    head.textContent = `▸ ${relPath}${untracked ? "  (untracked — full file)" : "  (full file)"}`;
    body.appendChild(head);
    const cls = untracked ? "diff-added" : "diff-context";
    for (const line of String(text).split("\n")) {
      const row = document.createElement("div");
      row.className = "diff-line " + cls;
      row.textContent = (untracked ? "+" : " ") + (line === "" ? "" : line);
      body.appendChild(row);
    }
    scrollEl.appendChild(body);
  } catch (e) {
    const err = document.createElement("div");
    err.className = "diff-empty";
    err.textContent = "Couldn't read file: " + String(e);
    scrollEl.appendChild(err);
  }
}
function closeDiff() {
  const o = document.getElementById("diff-overlay");
  if (o) o.classList.add("hidden");
}

// Diff is a PANE-scoped inspector → it now lives in the pane kebab menu (openPaneMenu),
// not the topbar (06-18 topbar dedupe). openDiff() is unchanged + still reachable.

// ---- Scheduler (Plan 05-02, D33): proactive admission-cap surface ----
// A SEPARATE rail section from "who needs you": queued spawns don't need the human,
// they need a free working slot. The cap (max_concurrent) is the user's rate-limit
// guardrail (pain #4); the backend gates SPAWNS against the live `working` count.
// Default cap MIRRORS the backend's clamp_default_cap (cores-1, floor 3, ceiling 8 —
// keep the two formulas in sync). The old hardcoded 3 was pushed blindly at startup,
// clobbering the backend's core-scaled default every launch → a normal 3-pane
// workspace left ZERO delegate budget ("no free capacity") on a 12-core machine.
function defaultCap() {
  const cores = (typeof navigator !== "undefined" && navigator.hardwareConcurrency) || 4;
  return Math.min(8, Math.max(3, cores - 1));
}

function schedMax() {
  // at_max_concurrent2: v2 key — the legacy key persisted the old hardcoded 3 for
  // every user, indistinguishable from a deliberate choice; ignoring it re-defaults
  // everyone to the core-scaled cap, and only an explicit stepper change persists.
  const v = parseInt(localStorage.getItem(LS_KEYS.maxConcurrent), 10);
  return Number.isFinite(v) && v >= 1 ? v : defaultCap();
}

// Persist + push the cap to the backend, then re-render. Clamp ≥1 (mirrors the
// backend clamp). `surface` toasts a backend rejection — but only on a USER action
// (the ± stepper), never the silent startup push, so a failed cap change can't leave
// the label silently disagreeing with the backend's actual admission gate. The toast
// is rare: set_max_concurrent (lib.rs) returns the clamped value and only rejects on
// an IPC-layer error.
function setSchedMax(n, surface = false) {
  const v = Math.max(1, n | 0);
  const prev = localStorage.getItem(LS_KEYS.maxConcurrent); // for rollback on backend reject
  localStorage.setItem(LS_KEYS.maxConcurrent, String(v));
  if (hasTauri()) {
    invoke("set_max_concurrent", { n: v }).catch((e) => {
      // ROLL BACK the optimistic persist: a rejected push must not leave the stored cap
      // (and the Scheduler label derived from it) disagreeing with the backend's actual
      // admission gate across this session AND every future launch.
      try {
        if (prev == null) localStorage.removeItem(LS_KEYS.maxConcurrent);
        else localStorage.setItem(LS_KEYS.maxConcurrent, prev);
      } catch (_) {}
      renderScheduler();
      if (surface) showToast("Couldn't set concurrency cap: " + String(e));
    });
  }
  renderScheduler();
}

// compact "12s" / "4m" / "2h" since enqueue (re-evaluated every 1s poll tick)
function elapsedLabel(ts) {
  const s = Math.max(0, Math.floor((Date.now() - ts) / 1000));
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  return `${Math.floor(s / 3600)}h`;
}

// Force-admit a queued pane immediately, bypassing the cap. The actual queued→live
// transition still arrives via the `workspace-admitted` event (single code path).
function runNow(id) {
  if (!hasTauri()) { showToast("Tauri API unavailable"); return; }
  invoke("run_now", { id }).catch((e) => showToast("run now failed: " + String(e)));
}

// Deferred initial-prompt injection: give the freshly-spawned harness TUI a beat to
// boot before typing the prompt, then submit via sendTaskSubmit (paste-mode-safe: text,
// a 350ms settle, then a separate Enter — a raw trailing "\n" can ride inside a paste
// and sit unsubmitted). A pane that died before the timer fires is marked dead (D30).
const PROMPT_INJECT_DELAY_MS = 900;
function sendDeferredPrompt(id, prompt) {
  setTimeout(async () => {
    // Memory prime (backend gate `memory_autoconsult`, default OFF → "" → byte-identical):
    // on the spawn path the deferred prompt IS the raw user-typed task (no protocol
    // wrapper), so prime on it directly — the same treatment doDispatchBridge / the
    // verify wave give their tasks. Primed HERE (the send choke point, not spawnPane)
    // so a queued spawn admitted later (admitPending → rec.prompt) recalls against the
    // store AS OF dispatch time, with the gate read fresh then. primeTask never throws.
    sendTaskSubmit(id, (await primeTask(prompt)) + prompt).catch((e) => { if (DEAD_RE.test(String(e))) noteDeadPanes([id]); });
  }, PROMPT_INJECT_DELAY_MS);
}

// Adopt an ownerless-but-live pane into a visible "Recovered" workspace (create-or-reuse).
// A pane in NO workspace is an invisible zombie: renderRail drops orphan rows (paneOwner
// null) and relayout never shows a pane outside ws.paneIds — so a bare ensureSession kept
// the PTY alive but gave the operator NO surface that showed it.
function adoptOrphanPane(id) {
  let wsId = Object.keys(workspaces).find((w) => workspaces[w].name === "Recovered" && !workspaces[w].dormant);
  if (!wsId) {
    wsId = newWsId();
    workspaces[wsId] = { name: "Recovered", color: WS_PALETTE[Object.keys(workspaces).length % WS_PALETTE.length], repo: "", harness: "claude", paneIds: [], sessionIds: [], harnesses: [], roles: [], models: [], counter: 0, count: 0, dormant: false };
  }
  ensureSession(id);
  const ws = workspaces[wsId];
  if (!ws.paneIds.includes(id)) ws.paneIds.push(id);
  ws.count = ws.paneIds.length;
  persistWorkspaces();
  renderWorkspaces();
  if (activeWs === wsId) relayout();
  return wsId;
}

// `workspace-admitted` (D33): the backend created the PTY for a previously queued
// spawn (a working slot freed, or "run now"). Transition queued → live: attach the
// terminal, fold the pane into its workspace, and send any deferred initial prompt.
function admitPending(id, harness) {
  const rec = pending[id];
  delete pending[id];
  persistPending(); // durable BEFORE any reload, so a reconcile can't re-admit + double-send
  if (!rec) {
    // reload-desync (D33 risk): the backend admitted a queued spawn but our
    // transient `pending` map was wiped by a webview reload, so we have no
    // record (its wsId/deferred-prompt are unrecoverable here). Adopt the now-live
    // PTY into the visible "Recovered" workspace — a bare ensureSession would leave
    // it an invisible zombie (renderRail filters orphan panes).
    adoptOrphanPane(id);
    renderScheduler();
    return;
  }
  const ws = workspaces[rec.wsId];
  if (ws) {
    ensureSession(id);
    if (!ws.paneIds.includes(id)) ws.paneIds.push(id);
    ws.count = ws.paneIds.length;
    ws.dormant = false;
    if (rec.prompt) sendDeferredPrompt(id, rec.prompt);
    // honor a queued split's intended placement (splitPane stashed it): place this pane
    // beside its anchor BEFORE relayout's reconcile runs, so reconcile keeps it there.
    if (rec.splitHint && ws.layout && ltree.hasPane(ws.layout, rec.splitHint.anchor)) {
      ws.layout = ltree.removeLeaf(ws.layout, id);
      ws.layout = ltree.splitLeaf(ws.layout, rec.splitHint.anchor, rec.splitHint.dir, id, rec.splitHint.where);
      persistLayout(ws);
    }
    persistWorkspaces();
    renderWorkspaces();
    // if its workspace is active, fold the freshly-live pane into the layout
    if (activeWs === rec.wsId) { if (!activeId) { activeId = id; setActiveLabel(id); } relayout(); }
  } else {
    // workspace closed while queued — adopt into "Recovered" so the live PTY is actually
    // VISIBLE (an ownerless pane is filtered out of the rail and never laid out).
    adoptOrphanPane(id);
  }
  renderScheduler();
}

// Render the Scheduler section: live "Scheduler · {working}/{N}" header + one row
// per queued pane (position · harness · elapsed · "run now"). {working} comes from
// list_queue (rows whose state === "working") — the SAME signal the backend caps on,
// so label and admission never disagree. Queued panes have no live session, so they
// never appear in list_queue, never double-count, and never enter the "who needs you"
// queue. Hidden entirely when nothing is queued (the header still carries the cap).
let _schedWorking = 0;
// Keyed reconcile (perf): the scheduler repaints every ~1s WHILE anything is pending
// (elapsedLabel ticks the "queued 12s" clock), but the ONLY thing that changes most
// ticks is each row's meta text. The old ul.replaceChildren() rebuilt every <li> + the
// i-play SVG + re-bound the run-now handler every second; now we keep nodes keyed by
// pending id and mutate only the meta text in place.
const schedNodes = new Map(); // id -> { li, meta }

function buildSchedRow(id) {
  const li = document.createElement("li");
  li.className = "schedrow";
  li.dataset.id = id;
  const dot = document.createElement("span");
  dot.className = "sched-dot";
  const sid = document.createElement("span");
  sid.className = "sched-id";
  sid.textContent = id;
  const meta = document.createElement("span");
  meta.className = "sched-meta";
  const run = document.createElement("button");
  run.className = "sched-run";
  run.title = "Admit now — bypass the concurrency cap";
  run.append(svgIcon("i-play"), document.createTextNode("run now"));
  run.onclick = (e) => { e.stopPropagation(); runNow(id); };
  li.append(dot, sid, meta, run);
  const rec = { li, meta };
  schedNodes.set(id, rec);
  return rec;
}

// Idempotent segmented capacity meter (Nothing-OS instrument readout): `max` discrete
// square blocks, `_schedWorking` filled (var(--display)), red only AT capacity, the rest
// var(--line-strong). The working count rides alongside as a Doto `.metric`. Clears +
// rebuilds the block strip each call so a cap change (± stepper) reflows the segments;
// label text stays the same SSOT (`_schedWorking`/`max`) the backend caps on.
function renderSchedMeter(working, max) {
  const head = document.getElementById("sched-head");
  if (!head) return;
  let meter = document.getElementById("sched-meter");
  if (!meter) {
    meter = document.createElement("span");
    meter.id = "sched-meter";
    meter.className = "sched-meter";
    meter.setAttribute("aria-hidden", "true");
    // place after the label, before the ± actions, inside #sched-head
    const actions = head.querySelector(".sched-head-actions");
    if (actions) head.insertBefore(meter, actions); else head.appendChild(meter);
  }
  const atCap = working >= max && max > 0;
  meter.classList.toggle("at-cap", atCap);
  // rebuild blocks (idempotent): clear, then append `max` <i>, first `working` filled
  meter.replaceChildren();
  for (let i = 0; i < max; i++) {
    const blk = document.createElement("i");
    if (i < working) blk.className = "on";
    meter.appendChild(blk);
  }
}

function renderScheduler(queue) {
  if (Array.isArray(queue)) _schedWorking = queue.filter((r) => r.state === "working").length;
  const max = schedMax();
  const label = document.getElementById("sched-label");
  // KEEP a mono-caps "SCHEDULER" word + the working count as a Doto `.metric` gauge.
  // textContent path is gone (we now own child nodes); guard against re-clobbering.
  if (label) {
    let m = label.querySelector(".metric");
    if (!label.dataset.instr) {
      label.dataset.instr = "1";
      label.replaceChildren();
      const word = document.createElement("span");
      word.className = "sched-word";
      word.textContent = "SCHEDULER";
      m = document.createElement("span");
      m.className = "metric";
      const sep = document.createElement("span");
      sep.className = "sched-frac";
      label.append(word, m, sep);
    }
    m = label.querySelector(".metric");
    if (m) m.textContent = String(_schedWorking);
    const frac = label.querySelector(".sched-frac");
    if (frac) frac.textContent = `/ ${max}`;
    m && m.classList.toggle("metric--accent", _schedWorking >= max && max > 0);
  }
  renderSchedMeter(_schedWorking, max);
  const ul = document.getElementById("scheduler");
  if (!ul) return;
  const ids = Object.keys(pending);
  const live = new Set(ids);
  // EXIT — drop nodes for panes that left the pending set (admitted / cancelled).
  for (const [id, rec] of [...schedNodes]) {
    if (!live.has(id)) { rec.li.remove(); schedNodes.delete(id); }
  }
  if (ids.length === 0) { ul.style.display = "none"; return; }
  ul.style.display = "";
  // REUSE / BUILD + update the only per-tick-varying field (live position + elapsed).
  ids.forEach((id, i) => {
    const rec = schedNodes.get(id) || buildSchedRow(id);
    const p = pending[id];
    // live position (order in the pending list) over the stale enqueue-time position
    rec.meta.textContent = `#${i + 1} · ${p.harness} · queued ${elapsedLabel(p.enqueuedAt)}`;
  });
  // ORDER — appendChild in pending order reorders existing nodes (no clone/rebuild).
  for (const id of ids) ul.appendChild(schedNodes.get(id).li);
}

// ---- workspaces (frontend grouping of PTY panes) ----

// Spawn one pane into a workspace. The pane id is machine-safe (derived from
// wsId, never the user's display name) since it becomes a git branch/worktree.
// Plan 04-02: each pane carries a stable conversation id. A fresh spawn mints one
// (claude `--session-id`); a reopen reuses the persisted id (claude `--resume`) so
// the prior conversation continues instead of starting blank.
// L4b: opt-in "fresh from main" toggle state (default OFF). When ON, Bridge auto-spawn
// resets each pane's worktree to current main before starting, so runs never build on
// a stale prior-run base (07-03 / D41 / RC-2). Persisted to localStorage; the toggle
// lives in the Bridge spawn UI (#br-fresh-toggle). Normal create / reopen is UNAFFECTED
// (the flag is only passed by spawnBridgeTeam — not by spawnPane itself).
let freshFromMainEnabled = (localStorage.getItem("at_fresh_from_main") === "1");

// spawnPane: optional freshFromMain param (L4b). Default undefined (= omitted from the
// invoke), so all existing call sites are byte-identical. Only spawnBridgeTeam (via
// createWorkspace) sets it when the operator's "Fresh from main" toggle is on.
//
// Uncommitted-work guard (L4b): when the backend returns "UNCOMMITTED_WORK:…", the
// destructive reset would discard real work. Surface a confirm dialog; if the operator
// confirms, re-invoke once with forceFresh=true. The guard only ever fires when
// freshFromMain=true, so normal spawns are completely unaffected.
// Two-click arm state for the fresh-from-main UNCOMMITTED_WORK force path (spawnPane):
// first refused spawn arms the pane id for 6s; a retry within the window force-resets.
let _forceFreshArmedId = null;
let _forceFreshArmTimer = null;

async function spawnPane(wsId, prompt, harnessOverride, resume = false, roleOverride, idxOverride, modelOverride, freshFromMain) {
  const ws = workspaces[wsId];
  if (!ws) return null;
  // D63: reopen passes the pane's ORIGINAL counter-idx so it keeps its id/worktree and
  // reads its OWN sparse-array slot (harnesses/roles/sessionIds are idx-keyed). `!= null`
  // so idx 0 works; keep `counter` past any explicit idx so a later add-agent can't collide.
  const idx = idxOverride != null ? idxOverride : ws.counter++;
  if (idxOverride != null && idx >= ws.counter) ws.counter = idx + 1;
  const id = `${wsId}-p${idx}`;
  deadPanes.delete(id); // a reopen reuses this id — clear any stale death flag
  _deadHintShown.delete(id); // the dead-typing hint re-arms with the fresh pane
  const harness = harnessOverride || ws.harness; // per-agent harness; ws.harness stays the default
  ws.sessionIds = ws.sessionIds || [];
  // Persist the harness PER PANE (not just the workspace default): a mixed
  // workspace (e.g. p0–p3 claude + p4 cursor) must reopen each pane with ITS
  // harness, else cursor panes respawn as claude and `--resume` a uuid claude
  // never registered → "No conversation found".
  ws.harnesses = ws.harnesses || [];
  ws.harnesses[idx] = harness;
  // 17-01: persist the typed ROLE PER PANE beside the harness (same lifecycle), so a
  // reopen re-injects the persona. On reopen roleOverride is undefined → fall back to
  // the stored ws.roles[idx]. Empty/none → undefined (omitted from the invoke → the
  // backend's fail-soft path yields a homogeneous pane = today's behavior).
  ws.roles = ws.roles || [];
  const role = (roleOverride !== undefined ? roleOverride : ws.roles[idx]) || undefined;
  ws.roles[idx] = role || null;
  // model-at-spawn: persist the MODEL PER PANE beside harness/role (same idx-keyed
  // lifecycle) so a reopen re-applies it. Unset → undefined (omitted from the invoke →
  // account default = today's behavior).
  ws.models = ws.models || [];
  const model = (modelOverride !== undefined ? modelOverride : ws.models[idx]) || undefined;
  ws.models[idx] = model || null;
  paneMetaById[id] = { harness, model: model || "" }; // id-keyed so phMeta/newline survive a migrate
  let sessionId = ws.sessionIds[idx];
  if (!resume || !sessionId) {
    // create (or reopen with no tracked id) → mint a fresh one and persist by index
    sessionId = (typeof crypto !== "undefined" && crypto.randomUUID)
      ? crypto.randomUUID()
      : `${wsId}-p${idx}-${Date.now()}`;
    ws.sessionIds[idx] = sessionId;
  }
  if (hasTauri()) {
    let res;
    // L4b: build the invoke args — fresh_from_main is only sent when true (omitting it
    // keeps all existing call sites byte-identical: Tauri treats a missing optional as
    // its default, which is `None` → `false` in the backend).
    const spawnArgs = { id, harness, repo: ws.repo, sessionId, resume, role, model };
    // Gap #4 (identity-on-rows): stamp the workspace's create-time tag on every pane
    // spawn (reopens included — ws.tag persists in localStorage) so the backend records
    // it into the live registry and queue rows carry it. Omitted when untagged.
    if (ws.tag) spawnArgs.tag = String(ws.tag);
    if (freshFromMain) spawnArgs.freshFromMain = true;
    try { res = await invoke("spawn_workspace", spawnArgs); }
    catch (e) {
      const errStr = String(e);
      // L4b: uncommitted-work guard — the backend refuses the destructive reset when
      // the worktree has uncommitted changes and force_fresh was not set. The sentinel
      // format is "UNCOMMITTED_WORK:<id>:<details>". NO window.confirm (wry/WKWebView can
      // no-op it — same reason onStopClick/bridgeOpenPr use two-click arm): the FIRST
      // attempt warns + arms this pane id; re-running the same spawn within 6s confirms
      // the destructive reset (re-invoke once with forceFresh=true).
      if (errStr.startsWith("UNCOMMITTED_WORK:")) {
        const detail = errStr.replace(/^UNCOMMITTED_WORK:[^:]*:/, "").trim();
        if (_forceFreshArmedId === id) {
          clearTimeout(_forceFreshArmTimer);
          _forceFreshArmedId = null;
          try {
            const forceArgs = { ...spawnArgs, forceFresh: true };
            res = await invoke("spawn_workspace", forceArgs);
          } catch (e2) {
            const err = document.getElementById("f-error");
            if (err) err.textContent = String(e2);
            showToast("Spawn failed (after confirm): " + String(e2));
            return null;
          }
        } else {
          _forceFreshArmedId = id;
          clearTimeout(_forceFreshArmTimer);
          _forceFreshArmTimer = setTimeout(() => { _forceFreshArmedId = null; }, 6000);
          const err = document.getElementById("f-error");
          if (err) err.textContent = `Fresh-from-main: ${id} has uncommitted changes — ${detail}. Retry within 6s to discard them.`;
          showToast(`⚠ ${id} has uncommitted work — retry the spawn within 6s to force-reset & DISCARD it`);
          return null;
        }
      } else {
        const err = document.getElementById("f-error");
        if (err) err.textContent = errStr;
        showToast("Spawn failed: " + errStr);
        return null;
      }
    }
    // Scheduler (D33): over the concurrency cap the backend creates NO PTY and
    // returns { queued:true, position }. Park the pane in the Scheduler section
    // (no xterm, NOT in ws.paneIds — it isn't live); the `workspace-admitted`
    // event attaches it when a working slot frees. Backward-compatible: an older
    // backend returning `()` → res is null → falsy `res.queued` → spawn as today.
    if (res && res.queued) {
      pending[id] = { id, wsId, harness, prompt: prompt || null, enqueuedAt: Date.now(), position: res.position };
      persistPending(); // survive a webview reload (backend queue stays alive)
      renderScheduler();
      return id;
    }
  }
  ensureSession(id);
  ws.paneIds.push(id);
  if (prompt) sendDeferredPrompt(id, prompt);
  return id;
}

// Create a workspace with N panes and make it active. `harnesses` (optional) is a
// per-pane harness list (a preset's "2 claude + 2 cursor"); when absent every pane
// uses the single `harness` (the legacy path — submitModal/quickCreate). The mixed
// seam already lives in spawnPane(harnessOverride); this just feeds it per index.
// `ws.harness` (the workspace default used by reopen/add-agent) tracks pane 0.
// `roles` (17-01, optional) is the per-pane role list from the wizard ("none" =
// roleless); when absent the scalar `role` (the add-agent modal) applies to every pane.
// L4b: freshFromMain is an optional opt-in (default undefined = omit). Only the Bridge
// auto-spawn path passes it (when the "Fresh from main" toggle is on); all other create
// paths are unaffected (the arg is absent → spawnPane omits it → backend gets default false).
async function createWorkspace({ name, color, repo, harness, count, prompt, role, harnesses, roles, models, freshFromMain, tag }) {
  const perPane = (Array.isArray(harnesses) && harnesses.length === count) ? harnesses : Array(count).fill(harness);
  // 17-01: per-pane roles (wizard) take precedence; map "none"/"" → undefined
  // (homogeneous). Fall back to the single `role` (add-agent) for every pane.
  const perRole = (Array.isArray(roles) && roles.length === count)
    ? roles.map((r) => (r && r !== "none" ? r : undefined))
    : Array(count).fill(role || undefined);
  // model-at-spawn: optional per-pane model list (same shape as harnesses/roles);
  // absent/short → account default for every pane.
  const perModel = (Array.isArray(models) && models.length === count)
    ? models.map((m) => m || undefined)
    : Array(count).fill(undefined);
  const wsId = newWsId();
  workspaces[wsId] = { name, color, repo, harness: perPane[0] || harness, tag: tag || undefined, paneIds: [], sessionIds: [], harnesses: [], roles: [], models: [], counter: 0, count, dormant: false };
  // Per-pane harness (wizard presets) + per-pane role compose: harness from perPane[i],
  // role from perRole[i]. spawnPane persists each into ws.harnesses[]/ws.roles[].
  // L4b: freshFromMain is threaded per-pane — each Bridge worker gets the reset flag.
  for (let i = 0; i < count; i++) await spawnPane(wsId, prompt, perPane[i], false, perRole[i], undefined, perModel[i], freshFromMain);
  gridMode = count > 1; // a multi-pane workspace opens in grid view (else relayout shows one pane)
  setActiveWs(wsId);
  persistWorkspaces();
  renderWorkspaces();
  return wsId;
}

// Per-row `+`: dormant → reopen; live → open the "add agent" picker (choose
// harness/count, like the create modal but scoped to this workspace's folder).
function addAgent(wsId) {
  const ws = workspaces[wsId];
  if (!ws) return;
  if (ws.dormant) return reopenWorkspace(wsId);
  openAddAgent(wsId);
}

// Spawn N agents into an existing workspace. `harnesses` (multi-select chips) is
// round-robin'd per pane so a mixed pick alternates (claude/cursor/claude/cursor);
// the single `harness` is the back-compat fallback when no set is passed.
async function addAgentsToWorkspace(wsId, { harness, harnesses, count, prompt, role, modelByHarness }) {
  const ws = workspaces[wsId];
  if (!ws) return;
  if (activeWs !== wsId) setActiveWs(wsId);
  const perPane = expandHarnesses(
    (Array.isArray(harnesses) && harnesses.length) ? harnesses : [harness || ws.harness || "claude"],
    count,
  );
  // 17-01: the added agents take the role chosen in the add modal (per-pane picker).
  // model-at-spawn: model is keyed BY HARNESS (model ids are harness-specific — one
  // flat model can't fit a mixed claude+codex team), looked up per pane's harness.
  const mFor = (h) => (modelByHarness && modelByHarness[h]) || undefined;
  for (let i = 0; i < count; i++) await spawnPane(wsId, prompt, perPane[i], false, role || undefined, undefined, mFor(perPane[i]));
  ws.count = ws.paneIds.length;
  if (activeWs === wsId && ws.paneIds.length > 1) { gridMode = true; updateGridBtn(); }
  persistWorkspaces();
  renderWorkspaces();
  relayout();
}

// Close a whole workspace: kill every pane's PTY + worktree, drop the def.
async function closeWorkspaceGroup(wsId) {
  const ws = workspaces[wsId];
  if (!ws) return;
  for (const id of [...ws.paneIds]) await closeWorkspace(id);
  // drop any QUEUED (un-admitted) panes for this workspace — no PTY to close, but
  // leaving them would orphan "run now" rows that could never attach.
  for (const pid of Object.keys(pending)) if (pending[pid].wsId === wsId) delete pending[pid];
  persistPending();
  renderScheduler();
  delete workspaces[wsId];
  if (activeWs === wsId) {
    const next = Object.keys(workspaces).find((w) => !workspaces[w].dormant) || null;
    if (next) setActiveWs(next);
    else { activeWs = null; activeId = null; setActiveLabel(null); relayout(); }
  }
  persistWorkspaces();
  renderWorkspaces();
}

// Make a workspace active; default the active pane to its first.
function setActiveWs(wsId) {
  const ws = workspaces[wsId];
  if (!ws) return;
  if (ws.dormant) return reopenWorkspace(wsId);
  activeWs = wsId;
  activeId = ws.paneIds[0] || null;
  setActiveLabel(activeId);
  updateGridBtn();
  relayout();
  renderWorkspaces();
  if (activeId) {
    const s = sessions[activeId];
    requestAnimationFrame(() => { if (s) { fitSession(s, activeId); s.term.focus(); } });
  }
  resetQueueSig();  // workspace switch: force the next queue tick to repaint
  pollRevealNow();  // its panes were in the hidden ~750ms lane — read them NOW so reveal is instant
  persistActiveWs();
}

// ---- Workspace DISPLAY-name rename (2026-06-18) — mutates ws.name (a display field, NEVER
// the machine wsId / git branch root) + re-persists. Mirrors beginPaneRename; isolated from
// the PTY/session lifecycle so a rename can never wedge a workspace. ----
// While a workspace row is being renamed, suppress the periodic list_queue re-render
// (renderWorkspaces does ul.replaceChildren()) — otherwise a poll mid-edit destroys the
// inline input. Set on rename start, cleared in finish() before its own re-render.
let renamingWs = null;
function beginWorkspaceRename(wsId, nameEl, row) {
  const ws = workspaces[wsId];
  if (!ws || row.querySelector("input.ws-rename")) return;
  renamingWs = wsId;
  const input = document.createElement("input");
  input.className = "ws-rename";
  input.value = ws.name || wsId;
  input.spellcheck = false;
  row.classList.add("renaming");
  nameEl.textContent = "";
  nameEl.appendChild(input);
  input.focus();
  input.select();
  let done = false;
  const finish = (commit) => {
    if (done) return;
    done = true;
    row.classList.remove("renaming");
    if (commit) {
      const v = (input.value || "").trim();
      if (v && v !== ws.name) { ws.name = v; try { persistWorkspaces(); } catch (_) {} }
    }
    renamingWs = null;
    try { renderWorkspaces(); } catch (_) { nameEl.textContent = ws.name || wsId; }
  };
  input.addEventListener("keydown", (e) => {
    e.stopPropagation();
    if (e.key === "Enter") { e.preventDefault(); finish(true); }
    else if (e.key === "Escape") { e.preventDefault(); finish(false); }
  });
  input.addEventListener("blur", () => finish(true));
  input.addEventListener("click", (e) => e.stopPropagation());
  input.addEventListener("mousedown", (e) => e.stopPropagation());
  input.addEventListener("dblclick", (e) => e.stopPropagation());
}

// ── workspace right-click context menu (Rename / Change Color / Add / Close) ──
// Matches the reference: the row stays clean (dot · name · badge · ✕); these actions live here.
let wsMenuEl = null;
function closeWorkspaceMenu() {
  if (!wsMenuEl) return;
  wsMenuEl.remove(); wsMenuEl = null;
  document.removeEventListener("mousedown", onWsMenuOutside, true);
  document.removeEventListener("keydown", onWsMenuKey, true);
}
function onWsMenuOutside(e) { if (wsMenuEl && !wsMenuEl.contains(e.target)) closeWorkspaceMenu(); }
function onWsMenuKey(e) {
  if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); closeWorkspaceMenu(); }
  else if (wsMenuEl) menuArrowNav(e, wsMenuEl);
}
function beginWorkspaceRenameById(wsId) {
  const row = document.querySelector(`.wsrow[data-ws="${wsId}"]`);
  const nameEl = row && row.querySelector(".ws-name");
  if (row && nameEl) beginWorkspaceRename(wsId, nameEl, row);
}
function openWorkspaceMenu(wsId, x, y) {
  closeWorkspaceMenu();
  const ws = workspaces[wsId];
  if (!ws) return;
  const menu = document.createElement("div");
  menu.className = "ph-menu";
  menu.setAttribute("role", "menu");
  const item = (label, glyph, hint, onClick, opts) => {
    const it = document.createElement("button");
    it.className = "ph-menu-item" + (opts && opts.danger ? " danger" : "");
    it.setAttribute("role", "menuitem");
    const g = document.createElement("span"); g.className = "ph-menu-ico"; g.textContent = glyph;
    const t = document.createElement("span"); t.className = "ph-menu-label"; t.textContent = label;
    it.append(g, t);
    if (hint) { const k = document.createElement("span"); k.className = "ph-menu-kbd"; k.textContent = hint; it.appendChild(k); }
    it.onclick = (e) => { e.stopPropagation(); closeWorkspaceMenu(); onClick(); };
    menu.appendChild(it);
  };
  const sep = () => { const s = document.createElement("div"); s.className = "ph-menu-sep"; menu.appendChild(s); };
  if (ws.dormant) {
    item("Reopen", "↻", "", () => reopenWorkspace(wsId));
    item("Rename", "✎", "F2", () => beginWorkspaceRenameById(wsId));
    sep();
    item("Forget workspace", "✕", "", () => closeWorkspaceGroup(wsId), { danger: true });
  } else {
    item("Rename", "✎", "F2", () => beginWorkspaceRenameById(wsId));
    // Change Color — a swatch row (matches the reference's "Change Color" affordance).
    const lab = document.createElement("div"); lab.className = "ph-menu-collabel"; lab.textContent = "Change color";
    menu.appendChild(lab);
    const colors = document.createElement("div"); colors.className = "ph-menu-colors";
    for (const c of WS_PALETTE) {
      const sw = document.createElement("button");
      sw.className = "ph-color-sw" + (ws.color === c ? " active" : "");
      sw.style.background = c; sw.title = c; sw.setAttribute("aria-label", "Set color " + c);
      sw.onclick = (e) => { e.stopPropagation(); ws.color = c; try { persistWorkspaces(); } catch (_) {} renderWorkspaces(); closeWorkspaceMenu(); };
      colors.appendChild(sw);
    }
    menu.appendChild(colors);
    item("Add agent", "＋", "", () => addAgent(wsId));
    // "Move all panes to" — merge every live pane of this workspace into another (#2 multiple).
    const moveTargets = Object.keys(workspaces).filter((w) => w !== wsId && !workspaces[w].dormant);
    if (moveTargets.length && ws.paneIds.filter((id) => sessions[id]).length) {
      const ml = document.createElement("div"); ml.className = "ph-menu-collabel"; ml.textContent = "Move all panes to";
      menu.appendChild(ml);
      for (const w of moveTargets) item(workspaces[w].name, "⇄", "", () => mergeWorkspaceInto(wsId, w));
    }
    sep();
    item("Close workspace", "✕", "⌘⇧W", () => closeWorkspaceGroup(wsId), { danger: true });
  }
  document.body.appendChild(menu);
  wsMenuEl = menu;
  const mw = menu.offsetWidth || 210, mh = menu.offsetHeight || 200;
  let left = Math.min(x, window.innerWidth - mw - 8); if (left < 8) left = 8;
  let top = Math.min(y, window.innerHeight - mh - 8); if (top < 8) top = 8;
  menu.style.left = left + "px"; menu.style.top = top + "px";
  document.addEventListener("mousedown", onWsMenuOutside, true);
  document.addEventListener("keydown", onWsMenuKey, true);
  menuFocusFirst(menu); // a11y: keyboard users land on the first item (Arrows cycle)
}

// The WORKSPACES list is the always-visible sidebar; pollQueue re-renders it on every
// queue-signature change (~1Hz during active runs). The old ul.replaceChildren() wipe
// rebuilt every row + re-bound every listener per tick — churning GC, dropping :hover /
// :focus, and killing CSS transitions. We keep nodes keyed by wsId and mutate only the
// changed fields in place. Listeners (built once) read `workspaces[wsId]` LIVE at click
// time, since the node now outlives the tick that built it.
const wsNodes = new Map(); // wsId -> { li, dot, name, badge, close }

function buildWsRow(wsId) {
  const li = document.createElement("li");
  li.dataset.ws = wsId;
  const dot = document.createElement("span");
  dot.className = "ws-dot";
  const name = document.createElement("span");
  name.className = "ws-name";
  name.addEventListener("dblclick", (e) => { e.stopPropagation(); beginWorkspaceRename(wsId, name, li); });
  const badge = document.createElement("span");
  badge.className = "ws-badge";
  // Clean row (dot · name · badge · ✕) — Rename / Change Color / Add agent live in the
  // right-click context menu (the inline ✎/+ overflowed the 5-col grid and wrapped the ✕).
  const close = document.createElement("button");
  close.className = "ws-close";
  close.textContent = "✕";
  close.setAttribute("aria-label", "Close workspace"); // a11y: glyph-only; kept live in updateWsRow
  close.onclick = (e) => { e.stopPropagation(); closeWorkspaceGroup(wsId); };
  li.append(dot, name, badge, close);
  // read dormant LIVE — the persistent node would otherwise capture a stale ws (the old
  // per-tick rebuild made the closure fresh for free; reconcile must read at click time).
  li.onclick = () => { const w = workspaces[wsId]; if (w) (w.dormant ? reopenWorkspace(wsId) : setActiveWs(wsId)); };
  li.addEventListener("contextmenu", (e) => { e.preventDefault(); e.stopPropagation(); openWorkspaceMenu(wsId, e.clientX, e.clientY); });
  // Keyboard-operable: the row was mouse-only (onclick + dblclick-rename + right-click menu).
  // Enter/Space activates it (open dormant / select); F2 renames (matches the title hint).
  li.tabIndex = 0;
  li.setAttribute("role", "button");
  li.setAttribute("aria-haspopup", "menu");
  li.addEventListener("keydown", (e) => {
    if (e.key === "Enter" || e.key === " ") { e.preventDefault(); li.onclick(); }
    else if (e.key === "F2") { e.preventDefault(); beginWorkspaceRename(wsId, name, li); }
    else if (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) {
      // a11y: the context menu was right-click-only. Open it at the row's own corner.
      e.preventDefault();
      const r = li.getBoundingClientRect();
      openWorkspaceMenu(wsId, r.left + 8, r.bottom);
    }
  });
  const rec = { li, dot, name, badge, close };
  wsNodes.set(wsId, rec);
  return rec;
}

function updateWsRow(rec, wsId, needIds, errIds) {
  const ws = workspaces[wsId];
  // A known-dead pane outranks needs-you: a corpse can't be replied to, so its
  // terminal-red wins over amber (and the backend may still report it needs_human
  // while it lingers in `sups`). Dead is a SEPARATE tier above the existing
  // list_queue "error" state — both paint --danger, neither regresses the other.
  const dead = !ws.dormant && ws.paneIds.some((p) => deadPanes.has(p));
  const needs = !ws.dormant && !dead && ws.paneIds.some((p) => needIds.has(p));
  const errored = !ws.dormant && !dead && !needs && ws.paneIds.some((p) => errIds.has(p));
  rec.li.className = "wsrow" + (wsId === activeWs ? " active" : "") + (ws.dormant ? " dormant" : "");
  // dot precedence: dormant → muted, dead/errored → red, needs-you → amber, else identity color
  rec.dot.style.background = ws.dormant ? "var(--muted)" : dead ? "var(--danger)" : needs ? "var(--need)" : errored ? "var(--danger)" : ws.color;
  // WCAG 1.4.1: the dot's meaning is otherwise color-only (dead/needs/error all differ by hue).
  // Expose a redundant text cue (screen-reader announced + hover tooltip) so the state isn't
  // conveyed by color alone. (A per-state glyph/shape is a follow-up CSS change.)
  const dotState = ws.dormant ? "dormant" : dead ? "dead" : needs ? "needs you" : errored ? "error" : "active";
  rec.dot.setAttribute("role", "img");
  rec.dot.setAttribute("aria-label", `status: ${dotState}`);
  rec.dot.title = dotState;
  if (rec.name.textContent !== ws.name) rec.name.textContent = ws.name;
  rec.name.title = (ws.dormant ? `${ws.name} — click to reopen (${ws.repo})` : ws.repo) + "  ·  right-click for actions · F2 to rename";
  const badgeText = String(ws.dormant ? ws.count : ws.paneIds.length);
  if (rec.badge.textContent !== badgeText) rec.badge.textContent = badgeText;
  rec.close.title = ws.dormant ? "Forget workspace" : "Close workspace (kills agents)";
  rec.close.setAttribute("aria-label", ws.dormant ? "Forget workspace" : "Close workspace (kills agents)");
}

// Pane-head status tick (Nothing-OS instrument readout): drive a `data-state` attribute
// on each live pane's `.pane-head` from the latest queue poll, so CSS can paint a small
// status dot — working=var(--display), idle=var(--muted), needs-input=var(--need) amber,
// error/dead=var(--accent) red. Precedence mirrors updateWsRow: dead corpse outranks
// needs-you, which outranks error, which outranks the raw working/idle state. Reads
// sessions[id].el (the term-pane) → its `.pane-head` child. No-op for panes with no head.
function paneStateFor(id, row) {
  if (deadPanes.has(id)) return "error";          // corpse → terminal red (dead tier)
  if (!row) return "idle";                         // no live queue row → idle/muted
  if (row.needs_human) return "needs-input";       // amber, awaiting the human
  if (row.state === "error") return "error";       // backend error tier → red
  if (row.state === "working") return "working";   // actively running → display white
  return "idle";                                   // starting/idle/anything else → muted
}
function paintPaneHeadTicks(queue) {
  const byId = new Map((queue || []).map((r) => [r.id, r]));
  for (const id of Object.keys(sessions)) {
    const s = sessions[id];
    if (!s || !s.el) continue;
    const head = s.el.querySelector(".pane-head");
    if (!head) continue;
    const st = paneStateFor(id, byId.get(id));
    if (head.dataset.state !== st) {
      head.dataset.state = st;
      // WCAG 1.4.1: the tick (.ph-id::before) is otherwise color-only. Mirror the ws-dot
      // fix: expose a redundant text cue on the .ph-id span — screen-reader announced via
      // aria-label, hover tooltip via title (rename hint preserved). "dead" is a frontend
      // tier (paneStateFor folds it into "error" for the CSS color; the word stays honest).
      const word = deadPanes.has(id) ? "dead"
        : st === "needs-input" ? "needs input"
        : st; // working / idle / error
      const phId = head.querySelector(".ph-id");
      if (phId) {
        phId.setAttribute("aria-label", `${id}: ${word}`);
        phId.title = `${word} · Double-click to rename · ${id}`;
      }
    }
  }
}

// Render the WORKSPACES list. `queue` (latest poll) drives the amber dot.
function renderWorkspaces(queue) {
  const ul = document.getElementById("workspaces");
  if (!ul) return;
  // don't clobber an in-progress inline rename (background poll). If the flag is stale
  // (no input actually present), clear it and proceed.
  if (renamingWs) { if (ul.querySelector("input.ws-rename")) return; renamingWs = null; }
  const needIds = new Set((queue || []).filter((r) => r.needs_human).map((r) => r.id));
  const errIds = new Set((queue || []).filter((r) => r.state === "error").map((r) => r.id));
  paintPaneHeadTicks(queue); // Nothing-OS pane-head status tick (data-state → CSS dot color)
  const wsIds = Object.keys(workspaces);
  const live = new Set(wsIds);
  // EXIT — drop nodes for workspaces that no longer exist (closed / forgotten).
  for (const [wsId, rec] of [...wsNodes]) {
    if (!live.has(wsId)) { rec.li.remove(); wsNodes.delete(wsId); }
  }
  // REUSE / BUILD + update in place.
  for (const wsId of wsIds) {
    const rec = wsNodes.get(wsId) || buildWsRow(wsId);
    updateWsRow(rec, wsId, needIds, errIds);
  }
  // ORDER — appendChild in key order moves existing nodes into place (no clone/rebuild).
  for (const wsId of wsIds) ul.appendChild(wsNodes.get(wsId).li);
}

// ---- workspace persistence (defs only; PTYs don't survive restart, D7) ----
function persistWorkspaces() {
  const defs = Object.keys(workspaces)
    .map((wsId) => {
      const ws = workspaces[wsId];
      // sessionIds persist so a dormant workspace can RESUME each pane's conversation
      // on reopen (Plan 04-02), not just respawn fresh agents.
      // harnesses[] persists the PER-PANE harness so a mixed workspace reopens
      // each pane with its own harness (not just the workspace default).
      // roles[] (17-01) persists the PER-PANE role on the same lifecycle, so a
      // reopen after a webview reload re-injects each pane's persona (else a
      // reopened Scout silently goes homogeneous).
      // idxList (D63): the surviving panes' ORIGINAL counter-idxs so reopen re-spawns
      // each at its own idx (reads its own idx-keyed harness/role/conversation), instead
      // of positional 0..count-1 (which mismapped a survivor onto a closed neighbor after
      // a non-last-pane close). Live = derive from paneIds; dormant = carry the loaded one
      // (NEVER read an undefined ws.idxList for a live ws).
      // models[] (model-at-spawn) persists the PER-PANE model override on the same
      // idx-keyed lifecycle as harnesses/roles, so reopen re-applies it.
      return { wsId, name: ws.name, color: ws.color, repo: ws.repo, harness: ws.harness, count: ws.dormant ? ws.count : ws.paneIds.length, sessionIds: ws.sessionIds || [], harnesses: ws.harnesses || [], roles: ws.roles || [], models: ws.models || [], idxList: ws.dormant ? (ws.idxList || null) : ws.paneIds.map(paneIdx) };
    })
    .filter((d) => d.count > 0);
  localStorage.setItem("at_workspaces", JSON.stringify(defs));
}

// On startup, persisted workspaces appear as dormant "reopen" rows (no live PTYs).
function loadWorkspaces() {
  let defs = [];
  try { defs = JSON.parse(localStorage.getItem("at_workspaces") || "[]"); } catch (_) { defs = []; }
  for (const d of defs) {
    if (!d.wsId) continue;
    workspaces[d.wsId] = { name: d.name, color: d.color, repo: d.repo, harness: d.harness, paneIds: [], sessionIds: Array.isArray(d.sessionIds) ? d.sessionIds : [], harnesses: Array.isArray(d.harnesses) ? d.harnesses : [], roles: Array.isArray(d.roles) ? d.roles : [], models: Array.isArray(d.models) ? d.models : [], idxList: Array.isArray(d.idxList) ? d.idxList : null, counter: 0, count: d.count || 1, dormant: true };
  }
  renderWorkspaces();
}

// ---- Scheduler pending persistence (Plan 05-02 reload-reconcile) ----
// Persist the whole queued-pane map (small + JSON-safe) so a webview reload doesn't
// orphan a queued pane (its row + "run now") or strand its admit event.
function persistPending() {
  try { localStorage.setItem("at_pending", JSON.stringify(Object.values(pending))); } catch (_) {}
}
// Restore the queued-pane map on load. After a reload the Rust backend (and its queue)
// is still alive, so these rows are real; reconcilePending() then sorts out any that
// were admitted while we were gone.
function loadPending() {
  let recs = [];
  try { recs = JSON.parse(localStorage.getItem("at_pending") || "[]"); } catch (_) { recs = []; }
  if (!Array.isArray(recs)) recs = [];
  for (const r of recs) { if (r && r.id) pending[r.id] = r; }
  renderScheduler();
}
// Reload-reconcile: a restored `pending` entry may already have been ADMITTED by the
// backend while the webview was reloading (its `workspace-admitted` event fired into
// the void). Any pending id the backend now lists as a LIVE workspace was admitted →
// adopt it via admitPending so it isn't stranded as a phantom "run now" row that can
// never attach. Queued (un-admitted) panes have no PTY, so they never appear in
// list_workspaces and are left as live Scheduler rows. Runs once per load.
//   Known limit: a FULL app restart (not a reload) kills the backend queue, so a
//   leftover entry is neither live (won't reconcile here) nor still-queued (its
//   "run now" rejects "no such pending workspace"); it lingers until its workspace is
//   closed (closeWorkspaceGroup drops it). In-scope reload is fully covered.
let _pendingReconciled = false;
function reconcilePending(all) {
  if (_pendingReconciled || !Array.isArray(all)) return;
  _pendingReconciled = true;
  if (!Object.keys(pending).length) return;
  const live = new Set(all);
  for (const id of Object.keys(pending)) {
    if (live.has(id)) admitPending(id, pending[id].harness);
  }
}

// Reopen a dormant workspace in place (same wsId, no duplicate): respawn N panes,
// RESUMING each pane's prior conversation (Plan 04-02). counter resets to 0 so pane
// ids + indices line up with the persisted sessionIds; sessionIds is preserved (not
// reset) so spawnPane(resume=true) reuses the stored id per index.
async function reopenWorkspace(wsId) {
  const ws = workspaces[wsId];
  if (!ws || !ws.dormant) return;
  ws.dormant = false;
  ws.counter = 0;
  ws.paneIds = [];
  ws.sessionIds = ws.sessionIds || [];
  ws.harnesses = ws.harnesses || [];
  ws.roles = ws.roles || [];
  // D63: re-spawn each SURVIVING pane at its ORIGINAL counter-idx (the persisted idxList),
  // NOT positional 0..count-1 — so a pane reopens with ITS OWN idx-keyed harness/role/
  // conversation and keeps its id/worktree. closeWorkspace compacts paneIds but leaves the
  // arrays idx-keyed, so 0..count-1 mismapped a survivor onto a closed neighbor's slot.
  // Falls back to 0..count-1 for legacy persisted data with no idxList. spawnPane reads
  // ws.roles[idx]/sessionIds[idx] by this idx (roleOverride undefined → its own role).
  const idxList = (ws.idxList && ws.idxList.length) ? ws.idxList : survivorIdxList(ws.paneIds, ws.count);
  gridMode = idxList.length > 1;
  for (const idx of idxList) await spawnPane(wsId, null, ws.harnesses[idx] || ws.harness, true, undefined, idx);
  setActiveWs(wsId);
  persistWorkspaces();
  renderWorkspaces();
}

// ---- shared modal focus management (a11y: WCAG 2.1.2 / 2.4.3) --------------------------
// One helper serves EVERY dialog: on open, remember the invoking trigger
// (document.activeElement at open time), give the card initial focus (unless the dialog's
// own deferred .focus() will land — ours runs synchronously, so a rAF field-focus still
// wins), and keep Tab/Shift+Tab cycling INSIDE the card. On close, restore focus to the
// trigger. Re-entrant (a stack): opening dialog B over dialog A pushes a frame; closing B
// restores focus into A. Escape stays with the existing global chain — this helper only
// owns Tab and focus restore.
// ---- new-workspace / add-agent modal ----
const modal = document.getElementById("modal");
let selectedCount = 1;
let modalWsId = null; // the workspace agents are added into (#modal is add-mode-only now)

// a11y: every toggle chip mirrors its .active class into aria-pressed (the wizard.js
// segmented-control pattern) — one helper so the two can never drift at a call site.
function setChipActive(el, on) {
  el.classList.toggle("active", !!on);
  el.setAttribute("aria-pressed", on ? "true" : "false");
}

function setCount(n) {
  selectedCount = n || 1;
  document.querySelectorAll("#f-count .count-tile").forEach((b) =>
    setChipActive(b, parseInt(b.dataset.n, 10) === selectedCount));
  updateHarnessPreview();
}

// ---- harness (multi-select) + role (single-select) chips (replaces the old <select>s) ----
// Harness: the ordered list of toggled-on chips (DOM is the single source of truth — the
// visual order is the round-robin order). Never empty — the click handler keeps ≥1 active.
function selectedHarnesses() {
  return [...document.querySelectorAll("#f-harness .count-tile.active")].map((b) => b.dataset.h);
}
// Role: the one active chip's id ("" = none → homogeneous).
function selectedRole() {
  const a = document.querySelector("#f-role .count-tile.active");
  return (a && a.dataset.r) || "";
}
// Set the harness chip — SINGLE-SELECT (one harness for the agents you add; mirrors the
// role chips). Exactly one chip is active; tapping a chip selects ONLY that harness. A
// mixed/alternating team is built in the layout wizard's per-pane pickers, NOT here — so
// "tap cursor → get cursor" is unambiguous (the old multi-select 'combine' let a stale
// pre-selected default linger and spawn the wrong harness).
function setHarnessChips(ids) {
  const want = (Array.isArray(ids) && ids.length ? ids[0] : "claude");
  document.querySelectorAll("#f-harness .count-tile").forEach((b) =>
    setChipActive(b, b.dataset.h === want));
  if (!document.querySelector("#f-harness .count-tile.active")) {
    const first = document.querySelector('#f-harness .count-tile[data-h="claude"]') || document.querySelector("#f-harness .count-tile");
    if (first) setChipActive(first, true);
  }
  // model-at-spawn: retarget the model input's autocomplete at the chosen harness
  // (model ids are harness-specific → a switch clears any typed id) + load its list.
  const fm = document.getElementById("f-model");
  if (fm) {
    if (fm.getAttribute("list") !== "dl-models-" + want) fm.value = "";
    fm.setAttribute("list", "dl-models-" + want);
    ensureModelsLoaded(want);
  }
  updateHarnessPreview();
}
// Single-select the role chip matching id ("" = none).
function setRoleChip(id) {
  const r = id || "";
  document.querySelectorAll("#f-role .count-tile").forEach((b) =>
    setChipActive(b, (b.dataset.r || "") === r));
  if (!document.querySelector("#f-role .count-tile.active")) {
    const none = document.querySelector('#f-role .count-tile[data-r=""]');
    if (none) setChipActive(none, true);
  }
}
// Preview: single-select → every added terminal uses the one chosen harness.
function updateHarnessPreview() {
  const el = document.getElementById("f-harness-preview");
  if (!el) return;
  const h = selectedHarnesses()[0] || "claude";
  const n = selectedCount || 1;
  el.textContent = `All ${n} terminal${n === 1 ? "" : "s"}: ${h}`;
}
// Wire the chips once (the modal is reused; listeners live for the page lifetime).
document.querySelectorAll("#f-harness .count-tile").forEach((btn) => {
  btn.onclick = () => setHarnessChips([btn.dataset.h]); // single-select: tap = exactly this harness
});
document.querySelectorAll("#f-role .count-tile").forEach((btn) => {
  btn.onclick = () => setRoleChip(btn.dataset.r || "");
});

function getRecents() {
  try { return JSON.parse(localStorage.getItem("at_recent_folders") || "[]"); } catch (_) { return []; }
}
function rememberFolder(f) {
  localStorage.setItem(LS_KEYS.lastFolder, f);
  const rec = getRecents().filter((x) => x !== f);
  rec.unshift(f);
  localStorage.setItem("at_recent_folders", JSON.stringify(rec.slice(0, 6)));
}
// (openModal — the modal's old "create" mode — was deleted 2026-07: the create flow moved
// wholly to the layout wizard (launchWizard) and openModal had zero call sites left. #modal
// now opens ONLY via openAddAgent below, so submitModal is add-mode-only.)

// Add-agent variant: folder/name are locked to the workspace —
// you only choose the harness (and how many) for the new agent(s).
function openAddAgent(wsId) {
  const ws = workspaces[wsId];
  if (!ws) return;
  modalWsId = wsId;
  document.getElementById("f-error").textContent = "";
  document.getElementById("modal-title").textContent = `Add an agent to ${ws.name}`;
  document.getElementById("modal-sub").textContent = `Folder: ${ws.repo}`;
  document.getElementById("row-name").style.display = "none";
  document.getElementById("row-folder").style.display = "none";
  document.getElementById("f-recents").style.display = "none";
  setHarnessChips([ws.harness]); // default to the workspace's harness (tap to add cursor/bash)
  const fmAdd = document.getElementById("f-model");
  if (fmAdd) fmAdd.value = ""; // per-add choice, never sticky
  // 17-01: the add modal starts at role=none (operator picks per added agent).
  setRoleChip("");
  document.getElementById("f-prompt").value = "";
  document.getElementById("f-create").textContent = "Add";
  setCount(1);
  modal.classList.remove("hidden");
  trapModalFocus(modal);
  const firstChip = document.querySelector("#f-harness .count-tile");
  if (firstChip) firstChip.focus();
}
function closeModal() { modal.classList.add("hidden"); releaseModalFocus(modal); }

// Create flow → the 3-step layout wizard (Phase 1). onCreate adds color + remembers
// last-used + spawns; the wizard owns folder/count/per-pane harness. Add-agent stays
// on #modal (openAddAgent), so the old modal path is untouched.
async function launchWizard() {
  let defaultFolder = localStorage.getItem(LS_KEYS.lastFolder) || "";
  if (!defaultFolder && hasTauri()) { try { defaultFolder = await invoke("default_folder"); } catch (_) {} }
  // model-at-spawn: load every harness's LIVE model list (curated floor paints sync
  // inside ensureModelsLoaded; the live enumeration replaces it when it lands — fetched
  // once per session, cached). Fire-and-forget: the wizard opens instantly.
  for (const h of FW_HARNESSES) ensureModelsLoaded(h);
  openWizard({
    onCreate: async (args) => {
      // Ruling D: the chosen folder MUST exist before we spawn — a stale pinned
      // preset.folder (or a typo) would make add_worktree fail and do_spawn fall back
      // to the bare path with NO git-worktree isolation (silent data-loss risk). Throw
      // so doCreate keeps the wizard open and surfaces this in #wiz-error (no spawn).
      if (hasTauri()) {
        let ok = false;
        try { ok = await invoke("path_is_dir", { path: args.repo }); } catch (_) { ok = false; }
        if (!ok) throw new Error("Folder not found: " + args.repo);
      }
      rememberFolder(args.repo);
      localStorage.setItem(LS_KEYS.lastHarness, args.harnesses[0]);
      localStorage.setItem(LS_KEYS.lastCount, String(args.count));
      const color = WS_PALETTE[Object.keys(workspaces).length % WS_PALETTE.length];
      await createWorkspace({ ...args, color });
    },
    defaultFolder,
    defaultHarness: "claude", // always claude (no sticky last-used → no silently-wrong default)
    count: parseInt(localStorage.getItem(LS_KEYS.lastCount), 10) || 1,
    // Scheduler (D33) snapshot for the wizard's overflow hint: schedMax() is the cap,
    // _schedWorking the live `working` count from the last list_queue poll. The wizard
    // never bypasses the cap; it only warns that the overflow will queue.
    cap: schedMax(),
    working: _schedWorking,
    recents: getRecents(),
  });
}
// (#new-btn — the old "WHO NEEDS YOU" header +New — was removed from the markup; the sole
// "+ New" is the WORKSPACES header #ws-new-btn, wired below.)
document.getElementById("f-cancel").onclick = closeModal;

document.querySelectorAll("#f-count .count-tile").forEach((btn) => {
  btn.onclick = () => setCount(parseInt(btn.dataset.n, 10) || 1);
});

async function submitModal() {
  // multi-select harness chips → an ordered set; round-robin across the count so a
  // mixed pick (e.g. claude + cursor) alternates per terminal. ≥1 always (chip handler).
  const harnesses = selectedHarnesses();
  const harness = harnesses[0] || "claude"; // single-harness paths + last-used memory
  const prompt = document.getElementById("f-prompt").value;
  // 17-01: the role from the single-select chip ("" = none → homogeneous pane).
  const role = selectedRole() || undefined;
  // model-at-spawn: the optional model id (the modal is single-harness, so one input).
  const model = (document.getElementById("f-model")?.value || "").trim() || undefined;
  const err = document.getElementById("f-error");
  if (!hasTauri()) { err.textContent = "Tauri API unavailable"; return; }
  // #modal opens ONLY via openAddAgent now (create moved to the wizard) — add-mode only.
  const wsId = modalWsId;
  closeModal();
  document.getElementById("f-prompt").value = "";
  await addAgentsToWorkspace(wsId, { harnesses, count: selectedCount, prompt, role, modelByHarness: model ? { [harness]: model } : undefined });
}
document.getElementById("f-create").onclick = submitModal;
// Enter in the Name or Working-folder field submits (textarea prompt keeps its newline)
["f-name", "f-repo"].forEach((id) => {
  const el = document.getElementById(id);
  if (el) el.addEventListener("keydown", (e) => { if (e.key === "Enter") { e.preventDefault(); submitModal(); } });
});

// Quick Create: spawn a workspace from the last-used folder/harness/count with no
// modal. Falls back to opening the modal if there's no remembered folder yet.
async function quickCreate() {
  if (!hasTauri()) { showToast("Tauri API unavailable"); return; }
  const folder = localStorage.getItem(LS_KEYS.lastFolder);
  if (!folder) { launchWizard(); return; }
  const harness = localStorage.getItem(LS_KEYS.lastHarness) || "claude";
  const count = parseInt(localStorage.getItem(LS_KEYS.lastCount), 10) || 1;
  const name = folder.split("/").filter(Boolean).pop() || "workspace";
  const color = WS_PALETTE[Object.keys(workspaces).length % WS_PALETTE.length];
  gridMode = count > 1;
  await createWorkspace({ name, color, repo: folder, harness, count });
}

// close one pane: kill PTY + remove worktree (backend) + dispose terminal, and
// splice it out of its owning workspace (else relayout() crashes on a stale id).
async function closeWorkspace(id) {
  if (!id) return;
  deadPanes.delete(id); // closing clears the death flag (the id may be reused on reopen)
  _deadHintShown.delete(id); // and re-arms the dead-typing hint for a reused id
  const wsId = paneOwner(id);
  if (hasTauri()) { try { await invoke("close_workspace", { id }); } catch (_) {} }
  const s = sessions[id];
  // free the WebGL context BEFORE term.dispose — xterm disposes loaded addons anyway,
  // but the explicit detach keeps the contexts==visible invariant obvious and survives
  // any addon-manager behavior change.
  if (s) { detachWebgl(s); try { s.term.dispose(); } catch (_) {} s.el.remove(); delete sessions[id]; }
  if (wsId && workspaces[wsId]) {
    const ws = workspaces[wsId];
    ws.paneIds = ws.paneIds.filter((p) => p !== id);
    ws.count = ws.paneIds.length;
    ws.layout = ltree.removeLeaf(ws.layout, id); // sibling inherits the closed pane's box
    if (ws.zoom === id) ws.zoom = null;
    if (ws.paneIds.length === 0) dropLayout(wsId); else persistLayout(ws);
  }
  if (activeId === id) {
    const ws = activeWs ? workspaces[activeWs] : null;
    activeId = ws && ws.paneIds.length ? ws.paneIds[0] : null;
  }
  setActiveLabel(activeId);
  persistWorkspaces();
  renderWorkspaces();
  relayout();
}

// ---- browser pane (iframe preview + capture) ----
let tabs = []; // {id, title, url}
let activeTab = null;
let tabSeq = 0;

function bFrame() { return document.getElementById("b-frame"); }
function bUrlInput() { return document.getElementById("b-url"); }

// localhost / bare host:port → http; everything else → https.
function normalizeUrl(raw) {
  let u = (raw || "").trim();
  if (!u) return "";
  if (!/^https?:\/\//i.test(u)) {
    u = (/^(localhost|127\.|0\.0\.0\.0|\d+\.\d+\.\d+\.\d+|\[)/i.test(u) ? "http://" : "https://") + u;
  }
  return u;
}
function shortLabel(url) {
  try { const u = new URL(url); return (u.host + u.pathname).replace(/\/$/, "") || url; }
  catch (_) { return url; }
}

function getBrowserRecents() {
  try {
    const r = JSON.parse(localStorage.getItem("at_browser_recents") || "null");
    if (Array.isArray(r) && r.length) return r;
  } catch (_) {}
  return ["localhost:5173", "localhost:3000"]; // seed with common dev ports
}
function addBrowserRecent(url) {
  const rec = getBrowserRecents().filter((x) => x !== url);
  rec.unshift(url);
  localStorage.setItem("at_browser_recents", JSON.stringify(rec.slice(0, 8)));
  renderBrowserRecents();
}
function renderBrowserRecents() {
  const box = document.getElementById("b-recents");
  if (!box) return;
  box.replaceChildren();
  for (const r of getBrowserRecents()) {
    const chip = document.createElement("button");
    chip.type = "button";
    chip.className = "recent-chip";
    chip.textContent = r;
    chip.title = r;
    chip.onclick = () => navigate(r);
    box.appendChild(chip);
  }
}

function renderTabs() {
  const strip = document.getElementById("browser-tabs");
  if (!strip) return;
  strip.setAttribute("role", "tablist"); // idempotent — the strip is static markup
  strip.setAttribute("aria-label", "Browser tabs");
  strip.querySelectorAll(".b-tab").forEach((el) => el.remove());
  const newBtn = document.getElementById("b-newtab");
  for (const t of tabs) {
    // a11y: the tab was a click-only <span>. Real <button role="tab"> + aria-selected;
    // the close glyph is its own labeled focusable control (.b-tab-close:focus-visible
    // already styled). Visual classes unchanged.
    const el = document.createElement("button");
    el.type = "button";
    el.className = "b-tab" + (t.id === activeTab ? " active" : "");
    el.setAttribute("role", "tab");
    el.setAttribute("aria-selected", t.id === activeTab ? "true" : "false");
    const title = document.createElement("span");
    title.className = "b-tab-title";
    title.textContent = t.title || "new tab";
    // Not a nested <button> (invalid inside a button): a span with role=button +
    // tabindex + key handling is the labeled, focusable close control.
    const close = document.createElement("span");
    close.className = "b-tab-close";
    close.textContent = "✕";
    close.setAttribute("role", "button");
    close.setAttribute("aria-label", "Close tab");
    close.tabIndex = 0;
    close.onclick = (e) => { e.stopPropagation(); closeTab(t.id); };
    close.addEventListener("keydown", (e) => {
      if (e.key === "Enter" || e.key === " ") { e.preventDefault(); e.stopPropagation(); closeTab(t.id); }
    });
    el.append(title, close);
    el.onclick = () => setActiveTab(t.id);
    strip.insertBefore(el, newBtn);
  }
}

function newTab(url) {
  const id = "t" + tabSeq++;
  tabs.push({ id, title: url ? shortLabel(normalizeUrl(url)) : "new tab", url: url ? normalizeUrl(url) : "" });
  setActiveTab(id);
  if (url) navigate(url);
}
function setActiveTab(id) {
  activeTab = id;
  const t = tabs.find((x) => x.id === id);
  if (t) { bUrlInput().value = t.url || ""; bFrame().src = t.url || "about:blank"; }
  renderTabs();
}
function closeTab(id) {
  const i = tabs.findIndex((x) => x.id === id);
  if (i < 0) return;
  tabs.splice(i, 1);
  if (activeTab === id) {
    const next = tabs[i] || tabs[i - 1] || null;
    if (next) setActiveTab(next.id);
    else { activeTab = null; bUrlInput().value = ""; bFrame().src = "about:blank"; renderTabs(); }
  } else {
    renderTabs();
  }
}

function navigate(raw) {
  const url = normalizeUrl(raw);
  if (!url) return;
  if (!activeTab) { newTab(raw); return; }
  const t = tabs.find((x) => x.id === activeTab);
  if (t) { t.url = url; t.title = shortLabel(url); }
  bUrlInput().value = url;
  bFrame().src = url;
  addBrowserRecent(raw.trim());
  renderTabs();
}

function reloadFrame() {
  const f = bFrame();
  const u = f.src;
  if (!u || u === "about:blank") return;
  f.src = "about:blank";
  requestAnimationFrame(() => { f.src = u; });
}

function currentUrl() {
  const t = activeTab && tabs.find((x) => x.id === activeTab);
  return (t && t.url) || normalizeUrl(bUrlInput().value);
}

function toggleBrowser() {
  const b = document.getElementById("browser");
  const hidden = b.classList.toggle("hidden");
  if (!hidden && tabs.length === 0) newTab(""); // open with one empty tab
  relayout(); // terminals refit to the narrower/wider #main
  if (!hidden) requestAnimationFrame(() => bUrlInput().focus());
}

// small transient toast (bottom-center)
let toastTimer;
function showToast(msg) {
  let t = document.getElementById("toast");
  // role="status" (polite live region) so screen readers announce the transient toast —
  // the element is created dynamically here, so the attribute must ride along.
  if (!t) { t = document.createElement("div"); t.id = "toast"; t.setAttribute("role", "status"); document.body.appendChild(t); }
  t.textContent = msg;
  t.classList.add("show");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => t.classList.remove("show"), 2600);
}

async function doCapture() {
  if (!hasTauri()) { showToast("Tauri API unavailable"); return; }
  try {
    const path = await invoke("capture_region");
    if (path) showToast("Captured → annotate in Preview, then ⌘A ⌘C and paste (⌘V/Ctrl+V) into an agent.");
    else showToast("Capture cancelled.");
  } catch (e) { showToast(String(e)); }
}
async function openExternal() {
  const url = currentUrl();
  if (!url) { showToast("enter a url first"); return; }
  if (!hasTauri()) { showToast("Tauri API unavailable"); return; }
  try { await invoke("open_external", { url }); } catch (e) { showToast(String(e)); }
}

// wire browser controls (the topbar #browser-btn is gone — the More menu's
// data-more="browser" entry is the sole opener now)
const bCloseEl = document.getElementById("b-close");
if (bCloseEl) bCloseEl.onclick = () => toggleBrowser();
const bGoEl = document.getElementById("b-go");
if (bGoEl) bGoEl.onclick = () => navigate(bUrlInput().value);
const bReloadEl = document.getElementById("b-reload");
if (bReloadEl) bReloadEl.onclick = () => reloadFrame();
const bNewTabEl = document.getElementById("b-newtab");
if (bNewTabEl) bNewTabEl.onclick = () => newTab("");
const bCaptureEl = document.getElementById("b-capture");
if (bCaptureEl) bCaptureEl.onclick = () => doCapture();
const bExternalEl = document.getElementById("b-external");
if (bExternalEl) bExternalEl.onclick = () => openExternal();
const bUrlEl = document.getElementById("b-url");
if (bUrlEl) bUrlEl.addEventListener("keydown", (e) => { if (e.key === "Enter") { e.preventDefault(); navigate(bUrlEl.value); } });
renderBrowserRecents();

// ---- Bridge orchestrator (Plan 04-03): one goal → a tailored task per pane ----
// Broadcast (⇉) sends the SAME keystrokes to every pane; the Bridge synthesizes a
// DIFFERENT, focus-aware task per pane (headless claude) and dispatches each to its
// own pane after a read-only preview.
const bridgeEl = document.getElementById("bridge");
let bridgePreview = []; // last synthesized [{id, task}]
// 06-19: planner-assigned roles by pane id. renderBridgePaneRows honors this as an OVERLAY
// so the auto-role pill SURVIVES a re-render — the live supervisor's role is None for an
// auto-planned pane, so without this the pill reverts to "no role" on the next render (e.g.
// after Send / a status repaint). Filled in synthesizeBridge, cleared in resetBridgePreview.
// Display-only — never a write surface (mirrors the request-only `focus`/`role` contract).
let bridgePlannedRoles = {};
let bridgeRunDir = null; // fan-in dir for the last dispatch (<dir>/<pane>.md per agent)
let bridgeGoal = ""; // goal carried from dispatch into result synthesis
// 07-02 auto-synthesis: monitor the dispatched panes' <run>/<id>.md reports and fan-in
// AUTOMATICALLY once they all SETTLE (non-empty + byte-size stable, or dead). Manual stays.
let bridgeAuto = (localStorage.getItem("at_bridge_auto") !== "0"); // default ON
// Bridge→Flywheel UNIFY seam (default OFF — shipped Bridge stays byte-identical when off).
// When ON, a finished synthesis reveals a "Run Flywheel" button that loads the synthesized
// PRD into the (triple-gated) Flywheel goal so the human can ship it as a coordinated PR.
let bridgeUnify = (localStorage.getItem(LS_KEYS.bridgeUnify) === "1");
// Feature toggle (default OFF → orchestration stays byte-identical when off): the eve Team
// Planner auto-assigns per-pane roles ONLY when this is on. Persisted; wired to #br-autoplan.
let bridgeAutoPlan = (localStorage.getItem("at_bridge_autoplan") === "1");
// Provenance: the Team Planner's FULL prompt from the last orchestrate (autoPlan only), carried
// from synthesizeBridge into dispatchBridge so it's persisted as <run_dir>/plan-prompt.md.
let bridgePlanPrompt = null;
let bridgePrdPath = null; // abs path of the last synthesized PRD (carried into the seam)
let bridgeSpawnHarnesses = ["claude"]; // multi-select team harnesses for auto-spawn (round-robin)
let bridgeSpawnCount = 3;              // auto-spawn team size

// ---- P2 unified entry: runner / loop axes (ship = existing bridgeUnify) ----
// runner: 'panes' (default = today's Orchestrate, byte-identical) | 'headless' (delegate/flywheel).
// loop:   off (default) | on (STUB this phase — routes to "coming soon (P3)", no execution).
// Both persisted file-local; defaults keep the modal behaviorally identical to today's Orchestrate.
let bridgeRunner = (localStorage.getItem(LS_KEYS.orchRunner) === "headless") ? "headless" : "panes";
let bridgeLoop   = (localStorage.getItem(LS_KEYS.orchLoop) === "1");
// Headless sub-block state (mirrors the #fw-* / #dl-* widgets the old Delegate/Flywheel modals used).
let brHlWorkers = 3;          // worker count (== old dlWorkers / fwWorkers default)
let brHlHarness = "claude";   // worker harness (validated against FW_HARNESSES, == old fwHarness)
function brHlModelTextEl() { return document.getElementById("br-hl-model-text"); }

// ---- Bridge DOCKED CHAT PANE (bc-*) — the non-blocking "Hermes" Bridge ----
// The phase machine stays the SSOT; the dock is a PROJECTOR: wrappers at the existing
// choke points (setBridgePhase / setPaneState / #br-note / #br-error) emit events into
// the pure chatReduce timeline, and the controller renders state.turns. Flag-gated
// default OFF (at_bridge_chat_dock) — off ⇒ bcEmit no-ops + the modal flow is
// byte-identical. The hidden #bridge modal stays in-DOM as state-holder/fallback.
let bridgeChatDock = (localStorage.getItem("at_bridge_chat_dock") === "1");
let bcState = initialChat();

// ---- Bridge primary-button phase machine (UX redesign) ----
// ONE primary button walks the whole flow; its label tells the user exactly where they are
// and what the next click does. Replaces the old Synthesize / Dispatch / Synthesize Results /
// Run Flywheel button cluster (operator feedback: three blue buttons + silent state changes
// read as "it does nothing").
//   idle       → "Plan tasks →"        (click = orchestrate a per-pane task plan)
//   planning   → disabled, in flight
//   preview    → "Send to team →"      (click = dispatch the previewed tasks)
//   running    → disabled, live "Working — n/m reports…" (poll updates the label)
//   ready      → "Build PRD →"         (click = fan-in synthesize; also the stuck/partial path)
//   collecting → disabled, in flight
//   prd        → "Run Flywheel →"      (ship mode only — hand the PRD to the gated Flywheel)
//   spawn      → hidden (the spawn UI has its own button)
let bridgePhase = "idle";
// Bumped on Cancel / modal reopen to invalidate an in-flight planner: the ~90s Opus planner
// runs async, so a late result from a cancelled/superseded run must NOT flip the modal back to
// a preview or leave it stuck on "Planning…". synthesizeBridge captures the token at start and
// only applies its result + phase if it's still the latest.
let bridgePlanToken = 0;
const BR_PHASE_LABEL = {
  idle: "Plan tasks →",
  planning: "Planning…",
  preview: "Send to team →",
  running: "Working…",
  ready: "Build PRD →",
  collecting: "Building PRD…",
  prd: "Run Flywheel →",
};
function setBridgePhase(phase, label) {
  bridgePhase = phase;
  bcEmit({ type: "phase", phase, label: label || BR_PHASE_LABEL[phase] || phase }); // W1 → dock
  const b = document.getElementById("br-primary");
  if (!b) return;
  if (phase === "spawn") { b.classList.add("hidden"); bcSyncAction(); return; }
  // Leaving "planning" (success, abort, or any transition) stops the elapsed-seconds ticker so it
  // can't keep overwriting the new label. Entering planning re-arms it from synthesizeBridge.
  if (phase !== "planning") stopPlanTicker();
  b.classList.remove("hidden");
  b.textContent = label || BR_PHASE_LABEL[phase] || phase;
  // disabled while in flight (running/collecting) OR while any synthesis is busy — a restored/
  // previewed plan sets phase "preview" (enabled) but an auto PRD synth may still be running; keep
  // the button locked until the synthesizer finishes (re-synced in its finally). NOTE: "planning"
  // is deliberately NOT disabled — the autoPlan Opus call + orchestrate split can run minutes, and
  // a dead-disabled button forced the operator to Cancel (which CLOSES the modal) just to retry.
  // The button stays live during planning as an in-place CANCEL (onclick → abortPlanning), so a
  // re-press never needs the modal closed+reopened.
  b.disabled = phase === "running" || phase === "collecting" || bridgeSynthBusy;
  if (phase !== "prd") b.title = "";
  bcSyncAction(); // the dock's action button mirrors #br-primary exactly (one dispatcher)
}

// Elapsed-seconds ticker for the planning button: the autoPlan Opus call + orchestrate split is
// silent and can run minutes, so a static "Planning…" reads as FROZEN (operator-reported). The
// ticker repaints "Cancel planning · Ns" once a second so the button proves it's alive AND that a
// click cancels. Started by synthesizeBridge, stopped by setBridgePhase on any non-planning phase.
let bridgePlanTimer = null;
function stopPlanTicker() {
  if (bridgePlanTimer) { clearInterval(bridgePlanTimer); bridgePlanTimer = null; }
}
function startPlanTicker() {
  stopPlanTicker();
  const t0 = Date.now();
  const tick = () => {
    if (bridgePhase !== "planning") { stopPlanTicker(); return; }
    const b = document.getElementById("br-primary");
    if (b) b.textContent = `Cancel planning · ${Math.round((Date.now() - t0) / 1000)}s`;
    bcSyncAction();
  };
  tick();
  bridgePlanTimer = setInterval(tick, 1000);
}
// In-place cancel for an in-flight plan: invalidate the running planner (token bump → its late
// result is discarded by synthesizeBridge's guard), drop any preview, return to "Plan tasks". Does
// NOT close the modal — the operator can immediately re-press Plan. The pre-existing #br-cancel
// button still exists (it also closes the modal); this is the lighter "stop, I'll retry" path.
function abortPlanning() {
  stopPlanTicker();
  bridgePlanToken++;
  resetBridgePreview();
  setBridgePhase("idle");
}

// Paste-then-Enter dispatch: a long multi-line task pasted into a TUI with a trailing "\n"
// can land in paste mode and sit UNSUBMITTED in the input line (the operator watched exactly
// that). Send the task, give the TUI a beat to exit paste mode, then a separate Enter.
async function sendTaskSubmit(id, text) {
  await invoke("send_input", { id, data: text });
  await new Promise((r) => setTimeout(r, 350));
  await invoke("send_input", { id, data: "\r" });
}

// Deterministic memory PRIME (backend gate `memory_autoconsult`, default OFF): ask
// the store for the top notes matching THIS task and return a "## Relevant memory"
// block to PREPEND to the built prompt, so the agent gets recalled context before
// acting. "" when the gate is off / store empty / no hits / any error → dispatch is
// byte-identical to today. Search on the RAW task text (not the wrapped prompt) so
// the boilerplate report-protocol never pollutes the ranking.
async function primeTask(task) {
  try {
    if (!task) return "";
    const block = await invoke("memory_prime_task", { task });
    return block || "";
  } catch (_) { return ""; }
}

// Per-pane status chip ("sent" → "working…" → "writing…" → "✓ report" / "✗ dead") so the
// modal answers "is it doing anything?" at a glance.
function setPaneState(id, text, cls) {
  bcEmit({ type: "pane", id, text, cls }); // W2 → dock stays live even with the modal closed
  const el = document.querySelector(`.br-pane-state[data-state-for="${CSS.escape(id)}"]`);
  if (!el) return;
  el.textContent = text;
  el.className = "br-pane-state" + (cls ? " " + cls : "");
}
let bridgeSynthBusy = false;  // a synthesis is in flight (concurrency guard — auto + manual)
let bridgeAutoFails = 0;      // consecutive AUTO synth failures (caps the auto-retry storm)
let bridgePaneIds = [];       // dispatched ids being monitored
let bridgeReadyPrev = {};     // id -> last-seen .md byte size (the size-stability check)
let bridgeDispatchedAt = 0;   // ms; for the stuck-pane timeout fallback
// 07-04 two-wave lifecycle is now AUTO-decided per run (no manual toggle): the orchestrator
// emits a run-level `two_wave` flag (true only when the goal needs ≥2 panes to write+commit
// shared source that must merge+verify together); research/analysis goals get single-wave.
// When ON, only CODE-wave panes dispatch first (commit barrier); once they settle the frontend
// assembles a 3-way-merged integration tree and THEN dispatches the held VERIFY-wave panes
// pointed at it (so QE stops guessing siblings' uncommitted APIs, RC-3). `bridgeTwoWaveAuto` is
// set by synthesizeBridge from the orchestrate envelope and read at dispatch time (like the old
// toggle was). Replaces the old `at_bridge_twowave` localStorage toggle (removed 06-06).
let bridgeTwoWaveAuto = false;
let bridgeVerifyPending = [];  // held verify-wave [{id,task}] awaiting code-wave assembly
let bridgeVerifyDispatched = false; // wave-2 already fired for this run (at-most-once)
let bridgeIntegPath = null;    // abs path of the assembled integration worktree (when ok)
let bridgeAssembling = false;  // assembly in flight (concurrency guard for the wave-1→2 step)
let bridgeCodePaneIds = [];    // the wave-1 code pane ids (used to assemble the integ tree)

// live panes of the active workspace, each with its PER-PANE harness + role (17-01).
// harness/role are keyed by the spawn counter encoded in the pane id (`${wsId}-p${idx}`),
// NOT the filtered `paneIds` position — `ws.harnesses[]`/`ws.roles[]` are counter-indexed
// (spawnPane uses `ws.counter++`) and the close path filters `paneIds` WITHOUT splicing
// them, so a positional lookup would drift after a close. `|| ws.harness` covers a
// pre-harnesses[] workspace; a roleless pane → null (rendered as id · harness + a pill).
function bridgeLivePanes() {
  const ws = activeWs ? workspaces[activeWs] : null;
  if (!ws) return [];
  const harnesses = ws.harnesses || [];
  const roles = ws.roles || [];
  const idxOf = (id) => { const m = /-p(\d+)$/.exec(id); return m ? Number(m[1]) : -1; };
  return ws.paneIds
    // exclude corpses: a dead PTY lingers in `sessions` (markPaneDead only flags
    // deadPanes, never disposes the tile so its last output stays readable), so the
    // sessions check alone would keep listing an agent that /exit'd. dead_pane_ids
    // (1s poll) populates deadPanes, so a reopened Bridge drops the dead pane here.
    .filter((id) => sessions[id] && !deadPanes.has(id))
    .map((id) => {
      const i = idxOf(id);
      return {
        id,
        harness: (i >= 0 && harnesses[i]) || ws.harness,
        role: (i >= 0 ? roles[i] : null) || null,
        model: (i >= 0 ? (ws.models || [])[i] : null) || null, // model-at-spawn (display)
      };
    });
}

// UNIFY: live panes across ALL workspaces (not just activeWs), RESTRICTED to the active
// workspace's repo. The Bridge ideate→PRD→Flywheel flow distributes to every connected
// harness, but `orchestrate` takes ONE repo — a cross-repo pane would recon the wrong tree —
// so we group by the active repo and drop mismatches. Same per-pane harness/role mapping as
// bridgeLivePanes. Used by the Bridge only when the unify ("ship mode") flag is on; default
// Bridge stays activeWs-scoped (byte-identical).
function bridgeAllLivePanes() {
  return allLivePanes({ workspaces, sessions, deadPanes, activeWs });
}

// resets only the SYNTHESIS PREVIEW (not the fan-in run state — see restoreBridgeRun)
// Resets only the SYNTHESIS PREVIEW (the #br-preview task list). It must NOT drop the
// auto-role overlay: openBridge() calls this on every modal OPEN, and the operator reopens
// the modal to watch the grid (and after Cancel) — clearing the overlay there is exactly
// what made the role pills revert to "no role" on reopen / pane-finish. The overlay is
// dropped ONLY when a genuinely fresh plan is about to recompute every role (resetBridgePlan).
function resetBridgePreview() {
  bridgePreview = [];
  document.getElementById("br-preview").classList.add("hidden");
  document.getElementById("br-preview-list").replaceChildren();
}

// Drops the planner's auto-role overlay. Called ONLY right before a new plan is synthesized
// (the roles are about to be recomputed) — never on a bare modal open / cancel / reopen.
function resetBridgePlan() {
  bridgePlannedRoles = {};
}

// ── Plan persistence ───────────────────────────────────────────────────────────────────
// A synthesized plan is EXPENSIVE (a ~minute-long headless planner call — a PAID Bedrock call
// on a Bedrock repo). Cancel/reopen used to DISCARD the previewed plan → the operator had to
// re-synthesize from scratch (caught live: wasted tokens, no UI gate). Persist the previewed
// plan keyed by workspace + goal + live team; openBridge re-offers it (phase "preview") with NO
// re-synth. Editing the goal or synthesizing afresh supersedes it. Mirrors restoreBridgeRun.
const BRIDGE_PLAN_KEY = "at_bridge_plan";
const BRIDGE_PLAN_TTL = 86400000; // 24h (same freshness window as the PRD re-offer)

// Paint the planner's per-task ROLE pill onto the matching pane rows (+ persist the overlay so
// re-renders keep it). Shared by synthesizeBridge (fresh plan) and restoreBridgePlan (re-offer).
function paintPlannedRolePills(preview) {
  for (const d of preview) {
    if (!d.role) continue;
    bridgePlannedRoles[d.id] = d.role;
    const inp = document.querySelector(`#br-panes .br-focus[data-id="${d.id}"]`);
    if (inp) inp.dataset.role = d.role;
    const paneRow = inp && inp.closest(".br-pane-row");
    const pill = paneRow && paneRow.querySelector(".br-role-pill");
    if (pill) { pill.textContent = d.role; pill.classList.remove("empty"); pill.title = `role: ${d.role}`; }
  }
}

// Build the #br-preview task-list DOM + reveal it. Shared by fresh-synth and plan-restore so
// the two paths can never drift in how a previewed plan looks.
function renderBridgePreviewList(preview) {
  const plist = document.getElementById("br-preview-list");
  plist.replaceChildren();
  for (const d of preview) {
    const row = document.createElement("div");
    row.className = "br-task-row";
    const pid = document.createElement("div");
    pid.className = "br-task-id";
    pid.textContent = d.id;
    const rolePill = document.createElement("span");
    rolePill.className = "br-role-pill" + (d.role ? "" : " empty");
    rolePill.textContent = d.role || "no role";
    if (d.role) rolePill.title = `role: ${d.role}`;
    const task = document.createElement("div");
    task.className = "br-task-text";
    task.textContent = d.task;
    row.append(pid, rolePill, task);
    plist.appendChild(row);
  }
  document.getElementById("br-preview").classList.remove("hidden");
}

// Persist the just-previewed plan (called after a successful synthesis). Keyed to the exact
// goal + pane set it was synthesized for, so restore only re-offers it for the SAME request.
function saveBridgePlan(goal, panes) {
  try {
    if (!activeWs || bridgePreview.length === 0) return;
    const paneIds = panes.map((p) => p.id).filter(Boolean).sort();
    localStorage.setItem(BRIDGE_PLAN_KEY, JSON.stringify({
      wsid: activeWs,
      goal: (goal || "").trim(),
      paneIds,
      tasks: bridgePreview,
      twoWave: bridgeTwoWaveAuto,
      roles: bridgePlannedRoles,
      planPrompt: bridgePlanPrompt, // provenance: carry the planner's full prompt through restore
      at: Date.now(),
    }));
  } catch (_) {}
}

function clearBridgePlan() {
  try { localStorage.removeItem(BRIDGE_PLAN_KEY); } catch (_) {}
}

// Re-offer the last previewed plan on open — but ONLY when it still matches: same workspace,
// same goal (the draft restored into #br-goal), same live team, and < TTL old. A mismatch (edited
// goal, changed team, stale) returns false so the operator re-plans. Returns true when restored
// (caller leaves phase at "preview"); never re-runs the costly planner.
function restoreBridgePlan(panes) {
  let saved = null;
  try { saved = JSON.parse(localStorage.getItem(BRIDGE_PLAN_KEY) || "null"); } catch (_) { saved = null; }
  if (!saved || saved.wsid !== activeWs) return false;
  if (Date.now() - (saved.at || 0) > BRIDGE_PLAN_TTL) { clearBridgePlan(); return false; }
  const goalNow = (document.getElementById("br-goal")?.value || "").trim();
  if (!saved.goal || saved.goal !== goalNow) return false; // edited/empty goal → stale, re-plan
  const liveIds = panes.map((p) => p.id).sort();
  const savedIds = Array.isArray(saved.paneIds) ? [...saved.paneIds].sort() : [];
  if (liveIds.length !== savedIds.length || liveIds.some((id, i) => id !== savedIds[i])) return false; // team changed
  const tasks = (saved.tasks || []).filter((d) => d && d.id && d.task);
  if (tasks.length === 0) return false;
  bridgePreview = tasks;
  bridgeTwoWaveAuto = !!saved.twoWave;
  bridgePlannedRoles = saved.roles || {};
  // Provenance: a restored plan must still persist the goal + the planner's full prompt at
  // dispatch (this run dispatched from a restore → plan-prompt.md was missing before this).
  bridgeGoal = saved.goal || "";
  bridgePlanPrompt = saved.planPrompt || null;
  paintPlannedRolePills(bridgePreview);
  renderBridgePreviewList(bridgePreview);
  bcEmit({ type: "plan", tasks: bridgePreview.map((d) => ({
    id: d.id, task: d.task, wave: d.wave || "code",
    ...(d.role != null && d.role !== "" ? { role: d.role } : {}),
  })), twoWave: bridgeTwoWaveAuto });
  const note = document.getElementById("br-note");
  if (note) {
    note.textContent = "Restored your previous plan — no re-synthesis. Send to dispatch, or edit the goal to re-plan.";
    note.classList.remove("hidden");
  }
  setBridgePhase("preview");
  return true;
}

// A dispatched fan-in run survives modal close (agents write for minutes while the
// user watches the grid). Persist it so reopening the Bridge restores the
// "Synthesize Results" affordance instead of silently losing the run.
function restoreBridgeRun() {
  const note = document.getElementById("br-note");
  let saved = null;
  try { saved = JSON.parse(localStorage.getItem(LS_KEYS.bridgeRun) || "null"); } catch (_) { saved = null; }
  // GHOST-RUN GUARD: a saved run whose panes are ALL gone (workspace closed / app restarted /
  // every pane dead) can never finish — restoring it traps the modal in a permanent
  // "0/n stuck" state (operator hit exactly this). Drop it. Exception: if reports were
  // already written before the panes died, partial fan-in is still valuable — the async
  // rescue below re-arms the run in "ready" instead.
  const savedPanes = (saved && Array.isArray(saved.panes)) ? saved.panes : [];
  const anyAlive = savedPanes.some((id) => sessions[id] && !deadPanes.has(id));
  if (saved && saved.dir && !anyAlive) {
    const dir = saved.dir;
    const savedGoal = saved.goal || "";
    try { localStorage.removeItem(LS_KEYS.bridgeRun); } catch (_) {}
    saved = null;
    // async rescue: if reports exist on disk, re-arm the run in "ready" so the human can
    // still fan in what was written before the panes died.
    if (hasTauri() && savedPanes.length) {
      invoke("bridge_ready", { dir }).then((rows) => {
        if ((rows || []).some((r) => r.bytes > 0)) {
          bridgeRunDir = dir;
          bridgeGoal = savedGoal;
          bridgePaneIds = savedPanes;
          setBridgePhase("ready", "Build PRD from what's ready →");
          if (note) { note.textContent = "A previous run's agents are gone, but their reports survived — you can still build the PRD."; note.classList.remove("hidden"); }
        }
      }).catch(() => {});
    }
    if (note) { note.textContent = "Previous run discarded — its agents are no longer running."; note.classList.remove("hidden"); }
  }
  if (saved && saved.dir) {
    bridgeRunDir = saved.dir;
    bridgeGoal = saved.goal || "";
    // restore the monitor state so the readiness poll resumes (an in-flight run may
    // still settle); a restored run is not yet synthesized.
    bridgePaneIds = savedPanes;
    bridgeDispatchedAt = saved.at || Date.now();
    bridgeReadyPrev = {};
    // 07-04: restore the two-wave state so a code wave still in flight resumes its
    // assemble→verify chain (an empty saved.verify means the verify wave already fired).
    if (saved.twoWave) {
      bridgeVerifyPending = Array.isArray(saved.verify) ? saved.verify : [];
      bridgeCodePaneIds = bridgeVerifyPending.length ? bridgePaneIds.slice() : bridgeCodePaneIds;
      bridgeVerifyDispatched = bridgeVerifyPending.length === 0;
    } else {
      bridgeVerifyPending = [];
      bridgeVerifyDispatched = false;
    }
    // NOTE: do NOT reset a concurrency guard here. synthesizeResults clears the live-run
    // state (at_bridge_run / bridgeRunDir) BEFORE its await, so if a synth is in flight
    // `saved` is already gone and we never reach this branch — no double-fire on reopen.
    // Show the run's GOAL (a blank field over a live run read as "lost my run").
    const goalEl = document.getElementById("br-goal");
    if (goalEl && bridgeGoal) goalEl.value = bridgeGoal;
    note.textContent = bridgeAuto
      ? "A dispatch is in progress — the PRD builds automatically when the agents finish."
      : "A dispatch is in progress. The button unlocks when the agents finish writing.";
    note.classList.remove("hidden");
    setBridgePhase("running", `Working — resuming ${bridgePaneIds.length} agent${bridgePaneIds.length === 1 ? "" : "s"}…`);
    showBridgeAutoToggle(true);
  } else if (!bridgeRunDir) {
    bridgePaneIds = [];
    note.classList.add("hidden");
    showBridgeAutoToggle(false);
  }
}

// show/hide + sync the "Auto-synthesize" toggle (only meaningful while a run is live)
function showBridgeAutoToggle(show) {
  const wrap = document.getElementById("br-auto-wrap");
  const box = document.getElementById("br-auto");
  if (box) box.checked = bridgeAuto;
  if (wrap) wrap.classList.toggle("hidden", !show);
  // (07-04 two-wave is now auto-decided per run — no toggle to sync here anymore.)
}

// Build the hidden modal's per-pane rows (id · harness · role pill · focus-pin input ·
// status chip). Extracted from openBridge so the DOCK can seed the same rows —
// synthesizeBridge reads its pane set from EXACTLY these inputs (#br-panes .br-focus),
// so whichever surface is active, the SSOT source stays one place.
function renderBridgePaneRows(panes) {
  const list = document.getElementById("br-panes");
  if (!list) return;
  list.replaceChildren();
  for (const p of panes) {
    const row = document.createElement("div");
    row.className = "br-pane-row";
    const label = document.createElement("span");
    label.className = "br-pane-id";
    label.textContent = `${p.id} · ${p.harness}`; // role moved to its own pill (label truncates)
    label.title = `${p.id} · ${p.harness}` + (p.model ? ` · ${p.model}` : " · account-default model"); // model-at-spawn
    // 17-01: the per-pane ROLE as its own always-legible pill (accent when set, a muted
    // "no role" otherwise so every pane's role slot is visible — the same role the
    // backend honors in orchestrate). The cramped .br-pane-id label was hiding it.
    // effective role = pinned/spawn-time role, ELSE the planner's auto-assigned role (overlay)
    // — so the pill survives a re-render even though the live supervisor's role is None for an
    // auto-planned pane (the "role disappears after Send" fix).
    const effRole = p.role || bridgePlannedRoles[p.id] || "";
    const rolePill = document.createElement("span");
    rolePill.className = "br-role-pill" + (effRole ? "" : " empty");
    rolePill.textContent = effRole || "no role";
    if (effRole) rolePill.title = `role: ${effRole}`;
    const input = document.createElement("input");
    input.className = "br-focus";
    input.dataset.id = p.id;
    input.dataset.harness = p.harness;
    input.dataset.role = effRole; // carry the typed/planned role to synthesizeBridge (→ orchestrate)
    input.placeholder = "focus / role (optional — auto-assigned)";
    input.setAttribute("list", "dl-roles"); // autocomplete the 8 role-library tokens (still free-text for a focus line)
    input.setAttribute("autocomplete", "off");
    input.value = ""; // 07-04: empty by default — the orchestrator infers role+task from the
                      // goal. A typed value is an optional PIN the prompt is told to honor
                      // (and never idle). The old harness-name pre-fill fed `focus=claude`, a
                      // non-signal that biased the model to full-fill every pane.
    // 06-25 fix: editing a focus AFTER a plan was previewed used to be SILENT — dispatchBridge
    // sends the FROZEN bridgePreview, not the live inputs, so a focus typed once the preview is
    // up never reached the team (root cause of "assigned builder, pane got no prompt"). An edit
    // now INVALIDATES the stale plan → the button returns to "Plan tasks", forcing a re-synth that
    // actually feeds the new focus to orchestrate. Fires once (the reset flips phase off "preview";
    // resetBridgePreview keeps the typed value, so the re-plan picks it up).
    input.addEventListener("input", () => {
      if (bridgePhase !== "preview") return;
      resetBridgePreview();
      setBridgePhase("idle");
      showToast("Focus changed — re-plan to apply it to the team.");
    });
    // live per-pane status chip (sent → working → ✓ report / ✗ dead), painted by the
    // readiness poll so the modal answers "is it doing anything?" at a glance.
    const st = document.createElement("span");
    st.className = "br-pane-state";
    st.dataset.stateFor = p.id;
    row.append(label, rolePill, input, st);
    list.appendChild(row);
  }
}

// ---- dock controller: bcEmit → chatReduce → bcScheduleRender ----------------
// bcEmit is the single funnel; a no-op when the flag is off (byte-identical default).
// State ALWAYS reduces (turns must accumulate while the dock is closed) — only the
// PAINT is gated + coalesced.
function bcEmit(event) {
  if (!bridgeChatDock) return;
  try { bcState = chatReduce(bcState, event); } catch (_) { return; }
  bcScheduleRender();
}

// Render gate (perf-2026-06-10, B-plan finding 4): paint only when the dock is
// actually VISIBLE (not merely feature-flagged — setPaneState emits per pane per
// pollBridgeReady tick, and a flag-on/closed dock was full-rebuilding the hidden
// thread N times per tick), and at most ONE bcRender per animation frame. A dock
// opened later repaints the accumulated backlog via openBridgeDock's explicit bcRender.
let _bcRafQueued = false;
function bcScheduleRender() {
  const dock = document.getElementById("bridge-dock");
  if (!dock || dock.classList.contains("hidden")) return;
  if (_bcRafQueued) return;
  _bcRafQueued = true;
  requestAnimationFrame(() => { _bcRafQueued = false; bcRender(); });
}

// Mirror #br-primary onto the dock's action button: same label, same disabled, same
// title — ONE dispatcher (the click is forwarded), so the two surfaces can never drift.
function bcSyncAction() {
  if (!bridgeChatDock) return;
  const a = document.getElementById("bc-action");
  const b = document.getElementById("br-primary");
  if (!a || !b) return;
  const showable = bridgePhase !== "idle" && bridgePhase !== "spawn";
  a.classList.toggle("hidden", !showable);
  a.textContent = b.textContent;
  a.disabled = b.disabled;
  a.title = b.title || "";
  // beacon: pulse while the Bridge is thinking/working
  const dock = document.getElementById("bridge-dock");
  if (dock) dock.classList.toggle("receiving", bridgePhase === "planning" || bridgePhase === "running" || bridgePhase === "collecting");
}

// Render state.turns into #bc-thread. Full rebuild (≤200 turns, 2.5s worst cadence) —
// textContent only (no innerHTML). NEVER calls .focus() (multitasking is the point);
// autoscrolls only when the user is already at the bottom.
function bcRender() {
  const thread = document.getElementById("bc-thread");
  if (!thread) return;
  // a11y: announce new turns to screen readers (the dock is a live conversation surface).
  if (!thread.hasAttribute("aria-live")) thread.setAttribute("aria-live", "polite");
  const atBottom = thread.scrollHeight - thread.scrollTop - thread.clientHeight < 40;
  thread.replaceChildren();
  for (const t of bcState.turns) {
    let el;
    if (t.kind === "user") {
      el = document.createElement("div");
      el.className = "bc-turn you";
      el.textContent = t.text;
    } else if (t.kind === "phase") {
      el = document.createElement("div");
      el.className = "bc-turn bc-typing";
      el.textContent = t.text + " ";
      const dots = document.createElement("span");
      dots.className = "bc-dots";
      el.appendChild(dots);
    } else if (t.kind === "plan") {
      el = document.createElement("div");
      el.className = "bc-card plan";
      const title = document.createElement("div");
      title.className = "bc-card-title";
      title.textContent = `Plan — ${t.tasks.length} task${t.tasks.length === 1 ? "" : "s"}` + (t.twoWave ? " · two-wave" : "");
      el.appendChild(title);
      for (const d of t.tasks) {
        const row = document.createElement("div");
        row.className = "bc-plan-row";
        const pid = document.createElement("span");
        pid.className = "bc-plan-id";
        pid.textContent = d.id;
        const task = document.createElement("span");
        task.className = "bc-plan-task";
        task.textContent = d.task;
        row.append(pid, task);
        // Surface the typed sub-agent role (omnigent envelope) when present.
        const roleBadge = formatRoleBadge(d.role);
        if (roleBadge) {
          const rl = document.createElement("span");
          rl.className = "bc-role";
          rl.textContent = roleBadge;
          row.appendChild(rl);
        }
        if (t.twoWave && d.wave === "verify") {
          const w = document.createElement("span");
          w.className = "bc-wave";
          w.textContent = "verify";
          row.appendChild(w);
        }
        el.appendChild(row);
      }
    } else if (t.kind === "status") {
      el = document.createElement("div");
      el.className = "bc-card";
      for (const [pid, st] of Object.entries(t.panes || {})) {
        const row = document.createElement("div");
        row.className = "bc-pane-row";
        const idEl = document.createElement("span");
        idEl.className = "bc-pane-id";
        idEl.textContent = pid;
        const stEl = document.createElement("span");
        stEl.className = "bc-pane-state" + (st.cls ? " " + st.cls : "");
        stEl.textContent = st.text;
        row.append(idEl, stEl);
        el.appendChild(row);
      }
    } else if (t.kind === "prd") {
      el = document.createElement("div");
      el.className = "bc-card prd";
      const title = document.createElement("div");
      title.className = "bc-card-title";
      title.textContent = "PRD ready";
      // a11y: the clickable path was a mouse-only div. role=button + tabindex + Enter/Space
      // keep the existing .bc-prd-path block styling (a real <button> would need a UA reset).
      const path = document.createElement("div");
      path.className = "bc-prd-path";
      path.textContent = t.path;
      path.title = "Open the PRD";
      path.setAttribute("role", "button");
      path.setAttribute("aria-label", "Open the PRD: " + t.path);
      path.tabIndex = 0;
      const openPrd = () => { if (hasTauri()) invoke("open_external", { url: t.path }).catch(() => {}); };
      path.onclick = openPrd;
      path.addEventListener("keydown", (e) => {
        if (e.key === "Enter" || e.key === " ") { e.preventDefault(); openPrd(); }
      });
      el.append(title, path);
    } else if (t.kind === "dispatched") {
      el = document.createElement("div");
      el.className = "bc-turn bridge";
      el.textContent = t.text;
    } else { // info | error
      el = document.createElement("div");
      el.className = "bc-turn " + (t.kind === "error" ? "bc-error" : "bc-info");
      el.textContent = t.text;
    }
    el.dataset.seq = String(t.seq);
    thread.appendChild(el);
  }
  if (atBottom) thread.scrollTop = thread.scrollHeight;
}

// Composer: a typed line is a GOAL (classifyInput guards the spawn verb). The goal is
// written into the hidden modal's #br-goal (the SSOT field synthesizeBridge reads) and
// the phase machine drives from there — the dock never forks the flow.
async function bcSubmit() {
  const inp = document.getElementById("bc-input");
  const text = (inp?.value || "").trim();
  if (!text) return;
  if (classifyInput(text, FW_HARNESSES) === "spawn") {
    bcEmit({ type: "info", text: "Team spawning from the dock lands in the next slice — for now use the Orchestrate modal's spawn (no panes + Ship mode) or ⌘⇧N." });
    return;
  }
  if (bridgePhase === "planning" || bridgePhase === "running" || bridgePhase === "collecting") {
    bcEmit({ type: "info", text: "A run is in flight — the new goal can go after it settles." });
    return;
  }
  inp.value = "";
  bcEmit({ type: "goal", text });
  const g = document.getElementById("br-goal");
  if (g) g.value = text;
  try { localStorage.setItem(LS_KEYS.bridgeGoalDraft, text); } catch (_) {} // programmatic set skips the input listener
  if (bridgePhase === "preview") { resetBridgePreview(); setBridgePhase("idle"); }
  else if (bridgePhase === "prd" || bridgePhase === "ready") {
    bcEmit({ type: "info", text: "New goal — re-planning (the previous PRD stays in History)." });
    setBridgePhase("idle");
  }
  // make sure the SSOT pane rows exist (the dock may be used without ever opening the modal)
  if (!document.querySelector("#br-panes .br-focus")) {
    renderBridgePaneRows(bridgeUnify ? bridgeAllLivePanes() : bridgeLivePanes());
  }
  synthesizeBridge();
}

function openBridgeDock() {
  const dock = document.getElementById("bridge-dock");
  if (!dock) return;
  dock.classList.remove("hidden");
  bcRender(); // explicit: paint the backlog that accumulated while hidden (bcScheduleRender no-ops on a hidden dock)
  document.getElementById("bridge-btn")?.classList.add("active");
  try { localStorage.setItem(LS_KEYS.bridgeDockOpen, "1"); } catch (_) {}
  const panes = bridgeUnify ? bridgeAllLivePanes() : bridgeLivePanes();
  renderBridgePaneRows(panes); // SSOT source for synthesizeBridge, dock or modal
  bcEmit({
    type: "info",
    text: panes.length
      ? `${panes.length} live pane${panes.length === 1 ? "" : "s"}: ` + panes.map((p) => `${p.id} · ${p.harness}${p.model ? ` · ${p.model}` : ""}${p.role ? ` (${p.role})` : ""}`).join("   ")
      : "No live agents — open a workspace first (dock spawn lands in the next slice).",
  });
  bcSyncAction();
  relayout(); // xterms refit to the narrower #main (the #browser discipline)
  requestAnimationFrame(() => document.getElementById("bc-input")?.focus()); // the ONLY focus the dock ever takes
}

function closeBridgeDock() {
  document.getElementById("bridge-dock")?.classList.add("hidden");
  document.getElementById("bridge-btn")?.classList.remove("active");
  try { localStorage.setItem(LS_KEYS.bridgeDockOpen, "0"); } catch (_) {}
  relayout();
}

function toggleBridgeDock() {
  const dock = document.getElementById("bridge-dock");
  if (!dock) return;
  if (dock.classList.contains("hidden")) openBridgeDock();
  else closeBridgeDock();
}

function openBridge() {
  const err = document.getElementById("br-error");
  if (err) err.textContent = "";
  // P2: restore the runner/loop axes from disk + sync their widgets. setBridgeRunner swaps the
  // panes/headless sub-blocks; for the DEFAULT (panes) it just re-asserts today's view and hands
  // the label back to the phase machine — the rest of openBridge below is the UNCHANGED panes path.
  bridgeRunner = (localStorage.getItem(LS_KEYS.orchRunner) === "headless") ? "headless" : "panes";
  bridgeLoop = (localStorage.getItem(LS_KEYS.orchLoop) === "1");
  const brLoopChk = document.getElementById("br-loop");
  if (brLoopChk) brLoopChk.checked = bridgeLoop;
  document.getElementById("br-loop-config")?.classList.toggle("hidden", !bridgeLoop);
  for (const t of document.querySelectorAll("#br-runner .count-tile")) setChipActive(t, t.dataset.runner === bridgeRunner);
  document.getElementById("br-panes-wrap")?.classList.toggle("hidden", bridgeRunner === "headless");
  document.getElementById("br-headless")?.classList.toggle("hidden", bridgeRunner !== "headless");
  if (bridgeRunner === "headless") {
    // Headless runner: no panes required (ship-on auto-creates a workspace; ship-off errors at submit
    // like the old Delegate). Open the modal with the headless widgets + gate; do NOT walk the
    // pane phase machine. The old #delegate/#flywheel modals stay in-DOM as fallback.
    document.getElementById("br-spawn")?.classList.add("hidden");
    document.getElementById("br-note")?.classList.add("hidden");
    document.getElementById("br-panes")?.replaceChildren();
    {
      let draft = "";
      try { draft = localStorage.getItem(LS_KEYS.bridgeGoalDraft) || ""; } catch (_) {}
      const g = document.getElementById("br-goal"); if (g) g.value = draft;
    }
    initBrHeadless();
    const noPane = !delegateParentId();
    syncBrHeadlessRepoRow();
    if (noPane && hasTauri()) {
      const repoEl = document.getElementById("br-hl-repo");
      if (repoEl && !repoEl.value) { invoke("default_folder").then((d) => { if (repoEl && !repoEl.value) repoEl.value = d; }).catch(() => {}); }
    }
    paintBridgePrimaryLabel();
    refreshBridgeHeadlessGate();
    bridgeEl.classList.remove("hidden");
    trapModalFocus(bridgeEl);
    requestAnimationFrame(() => document.getElementById("br-goal").focus());
    return;
  }
  // ── runner = 'panes' (DEFAULT): the UNCHANGED pre-P2 Orchestrate open path ──
  document.getElementById("br-hl-gate")?.classList.add("hidden");
  paintShipGateWarning(); // ship checked + flywheel flags gated off (last-known payload) → inline warn
  // UNIFY: distribute across every connected harness (all live workspaces on the active repo);
  // default Bridge stays scoped to the active workspace.
  const panes = bridgeUnify ? bridgeAllLivePanes() : bridgeLivePanes();
  if (panes.length === 0) {
    // ideate-from-nothing — reveal the auto-spawn UI instead of bailing. (Was gated on
    // bridgeUnify, which silently broke Orchestrate-from-empty on a fresh webview store
    // where at_bridge_unify is unset → false. showBridgeSpawn is unify-independent, so the
    // guard withheld the spawn UI for no reason; this also fixes stable on first launch.)
    if (document.getElementById("br-spawn")) { showBridgeSpawn(); return; }
    showToast("Open a workspace with at least one agent first");
    return;
  }
  // normal orchestrate view: spawn UI hidden, primary button starts at "Plan tasks".
  document.getElementById("br-spawn")?.classList.add("hidden");
  bridgePlanToken++; // a fresh open invalidates any in-flight planner from a prior open
  setBridgePhase("idle");
  renderBridgePaneRows(panes);
  // GOAL = a persistent DRAFT, not per-open scratch: Cancel→reopen used to blank it,
  // which silently dispatched/synthesized with an empty goal downstream (operator-
  // caught twice). Restore the draft; it clears only when a PRD completes.
  {
    let draft = "";
    try { draft = localStorage.getItem(LS_KEYS.bridgeGoalDraft) || ""; } catch (_) {}
    document.getElementById("br-goal").value = draft;
  }
  bridgePrdPath = null;
  bridgeShipDir = null;
  resetBridgePreview();
  restoreBridgeRun(); // re-arms an in-flight run (phase "running") or discards a ghost
  // Nothing running? Re-offer the last synthesized plan (same goal+team, <24h) so a
  // Cancel/reopen restores the previewed tasks instead of re-running the costly planner.
  if (bridgePhase === "idle") restoreBridgePlan(panes);
  // No live run? Re-offer the LAST PRD (≤24h) — auto-synth usually finishes with the modal
  // closed, and reopening must not lose the result of a finished run. A HELD/REJECT-named
  // doc is still the right Flywheel input on a recon goal (verdict = "bug confirmed, tree
  // unfixed", which is exactly what the fix cycle is for).
  if (bridgeUnify && !bridgeRunDir && bridgePhase === "idle") {
    let prd = null;
    try { prd = JSON.parse(localStorage.getItem(LS_KEYS.bridgePrd) || "null"); } catch (_) { prd = null; }
    if (prd && prd.path && Date.now() - (prd.at || 0) < BRIDGE_PLAN_TTL) {
      bridgePrdPath = prd.path;
      bridgeShipDir = prd.shipDir || null; // PASS+folded runs re-offer "Open PR →"
      bcEmit({ type: "prd", path: prd.path });
      setBridgePhase("prd", bridgeShipDir ? "Open PR →" : undefined);
      paintBridgeFlywheelGate();
      const note = document.getElementById("br-note");
      if (note) {
        note.textContent = bridgeShipDir
          ? "Your last run PASSED with a folded integration tree — review the doc, then Open PR."
          : "Your last run's PRD is ready — review it, then Run Flywheel to ship the fix. (A HELD/REJECT verdict on a recon run means the bug is confirmed and not yet fixed — that's the Flywheel's job.)";
        note.classList.remove("hidden");
      }
    }
  }
  bridgeEl.classList.remove("hidden");
  trapModalFocus(bridgeEl);
  requestAnimationFrame(() => document.getElementById("br-goal").focus());
}

function closeBridge() { bridgeEl.classList.add("hidden"); releaseModalFocus(bridgeEl); }

// UNIFY auto-spawn: when the Bridge opens with no live panes (ideate-from-nothing), show a
// repo + harness-team picker instead of bailing. Mirrors the Flywheel P3 bootstrap but spawns
// a MIXED team (round-robin) so the idea fans across several harnesses.
async function showBridgeSpawn() {
  const spawn = document.getElementById("br-spawn");
  if (!spawn) return;
  document.getElementById("br-panes")?.replaceChildren();
  setBridgePhase("spawn"); // no panes yet — the spawn UI has its own button
  resetBridgePreview();
  const repoEl = document.getElementById("br-repo");
  if (repoEl && !repoEl.value && hasTauri()) { try { repoEl.value = await invoke("default_folder"); } catch (_) {} }
  bridgeSpawnHarnesses = ["claude"];
  bridgeSpawnCount = 3;
  for (const t of spawn.querySelectorAll("#br-count .count-tile")) setChipActive(t, t.dataset.n === String(bridgeSpawnCount));
  // L4b: sync the "Fresh from main" checkbox to the current toggle state on every open.
  const freshEl = document.getElementById("br-fresh-toggle");
  if (freshEl) freshEl.checked = freshFromMainEnabled;
  renderBridgeHarness();
  spawn.classList.remove("hidden");
  bridgeEl.classList.remove("hidden");
  trapModalFocus(bridgeEl); // idempotent — one trap per dialog even when openBridge also ran
  requestAnimationFrame(() => document.getElementById("br-repo")?.focus());
}

// Team harness chips (multi-select; ≥1 always kept). Same palette as the Flywheel picker.
function renderBridgeHarness() {
  const row = document.getElementById("br-harness");
  if (!row) return;
  row.replaceChildren();
  for (const h of FW_HARNESSES) {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "count-tile" + (bridgeSpawnHarnesses.includes(h) ? " active" : "");
    b.setAttribute("aria-pressed", bridgeSpawnHarnesses.includes(h) ? "true" : "false");
    b.textContent = FW_HARNESS_LABEL[h] || h;
    b.onclick = () => {
      const i = bridgeSpawnHarnesses.indexOf(h);
      if (i >= 0) { if (bridgeSpawnHarnesses.length > 1) bridgeSpawnHarnesses.splice(i, 1); }
      else bridgeSpawnHarnesses.push(h);
      renderBridgeHarness();
    };
    row.appendChild(b);
  }
  renderBridgeModels();
}

// model-at-spawn: one optional model input PER SELECTED HARNESS (model ids are
// harness-specific — a single flat input can't fit a mixed claude+codex team). Values
// survive re-renders (chip toggles) so typing then adding a harness doesn't wipe them.
function renderBridgeModels() {
  const box = document.getElementById("br-models");
  if (!box) return;
  const keep = {};
  for (const inp of box.querySelectorAll(".br-model-input")) keep[inp.dataset.h] = inp.value;
  box.replaceChildren();
  for (const h of bridgeSpawnHarnesses) {
    const label = document.createElement("label");
    label.className = "modal-sub";
    label.textContent = `${FW_HARNESS_LABEL[h] || h} model (optional — blank = account default)`;
    const inp = document.createElement("input");
    inp.type = "text";
    inp.className = "fw-model-text br-model-input";
    inp.dataset.h = h;
    inp.setAttribute("list", "dl-models-" + h);
    inp.placeholder = "model id";
    inp.autocomplete = "off";
    inp.spellcheck = false;
    inp.value = keep[h] || "";
    label.appendChild(inp);
    box.appendChild(label);
    ensureModelsLoaded(h); // fills dl-models-<h> (curated floor now, live when it lands)
  }
}
// Read the per-harness model picks: { harness: modelId } (blank inputs omitted).
function bridgeSpawnModels() {
  const map = {};
  for (const inp of document.querySelectorAll("#br-models .br-model-input")) {
    const v = inp.value.trim();
    if (v) map[inp.dataset.h] = v;
  }
  return map;
}

// Spawn the mixed team on the entered repo, poll until ≥1 pane is live (≤8s), then re-render
// the Bridge into its normal orchestrate view (carrying any goal the human already typed).
async function spawnBridgeTeam() {
  const err = document.getElementById("br-error");
  if (err) err.textContent = "";
  if (!hasTauri()) { if (err) err.textContent = "Tauri API unavailable."; return; }
  const repo = (document.getElementById("br-repo")?.value || "").trim();
  if (!repo) { if (err) err.textContent = "Enter a repo folder."; return; }
  let ok = false;
  try { ok = await invoke("path_is_dir", { path: repo }); } catch (_) { ok = false; }
  if (!ok) { if (err) err.textContent = "Folder not found: " + repo; return; }
  const goalStash = document.getElementById("br-goal")?.value || "";
  const btn = document.getElementById("br-spawn-go");
  if (btn) { btn.disabled = true; btn.textContent = "Spawning…"; }
  try {
    const count = bridgeSpawnCount;
    const name = repo.split("/").filter(Boolean).pop() || "workspace";
    const color = WS_PALETTE[Object.keys(workspaces).length % WS_PALETTE.length];
    const harnesses = expandHarnesses(bridgeSpawnHarnesses, count); // round-robin → length == count
    const mByH = bridgeSpawnModels(); // harness → model picks (model-at-spawn)
    const models = harnesses.map((h) => mByH[h] || undefined);
    // L4b: pass freshFromMain when the operator's "Fresh from main" toggle is on. This
    // ensures every Bridge worker's worktree is reset to current main before spawning,
    // eliminating the stale-base RC-2 saga. Default OFF → byte-identical behavior.
    const freshFromMain = freshFromMainEnabled || undefined;
    const wsId = await createWorkspace({ name, color, repo, harnesses, count, models, freshFromMain }); // sets activeWs
    const ids = (workspaces[wsId] && workspaces[wsId].paneIds) || [];
    for (let i = 0; i < 40 && !ids.some((id) => sessions[id]); i++) await new Promise((r) => setTimeout(r, 200));
    if (!ids.some((id) => sessions[id])) {
      if (err) err.textContent = "The team didn't come live (panes may be queued — free a slot or use fewer).";
      return;
    }
    document.getElementById("br-spawn")?.classList.add("hidden");
    openBridge(); // activeWs is the new team → renders the normal orchestrate view
    const g = document.getElementById("br-goal"); if (g) g.value = goalStash; // restore the typed idea
    // one-click flow: a goal was already typed → go straight to planning (the operator
    // shouldn't have to find a second button after "Spawn team").
    if (goalStash.trim()) synthesizeBridge();
  } catch (e) {
    if (err) err.textContent = "Couldn't spawn the team: " + e;
  } finally {
    if (btn) { btn.disabled = false; btn.textContent = "Spawn team & plan →"; }
  }
}

// ---- Delegate (P1.5/P1.7): human fires the AGENT's move — spawn throwaway workers for a goal ----
// FORKED from openBridge but radically simpler: Bridge fans a goal over the panes YOU opened; Delegate
// spawns NEW ephemeral workers (isolated worktrees, swept after) → one merged result. parent_id is a
// LIVE PANE id (sups is keyed by pane id) — NOT activeWs. The webview reaches the backend via
// invoke("delegate", …) like Bridge reaches invoke("orchestrate", …); it never dials the UDS socket.
// Triple-gated default-OFF: a stock build's `delegate` returns a clean refusal, so we READ gate state
// on open and disable the fire button (never fire into an error).
const delegateEl = document.getElementById("delegate");
let dlWorkers = 3; // selected worker count (mirrors the backend max_workers default)

function delegateParentId() {
  if (activeId && sessions[activeId] && !deadPanes.has(activeId)) return activeId;
  const live = bridgeLivePanes(); // excludes corpses
  return live.length ? live[0].id : null;
}

async function openDelegate() {
  if (!delegateEl) return;
  const err = document.getElementById("dl-error");
  if (err) err.textContent = "";
  if (!delegateParentId()) { showToast("Open a workspace with at least one live agent first"); return; }
  const goalEl = document.getElementById("dl-goal");
  if (goalEl) goalEl.value = "";
  dlWorkers = 3;
  for (const t of delegateEl.querySelectorAll("#dl-count .count-tile")) {
    setChipActive(t, t.dataset.n === String(dlWorkers));
  }
  delegateEl.classList.remove("hidden");
  trapModalFocus(delegateEl);
  document.getElementById("delegate-btn")?.classList.add("active");
  await refreshDelegateGate(); // paints #dl-gate + enables/disables #dl-fire from real gate state
  requestAnimationFrame(() => document.getElementById("dl-goal")?.focus());
}

function closeDelegate() {
  if (!delegateEl) return;
  delegateEl.classList.add("hidden");
  releaseModalFocus(delegateEl);
  document.getElementById("delegate-btn")?.classList.remove("active");
}

// §9.3 trusted-repo arm-confirm. A worker execs arbitrary code (cargo build.rs/tests) in the
// parent pane's repo, so the backend REFUSES an unarmed repo. This is the one-time in-app confirm:
// first submit on an unarmed repo WARNS + requires a confirming second click (no window.confirm —
// WKWebView no-op risk); the second click arms it (delegate_trust_repo) and proceeds. Returns true
// when it's safe to fire. Shared by both the Delegate and Flywheel submits.
let armPendingRepo = null;
let armPendingTimer = null;
async function ensureRepoArmed(parentId, errEl) {
  const repo = workspaces[paneOwner(parentId)]?.repo;
  if (!repo) return true; // no git repo → the backend's own "no git repo" refusal handles it
  let trusted = false;
  try { trusted = await invoke("delegate_is_repo_trusted", { path: repo }); }
  catch (_) { return true; } // older backend without the command → don't block
  if (trusted) return true;
  if (armPendingRepo === repo) {
    clearTimeout(armPendingTimer); armPendingRepo = null;
    try { await invoke("delegate_trust_repo", { path: repo }); }
    catch (e) { if (errEl) errEl.textContent = "Couldn't arm repo: " + e; return false; }
    return true;
  }
  armPendingRepo = repo;
  clearTimeout(armPendingTimer);
  armPendingTimer = setTimeout(() => { armPendingRepo = null; }, 6000);
  if (errEl) errEl.textContent = `⚠ ${repo} isn't armed for autonomous runs — workers exec arbitrary code (cargo) here. Click again within 6s to ARM this repo & run.`;
  return false;
}

async function submitDelegate() {
  const err = document.getElementById("dl-error");
  if (err) err.textContent = "";
  const goal = (document.getElementById("dl-goal")?.value || "").trim();
  if (!goal) { if (err) err.textContent = "Enter a goal first."; return; }
  const parentId = delegateParentId();
  if (!parentId) { if (err) err.textContent = "No live parent pane to delegate from."; return; }
  if (!hasTauri()) { if (err) err.textContent = "Tauri API unavailable."; return; }
  if (!(await ensureRepoArmed(parentId, err))) return; // §9.3: one-time arm-confirm per repo
  const fire = document.getElementById("dl-fire");
  if (fire) { fire.disabled = true; fire.textContent = "Delegating…"; }
  try {
    // camelCase args → Tauri maps them to the snake_case Rust params (parent_id/max_workers).
    const res = await invoke("delegate", { parentId, goal, maxWorkers: dlWorkers, depth: 1, workspaceId: paneOwner(parentId) || "", critique: bridgeAutoPlan });
    const runId = res && res.run_id ? res.run_id : null;
    const n = res && res.workers != null ? res.workers : dlWorkers;
    showToast(`Delegated to ${n} worker${n === 1 ? "" : "s"}${runId ? ` · ${runId}` : ""}. Watch them in Delegations.`);
    closeDelegate();
    openDelegations(); // also auto-opens on first event, but open NOW for instant feedback
  } catch (e) {
    if (err) err.textContent = delegateRefusalCopy(String(e));
  } finally {
    if (fire) { fire.disabled = false; fire.textContent = "Delegate →"; }
  }
}

// Map a backend response_code (or raw error) to a one-line human reason.
function delegateRefusalCopy(raw) {
  if (raw.includes("DELEGATE_UNAVAILABLE")) return "Delegation isn't built into this app (needs the delegate-live build). See Settings → Autonomous delegation.";
  if (raw.includes("AUTONOMY_DISABLED")) return "Autonomous delegation is disarmed. Arm it in Settings → Autonomous delegation.";
  if (raw.includes("MUTATIONS_DISABLED")) return "Mutations are disabled in mcp-config.json — delegation can't run.";
  if (raw.includes("DELEGATION_IN_FLIGHT")) return "A delegation is already running — wait for it to finish (one at a time).";
  if (raw.includes("UNKNOWN_WORKSPACE")) return "The parent pane is no longer live — reopen a workspace and retry.";
  return "Delegation failed: " + raw;
}

const dlCountEl = document.getElementById("dl-count");
if (dlCountEl) dlCountEl.addEventListener("click", (e) => {
  const tile = e.target.closest(".count-tile");
  if (!tile) return;
  dlWorkers = parseInt(tile.dataset.n, 10) || 3;
  for (const t of dlCountEl.querySelectorAll(".count-tile")) setChipActive(t, t === tile);
});
const delegateBtnEl = document.getElementById("delegate-btn");
if (delegateBtnEl) delegateBtnEl.onclick = () => openDelegate();
const dlCancelEl = document.getElementById("dl-cancel");
if (dlCancelEl) dlCancelEl.onclick = () => closeDelegate();
const dlFireEl = document.getElementById("dl-fire");
if (dlFireEl) dlFireEl.onclick = () => submitDelegate();
if (delegateEl) delegateEl.addEventListener("mousedown", (e) => { if (e.target === delegateEl) closeDelegate(); });

// ───────────────────────── Flywheel (Phase 0 chassis) ─────────────────────────
// One press = one cycle, riding the SAME `delegate` backend command + triple-gate +
// the `delegate-run-result` completion card. Phase 0 is the DISARMED chassis: it runs
// the audit → synthesize → verdict cycle (live only in an armed delegate-live build);
// the PR step is GUIDED (open it yourself from the result card). Autonomous code-fix +
// auto-PR are the gated next phases — see .paul/analysis/flywheel/PLAN.md. Reuses
// delegateParentId / delegateRefusalCopy / openDelegations so there is ONE backend path.
const flywheelEl = document.getElementById("flywheel");
let fwWorkers = 3;

// Worker harness + model cascade. bash is excluded (no model / no autonomous edit loop). Only
// CLAUDE is live-verifiable AND keeps the token/cost meter (it emits parseable usage) + a tight
// allowlist (commit ✓ / push ✗). The others run on YOUR subscription (the meter goes blind →
// "subscription" label) via a coarser autonomous-commit mode, and are UNVERIFIED → experimental.
const FW_HARNESSES = ["claude", "codex", "cursor", "commandcode", "opencode", "pi", "grok"];
const FW_HARNESS_LABEL = { claude: "Claude", codex: "Codex", cursor: "Cursor", commandcode: "CommandCode", opencode: "OpenCode", pi: "Pi", grok: "Grok Build" };
// Curated quick-pick models per harness. NOT exhaustive + ACCOUNT-SCOPED (ids rot + depend on your
// auth/providers) — the text field is the SOURCE OF TRUTH and accepts anything; "" (the "(default)"
// chip) = the harness account default (no --model / no -m), the SAFEST pick (a stale/unauthed id 400s).
// Deep-verified live on this machine: codex/cursor = plain slug, opencode = provider/model. codex
// `-codex` slugs are API-key-only → omitted (400 on a ChatGPT-auth account).
// opencode: leading "" is the resilient default (NO -m → opencode uses its own authed config;
// e.g. openrouter/… when that is what auth.json has). Explicit provider/model ids below are
// OPT-IN only — github-copilot/* 400s when that provider is not authenticated.
const FW_MODELS = {
  claude: ["claude-haiku-4-5", "claude-sonnet-4-6", "claude-opus-4-8"],
  codex: ["gpt-5.5", "gpt-5.4-mini"],
  cursor: ["composer-2.5-fast", "composer-2.5", "gpt-5.5-high", "auto"],
  commandcode: ["claude-sonnet-4-6", "claude-opus-4-8", "gpt-5.5"],
  opencode: ["", "github-copilot/claude-haiku-4.5", "github-copilot/claude-sonnet-4.5", "github-copilot/gpt-5.4"],
  pi: ["", "anthropic/claude-sonnet-4", "openai/gpt-4o", "google/gemini-2.5-pro"],
  grok: ["grok-4.5"],
};
let fwHarness = "claude";
function fwModelTextEl() { return document.getElementById("fw-model-text"); }

// Dynamic per-harness model lists (probe wf_569a1cf2-619): the backend `list_harness_models` runs
// each harness's REAL enumeration (cursor `cursor-agent models`, commandcode `--list-models`,
// opencode `models`, codex = ~/.codex cache read; claude = none). Curated FW_MODELS tiles stay the
// quick-pick + guaranteed floor; the FULL live list backs the free-text <datalist> autocomplete (so
// cursor's 129 / opencode's 84 ids don't become 129 tiles). Session cache only — NEVER persist to
// disk (state_root is wiped on launch; lists are account/machine-scoped → re-probe per session).
const fwModelCache = {};   // { harness: { source: "dynamic"|"fallback", models: [...] } }
// PER-HARNESS generation tokens: a slow fetch must not paint over a NEWER fetch of the
// SAME harness — but parallel loads of DIFFERENT harnesses must all land. (A single
// global token silently discarded 4 of 5 fetches when the Bridge spawn / wizard loaded
// every harness's list in a loop → datalists stuck at the 3 curated entries, no
// deepseek under commandcode. Operator-caught.)
const fwModelGen = {};     // { harness: n }

// ASYNC fetch-into-cache (modeled on refreshFlywheelGate). Cache hit → just repaint. Never blocks
// the picker: renderFlywheelModels is sync + reads the cache; this fills it then repaints if current.
async function ensureModelsLoaded(harness) {
  if (fwModelCache[harness]) { if (harness === fwHarness) renderFlywheelModels(); fillHarnessDatalist(harness); return; }
  if (!hasTauri()) { fillHarnessDatalist(harness); return; } // dev/web → curated only
  fillHarnessDatalist(harness); // curated floor NOW; live list repaints when it lands
  const gen = (fwModelGen[harness] = (fwModelGen[harness] || 0) + 1);
  let res;
  try { res = await invoke("list_harness_models", { harness }); }
  catch (_) { res = { source: "fallback", models: [] }; } // older backend → curated fallback
  if (gen !== fwModelGen[harness]) return; // a newer fetch of THIS harness superseded us
  fwModelCache[harness] = { source: res && res.source ? res.source : "fallback", models: (res && Array.isArray(res.models)) ? res.models : [] };
  if (harness === fwHarness) renderFlywheelModels();
  fillHarnessDatalist(harness);
}

// ---- model-at-spawn: shared per-harness <datalist>s (dl-models-<h>) ----
// One global datalist per harness backs EVERY model input outside the Flywheel modal
// (add-agent #f-model, Bridge-spawn #br-models inputs, wizard per-pane inputs) — inputs
// reference them via the `list` attribute. Curated FW_MODELS is the synchronous floor;
// ensureModelsLoaded upgrades to the live enumeration when it lands. Created lazily in
// <body> (datalists are invisible).
function ensureHarnessDatalist(h) {
  let dl = document.getElementById("dl-models-" + h);
  if (!dl) {
    dl = document.createElement("datalist");
    dl.id = "dl-models-" + h;
    document.body.appendChild(dl);
  }
  return dl;
}
function fillHarnessDatalist(h) {
  const dl = ensureHarnessDatalist(h);
  const entry = fwModelCache[h];
  const full = (entry && entry.models.length) ? entry.models : (FW_MODELS[h] || []);
  dl.replaceChildren();
  // Skip "" — blank field already means account default (no -m); empty datalist options are useless.
  for (const m of full) {
    if (!m) continue;
    const o = document.createElement("option");
    o.value = m;
    dl.appendChild(o);
  }
}

// ── Shared harness/model picker factory (#E4) ─────────────────────────────────────────────
// The SAME harness-tiles + model-tiles + harness-note picker is rendered in TWO places: the
// Flywheel modal's #fw-* widgets and the unified Orchestrate modal's headless #br-hl-* widgets.
// They were byte-near twins (only the id-prefix + the module-level harness `let` differed), so
// this one factory builds both, keyed by `prefix` (fw- vs br-hl-) plus get/set closures over the
// shared harness state and the harness-specific model-text input getter. DOM + behavior are
// preserved exactly; the fw*/brHl* names below stay as thin aliases so every call site is
// unchanged. Shared FW_HARNESSES / FW_MODELS / FW_HARNESS_LABEL / fwModelCache / ensureModelsLoaded.
function makeHarnessPicker({ prefix, getHarness, setHarness, modelTextEl }) {
  const $ = (suffix) => document.getElementById(prefix + suffix);
  function renderModels() {
    const row = $("model");
    if (!row) return;
    const txt = modelTextEl();
    const cur = txt ? txt.value.trim() : "";
    row.replaceChildren();
    const mk = (id, label) => {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "count-tile" + ((id === "" ? cur === "" : cur === id) ? " active" : "");
      b.setAttribute("aria-pressed", (id === "" ? cur === "" : cur === id) ? "true" : "false");
      b.textContent = label;
      b.onclick = () => { if (txt) txt.value = id; renderModels(); };
      return b;
    };
    // "(default)" → empty model string → supervisor model_args omits -m/--model (account default).
    // FW_MODELS may include "" (opencode) as a documented resilient entry; skip it here so we
    // don't paint two identical default chips.
    row.appendChild(mk("", "(default)"));
    for (const m of (FW_MODELS[getHarness()] || [])) {
      if (!m) continue;
      row.appendChild(mk(m, m));
    }
    // Back the free-text field with a <datalist> of the FULL live list (autocomplete over all real
    // ids without exploding the tile row). Falls back to the curated set when there's no live list.
    const entry = fwModelCache[getHarness()];
    const dl = $("model-list");
    if (dl) {
      dl.replaceChildren();
      const full = (entry && entry.models.length) ? entry.models : (FW_MODELS[getHarness()] || []);
      for (const m of full) {
        if (!m) continue;
        const o = document.createElement("option");
        o.value = m;
        dl.appendChild(o);
      }
    }
    // Honest source badge: live (dynamic enumeration) vs curated (this harness can't list).
    const src = $("model-src");
    if (src) {
      if (entry && entry.source === "dynamic" && entry.models.length) {
        src.textContent = `· ${entry.models.length} live models — type to search`;
      } else if (entry && entry.source === "fallback") {
        src.textContent = "· curated (this harness can't list models) — type any id";
      } else {
        src.textContent = "";
      }
    }
  }
  function paintNote() {
    const note = $("harness-note");
    if (!note) return;
    if (getHarness() === "claude") {
      note.textContent = "— token/cost meter on · commit ✓ / push ✗";
      note.className = "fw-note";
    } else {
      note.textContent = `— experimental · runs on your ${FW_HARNESS_LABEL[getHarness()]} subscription (no per-token meter) · broader commit grant`;
      note.className = "fw-note fw-note-warn";
    }
  }
  function renderHarness() {
    const row = $("harness");
    if (!row) return;
    row.replaceChildren();
    for (const h of FW_HARNESSES) {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "count-tile" + (h === getHarness() ? " active" : "");
      b.setAttribute("aria-pressed", h === getHarness() ? "true" : "false");
      b.textContent = FW_HARNESS_LABEL[h] || h;
      b.onclick = () => {
        if (getHarness() === h) return;
        setHarness(h);
        try { localStorage.setItem(LS_KEYS.fwHarness, h); } catch (_) {} // shared key — last pick sticks
        const txt = modelTextEl(); if (txt) txt.value = ""; // a model id is harness-specific
        renderHarness();
        renderModels();
        paintNote();
        ensureModelsLoaded(h); // fetch this harness's live models (fills tiles' datalist when it lands)
      };
      row.appendChild(b);
    }
  }
  return { renderHarness, renderModels, paintNote };
}

// Flywheel-modal instance + thin aliases (call sites unchanged).
const fwPicker = makeHarnessPicker({
  prefix: "fw-",
  getHarness: () => fwHarness,
  setHarness: (h) => { fwHarness = h; },
  modelTextEl: fwModelTextEl,
});
function renderFlywheelModels() { fwPicker.renderModels(); }
function paintFlywheelHarnessNote() { fwPicker.paintNote(); }
function renderFlywheelHarness() { fwPicker.renderHarness(); }

async function openFlywheel() {
  if (!flywheelEl) return;
  const err = document.getElementById("fw-error");
  if (err) err.textContent = "";
  // P3 hybrid bootstrap: a live agent is NO LONGER required — if none is open, reveal a repo-folder
  // row and the cycle auto-creates a 1-pane workspace there ("plug in an idea + a repo, no setup").
  const noPane = !delegateParentId();
  const repoRow = document.getElementById("fw-repo-row");
  if (repoRow) repoRow.classList.toggle("hidden", !noPane);
  if (noPane) {
    const repoEl = document.getElementById("fw-repo");
    if (repoEl && !repoEl.value && hasTauri()) {
      try { repoEl.value = await invoke("default_folder"); } catch (_) {}
    }
  }
  // keep any prior goal so re-pressing (the per-press loop) can iterate on it.
  fwWorkers = 3;
  for (const t of flywheelEl.querySelectorAll("#fw-count .count-tile")) {
    setChipActive(t, t.dataset.n === String(fwWorkers));
  }
  // restore the last-picked harness (model is a fresh choice each press — ids are harness-specific).
  try { fwHarness = localStorage.getItem(LS_KEYS.fwHarness) || "claude"; } catch (_) { fwHarness = "claude"; }
  if (!FW_HARNESSES.includes(fwHarness)) fwHarness = "claude";
  const mt = fwModelTextEl(); if (mt) mt.value = "";
  renderFlywheelHarness();
  renderFlywheelModels();
  paintFlywheelHarnessNote();
  ensureModelsLoaded(fwHarness); // fire-and-forget: dialog opens instantly, live models fill in
  flywheelEl.classList.remove("hidden");
  trapModalFocus(flywheelEl);
  document.getElementById("flywheel-btn")?.classList.add("active");
  await refreshFlywheelGate(); // paints #fw-gate + enables/disables #fw-fire from real gate state
  requestAnimationFrame(() => document.getElementById("fw-goal")?.focus());
}

function closeFlywheel() {
  if (!flywheelEl) return;
  flywheelEl.classList.add("hidden");
  releaseModalFocus(flywheelEl);
  document.getElementById("flywheel-btn")?.classList.remove("active");
}

async function submitFlywheel() {
  const err = document.getElementById("fw-error");
  if (err) err.textContent = "";
  const goal = (document.getElementById("fw-goal")?.value || "").trim();
  if (!goal) { if (err) err.textContent = "Enter a goal first."; return; }
  if (!hasTauri()) { if (err) err.textContent = "Tauri API unavailable."; return; }
  const fire = document.getElementById("fw-fire");
  // P3 hybrid bootstrap: use the live agent if there is one, else AUTO-CREATE a 1-pane workspace on
  // the entered repo folder and delegate from THAT pane (captured explicitly — never guess a pane
  // from another repo). The new pane may be queued over the cap → poll until it's live (≤8s).
  let parentId = delegateParentId();
  if (!parentId) {
    const repo = (document.getElementById("fw-repo")?.value || "").trim();
    if (!repo) { if (err) err.textContent = "No live agent — enter a repo folder to auto-create a workspace, or open one first."; return; }
    let ok = false;
    try { ok = await invoke("path_is_dir", { path: repo }); } catch (_) { ok = false; }
    if (!ok) { if (err) err.textContent = "Folder not found: " + repo; return; }
    if (fire) { fire.disabled = true; fire.textContent = "Creating workspace…"; }
    try {
      const name = repo.split("/").filter(Boolean).pop() || "workspace";
      const color = WS_PALETTE[Object.keys(workspaces).length % WS_PALETTE.length];
      const wsId = await createWorkspace({ name, color, repo, harness: fwHarness, count: 1 });
      parentId = workspaces[wsId] && workspaces[wsId].paneIds && workspaces[wsId].paneIds[0];
      for (let i = 0; i < 40 && parentId && !sessions[parentId]; i++) {
        await new Promise((r) => setTimeout(r, 200));
      }
    } catch (e) {
      if (err) err.textContent = "Couldn't create a workspace: " + e;
      if (fire) { fire.disabled = false; fire.textContent = "Start cycle →"; }
      return;
    }
    if (!parentId || !sessions[parentId]) {
      if (err) err.textContent = "The auto-created agent didn't come live (it may be queued — free a slot or open a 1-pane workspace manually).";
      if (fire) { fire.disabled = false; fire.textContent = "Start cycle →"; }
      return;
    }
  }
  if (!(await ensureRepoArmed(parentId, err))) { if (fire) { fire.disabled = false; fire.textContent = "Start cycle →"; } return; } // §9.3 arm-confirm
  if (fire) { fire.disabled = true; fire.textContent = "Starting cycle…"; }
  try {
    // Phase 0 rides the SAME delegate command. camelCase → snake_case Rust params.
    // harness/model = the operator's per-run worker choice (blank model → harness account default).
    const model = (fwModelTextEl()?.value || "").trim();
    const res = await invoke("delegate", { parentId, goal, maxWorkers: fwWorkers, depth: 1, harness: fwHarness, model: model || null, workspaceId: paneOwner(parentId) || "", critique: bridgeAutoPlan });
    const runId = res && res.run_id ? res.run_id : null;
    const n = res && res.workers != null ? res.workers : fwWorkers;
    showToast(`Flywheel cycle started — ${n} worker${n === 1 ? "" : "s"}${runId ? ` · ${runId}` : ""}. Watch it in Delegations; the result card lands when it settles.`);
    closeFlywheel();
    openDelegations(); // the verdict/result card surfaces here (shared dgRuns lane)
  } catch (e) {
    if (err) err.textContent = delegateRefusalCopy(String(e)); // same refusal taxonomy
  } finally {
    if (fire) { fire.disabled = false; fire.textContent = "Start cycle →"; }
  }
}

// ── Ship-mode gate visibility (flywheel_apply / flywheel_ship / loop_autonomy) ──
// These fields are OPTIONAL in delegate_gate_status — an older backend omits them entirely.
// Presence-gated rendering: undefined → render nothing (backward-compat), boolean → chip/line.
let lastDelegateGate = null; // last delegate_gate_status payload, for the ship-checkbox warning
// fwGateChips + flywheelPhaseCopy now live in ./flywheel-gate-core.js (pure, unit-tested).
// Inline warning next to the Ship checkbox (#br-unify): checked + either flywheel flag
// EXPLICITLY false → the run silently degrades to report-only; say so where the user armed it.
// Fields absent (older backend) → no warning (we can't know), element stays hidden.
function paintShipGateWarning() {
  const box = document.getElementById("br-unify");
  const label = box ? box.closest("label") : null;
  if (!label) return;
  const g = lastDelegateGate;
  const gatedOff = !!(g && (g.flywheel_apply === false || g.flywheel_ship === false));
  let warn = document.getElementById("br-ship-gate-warn");
  if (!(box.checked && gatedOff)) { if (warn) warn.classList.add("hidden"); return; }
  if (!warn) {
    warn = document.createElement("p");
    warn.id = "br-ship-gate-warn";
    warn.className = "modal-sub fw-note fw-note-warn";
    label.insertAdjacentElement("afterend", warn);
  }
  warn.textContent = "Ship mode is gated off (flywheel_apply/flywheel_ship in mcp-config.json) — run will be report-only";
  warn.classList.remove("hidden");
}

// Paint #fw-gate + gate #fw-fire from the SAME delegate triple-gate (mutations + autonomy + delegate-live).
async function refreshFlywheelGate() {
  if (!hasTauri()) return;
  let g;
  try { g = await invoke("delegate_gate_status"); }
  catch (_) { return; } // older backend → leave UI as-is
  lastDelegateGate = g;
  paintShipGateWarning();
  const ph = document.getElementById("fw-phase"); if (ph) ph.textContent = flywheelPhaseCopy(g);
  const mut = !!(g && g.allow_mutations);
  const live = !!(g && g.delegate_live);
  const armed = !!(g && Number(g.autonomy_ceiling) >= 1);
  const inFlight = !!(g && g.in_flight);
  const ready = mut && live && armed && !inFlight;
  const strip = document.getElementById("fw-gate");
  if (strip) {
    let msg;
    if (ready) msg = "✓ Ready — one press runs a cycle.";
    else if (inFlight) msg = "A run is already going — wait for it to finish (one at a time).";
    else if (!live) msg = "This build can't run the live cycle yet (it needs the delegate-live build).";
    else if (!mut) msg = "Agent actions are off — set \"allow_mutations\": true in mcp-config.json, then reopen.";
    else msg = "Not armed yet — turn on “Autonomous delegation” in Settings to enable this.";
    strip.textContent = msg + fwGateChips(g); // flywheel apply/ship flags, only when the backend sends them
    strip.classList.toggle("ready", ready);
  }
  const fire = document.getElementById("fw-fire");
  if (fire) {
    fire.disabled = !ready;
    fire.title = ready ? "" :
      inFlight ? "A run is already going (one at a time)." :
      !live ? "This build can't run the cycle (needs the delegate-live build)." :
      !mut ? "Mutations are disabled in mcp-config.json." :
      "Autonomous delegation is disarmed — arm it in Settings.";
  }
}

const flywheelBtnEl = document.getElementById("flywheel-btn");
if (flywheelBtnEl) flywheelBtnEl.onclick = () => openFlywheel();
const fwCancelEl = document.getElementById("fw-cancel");
if (fwCancelEl) fwCancelEl.onclick = () => closeFlywheel();
const fwFireEl = document.getElementById("fw-fire");
if (fwFireEl) fwFireEl.onclick = () => submitFlywheel();
const fwCountEl = document.getElementById("fw-count");
if (fwCountEl) fwCountEl.addEventListener("click", (e) => {
  const tile = e.target.closest(".count-tile");
  if (!tile) return;
  fwWorkers = parseInt(tile.dataset.n, 10) || 3;
  for (const t of fwCountEl.querySelectorAll(".count-tile")) setChipActive(t, t === tile);
});
if (flywheelEl) flywheelEl.addEventListener("mousedown", (e) => { if (e.target === flywheelEl) closeFlywheel(); });

// ─────────────────── P2 unified entry: headless sub-block (in #bridge) ───────────────────
// Mirrors the #fw-* harness/model/count widgets that the old Flywheel modal owned, but rendered
// into the #br-hl-* ids inside the unified Orchestrate modal. Reuses FW_HARNESSES / FW_MODELS /
// FW_HARNESS_LABEL / ensureModelsLoaded (the model cache is keyed by harness → shared). No new
// backend; these only collect the {workers, harness, model} the unified submit forwards.
// Headless (#br-hl-*) instance of the SAME picker factory (#E4). Thin aliases keep call sites
// (initBrHeadless, the onclick chain) unchanged.
const brHlPicker = makeHarnessPicker({
  prefix: "br-hl-",
  getHarness: () => brHlHarness,
  setHarness: (h) => { brHlHarness = h; },
  modelTextEl: brHlModelTextEl,
});
function renderBrHlHarness() { brHlPicker.renderHarness(); }
function renderBrHlModels() { brHlPicker.renderModels(); }
function paintBrHlHarnessNote() { brHlPicker.paintNote(); }
// Initialize the headless harness/model widgets (called from openBridge). Restores the last-picked
// harness (shared key with Flywheel), renders tiles, kicks the live-model fetch.
function initBrHeadless() {
  try { brHlHarness = localStorage.getItem(LS_KEYS.fwHarness) || "claude"; } catch (_) { brHlHarness = "claude"; }
  if (!FW_HARNESSES.includes(brHlHarness)) brHlHarness = "claude";
  brHlWorkers = 3;
  for (const t of document.querySelectorAll("#br-hl-count .count-tile")) setChipActive(t, t.dataset.n === String(brHlWorkers));
  const mt = brHlModelTextEl(); if (mt) mt.value = "";
  renderBrHlHarness();
  renderBrHlModels();
  paintBrHlHarnessNote();
  ensureModelsLoaded(brHlHarness);
}
// Headless count tiles.
const brHlCountEl = document.getElementById("br-hl-count");
if (brHlCountEl) brHlCountEl.addEventListener("click", (e) => {
  const tile = e.target.closest(".count-tile");
  if (!tile) return;
  brHlWorkers = parseInt(tile.dataset.n, 10) || 3;
  for (const t of brHlCountEl.querySelectorAll(".count-tile")) setChipActive(t, t === tile);
});

// Paint #br-hl-gate + gate #br-primary (headless runner only) from the SAME delegate triple-gate.
// Mirrors refreshFlywheelGate; only runs when the headless block is showing.
async function refreshBridgeHeadlessGate() {
  const strip = document.getElementById("br-hl-gate");
  if (bridgeRunner !== "headless") { if (strip) strip.classList.add("hidden"); return; }
  if (!hasTauri()) return;
  let g;
  try { g = await invoke("delegate_gate_status"); } catch (_) { return; }
  lastDelegateGate = g;
  paintShipGateWarning();
  const mut = !!(g && g.allow_mutations);
  const live = !!(g && g.delegate_live);
  const armed = !!(g && Number(g.autonomy_ceiling) >= 1);
  const inFlight = !!(g && g.in_flight);
  const ready = mut && live && armed && !inFlight;
  if (strip) {
    strip.classList.remove("hidden");
    let msg;
    if (ready) msg = "✓ Ready — headless workers will run.";
    else if (inFlight) msg = "A run is already going — wait for it to finish (one at a time).";
    else if (!live) msg = "This build can't run headless workers yet (it needs the delegate-live build).";
    else if (!mut) msg = "Agent actions are off — set \"allow_mutations\": true in mcp-config.json, then reopen.";
    else msg = "Not armed yet — turn on “Autonomous delegation” in Settings to enable this.";
    strip.textContent = msg + fwGateChips(g); // flywheel apply/ship flags, only when the backend sends them
    strip.classList.toggle("ready", ready);
  }
}

// ─────────────────── P2 unified submit (collapses submitDelegate + submitFlywheel) ───────────────────
// ONE parameterized headless submit. Builds the SAME invoke('delegate', {...}) arg objects the old
// two functions built, parameterized by {runner, ship, loop, workers, harness, model, critique}.
//   ship=false → report-only = OLD submitDelegate arg shape (NO harness/model field), requires a live pane.
//   ship=true  → flywheel armed = OLD submitFlywheel arg shape (adds harness + model||null), auto-creates a
//                1-pane workspace from #br-hl-repo when no pane is live (the Flywheel P3 bootstrap).
// critique:bridgeAutoPlan in BOTH — byte-identical to the old code. depth:1 + workspaceId:paneOwner||"" in both.
async function submitUnifiedHeadless() {
  const err = document.getElementById("br-error");
  if (err) err.textContent = "";
  const goal = (document.getElementById("br-goal")?.value || "").trim();
  if (!goal) { if (err) err.textContent = "Enter a goal first."; return; }
  if (!hasTauri()) { if (err) err.textContent = "Tauri API unavailable."; return; }
  const ship = !!bridgeUnify;
  const btn = document.getElementById("br-primary");
  // headless restore: repaint the ship-driven label (Delegate/Start cycle), NOT the panes phase label.
  const restore = () => { if (btn) { btn.disabled = false; paintBridgePrimaryLabel(); } };

  let parentId = delegateParentId();
  if (ship) {
    // Flywheel path: no live pane → auto-create a 1-pane workspace from the repo folder (P3 bootstrap).
    if (!parentId) {
      const repo = (document.getElementById("br-hl-repo")?.value || "").trim();
      if (!repo) { if (err) err.textContent = "No live agent — enter a repo folder to auto-create a workspace, or open one first."; return; }
      let ok = false;
      try { ok = await invoke("path_is_dir", { path: repo }); } catch (_) { ok = false; }
      if (!ok) { if (err) err.textContent = "Folder not found: " + repo; return; }
      if (btn) { btn.disabled = true; btn.textContent = "Creating workspace…"; }
      try {
        const name = repo.split("/").filter(Boolean).pop() || "workspace";
        const color = WS_PALETTE[Object.keys(workspaces).length % WS_PALETTE.length];
        const wsId = await createWorkspace({ name, color, repo, harness: brHlHarness, count: 1 });
        parentId = workspaces[wsId] && workspaces[wsId].paneIds && workspaces[wsId].paneIds[0];
        for (let i = 0; i < 40 && parentId && !sessions[parentId]; i++) { await new Promise((r) => setTimeout(r, 200)); }
      } catch (e) {
        if (err) err.textContent = "Couldn't create a workspace: " + e;
        restore(); return;
      }
      if (!parentId || !sessions[parentId]) {
        if (err) err.textContent = "The auto-created agent didn't come live (it may be queued — free a slot or open a 1-pane workspace manually).";
        restore(); return;
      }
    }
  } else {
    // Report-only path = old Delegate: a live parent pane is REQUIRED (no auto-create).
    if (!parentId) { if (err) err.textContent = "No live parent pane to delegate from."; return; }
  }
  if (!(await ensureRepoArmed(parentId, err))) { restore(); return; } // §9.3 arm-confirm (shared)
  if (btn) { btn.disabled = true; btn.textContent = ship ? "Starting cycle…" : "Delegating…"; }
  try {
    // BUILD THE EXACT OLD ARG OBJECTS — field-for-field:
    //   ship=false → submitDelegate's:  { parentId, goal, maxWorkers, depth:1, workspaceId, critique }
    //   ship=true  → submitFlywheel's:  { parentId, goal, maxWorkers, depth:1, harness, model:model||null, workspaceId, critique }
    let args;
    if (ship) {
      const model = (brHlModelTextEl()?.value || "").trim();
      args = { parentId, goal, maxWorkers: brHlWorkers, depth: 1, harness: brHlHarness, model: model || null, workspaceId: paneOwner(parentId) || "", critique: bridgeAutoPlan };
    } else {
      args = { parentId, goal, maxWorkers: brHlWorkers, depth: 1, workspaceId: paneOwner(parentId) || "", critique: bridgeAutoPlan };
    }
    const res = await invoke("delegate", args);
    const runId = res && res.run_id ? res.run_id : null;
    const n = res && res.workers != null ? res.workers : brHlWorkers;
    showToast(ship
      ? `Flywheel cycle started — ${n} worker${n === 1 ? "" : "s"}${runId ? ` · ${runId}` : ""}. Watch it in Delegations; the result card lands when it settles.`
      : `Delegated to ${n} worker${n === 1 ? "" : "s"}${runId ? ` · ${runId}` : ""}. Watch them in Delegations.`);
    closeBridge();
    openDelegations();
  } catch (e) {
    if (err) err.textContent = delegateRefusalCopy(String(e)); // same refusal taxonomy
  } finally {
    restore();
  }
}

// ─────────────────── P3 LOOPS: save-from-modal + Run-Now ───────────────────
// Build a LoopConfig from the current Orchestrate modal (goal / runner / ship / harness /
// workers + the #br-loop-config widgets) and call loop_create (server mints the id), then
// loop_run_now to enqueue ONE immediate manual iteration. Mirrors the SHARED CONTRACT shape
// verbatim — every field optional backend-side (#[serde(default)]), so we only send what the
// modal knows. Manual schedule only this phase.
function readLoopStopFromModal() {
  const kind = document.getElementById("br-loop-stop")?.value || "until_pass";
  let n = parseInt(document.getElementById("br-loop-maxiters")?.value || "3", 10);
  if (!Number.isFinite(n) || n < 1) n = 3;
  if (n > 20) n = 20;
  // EVERY StopCondition variant carries max_iters (CONTRACT §3.1).
  if (kind === "max_iters") return { kind: "max_iters", max_iters: n };
  if (kind === "goal_met") return { kind: "goal_met", check_goal: "", max_iters: n };
  return { kind: "until_pass", max_iters: n };
}
function bridgeLoopConfigFromModal() {
  const goal = (document.getElementById("br-goal")?.value || "").trim();
  const runner = "delegate"; // P3: loops run headless (delegate); bridge-runner loops are P4+
  const model = (brHlModelTextEl()?.value || "").trim();
  const parentId = delegateParentId();
  const wsId = paneOwner(parentId) || activeWs || "";
  const repo = (workspaces[wsId] && workspaces[wsId].repo) || (document.getElementById("br-hl-repo")?.value || "").trim();
  const concurrency = document.getElementById("br-loop-concurrency")?.value === "parallel" ? "parallel" : "serialized";
  const name = goal ? (goal.length > 60 ? goal.slice(0, 57) + "…" : goal) : "Untitled loop";
  return {
    name,
    goal,
    repo,
    workspace_id: wsId,
    runner,                                  // "bridge" | "delegate"
    ship: !!bridgeUnify,
    harness: brHlHarness,
    model: model || null,
    workers: brHlWorkers,
    concurrency,                             // "serialized" | "parallel"
    preflight: { repo_map: true, serena: false },
    merge_target: "loop-integration",
    schedule: { kind: "manual" },            // P3: Manual only
    stop: readLoopStopFromModal(),
    gates_required: [],
    enabled: true,
  };
}
async function submitBridgeLoop() {
  const err = document.getElementById("br-error");
  if (err) err.textContent = "";
  const goal = (document.getElementById("br-goal")?.value || "").trim();
  if (!goal) { if (err) err.textContent = "Enter a goal first."; return; }
  if (!hasTauri()) { if (err) err.textContent = "Tauri API unavailable."; return; }
  const btn = document.getElementById("br-primary");
  const cfg = bridgeLoopConfigFromModal();
  if (btn) { btn.disabled = true; btn.textContent = "Saving loop…"; }
  let id = null;
  try {
    id = await invoke("loop_create", { cfg });   // server mints + returns the id
  } catch (e) {
    if (err) err.textContent = "Couldn't save the loop: " + String(e);
    if (btn) { btn.disabled = false; paintBridgePrimaryLabel(); }
    return;
  }
  // Fire ONE immediate manual iteration. A run-now failure shouldn't lose the saved loop.
  try {
    await invoke("loop_run_now", { id });
    showToast(`Loop “${cfg.name}” saved and started — watch iterations in Runs.`);
  } catch (e) {
    showToast(`Loop “${cfg.name}” saved, but Run-Now failed: ${String(e)}`);
  } finally {
    if (btn) { btn.disabled = false; paintBridgePrimaryLabel(); }
  }
  closeBridge();
  if (loopsEl && !loopsEl.classList.contains("hidden")) renderLoops();
}

// P1.7: read the delegate triple-gate (C3) and paint BOTH the Delegate-modal strip and the Settings
// arming readout. delegate_live is the compile-time cargo feature (cfg!), reported by the backend.
async function refreshDelegateGate() {
  if (!hasTauri()) return;
  let g;
  try { g = await invoke("delegate_gate_status"); }
  catch (_) { return; } // command missing (older backend) → leave UI as-is
  lastDelegateGate = g;
  paintShipGateWarning();
  const mut = !!(g && g.allow_mutations);
  const live = !!(g && g.delegate_live);
  const armed = !!(g && Number(g.autonomy_ceiling) >= 1);
  const inFlight = !!(g && g.in_flight);
  const ok = (b) => (b ? "✓" : "✗");

  const st = document.getElementById("dl-gate-status");
  if (st) st.textContent =
    `mutations ${ok(mut)}   ·   autonomy ${armed ? "armed" : "off"}   ·   delegate-live ${ok(live)}` +
    fwGateChips(g) + // flywheel apply/ship flags, only when the backend sends them
    (inFlight ? "   ·   a delegation is running" : "");
  const armToggle = document.getElementById("dl-arm-toggle");
  if (armToggle) armToggle.checked = armed;

  // agent→agent send-input arm (narrow axis, independent of the autonomy/mutations gates above).
  const si = !!(g && g.send_input_enabled);
  const siToggle = document.getElementById("si-arm-toggle");
  if (siToggle) siToggle.checked = si;
  const siStat = document.getElementById("si-gate-status");
  if (siStat) siStat.textContent =
    `agent→agent send-input ${si ? "ARMED" : "off"}` +
    "   ·   coordinator-only · single-line · live-target";

  const strip = document.getElementById("dl-gate");
  if (strip) {
    // Plain status — say what's READY or what's blocking, not "gates — mutations ✓ · …" jargon.
    let msg;
    if (mut && live && armed && !inFlight) msg = "✓ Ready to delegate.";
    else if (inFlight) {
      const a = g && g.active;
      if (a && a.stale) msg = `⚠ A run looks stuck (no progress ${Math.floor((a.idle_ms || 0) / 60000)}m) — clear it from the progress chip (bottom-right).`;
      else if (a) msg = `A delegation is running — ${a.phase || "working"} · ${fmtElapsed(a.elapsed_ms)} (one at a time).`;
      else msg = "A delegation is already running — wait for it to finish (one at a time).";
    }
    else if (!live) msg = "This build can't delegate yet (it needs the delegate-live build).";
    else if (!mut) msg = "Agent actions are off — set \"allow_mutations\": true in mcp-config.json, then reopen.";
    else msg = "Not armed yet — turn on “Autonomous delegation” in Settings to enable this.";
    strip.textContent = msg;
    strip.classList.toggle("ready", mut && live && armed && !inFlight);
  }
  const fire = document.getElementById("dl-fire");
  if (fire) {
    const ready = mut && live && armed && !inFlight;
    fire.disabled = !ready;
    fire.title = ready ? "" :
      inFlight ? "A delegation is already running (one at a time)." :
      !live ? "This build can't delegate (needs the delegate-live build)." :
      !mut ? "Mutations are disabled in mcp-config.json." :
      "Autonomous delegation is disarmed — arm it in Settings.";
  }
}

// ── Background headless-run HUD (persistent progress chip) ────────────────────────────────────────
// A delegate/flywheel run is a DETACHED backend controller — once you close the modal there is no sign
// it is still working. This chip (bottom-right, app-wide) shows it is alive: the current phase + a live
// elapsed timer + an animated bar. CRITICALLY, it also surfaces a STUCK run: a hung controller never
// runs its RAII release, so the single-flight lock would jam silently and every new run would bounce —
// the chip flips to a "no progress for Nm" warning with a human Force-clear, so recovery no longer means
// restarting the whole app. Polls `delegate_gate_status` (1.2s while live for the timer, 4s when idle).
let _dlHudTimer = null;
function dlHudEl() {
  let h = document.getElementById("dl-hud");
  if (!h) {
    h = document.createElement("div");
    h.id = "dl-hud";
    h.className = "dl-hud hidden";
    // a11y: the HUD is the only live sign a detached run is alive — announce its phase
    // changes politely; the stall/stuck warning flips to assertive below (in pollDelegateHud).
    h.setAttribute("role", "status");
    h.setAttribute("aria-live", "polite");
    document.body.appendChild(h);
  }
  return h;
}
let _dlHudWasActive = false; // active→hidden edge → announce completion (the run finishes silently otherwise)
function fmtElapsed(ms) {
  const s = Math.max(0, Math.floor((ms || 0) / 1000));
  const m = Math.floor(s / 60);
  return m > 0 ? `${m}m ${String(s % 60).padStart(2, "0")}s` : `${s}s`;
}
async function pollDelegateHud() {
  let g = null;
  if (hasTauri()) { try { g = await invoke("delegate_gate_status"); } catch (_) {} }
  const a = g && g.active;
  const h = dlHudEl();
  if (a) {
    const stale = !!a.stale;
    h.classList.remove("hidden");
    h.classList.toggle("stale", stale);
    // a stuck run is urgent — bump the live region to assertive so SRs interrupt; a
    // healthy phase update stays polite.
    h.setAttribute("aria-live", stale ? "assertive" : "polite");
    _dlHudWasActive = true;
    h.replaceChildren(); // clear (all content below is set via textContent / createElement — no HTML injection)
    const bar = document.createElement("div"); bar.className = "dl-hud-bar";
    const fill = document.createElement("div"); fill.className = "dl-hud-fill"; bar.appendChild(fill);
    const label = document.createElement("div"); label.className = "dl-hud-label";
    label.textContent = stale
      ? `⚠ No progress for ${Math.floor((a.idle_ms || 0) / 60000)}m — run may be stuck`
      : `⟳ ${(a.phase || "working").toString()}`;
    const meta = document.createElement("div"); meta.className = "dl-hud-meta";
    meta.textContent = `${(a.run_id || "").replace(/^delegate-/, "run ")} · ${fmtElapsed(a.elapsed_ms)}`;
    h.appendChild(bar); h.appendChild(label); h.appendChild(meta);
    if (stale) {
      const btn = document.createElement("button");
      btn.className = "dl-hud-clear";
      btn.textContent = "Force clear";
      btn.onclick = forceClearInFlight;
      h.appendChild(btn);
    }
  } else {
    h.classList.add("hidden");
    h.classList.remove("stale");
    // active→hidden edge: the run just finished. It was silent before — announce it once.
    if (_dlHudWasActive) { _dlHudWasActive = false; showToast("Background run finished."); }
  }
  clearTimeout(_dlHudTimer);
  _dlHudTimer = setTimeout(pollDelegateHud, a ? 1200 : 4000);
}
// Operator escape hatch for a stuck single-flight lock. The backend REFUSES unless the run is provably
// stale (no heartbeat past its threshold), so this can never clear a healthy live run.
async function forceClearInFlight() {
  if (!hasTauri()) { showToast("Tauri API unavailable"); return; }
  try {
    const r = await invoke("delegate_force_clear_in_flight");
    if (r && r.cleared) showToast("Cleared the stuck run lock — you can start a new run now.");
    else showToast((r && r.detail) || "Nothing to clear.");
  } catch (e) { showToast("Force-clear failed: " + String(e)); }
  pollDelegateHud();
  refreshDelegateGate();
}

// Two-step ARM (no window.confirm — wry/WKWebView may no-op it). Checking the box REVEALS an inline
// confirm; UNchecking disarms immediately (the safe direction, no confirm needed).
function hideArmConfirm() { document.getElementById("dl-arm-confirm")?.classList.add("hidden"); }
function onArmToggle(checked) {
  if (checked) document.getElementById("dl-arm-confirm")?.classList.remove("hidden");
  else { hideArmConfirm(); armSetAutonomy(false); }
}
// The ONE write (C4): flip autonomy_ceiling only — NEVER allow_mutations (blast radius beyond delegate).
async function armSetAutonomy(arm) {
  hideArmConfirm();
  if (!hasTauri()) { showToast("Tauri API unavailable"); await refreshDelegateGate(); return; }
  try {
    await invoke("delegate_set_autonomy", { ceiling: arm ? 1 : 0 });
    showToast(arm ? "Autonomous delegation armed (autonomy_ceiling = 1)." : "Autonomous delegation disarmed.");
  } catch (e) { showToast(String(e)); }
  await refreshDelegateGate(); // re-paint from backend truth (reverts the toggle if the write failed)
}

// agent→agent send-input arm (narrow axis). Mirrors the autonomy arm: turning ON reveals a
// confirm step; turning OFF disarms immediately (safe direction).
function siHideConfirm() { document.getElementById("si-arm-confirm")?.classList.add("hidden"); }
function onSendInputToggle(checked) {
  if (checked) document.getElementById("si-arm-confirm")?.classList.remove("hidden");
  else { siHideConfirm(); setSendInputEnabled(false); }
}
async function setSendInputEnabled(enabled) {
  siHideConfirm();
  if (!hasTauri()) { showToast("Tauri API unavailable"); await refreshDelegateGate(); return; }
  try {
    await invoke("set_send_input_enabled", { enabled });
    showToast(enabled ? "Agent→agent send-input ARMED (coordinator-only)." : "Agent→agent send-input disarmed.");
  } catch (e) { showToast(String(e)); }
  await refreshDelegateGate(); // re-paint from backend truth (reverts the toggle if the write failed)
}

async function synthesizeBridge() {
  const err = document.getElementById("br-error");
  err.textContent = "";
  const goal = document.getElementById("br-goal").value.trim();
  if (!goal) { err.textContent = "Enter a goal first."; return; }
  // re-read the listed panes from the focus inputs, then keep only those still alive.
  // A listed pane is NOT live if its session was disposed (closed → delete sessions[id])
  // OR its PTY child exited (the 1s poll's dead_pane_ids sweep added it to deadPanes —
  // a corpse lingers in `sessions` until close, so the deadPanes check is what catches an
  // agent that /exit'd while the modal was open). The modal may have listed a pane as
  // live at open, then it died before Synthesize. Orchestrating only the survivors
  // silently makes the team SMALLER than shown (N rows, fewer tasks) — reads as a dispatch
  // bug. WARN, naming the dropped ids, then proceed with the live ones.
  const isLive = (p) => sessions[p.id] && !deadPanes.has(p.id);
  const listed = [...document.querySelectorAll("#br-panes .br-focus")]
    .map((i) => ({ id: i.dataset.id, harness: i.dataset.harness, role: i.dataset.role || null, focus: i.value.trim() || null }));
  const panes = listed.filter(isLive);
  if (panes.length === 0) { err.textContent = "No live panes to orchestrate."; return; }
  const dropped = listed.filter((p) => !isLive(p)).map((p) => p.id);
  if (dropped.length) {
    showToast(`${dropped.length} of ${listed.length} panes not live (${dropped.join(", ")}) — re-spawn to include them. Orchestrating the ${panes.length} live one${panes.length === 1 ? "" : "s"}.`);
  }
  if (!hasTauri()) { err.textContent = "Tauri API unavailable."; return; }
  setBridgePhase("planning");
  resetBridgePreview();
  resetBridgePlan(); // a fresh plan recomputes every role → drop the prior overlay first
  const myToken = ++bridgePlanToken; // Cancel/reopen invalidates this run → a late result is discarded
  startPlanTicker(); // live "Cancel planning · Ns" so the wait never reads as frozen
  // Watchdog: the backend self-kills at ~plan(90s)+orchestrate(300s); if the IPC itself never
  // settles (lost reply, reaped-but-not-resolved), nothing would reset the phase. Bound it well
  // ABOVE the backend ceiling so this only catches a dead IPC, never a slow-but-healthy plan.
  const watchdog = setTimeout(() => {
    if (myToken !== bridgePlanToken) return; // already settled/aborted → no-op
    bridgePlanToken++; // invalidate the in-flight call so its late reply can't flip the UI
    bridgePreview = [];
    err.textContent = "Planning timed out — try again.";
    setBridgePhase("idle");
  }, 420000);
  let planned = false;
  try {
    // 17-01: pass the active workspace's repo so a Scout/Coordinator synthesis pass
    // runs ROOTED at the target repo (not $HOME) and can actually read it. Optional —
    // a missing repo falls back to $HOME (today's behavior).
    const orchRepo = (activeWs && workspaces[activeWs] && workspaces[activeWs].repo) || undefined;
    const dispatch = await invoke("orchestrate", { panes, goal, repo: orchRepo, autoPlan: bridgeAutoPlan });
    if (myToken !== bridgePlanToken) return; // cancelled/superseded during the await → discard this result
    // 07-04: `orchestrate` now returns {two_wave, tasks}. Back-compat: a bare array (older
    // backend) → two_wave=false (single-wave). Capture the run-level flag for dispatchBridge.
    const tasks = Array.isArray(dispatch) ? dispatch : (dispatch && dispatch.tasks) || [];
    bridgeTwoWaveAuto = !Array.isArray(dispatch) && !!(dispatch && dispatch.two_wave);
    // Provenance: stash the goal + the planner's full prompt so dispatchBridge persists them.
    bridgeGoal = goal;
    bridgePlanPrompt = (dispatch && dispatch.plan_prompt) || null;
    bridgePreview = tasks.filter((d) => d && d.id && d.task);
    if (bridgePreview.length === 0) { err.textContent = "Synthesis produced no tasks."; return; }
    // 06-19: surface the Team Planner's auto-assigned roles back onto the pane rows (pill +
    // dataset.role) so the operator SEES the team the planner formed, and a re-synthesize
    // treats an accepted role as a PIN (orchestrate falls back to the frontend role when the
    // live pane has none). A user can still override by editing the focus input before Send.
    paintPlannedRolePills(bridgePreview);
    // Carry the typed role + owns hint (omnigent Dispatch envelope) into the plan card
    // when the backend supplied them; absent → no-op, same {id,task,wave} shape.
    bcEmit({ type: "plan", tasks: bridgePreview.map((d) => ({
      id: d.id, task: d.task, wave: d.wave || "code",
      ...(d.role != null && d.role !== "" ? { role: d.role } : {}),
      ...(Array.isArray(d.owns) && d.owns.length ? { owns: d.owns } : {}),
    })), twoWave: bridgeTwoWaveAuto });
    renderBridgePreviewList(bridgePreview);
    saveBridgePlan(goal, panes); // persist so Cancel/reopen re-offers it (no costly re-synth)
    planned = true;
  } catch (e) {
    bridgePreview = [];
    err.textContent = "Planning failed: " + String(e);
  } finally {
    clearTimeout(watchdog); // the call settled (or was superseded) → disarm the IPC watchdog
    // next click: send the previewed tasks; a failed/empty plan returns to "Plan tasks".
    // Only the LATEST plan owns the phase — a cancelled/superseded run must NOT flip the UI
    // (otherwise a late planner result reopens a stale preview, or leaves a stuck "Planning…").
    if (myToken === bridgePlanToken) setBridgePhase(planned ? "preview" : "idle");
  }
}

// human-readable run-folder name: YYYY-MM-DD-HHMMSS (seconds avoid collisions on
// rapid re-dispatch). Sanitized backend-side into a safe single path segment.
function bridgeRunLabel() {
  const d = new Date();
  const p = (n) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}-${p(d.getHours())}${p(d.getMinutes())}${p(d.getSeconds())}`;
}

// FAN-OUT: dispatch each synthesized task to ITS pane only (never broadcast); skip
// dead panes. Each task is appended with an instruction to write the agent's result
// to <runDir>/<pane>.md, so the Bridge can FAN-IN and synthesize a final document.
// SYNCHRONOUS in-flight latch for dispatchBridge: setBridgePhase("running") only fires
// after the first awaits, so a double-click during `bridge_new_run` used to start a
// SECOND dispatch (two run dirs, tasks typed twice into every pane). The latch is set
// before the first await and released on EVERY path (success, early return, throw).
let _bridgeDispatching = false;
async function dispatchBridge() {
  if (_bridgeDispatching) return;
  _bridgeDispatching = true;
  try { await doDispatchBridge(); }
  finally { _bridgeDispatching = false; }
}

async function doDispatchBridge() {
  // never dispatch while a synthesis is in flight — a restored/previewed plan can leave the
  // primary button enabled (phase "preview") while an auto PRD synth is still running; sending
  // now would race the synthesizer. Wait for it to finish.
  if (bridgeSynthBusy) { showToast("Synthesis in progress — wait for it to finish"); return; }
  if (!bridgePreview.length) return;
  // never clobber a known goal with a blanked field (reopen blanks #br-goal; the plan
  // was synthesized FROM a goal, so keep the last non-empty one).
  bridgeGoal = document.getElementById("br-goal").value.trim() || bridgeGoal;
  // mint the shared fan-in dir (best-effort: dispatch still works without it)
  bridgeRunDir = null;
  // write under the active workspace's folder (<repo>/bridge/<timestamp>/) so the
  // output is findable + shows in git status; backend falls back to App Support.
  const repo = (activeWs && workspaces[activeWs]) ? workspaces[activeWs].repo : null;
  try { bridgeRunDir = await invoke("bridge_new_run", { repo, label: bridgeRunLabel() }); }
  catch (_) { bridgeRunDir = null; }
  // persist so closing the modal (to watch the agents) doesn't lose the fan-in run
  if (bridgeRunDir) {
    try { localStorage.setItem(LS_KEYS.bridgeRun, JSON.stringify({ dir: bridgeRunDir, goal: bridgeGoal })); } catch (_) {}
  }
  // ADE slice 2: the role-prompt SECTION PACK (core/roles library via role_prompt_sections)
  // — injection-defense/freshness/escalation preamble + the fan-in write protocol
  // (report_head/report_tail are byte-identical to the old inline literal) + the LESSON
  // harvest hook. Fetched ONCE per dispatch batch (never per pane, never persisted — the
  // text is versioned with the app). Only needed when a run dir exists (no run dir → bare
  // tasks, today's behavior). A fetch failure would dispatch protocol-less tasks that never
  // write reports (the silent settle-timeout class) → abort LOUDLY before anything is sent.
  let SEC = null;
  if (bridgeRunDir) {
    try { SEC = await invoke("role_prompt_sections", { role: "worker" }); }
    catch (e) {
      showToast("Dispatch aborted — role prompt sections unavailable: " + String(e));
      bridgeRunDir = null;
      try { localStorage.removeItem(LS_KEYS.bridgeRun); } catch (_) {}
      setBridgePhase("preview"); // plan intact — the user can retry
      return;
    }
  }
  // Build a pane's full prompt: its task + (when we have a run dir) the section block.
  // Appended INLINE (no embedded newline): send_input types raw bytes into the agent's
  // TUI where \n submits, so the whole prompt must be ONE line with a single trailing
  // newline — else the task submits early and the instruction lands as a separate
  // message. (The section pack is single-line by construction — locked in core/roles.)
  // 07-04: two-wave needs a run dir (panes commit + assemble against it). OFF or no run
  // dir → byte-for-byte the single-wave 07-01/07-02 path (no commit barrier, dispatch ALL
  // panes now, synth vs main). A pane's wave defaults to "code" (back-compat).
  const twoWave = bridgeTwoWaveAuto && !!bridgeRunDir;
  const isVerify = (d) => twoWave && d.wave === "verify";
  // 07-04 AC-2 commit barrier: a CODE-wave pane (two-wave only) commits its owned files on
  // its branch after writing <pane>.md, so the work EXISTS as a commit a sibling can read
  // via the assembled integration tree (an uncommitted working tree is invisible cross-pane).
  const buildTask = (d) => {
    let task = d.task;
    if (bridgeRunDir) {
      task += SEC.preamble + SEC.report_head + bridgeRunDir + '/' + d.id + '.md' + SEC.report_tail;
      if (twoWave && d.wave !== "verify") {
        task += '  THEN commit your owned files on your branch so siblings can assemble them:'
          + ' run `git add -A && git commit -m ' + "'bridge:" + bridgeRunLabel() + ':' + d.id + "'`"
          + ' (a commit is required — uncommitted work is invisible to the other panes).';
      }
    }
    return task;
  };
  // Skip panes with no live session (never spawned / closed) and KNOWN-dead corpses
  // (D30) — typing into a dead PTY just errors, and a corpse never writes its fan-in
  // report, so it must not enter the manifest/monitor either. Count the corpses.
  // 07-04 two-wave: VERIFY panes are NOT dispatched in this first pass — they wait until
  // the code wave commits and bridge_assemble_integration builds the tree (dispatchVerifyWave).
  let dead = 0;
  const liveTargets = [];
  const heldVerify = [];
  for (const d of bridgePreview) {
    if (!sessions[d.id]) continue;
    if (deadPanes.has(d.id)) { dead++; continue; }
    if (isVerify(d)) { heldVerify.push(d); continue; }
    liveTargets.push(d);
  }
  bridgeVerifyPending = heldVerify;
  bridgeVerifyDispatched = false;
  bridgeIntegPath = null;
  setBridgePhase("running", "Sending tasks…");
  // SSOT: persist the accepted planner roles onto the live supervisors NOW (the commit gate),
  // so bridgeLivePanes().role is authoritative for every staffed pane → the role pill survives
  // ALL re-renders + a webview reload (no longer dependent on the in-memory overlay), and the
  // fan-in attribution (pane_contributors) becomes role-aware. Covers the code wave AND the held
  // verify wave (reviewer/security). Best-effort — a failure leaves the overlay as the fallback.
  try {
    const rolePairs = bridgePreview
      .filter((d) => d.role && sessions[d.id] && !deadPanes.has(d.id))
      .map((d) => [d.id, d.role]);
    if (rolePairs.length) await invoke("set_pane_roles", { roles: rolePairs });
  } catch (_) { /* non-fatal: the bridgePlannedRoles overlay still covers the pill */ }
  // await each send (order preserved) so a pane that dies ON dispatch is caught and
  // skipped from fan-in, not silently swallowed by a fire-and-forget invoke.
  // sendTaskSubmit = paste then a separate deferred Enter (long pastes can sit
  // unsubmitted in the TUI input when the trailing \n rides inside the paste).
  const sent = await Promise.all(liveTargets.map(async (d) =>
    sendTaskSubmit(d.id, (await primeTask(d.task)) + buildTask(d))
      .then(() => ({ id: d.id, ok: true, dead: false }))
      // ANY send failure = the pane did NOT get its task. A dead-pane reject additionally
      // marks the corpse; a non-dead error (IPC hiccup etc.) is still a FAILURE — the old
      // `ok: !DEAD_RE.test(e)` counted it as dispatched and fan-in waited on a report that
      // was never asked for.
      .catch((e) => ({ id: d.id, ok: false, dead: DEAD_RE.test(String(e)), err: String(e) }))));
  const dispatchedIds = [];
  const newlyDead = [];
  const failedSends = [];
  for (const r of sent) {
    if (r.ok) dispatchedIds.push(r.id);
    else if (r.dead) newlyDead.push(r.id);
    else failedSends.push(r);
  }
  for (const id of newlyDead) deadPanes.add(id);
  if (newlyDead.length) renderWorkspaces(_lastQueue);
  dead += newlyDead.length;
  if (failedSends.length) {
    showToast(`${failedSends.length} send${failedSends.length === 1 ? "" : "s"} failed (not dead — not dispatched): ` + failedSends.map((r) => r.id).join(", "));
  }
  const n = dispatchedIds.length;
  // every wave-1 target was a corpse → nothing dispatched: tell the user and tear down the
  // (now pointless) fan-in run so the monitor/synthesis don't wait on nobody. (In two-wave
  // mode this means no live CODE pane — there is nothing to assemble for the verify wave.)
  if (n === 0) {
    showToast(dead ? `${dead} pane${dead === 1 ? "" : "s"} dead, skipped — nothing dispatched` : (twoWave ? "No live code panes to dispatch (two-wave needs ≥1 code pane)." : "No live panes to dispatch."));
    if (bridgeRunDir) { bridgeRunDir = null; try { localStorage.removeItem(LS_KEYS.bridgeRun); } catch (_) {} }
    bridgeVerifyPending = [];
    setBridgePhase("preview"); // plan is intact — let the user retry the send
    return;
  }
  // 07-01 P2-B: write the authoritative dispatched-id manifest so fan-in reads exactly
  // these panes (ignores phantom / off-convention .md double-writes). Best-effort. In
  // two-wave mode this is the CODE wave only; the verify wave's ids are appended when it
  // dispatches (dispatchVerifyWave) so fan-in still synthesizes every report.
  if (bridgeRunDir && dispatchedIds.length) {
    // Persist the planned prompts (provenance) so the PRD fan-in records what each pane was
    // ASKED, not only what it reported. Clean d.task — the report-path wrapper is added by buildTask.
    // Also persist the goal + the planner's full prompt (companion plan-prompt.md).
    const plan = bridgePreview
      .filter((d) => dispatchedIds.includes(d.id))
      .map((d) => ({ id: d.id, role: d.role || null, task: d.task }));
    try {
      // LOAD-BEARING await (was a swallowed fire-and-forget): compute_pane_ready reads the
      // manifest, so a silently-failed write left the run permanently "running" with the
      // monitor waiting on panes it couldn't see. On failure: toast LOUDLY and abort the
      // fan-in run (the tasks are already typed into the panes — the agents keep working —
      // but there is no monitorable run; the operator re-dispatches or reads panes directly).
      await invoke("bridge_write_manifest", {
        dir: bridgeRunDir,
        panes: dispatchedIds,
        plan,
        goal: bridgeGoal || null,
        planPrompt: bridgePlanPrompt || null,
      });
    } catch (e) {
      showToast("Bridge manifest write FAILED — fan-in aborted (agents did get their tasks): " + String(e));
      bridgeRunDir = null;
      try { localStorage.removeItem(LS_KEYS.bridgeRun); } catch (_) {}
    }
  }
  // 07-02: arm the readiness monitor for these panes (pollBridgeReady auto-fires synthesis,
  // OR — in two-wave mode — assembly + the verify wave, then synthesis).
  bridgePaneIds = dispatchedIds;
  bridgeCodePaneIds = twoWave ? dispatchedIds.slice() : [];
  bridgeAutoFails = 0;
  bridgeReadyPrev = {};
  bridgeNudged = {};
  bridgeAnnounced = {};
  bridgeDispatchedAt = Date.now();
  try { localStorage.removeItem(LS_KEYS.bridgePrd); } catch (_) {} // a new run supersedes the old PRD
  if (bridgeRunDir) {
    try { localStorage.setItem(LS_KEYS.bridgeRun, JSON.stringify({ dir: bridgeRunDir, goal: bridgeGoal, panes: dispatchedIds, at: bridgeDispatchedAt, twoWave, verify: bridgeVerifyPending })); } catch (_) {}
  }
  const vN = bridgeVerifyPending.length;
  bcEmit({ type: "dispatched", n, dead, heldVerify: twoWave && vN > 0 });
  showToast(`Bridge dispatched to ${n} ${twoWave ? "code " : ""}pane${n === 1 ? "" : "s"}` + (twoWave && vN ? ` · ${vN} verify pane${vN === 1 ? "" : "s"} held for assembly` : "") + (dead ? ` · ${dead} dead, skipped` : ""));
  // mark the dispatched panes' chips so the modal shows the send landed.
  for (const id of dispatchedIds) setPaneState(id, "task sent — working…");
  for (const id of newlyDead) setPaneState(id, "✗ dead", "bad");
  for (const r of failedSends) setPaneState(r.id, "✗ send failed — not dispatched", "bad");
  // FAN-IN step: keep the modal open; the primary button shows live progress.
  const note = document.getElementById("br-note");
  if (bridgeRunDir) {
    note.textContent = twoWave
      ? `Wave 1: dispatched ${n} code pane(s)` + (vN ? ` (${vN} verify held)` : "") + `. Assembling + dispatching verify when they commit…`
      : (bridgeAuto
          ? `Dispatched to ${n} pane(s) — the PRD builds automatically when they finish writing.`
          : `Dispatched to ${n} pane(s). The button unlocks when they finish writing.`);
    note.classList.remove("hidden");
    setBridgePhase("running", `Working — 0/${n} reports…`);
    showBridgeAutoToggle(true);
  } else {
    // no run dir (e.g. headless) — nothing to fan in; behave like the old one-shot
    closeBridge();
  }
}

// 07-04 wave-1 → wave-2 bridge: the code wave has committed; assemble its branches into ONE
// integration worktree via a real 3-way merge (bridge_assemble_integration), surface any
// co-written-file conflicts (visible, never silently dropped — the whole point), then
// dispatch the held VERIFY-wave panes pointed at the ASSEMBLED tree's absolute path so a QE
// pane greps the real merged code instead of guessing a sibling's uncommitted API (RC-3).
//
// NOTE on cwd: a verify pane is an already-LIVE PTY whose cwd was frozen at spawn (the same
// constraint that deferred per-run worktrees in 07-01), so we cannot RELOCATE it into the
// integration worktree. Instead we point it there via TASK TEXT (absolute paths to grep /
// `cargo test --manifest-path <integ>/...`) — faithful to AC-4's intent (verify reads the
// assembled tree) without touching send_input/spawn. The live end-to-end behavior is the
// plan's blocking human-verify checkpoint; this wiring is unit/compile-verified + human-gated.
async function assembleAndDispatchVerify() {
  if (bridgeAssembling || bridgeVerifyDispatched || !bridgeRunDir) return;
  bridgeAssembling = true;
  const dir = bridgeRunDir;
  const goal = bridgeGoal;
  const codePanes = bridgeCodePaneIds.slice();
  const verify = bridgeVerifyPending.slice();
  const note = document.getElementById("br-note");
  // ADE slice 2: the VERIFIER-flavored section pack, fetched ONCE for the wave (its
  // report contract differs from the worker flavor by one clause — core/roles is the
  // SSOT). Failure = the verify panes would get protocol-less tasks that never settle:
  // abort the WAVE loudly (at-most-once, like a degraded assembly) and let synthesis
  // proceed on the code-wave reports already in hand.
  let SECV = null;
  try { SECV = await invoke("role_prompt_sections", { role: "verifier" }); }
  catch (e) {
    bridgeVerifyDispatched = true;
    bridgeVerifyPending = [];
    showToast("Verify wave aborted — role prompt sections unavailable: " + String(e));
    if (note) note.textContent = "Verify wave aborted (role prompt sections unavailable) — synthesizing code-wave reports only.";
    bridgeAssembling = false;
    return;
  }
  if (note) note.textContent = "Wave 1 committed — assembling the integration tree (3-way merge)…";
  let res = null;
  try {
    res = await invoke("bridge_assemble_integration", { dir, codePanes });
  } catch (e) {
    if (note) note.textContent = "Integration assembly failed: " + String(e) + " — falling back to single-wave synthesis.";
  }
  bridgeVerifyDispatched = true; // at-most-once regardless of outcome
  // surface conflicts LOUDLY (visible, human-resolvable) — never silently merged.
  const conflicts = (res && Array.isArray(res.conflicts)) ? res.conflicts : [];
  if (conflicts.length) {
    const summary = conflicts.map((c) => `${c.file} (${(c.panes || []).join(", ")})`).join("; ");
    showToast(`Integration conflicts — resolve manually: ${summary}`);
  }
  bridgeIntegPath = (res && res.ok && res.worktree) ? res.worktree : null;
  // dispatch the held verify panes. When we have an assembled tree, point each at its
  // absolute path; otherwise (assembly degraded) verify still runs but against its own tree.
  const liveVerify = verify.filter((d) => sessions[d.id] && !deadPanes.has(d.id));
  const buildVerifyTask = (d) => {
    let task = d.task;
    if (bridgeIntegPath) {
      task += '  —  IMPORTANT: the team\'s committed wave-1 code is assembled at this absolute path: '
        + bridgeIntegPath + ' . Read the REAL source there (e.g. grep ' + bridgeIntegPath
        + ' and run `cargo test --manifest-path ' + bridgeIntegPath + '/app/src-tauri/Cargo.toml`)'
        + ' — do NOT guess sibling signatures; verify against that tree.';
    }
    if (conflicts.length) {
      const cs = conflicts.map((c) => c.file).join(", ");
      task += '  NOTE: these files had unresolved 3-way merge conflicts and were NOT integrated: '
        + cs + ' — flag them as needing human resolution, do not assume their merged state.';
    }
    // Verifier section block from core/roles (defense/freshness/escalation preamble + the
    // verify-flavored write protocol + LESSON hook); report path spliced per pane.
    task += SECV.preamble + SECV.report_head + dir + '/' + d.id + '.md' + SECV.report_tail;
    return task;
  };
  const sent = await Promise.all(liveVerify.map(async (d) =>
    sendTaskSubmit(d.id, (await primeTask(d.task)) + buildVerifyTask(d)) // paste + deferred Enter (same no-submit fix as wave 1)
      .then(() => ({ id: d.id, ok: true }))
      .catch((e) => ({ id: d.id, ok: !DEAD_RE.test(String(e)) }))));
  const verifyIds = [];
  for (const r of sent) { if (r.ok) verifyIds.push(r.id); else deadPanes.add(r.id); }
  // the manifest + monitor must now include BOTH waves so synthesis fans in every report.
  const allIds = bridgeCodePaneIds.concat(verifyIds);
  // LOAD-BEARING await (was a swallowed fire-and-forget): bridge_synthesize reads this
  // manifest, so a silently-failed rewrite leaves it listing ONLY the code panes → the
  // reviewer/security reports are dropped from the PRD without a trace. Mirror the wave-1
  // hardened write (~main.js dispatch): on failure toast LOUDLY and abort the fan-in run
  // (verify agents already got their tasks — they keep working; there's just no monitorable
  // run to synthesize, so the operator re-dispatches or reads panes directly).
  try {
    await invoke("bridge_write_manifest", { dir, panes: allIds });
  } catch (e) {
    showToast("Bridge verify-wave manifest write FAILED — fan-in aborted (verify agents did get their tasks): " + String(e));
    bridgeRunDir = null;
    try { localStorage.removeItem(LS_KEYS.bridgeRun); } catch (_) {}
    bridgeVerifyPending = [];
    if (note) note.textContent = "Verify-wave manifest write failed — fan-in aborted. Read the verify panes directly, or re-run.";
    bridgeAssembling = false;
    return;
  }
  bridgePaneIds = allIds;
  bridgeVerifyPending = [];
  bridgeReadyPrev = {};
  bridgeNudged = {};
  bridgeAnnounced = {};
  bridgeDispatchedAt = Date.now(); // reset the stuck-timer for the verify wave
  try { localStorage.setItem(LS_KEYS.bridgeRun, JSON.stringify({ dir, goal, panes: allIds, at: bridgeDispatchedAt, twoWave: true, verify: [] })); } catch (_) {}
  if (note) {
    const okMsg = bridgeIntegPath ? "assembled" : "assembly degraded (single-wave fallback)";
    note.textContent = `Wave 2: dispatched ${verifyIds.length} verify pane(s) against the ${okMsg} tree`
      + (conflicts.length ? ` · ${conflicts.length} conflict(s) flagged` : "") + ". Synthesizing when they finish…";
  }
  showToast(`Wave 2 dispatched to ${verifyIds.length} verify pane(s)` + (bridgeIntegPath ? " (assembled tree)" : " (assembly degraded)"));
  bridgeAssembling = false;
}

// 07-02: poll the dispatched panes' fan-in reports; auto-fire synthesis ONCE when every
// pane has SETTLED. A LIVE pane settles only when its report is COMPLETE (contains the
// final "## BOUNDARIES" section — the agent finished writing, not just paused mid-output)
// AND byte-size-stable since the last poll. A DEAD pane settles when it has no output or
// its (truncated) output is size-stable. Own ~2.5s timer; inert unless a run is live + idle.
// ---- spoken/visible run announcements (operator-requested continuous monitoring) ----
// Every meaningful pane/run transition gets a toast + a SPOKEN one-liner (existing
// non-blocking `speak` cmd → /usr/bin/say), so the operator hears which harness
// finished / stalled / died without watching the grid. Mute: localStorage
// at_announce = "off" (default ON).
function paneHarnessName(id) {
  const ws = workspaces[paneOwner(id)];
  const m = /-p(\d+)$/.exec(id || "");
  const idx = m ? Number(m[1]) : -1;
  return (ws && idx >= 0 && (ws.harnesses || [])[idx]) || (ws && ws.harness) || "agent";
}
function announce(text) {
  let on = true;
  try { on = localStorage.getItem("at_announce") !== "off"; } catch (_) {}
  if (!on) return;
  showToast(text);
  if (hasTauri()) invoke("speak", { text }).catch(() => {});
}
let bridgeAnnounced = {}; // id -> last announced state ("done"|"dead"|"stalled"); reset per dispatch
function announcePaneOnce(id, state, text) {
  if (bridgeAnnounced[id] === state) return;
  bridgeAnnounced[id] = state;
  announce(text);
}

const BRIDGE_STUCK_MS = 900000; // 15 min → surface the MANUAL "build from what's ready" escape.
// Bumped 5m→15m (2026-06-22): a Bedrock verify-wave reviewer reading the whole assembled diff
// legitimately runs well past 5m — the old window made the "looks stuck" nag fire prematurely.
// Stall detector (live-fired 2026-06-10: a commandcode pane lost its task after a sandbox
// "outside workspace" dance and sat idle forever — chip read "working…", auto-synth never
// fired). A live pane with NO report and NO PTY output for this long gets ONE nudge
// (re-sends the write protocol). A LIVE pane is then WAITED for — NEVER silently abandoned into
// synthesis (see pollBridgeReady); only a DEAD pane auto-settles without a report. The operator's
// manual "Build PRD from what's ready" (shown once `stuck`) is the deliberate escape. Bumped
// 2m→10m for Bedrock (the nudge fires later; the pane is no longer abandoned a window after it).
const BRIDGE_QUIET_MS = 600000;
let bridgeNudged = {}; // id -> ts of the one-shot nudge this run (reset per dispatch)
async function pollBridgeReady() {
  try {
    if (hasTauri() && bridgeRunDir && !bridgeSynthBusy && !bridgeAssembling && bridgePaneIds.length) {
      const rows = await invoke("bridge_ready", { dir: bridgeRunDir });
      const total = rows.length || bridgePaneIds.length;
      let settledCount = 0;
      let allSettled = rows.length > 0;
      let anyOutput = false;
      // 07-T3: build the code-wave set once per poll tick; a pane is code-wave when
      // bridgeCodePaneIds is non-empty AND the pane id is in that set. This is false for
      // single-wave runs (bridgeCodePaneIds is []) and for verify-wave panes (not in the
      // set). The commit gate in isPaneSettled is only enforced for code-wave panes.
      const codeWaveSet = bridgeCodePaneIds.length
        ? new Set(bridgeCodePaneIds)
        : null;
      for (const r of rows) {
        if (r.bytes > 0) anyOutput = true;
        // 07-T3: use the shared pure predicate (bridge-settle.js). For code-wave panes in
        // a two-wave run this additionally requires r.committed (≥1 commit on the branch),
        // preventing premature assembly when the agent writes "## BOUNDARIES" before
        // completing its `git commit`.
        const isCodeWave = codeWaveSet !== null && codeWaveSet.has(r.id);
        const settled = isPaneSettled(r, bridgeReadyPrev[r.id] ?? 0, isCodeWave);
        // live per-pane chip (open modal only — setPaneState no-ops when the row is absent)
        if (r.dead) {
          setPaneState(r.id, r.bytes > 0 ? "✗ died (partial report)" : "✗ dead", "bad");
          announcePaneOnce(r.id, "dead", `${paneHarnessName(r.id)} pane died${r.bytes > 0 ? " with a partial report" : ""} — needs you.`);
        }
        else if (settled) {
          setPaneState(r.id, "✓ report", "ok");
          announcePaneOnce(r.id, "done", `${paneHarnessName(r.id)} finished its report.`);
        }
        // 07-T3: code-wave pane wrote its report (complete + stable) but the commit has
        // not landed yet — surface "committing…" so the operator doesn't see a stale
        // "writing report…" chip when the agent is actually running `git commit`.
        else if (isCodeWave && r.complete && r.bytes > 0 && bridgeReadyPrev[r.id] === r.bytes && !r.committed) setPaneState(r.id, "committing…");
        else if (r.bytes > 0) setPaneState(r.id, "writing report…");
        else {
          // No report bytes at all: stall detection. quiet = time since the pane last
          // produced ANY PTY output (fallback: since dispatch, for a pane with no entry).
          const s = sessions[r.id];
          const quiet = Date.now() - ((s && s.lastOut) || bridgeDispatchedAt);
          if (quiet > BRIDGE_QUIET_MS) {
            if (!bridgeNudged[r.id]) {
              bridgeNudged[r.id] = Date.now();
              sendTaskSubmit(
                r.id,
                `REMINDER: write your COMPLETE report as Markdown to this EXACT path NOW (create parent dirs; overwrite; end with a "## BOUNDARIES" section): ${bridgeRunDir}/${r.id}.md — write it with whatever you have so far; if you already finished, write that same report to that exact path.`
              ).catch(() => {}); // fire-and-forget nudge — a dead/closed pane reject must not throw out of the poll tick
              setPaneState(r.id, "nudged — write the report…");
            } else if (Date.now() - bridgeNudged[r.id] > BRIDGE_QUIET_MS) {
              // 2026-06-22: a LIVE pane is NEVER auto-abandoned into synthesis. The verify-wave
              // reviewer can be legitimately slow on Bedrock (reads the whole assembled diff); the
              // old behavior counted it settled-failed and built the PRD WITHOUT it (final.HELD.md
              // written before the reviewer's report — observed live). Keep it UNsettled so
              // auto-synth WAITS; only a DEAD pane (handled above) settles without a report. The
              // operator's manual "Build PRD from what's ready" (shown once `stuck`) is the escape.
              setPaneState(r.id, "slow — still waiting (Build PRD from what's ready to proceed without it)");
            } else {
              setPaneState(r.id, "nudged — write the report…");
            }
          } else {
            setPaneState(r.id, "working…");
          }
        }
        if (settled) settledCount++; else allSettled = false;
      }
      bridgeReadyPrev = {};
      for (const r of rows) bridgeReadyPrev[r.id] = r.bytes;
      const autoGaveUp = bridgeAutoFails >= 3;
      const stuck = Date.now() - bridgeDispatchedAt > BRIDGE_STUCK_MS;
      const allDead = rows.length > 0 && rows.every((r) => r.dead);
      const note = document.getElementById("br-note");
      // WEDGE GUARD: every dispatched pane died with ZERO output. Dead panes settle without a
      // report, so allSettled is true but anyOutput is false — neither the synth branch (needs
      // anyOutput) nor the manual escape (gated on stuck && anyOutput) can ever fire, and the
      // run sits "running" forever. Terminate it: error toast + reset the phase (not wedged).
      if (allSettled && !anyOutput && allDead) {
        if (note) note.textContent = `All ${total} pane(s) died with no output — run aborted.`;
        showToast(`Orchestrate aborted — all ${total} pane(s) died before producing any output.`);
        announcePaneOnce("__all", "dead", `All agents died with no output — the run was aborted.`);
        bridgeRunDir = null;
        try { localStorage.removeItem(LS_KEYS.bridgeRun); } catch (_) {}
        bridgePaneIds = [];
        bridgeVerifyPending = [];
        if (bridgePhase === "running" || bridgePhase === "ready") setBridgePhase("preview"); // plan intact — allow a re-dispatch
        setTimeout(pollBridgeReady, 2500); // keep the perpetual poll alive for a future run (this one is inert now)
        return;
      }
      // 07-04 two-wave: when the CODE wave has settled and we have not yet dispatched the
      // VERIFY wave, assemble the integration tree + dispatch verify INSTEAD of synthesizing.
      // The verify panes then write their own reports and the NEXT settle (all panes done)
      // proceeds to synthesis — now retargeted at the assembled tree (AC-5). Gated on Auto
      // (the auto path drives the chain) so it can't fire while a manual run is paused.
      if (allSettled && anyOutput && bridgeVerifyPending.length && !bridgeVerifyDispatched && bridgeAuto && !autoGaveUp) {
        assembleAndDispatchVerify();
        setTimeout(pollBridgeReady, 2500);
        return;
      }
      // phase-aware primary-button updates: only while the run owns the button (never
      // stomp planning/collecting/prd, which belong to in-flight actions or the seam).
      const runOwnsButton = bridgePhase === "running" || bridgePhase === "ready";
      if (allSettled && anyOutput) {
        // Authoritative verdict (the backend verify_dispatched engine, NOT a JS re-derive):
        // the machine answer to "did every dispatched harness produce a USABLE report",
        // surfaced in the settle note so the operator sees ok/empty/incomplete/missing
        // per pane instead of just "N/M reports".
        let verifyNote = "";
        try {
          const verdicts = await invoke("bridge_verify", { dir: bridgeRunDir });
          const usable = verdicts.filter((v) => v.status === "ok").length;
          const bad = verdicts.filter((v) => v.status !== "ok");
          verifyNote =
            ` — verified ${usable}/${verdicts.length} usable` +
            (bad.length ? ` (${bad.map((v) => `${paneHarnessName(v.id)}:${v.status}`).join(", ")})` : "");
        } catch (_) {}
        if (bridgeAuto && !autoGaveUp) {
          if (note) note.textContent = `All agents done (${settledCount}/${total})${verifyNote} — building the PRD…`;
          announcePaneOnce("__all", "done", `All agents done — ${settledCount} of ${total} reports in. Building the PRD.`);
          synthesizeResults(true); // auto; one-shot via bridgeSynthBusy
        } else {
          if (note) note.textContent = `All agents done (${settledCount}/${total})${verifyNote}.`;
          if (runOwnsButton) setBridgePhase("ready"); // "Build PRD →" unlocked
        }
      } else {
        if (note) {
          note.textContent = stuck
            ? (anyOutput
                ? `${settledCount}/${total} done; some agents look stuck — you can build the PRD from what's ready.`
                : "No reports yet and it's been a while — check the panes: if a task is sitting unsubmitted in an input line, click the pane and press Enter.")
            : (bridgeAuto && !autoGaveUp
                ? `Working — ${settledCount}/${total} reports in. The PRD builds automatically when all finish.`
                : `Working — ${settledCount}/${total} reports in.`);
        }
        if (runOwnsButton) {
          if (stuck && anyOutput) setBridgePhase("ready", "Build PRD from what's ready →");
          else setBridgePhase("running", `Working — ${settledCount}/${total} reports…`);
        }
      }
    }
  } catch (_) {}
  setTimeout(pollBridgeReady, 2500);
}

// FAN-IN: read each agent's <runDir>/<pane>.md, synthesize one final document
// (headless claude), save it, open it. `auto` = fired by the readiness poll (capped) vs a
// manual click (always allowed). bridgeSynthBusy prevents concurrent runs; the live-run
// state is cleared BEFORE the (long) await so a modal reopen / next poll can't double-fire.
async function synthesizeResults(auto = false) {
  if (!bridgeRunDir || bridgeSynthBusy) return;
  if (auto && bridgeAutoFails >= 3) return; // auto gave up after repeated failures; manual still works
  // Two-wave guard (manual path): with the verify wave still HELD (e.g. Auto off, so the
  // poll's auto chain never fired), a manual "Build PRD" here would synthesize WITHOUT any
  // verification AND silently clear bridgeVerifyPending on success. Route through the SAME
  // assembly+dispatch the auto path uses; synthesis re-fires when the verify reports settle
  // (auto) or on the next manual click once wave 2 is out.
  if (bridgeVerifyPending.length && !bridgeVerifyDispatched) {
    showToast("Verify wave still held — assembling + dispatching it first; synthesize when it reports");
    assembleAndDispatchVerify();
    return;
  }
  bridgeSynthBusy = true;
  const dir = bridgeRunDir;
  // goal fallback chain: the in-memory goal can be lost across spawn→reopen/reload paths
  // (operator hit "PRD build failed: empty goal" with BOTH reports ready on disk). The
  // synthesizer must never refuse over a lost label — the reports are the substance.
  const goal = bridgeGoal
    || (document.getElementById("br-goal")?.value || "").trim()
    || "(original goal text unavailable — synthesize the team's reports into one coherent document)";
  const panes = bridgePaneIds.slice();
  // clear the live-run state NOW (before the tens-of-seconds await) so a reopen or the
  // next 2.5s poll sees no live run and cannot start a SECOND concurrent synthesis.
  bridgeRunDir = null;
  bridgePaneIds = [];
  try { localStorage.removeItem(LS_KEYS.bridgeRun); } catch (_) {}
  const err = document.getElementById("br-error");
  err.textContent = "";
  setBridgePhase("collecting");
  // Live PRD-synthesis progress: reveal the bar + tick an elapsed timer. Backend
  // (bridge-synth-progress) fills the phase label + bar %; the timer + shimmer cover the long,
  // opaque Opus fan-in so the user sees it's alive — not skipped or neglected.
  const synthProg = document.getElementById("br-synth-progress");
  const synthBar = document.getElementById("br-synth-bar");
  const synthPhaseEl = document.getElementById("br-synth-phase");
  const synthElapsedEl = document.getElementById("br-synth-elapsed");
  if (synthProg) synthProg.classList.remove("hidden");
  if (synthBar) synthBar.style.width = "5%";
  if (synthPhaseEl) synthPhaseEl.textContent = "Starting synthesis…";
  const synthStart = Date.now();
  const synthTimer = setInterval(() => {
    if (!synthElapsedEl) return;
    const s = Math.floor((Date.now() - synthStart) / 1000);
    synthElapsedEl.textContent = `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
  }, 1000);
  try {
    const path = await invoke("bridge_synthesize", { dir, goal });
    try { await invoke("open_external", { url: path }); } catch (_) {}
    showToast(`PRD ready: ${path}`);
    try { localStorage.removeItem(LS_KEYS.bridgeGoalDraft); } catch (_) {} // run complete — draft served its purpose
    bridgeAutoFails = 0;
    // 07-04: this run is finished — clear two-wave state so the next run starts clean.
    bridgeVerifyPending = [];
    bridgeVerifyDispatched = false;
    bridgeCodePaneIds = [];
    bridgeIntegPath = null;
    if (bridgeUnify) {
      // UNIFY seam: keep the modal open. TWO tails, picked by what the run actually was:
      //   PASS synthesis over a REAL integration fold → the team already coded+tested; the
      //   primary becomes "Open PR →" (push the integ branch — no flywheel re-implementation).
      //   Anything else (recon/advisory/HELD) → "Run Flywheel →" (workers implement the fix).
      bridgePrdPath = path;
      bridgeShipDir = null;
      if (/(^|\/)final\.md$/.test(path)) {
        try { await invoke("read_text_file", { path: dir + "/integration.json" }); bridgeShipDir = dir; } catch (_) {}
      }
      // persist: auto-synth often completes with the MODAL CLOSED (the run survives close by
      // design) — without this, reopening drops the finished PRD and the user reopens to a
      // blank "Plan tasks" with no trace of their run (operator hit exactly that).
      try { localStorage.setItem(LS_KEYS.bridgePrd, JSON.stringify({ path, at: Date.now(), shipDir: bridgeShipDir })); } catch (_) {}
      bcEmit({ type: "prd", path });
      setBridgePhase("prd", bridgeShipDir ? "Open PR →" : undefined);
      paintBridgeFlywheelGate(); // surface arm-state at the checkpoint (don't click through blind)
      const note = document.getElementById("br-note");
      if (note) {
        note.textContent = bridgeShipDir
          ? "PASS — the team's work is folded + tested. Review the doc, then Open PR to ship the integration branch."
          : "PRD ready (opened) — review it, then Run Flywheel to ship it as a PR.";
        note.classList.remove("hidden");
      }
      announce(bridgeShipDir
        ? "Synthesis done — verdict PASS. The integration tree is tested; you can open the PR."
        : (/HELD/.test(path) ? "Synthesis done — verdict HELD. Review needed." : "PRD ready — review it, then run the flywheel."));
    } else {
      bcEmit({ type: "prd", path }); // card without the ship button (non-unify)
      closeBridge();
      setBridgePhase("idle");
    }
  } catch (e) {
    err.textContent = "PRD build failed: " + String(e);
    if (auto) bridgeAutoFails++; // cap the auto-retry storm; manual is never capped
    // restore the run so a retry (manual always; auto until the cap) can re-fire
    bridgeRunDir = dir;
    bridgePaneIds = panes;
    try { localStorage.setItem(LS_KEYS.bridgeRun, JSON.stringify({ dir, goal, panes, at: bridgeDispatchedAt })); } catch (_) {}
    setBridgePhase("ready", "Retry — build PRD →");
  } finally {
    bridgeSynthBusy = false;
    // synth finished — re-enable the primary button for the now-current phase, preserving the
    // label already set in try/catch ("Open PR →" / "Retry — build PRD →"). Without this the
    // bridgeSynthBusy guard in setBridgePhase would leave it disabled until the next phase change.
    { const b = document.getElementById("br-primary"); if (b) setBridgePhase(bridgePhase, b.textContent); }
    clearInterval(synthTimer);
    if (synthBar) synthBar.style.width = "100%";
    // leave the completed bar a beat (so the jump to 100% reads), then hide for the next run
    setTimeout(() => { document.getElementById("br-synth-progress")?.classList.add("hidden"); }, 1200);
  }
}

// SHIP tail state: the run dir of a PASS synthesis over a real integration fold. When set,
// the PRD-phase primary is "Open PR →" (push the integ branch) instead of "Run Flywheel →"
// (re-implement from findings) — the team's code is already written, folded, and tested.
let bridgeShipDir = null;
// §9.3 arm-confirm state for the ship path (mirrors armPendingRepo for Delegate/Flywheel submit).
let bridgeArmPendingDir = null;
let bridgeArmTimer = null;

async function bridgeOpenPr() {
  if (!bridgeShipDir || !hasTauri()) return;
  const err = document.getElementById("br-error");
  const note = document.getElementById("br-note");
  if (err) err.textContent = "";
  // §9.3 arm-confirm for the panes-runner ship path. bridge_open_pr REFUSES an unarmed repo (the
  // push runs under the human's credentials); unlike Delegate/Flywheel submit (ensureRepoArmed)
  // this path had no in-app arm → the operator hit a dead-end "not armed" error with nowhere to
  // arm. Two-click, same UX as ensureRepoArmed: first Open-PR click on an unarmed repo WARNS +
  // primes; a second click within 6s arms it (delegate_trust_repo) then pushes. The repo is the
  // run dir minus its trailing `/bridge/<run>` (matches the backend's dirp.parent().parent()).
  const repo = bridgeShipDir.replace(/\/+$/, "").split("/").slice(0, -2).join("/");
  let trusted = true;
  try { trusted = await invoke("delegate_is_repo_trusted", { path: repo }); }
  catch (_) { trusted = true; } // older backend without the command → don't block (backend still gates)
  if (!trusted) {
    if (bridgeArmPendingDir === bridgeShipDir) {
      clearTimeout(bridgeArmTimer); bridgeArmPendingDir = null;
      try { await invoke("delegate_trust_repo", { path: repo }); }
      catch (e) { if (err) err.textContent = "Couldn't arm repo: " + e; return; }
      // armed — fall through to the push below
    } else {
      bridgeArmPendingDir = bridgeShipDir;
      clearTimeout(bridgeArmTimer);
      bridgeArmTimer = setTimeout(() => { bridgeArmPendingDir = null; }, 6000);
      if (err) err.textContent = `⚠ ${repo} isn't armed — the PR push runs under your git credentials. Click Open PR again within 6s to ARM this repo & push.`;
      if (note) { note.textContent = "Arming a repo is a one-time trust decision; it's remembered for next time."; note.classList.remove("hidden"); }
      setBridgePhase("prd", "Open PR → (arm & push)");
      return;
    }
  }
  setBridgePhase("collecting", "Opening PR…");
  try {
    const url = await invoke("bridge_open_pr", { dir: bridgeShipDir });
    try { localStorage.removeItem(LS_KEYS.bridgePrd); } catch (_) {} // shipped — don't re-offer
    announce("Pull request opened — review and merge it.");
    showToast("PR opened — review + merge it: " + url);
    bcEmit({ type: "info", text: "PR opened: " + url });
    if (note) { note.textContent = "PR opened — review + merge it: " + url; note.classList.remove("hidden"); }
    try { await invoke("open_external", { url }); } catch (_) {}
    bridgeShipDir = null;
    bridgePrdPath = null;
    setBridgePhase("idle", "PR opened ✓ — new goal →");
  } catch (e) {
    if (err) err.textContent = "PR failed: " + String(e);
    if (note) { note.textContent = "Manual fallback: push the integration branch from its worktree, then `gh pr create --base main --body-file <run>/final.md`."; note.classList.remove("hidden"); }
    setBridgePhase("prd", "Open PR → (retry)");
  }
}

// UNIFY seam handler: read the synthesized PRD, hand it to the Flywheel as the goal. The PRD
// must travel as CONTENT (the flywheel's workers run in fresh worktrees and can't see the
// untracked bridge/<run>/final.md). The gate fires only when the human presses Start cycle.
async function runFlywheelFromPrd() {
  if (!bridgePrdPath) return;
  if (!hasTauri()) { showToast("Tauri API unavailable."); return; }
  setBridgePhase("collecting", "Loading PRD…");
  let prd = "";
  try {
    prd = await invoke("read_text_file", { path: bridgePrdPath });
  } catch (e) {
    showToast("Couldn't read the PRD: " + String(e));
    setBridgePhase("prd");
    paintBridgeFlywheelGate();
    return;
  }
  setBridgePhase("prd");
  try { localStorage.removeItem(LS_KEYS.bridgePrd); } catch (_) {} // consumed — don't re-offer it
  // Strip the synthesizer's machine-verdict banner (e.g. "> [BRIDGE REJECT — authoritative
  // tests FAILED] …") from the top of the doc: it's meta about the RECON tree (a recon run's
  // reject just means "bug confirmed, unfixed"), not an instruction — left in, it becomes the
  // visible "prompt" on the History card and reads like the flywheel run itself failed.
  // Same for the IN-BODY "## MACHINE VERDICT: …" heading — relabel it as the pre-fix recon
  // verdict so the doc stays honest but can't be misread as THIS run's outcome.
  prd = prd
    .replace(/^(?:>\s*\[(?:BRIDGE|DELEGATE)[^\n]*\n+)+/, "")
    .replace(/^(#{1,3})\s*MACHINE VERDICT:\s*(.*)$/m, "$1 RECON VERDICT (before the fix — superseded by this run): $2")
    // bold-inline variant (the synthesizer sometimes emits "**MACHINE VERDICT: HOLD.** …"
    // instead of a heading — live-fired 2026-06-09; left in, it primes the next synthesizer
    // to treat the goal as a finished report).
    .replace(/\*\*MACHINE VERDICT:\s*([^*]*)\*\*/g, "**RECON VERDICT (before the fix — superseded by this run): $1**")
    .replace(/^\s+/, "");
  closeBridge();
  await openFlywheel(); // paints the gate + reveals the repo row if no live pane
  const g = document.getElementById("fw-goal");
  if (g) { g.value = prd; g.focus(); }
  showToast("PRD loaded into the Flywheel — review the goal, then Start cycle → to ship a PR.");
}

// UNIFY slice 5: at the PRD checkpoint, read the flywheel triple-gate so a disarmed user sees
// they'll need to arm BEFORE clicking through (openFlywheel paints #fw-gate too, but only after
// the click). Reuses the same delegate_gate_status as refreshFlywheelGate.
async function paintBridgeFlywheelGate() {
  const btn = document.getElementById("br-primary");
  if (!btn || !hasTauri()) return;
  let g;
  try { g = await invoke("delegate_gate_status"); } catch (_) { return; }
  lastDelegateGate = g;
  paintShipGateWarning();
  if (bridgePhase !== "prd") return; // only label the PRD checkpoint (the await may outlive the phase)
  // A PASS+folded run owns the "Open PR →" label (the integ tree is built — there's nothing for the
  // Flywheel to re-implement). Without this, paintBridgeFlywheelGate clobbered "Open PR →" back to
  // "Run Flywheel →" right after openBridge restored it (note said "…Open PR" but the button didn't).
  if (bridgeShipDir) return;
  const ready = !!(g && g.allow_mutations && g.delegate_live && Number(g.autonomy_ceiling) >= 1);
  btn.title = ready
    ? "Send this synthesized PRD to the Flywheel — it writes the code and opens one PR"
    : "You can load the PRD now, but the Flywheel isn't armed — you'll arm (mutations + autonomy + delegate-live) before Start cycle.";
  btn.textContent = ready ? "Run Flywheel →" : "Run Flywheel (arm needed) →";
  bcSyncAction(); // dock's action mirror picks up the gate label
}

const bridgeBtnEl = document.getElementById("bridge-btn");
if (bridgeBtnEl) bridgeBtnEl.onclick = (e) => {
  // Shift-click = flip the dock flag (the slice-1 test affordance; a Settings toggle
  // can replace it when the dock flips default-ON in slice 3).
  if (e.shiftKey) {
    bridgeChatDock = !bridgeChatDock;
    try { localStorage.setItem("at_bridge_chat_dock", bridgeChatDock ? "1" : "0"); } catch (_) {}
    showToast(bridgeChatDock ? "Orchestrate dock ON — click Orchestrate to open the chat pane." : "Orchestrate dock OFF — Orchestrate opens the modal.");
    if (!bridgeChatDock) closeBridgeDock();
    return;
  }
  if (bridgeChatDock) toggleBridgeDock(); else openBridge();
};
// ---- dock wiring (inert when the flag is off — the dock stays hidden) ----
const bcFormEl = document.getElementById("bc-form");
if (bcFormEl) bcFormEl.onsubmit = (e) => { e.preventDefault(); bcSubmit(); };
const bcInputEl = document.getElementById("bc-input");
if (bcInputEl) bcInputEl.addEventListener("keydown", (e) => {
  if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); bcSubmit(); } // Shift+Enter = newline
});
const bcCloseEl = document.getElementById("bc-close");
if (bcCloseEl) bcCloseEl.onclick = () => closeBridgeDock();
const bcActionEl = document.getElementById("bc-action");
if (bcActionEl) bcActionEl.onclick = () => document.getElementById("br-primary")?.click(); // ONE dispatcher
// Ship-mode mirror: single source = the modal toggle's existing handler.
const bcUnifyEl = document.getElementById("bc-unify");
if (bcUnifyEl) {
  bcUnifyEl.checked = bridgeUnify;
  bcUnifyEl.onchange = () => {
    const m = document.getElementById("br-unify");
    if (m) { m.checked = bcUnifyEl.checked; m.dispatchEvent(new Event("change")); }
    else { bridgeUnify = bcUnifyEl.checked; try { localStorage.setItem(LS_KEYS.bridgeUnify, bridgeUnify ? "1" : "0"); } catch (_) {} }
  };
}
// Feature toggle: the eve Team Planner (auto-role assignment). Default OFF → original manual
// orchestration. Restores persisted state on load; persists on change. `bridgeAutoPlan` is read
// in synthesizeBridge and passed to the `orchestrate` command as `autoPlan`.
const brAutoPlanEl = document.getElementById("br-autoplan");
if (brAutoPlanEl) {
  brAutoPlanEl.checked = bridgeAutoPlan;
  brAutoPlanEl.onchange = () => {
    bridgeAutoPlan = brAutoPlanEl.checked;
    try { localStorage.setItem("at_bridge_autoplan", bridgeAutoPlan ? "1" : "0"); } catch (_) {}
  };
}
// W3: project every #br-note / #br-error write into the chat (MutationObserver — zero
// call-site edits, future sites covered). Dedupe identical text so the 2.5s poll's
// rewrite of the same progress line can't flood the thread.
{
  let lastNote = "", lastErr = "";
  const noteEl = document.getElementById("br-note");
  if (noteEl) new MutationObserver(() => {
    const t = noteEl.textContent.trim();
    if (!t || t === lastNote) return;
    lastNote = t;
    bcEmit({ type: "info", text: t });
  }).observe(noteEl, { childList: true, characterData: true, subtree: true });
  const errEl = document.getElementById("br-error");
  if (errEl) new MutationObserver(() => {
    const t = errEl.textContent.trim();
    if (!t || t === lastErr) return;
    lastErr = t;
    bcEmit({ type: "error", text: t });
  }).observe(errEl, { childList: true, characterData: true, subtree: true });
}
// Cancel actually cancels: invalidate any in-flight planner (so its late result can't flip the
// modal back / leave it stuck on "Planning…"), and reset a PRE-DISPATCH phase to idle so a
// reopen is clean. A live run (running/ready/prd) is left intact — the dock keeps tracking it.
// SHARED by #br-cancel and the global Escape key so the two dismiss paths can never drift
// (Escape used to bare-closeBridge, leaving a stuck "Planning…" for the reopen).
function cancelBridge() {
  bridgePlanToken++;
  if (bridgePhase === "planning" || bridgePhase === "preview" || bridgePhase === "idle") {
    resetBridgePreview();
    setBridgePhase("idle");
  }
  closeBridge();
}
const brCancelEl = document.getElementById("br-cancel");
if (brCancelEl) brCancelEl.onclick = cancelBridge;
// ONE primary button — THE single router. It reads three axes at click time:
//   runner (at_orch_runner, default 'panes') · ship (bridgeUnify) · loop (at_orch_loop, default off).
// DESIGN §2.3 matrix:
//   panes    + ship off + loop off → today's orchestrate → bridge_synthesize  (DEFAULT — UNCHANGED)
//   panes    + ship on  + loop off → orchestrate → synthesize → prd → Open PR (today's ship path)
//   headless + ship off + loop off → delegate report-only (old submitDelegate)
//   headless + ship on  + loop off → delegate + flywheel armed → PR (old submitFlywheel)
//   any      + loop on             → STUB "loop: coming soon (P3)" — no execution
// HARD INVARIANT: runner='panes' && loop=off falls through to the EXACT pre-P2 phase machine below
// (byte-identical: same bridgePhase branches, same synthesizeBridge/dispatchBridge/synthesizeResults
// /bridgeOpenPr/runFlywheelFromPrd calls). The ship-on PANES path was ALREADY this same chain.
const brPrimaryEl = document.getElementById("br-primary");
if (brPrimaryEl) brPrimaryEl.onclick = () => {
  // Loop axis (any runner): SAVE a LoopConfig from the modal, then fire one manual iteration (P3).
  if (bridgeLoop) { submitBridgeLoop(); return; }
  // Headless runner (loop off): the unified submit (report-only when ship off, flywheel when ship on).
  if (bridgeRunner === "headless") { submitUnifiedHeadless(); return; }
  // Live-panes runner (loop off) — the UNCHANGED phase machine (ship on/off both ride this chain).
  if (bridgePhase === "planning") abortPlanning(); // in-place cancel: stop the plan, stay in the modal, re-press to retry
  else if (bridgePhase === "idle") synthesizeBridge();
  else if (bridgePhase === "preview") dispatchBridge();
  else if (bridgePhase === "ready") synthesizeResults();
  else if (bridgePhase === "prd") { if (bridgeShipDir) bridgeOpenPr(); else runFlywheelFromPrd(); }
};
// editing the goal while a plan is previewed = the plan is stale → back to "Plan tasks".
// Every edit also persists the DRAFT (survives Cancel/reopen/restart).
const brGoalEl = document.getElementById("br-goal");
if (brGoalEl) brGoalEl.addEventListener("input", () => {
  try { localStorage.setItem(LS_KEYS.bridgeGoalDraft, brGoalEl.value); } catch (_) {}
  if (bridgePhase === "preview") { resetBridgePreview(); setBridgePhase("idle"); }
  clearBridgePlan(); // edited goal → any saved plan is for the OLD goal; drop it so reopen re-plans
});
// UNIFY auto-spawn wiring: team-size tiles + "Spawn team".
const brSpawnGoEl = document.getElementById("br-spawn-go");
if (brSpawnGoEl) brSpawnGoEl.onclick = () => spawnBridgeTeam();
for (const t of document.querySelectorAll("#br-count .count-tile")) {
  t.onclick = () => {
    bridgeSpawnCount = Number(t.dataset.n) || 3;
    for (const x of document.querySelectorAll("#br-count .count-tile")) setChipActive(x, x === t);
  };
}
// 07-02: Auto-synthesize toggle (default ON, persisted). Flipping ON mid-run lets the
// next readiness poll auto-fire; OFF reverts to manual "Synthesize Results".
const brAutoEl = document.getElementById("br-auto");
if (brAutoEl) {
  brAutoEl.checked = bridgeAuto;
  brAutoEl.onchange = () => {
    bridgeAuto = brAutoEl.checked;
    try { localStorage.setItem("at_bridge_auto", bridgeAuto ? "1" : "0"); } catch (_) {}
  };
}
// UNIFY seam toggle (default OFF, persisted). ON reveals the "Run Flywheel" button after synthesis.
const brUnifyEl = document.getElementById("br-unify");
if (brUnifyEl) {
  brUnifyEl.checked = bridgeUnify;
  brUnifyEl.onchange = () => {
    bridgeUnify = brUnifyEl.checked;
    try { localStorage.setItem(LS_KEYS.bridgeUnify, bridgeUnify ? "1" : "0"); } catch (_) {}
    paintShipGateWarning(); // ship checked while flywheel_apply/flywheel_ship gated off → inline warn

    // ship-mode turned OFF at the PRD checkpoint → there's nothing left to do here; reset.
    if (!bridgeUnify && bridgePhase === "prd") { bridgePrdPath = null; bridgeShipDir = null; setBridgePhase("idle"); }
    // headless: the primary label is ship-driven (Delegate ↔ Start cycle) → repaint on toggle;
    // and the repo-folder row is ship-on-only → re-sync its visibility.
    if (bridgeRunner === "headless") { paintBridgePrimaryLabel(); syncBrHeadlessRepoRow(); }
  };
}
// L4b: "Fresh from main" toggle — opt-in, default OFF. When ON, Bridge auto-spawn resets
// every worker worktree to current main before spawning (07-03 / D41 freshen_worktree).
// Wired to the #br-fresh-toggle checkbox in the Bridge spawn sub-block.
const brFreshToggleEl = document.getElementById("br-fresh-toggle");
if (brFreshToggleEl) {
  brFreshToggleEl.checked = freshFromMainEnabled;
  brFreshToggleEl.onchange = () => {
    freshFromMainEnabled = brFreshToggleEl.checked;
    try { localStorage.setItem("at_fresh_from_main", freshFromMainEnabled ? "1" : "0"); } catch (_) {}
  };
}
// 07-04 two-wave is auto-decided per run (orchestrator emits `two_wave`) — no toggle to wire.

// ─────────────────── P2 unified entry: runner + loop toggle wiring ───────────────────
// setBridgeRunner swaps the panes ↔ headless sub-blocks and repaints the primary label.
//  - panes  : show #br-panes-wrap; restore the phase-driven label (the UNCHANGED phase machine).
//  - headless: show #br-headless; set a static label by ship; init the headless widgets + gate.
// The DEFAULT (panes) leaves the modal byte-identical to today — setBridgePhase keeps owning the label.
// The repo-folder row is ONLY meaningful for the ship-on auto-create bootstrap (flywheel parity).
// Report-only (ship-off) requires a live pane and ignores the repo input → hide it there to avoid
// implying it does anything. Re-evaluated on open, runner switch, and ship toggle.
function syncBrHeadlessRepoRow() {
  const noPane = !delegateParentId();
  document.getElementById("br-hl-repo-row")?.classList.toggle("hidden", !(noPane && bridgeUnify));
}
function paintBridgePrimaryLabel() {
  const b = document.getElementById("br-primary");
  if (!b) return;
  if (bridgeRunner === "headless") {
    b.classList.remove("hidden");
    b.disabled = false;
    b.textContent = bridgeUnify ? "Start cycle →" : "Delegate →";
    b.title = "";
    bcSyncAction();
  } else {
    // panes → hand the label back to the phase machine (idle → "Plan tasks →", etc.).
    setBridgePhase(bridgePhase);
  }
}
function setBridgeRunner(runner) {
  bridgeRunner = (runner === "headless") ? "headless" : "panes";
  try { localStorage.setItem(LS_KEYS.orchRunner, bridgeRunner); } catch (_) {}
  for (const t of document.querySelectorAll("#br-runner .count-tile")) setChipActive(t, t.dataset.runner === bridgeRunner);
  const panesWrap = document.getElementById("br-panes-wrap");
  const headless = document.getElementById("br-headless");
  if (panesWrap) panesWrap.classList.toggle("hidden", bridgeRunner === "headless");
  if (headless) headless.classList.toggle("hidden", bridgeRunner !== "headless");
  if (bridgeRunner === "headless") {
    initBrHeadless();
    syncBrHeadlessRepoRow();
    refreshBridgeHeadlessGate();
  } else {
    document.getElementById("br-hl-gate")?.classList.add("hidden");
  }
  paintBridgePrimaryLabel();
}
const brRunnerEl = document.getElementById("br-runner");
if (brRunnerEl) brRunnerEl.addEventListener("click", (e) => {
  const tile = e.target.closest(".count-tile");
  if (!tile) return;
  setBridgeRunner(tile.dataset.runner);
});
// Loop toggle (default OFF, persisted). ON reveals the #br-loop-config stub + repaints the primary
// label note. The loop ROUTE is a STUB (P3) — wired in the #br-primary router as "coming soon".
const brLoopEl = document.getElementById("br-loop");
if (brLoopEl) {
  brLoopEl.checked = bridgeLoop;
  brLoopEl.onchange = () => {
    bridgeLoop = brLoopEl.checked;
    try { localStorage.setItem(LS_KEYS.orchLoop, bridgeLoop ? "1" : "0"); } catch (_) {}
    document.getElementById("br-loop-config")?.classList.toggle("hidden", !bridgeLoop);
  };
}

// ---- Speak & Send (Plan 05-03): human replies to the top who-needs-you agent ----
// Voice + reply: speak the human's text aloud, THEN deliver it to the agent at the
// top of the queue (same target as ⌘⇧J). Model-A (D9-D10 / AC-5): the human authors
// EVERY reply and explicitly clicks Send — nothing is auto-generated or auto-sent.
const speakEl = document.getElementById("speak");
let speakTargetId = null; // locked at open so we reply to the agent the human SAW

// the agent to reply to: top-of-queue needs_human (same as focusTop), skipping any
// pane we already know is dead (the backend can keep listing a corpse as needs_human).
function speakTopTarget(queue) {
  const q = queue || [];
  return q.find((r) => r.needs_human && !deadPanes.has(r.id)) || q.find((r) => !deadPanes.has(r.id)) || null;
}

async function openSpeak() {
  if (!hasTauri()) { showToast("Tauri API unavailable"); return; }
  let queue = [];
  try { queue = await invoke("list_queue"); } catch (_) { queue = _lastQueue; }
  const top = speakTopTarget(queue);
  if (!top) { showToast("No agent needs a reply — nothing to send."); return; } // skip + clear toast
  speakTargetId = top.id;
  document.getElementById("sp-target-id").textContent = top.id;
  const meta = document.getElementById("sp-target-meta");
  // why it needs you (approval / question) — amber, the same signal as the queue
  meta.textContent = top.needs_human ? (top.reason && top.reason !== "-" ? top.reason : "needs you") : (top.state || "");
  document.getElementById("sp-error").textContent = "";
  const ta = document.getElementById("sp-text");
  ta.value = "";
  // reset push-to-talk to idle each open (unless the mic was permanently denied this
  // session — keep it disabled so we don't re-prompt a known-denied device).
  if (typeof setMicState === "function") setMicState(micDisabled ? "disabled" : "idle");
  speakEl.classList.remove("hidden");
  trapModalFocus(speakEl);
  requestAnimationFrame(() => ta.focus());
}

function closeSpeak() { speakEl.classList.add("hidden"); releaseModalFocus(speakEl); }

// mark a pane dead → light its error dot NOW (don't wait for the 1s poll) and stop
// re-targeting it. Re-renders the workspace list from the cached last poll.
function markPaneDead(id) {
  if (!id) return;
  deadPanes.add(id);
  renderWorkspaces(_lastQueue);
}

// Mark NEWLY-dead panes (light the D30 red dot, stop re-targeting) and surface ONE
// coalesced "N panes dead, skipped" toast — DEBOUNCED so a burst of broadcast
// keystrokes hitting several corpses yields a single toast, not one per keystroke.
// Already-dead ids are ignored (they're filtered out before the send, so they never
// re-trigger). Used by broadcast; dispatch counts its own (discrete) and toasts inline.
let _deadToastTimer = null;
let _deadToastIds = new Set();
function noteDeadPanes(ids) {
  let added = false;
  for (const id of ids) { if (id && !deadPanes.has(id)) { deadPanes.add(id); _deadToastIds.add(id); added = true; } }
  if (!added) return;
  resetQueueSig(); // death changes the rail/board tiers too — make the next tick repaint them
  renderWorkspaces(_lastQueue);
  clearTimeout(_deadToastTimer);
  _deadToastTimer = setTimeout(() => {
    const n = _deadToastIds.size;
    _deadToastIds = new Set();
    if (n > 0) showToast(`${n} pane${n === 1 ? "" : "s"} dead, skipped`);
  }, 400);
}

// Speak the reply aloud, THEN send it to the locked target. ONE line, single
// trailing newline (send_input types raw into the agent's TUI where \n submits —
// mirrors dispatchBridge). Voice is auxiliary: a speak() failure never blocks the
// reply (and degrades to send-only before the backend `speak` command lands).
async function sendSpeak() {
  const err = document.getElementById("sp-error");
  err.textContent = "";
  if (!hasTauri()) { err.textContent = "Tauri API unavailable."; return; }
  if (!speakTargetId) { showToast("No agent needs a reply — nothing to send."); closeSpeak(); return; }
  const raw = document.getElementById("sp-text").value;
  const reply = raw.replace(/\s*\n\s*/g, " ").trim(); // collapse newlines → one line
  if (!reply) { err.textContent = "Type a reply first."; return; }
  const target = speakTargetId;
  const btn = document.getElementById("sp-send");
  btn.disabled = true;
  btn.textContent = "Speaking…"; // TTS can take a beat — mirror Bridge's busy label
  try {
    try { await invoke("speak", { text: reply }); } catch (_) {} // voice best-effort
    await invoke("send_input", { id: target, data: reply + "\n" });
    showToast(`Sent to ${target}`);
    closeSpeak();
  } catch (e) {
    const msg = String(e);
    // dead pane: backend returns "workspace is no longer alive" (planned) or "no such
    // workspace" (current). Either way → skip + toast + light the error dot (D30).
    if (/no longer alive|no such workspace|not alive/i.test(msg)) {
      markPaneDead(target);
      showToast("1 pane dead, skipped");
      closeSpeak();
    } else {
      err.textContent = "Send failed: " + msg;
    }
  } finally {
    btn.disabled = false;
    btn.replaceChildren(svgIcon("i-send"), document.createTextNode(" Speak & Send")); // restore label + icon
  }
}

const spCancelEl = document.getElementById("sp-cancel");
if (spCancelEl) spCancelEl.onclick = () => closeSpeak();
const spSendEl = document.getElementById("sp-send");
if (spSendEl) spSendEl.onclick = () => sendSpeak();
const spTextEl = document.getElementById("sp-text");
// ⌘↵ / Ctrl+↵ in the textarea → Speak & Send (an explicit human action, not auto-send)
if (spTextEl) spTextEl.addEventListener("keydown", (e) => {
  if ((e.metaKey || e.ctrlKey) && e.key === "Enter") { e.preventDefault(); sendSpeak(); }
});

// ---- Push-to-talk dictation (Plan 05-04): hold the mic → in-app capture → whisper →
// FILL the reply field above. Model-A unchanged: the transcript only fills the field;
// the human still reviews + presses Speak & Send (no auto-send, no new inject path).
// Capture lives in the entitled APP (cpal/TCC); the file-only whisper sidecar needs no
// TCC. Mic-denial degrades: the button disables + a toast, and typed replies keep
// working (AC-4) — never a crash.
const spMicEl = document.getElementById("sp-mic");
const spMicLabelEl = document.getElementById("sp-mic-label");
let micState = "idle"; // idle | recording | transcribing | disabled
let micDisabled = false; // sticky once a deny/error is seen this session

function setMicLabel(text) { if (spMicLabelEl) spMicLabelEl.textContent = text; }

// reflect dictation state on the button (an explicit "active" affordance — the
// recording class is a deliberate accent, NOT a status-queue color, per the lane rule).
function setMicState(s) {
  micState = s;
  if (!spMicEl) return;
  spMicEl.classList.toggle("recording", s === "recording");
  spMicEl.classList.toggle("busy", s === "transcribing");
  spMicEl.disabled = (s === "transcribing" || s === "disabled");
  setMicLabel(
    s === "recording" ? "Listening… release to transcribe" :
    s === "transcribing" ? "Transcribing…" :
    s === "disabled" ? "Mic unavailable" :
    "Hold to talk"
  );
}

// mic permission denied / device error → disable PTT for the session + toast. Typed
// replies + Speak & Send still work (AC-4). DENY surfaces as a backend start error.
function disableMic(reason) {
  micDisabled = true;
  setMicState("disabled");
  showToast(reason || "Mic access needed for dictation — type your reply instead.");
}

let _micBusy = false; // guard against overlapping press/release races
async function micPressStart() {
  if (micDisabled || _micBusy || micState !== "idle") return;
  if (!hasTauri()) { disableMic("Dictation needs the desktop app."); return; }
  _micBusy = true;
  try {
    await invoke("start_dictation");
    setMicState("recording");
  } catch (e) {
    // backend returns a typed error on mic DENY / no-device — degrade, never crash.
    disableMic("Mic access needed for dictation — " + String(e).replace(/^.*?: /, ""));
  } finally {
    _micBusy = false;
  }
}

async function micPressEnd() {
  if (micState !== "recording" || _micBusy) return;
  _micBusy = true;
  setMicState("transcribing");
  try {
    const wav = await invoke("stop_dictation");
    const text = await invoke("transcribe", { wavPath: wav });
    const t = (text || "").trim();
    if (t && spTextEl) {
      // FILL the reply field — append to whatever the human already typed (Model-A:
      // they review + send). Add a space if appending mid-text.
      const cur = spTextEl.value;
      spTextEl.value = cur && !/\s$/.test(cur) ? cur + " " + t : cur + t;
      spTextEl.focus();
    } else if (!t) {
      showToast("Didn't catch that — try again or type your reply.");
    }
    setMicState("idle");
  } catch (e) {
    // transcription/convert failure → degrade to idle (typed reply still works).
    setMicState("idle");
    showToast("Dictation failed: " + String(e).replace(/^.*?: /, ""));
  } finally {
    _micBusy = false;
  }
}

if (spMicEl) {
  // hold = record, release/leave = transcribe. Pointer events cover mouse + touch +
  // pen; releasing OUTSIDE the button (pointerleave while pressed) also stops, so a
  // drag-off never leaves the mic stuck open.
  spMicEl.addEventListener("pointerdown", (e) => { e.preventDefault(); micPressStart(); });
  spMicEl.addEventListener("pointerup", (e) => { e.preventDefault(); micPressEnd(); });
  spMicEl.addEventListener("pointerleave", () => { if (micState === "recording") micPressEnd(); });
  // keyboard a11y: Space/Enter held on the focused button = press, keyup = release.
  spMicEl.addEventListener("keydown", (e) => {
    if ((e.key === " " || e.key === "Enter") && !e.repeat) { e.preventDefault(); micPressStart(); }
  });
  spMicEl.addEventListener("keyup", (e) => {
    if (e.key === " " || e.key === "Enter") { e.preventDefault(); micPressEnd(); }
  });
}

// ---- Settings → MCP loopback HTTP (advanced) ---------------------------------
// DRAFT for human security review (06-02 Phase B). There is no pre-existing Settings
// surface in index.html — this whole modal + its topbar trigger are built at runtime
// (same pattern as showToast: createElement + append to body), so no HTML/CSS file is
// touched. Backend is the single source of truth: we never cache `enabled` locally and
// always re-read mcp_http_status(). The token is sensitive — rendered via textContent
// only (never innerHTML / console.log), masked by default, revealed on demand.
//
// SECURITY NOTE (for the reviewer reading this code, not enforced here): on TCP
// 127.0.0.1 the Unix-socket peer-euid gate is gone — the Bearer token (a 0600 file,
// at-rest secrecy = the same-user boundary) + Origin/Host checks ARE the gate. The
// transport is double-gated OFF by default: it binds only if http_enabled, and every
// mutating op STILL passes allow_mutations inside handle_socket_request. All of that
// lives in the backend; this lane is pure UI that calls invoke() — it neither
// reimplements nor weakens any of it.
let settingsEl = null;          // the overlay (built lazily on first open)
let mcpHttpTokenShown = false;  // reveal/mask state for the token row (reset on each open)
let mcpHttpToken = "";          // last fetched/regenerated token, held only in-memory for copy/reveal

// Build the settings overlay once. The overlay LAYOUT (fixed/inset/scrim/flex-center)
// is ID-selectored in CSS as `#modal,#bridge,#speak` — a new `#settings` id won't match
// it — so those few props are set inline here (CSS custom properties resolve fine in
// inline styles). Everything inside reuses the class-selectored design-token styles
// (.modal-card, .modal-sub, .modal-actions, .primary) for free.
function buildSettings() {
  if (settingsEl) return settingsEl;
  const overlay = document.createElement("div");
  overlay.id = "settings";
  overlay.className = "hidden";
  Object.assign(overlay.style, {
    position: "fixed", inset: "0", background: "var(--scrim)",
    display: "flex", alignItems: "center", justifyContent: "center", zIndex: "50",
  });

  const card = document.createElement("div");
  card.className = "modal-card";
  card.style.width = "480px";

  const h2 = document.createElement("h2");
  h2.textContent = "Settings";
  card.appendChild(h2);

  // --- SKIN picker (re-skins the whole app by swapping html[data-skin]) ---
  const skinLabel = document.createElement("div");
  skinLabel.className = "br-section-label";
  skinLabel.textContent = "SKIN";
  card.appendChild(skinLabel);

  const skinRow = document.createElement("div");
  skinRow.id = "skin-row";
  skinRow.className = "chip-row";
  const SKIN_TILES = [
    ["nothing", "Nothing"],
    ["aurora", "Aurora"],
    ["atelier", "Atelier"],
    ["phosphor", "Phosphor"],
    ["precision", "Precision"],
    ["liquid-glass", "Liquid Glass"],
  ];
  const active = currentSkin();
  for (const [name, display] of SKIN_TILES) {
    const tile = document.createElement("button");
    tile.type = "button";
    tile.className = "count-tile skin-tile" + (name === active ? " active" : "");
    tile.setAttribute("aria-pressed", name === active ? "true" : "false");
    tile.dataset.skin = name;
    tile.textContent = display;
    tile.addEventListener("click", () => {
      setSkin(name);
      // Repaint the active marker live across the whole row.
      for (const t of skinRow.querySelectorAll(".skin-tile")) {
        setChipActive(t, t.dataset.skin === name);
      }
    });
    skinRow.appendChild(tile);
  }
  card.appendChild(skinRow);

  // --- TRUSTED REPOSITORIES — navigable allowlist (add / remove) instead of editing
  //     trusted-repos.txt by hand. Arming a repo lets autonomous workers run its build/tests
  //     in throwaway worktrees (NOT a sandbox — cargo runs arbitrary code), so ADD is
  //     confirm-gated (two-click); REMOVE only revokes trust, so it needs no confirm. ---
  const trLabel = document.createElement("div");
  trLabel.className = "br-section-label";
  trLabel.textContent = "TRUSTED REPOSITORIES";
  card.appendChild(trLabel);

  const trHint = document.createElement("p");
  trHint.className = "modal-sub";
  trHint.style.cssText = "margin:0 0 var(--s-2);";
  trHint.textContent =
    "Repos your agents may run autonomous workers in (build/tests in throwaway worktrees). " +
    "Remove to revoke. Adding arms a repo — workers execute its code, so only add repos you trust.";
  card.appendChild(trHint);

  const trList = document.createElement("div");
  trList.id = "trusted-repos-list";
  trList.style.cssText =
    "display:flex;flex-direction:column;gap:var(--s-1);margin:0 0 var(--s-2);max-height:180px;overflow:auto;";
  card.appendChild(trList);

  const trAddRow = document.createElement("div");
  trAddRow.style.cssText = "display:flex;gap:var(--s-1);align-items:center;";
  const trInput = document.createElement("input");
  trInput.id = "trusted-repo-add-input";
  trInput.type = "text";
  trInput.placeholder = "/absolute/path/to/repo";
  trInput.setAttribute("aria-label", "Repository path to trust");
  trInput.style.cssText =
    "flex:1;min-width:0;padding:var(--s-1) var(--s-2);border:1px solid var(--line);" +
    "border-radius:var(--r-2);background:var(--bg);color:var(--fg);font-family:var(--font-mono);font-size:12px;";
  const trAddBtn = document.createElement("button");
  trAddBtn.id = "trusted-repo-add-btn";
  trAddBtn.type = "button";
  trAddBtn.textContent = "Add";
  trAddBtn.style.cssText =
    "padding:var(--s-1) var(--s-3);border-radius:var(--r-2);border:1px solid var(--line-strong);" +
    "background:var(--surface-2);color:var(--fg);cursor:pointer;font-size:12px;white-space:nowrap;";
  trAddRow.append(trInput, trAddBtn);
  card.appendChild(trAddRow);

  const trAddErr = document.createElement("p");
  trAddErr.id = "trusted-repo-add-err";
  trAddErr.className = "modal-sub";
  trAddErr.style.cssText = "min-height:14px;margin:var(--s-1) 0 var(--s-3);";
  card.appendChild(trAddErr);

  trAddBtn.addEventListener("click", () => addTrustedRepoFromSettings());
  trInput.addEventListener("keydown", (e) => { if (e.key === "Enter") addTrustedRepoFromSettings(); });

  // --- CONFIG FILES — reveal the on-disk files (no hunting). The mutation-authority gates
  //     (allow_mutations, flywheel_apply/ship, …) stay file-only BY DESIGN — a compromised
  //     webview must not be able to flip mutation authority — so they're not editable here;
  //     this just opens the file for the rare manual edit. ---
  const cfgLabel = document.createElement("div");
  cfgLabel.className = "br-section-label";
  cfgLabel.textContent = "CONFIG FILES";
  card.appendChild(cfgLabel);

  const cfgHint = document.createElement("p");
  cfgHint.className = "modal-sub";
  cfgHint.style.cssText = "margin:0 0 var(--s-2);";
  cfgHint.textContent =
    "The mutation-authority gates (allow_mutations, flywheel_apply/ship, …) are file-only by " +
    "design — reveal the file to edit them by hand. Live gate state shows in the arming section below.";
  card.appendChild(cfgHint);

  const cfgRow = document.createElement("div");
  cfgRow.style.cssText = "display:flex;gap:var(--s-1);flex-wrap:wrap;margin:0 0 var(--s-3);";
  const revealCfgBtn = document.createElement("button");
  revealCfgBtn.type = "button";
  revealCfgBtn.textContent = "Reveal mcp-config.json";
  revealCfgBtn.style.cssText =
    "padding:var(--s-1) var(--s-3);border-radius:var(--r-2);border:1px solid var(--line-strong);" +
    "background:var(--surface-2);color:var(--fg);cursor:pointer;font-size:12px;";
  const revealTrBtn = document.createElement("button");
  revealTrBtn.type = "button";
  revealTrBtn.textContent = "Reveal trusted-repos.txt";
  revealTrBtn.style.cssText = revealCfgBtn.style.cssText;
  revealCfgBtn.addEventListener("click", () => revealConfigFile("mcp_config"));
  revealTrBtn.addEventListener("click", () => revealConfigFile("trusted_repos"));
  cfgRow.append(revealCfgBtn, revealTrBtn);
  card.appendChild(cfgRow);

  // --- subsection header ---
  const subLabel = document.createElement("div");
  subLabel.className = "br-section-label";
  subLabel.textContent = "MCP loopback HTTP (advanced)";
  card.appendChild(subLabel);

  // --- enable toggle row ---
  const toggleRow = document.createElement("label");
  toggleRow.style.display = "flex";
  toggleRow.style.alignItems = "center";
  toggleRow.style.gap = "var(--s-2)";
  const toggle = document.createElement("input");
  toggle.type = "checkbox";
  toggle.id = "mcp-http-toggle";
  toggle.style.width = "auto";
  toggle.style.margin = "0";
  const toggleText = document.createElement("span");
  toggleText.textContent = "Enable loopback HTTP control plane";
  toggleText.style.color = "var(--fg)";
  toggleText.style.fontSize = "13px";
  toggleRow.append(toggle, toggleText);
  card.appendChild(toggleRow);

  // "takes effect on next launch" note — shown after a toggle, hidden otherwise.
  const launchNote = document.createElement("p");
  launchNote.id = "mcp-http-launch-note";
  launchNote.className = "modal-sub hidden";
  launchNote.textContent = "Takes effect on next app launch (the listener binds at startup).";
  launchNote.style.margin = "0 0 var(--s-3)";
  card.appendChild(launchNote);

  // --- status block ---
  const status = document.createElement("div");
  status.id = "mcp-http-status";
  status.style.cssText =
    "font-size:12px;line-height:1.6;color:var(--fg-secondary);" +
    "background:var(--bg);border:1px solid var(--line);border-radius:var(--r-2);" +
    "padding:var(--s-2) var(--s-3);margin:0 0 var(--s-3);font-family:var(--font-mono);";
  status.textContent = "Loading status…";
  card.appendChild(status);

  // --- token row ---
  const tokenLabel = document.createElement("div");
  tokenLabel.className = "br-section-label";
  tokenLabel.textContent = "Bearer token";
  card.appendChild(tokenLabel);

  const tokenField = document.createElement("div");
  tokenField.id = "mcp-http-token";
  tokenField.textContent = "••••••••  (hidden)";
  tokenField.style.cssText =
    "font-family:var(--font-mono);font-size:12px;color:var(--fg);" +
    "background:var(--bg);border:1px solid var(--line);border-radius:var(--r-2);" +
    "padding:var(--s-2) var(--s-3);margin:0 0 var(--s-2);word-break:break-all;min-height:18px;";
  card.appendChild(tokenField);

  const tokenActions = document.createElement("div");
  tokenActions.style.cssText = "display:flex;gap:var(--s-2);margin:0 0 var(--s-3);";
  const mkBtn = (id, text) => {
    const b = document.createElement("button");
    b.id = id;
    b.type = "button";
    b.textContent = text;
    b.style.cssText =
      "padding:var(--s-1) var(--s-3);border-radius:var(--r-2);border:1px solid var(--line);" +
      "background:var(--bg);color:var(--fg);cursor:pointer;font-size:12px;font-weight:var(--fw-medium);";
    return b;
  };
  const revealBtn = mkBtn("mcp-http-reveal", "Reveal");
  const copyBtn = mkBtn("mcp-http-copy", "Copy");
  const regenBtn = mkBtn("mcp-http-regen", "Regenerate");
  tokenActions.append(revealBtn, copyBtn, regenBtn);
  card.appendChild(tokenActions);

  // --- warning line ---
  const warn = document.createElement("p");
  warn.className = "modal-sub";
  warn.style.color = "var(--warning)";
  warn.style.margin = "0 0 var(--s-4)";
  warn.textContent =
    "Warning: enabling exposes a local HTTP control plane on 127.0.0.1 guarded ONLY " +
    "by this Bearer token. Mutating calls STILL require allow_mutations in " +
    "mcp-config.json. Keep this token secret — anyone who can read it controls the plane.";
  card.appendChild(warn);

  // --- Autonomous delegation (arming) subsection (P1.7) ---
  // Writes ONLY autonomy_ceiling (the delegation-specific gate); allow_mutations + delegate-live are
  // shown READ-ONLY. allow_mutations gates EVERY Phase-B mutation tool, so it stays file-only here.
  const dlSubLabel = document.createElement("div");
  dlSubLabel.className = "br-section-label";
  dlSubLabel.textContent = "Autonomous delegation (arming)";
  card.appendChild(dlSubLabel);

  const dlGateStatus = document.createElement("div");
  dlGateStatus.id = "dl-gate-status";
  dlGateStatus.style.cssText =
    "font-size:12px;line-height:1.6;color:var(--muted);" +
    "background:var(--bg);border:1px solid var(--line);border-radius:var(--r-2);" +
    "padding:var(--s-2) var(--s-3);margin:0 0 var(--s-3);font-family:var(--font-mono);";
  dlGateStatus.textContent = "Loading gates…";
  card.appendChild(dlGateStatus);

  const armRow = document.createElement("label");
  armRow.style.cssText = "display:flex;align-items:center;gap:var(--s-2);";
  const armToggle = document.createElement("input");
  armToggle.type = "checkbox";
  armToggle.id = "dl-arm-toggle";
  armToggle.style.cssText = "width:auto;margin:0;";
  const armText = document.createElement("span");
  armText.textContent = "Arm autonomous delegation (sets autonomy_ceiling ≥ 1)";
  armText.style.cssText = "color:var(--fg);font-size:13px;";
  armRow.append(armToggle, armText);
  card.appendChild(armRow);

  const armConfirm = document.createElement("div");
  armConfirm.id = "dl-arm-confirm";
  armConfirm.className = "hidden";
  armConfirm.style.cssText = "display:flex;align-items:center;gap:var(--s-2);margin:var(--s-2) 0;";
  const armConfirmBtn = document.createElement("button");
  armConfirmBtn.id = "dl-arm-confirm-btn";
  armConfirmBtn.type = "button";
  armConfirmBtn.textContent = "Confirm — arm it";
  armConfirmBtn.style.cssText =
    "padding:var(--s-1) var(--s-3);border-radius:var(--r-2);border:1px solid var(--danger);" +
    "background:var(--danger);color:var(--on-accent);cursor:pointer;font-size:12px;font-weight:var(--fw-medium);";
  const armCancelBtn = document.createElement("button");
  armCancelBtn.id = "dl-arm-cancel-btn";
  armCancelBtn.type = "button";
  armCancelBtn.textContent = "Cancel";
  armCancelBtn.style.cssText =
    "padding:var(--s-1) var(--s-3);border-radius:var(--r-2);border:1px solid var(--line);" +
    "background:var(--bg);color:var(--fg);cursor:pointer;font-size:12px;";
  armConfirm.append(armConfirmBtn, armCancelBtn);
  card.appendChild(armConfirm);

  const armTrust = document.createElement("p");
  armTrust.className = "modal-sub";
  armTrust.style.cssText = "color:var(--warning);margin:var(--s-1) 0 var(--s-3);";
  armTrust.textContent =
    "Arming lets your agents spawn ephemeral workers unattended. Workers run your repo's build/tests in " +
    "throwaway git worktrees, swept when done — this is NOT a sandbox (cargo runs arbitrary code). Only arm " +
    "in a repo you trust. Arming sets ONLY autonomy_ceiling; it does NOT enable mutations or the delegate-live build.";
  card.appendChild(armTrust);

  armToggle.addEventListener("change", () => onArmToggle(armToggle.checked));
  armConfirmBtn.addEventListener("click", () => armSetAutonomy(true));
  armCancelBtn.addEventListener("click", () => { armToggle.checked = false; hideArmConfirm(); });

  // --- agent→agent send-input arm (narrow, confirm-gated; NARROWER than allow_mutations) ---
  const siStatus = document.createElement("p");
  siStatus.id = "si-gate-status";
  siStatus.className = "modal-sub";
  siStatus.style.cssText =
    "background:var(--surface-2);border-radius:var(--r-2);padding:var(--s-2) var(--s-3);" +
    "margin:var(--s-3) 0 var(--s-2);font-family:var(--font-mono);";
  siStatus.textContent = "Loading…";
  card.appendChild(siStatus);

  const siRow = document.createElement("label");
  siRow.style.cssText = "display:flex;align-items:center;gap:var(--s-2);";
  const siToggle = document.createElement("input");
  siToggle.type = "checkbox";
  siToggle.id = "si-arm-toggle";
  siToggle.style.cssText = "width:auto;margin:0;";
  const siText = document.createElement("span");
  siText.textContent = "Enable agent→agent send-input (a Coordinator pane prompts another pane)";
  siText.style.cssText = "color:var(--fg);font-size:13px;";
  siRow.append(siToggle, siText);
  card.appendChild(siRow);

  const siConfirm = document.createElement("div");
  siConfirm.id = "si-arm-confirm";
  siConfirm.className = "hidden";
  siConfirm.style.cssText = "display:flex;align-items:center;gap:var(--s-2);margin:var(--s-2) 0;";
  const siConfirmBtn = document.createElement("button");
  siConfirmBtn.id = "si-arm-confirm-btn";
  siConfirmBtn.type = "button";
  siConfirmBtn.textContent = "Confirm — arm send-input";
  siConfirmBtn.style.cssText =
    "padding:var(--s-1) var(--s-3);border-radius:var(--r-2);border:1px solid var(--danger);" +
    "background:var(--danger);color:var(--on-accent);cursor:pointer;font-size:12px;font-weight:var(--fw-medium);";
  const siCancelBtn = document.createElement("button");
  siCancelBtn.id = "si-arm-cancel-btn";
  siCancelBtn.type = "button";
  siCancelBtn.textContent = "Cancel";
  siCancelBtn.style.cssText =
    "padding:var(--s-1) var(--s-3);border-radius:var(--r-2);border:1px solid var(--line);" +
    "background:var(--bg);color:var(--fg);cursor:pointer;font-size:12px;";
  siConfirm.append(siConfirmBtn, siCancelBtn);
  card.appendChild(siConfirm);

  const siTrust = document.createElement("p");
  siTrust.className = "modal-sub";
  siTrust.style.cssText = "color:var(--warning);margin:var(--s-1) 0 var(--s-3);";
  siTrust.textContent =
    "Lets a Coordinator-role pane write one line into ANOTHER live pane's input (cross-agent prompting). " +
    "Still gated: only a Coordinator can send, the line is single-line/no-control-bytes, the target must be " +
    "alive. Narrower than allow_mutations (does NOT enable orchestrate/broadcast/delegate). Takes effect on " +
    "the next call — no respawn. A Coordinator pane needs the coordinator build to actually have the tool.";
  card.appendChild(siTrust);

  siToggle.addEventListener("change", () => onSendInputToggle(siToggle.checked));
  siConfirmBtn.addEventListener("click", () => setSendInputEnabled(true));
  siCancelBtn.addEventListener("click", () => { siToggle.checked = false; siHideConfirm(); });

  // --- error + close ---
  const errP = document.createElement("p");
  errP.id = "mcp-http-error";
  errP.style.cssText = "color:var(--danger);font-size:12px;line-height:1.45;min-height:16px;margin:0 0 var(--s-2);";
  card.appendChild(errP);

  const actions = document.createElement("div");
  actions.className = "modal-actions";
  const closeBtn = document.createElement("button");
  closeBtn.id = "mcp-http-close";
  closeBtn.type = "button";
  closeBtn.textContent = "Close";
  actions.appendChild(closeBtn);
  card.appendChild(actions);

  overlay.appendChild(card);
  document.body.appendChild(overlay);
  settingsEl = overlay;

  // wire interactions
  toggle.addEventListener("change", () => mcpHttpSetEnabled(toggle.checked));
  revealBtn.addEventListener("click", mcpHttpReveal);
  copyBtn.addEventListener("click", mcpHttpCopy);
  regenBtn.addEventListener("click", mcpHttpRegen);
  closeBtn.addEventListener("click", closeSettings);
  // click the scrim (outside the card) to dismiss
  overlay.addEventListener("mousedown", (e) => { if (e.target === overlay) closeSettings(); });

  return overlay;
}

function setMcpHttpError(msg) {
  const el = document.getElementById("mcp-http-error");
  if (el) el.textContent = msg || "";
}

// Mask the token field and reset reveal state (called on open and after mask).
function maskMcpHttpToken() {
  mcpHttpTokenShown = false;
  const field = document.getElementById("mcp-http-token");
  const btn = document.getElementById("mcp-http-reveal");
  if (field) field.textContent = mcpHttpToken ? "••••••••  (hidden)" : "(no token yet)";
  if (btn) btn.textContent = "Reveal";
}

// Read backend status (the single source of truth) and repaint the modal.
async function refreshMcpHttpStatus() {
  if (!hasTauri()) { setMcpHttpError("Tauri API unavailable"); return; }
  try {
    const s = await invoke("mcp_http_status"); // { enabled, port, token_present }
    const toggle = document.getElementById("mcp-http-toggle");
    if (toggle) toggle.checked = !!(s && s.enabled);
    const status = document.getElementById("mcp-http-status");
    if (status) {
      const enabled = s && s.enabled ? "yes" : "no";
      const port = s && s.port != null ? String(s.port) : "not running";
      const tok = s && s.token_present ? "present" : "missing";
      status.textContent = `enabled: ${enabled}   ·   bound port: ${port}   ·   token: ${tok}`;
    }
    // keep the masked token field agreeing with token_present (the source of truth) —
    // a token can exist on the backend before we've ever revealed it this session, so
    // the mask must read "hidden", not "(no token yet)". Don't clobber a live reveal.
    if (!mcpHttpTokenShown) {
      const field = document.getElementById("mcp-http-token");
      if (field) field.textContent = s && s.token_present ? "••••••••  (hidden)" : "(no token yet)";
    }
  } catch (e) {
    setMcpHttpError(String(e));
  }
}

async function openSettings() {
  if (!hasTauri()) { showToast("Tauri API unavailable"); return; }
  buildSettings();
  setMcpHttpError("");
  // forget any previously revealed token + hide the launch note from a prior session
  mcpHttpToken = "";
  maskMcpHttpToken();
  document.getElementById("mcp-http-launch-note")?.classList.add("hidden");
  settingsEl.classList.remove("hidden");
  trapModalFocus(settingsEl);
  await refreshMcpHttpStatus();
  hideArmConfirm(); // P1.7: a prior session's revealed arm-confirm shouldn't persist
  trustedAddPending = null; // don't carry a half-armed Add across opens
  await refreshTrustedRepos(); // paint the trusted-repo list from backend truth
  await refreshDelegateGate(); // P1.7: paint the arming readout + toggle from backend truth
}

function closeSettings() { if (settingsEl) { settingsEl.classList.add("hidden"); releaseModalFocus(settingsEl); } }

// ---- Trusted-repositories manager (Settings) --------------------------------------------
let trustedAddPending = null;      // repo path awaiting the confirming 2nd Add click
let trustedAddPendingAt = 0;       // timestamp of the 1st Add click (6s confirm window)

// (Re)render the trusted-repo list from backend truth. Each row = path + Remove.
async function refreshTrustedRepos() {
  const list = document.getElementById("trusted-repos-list");
  if (!list || !hasTauri()) return;
  let repos = [];
  try { repos = await invoke("list_trusted_repos"); } catch (_) { repos = []; }
  if (!Array.isArray(repos)) repos = []; // older backend without the command → null
  renderTrustedReposList(list, repos, removeTrustedRepo); // DOM builder lives in ./settings-core.js
}

// Add is privilege-escalating (workers run the repo's code) → two-click confirm, mirroring the
// arm-confirm pattern used elsewhere (no window.confirm — WKWebView may no-op it).
async function addTrustedRepoFromSettings() {
  const input = document.getElementById("trusted-repo-add-input");
  const err = document.getElementById("trusted-repo-add-err");
  if (!input || !hasTauri()) return;
  const path = (input.value || "").trim();
  if (err) { err.style.color = "var(--warning)"; err.textContent = ""; }
  if (!path) { if (err) { err.style.color = "var(--danger)"; err.textContent = "Enter a repository path."; } return; }

  const now = Date.now();
  const dec = trustedAddDecision(trustedAddPending, trustedAddPendingAt, now, path); // pure; ./settings-core.js
  trustedAddPending = dec.pending;
  trustedAddPendingAt = dec.pendingAt;
  if (dec.action === "confirm") {
    try {
      const armed = await invoke("trust_repo_cmd", { path });
      input.value = "";
      if (err) err.textContent = "";
      showToast("Trusted: " + armed);
      await refreshTrustedRepos();
    } catch (e) {
      if (err) { err.style.color = "var(--danger)"; err.textContent = String(e); }
    }
    return;
  }
  if (err) {
    err.style.color = "var(--warning)";
    err.textContent = "⚠ Arming lets workers run this repo's build/tests. Click Add again within 6s to confirm.";
  }
}

async function removeTrustedRepo(path) {
  if (!hasTauri()) return;
  try {
    await invoke("untrust_repo_cmd", { path });
    showToast("Removed from trusted repos.");
    await refreshTrustedRepos();
  } catch (e) {
    showToast("Couldn't remove: " + e);
  }
}

// Open a config file in Finder so the user doesn't have to hunt for it.
async function revealConfigFile(which) {
  if (!hasTauri()) return;
  try {
    const locs = await invoke("config_file_locations");
    const path = which === "trusted_repos" ? locs.trusted_repos : locs.mcp_config;
    if (!path) { showToast("Path unavailable."); return; }
    await invoke("reveal_path", { path });
  } catch (e) {
    showToast("Couldn't reveal file: " + e);
  }
}

// Toggle enable/disable. Backend writes http_enabled in mcp-config.json and ensures a
// token exists on enable; binding only takes effect next launch, so we surface that note
// and then re-read status (which still reflects the live listener, not the new flag).
async function mcpHttpSetEnabled(enabled) {
  if (!hasTauri()) { setMcpHttpError("Tauri API unavailable"); return; }
  setMcpHttpError("");
  try {
    await invoke("mcp_http_set_enabled", { enabled });
    document.getElementById("mcp-http-launch-note")?.classList.remove("hidden");
  } catch (e) {
    setMcpHttpError(String(e));
    // revert the checkbox to the backend's actual state on failure
  }
  await refreshMcpHttpStatus();
}

// Reveal toggles between the real token (fetched same-user from the 0600 file) and the mask.
async function mcpHttpReveal() {
  const field = document.getElementById("mcp-http-token");
  const btn = document.getElementById("mcp-http-reveal");
  if (mcpHttpTokenShown) { maskMcpHttpToken(); return; }
  if (!hasTauri()) { setMcpHttpError("Tauri API unavailable"); return; }
  setMcpHttpError("");
  try {
    mcpHttpToken = await invoke("mcp_http_reveal_token");
    if (field) field.textContent = mcpHttpToken || "(no token yet)";
    if (btn) btn.textContent = "Hide";
    mcpHttpTokenShown = true;
  } catch (e) {
    setMcpHttpError(String(e));
  }
}

// Copy fetches the token if we don't already hold it, then writes to the clipboard.
async function mcpHttpCopy() {
  if (!hasTauri()) { setMcpHttpError("Tauri API unavailable"); return; }
  setMcpHttpError("");
  try {
    if (!mcpHttpToken) mcpHttpToken = await invoke("mcp_http_reveal_token");
    if (!mcpHttpToken) { setMcpHttpError("No token to copy — regenerate first."); return; }
    await navigator.clipboard.writeText(mcpHttpToken);
    showToast("Token copied to clipboard.");
  } catch (e) {
    setMcpHttpError(String(e));
  }
}

// Regenerate mints a new 32-byte token (0600 file) and returns the hex once for display.
async function mcpHttpRegen() {
  if (!hasTauri()) { setMcpHttpError("Tauri API unavailable"); return; }
  setMcpHttpError("");
  try {
    mcpHttpToken = await invoke("mcp_http_regenerate_token");
    const field = document.getElementById("mcp-http-token");
    const btn = document.getElementById("mcp-http-reveal");
    if (field) field.textContent = mcpHttpToken || "(no token yet)";
    if (btn) btn.textContent = "Hide";
    mcpHttpTokenShown = true;
    showToast("New token generated — copy it now.");
    await refreshMcpHttpStatus();
  } catch (e) {
    setMcpHttpError(String(e));
  }
}

// Topbar trigger — created at runtime and appended to .topbar-actions. The sprite has
// no gear icon (and `<use>` on a missing symbol renders blank), so this is a text label,
// styled inline to match the other topbar buttons (#browser-btn et al. are ID-selectored,
// so a bare button wouldn't inherit their look).
// Settings now lives in the topbar "⋯ More" overflow menu (06-18 topbar dedupe).
// openSettings() is unchanged + wired from the More menu item below.

// ---- hotkeys ----
// #4 workspace keyboard nav (Alt+↑/↓): ordered LIVE (non-dormant) workspace ids + a typing
// guard so a focused xterm/input keeps its keystrokes. Additive; never touches PTY lifecycle.
function _liveWsIds() { return Object.keys(workspaces).filter((w) => !workspaces[w].dormant); }
function _isTypingTarget(e) {
  const t = e.target;
  if (!t) return false;
  if (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable) return true;
  return !!(t.closest && t.closest(".xterm"));
}
function cycleActiveWs(dir) {
  const ids = _liveWsIds();
  if (ids.length < 2) { if (ids.length === 1) setActiveWs(ids[0]); return; }
  let i = ids.indexOf(activeWs);
  if (i === -1) i = dir > 0 ? -1 : 0;
  setActiveWs(ids[(i + dir + ids.length) % ids.length]);
}
document.addEventListener("keydown", (e) => {
  if (e.metaKey && e.shiftKey && (e.key === "J" || e.key === "j")) {
    e.preventDefault();
    focusTop();
  } else if (e.altKey && !e.metaKey && !e.ctrlKey && e.key === "ArrowDown" && !_isTypingTarget(e)) {
    e.preventDefault();
    cycleActiveWs(1);
  } else if (e.altKey && !e.metaKey && !e.ctrlKey && e.key === "ArrowUp" && !_isTypingTarget(e)) {
    e.preventDefault();
    cycleActiveWs(-1);
  } else if (e.metaKey && (e.key === "n" || e.key === "N")) {
    e.preventDefault();
    launchWizard();
  } else if (e.metaKey && e.key === "\\") {
    e.preventDefault();
    toggleRail(); // ⌘\ — hide/show the sidebar
  } else if (e.key === "F2" && activeWs && !_isTypingTarget(e)) {
    e.preventDefault();
    beginWorkspaceRenameById(activeWs); // F2 — rename the active workspace
  } else if (e.metaKey && e.shiftKey && (e.key === "W" || e.key === "w")) {
    e.preventDefault();
    if (activeWs) closeWorkspaceGroup(activeWs); // ⌘⇧W — close the active workspace
  } else if (e.metaKey && e.altKey && e.code === "KeyG") {
    e.preventDefault();
    autoTile(); // ⌘⌥G — auto-tile / re-balance the layout
  } else if (e.metaKey && !e.altKey && (e.key === "g" || e.key === "G")) {
    e.preventDefault();
    toggleGrid();
  } else if (e.metaKey && e.shiftKey && (e.key === "D" || e.key === "d")) {
    e.preventDefault();
    splitPane("h"); // ⌘⇧D — split the focused pane DOWN (horizontal divider)
  } else if (e.metaKey && !e.shiftKey && (e.key === "d" || e.key === "D")) {
    e.preventDefault();
    splitPane("v"); // ⌘D — split the focused pane RIGHT (vertical divider)
  } else if (e.metaKey && (e.key === "b" || e.key === "B")) {
    e.preventDefault();
    toggleBrowser();
  } else if (e.metaKey && e.shiftKey && (e.key === "I" || e.key === "i")) {
    e.preventDefault();
    toggleBroadcast();
  } else if (e.metaKey && e.shiftKey && (e.key === "O" || e.key === "o")) {
    e.preventDefault();
    if (bridgeChatDock) toggleBridgeDock(); else openBridge();
  } else if (e.metaKey && e.shiftKey && (e.key === "R" || e.key === "r")) {
    e.preventDefault();
    openSpeak();
  } else if (e.key === "Escape" && !document.getElementById("diff-overlay")?.classList.contains("hidden") && document.getElementById("diff-overlay")) {
    closeDiff();
  } else if (e.key === "Escape" && boardOpen) {
    closeBoard();
  } else if (e.key === "Escape" && graphOpen) {
    // Layered dismissal, innermost surface first: an armed connect flow, then the
    // Second Brain editor panel, then the overlay itself — one Escape per layer.
    if (_brainConnectMode) brainSetConnectMode(false);
    else if (brainPanelOpen) closeBrainPanel();
    else closeGraph();
  } else if (e.key === "Escape" && settingsEl && !settingsEl.classList.contains("hidden")) {
    closeSettings();
  } else if (e.key === "Escape" && !speakEl.classList.contains("hidden")) {
    closeSpeak();
  } else if (e.key === "Escape" && !bridgeEl.classList.contains("hidden")) {
    cancelBridge(); // SAME cancel path as #br-cancel (token bump + phase reset) — never a bare close
  } else if (e.key === "Escape" && delegationsEl && !delegationsEl.classList.contains("hidden")) {
    closeDelegations();
  } else if (e.key === "Escape" && delegateEl && !delegateEl.classList.contains("hidden")) {
    closeDelegate();
  } else if (e.key === "Escape" && flywheelEl && !flywheelEl.classList.contains("hidden")) {
    closeFlywheel();
  } else if (e.key === "Escape" && historyEl && !historyEl.classList.contains("hidden")) {
    closeHistory();
  } else if (e.key === "Escape" && loopsEl && !loopsEl.classList.contains("hidden")) {
    closeLoops();
  } else if (e.key === "Escape" && !modal.classList.contains("hidden")) {
    closeModal();
  }
});

// OS-global ⌘⇧J: Rust raises the window + emits "jump-to-top".
const tauriEvent = window.__TAURI__ && window.__TAURI__.event;
if (tauriEvent && tauriEvent.listen) {
  tauriEvent.listen("jump-to-top", () => focusTop());
}

// ---- OS file drag-drop → paste the path(s) into the pane under the cursor ----
// Tauri's webview intercepts native file drops (dragDropEnabled defaults true)
// and only emits tauri://drag-* events — nothing reaches xterm, so dragging a
// file (e.g. from BridgeShot/Finder) onto a terminal silently did nothing.
// Listen for the events, hit-test the pane under the drop, and send the
// shell-escaped path(s) to that pane's PTY (trailing space, no newline — the
// operator decides when to run).
if (tauriEvent && tauriEvent.listen) {
  let dropHover = null; // pane id currently highlighted as the drop target
  const paneAt = (pos) => {
    if (!pos) return null;
    const dpr = window.devicePixelRatio || 1;
    const el = document.elementFromPoint(pos.x / dpr, pos.y / dpr);
    const paneEl = el && el.closest && el.closest(".term-pane");
    if (!paneEl) return null;
    for (const pid of Object.keys(sessions)) if (sessions[pid].el === paneEl) return pid;
    return null;
  };
  const setDropHover = (pid) => {
    if (dropHover === pid) return;
    if (dropHover && sessions[dropHover]) sessions[dropHover].el.classList.remove("drop-target");
    dropHover = pid;
    if (pid && sessions[pid]) sessions[pid].el.classList.add("drop-target");
  };
  const shellQuote = (p) => "'" + String(p).replace(/'/g, "'\\''") + "'";
  tauriEvent.listen("tauri://drag-over", (e) => setDropHover(paneAt(e.payload && e.payload.position)));
  tauriEvent.listen("tauri://drag-leave", () => setDropHover(null));
  tauriEvent.listen("tauri://drag-drop", (e) => {
    const payload = e.payload || {};
    const pid = paneAt(payload.position) || activeId;
    setDropHover(null);
    const paths = payload.paths || [];
    if (!pid || !sessions[pid] || !paths.length) return;
    setActivePane(pid);
    invoke("send_input", { id: pid, data: paths.map(shellQuote).join(" ") + " " })
      .catch((err) => showToast("drop failed: " + String(err)));
    showToast(`${paths.length} path${paths.length > 1 ? "s" : ""} → ${pid}`);
  });
}

// 06-02 (MCP Phase B): `team_focus_workspace` over the socket → Rust raises the
// window + emits "focus-workspace" with the workspace id → select THAT pane (not
// the queue top, which is `jump-to-top`'s job). Distinct event so the two focus
// paths don't fork. Payload is the id string. setActive (no-op on an unknown id)
// makes that workspace's owner active and focuses the pane.
if (tauriEvent && tauriEvent.listen) {
  tauriEvent.listen("focus-workspace", (e) => {
    const id = e && e.payload;
    if (id) setActive(id);
  });
}

// One-shot early-death auto-respawn (backend arm_early_death_respawn): a harness that
// crashed within seconds of spawn (sporadic opencode/Bun startup segfault,
// anomalyco/opencode#31607) was respawned ONCE at the same pane id — a brand-new PTY,
// so this pane's read cursor must restart from 0 and its corpse UI state must clear
// (dead dot / typing short-circuit). A failed respawn is surfaced honestly instead.
if (tauriEvent && tauriEvent.listen) {
  tauriEvent.listen("pane-early-respawn", (e) => {
    const p = (e && e.payload) || {};
    const id = p.id;
    if (!id) return;
    if (p.ok) {
      const s = sessions[id];
      if (s) {
        s.consumed = 0; // fresh PTY buffer — restart the delta cursor
        try { s.term.reset(); } catch (_) {}
      }
      deadPanes.delete(id);
      _deadHintShown.delete(id);
      showToast(`${p.harness || "harness"} crashed at startup — respawned ${id} automatically (one retry)`);
    } else {
      showToast(`${id} crashed at startup and the auto-respawn failed: ${p.error || "unknown error"}`);
    }
  });
}

// #262 ext: EXTERNAL visible-grid spawn. The socket handler already gated WHO (pid-pin to
// the authorized GlikaAgents binary) + WHAT (trusted-repo allowlist, harness allowlist [no
// bash], count cap). This listener adds the load-bearing HUMAN CONFIRM the pid-pin cannot
// provide (a prompt-injected brain IS the binary), then runs the SAME createWorkspace /
// addAgentsToWorkspace the in-app launcher uses — so panes render in the grid.
// Plain-DOM confirm overlay (window.confirm is a no-op in wry/WKWebView — see the
// other call sites that avoid it). Returns a Promise<bool>. Allow defaults to focus.
function externalSpawnConfirm(title, lines) {
  return new Promise((resolve) => {
    const ov = document.createElement("div");
    ov.style.cssText = "position:fixed;inset:0;z-index:99999;display:flex;align-items:center;justify-content:center;background:rgba(0,0,0,.55);";
    const card = document.createElement("div");
    card.style.cssText = "max-width:440px;width:90%;background:var(--bg,#1c1c1c);color:var(--fg,#eaeaea);border:1px solid var(--line,#444);border-radius:10px;padding:18px 20px;font:13px/1.5 ui-monospace,monospace;box-shadow:0 8px 40px rgba(0,0,0,.5);";
    const h = document.createElement("div");
    h.textContent = title;
    h.style.cssText = "font-weight:600;margin-bottom:10px;";
    const body = document.createElement("div");
    body.textContent = lines;
    body.style.cssText = "white-space:pre-wrap;opacity:.9;margin-bottom:16px;";
    const row = document.createElement("div");
    row.style.cssText = "display:flex;gap:10px;justify-content:flex-end;";
    const cancel = document.createElement("button");
    cancel.textContent = "Cancel";
    cancel.style.cssText = "padding:7px 14px;border-radius:7px;border:1px solid var(--line,#555);background:transparent;color:inherit;cursor:pointer;";
    const allow = document.createElement("button");
    allow.textContent = "Allow";
    allow.style.cssText = "padding:7px 14px;border-radius:7px;border:1px solid #c33;background:#a22;color:#fff;cursor:pointer;font-weight:600;";
    const done = (v) => { try { document.body.removeChild(ov); } catch (_) {} resolve(v); };
    cancel.onclick = () => done(false);
    allow.onclick = () => done(true);
    row.appendChild(cancel); row.appendChild(allow);
    card.appendChild(h); card.appendChild(body); card.appendChild(row);
    ov.appendChild(card); document.body.appendChild(ov);
    setTimeout(() => allow.focus(), 0);
  });
}

// Resolve an external add_pane target: match a live workspace by id (key) OR by the
// `tag` stamped at create time. Returns the wsId or null (caller declines on null when a
// target was explicitly named — never silently lands on the wrong/active workspace).
function resolveExternalTarget(target) {
  if (!target) return null;
  const t = String(target).trim();
  // Exact wsId match wins (ids are unique).
  if (workspaces[t] && !workspaces[t].dormant) return t;
  // Else match by tag — but DECLINE if the tag is ambiguous (≥2 live workspaces share
  // it), rather than silently picking the first enumerated one.
  const byTag = Object.keys(workspaces).filter((wsId) => {
    const ws = workspaces[wsId];
    return ws && !ws.dormant && ws.tag && String(ws.tag) === t;
  });
  return byTag.length === 1 ? byTag[0] : null;
}

// External-caller pane clamp (per-create expansion + the add_pane ceiling). The cap now
// arrives CONFIG-RESOLVED on the event payload (`p.cap`, additive — Rust resolves the
// operator's external_spawn_max_panes; fallback 8 for older backends, hard FE ceiling 16,
// enforced in external-spawn-core.js). Still enforced FE-side because the brain is a
// prompt-injection-class caller; cap unbounded growth the in-app human path doesn't fear.

if (tauriEvent && tauriEvent.listen) {
  tauriEvent.listen("external-spawn", async (e) => {
    const p = (e && e.payload) || {};
    const cap = externalSpawnCap(p);
    try {
      if (p.op === "create_workspace") {
        const repo = String(p.repo || "").trim();
        if (!repo) return;
        // Expand the per-pane spec list → flat parallel arrays createWorkspace consumes.
        // Back-compat: a scalar payload (no `panes`) becomes a single homogeneous group.
        const groups = (Array.isArray(p.panes) && p.panes.length)
          ? p.panes
          : [{ harness: p.harness || "claude", role: p.role, model: p.model, count: p.count || 1 }];
        const { harnesses, roles, models, requested, truncated } = expandExternalPanes(groups, cap);
        const count = harnesses.length;
        if (count === 0) return;
        const planLines = groups.map((g) => {
          const n = Math.max(1, Number(g.count) || 1);
          return "   – " + n + "× " + (g.harness || "claude") + (g.role ? " (" + g.role + ")" : "");
        }).join("\n");
        const ok = p.no_confirm ? true : await externalSpawnConfirm(
          "GlikaAgents wants to OPEN a new workspace",
          "• repo: " + repo + "\n• panes:\n" + planLines +
          "\n\nThis spawns " + count + " live agent session(s)." +
          (truncated ? "\n⚠ Requested " + requested + " panes; clamped to this install's cap (" + cap + ")." : "")
        );
        if (!ok) { try { showToast("External spawn declined"); } catch (_) {} return; }
        const name = (p.tag ? p.tag + " · " : "") + (repo.split("/").filter(Boolean).pop() || "workspace");
        const color = WS_PALETTE[Object.keys(workspaces).length % WS_PALETTE.length];
        await createWorkspace({ name, color, repo, count, harnesses, roles, models, tag: p.tag || undefined });
        try { showToast("Spawned " + count + " pane(s) on " + name); } catch (_) {}
      } else if (p.op === "add_pane") {
        // Target by id/tag when named; fall back to active only if nothing was named.
        let wsId;
        if (p.target_workspace) {
          wsId = resolveExternalTarget(p.target_workspace);
          if (!wsId) { try { showToast("External add-pane: workspace '" + p.target_workspace + "' not found"); } catch (_) {} return; }
        } else {
          wsId = activeWs;
          if (!wsId) { try { showToast("External add-pane: no active workspace (name a target)"); } catch (_) {} return; }
        }
        const ws = workspaces[wsId];
        if (ws && Array.isArray(ws.paneIds) && ws.paneIds.length >= cap) {
          console.warn("[external-spawn] add_pane refused: " + wsId + " at pane ceiling (" + cap + ")");
          try { showToast("External add-pane: workspace already at pane ceiling (" + cap + ")"); } catch (_) {} return;
        }
        const harness = p.harness || (ws && ws.harness) || "claude";
        const wsName = (ws && ws.name) || wsId;
        const ok = p.no_confirm ? true : await externalSpawnConfirm(
          "GlikaAgents wants to ADD a pane",
          "• workspace: " + wsName + " (" + wsId + ")\n• harness: " + harness +
          (p.role ? "\n• role: " + p.role : "")
        );
        if (!ok) { try { showToast("External add-pane declined"); } catch (_) {} return; }
        await addAgentsToWorkspace(wsId, { harness, count: 1, role: p.role || undefined });
        try { showToast("Added a pane to " + wsName); } catch (_) {}
      }
    } catch (err) {
      try { showToast("External spawn failed: " + err); } catch (_) {}
    }
  });
}

// ---- dev self-updater: "Update Available" card + persistent sidebar pill ----
// The backend (1s timer) emits `update-available` once per fresh dev build when
// a dev-source pointer exists. We surface a card + a sidebar pill; the user
// chooses when to apply. Applying relaunches (a running app can't hot-swap its
// own binary), so it's never automatic — "just don't close it" until I click.
function showUpdate(version) {
  const sub = document.getElementById("up-sub");
  if (sub) sub.textContent = version ? `Agent Teams v${version}` : "Agent Teams";
  document.getElementById("update-card")?.classList.remove("hidden");
  document.getElementById("update-pill")?.classList.remove("hidden");
}
function hideUpdateCard() { document.getElementById("update-card")?.classList.add("hidden"); }
async function applyUpdate() {
  hideUpdateCard();
  if (!hasTauri()) { showToast("Tauri API unavailable"); return; }
  showToast("Updating — the app will relaunch…");
  try { await invoke("apply_update"); }
  catch (e) { showToast("Update failed: " + String(e)); }
}
document.getElementById("up-now")?.addEventListener("click", applyUpdate);
document.getElementById("update-pill")?.addEventListener("click", applyUpdate);
document.getElementById("up-later")?.addEventListener("click", hideUpdateCard);
document.getElementById("up-x")?.addEventListener("click", hideUpdateCard);
if (tauriEvent && tauriEvent.listen) {
  tauriEvent.listen("update-available", (e) => showUpdate(e && e.payload && e.payload.version));
  // PRD-synthesis progress: backend emits a {phase, pct} per milestone (gather → re-fold →
  // tests/verdict → synthesize (Opus) → adjudicate → write). Fill the phase label + bar so the
  // long opaque fan-in is visibly alive, not skipped. The elapsed timer (synthesizeResults) keeps
  // ticking between phases — the shimmer + timer cover the minutes-long Opus step.
  tauriEvent.listen("bridge-synth-progress", (e) => {
    const p = (e && e.payload) || {};
    const phaseEl = document.getElementById("br-synth-phase");
    const barEl = document.getElementById("br-synth-bar");
    if (phaseEl && p.phase) phaseEl.textContent = p.phase;
    if (barEl && typeof p.pct === "number") barEl.style.width = Math.max(0, Math.min(100, p.pct)) + "%";
  });
}

// ---- Delegations panel: WATCH headless delegate workers live ----
// Delegate workers run via `claude -p --output-format stream-json` in isolated worktrees.
// They are NOT in list_workspaces, so they never appear in the rail (this panel is the
// only place to observe them). The backend emits:
//   "delegate-worker-log"    { run_id, worker, lines:[..] } — batched stream-json lines
//                            (legacy single-{line} shape still accepted; CONTRACT seam 2)
//   "delegate-worker-status" { run_id?, worker, status, repo? }  — running | retired
// Two invariants drive the design:
//   (1) Listeners are registered at module load and accumulate into `dgWorkers` even while
//       the panel is closed — log lines can arrive BEFORE the "running" status (the stdout
//       reader thread is spawned before the status emit), and before the user ever opens it.
//   (2) ALL model-derived text (assistant text, tool input, raw fallback) is rendered via
//       textContent — NEVER innerHTML — because a worker's stdout is most-untrusted model
//       output (XSS).
const delegationsEl = document.getElementById("delegations");
const historyEl = document.getElementById("history"); // dedicated run-history panel (workspace-scoped)
const dgWorkers = new Map(); // worker -> { status, runId, repo, lines:[{kind,text}], finalDoc }
const dgRuns = new Map(); // run_id -> { verdict, path, digest } — run-level completion card (P0.2)
// DG_MAX_LINES + the pure feed/time/verdict helpers now live in ./delegation-log-core.js (imported).
let dgSeenAnyEvent = false;
// run_id we've already auto-surfaced the panel for — so a delegation AUTO-OPENS the panel the
// first time a worker goes live (the user shouldn't have to hunt for a toolbar button to see
// their delegation running), but a manual close STICKS for that run (we don't re-pop). A NEW
// run (new run_id) auto-opens again.
let dgAutoOpenedRun = null;

// ── Run-history view state (lives in JS, NOT the DOM, so it survives a re-render). History renders
// in the dedicated #history panel (#hist-list) on open + on a run completion — NOT on every
// worker-stream tick (those touch #dg-workers via renderWorkers). Each run is a COMPACT one-line row
// that expands its detail lazily on click. The view is WORKSPACE-SCOPED via histWsFilter. ──
// ACCORDION: exactly ONE run expanded at a time = dgExpandedId (the newest, by default). Set on
// open / scope-change / completion to the newest in scope; a row click toggles it. null = none open.
let dgExpandedId = null;
let dgShowAllHistory = false;          // "show older" toggle (render-cap escape hatch)
let dgBenchOpen = false;               // P4: "Model benchmarks" rollup expanded? (default collapsed — keep the panel clean)
const DG_HISTORY_RENDER_CAP = 50;      // newest N rendered; the rest hide behind "show older"
let histWsFilter = "__all__";          // History workspace scope: "__all__" or a workspace id (set on open)
// (pane→workspace uses the existing `paneOwner(paneId)`; active workspace = the `activeWs` global.)

// The newest run_id in the CURRENT workspace scope (for default-expand). Newest = max ts.
function dgNewestInScope() {
  const f = [...dgRuns.entries()]
    .filter(([, r]) => histWsFilter === "__all__" || (r.workspace_id || "") === histWsFilter)
    .sort((a, b) => dgRunTs(b[0], b[1]) - dgRunTs(a[0], a[1]));
  return f.length ? f[0][0] : null;
}

// ── Session cost meter (rail readout) ─────────────────────────────────────────────────────────
// Sum of every run's usage.cost_usd across the History store (dgRuns) — the SAME field bench-core
// benchmarkRows sums per (harness·model). Missing / null / non-numeric → 0, so a partial-usage or
// old/default-build record contributes nothing rather than NaN-poisoning the total. Cheap: a small
// reduce over the in-memory Map, run on the poll tick + on history change.
function sessionCostUsd() {
  let sum = 0;
  for (const run of dgRuns.values()) {
    const c = run && run.usage && Number(run.usage.cost_usd);
    if (c > 0) sum += c; // Number(undefined|null|"")→NaN; NaN>0 is false → skipped
  }
  return sum;
}
// Soft budget: above it, the metric flips to the ONE signal-red accent (.metric--accent). Override
// via localStorage at_cost_budget; 0 / unset → no budget (never flips). Kept dead-simple per spec.
const SESSION_COST_BUDGET = (() => {
  try { const v = parseFloat(localStorage.getItem("at_cost_budget")); return v > 0 ? v : 0; }
  catch (_) { return 0; }
})();
// The readout is created lazily in JS (not index.html) and inserted just before #hint at the rail
// bottom. Cached so we build the node once; subsequent calls only touch the metric text/class.
let _sessionCostEl = null;
function ensureSessionCostEl() {
  if (_sessionCostEl) return _sessionCostEl;
  const rail = document.getElementById("rail");
  const hint = document.getElementById("hint");
  if (!rail) return null;
  const el = document.createElement("div");
  el.id = "session-cost";
  const label = document.createElement("span");
  label.className = "sc-label"; // mono-caps "SESSION"
  label.textContent = "SESSION";
  const metric = document.createElement("span");
  metric.className = "metric"; // Doto readout (CSS in polish_7_costmeter.css)
  metric.textContent = "$0.00";
  el.appendChild(label);
  el.appendChild(metric);
  if (hint && hint.parentNode === rail) rail.insertBefore(el, hint);
  else rail.appendChild(el);
  _sessionCostEl = el;
  return el;
}
function renderSessionCost() {
  const el = ensureSessionCostEl();
  if (!el) return;
  const metric = el.querySelector(".metric");
  if (!metric) return;
  const usd = sessionCostUsd();
  metric.textContent = "$" + usd.toFixed(2);
  metric.classList.toggle("metric--accent", SESSION_COST_BUDGET > 0 && usd > SESSION_COST_BUDGET);
}

// dgRelTime / dgDurationMs / dgFmtDur → ./delegation-log-core.js (pure, unit-tested).

// Lazily create (or fetch) a worker's accumulator. Called from BOTH listeners so an early
// log line creates the entry before the status arrives (and vice-versa).
function dgEntry(worker) {
  let w = dgWorkers.get(worker);
  if (!w) { w = { status: "running", runId: null, repo: null, lines: [], finalDoc: null }; dgWorkers.set(worker, w); }
  return w;
}

// dgPush / dgIngestLine (stream-json → feed entries) → ./delegation-log-core.js (pure, unit-tested).

// Render the whole panel from `dgWorkers` (cheap — only when open or on open). Uses
// textContent for every model-derived string.
function renderDelegations() {
  if (!delegationsEl) return;
  // Delegations is LIVE-WORKERS-ONLY now — run history lives in its own #history panel. (This is
  // also the render efficiency win: worker-stream ticks only ever rebuild the small workers list.)
  renderWorkers();
}

// dgRunTs / dgVerdictUx / dgReviewData → ./delegation-log-core.js (pure, unit-tested).

// Render the READ-ONLY P5 review/CRAP block into `wrap` (an EXPANDED run detail). Display only —
// the verdict pill above remains driven by the pure bridge/CRAP verdict; here we surface the
// reviewer's decision, severity-colored findings ({severity,domain,why,cite}), the calibrated flag
// (warn when UNTRUSTED), and the CRAP delta summary (gate_would_block + regressed/over-threshold
// methods). Every model-derived string via textContent (reviewer output is untrusted).
function dgRenderReview(wrap, data) {
  const sec = document.createElement("div");
  sec.className = "dg-review";

  // ── decision + advisory note ──
  const head = document.createElement("div");
  head.className = "dg-review-head";
  const known = data.dec === "approve" || data.dec === "request_changes";
  const pill = document.createElement("span");
  pill.className = "dg-review-decision " + (known ? data.dec : "unknown");
  pill.textContent = data.dec === "approve" ? "✓ Reviewer: APPROVE"
    : data.dec === "request_changes" ? "✕ Reviewer: REQUEST CHANGES"
    : "Reviewer";
  head.appendChild(pill);
  const adv = document.createElement("span");
  adv.className = "dg-review-advisory";
  adv.textContent = "advisory — logged, not yet enforced";
  adv.title = "P5: the smart PR-review gate records its verdict + findings but does NOT block or merge. The verdict badge above is the run's enforced outcome.";
  head.appendChild(adv);
  sec.appendChild(head);

  // ── calibration (the ponytail self-test): UNTRUSTED → its APPROVE was coerced to REQUEST_CHANGES ──
  if (data.calibrated != null) {
    const cal = document.createElement("div");
    cal.className = "dg-review-cal " + (data.calibrated ? "trusted" : "untrusted");
    cal.textContent = data.calibrated
      ? "✓ Reviewer calibrated — passed the known-bad / known-good self-test this run."
      : "⚠ Reviewer UNTRUSTED — failed the calibration self-test this run; its verdict was coerced to REQUEST CHANGES (fail-closed). Treat the decision as unreliable.";
    sec.appendChild(cal);
  }

  // ── findings: severity-colored {severity, domain, why, cite} ──
  const findings = Array.isArray(data.findings) ? data.findings : [];
  if (findings.length) {
    const list = document.createElement("div");
    list.className = "dg-review-findings";
    for (const f of findings) {
      if (!f || typeof f !== "object") continue;
      const sev = (typeof f.severity === "string" ? f.severity : "").toLowerCase();
      const sevCls = (sev === "block" || sev === "major" || sev === "minor" || sev === "info") ? sev : "info";
      const fEl = document.createElement("div");
      fEl.className = "dg-finding " + sevCls;
      const sevEl = document.createElement("span");
      sevEl.className = "dg-finding-sev " + sevCls;
      sevEl.textContent = sev || "info";
      const domEl = document.createElement("span");
      domEl.className = "dg-finding-domain";
      domEl.textContent = typeof f.domain === "string" && f.domain ? f.domain : "—";
      const whyEl = document.createElement("span");
      whyEl.className = "dg-finding-why";
      whyEl.textContent = typeof f.why === "string" ? f.why : "";
      fEl.append(sevEl, domEl, whyEl);
      if (typeof f.cite === "string" && f.cite) {
        const citeEl = document.createElement("span");
        citeEl.className = "dg-finding-cite";
        citeEl.textContent = f.cite;
        fEl.appendChild(citeEl);
      }
      list.appendChild(fEl);
    }
    if (list.childElementCount) sec.appendChild(list);
  }

  // ── CRAP delta (§3.10): coverage-regression veto. gate_would_block + regressed / new-over-threshold. ──
  if (data.crap && typeof data.crap === "object") {
    const crapEl = document.createElement("div");
    crapEl.className = "dg-crap";
    const wouldBlock = data.crap.gate_would_block === true;
    const ch = document.createElement("div");
    ch.className = "dg-crap-head " + (wouldBlock ? "block" : "clear");
    ch.textContent = wouldBlock
      ? "⚠ CRAP gate would block — coverage regressed on touched methods."
      : "✓ CRAP delta clean — no coverage regression on touched methods.";
    crapEl.appendChild(ch);
    const named = (arr, label) => {
      if (!Array.isArray(arr) || !arr.length) return;
      const cap = document.createElement("div");
      cap.className = "dg-crap-head";
      cap.textContent = label;
      crapEl.appendChild(cap);
      const ul = document.createElement("ul");
      ul.className = "dg-crap-list";
      for (const m of arr) {
        const li = document.createElement("li");
        // entries may be a bare method name or an object {method, crap, ...} — render readably.
        li.textContent = (m && typeof m === "object")
          ? [m.method || m.name || m.ref || "", m.crap != null ? `CRAP ${m.crap}` : ""].filter(Boolean).join(" — ")
          : String(m);
        ul.appendChild(li);
      }
      crapEl.appendChild(ul);
    };
    named(data.crap.regressions, "Regressed methods:");
    named(data.crap.new_over_threshold, "New methods over threshold:");
    sec.appendChild(crapEl);
  }

  wrap.appendChild(sec);
}

// Build the FULL detail block for an EXPANDED run row — created LAZILY (only for expanded rows) so a
// long history renders as cheap one-line rows by default. All model-derived strings via textContent
// (goal/path are untrusted).
function dgBuildDetail(runId, run) {
  const ux = dgVerdictUx((run.verdict || "").toLowerCase());
  const wrap = document.createElement("div");
  wrap.className = "dg-result-detail";

  const meanEl = document.createElement("div");
  meanEl.className = "dg-result-mean";
  meanEl.textContent = ux.mean;
  wrap.appendChild(meanEl);

  if (run.goal) {
    const goalEl = document.createElement("div");
    goalEl.className = "dg-result-goal";
    // label it: this block is the run's GOAL/prompt, NOT its verdict — a PRD goal can
    // contain the recon's own verdict text, which read as "this run failed" without it.
    goalEl.textContent = `💬 goal: ${run.goal}`;
    goalEl.title = "The prompt this run was given (the verdict badge above is the run's own result):\n\n" + run.goal;
    wrap.appendChild(goalEl);
  }

  // harness + model + TIMING — the benchmark line (which harness/model, how fast, what cost).
  if (run.harness || dgDurationMs(runId, run)) {
    const hm = document.createElement("div");
    hm.className = "dg-result-harness";
    const parts = [];
    if (run.harness) parts.push(`${run.harness} · ${run.model ? run.model : "default model"}`);
    const dur = dgFmtDur(dgDurationMs(runId, run));
    if (dur) parts.push(`⏱ ${dur}`);
    hm.textContent = `🔧 ${parts.join(" · ")}`;
    wrap.appendChild(hm);
  }

  // honest note — a shipped run opened a PR (NEVER merged to main); a report/local run merged nothing.
  const noteEl = document.createElement("div");
  noteEl.className = "dg-result-note";
  noteEl.textContent = run.pr_url
    ? "🔀 A PR is open from a tested merge — review + merge it. Pressing Flywheel again opens a NEW PR."
    : "📄 Nothing was merged to your main branch — review the report and act on it.";
  wrap.appendChild(noteEl);

  if (run.usage && (run.usage.cost_usd || run.usage.input || run.usage.output)) {
    const u = run.usage;
    const k = (n) => { n = Number(n) || 0; return n >= 1000 ? (n / 1000).toFixed(n >= 10000 ? 0 : 1) + "k" : String(n); };
    const cost = Number(u.cost_usd) || 0;
    const costEl = document.createElement("div");
    costEl.className = "dg-result-cost";
    costEl.textContent = `💰 ${benchCost(cost, 1)} · ${k(u.input)} in / ${k(u.output)} out`
      + (u.cache_read ? ` · ${k(u.cache_read)} cached` : "");
    costEl.title = "Run cost — orchestrator + workers + synthesizer (input/output = billed tokens; cached = cache-read).";
    wrap.appendChild(costEl);
  }

  // P5 (unified-engine §3.6/§3.10): smart-PR-review verdict + CRAP delta, READ-ONLY. Rendered only
  // when the run carries them (advisory/old runs → null → block skipped).
  const reviewData = dgReviewData(run);
  if (reviewData) dgRenderReview(wrap, reviewData);

  if (run.path || run.pr_url) {
    const foot = document.createElement("div");
    foot.className = "dg-result-foot";
    if (run.path) {
      const pathEl = document.createElement("span");
      pathEl.className = "dg-result-path";
      pathEl.textContent = run.path; // REAL path from the event (final.md / final.HELD.md)
      const openBtn = document.createElement("button");
      openBtn.className = "dg-open";
      openBtn.type = "button";
      openBtn.textContent = "Open report ↗";
      openBtn.addEventListener("click", () => dgOpenResult(run.path)); // P0.3
      foot.append(pathEl, openBtn);
    }
    if (run.pr_url) {
      const prBtn = document.createElement("button");
      prBtn.className = "dg-open";
      prBtn.type = "button";
      prBtn.textContent = "Open PR ↗";
      prBtn.addEventListener("click", () => dgOpenPR(run.pr_url));
      foot.append(prBtn);
    }
    wrap.appendChild(foot);
  }
  return wrap;
}

// Render the RUN-HISTORY into #hist-list (dedicated panel). Called on History open + on a run
// COMPLETION only (never per worker-stream tick). Compact one-line rows; detail built lazily; collapsible
// section header; render-capped at DG_HISTORY_RENDER_CAP with a "show older" escape hatch.
// P4 BENCHMARK rollup: one row per (harness · model) — avg ⏱ / avg $ / gated pass-rate,
// FASTEST-first — so "which model is faster/cheaper" is answerable at a glance. Pure math is
// `benchmarkRows` (bench-core.js, unit-tested); this only paints it. Workspace-scoped to match
// the history list (same `histWsFilter`). Collapsed by default; the table builds ONLY when open
// (cheap when closed even with a huge history). Hidden entirely when the scope has no runs.
function renderBench() {
  const host = document.getElementById("hist-bench");
  if (!host) return;
  host.replaceChildren();
  const rows = benchmarkRows([...dgRuns.entries()], histWsFilter);
  if (!rows.length) { host.classList.add("hidden"); return; }
  host.classList.remove("hidden");

  const head = document.createElement("button");
  head.type = "button";
  head.className = "hist-bench-head";
  head.setAttribute("aria-expanded", String(dgBenchOpen));
  const chev = document.createElement("span");
  chev.className = "dg-row-chev";
  chev.textContent = dgBenchOpen ? "▾" : "▸";
  const title = document.createElement("span");
  title.className = "hist-bench-title";
  title.textContent = `⚡ Model benchmarks · ${rows.length}`;
  head.append(chev, title);
  head.addEventListener("click", () => { dgBenchOpen = !dgBenchOpen; renderBench(); });
  host.appendChild(head);
  if (!dgBenchOpen) return;

  const cap = document.createElement("div");
  cap.className = "hist-bench-cap";
  cap.textContent = "Fastest first · averages across this workspace's runs (pass-rate over tested runs only).";
  host.appendChild(cap);

  const table = document.createElement("div");
  table.className = "hist-bench-table";
  const hdr = document.createElement("div");
  hdr.className = "hist-bench-row hist-bench-hdr";
  for (const [t, cls] of [["Harness · model", "bm-name"], ["Runs", "bm-n"], ["Avg ⏱", "bm-dur"], ["Avg $", "bm-cost"], ["Pass", "bm-pass"]]) {
    const c = document.createElement("span"); c.className = cls; c.textContent = t; hdr.appendChild(c);
  }
  table.appendChild(hdr);
  rows.forEach((r, i) => {
    const tr = document.createElement("div");
    tr.className = "hist-bench-row" + (i === 0 && r.avgDurMs ? " fastest" : "");
    const name = document.createElement("span");
    name.className = "bm-name";
    name.textContent = `${r.harness} · ${r.model}`;
    name.title = name.textContent + (i === 0 && r.avgDurMs ? " — fastest by avg time" : "");
    const n = document.createElement("span"); n.className = "bm-n"; n.textContent = String(r.runs);
    n.title = `${r.timedRuns} timed · ${r.costRuns} with cost`;
    // Doto instrument-readout: avg duration rides as a `.metric` gauge.
    const dur = document.createElement("span"); dur.className = "bm-dur";
    { const dm = document.createElement("span"); dm.className = "metric"; dm.textContent = benchDur(r.avgDurMs); dur.appendChild(dm); }
    const cost = document.createElement("span"); cost.className = "bm-cost"; cost.textContent = benchCost(r.avgCostUsd, r.costRuns);
    const pass = document.createElement("span"); pass.className = "bm-pass";
    pass.title = r.passRate === null ? "no tested (pass/reject) runs" : `${r.passes}/${r.gatedRuns} tested runs passed`;
    if (r.passRate === null) {
      pass.textContent = "—";
    } else {
      // tiny segmented bar — r.gatedRuns blocks, r.passes filled (Nothing-OS, square blocks)
      const pct = document.createElement("span"); pct.className = "metric bm-pass-pct"; pct.textContent = `${Math.round(r.passRate * 100)}%`;
      const bar = document.createElement("span"); bar.className = "bm-pass-bar"; bar.setAttribute("aria-hidden", "true");
      const total = Math.max(1, r.gatedRuns | 0);
      for (let k = 0; k < total; k++) { const blk = document.createElement("i"); if (k < (r.passes | 0)) blk.className = "on"; bar.appendChild(blk); }
      pass.append(pct, bar);
    }
    tr.append(name, n, dur, cost, pass);
    table.appendChild(tr);
  });
  host.appendChild(table);
}

function renderHistory() {
  const host = document.getElementById("hist-list");
  if (!host) return;
  host.replaceChildren();
  renderBench(); // P4: refresh the per-model rollup alongside the run list (shares histWsFilter)
  const all = [...dgRuns.entries()].sort((a, b) => dgRunTs(b[0], b[1]) - dgRunTs(a[0], a[1]));
  populateHistWsPicker(all);
  // Workspace scope: show only the selected workspace's runs (or all). Human-only is enforced
  // backend-side (parse_run_history) + by the live event carrying initiator:"human".
  const filtered = histWsFilter === "__all__"
    ? all
    : all.filter(([, r]) => (r.workspace_id || "") === histWsFilter);
  const empty = document.getElementById("hist-empty");
  if (empty) empty.classList.toggle("hidden", filtered.length > 0);
  if (!filtered.length) return;
  // NOTE: dgExpandedId is set by the EVENTS (open / picker change / completion) to the newest in
  // scope; render only READS it, so a user collapse-all (dgExpandedId=null) sticks.

  const visible = dgShowAllHistory ? filtered : filtered.slice(0, DG_HISTORY_RENDER_CAP);
  const rows = document.createElement("div"); // ONE rounded panel; rows are thin divided list items
  rows.className = "dg-rows";
  host.appendChild(rows);
  for (const [runId, run] of visible) {
    const v = (run.verdict || "").toLowerCase();
    const known = v === "pass" || v === "hold" || v === "reject" || v === "advisory" || v === "pr-failed";
    const ux = dgVerdictUx(v);
    const expanded = runId === dgExpandedId;

    const rcard = document.createElement("div");
    rcard.className = "dg-result" + (expanded ? " expanded" : "");

    // COMPACT one-line row: chevron + verdict pill + relative time + cost/PR marker. Click toggles.
    const row = document.createElement("button");
    row.type = "button";
    row.className = "dg-result-row";
    row.setAttribute("aria-expanded", String(expanded));
    row.title = runId; // the backend run id, on hover (kept off the dense row)

    const chev = document.createElement("span");
    chev.className = "dg-row-chev";
    chev.textContent = expanded ? "▾" : "▸";

    const verd = document.createElement("span");
    verd.className = "dg-verdict " + (known ? v : "unknown");
    verd.textContent = ux.pill;

    const when = document.createElement("span");
    when.className = "dg-row-when";
    const ts = dgRunTs(runId, run);
    when.textContent = ts ? dgRelTime(ts) : "";

    // harness tag — scannable across runs for comparison ("which harness did this?").
    const harn = document.createElement("span");
    harn.className = "dg-row-harness";
    if (run.harness) { harn.textContent = run.harness; harn.title = run.model ? `${run.harness} · ${run.model}` : `${run.harness} · default model`; }

    const meta = document.createElement("span");
    meta.className = "dg-row-meta";
    // Doto instrument-readout pass: the duration (and cost) ride as `.metric` Doto gauges
    // rather than plain inline text. The duration/cost values are wrapped in their own
    // spans; the surrounding separators + glyphs + review markers stay text nodes so the
    // existing meta layout is byte-for-byte preserved (no per-run test bar — the record
    // carries no per-run test counts, so none is fabricated).
    const txt = (s) => meta.appendChild(document.createTextNode(s));
    const dur = dgFmtDur(dgDurationMs(runId, run)); // per-run TIMING (benchmark)
    if (dur) {
      txt(" · ⏱ ");
      const m = document.createElement("span"); m.className = "metric"; m.textContent = dur; meta.appendChild(m);
    }
    const cost = run.usage && Number(run.usage.cost_usd);
    if (cost) {
      txt(" · ");
      const m = document.createElement("span"); m.className = "metric"; m.textContent = benchCost(cost, 1); meta.appendChild(m);
    }
    if (run.pr_url) txt(" · 🔀 PR");
    // P5: at-a-glance review markers on the collapsed row (full detail on expand). The reviewer is
    // ADVISORY in P5, so these annotate — they don't replace the enforced verdict pill on the left.
    const rd = dgReviewData(run);
    if (rd) {
      if (rd.calibrated === false) txt(" · ⚠ uncal");
      else if (rd.dec === "request_changes") txt(" · ✕ review");
      else if (rd.dec === "approve") txt(" · ✓ review");
      if (rd.crap && rd.crap.gate_would_block === true) txt(" · ⚠ CRAP");
    }

    row.append(chev, verd, when, harn, meta);
    // ACCORDION: clicking a row opens it + collapses the rest; clicking the open one collapses it.
    row.addEventListener("click", () => {
      dgExpandedId = (dgExpandedId === runId) ? null : runId;
      renderHistory();
    });
    rcard.appendChild(row);

    if (expanded) rcard.appendChild(dgBuildDetail(runId, run)); // detail built ONLY when expanded
    rows.appendChild(rcard);
  }

  if (!dgShowAllHistory && filtered.length > DG_HISTORY_RENDER_CAP) {
    const more = document.createElement("button");
    more.type = "button";
    more.className = "dg-show-older";
    more.textContent = `Show older (${filtered.length - DG_HISTORY_RENDER_CAP})`;
    more.addEventListener("click", () => { dgShowAllHistory = true; renderHistory(); });
    host.appendChild(more);
  }
}

// Populate the #hist-ws workspace picker from the workspaces present in the run set (+ "All
// workspaces"). Only rebuilds when the option set changes, so it never clobbers the user's
// selection mid-interaction. Workspace NAMES come from the live `workspaces` map (fallback: id).
function populateHistWsPicker(all) {
  const sel = document.getElementById("hist-ws");
  if (!sel) return;
  const ids = [];
  const seen = new Set();
  for (const [, r] of all) {
    const id = r.workspace_id || "";
    if (!seen.has(id)) { seen.add(id); ids.push(id); }
  }
  const opts = [["__all__", "All workspaces"]];
  for (const id of ids) {
    if (id === "") { opts.push(["", "(no workspace)"]); continue; }
    const ws = workspaces[id];
    opts.push([id, ws && ws.name ? ws.name : id]);
  }
  const sig = opts.map((o) => o[0]).join("|");
  if (sel.dataset.sig !== sig) {
    sel.replaceChildren();
    for (const [val, label] of opts) {
      const o = document.createElement("option");
      o.value = val; o.textContent = label;
      sel.appendChild(o);
    }
    sel.dataset.sig = sig;
  }
  // keep the picker synced to histWsFilter; if the scoped workspace vanished, fall back to All.
  if (![...sel.options].some((o) => o.value === histWsFilter)) histWsFilter = "__all__";
  sel.value = histWsFilter;
}

// Render the LIVE-WORKERS section into #dg-workers. Called on EVERY worker-stream tick — cheap
// because it only touches this container, never the (possibly large) history rows.
function renderWorkers() {
  const host = document.getElementById("dg-workers");
  if (!host) return;
  host.replaceChildren();
  const workers = [...dgWorkers.entries()];
  dgUpdateEmpty();
  if (workers.length) {
    const h = document.createElement("div");
    h.className = "dg-section";
    h.textContent = `Live workers · ${workers.length}`;
    host.appendChild(h);
  }
  const feeds = []; // scroll-pinned AFTER the build loop — see below
  for (const [id, w] of workers) {
    const card = document.createElement("div");
    card.className = "dg-worker";

    const head = document.createElement("div");
    head.className = "dg-worker-head";
    const wid = document.createElement("span");
    wid.className = "dg-worker-id";
    wid.textContent = id; // worker id is backend-generated (delegate-…-wN) but render safely anyway
    const pill = document.createElement("span");
    pill.className = "dg-pill " + (w.status || "running");
    pill.textContent = w.status || "running";
    head.append(wid, pill);
    card.appendChild(head);

    const feed = document.createElement("div");
    feed.className = "dg-feed";
    if (w.lines.length === 0) {
      const ln = document.createElement("div");
      ln.className = "dg-line raw";
      // claude streams (stream-json) live; the other harnesses BUFFER stdout → no live feed until
      // they finish. Say so, so a working run doesn't read as a frozen "waiting…".
      ln.textContent = (w.harness && w.harness !== "claude")
        ? `running — ${w.harness} doesn't stream live; output appears when it finishes (~1 min)`
        : "…waiting for first output";
      feed.appendChild(ln);
    } else {
      for (const entry of w.lines) {
        const ln = document.createElement("div");
        ln.className = "dg-line"
          + (entry.kind === "tool" ? " tool" : entry.kind === "raw" ? " raw" : entry.kind === "error" ? " error" : "");
        ln.textContent = entry.text; // SAFE: never innerHTML of worker output
        feed.appendChild(ln);
      }
    }
    card.appendChild(feed);
    host.appendChild(card);
    feeds.push(feed);
  }
  // Pin every feed to its newest line in ONE trailing pass (perf-2026-06-10, C-plan
  // finding 2): scrollHeight reads interleaved with appends forced a synchronous
  // layout PER CARD; batching all appends first costs a single layout flush total.
  for (const f of feeds) f.scrollTop = f.scrollHeight;
  dgUpdateStop(); // P1.6: keep the Stop button's enabled state in sync

  // 16-T3: COMPLETION DIGEST — surface the most recent run's outcome here in the
  // Delegations panel instead of typing it into the parent pane's PTY input.
  // A run completion is a RARE event (once per delegation), not a per-tick stream,
  // so this is rendered as a static card below the live workers section.
  // We pick the newest completed run that has a non-empty digest; if there's none
  // (no run has finished yet, or history is cleared) the block is silently omitted.
  dgRenderCompletionDigest(host);
}

// 16-T3: build and append a completion digest card into `host` for the most recently
// finished run (newest by ts, must have a digest). Called at the tail of renderWorkers
// so it appears below the live workers feed — visible without leaving the Delegations
// panel — after each delegation run finishes.
function dgRenderCompletionDigest(host) {
  if (!dgRuns.size) return;
  // Sort descending by run timestamp; take the first entry that carries a digest.
  const sorted = [...dgRuns.entries()].sort((a, b) => dgRunTs(b[0], b[1]) - dgRunTs(a[0], a[1]));
  const entry = sorted.find(([, r]) => typeof r.digest === "string" && r.digest);
  if (!entry) return;
  const [runId, run] = entry;

  const ux = dgVerdictUx((run.verdict || "").toLowerCase());
  const v = (run.verdict || "").toLowerCase();
  const known = v === "pass" || v === "hold" || v === "reject" || v === "advisory" || v === "pr-failed";

  const sec = document.createElement("div");
  sec.className = "dg-section";
  sec.textContent = "Last run — completion digest";
  host.appendChild(sec);

  const card = document.createElement("div");
  card.className = "dg-result" + (known ? " " + v : "");
  card.title = runId; // run id on hover for the operator

  // verdict pill + relative time on one head row
  const cardHead = document.createElement("div");
  cardHead.className = "dg-result-head";
  const verd = document.createElement("span");
  verd.className = "dg-verdict " + (known ? v : "unknown");
  verd.textContent = ux.pill;
  cardHead.appendChild(verd);
  if (run.ts_ms) {
    const when = document.createElement("span");
    when.className = "dg-row-when";
    when.textContent = dgRelTime(run.ts_ms);
    cardHead.appendChild(when);
  }
  card.appendChild(cardHead);

  // the full digest line (monospace, pre-wrap) — the exact text the backend composed,
  // surfaced HERE instead of being typed into the parent PTY. Never innerHTML.
  const dig = document.createElement("div");
  dig.className = "dg-result-digest";
  dig.textContent = run.digest;
  card.appendChild(dig);

  // plain-language verdict meaning — same text the History panel shows on expand
  const mean = document.createElement("div");
  mean.className = "dg-result-mean";
  mean.textContent = ux.mean;
  card.appendChild(mean);

  // action row: Open report / Open PR (same helpers the History panel uses)
  if (run.path || run.pr_url) {
    const foot = document.createElement("div");
    foot.className = "dg-result-foot";
    if (run.path) {
      const openBtn = document.createElement("button");
      openBtn.className = "dg-open";
      openBtn.type = "button";
      openBtn.textContent = "Open report ↗";
      openBtn.addEventListener("click", () => dgOpenResult(run.path));
      foot.appendChild(openBtn);
    }
    if (run.pr_url) {
      const prBtn = document.createElement("button");
      prBtn.className = "dg-open";
      prBtn.type = "button";
      prBtn.textContent = "Open PR ↗";
      prBtn.addEventListener("click", () => dgOpenPR(run.pr_url));
      foot.appendChild(prBtn);
    }
    card.appendChild(foot);
  }

  host.appendChild(card);
}

// #dg-empty shows only when there are NO live workers (Delegations is workers-only now; past runs
// live in the History panel with its own #hist-empty).
function dgUpdateEmpty() {
  const empty = document.getElementById("dg-empty");
  if (empty) empty.classList.toggle("hidden", dgWorkers.size > 0);
}

// Toolbar badge: count of workers currently "running" (so a fired delegation is noticeable
// without hijacking the screen with an auto-opened scrim modal).
function dgUpdateBadge() {
  // History changed (a run started/finished/rehydrated) → refresh the rail cost readout. Cheap and
  // safe to call here: dgUpdateBadge fires on every worker/run event + after loadDelegateHistory.
  renderSessionCost();
  const badge = document.getElementById("dg-badge");
  if (!badge) return;
  const open = delegationsEl && !delegationsEl.classList.contains("hidden");
  let running = 0;
  for (const w of dgWorkers.values()) if (w.status === "running") running++;
  // badge only matters when the panel is CLOSED and something happened
  if (open || (!dgSeenAnyEvent)) { badge.classList.add("hidden"); return; }
  if (running > 0) { badge.textContent = String(running); badge.classList.remove("hidden"); }
  else if (dgWorkers.size > 0) { badge.textContent = "✓"; badge.classList.remove("hidden"); }
  else { badge.classList.add("hidden"); }
}

// rAF-coalesced flush for the worker-stream listeners (perf-2026-06-10 seam 2 /
// B-plan B3, C-plan C-F2): listeners are INGEST-ONLY and arm this; renderWorkers +
// dgUpdateBadge run at most ONCE per animation frame regardless of event rate —
// instead of one full panel rebuild (≈workers × 200 line-divs + a forced layout per
// card) PER OUTPUT LINE, during exactly the period the panel auto-opens for. The
// armed flag IS the dirty bit; the open-check lives in the flush (not at ingest) so
// a panel opened mid-burst paints on the very next frame.
let _dgRafQueued = false;
function dgScheduleRender() {
  if (_dgRafQueued) return;
  _dgRafQueued = true;
  requestAnimationFrame(() => {
    _dgRafQueued = false;
    if (delegationsEl && !delegationsEl.classList.contains("hidden")) renderWorkers();
    dgUpdateBadge();
  });
}

function openDelegations() {
  if (!delegationsEl) return;
  renderDelegations(); // live workers only
  delegationsEl.classList.remove("hidden");
  trapModalFocus(delegationsEl);
  const badge = document.getElementById("dg-badge");
  if (badge) badge.classList.add("hidden"); // clear the badge on open
}

// AUDIT TRAIL: rehydrate past runs from the persisted log (delegate_run_history → HUMAN-initiated,
// newest 200) into dgRuns so the History panel survives an app restart. Records already present (a
// live card) are skipped. Re-renders the History panel if it's open. Best-effort (older backend → no-op).
async function loadDelegateHistory() {
  if (!hasTauri()) return;
  let hist;
  try { hist = await invoke("delegate_run_history"); }
  catch (_) { return; } // command absent (older build) → leave the live cards as-is
  if (!Array.isArray(hist) || !hist.length) return;
  let added = false;
  for (const r of hist) {
    if (!r || !r.run_id || dgRuns.has(r.run_id)) continue;
    dgRuns.set(r.run_id, {
      verdict: r.verdict || "",
      path: r.path || "",
      digest: "",
      pr_url: r.pr_url || "",
      usage: (r.usage && typeof r.usage === "object") ? r.usage : null,
      goal: r.goal || "",
      ts_ms: r.ts_ms || 0,
      duration_ms: Number(r.duration_ms) || 0, // overall wall-clock (0 → fall back to run_id parse)
      workspace_id: r.workspace_id || "", // History scope
      initiator: r.initiator || "human",
      harness: r.harness || "",
      model: r.model || "",
      // P5: persisted review/CRAP fields (DelegateRunRecord, serde-default → may be absent on
      // old records → normalized to empty/null so the History detail degrades gracefully).
      review_decision: typeof r.review_decision === "string" ? r.review_decision : "",
      review_findings: Array.isArray(r.review_findings) ? r.review_findings : [],
      review_calibrated: typeof r.review_calibrated === "boolean" ? r.review_calibrated : null,
      crap_delta: (r.crap_delta && typeof r.crap_delta === "object") ? r.crap_delta : null,
      historic: true,
    });
    added = true;
  }
  if (added) renderSessionCost(); // rehydrated runs carry usage → refresh the rail cost readout
  if (added && historyEl && !historyEl.classList.contains("hidden")) renderHistory();
}
function closeDelegations() {
  if (!delegationsEl) return;
  delegationsEl.classList.add("hidden");
  releaseModalFocus(delegationsEl);
  // prune RETIRED workers from prior runs (bound memory — review LOW) and re-arm the badge so
  // the NEXT delegation re-alerts (dgSeenAnyEvent otherwise sticks true → badge never re-shows).
  for (const [k, v] of dgWorkers) if (v.status === "retired") dgWorkers.delete(k);
  dgResetStopButton(); // P1.6: never persist a half-armed Stop across close/reopen
  dgSeenAnyEvent = false;
  dgUpdateBadge();
  // NOTE: dgRuns is NOT cleared here — it's the History store now (its own panel; persists the
  // session and reloads from disk on History open).
}

// ── Dedicated History panel ──
async function openHistory() {
  if (!historyEl) return;
  dgShowAllHistory = false;        // reset the render cap on each open
  await loadDelegateHistory();     // pull the persisted (human-only) runs into dgRuns
  // Default scope = the ACTIVE workspace, but ONLY when it owns the NEWEST run — otherwise "All".
  // Rationale: a ship-mode flywheel run launched from an Orchestrate team is attributed to that
  // EPHEMERAL team's workspace id (not the active workspace), so scoping to the active workspace
  // hid a just-finished run behind a filter the user would never think to flip. Defaulting to
  // "All" whenever the newest run lives elsewhere guarantees a fresh run is visible on open.
  // `activeWs` is the app's active-workspace global; fall back to the active pane's owner.
  const scope = activeWs || paneOwner(delegateParentId()) || "";
  const newest = [...dgRuns.entries()].sort((a, b) => dgRunTs(b[0], b[1]) - dgRunTs(a[0], a[1]))[0];
  const newestWs = newest ? (newest[1].workspace_id || "") : "";
  const scopeOwnsNewest = !!scope && newestWs === scope;
  histWsFilter = scopeOwnsNewest ? scope : "__all__";
  dgExpandedId = dgNewestInScope(); // default-expand the newest run in this scope (accordion)
  historyEl.classList.remove("hidden");
  trapModalFocus(historyEl);
  renderHistory();
}
function closeHistory() {
  if (!historyEl) return;
  historyEl.classList.add("hidden");
  releaseModalFocus(historyEl);
}

// Auto-surface the panel ONCE per run when its run_id first becomes known (a worker went
// live). Keyed on run_id so a manual close mid-run won't re-pop, but a fresh delegation does.
function dgMaybeAutoOpen(w) {
  if (!w || !w.runId || w.runId === dgAutoOpenedRun) return;
  dgAutoOpenedRun = w.runId;
  if (delegationsEl && delegationsEl.classList.contains("hidden")) openDelegations();
}

// P0.3: open the synthesized result (final.md / final.HELD.md) in Finder/editor via the
// always-compiled `reveal_path` command. Path is backend-supplied (the run dir), never worker
// stdout. Mirrors openExternal: hasTauri guard + try/catch + toast.
async function dgOpenResult(path) {
  if (!path) return;
  if (!hasTauri()) { showToast("Tauri API unavailable"); return; }
  try { await invoke("reveal_path", { path }); }
  catch (e) { showToast(String(e)); }
}

// Phase 3: open the PR URL the controller created (https://…) in the default browser via
// open_external. Backend-supplied URL (from `gh pr create`), never worker stdout.
async function dgOpenPR(url) {
  if (!url) return;
  if (!hasTauri()) { showToast("Tauri API unavailable"); return; }
  try { await invoke("open_external", { url }); }
  catch (e) { showToast(String(e)); }
}

// P1.6: the loud kill-switch. Sets the backend cancel flag (delegate_stop). HONEST copy: the
// controller checks cancelled() at each PHASE boundary, so a worker mid-call halts at its next phase
// (≤3s), not instantly. Two-click in-app arm→confirm (no window.confirm — a wry/WKWebView no-op risk).
let dgStopArmed = false;
let dgStopArmTimer = null;
function dgResetStopButton() {
  dgStopArmed = false;
  clearTimeout(dgStopArmTimer);
  const btn = document.getElementById("dg-stop");
  if (btn) { btn.textContent = "Stop all workers"; btn.classList.remove("armed"); }
  dgUpdateStop();
}
function onStopClick() {
  if (!dgStopArmed) {
    dgStopArmed = true;
    const btn = document.getElementById("dg-stop");
    if (btn) { btn.textContent = "Confirm stop?"; btn.classList.add("armed"); }
    clearTimeout(dgStopArmTimer);
    dgStopArmTimer = setTimeout(dgResetStopButton, 4000); // lapse → revert (no accidental kill)
    return;
  }
  stopAllWorkers();
}
async function stopAllWorkers() {
  clearTimeout(dgStopArmTimer);
  dgStopArmed = false;
  const btn = document.getElementById("dg-stop");
  if (!hasTauri()) { showToast("Tauri API unavailable"); dgResetStopButton(); return; }
  if (btn) { btn.disabled = true; btn.textContent = "Stopping…"; btn.classList.remove("armed"); }
  try {
    await invoke("delegate_stop");
    showToast("Stopping delegation — workers halt at their next phase and are swept.");
  } catch (e) {
    showToast("Stop failed: " + String(e));
  } finally {
    if (btn) btn.textContent = "Stop all workers";
    dgUpdateStop();
  }
}
// A delegation is "in flight" if any worker is running, OR events seen but no completion card yet
// (the orchestrate dead-window before the first worker status). Enable Stop exactly then.
function dgRunInFlight() {
  for (const w of dgWorkers.values()) if (w.status === "running") return true;
  if (dgSeenAnyEvent && dgRuns.size === 0) {
    for (const w of dgWorkers.values()) if (w.status !== "retired") return true;
    if (dgWorkers.size === 0) return true; // events arrived but no worker entry yet (very early)
  }
  return false;
}
function dgUpdateStop() {
  const btn = document.getElementById("dg-stop");
  if (btn) btn.disabled = !dgRunInFlight();
}
const dgStopEl = document.getElementById("dg-stop");
if (dgStopEl) dgStopEl.onclick = () => onStopClick(); // two-click arm→confirm (no window.confirm)

const dgCloseEl = document.getElementById("dg-close");
if (dgCloseEl) dgCloseEl.onclick = () => closeDelegations();
// click the scrim (outside the card) to close, mirroring nothing-special — additive nicety
if (delegationsEl) delegationsEl.addEventListener("click", (e) => { if (e.target === delegationsEl) closeDelegations(); });

// Dedicated History panel wiring (opened via the More menu; close; scrim; workspace-scope picker).
const histCloseEl = document.getElementById("hist-close");
if (histCloseEl) histCloseEl.onclick = () => closeHistory();
if (historyEl) historyEl.addEventListener("click", (e) => { if (e.target === historyEl) closeHistory(); });
const histWsEl = document.getElementById("hist-ws");
if (histWsEl) histWsEl.addEventListener("change", () => { histWsFilter = histWsEl.value; dgShowAllHistory = false; dgExpandedId = dgNewestInScope(); renderHistory(); });

// ─────────────────────────── P3 LOOPS surface ───────────────────────────
// A sibling to the #history list: lists saved loops (loop_list) with create/edit/pause/delete/
// Run-Now. CRUD maps 1:1 to the SHARED CONTRACT commands (loop_create/loop_update/loop_delete/
// loop_set_enabled/loop_run_now). Loops are SAVED from the Orchestrate modal (submitBridgeLoop);
// this panel manages the saved set. Per-loop iterations land in Runs (delegate-runs.jsonl, the
// backend tags each record with loop_id).
const loopsEl = document.getElementById("loops");
let loopsCache = [];           // last loop_list() result
let loopsExpandedId = null;    // accordion: which loop is open

function loopsSetError(msg) {
  const e = document.getElementById("loops-error");
  if (!e) return;
  e.textContent = msg || "";
  e.classList.toggle("hidden", !msg);
}
function loopStopLabel(stop) {
  if (!stop || typeof stop !== "object") return "—";
  const k = stop.kind || "";
  const n = stop.max_iters != null ? stop.max_iters : "?";
  if (k === "max_iters") return `max ${n} iters`;
  if (k === "goal_met") return `goal-met · ≤${n}`;
  return `until pass · ≤${n}`;   // until_pass (default)
}
function loopScheduleLabel(sched) {
  const k = sched && sched.kind ? sched.kind : "manual";
  return k.charAt(0).toUpperCase() + k.slice(1);
}
async function loadLoops() {
  if (!hasTauri()) { loopsCache = []; return; }
  try { loopsCache = await invoke("loop_list") || []; }
  catch (e) { loopsCache = []; loopsSetError("Couldn't load loops: " + String(e)); return; }
  if (!Array.isArray(loopsCache)) loopsCache = [];
}
function renderLoops() {
  const host = document.getElementById("loops-list");
  if (!host) return;
  host.replaceChildren();
  const empty = document.getElementById("loops-empty");
  if (empty) empty.classList.toggle("hidden", loopsCache.length > 0);
  if (!loopsCache.length) return;

  const rows = document.createElement("div");
  rows.className = "dg-rows";
  host.appendChild(rows);

  for (const lp of loopsCache) {
    const expanded = lp.id === loopsExpandedId;
    const rcard = document.createElement("div");
    rcard.className = "dg-result" + (expanded ? " expanded" : "");

    const row = document.createElement("button");
    row.type = "button";
    row.className = "dg-result-row";
    row.setAttribute("aria-expanded", String(expanded));
    row.title = lp.id || "";

    const chev = document.createElement("span");
    chev.className = "dg-row-chev";
    chev.textContent = expanded ? "▾" : "▸";

    const verd = document.createElement("span");
    const lastV = (lp.last_run && lp.last_run.verdict || "").toLowerCase();
    const knownV = lastV === "pass" || lastV === "hold" || lastV === "reject" || lastV === "advisory" || lastV === "pr-failed";
    verd.className = "dg-verdict " + (knownV ? lastV : (lp.enabled ? "unknown" : "hold"));
    verd.textContent = lp.enabled ? (knownV ? lastV : "loop") : "paused";

    const name = document.createElement("span");
    name.className = "dg-row-when";
    name.textContent = lp.name || lp.goal || "(untitled)";

    const harn = document.createElement("span");
    harn.className = "dg-row-harness";
    harn.textContent = lp.runner || "";
    harn.title = `${lp.runner || "?"}${lp.ship ? " · ship" : ""} · ${lp.harness || "default"}`;

    const meta = document.createElement("span");
    meta.className = "dg-row-meta";
    let metaStr = ` · ${loopScheduleLabel(lp.schedule)} · ${loopStopLabel(lp.stop)}`;
    if (lp.ship) metaStr += " · ship";
    if (lp.last_run && lp.last_run.pr_url) metaStr += " · 🔀 PR";
    meta.textContent = metaStr;

    row.append(chev, verd, name, harn, meta);
    row.addEventListener("click", () => {
      loopsExpandedId = (loopsExpandedId === lp.id) ? null : lp.id;
      renderLoops();
    });
    rcard.appendChild(row);

    if (expanded) rcard.appendChild(buildLoopDetail(lp));
    rows.appendChild(rcard);
  }
}
function buildLoopDetail(lp) {
  const wrap = document.createElement("div");
  wrap.className = "dg-result-detail";

  const goal = document.createElement("p");
  goal.className = "modal-sub loop-goal";
  goal.textContent = lp.goal || "(no goal)";
  wrap.appendChild(goal);

  const facts = document.createElement("p");
  facts.className = "modal-sub";
  facts.textContent =
    `Runner ${lp.runner || "?"} · workers ${lp.workers != null ? lp.workers : "?"} · ` +
    `concurrency ${lp.concurrency || "serialized"} · merge → ${lp.merge_target || "loop-integration"}` +
    (lp.last_run && lp.last_run.verdict ? ` · last: ${lp.last_run.verdict}` : "");
  wrap.appendChild(facts);

  const actions = document.createElement("div");
  actions.className = "modal-actions";

  const runBtn = document.createElement("button");
  runBtn.type = "button";
  runBtn.className = "primary";
  runBtn.textContent = "Run now";
  runBtn.addEventListener("click", () => loopRunNow(lp.id));

  const pauseBtn = document.createElement("button");
  pauseBtn.type = "button";
  pauseBtn.textContent = lp.enabled ? "Pause" : "Resume";
  pauseBtn.addEventListener("click", () => loopSetEnabled(lp.id, !lp.enabled));

  const editBtn = document.createElement("button");
  editBtn.type = "button";
  editBtn.textContent = "Edit goal";
  editBtn.addEventListener("click", () => beginLoopGoalEdit(lp, goal));

  const delBtn = document.createElement("button");
  delBtn.type = "button";
  delBtn.className = "dg-stop-btn";
  delBtn.textContent = "Delete";
  delBtn.addEventListener("click", () => loopDelete(lp.id));

  actions.append(runBtn, pauseBtn, editBtn, delBtn);
  wrap.appendChild(actions);
  return wrap;
}
async function loopRunNow(id) {
  if (!id || !hasTauri()) return;
  loopsSetError("");
  try { await invoke("loop_run_now", { id }); showToast("Loop iteration enqueued — watch it in Runs."); }
  catch (e) { loopsSetError("Run-Now failed: " + String(e)); }
}
async function loopSetEnabled(id, enabled) {
  if (!id || !hasTauri()) return;
  loopsSetError("");
  try { await invoke("loop_set_enabled", { id, enabled }); }
  catch (e) { loopsSetError("Couldn't update: " + String(e)); }
  await loadLoops();
  renderLoops();
}
async function loopDelete(id) {
  if (!id || !hasTauri()) return;
  loopsSetError("");
  try { await invoke("loop_delete", { id }); }
  catch (e) { loopsSetError("Couldn't delete: " + String(e)); return; }
  if (loopsExpandedId === id) loopsExpandedId = null;
  await loadLoops();
  renderLoops();
}
// Minimal in-place goal edit for P3 — an INLINE input swapped over the goal line, NOT
// window.prompt (wry/WKWebView can no-op the native dialogs; same reason onStopClick and
// the force-respawn path use in-app patterns). Mirrors beginWorkspaceRename: Enter commits,
// Escape cancels, blur commits; renderLoops repaints the row either way.
function beginLoopGoalEdit(lp, goalEl) {
  if (!lp || !lp.id || !hasTauri() || !goalEl || goalEl.querySelector("input")) return;
  const input = document.createElement("input");
  input.className = "ws-rename";
  input.value = lp.goal || "";
  input.spellcheck = false;
  goalEl.textContent = "";
  goalEl.appendChild(input);
  input.focus();
  input.select();
  let done = false;
  const finish = async (commit) => {
    if (done) return;
    done = true;
    const goal = commit ? (input.value || "").trim() : "";
    if (!commit || !goal || goal === (lp.goal || "")) { renderLoops(); return; }
    loopsSetError("");
    const name = goal.length > 60 ? goal.slice(0, 57) + "…" : goal;
    try { await invoke("loop_update", { id: lp.id, patch: { goal, name } }); }
    catch (e) { loopsSetError("Couldn't save edit: " + String(e)); renderLoops(); return; }
    await loadLoops();
    renderLoops();
  };
  input.addEventListener("keydown", (e) => {
    e.stopPropagation();
    if (e.key === "Enter") { e.preventDefault(); finish(true); }
    else if (e.key === "Escape") { e.preventDefault(); finish(false); }
  });
  input.addEventListener("blur", () => finish(true));
  input.addEventListener("click", (e) => e.stopPropagation());
  input.addEventListener("mousedown", (e) => e.stopPropagation());
}
// Read-only auto-refire arm line for the Loops panel. `loop_autonomy` is OPTIONAL in
// delegate_gate_status (older backend omits it) — absent → say nothing (line stays hidden).
async function paintLoopAutonomyLine() {
  const existing = document.getElementById("loops-autonomy");
  if (existing) existing.classList.add("hidden");
  if (!hasTauri()) return;
  let g;
  try { g = await invoke("delegate_gate_status"); } catch (_) { return; }
  lastDelegateGate = g;
  if (!g || g.loop_autonomy === undefined) return;
  let el = existing;
  if (!el) {
    const list = document.getElementById("loops-list");
    if (!list || !list.parentNode) return;
    el = document.createElement("p");
    el.id = "loops-autonomy";
    el.className = "modal-sub";
    list.parentNode.insertBefore(el, list);
  }
  el.textContent = `auto-refire: ${g.loop_autonomy ? "armed" : "off"} (loop_autonomy)`;
  el.classList.remove("hidden");
}
async function openLoops() {
  if (!loopsEl) return;
  loopsSetError("");
  paintLoopAutonomyLine(); // fire-and-forget — presence-gated, hidden on older backends
  await loadLoops();
  // default-expand the first loop (mirrors History's newest-expanded default)
  loopsExpandedId = loopsCache.length ? loopsCache[0].id : null;
  loopsEl.classList.remove("hidden");
  trapModalFocus(loopsEl);
  document.getElementById("loops-btn")?.classList.add("active");
  renderLoops();
}
function closeLoops() {
  if (!loopsEl) return;
  loopsEl.classList.add("hidden");
  releaseModalFocus(loopsEl);
  document.getElementById("loops-btn")?.classList.remove("active");
}
const loopsBtnEl = document.getElementById("loops-btn");
if (loopsBtnEl) loopsBtnEl.onclick = () => openLoops();
const loopsCloseEl = document.getElementById("loops-close");
if (loopsCloseEl) loopsCloseEl.onclick = () => closeLoops();
if (loopsEl) loopsEl.addEventListener("click", (e) => { if (e.target === loopsEl) closeLoops(); });

// Register the two listeners at module load (NOT inside openDelegations) so the map fills
// even when the panel is closed. Re-render only when the panel is open (cheap when closed).
if (tauriEvent && tauriEvent.listen) {
  tauriEvent.listen("delegate-worker-log", (e) => {
    const p = e && e.payload;
    if (!p || !p.worker) return;
    dgSeenAnyEvent = true;
    const w = dgEntry(p.worker);
    if (p.run_id && !w.runId) w.runId = p.run_id;
    // BOTH payload shapes (CONTRACT seam 2): batched {lines:[..]} — lane W1 switches
    // the backend to per-worker ~120ms pumps in this same build — and legacy {line},
    // so either side can land first without dropping a single worker's stream.
    const lines = Array.isArray(p.lines) ? p.lines : (typeof p.line === "string" ? [p.line] : []);
    for (const ln of lines) dgIngestLine(w, typeof ln === "string" ? ln : "");
    dgMaybeAutoOpen(w);
    // INGEST-ONLY here: rendering is coalesced to ≤1 renderWorkers/frame (the rAF
    // flush). The history section is untouched either way — that split keeps a long
    // history from rebuilding on worker output.
    dgScheduleRender();
  });
  tauriEvent.listen("delegate-worker-status", (e) => {
    const p = e && e.payload;
    if (!p || !p.worker) return;
    dgSeenAnyEvent = true;
    const w = dgEntry(p.worker);
    // "retired" payload is {worker,status} only — DON'T clobber an existing runId/repo with undefined.
    if (p.run_id) w.runId = p.run_id;
    if (p.repo) w.repo = p.repo;
    if (p.harness) w.harness = p.harness; // for the "streams at completion" note (non-claude buffer)
    if (p.status === "retired") w.status = "retired";
    else if (p.status === "running" && w.status !== "done") w.status = "running";
    dgMaybeAutoOpen(w);
    // Status flips fold into the same coalesced flush as log lines (a pill change
    // never alters the history rows, and per-event renders re-amplify under load).
    dgScheduleRender();
  });
  // P0.2: run-level completion — ONE event per delegation after synthesis. Carries the verdict
  // (lowercase pass|hold|reject), the REAL written path (final.md | final.HELD.md), and the
  // completion digest → drives the Delegations completion card (16-T3: digest is shown HERE,
  // never typed into the parent pane's PTY). Inert in a default build (the backend only
  // emits this behind cfg(feature="delegate-live")).
  tauriEvent.listen("delegate-run-result", (e) => {
    const p = e && e.payload;
    if (!p || !p.run_id) return;
    dgSeenAnyEvent = true;
    dgRuns.set(p.run_id, {
      verdict: typeof p.verdict === "string" ? p.verdict : "",
      path: typeof p.path === "string" ? p.path : "",
      // 16-T3: digest is now DISPLAYED IN THE DELEGATIONS PANEL by dgRenderCompletionDigest.
      // It was previously written to the parent pane's PTY input (say_back in lib.rs) which
      // caused it to be submitted as an agent prompt. The backend no longer calls say_back for
      // the completion digest; the payload field here is the sole consumer.
      digest: typeof p.digest === "string" ? p.digest : "",
      // Phase 3: present only when a shipped run opened a PR (null/absent otherwise).
      pr_url: typeof p.pr_url === "string" ? p.pr_url : "",
      // token/cost meter (orchestrator + workers + synthesizer) — absent on old/default builds.
      usage: (p.usage && typeof p.usage === "object") ? p.usage : null,
      // audit context (also persisted to the run record for the History view).
      goal: typeof p.goal === "string" ? p.goal : "",
      ts_ms: typeof p.ts_ms === "number" ? p.ts_ms : 0,
      // History scope/provenance (v2): live runs carry them so the History panel scopes + filters
      // a still-open run exactly like a rehydrated one.
      workspace_id: typeof p.workspace_id === "string" ? p.workspace_id : "",
      initiator: typeof p.initiator === "string" ? p.initiator : "human",
      harness: typeof p.harness === "string" ? p.harness : "",
      model: typeof p.model === "string" ? p.model : "",
      // P5 (unified-engine §3.6/§3.10): smart-PR-review verdict + CRAP delta. ADVISORY in P5
      // (logged-only, does NOT drive `verdict` above) — absent on advisory/old runs → omitted,
      // so dgReviewData() degrades to null and nothing is rendered. Normalized at READ time.
      review_decision: typeof p.review_decision === "string" ? p.review_decision : "",
      review_findings: Array.isArray(p.review_findings) ? p.review_findings : [],
      review_calibrated: typeof p.review_calibrated === "boolean" ? p.review_calibrated : null,
      crap_delta: (p.crap_delta && typeof p.crap_delta === "object") ? p.crap_delta : null,
    });
    // spoken completion (continuous-monitoring ask): the flywheel verdict, hands-free.
    announce(`Flywheel run finished — verdict ${p.verdict || "unknown"}${p.pr_url ? ". A pull request was opened." : "."}`);
    // A run completed → accordion-expand THIS run (newest), collapsing the rest. Re-render the
    // History panel (new row) if open.
    dgExpandedId = p.run_id;
    if (historyEl && !historyEl.classList.contains("hidden")) renderHistory();
    // 16-T3: surface the completion digest in the Delegations panel. If it is already open,
    // re-render it (dgRenderCompletionDigest runs at the tail of renderWorkers). If it is
    // CLOSED, open it — the operator fired this delegation and deserves to see the outcome
    // without having to hunt for the toolbar button (same logic as dgMaybeAutoOpen at run START).
    if (delegationsEl) {
      if (delegationsEl.classList.contains("hidden")) {
        openDelegations(); // opens + calls renderDelegations which calls renderWorkers
      } else {
        renderWorkers();   // already open — refresh to paint the completion card
      }
    }
    dgUpdateBadge();
  });
}

// refit the active terminal on window resize (debounced; active only)
let resizeTimer;
window.addEventListener("resize", () => {
  clearTimeout(resizeTimer);
  resizeTimer = setTimeout(() => {
    // tile rects are computed from host.clientWidth/Height, so a window resize MUST recompute
    // them (not just refit) — relayout() re-places panes, refits the visible set, and rebuilds
    // dividers from the new geometry.
    relayout();
  }, 120);
});

// sidebar toggle: wire + restore the persisted collapsed state on startup.
const railToggleEl = document.getElementById("rail-toggle");
if (railToggleEl) railToggleEl.onclick = () => toggleRail();
applyRailCollapsed((() => { try { return localStorage.getItem(RAIL_COLLAPSED_KEY) === "1"; } catch (_) { return false; } })());
const gridBtnEl = document.getElementById("grid-btn");
if (gridBtnEl) gridBtnEl.onclick = () => toggleGrid();
const broadcastBtnEl = document.getElementById("broadcast-btn");
if (broadcastBtnEl) broadcastBtnEl.onclick = () => toggleBroadcast();

// WORKSPACES section header: + New (modal) and a collapse caret.
const wsQuickBtnEl = document.getElementById("ws-quick-btn");
if (wsQuickBtnEl) wsQuickBtnEl.onclick = () => quickCreate();
const wsNewBtnEl = document.getElementById("ws-new-btn");
if (wsNewBtnEl) wsNewBtnEl.onclick = launchWizard;
// BridgeMind launcher mode cards → existing actions (delegated; all targets exist).
document.getElementById("launcher")?.addEventListener("click", (e) => {
  const btn = e.target.closest("[data-lc]");
  if (!btn) return;
  e.preventDefault();
  const mode = btn.dataset.lc;
  if (mode === "workspace") launchWizard();
  else if (mode === "swarm") openBridge();
  else if (mode === "board") openBoard();
  else if (mode === "memory") openDelegations();
});
// auto-tile: re-balance all panes into an even layout.
const autoTileBtnEl = document.getElementById("auto-tile-btn");
if (autoTileBtnEl) autoTileBtnEl.onclick = () => autoTile();
// #6 split-tree: Split-right (v) / Split-down (h) spawn a new pane splitting the focused one.
const splitVBtnEl = document.getElementById("split-v-btn");
if (splitVBtnEl) splitVBtnEl.onclick = () => splitPane("v");
const splitHBtnEl = document.getElementById("split-h-btn");
if (splitHBtnEl) splitHBtnEl.onclick = () => splitPane("h");

// 06-18 topbar dedupe: "Runs" → the persistent run-history panel (Delegate + Flywheel runs,
// workspace-scoped — the merged successor to the old Delegations/History split). The live
// streaming-worker view stays reachable via "⋯ More → Live workers".
const runsBtnEl = document.getElementById("runs-btn");
if (runsBtnEl) runsBtnEl.onclick = () => openHistory();
// "⋯ More" overflow menu: secondary/utility actions, all routed to their existing functions.
const moreBtnEl = document.getElementById("topbar-more-btn");
const moreMenuEl = document.getElementById("topbar-more-menu");
function closeTopbarMore() {
  if (!moreMenuEl) return;
  moreMenuEl.classList.add("hidden");
  if (moreBtnEl) moreBtnEl.setAttribute("aria-expanded", "false");
  document.removeEventListener("mousedown", onTopbarMoreOutside, true);
  document.removeEventListener("keydown", onTopbarMoreKey, true);
}
function onTopbarMoreOutside(e) { if (moreMenuEl && !moreMenuEl.contains(e.target) && e.target !== moreBtnEl) closeTopbarMore(); }
function onTopbarMoreKey(e) { if (e.key === "Escape") closeTopbarMore(); }
if (moreBtnEl && moreMenuEl) {
  moreBtnEl.onclick = (e) => {
    e.stopPropagation();
    if (!moreMenuEl.classList.contains("hidden")) { closeTopbarMore(); return; }
    // #topbar has backdrop-filter, which makes it the containing block for position:fixed
    // descendants — so the menu's viewport-computed left/top would resolve against #topbar's
    // box and render off-screen. Reparent to <body> (idempotent) so fixed === viewport, exactly
    // like the pane kebab (openPaneMenu appends to body for the same reason).
    if (moreMenuEl.parentElement !== document.body) document.body.appendChild(moreMenuEl);
    moreMenuEl.classList.remove("hidden");
    moreBtnEl.setAttribute("aria-expanded", "true");
    const r = moreBtnEl.getBoundingClientRect();
    const mw = moreMenuEl.offsetWidth || 230;
    let left = Math.min(r.right - mw, window.innerWidth - mw - 8);
    if (left < 8) left = 8;
    moreMenuEl.style.left = left + "px";
    moreMenuEl.style.top = (r.bottom + 6) + "px";
    document.addEventListener("mousedown", onTopbarMoreOutside, true);
    document.addEventListener("keydown", onTopbarMoreKey, true);
  };
  moreMenuEl.addEventListener("click", (e) => {
    const item = e.target.closest("[data-more]");
    if (!item) return;
    e.stopPropagation();
    closeTopbarMore();
    const a = item.dataset.more;
    if (a === "speak") openSpeak();
    else if (a === "live") openDelegations();
    else if (a === "browser") toggleBrowser();
    else if (a === "settings") openSettings();
  });
}
const wsCollapseEl = document.getElementById("ws-collapse");
if (wsCollapseEl) wsCollapseEl.onclick = () => {
  const list = document.getElementById("workspaces");
  const collapsed = list.classList.toggle("collapsed");
  wsCollapseEl.textContent = collapsed ? "▸" : "▾";
};

// Scheduler cap stepper (±1, clamp ≥1) → set_max_concurrent + persist + re-render.
const schedIncEl = document.getElementById("sched-inc");
if (schedIncEl) schedIncEl.onclick = () => setSchedMax(schedMax() + 1, true);
const schedDecEl = document.getElementById("sched-dec");
if (schedDecEl) schedDecEl.onclick = () => setSchedMax(schedMax() - 1, true);

// `workspace-admitted` (D33): a queued spawn got its PTY — attach its live terminal.
if (tauriEvent && tauriEvent.listen) {
  tauriEvent.listen("workspace-admitted", (e) => {
    const p = (e && e.payload) || {};
    if (p.id) admitPending(p.id, p.harness);
  });
}

loadWorkspaces();
// Restored workspaces are DORMANT (no live panes), so `sessions` is empty on launch but
// relayout()/syncEmptyState() never ran — leaving the DOM in its HTML default (bare
// #placeholder shown, rich #launcher hidden). Resolve the empty state once at startup so
// an empty launch shows the centered landing menu, not the fallback hint line.
syncEmptyState();
setActiveLabel(activeId); // seed #main's .no-session class at boot (styles.css keys the label's live tick on it)
loadPending(); // restore queued Scheduler rows so a webview reload doesn't orphan them
// Push the cap at startup ONLY if the user explicitly set one (v2 key); otherwise just
// render — the backend already booted with its core-scaled default, and pushing the
// frontend fallback here is exactly the bug that pinned every machine to cap 3.
if (Number.isFinite(parseInt(localStorage.getItem(LS_KEYS.maxConcurrent), 10))) setSchedMax(schedMax());
else renderScheduler();
pollOutput();
pollQueue();
renderSessionCost(); // seed the rail cost readout ($0.00) at boot, before the first poll/history load
pollDelegateHud(); // background headless-run progress chip (+ stuck-lock force-clear)
restoreBridgeRun(); // resume an in-flight Bridge fan-in after a webview reload — else pollBridgeReady's guard (bridgeRunDir && bridgePaneIds.length) stays false and auto-synth is silently inert until the modal is reopened
// dock: a reload during a run reopens the dock showing the resumed run (the restore
// above already emitted its turns via the wrappers/observers).
if (bridgeChatDock) {
  let dockOpen = false;
  try { dockOpen = localStorage.getItem(LS_KEYS.bridgeDockOpen) === "1"; } catch (_) {}
  if (dockOpen) openBridgeDock();
}
pollBridgeReady(); // 07-02: auto-synthesize when a dispatched run's agents finish
