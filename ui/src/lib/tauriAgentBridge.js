// TauriAgentBridge — real implementation of the agentBridge contract, wired to
// the jtmilan/agent-teams Rust backend (Tauri v2 commands in app/src-tauri/src/lib.rs).
//
// Activates automatically when this UI runs inside the Tauri shell
// (window.__TAURI__ present). In the hosted web preview the mock bridge is used.
//
// Command mapping (bridge method → Tauri command):
//   subscribe            → poll list_queue + read_output_delta_batch + dead_pane_ids
//   spawnAgents          → spawn_workspace(id, harness, repo, ...)
//   sendInput / delegate → send_input(id, data)
//   broadcast(To)        → send_input per pane
//   closeWorkspace       → close_workspace(id) per pane
//   restartAgents        → close_workspace + spawn_workspace
//
// Requires `app.withGlobalTauri: true` in tauri.conf.json (exposes window.__TAURI__).

import { randomAgentName } from "@/lib/agentNames";

// Prod parity: POLL_TICK_MS = 120 in agent-teams app/src/poll-core.js:7 — 500ms reads a beat
// behind a streaming TUI. Guarded against overlap in _poll (120ms can undercut a slow invoke).
const POLL_MS = 120;

// UI kind (agentTypes.js key) → backend harness wire string. Wire strings are the
// `descriptor().wire` values in agent-teams core/harness/src/lib.rs, parsed by
// `parse_harness` (app/src-tauri/src/lib.rs) — NOT the CLI command name (cursor's
// cmd is "cursor-agent" but its wire string is "cursor"). Unknown kinds fall
// back to "bash".
const HARNESS_WIRE = {
  "claude-code": "claude",
  cursor: "cursor",
  codex: "codex",
  opencode: "opencode",
  commandcode: "commandcode",
  cline: "cline",
  grok: "grok",
  bash: "bash",
};

// This UI collects no repo/folder in the spawn form, and the backend's
// default_folder() returns $HOME — which is not a git repo, so add_worktree
// degrades every pane to the bare home dir (no worktree isolation). Default
// instead to a DEDICATED, ISOLATED git repo so panes get real per-pane
// worktrees WITHOUT ever touching the agent-teams repo this app is built from
// (spawning into agent-teams itself lets the app's startup wipe / worktree reap
// collide with the repo under development). Overridable per-config via cfg.repo.
const DEFAULT_REPO = "/Users/jeffrymilan/agent-teams-devapp-workspace";

export function isTauri() {
  return typeof window !== "undefined" && !!window.__TAURI__;
}

const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);

const SPAWNED_KEY = "hr:spawned-panes";

function loadSpawned() {
  try {
    const v = JSON.parse(localStorage.getItem(SPAWNED_KEY) || "{}");
    return v && typeof v === "object" && !Array.isArray(v) ? v : {};
  } catch {
    return {};
  }
}

export class TauriAgentBridge {
  constructor() {
    this.agents = [];        // our UI agent shape, keyed by pane id
    this.offsets = {};       // pane id → last `next` cursor for read_output_delta_batch
    this.raw = {};           // pane id → accumulated RAW PTY bytes (fed verbatim to xterm)
    this.gen = {};           // pane id → generation; bumps on a `truncated` history gap → term reset
    this.spawned = loadSpawned(); // pane id → {kind, role} the adapter itself spawned; read set is
                             // the UNION of these and list_queue ids, so a state-blind pane
                             // (bash, grok, … — never writes events.jsonl → never enters
                             // list_queue) still streams.
                             // Persisted: a webview reload must not orphan live state-blind panes
                             // (their PTYs survive in the backend but list_queue never surfaces them).
    this.listeners = new Set();
    this.timer = null;
  }

  _saveSpawned() {
    try { localStorage.setItem(SPAWNED_KEY, JSON.stringify(this.spawned)); } catch { /* private mode */ }
  }

  subscribe(cb) {
    this.listeners.add(cb);
    cb(this.agents);
    return () => this.listeners.delete(cb);
  }

  _emit() {
    for (const cb of this.listeners) cb([...this.agents]);
  }

  start() {
    if (this.timer) return;
    this.timer = setInterval(() => this._poll(), POLL_MS);
  }

  async _poll() {
    // A single bad tick must NOT kill the whole poll loop — it runs on setInterval,
    // so an unhandled rejection would silently stop all UI updates. Isolate each tick.
    // Overlap guard: at 120ms a slow invoke chain can outlive the tick — skip, don't stack.
    if (this._polling) return;
    this._polling = true;
    try {
      await this._pollOnce();
    } catch (e) {
      console.error("[TauriAgentBridge] poll failed:", e);
    } finally {
      this._polling = false;
    }
  }

  async _pollOnce() {
    // 1. Ranked "who needs you" queue — authoritative status source.
    const queue = await invoke("list_queue");
    const dead = new Set(await invoke("dead_pane_ids"));
    const rowById = {};
    for (const row of queue) rowById[row.id] = row;

    // 2. Read output for the UNION of queue ids and adapter-spawned ids. `list_queue`
    //    only surfaces panes that have an on-disk events.jsonl, so a state-blind pane
    //    like bash or grok (no hook, excluded from the synthetic ready event) never
    //    appears there — reading only queue ids leaves it silent forever. Spawned ids
    //    close that gap (mirrors the prod frontend reading over its own session map,
    //    not the queue).
    const ids = Array.from(new Set([...queue.map((r) => r.id), ...Object.keys(this.spawned)]));
    const reqs = ids.map((id) => ({ id, since: this.offsets[id] || 0 }));
    const deltas = reqs.length ? await invoke("read_output_delta_batch", { reqs }) : [];
    // Ghost pruning: `spawned` persists across reloads, but a backend RESTART kills all
    // PTYs and wipes its registry — a persisted id the backend no longer knows returns
    // no delta entry and no queue row forever. Forget an id after ~20 consecutive
    // silent ticks (10s) with no queue row, no delta, and no dead-flag.
    const answered = new Set(deltas.map((d) => d.id));
    this._miss = this._miss || {};
    for (const id of Object.keys(this.spawned)) {
      if (rowById[id] || answered.has(id) || dead.has(id)) { delete this._miss[id]; continue; }
      this._miss[id] = (this._miss[id] || 0) + 1;
      if (this._miss[id] > 20) { this._forget(id); delete this._miss[id]; }
    }
    for (const d of deltas) {
      if (typeof d.data !== "string") continue;
      // `truncated` = the cursor fell behind the ring buffer's start: the byte window
      // was replayed from a later base, so the accumulated scrollback is stale. Reset
      // this pane's buffer and bump its generation so the terminal does a full RIS.
      if (d.truncated) {
        // Cap a huge replay too (backend retains 4MB) — the gen bump below already
        // resets the terminal, so starting mid-stream costs at most one cosmetic
        // fragment at the very top of scrollback.
        this.raw[d.id] = d.data.slice(-160000);
        this.gen[d.id] = (this.gen[d.id] || 0) + 1;
      } else {
        // The pane writes raw.slice(writtenRef) — an ABSOLUTE cursor into this
        // string. The old silent `.slice(-200000)` front-trim shifted the string
        // under that cursor: the first crossing dropped delta bytes mid-escape
        // (literal "245;48;5;233m" fragments on grok panes), and once pinned at
        // exactly the cap, raw.length === writtenRef → the pane FROZE (no new
        // bytes ever written; backend truncation at 4MB never fires because our
        // `since` cursor keeps advancing). Trim in big hysteresis steps WITH a
        // gen bump instead, so the pane does reset + full rewrite of the window.
        const grown = (this.raw[d.id] || "") + d.data;
        if (grown.length > 240000) {
          this.raw[d.id] = grown.slice(-120000);
          this.gen[d.id] = (this.gen[d.id] || 0) + 1;
        } else {
          this.raw[d.id] = grown;
        }
      }
      if (typeof d.next === "number") this.offsets[d.id] = d.next;
    }

    // 3. Build the agent list from the same union: every queue row, plus any spawned id
    //    not yet surfaced by the queue (rendered optimistically as "starting"/"working").
    this.agents = ids.map((id) => {
      const row = rowById[id];
      const prev = this.agents.find((a) => a.id === id);
      const sp = this.spawned[id];
      return {
        id,
        name: prev?.name || randomAgentName(),
        kind: (row && row.harness) || prev?.kind || sp?.kind || "bash",
        role: (row && row.role) || prev?.role || sp?.role,
        branch: prev?.branch || "",
        worktree: prev?.worktree || "",
        status: dead.has(id) ? "error" : row ? mapStatus(row) : prev?.status === "working" ? "working" : "starting",
        attention: row && row.needs_human
          ? { reason: row.reason || "Agent is waiting on you", since: row.since || Date.now() }
          : null,
        // Raw PTY bytes for the xterm terminal; `gen` triggers a reset on a history gap.
        raw: this.raw[id] || "",
        gen: this.gen[id] || 0,
      };
    });
    this._emit();
  }

  async spawnAgents(configs, _name) {
    // Backend groups panes into a workspace by right-splitting the id on "-p"
    // (wsNNNNNxK-pN shape) — one workspace id per spawnAgents call.
    const ws = "ws" + String(Math.floor(10000 + Math.random() * 90000)) + "x0";
    for (const [index, cfg] of configs.entries()) {
      const id = `${ws}-p${index}`;
      // Register the id up front so its output is read from the next tick (before the
      // pane ever enters list_queue) and an optimistic "starting" pane renders now.
      this.spawned[id] = { kind: cfg.kind, role: cfg.role };
      this.offsets[id] = 0;
      try {
        await invoke("spawn_workspace", {
          id,
          harness: HARNESS_WIRE[cfg.kind] || "bash",
          repo: cfg.repo || DEFAULT_REPO,
          role: cfg.role,
        });
        if (cfg.role) await invoke("set_pane_roles", { roles: [[id, cfg.role]] });
      } catch (e) {
        // A failed spawn must be visible, not a silent unhandled rejection.
        delete this.spawned[id];
        this.raw[id] = "\r\n\x1b[31m[spawn failed] " + String(e && e.message ? e.message : e) + "\x1b[0m\r\n";
        console.error("[TauriAgentBridge] spawn_workspace failed:", id, e);
      }
    }
    this._saveSpawned();
    this._poll();
  }

  _forget(id) {
    delete this.spawned[id];
    delete this.offsets[id];
    delete this.raw[id];
    delete this.gen[id];
    this._saveSpawned();
  }

  sendInput(id, text) { return invoke("send_input", { id, data: text + "\n" }); }
  // Raw keystrokes from the xterm terminal — sent verbatim (NO trailing newline; the
  // terminal already includes \r etc.). Distinct from sendInput's line-submit path.
  sendRaw(id, data) { return invoke("send_input", { id, data }); }
  // NO .catch here: the rejection is the AgentPane's signal to keep its "acked
  // dims" guard open and retry — resize_pty races the multi-second spawn_workspace
  // (optimistic pane mounts first → "no such workspace"), and swallowing that left
  // the PTY at its 30×100 spawn size forever while xterm re-fit without it.
  resizePane(id, rows, cols) { return invoke("resize_pty", { id, rows, cols }); }
  delegate(id, task) { return this.sendInput(id, task); }
  broadcast(text) { return Promise.all(this.agents.map((a) => this.sendInput(a.id, text))); }
  broadcastTo(ids, text) { return Promise.all(ids.map((id) => this.sendInput(id, text))); }

  pauseAgents(ids) { return Promise.all(ids.map((id) => invoke("pause_pane", { id }))); }
  async restartAgents(ids) {
    for (const id of ids) { await invoke("close_workspace", { id }); this._forget(id); }
  }
  resumeAll() { return Promise.all(this.agents.map((a) => invoke("resume_pane", { id: a.id }))); }
  stopAll() { return this.closeWorkspace(); }
  advanceStarting() {}
  setRunning(_r) {}
  loadDemoFleet() {}

  async closeWorkspace() {
    for (const a of this.agents) await invoke("close_workspace", { id: a.id });
    this.agents = [];
    this.offsets = {};
    this.raw = {};
    this.gen = {};
    this.spawned = {};
    this._saveSpawned();
    this._emit();
  }
}

function mapStatus(row) {
  // QueueRow: state ∈ idle|working|waiting|done|error, needs_human: bool.
  if (row.needs_human) return "needs_input";
  if (row.state === "error") return "error";
  if (row.state === "working") return "working";
  return "idle"; // idle | waiting | done without needs_human
}