import React, { useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { STATUS_META } from "@/lib/agentData";
import { AGENT_KINDS } from "@/lib/agentTypes";
import AttentionPrompt from "@/components/command/AttentionPrompt";

// ---- renderer policy: WebGL2 on VISIBLE panes only (ported from agent-teams main.js:677-760).
// The DOM renderer rebuilds each dirty row's span-DOM per frame — N streaming TUIs ≈ 10-30k node
// churn/s. WebGL moves that to GPU draws. WKWebView caps live WebGL contexts per page (~16; the
// OLDEST context gets context-lost when exceeded), so attach ONLY to visible panes and dispose on
// hide. Hidden panes fall back to the DOM renderer (xterm 6 render-pauses them). Kill-switch:
// localStorage at_no_webgl="1" → never attach (field rollback without a rebuild).
const MAX_WEBGL = 8;
let webglDisabled = (() => { try { return localStorage.getItem("at_no_webgl") === "1"; } catch { return false; } })();
let webglLive = 0; // live attached contexts (visible panes only) — kept ≤ MAX_WEBGL
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
  if (webglDisabled || !s || s.webgl || s.webglBlocked || !s.term || webglLive >= MAX_WEBGL) return;
  let WebglAddon;
  try {
    WebglAddon = await loadWebglCtor();
  } catch {
    s.webglBlocked = true; // module load failed — DOM renderer stays; a later pane can retry the import
    return;
  }
  if (webglDisabled || s.webgl || s.webglBlocked || !s.term || webglLive >= MAX_WEBGL) return;
  try {
    const addon = new WebglAddon();
    addon.onContextLoss(() => {
      // GPU context lost (OOM / suspend / cap eviction): dispose restores the DOM renderer.
      // PERMANENT fallback for THIS pane — re-attaching on a pressured GPU just flaps.
      try { addon.dispose(); } catch { /* already disposed */ }
      if (s.webgl) { s.webgl = null; webglLive = Math.max(0, webglLive - 1); }
      s.webglBlocked = true;
    });
    s.term.loadAddon(addon); // throws when WebGL2 is unavailable in this webview
    s.webgl = addon;
    webglLive += 1;
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
  if (!s || !s.webgl) return;
  try { s.webgl.dispose(); } catch { /* already disposed */ }
  s.webgl = null;
  webglLive = Math.max(0, webglLive - 1);
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

export default function AgentPane({ agent, selected, checked, onToggleCheck, onSelect, onRespond, onInput, onResize, style, onDragStart, visible = true }) {
  const meta = STATUS_META[agent.status];
  const mountRef = useRef(null);
  const termRef = useRef(null);
  const fitRef = useRef(null);
  const writtenRef = useRef(0); // bytes of agent.raw already written to the terminal
  const genRef = useRef(agent.gen || 0);
  // Per-pane renderer state for the WebGL policy (term set at mount; webgl managed by visibility).
  const glRef = useRef({ term: null, webgl: null, webglBlocked: false });

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
    const onDataDisposable = term.onData((data) => onInput && onInput(agent.id, data));

    // Sync the backend PTY winsize to the terminal so TUIs paint at the widget size.
    // Prod-parity fitSession guard (agent-teams main.js:667-675): only tell the PTY to
    // resize when rows/cols ACTUALLY changed — prevents a resize→SIGWINCH→repaint→resize
    // feedback loop, and keeps a seam drag from flooding resize_pty per pixel. The fit
    // itself is rAF-coalesced so a burst of ResizeObserver ticks costs one reflow.
    let lastRows = 0;
    let lastCols = 0;
    let fitRaf = 0;
    const pushResize = () => {
      if (fitRaf) return;
      fitRaf = requestAnimationFrame(() => {
        fitRaf = 0;
        try { fit && fit.fit(); } catch { return; }
        if (!term.rows || !term.cols) return;
        if (term.rows === lastRows && term.cols === lastCols) return;
        lastRows = term.rows;
        lastCols = term.cols;
        if (onResize) onResize(agent.id, term.rows, term.cols);
      });
    };
    pushResize();
    const ro = new ResizeObserver(pushResize);
    if (mountRef.current) ro.observe(mountRef.current);

    return () => {
      if (fitRaf) cancelAnimationFrame(fitRaf);
      ro.disconnect();
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
      className={`flex flex-col border cursor-pointer transition-all duration-200 bg-[#0A1219] overflow-hidden ${border}`}
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
          <span className="font-heading font-bold tracking-[0.15em] text-sm text-cyan-300">{agent.name || agent.id}</span>
          <span className="font-mono text-[10px] text-cyan-600 tracking-wider truncate">{AGENT_KINDS[agent.kind]?.label} · {agent.id}</span>
          {agent.role && (
            <span className="font-mono text-[10px] text-cyan-700 tracking-wider uppercase truncate">[{agent.role}]</span>
          )}
        </div>
        <span className={`font-mono text-sm font-bold shrink-0 ${meta.color}`}>({meta.badge})</span>
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
