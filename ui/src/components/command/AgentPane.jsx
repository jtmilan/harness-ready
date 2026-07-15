import React, { useEffect, useLayoutEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { STATUS_META } from "@/lib/agentData";
import { AGENT_KINDS } from "@/lib/agentTypes";
import { setPaneLabel, usePaneLabel } from "@/lib/paneLabels";
import AttentionPrompt from "@/components/command/AttentionPrompt";
import PaneMenu from "@/components/command/PaneMenu";

// ---- renderer policy: WebGL2 on VISIBLE panes only (ported from agent-teams main.js:677-760).
// The DOM renderer rebuilds each dirty row's span-DOM per frame — N streaming TUIs ≈ 10-30k node
// churn/s. WebGL moves that to GPU draws. WKWebView caps live WebGL contexts per page (~16; the
// OLDEST context gets context-lost when exceeded), so attach ONLY to visible panes and dispose on
// hide. Hidden panes fall back to the DOM renderer (xterm 6 render-pauses them). Kill-switch:
// localStorage at_no_webgl="1" → never attach (field rollback without a rebuild).
//
// Slot tracking is a Set of per-pane state objects (not a bare counter). A counter can only be
// +1/−1'd correctly on every path forever; a Set is membership-idempotent — detach/context-loss
// races cannot permanently inflate the live count, and size is the source of truth for the cap.
const MAX_WEBGL = 8;
let webglDisabled = (() => { try { return localStorage.getItem("at_no_webgl") === "1"; } catch { return false; } })();
const webglAttached = new Set(); // pane state objects currently holding a live WebGL context
let _WebglCtor = null;
let _WebglLoad = null;
function loadWebglCtor() {
  if (_WebglCtor) return Promise.resolve(_WebglCtor);
  if (!_WebglLoad) {
    _WebglLoad = import("@xterm/addon-webgl")
      .then((m) => { _WebglCtor = m.WebglAddon; return _WebglCtor; })
      .catch((e) => { _WebglLoad = null; throw e; }); // allow a retry on a transient load failure
  }
  return _WebglLoad;
}
// s = { term, webgl, webglBlocked } per-pane renderer state (a ref's .current in the component).
// Guards are RE-CHECKED after the await: the pane may have hidden/disposed while loading.
async function attachWebgl(s) {
  if (webglDisabled || !s || s.webgl || s.webglBlocked || !s.term || webglAttached.size >= MAX_WEBGL) return;
  let WebglAddon;
  try {
    WebglAddon = await loadWebglCtor();
  } catch {
    s.webglBlocked = true; // module load failed — DOM renderer stays; a later pane can retry the import
    return;
  }
  if (webglDisabled || s.webgl || s.webglBlocked || !s.term || webglAttached.size >= MAX_WEBGL) return;
  try {
    const addon = new WebglAddon();
    addon.onContextLoss(() => {
      // GPU context lost (OOM / suspend / cap eviction): dispose restores the DOM renderer.
      // PERMANENT fallback for THIS pane — re-attaching on a pressured GPU just flaps.
      try { addon.dispose(); } catch { /* already disposed */ }
      // Identity check: only release if THIS addon is still the registered one (detachWebgl may
      // have already cleared the slot — Set.delete is idempotent either way).
      if (s.webgl === addon) s.webgl = null;
      webglAttached.delete(s);
      s.webglBlocked = true;
    });
    s.term.loadAddon(addon); // throws when WebGL2 is unavailable in this webview
    // Re-check after loadAddon: unmount/hide may have raced the async ctor path and already
    // run detachWebgl (s.term null). Never claim a slot for a dead pane.
    if (!s.term || s.webgl || s.webglBlocked) {
      try { addon.dispose(); } catch { /* */ }
      return;
    }
    s.webgl = addon;
    webglAttached.add(s);
  } catch {
    // Per-pane block first: webgl2 getContext can fail TRANSIENTLY under GPU pressure — one
    // flaky pane must not downgrade every pane. Latch globally only when a bare capability
    // probe agrees WebGL2 is truly absent.
    s.webglBlocked = true;
    try {
      if (!document.createElement("canvas").getContext("webgl2")) webglDisabled = true;
    } catch { webglDisabled = true; }
  }
}
function detachWebgl(s) {
  if (!s) return;
  if (s.webgl) {
    try { s.webgl.dispose(); } catch { /* already disposed */ }
    s.webgl = null;
  }
  // Always drop membership (idempotent). A bare counter would under/over-count when context-loss
  // and unmount both fire; Set.delete cannot permanently inflate the live size.
  webglAttached.delete(s);
}

// Terminal theme sourced to match the pane's CRT look (cyan-on-near-black).
const TERM_THEME = {
  background: "#0A1219",
  foreground: "#9fe6f5",
  cursor: "#67e8f9",
  selectionBackground: "#164e63",
  black: "#0A1219", brightBlack: "#334155",
  red: "#f87171", brightRed: "#fca5a5",
  green: "#4ade80", brightGreen: "#86efac",
  yellow: "#fcd34d", brightYellow: "#fde68a",
  blue: "#60a5fa", brightBlue: "#93c5fd",
  magenta: "#c084fc", brightMagenta: "#d8b4fe",
  cyan: "#22d3ee", brightCyan: "#67e8f9",
  white: "#cbd5e1", brightWhite: "#f1f5f9",
};

export default function AgentPane({ agent, selected, checked, onToggleCheck, onSelect, onRespond, onInput, onResize, style, onDragStart, visible = true, zoomed = false, onMaximize, onMenuAction, workspaces = [] }) {
  const meta = STATUS_META[agent.status];
  const mountRef = useRef(null);
  const termRef = useRef(null);
  const fitRef = useRef(null);
  const writtenRef = useRef(0); // bytes of agent.raw already written to the terminal
  const genRef = useRef(agent.gen || 0);
  // Debounced fit→resize_pty trigger owned by the mount effect; exposed via a ref so
  // the layout-props effect below can nudge it without re-running the mount effect.
  const pushResizeRef = useRef(null);
  // Per-pane renderer state for the WebGL policy (term set at mount; webgl managed by visibility).
  const glRef = useRef({ term: null, webgl: null, webglBlocked: false });

  // Latest-prop refs for callbacks bound once inside the mount-once xterm effect (deps =
  // [agent.id] only — the terminal must never be re-created; resize fix 870dacf depends on
  // that). Without these, term.onData / the fit→resize_pty path close over the FIRST render's
  // onInput / onResize forever (BUG-2: broadcast stays false inside the bound keystroke path).
  // onSelect is NOT once-bound — it rides the container's re-rendered JSX handlers — so no ref.
  const onInputRef = useRef(onInput);
  const onResizeRef = useRef(onResize);
  useEffect(() => { onInputRef.current = onInput; });
  useEffect(() => { onResizeRef.current = onResize; });

  // ---- Rename (display label).
  // The label READ lives here because this component renders the header label and nothing else
  // does; routing it through a prop would need a Home.jsx edit, which belongs to another lane.
  // `paneLabels` owns the storage and the subscription (BRIEF C2's stated exception), so this
  // pane never sees localStorage and re-renders when ANY pane's menu commits a rename. The write
  // sits next to the read on purpose: split across the boundary, the module's storage would drift
  // from React state and the header would show a stale name until an unrelated re-render.
  const labelOverride = usePaneLabel(agent.id);
  const defaultName = agent.name || agent.id; // what the header shows with no override
  const displayName = labelOverride || defaultName;
  const [renaming, setRenaming] = useState(false);
  const renameDoneRef = useRef(false); // one commit per edit — ignores a trailing blur
  const renameInputRef = useRef(null);
  const renameOpenRef = useRef(0);

  const beginRename = () => {
    renameDoneRef.current = false;
    // Next task, NOT now. Coming from the kebab, Radix is still tearing the menu down and moving
    // focus when onSelect fires; an editor mounted inside that teardown is immediately blurred BY
    // the teardown, and blur commits — so it would close the instant it opened. Yielding once
    // lets the menu finish, and then nothing else is competing for focus.
    //
    // A timer, not requestAnimationFrame: rAF is tied to painting and is throttled/parked while
    // the window is occluded or backgrounded, which would silently strand the editor closed. The
    // ordering this needs is a task boundary, and paint has nothing to do with it.
    clearTimeout(renameOpenRef.current);
    renameOpenRef.current = setTimeout(() => setRenaming(true), 0);
  };
  // Layout cleanup runs before the focused input is removed from the DOM, so renameDoneRef is
  // latched BEFORE the synthetic unmount blur. That blur would otherwise call finishRename →
  // setRenaming(false) (setState on unmount) and setPaneLabel (localStorage write during teardown).
  // Discard mid-rename on close; a real Enter/blur while mounted still commits normally.
  useLayoutEffect(() => () => {
    renameDoneRef.current = true;
    clearTimeout(renameOpenRef.current);
  }, []);

  const finishRename = (value, commit) => {
    if (renameDoneRef.current) return;
    renameDoneRef.current = true;
    setRenaming(false);
    if (!commit) return;
    const typed = (value || "").trim();
    // Committing the name the pane already shows CLEARS the override instead of storing a copy of
    // it. paneLabels applies prod's "same as the id ⇒ clear" rule (main.js:225), but this fork's
    // default is `agent.name || agent.id`, and only this component knows that. Without this, a
    // rename dialog opened and confirmed unchanged would pin today's `agent.name` forever — and
    // silently win over the real one if the backend ever renamed the agent.
    const label = typed === defaultName ? "" : typed;
    setPaneLabel(agent.id, label);
    // Still announced, so the container can react (toast/telemetry) even though the store above
    // is already authoritative for what the header paints.
    onMenuAction && onMenuAction("rename", agent.id, { label });
  };

  // Focus the editor explicitly rather than via `autoFocus`: the rename is normally triggered
  // FROM the kebab menu, and the menu's closing focus-restore lands after mount. PaneMenu
  // suppresses that restore for this one item; taking the focus here as well means the editor
  // still lands focused when it is opened by double-click, with no menu involved at all.
  useEffect(() => {
    if (!renaming) return;
    const el = renameInputRef.current;
    if (!el) return;
    el.focus();
    el.select(); // whole-label select: renaming usually replaces rather than appends
  }, [renaming]);

  const handleMenuAction = (action, payload) => {
    if (action === "rename") {
      // The only item handled locally: it opens the inline editor. The contract's
      // `{ label }` payload only exists once the edit COMMITS, via finishRename.
      beginRename();
      return;
    }
    onMenuAction && onMenuAction(action, agent.id, payload);
  };

  // Mount a single xterm per pane (created once, kept for the pane's lifetime — the raw
  // PTY bytes are fed to it verbatim, exactly like the prod frontend's per-pane Terminal).
  useEffect(() => {
    const term = new Terminal({
      convertEol: false,
      cursorBlink: true,
      disableStdin: false,
      fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
      fontSize: 11,
      scrollback: 4000,
      theme: TERM_THEME,
    });
    let fit = null;
    try { fit = new FitAddon(); term.loadAddon(fit); } catch { fit = null; }
    term.open(mountRef.current);
    try { fit && fit.fit(); } catch { /* mount not laid out yet */ }
    termRef.current = term;
    fitRef.current = fit;
    glRef.current.term = term;

    // Keystrokes → raw PTY input (no trailing newline; xterm already includes \r etc.).
    // Call through the ref so a later onInput (e.g. broadcast toggle) is seen without
    // re-creating the terminal (mount effect deliberately runs once per agent.id).
    const onDataDisposable = term.onData((data) => {
      const fn = onInputRef.current;
      if (fn) fn(agent.id, data);
    });

    // Sync the backend PTY winsize to the terminal so TUIs paint at the widget size.
    // Prod-parity fitSession guard (agent-teams main.js:667-675): only tell the PTY to
    // resize when rows/cols ACTUALLY changed — prevents a resize→SIGWINCH→repaint→resize
    // feedback loop, and keeps a seam drag from flooding resize_pty per pixel.
    //
    // lastRows/lastCols hold the dims the BACKEND ACKED, not the dims we last tried:
    // resize_pty races the multi-second spawn_workspace (the optimistic pane mounts
    // before the PTY registers → "no such workspace"), and the old code cached the
    // dims on a swallowed failure — leaving the PTY at its 30×100 spawn size FOREVER
    // (the change-guard blocked every later sync). On failure we now keep the guard
    // stale and retry on a timer until the PTY acks, so a pane that finishes spawning
    // gets its winsize without needing another geometry change.
    let lastRows = 0;   // last dims acked by resize_pty (0 = never synced)
    let lastCols = 0;
    let syncing = false;  // resize_pty in flight
    let pending = false;  // geometry changed while an invoke was in flight
    let retries = 0;      // autonomous retry budget (reset by fresh geometry events)
    let debTimer = 0;
    let retryTimer = 0;
    let disposed = false;
    const MAX_RETRIES = 40; // × 500ms ≈ 20s — covers a slow worktree-backed spawn

    const syncNow = () => {
      if (disposed || !term.element) return; // never fit() before term.open()
      // 0×0 / collapsed rect (display:none ws, transient layout): skip — the
      // ResizeObserver fires again when the pane gets a real box. Without this,
      // FitAddon can propose its 2×1 minimum and SIGWINCH the TUI into garbage.
      const box = mountRef.current && mountRef.current.getBoundingClientRect();
      if (!box || box.width < 2 || box.height < 2) return;
      try { fit && fit.fit(); } catch { return; }
      const rows = term.rows;
      const cols = term.cols;
      if (!rows || !cols) return;
      if (rows === lastRows && cols === lastCols) return; // PTY already matches
      if (syncing) { pending = true; return; }
      syncing = true;
      // Same latest-ref pattern as onData: onResize is bound once in this mount effect.
      Promise.resolve(onResizeRef.current && onResizeRef.current(agent.id, rows, cols))
        .then(() => { lastRows = rows; lastCols = cols; })
        .catch(() => {
          // Backend can't resize yet (pane still spawning / transient) — leave the
          // acked dims stale so the guard stays open, and retry until it lands.
          if (!disposed && retries < MAX_RETRIES) {
            retries += 1;
            clearTimeout(retryTimer);
            retryTimer = setTimeout(syncNow, 500);
          }
        })
        .finally(() => {
          syncing = false;
          if (pending && !disposed) { pending = false; syncNow(); }
        });
    };
    // Trailing ~80ms debounce: a live window drag / seam drag fires ResizeObserver
    // per frame; one fit + at most one resize_pty lands after the burst settles.
    const pushResize = () => {
      if (disposed) return;
      retries = 0; // a fresh geometry event re-arms the retry budget
      clearTimeout(debTimer);
      debTimer = setTimeout(syncNow, 80);
    };
    pushResizeRef.current = pushResize;
    pushResize();
    // Container geometry (tiling/seam drag/maximize/restore/cross-ws move all end up
    // resizing this element) + window resize as belt-and-braces for webview quirks.
    const ro = new ResizeObserver(pushResize);
    if (mountRef.current) ro.observe(mountRef.current);
    window.addEventListener("resize", pushResize);

    return () => {
      disposed = true;
      clearTimeout(debTimer);
      clearTimeout(retryTimer);
      window.removeEventListener("resize", pushResize);
      ro.disconnect();
      pushResizeRef.current = null;
      onDataDisposable.dispose();
      detachWebgl(glRef.current);
      glRef.current.term = null;
      term.dispose();
      termRef.current = null;
      fitRef.current = null;
      writtenRef.current = 0;
    };
    // agent.id is stable for a pane's lifetime; deliberately mount-once.
  }, [agent.id]);

  // Deterministic re-fit on layout-driven geometry changes (tiling mode switch,
  // maximize/restore, seam-drag commit, visibility flip). The ResizeObserver above
  // covers these too, but keying on the actual rect props costs nothing and keeps
  // the pane correct even if an observation is missed under WKWebView.
  const styleW = style ? style.width : undefined;
  const styleH = style ? style.height : undefined;
  useEffect(() => {
    if (pushResizeRef.current) pushResizeRef.current();
  }, [styleW, styleH, visible]);

  // WebGL on visible panes only (prod policy): attach when shown, free the GPU context on hide.
  useEffect(() => {
    if (visible) attachWebgl(glRef.current);
    else detachWebgl(glRef.current);
  }, [visible]);

  // Stream new bytes into the terminal. `agent.raw` is append-only; write only the
  // un-written tail. A bumped `agent.gen` means the backend replayed from a later base
  // (history gap) → reset and rewrite from the top.
  useEffect(() => {
    const term = termRef.current;
    if (!term) return;
    // Web-preview fallback: the mock bridge has no raw byte stream, so render its line
    // array as terminal text instead.
    const raw = agent.raw !== undefined ? agent.raw : (agent.output || []).join("\r\n");
    if ((agent.gen || 0) !== genRef.current) {
      genRef.current = agent.gen || 0;
      writtenRef.current = 0;
      term.reset();
    }
    if (raw.length > writtenRef.current) {
      term.write(raw.slice(writtenRef.current));
      writtenRef.current = raw.length;
    } else if (raw.length < writtenRef.current) {
      // buffer shrank without a gen bump (e.g. mock replaced output) — resync.
      term.reset();
      term.write(raw);
      writtenRef.current = raw.length;
    }
  }, [agent.raw, agent.gen, agent.output]);

  const border = selected
    ? "border-cyan-300 shadow-[0_0_16px_rgba(0,229,255,0.45)]"
    : agent.status === "needs_input"
    ? "border-amber-400/80 shadow-[0_0_14px_rgba(251,191,36,0.35)]"
    : "border-cyan-800/70 hover:border-cyan-500";

  return (
    <div
      id={`pane-${agent.id}`}
      data-pane-id={agent.id}
      style={style}
      onClick={() => onSelect(agent.id)}
      // Focus into the embedded xterm textarea must select this pane (BUG-1: ⌘⇧G uses
      // selectedId, which previously only updated on container click). Capture so focus
      // inside the terminal still bubbles here. Skip if already selected — onSelect is
      // idempotent from this side; avoids redundant parent work on every re-focus.
      onFocusCapture={() => { if (!selected) onSelect(agent.id); }}
      // `zoomed` is a styling/query hook only — the actual zoom geometry arrives as
      // the `style` rect from the tiling layer, which hands a zoomed pane the whole host box.
      className={`flex flex-col border cursor-pointer transition-all duration-200 bg-[#0A1219] overflow-hidden ${zoomed ? "zoomed " : ""}${border}`}
    >
      <div
        onPointerDown={(e) => onDragStart && onDragStart(agent.id, e)}
        className="flex items-center justify-between px-3 py-1.5 border-b border-cyan-800/70 bg-[#0C1720] cursor-grab active:cursor-grabbing select-none"
      >
        <div className="flex items-center gap-2.5 min-w-0">
          <button
            onClick={(e) => { e.stopPropagation(); onToggleCheck(agent.id); }}
            title="Select for bulk actions"
            className={`w-4 h-4 border flex items-center justify-center shrink-0 transition-colors ${
              checked ? "bg-cyan-300 border-cyan-300" : "border-cyan-700 hover:border-cyan-400"
            }`}
          >
            {checked && <span className="text-[#0A1219] text-[10px] font-bold leading-none">✓</span>}
          </button>
          {renaming ? (
            <input
              ref={renameInputRef}
              defaultValue={displayName}
              // The head drags on pointerdown and selects on click — keep both off the field.
              onPointerDown={(e) => e.stopPropagation()}
              onClick={(e) => e.stopPropagation()}
              onKeyDown={(e) => {
                // Only the keys handled here stop propagating: a blanket stop would swallow the
                // app's global shortcuts, and whether those fire while a text field has focus is
                // the shortcut owner's call, not this pane's.
                if (e.key === "Enter") { e.stopPropagation(); finishRename(e.currentTarget.value, true); }
                else if (e.key === "Escape") { e.stopPropagation(); finishRename(null, false); }
              }}
              onBlur={(e) => finishRename(e.currentTarget.value, true)}
              aria-label="Pane name"
              className="font-heading font-bold tracking-[0.15em] text-sm text-cyan-100 bg-[#0A1219] border border-cyan-500 outline-none px-1 py-0 min-w-0 w-32"
            />
          ) : (
            <span
              onDoubleClick={(e) => { e.stopPropagation(); beginRename(); }}
              title={labelOverride ? `${displayName} (renamed from ${defaultName})` : "Double-click to rename"}
              className="font-heading font-bold tracking-[0.15em] text-sm text-cyan-300 truncate"
            >
              {displayName}
            </span>
          )}
          <span className="font-mono text-[10px] text-cyan-600 tracking-wider truncate">{AGENT_KINDS[agent.kind]?.label} · {agent.id}</span>
          {agent.role && (
            <span className="font-mono text-[10px] text-cyan-700 tracking-wider uppercase truncate">[{agent.role}]</span>
          )}
        </div>
        <div className="flex items-center gap-1 shrink-0">
          <span className={`font-mono text-sm font-bold ${meta.color}`}>({meta.badge})</span>
          <button
            // stopPropagation, not preventDefault: the head's onPointerDown would otherwise read
            // this click as the start of a pane drag.
            onPointerDown={(e) => e.stopPropagation()}
            onClick={(e) => { e.stopPropagation(); onMaximize && onMaximize(agent.id); }}
            title={zoomed ? "Restore pane" : "Maximize pane"}
            aria-label={zoomed ? "Restore pane" : "Maximize pane"}
            aria-pressed={zoomed}
            className="w-4 h-5 flex items-center justify-center text-sm leading-none text-cyan-600 hover:text-cyan-300 hover:bg-cyan-300/10 transition-colors"
          >
            {zoomed ? "⤡" : "⤢"}
          </button>
          <PaneMenu hasBranch={!!agent.branch} workspaces={workspaces} onAction={handleMenuAction} />
        </div>
      </div>
      <div className="px-3 py-1 border-b border-cyan-900/50 font-mono text-[10px] text-cyan-700 truncate">
        ⎇ {agent.branch} · {agent.worktree}
      </div>
      <div
        ref={mountRef}
        onClick={(e) => e.stopPropagation()}
        className="crt-screen flex-1 min-h-0 overflow-hidden p-1.5 terminal-scroll"
      />
      {agent.status === "needs_input" && <AttentionPrompt agent={agent} onRespond={onRespond} />}
    </div>
  );
}
