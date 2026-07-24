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
//   broadcastRaw         → send_input per LIVE pane, verbatim (broadcast toggle mode)
//   closeWorkspace       → close_workspace(id) per pane
//   restartAgents        → close_workspace + spawn_workspace
//
// Requires `app.withGlobalTauri: true` in tauri.conf.json (exposes window.__TAURI__).

import { randomAgentName } from "@/lib/agentNames";
import { reconcilePaneLabels } from "@/lib/paneLabels";
import { assignMany, unassign } from "@/lib/workspaceAssign";
import { toast } from "@/components/ui/use-toast";

// Prod parity: POLL_TICK_MS = 120 in agent-teams app/src/poll-core.js:7 — 500ms reads a beat
// behind a streaming TUI. Guarded against overlap in _poll (120ms can undercut a slow invoke).
const POLL_MS = 120;

/**
 * After a byte-window slice, advance to a safe replay start so term.reset()+write never
 * begins mid-CSI/SGR (which leaves the emulator with broken DECAWM/SGR → 1-col wrap).
 * 1) Drop through the first newline (discard the likely-truncated first line).
 * 2) If still opening on ESC, skip past a complete CSI/simple ESC or drop a lone incomplete ESC.
 */
export function trimAtSafeBoundary(s) {
  if (!s) return s;
  const nl = s.indexOf("\n");
  if (nl !== -1) s = s.slice(nl + 1);
  // Partial ESC at the head (slice landed inside an escape, or no newline was present).
  if (s.charCodeAt(0) === 0x1b /* ESC */) {
    let i = 1;
    if (i < s.length && s[i] === "[") {
      // CSI: ESC [ intermediate/params… final byte in 0x40–0x7E
      i += 1;
      while (i < s.length) {
        const c = s.charCodeAt(i);
        i += 1;
        if (c >= 0x40 && c <= 0x7e) break;
      }
      // Incomplete CSI (no final byte in the window): drop the orphan prefix entirely.
      s = s.slice(i);
    } else if (i < s.length) {
      s = s.slice(2); // ESC + single-char sequence
    } else {
      s = ""; // lone ESC with nothing after
    }
  }
  return s;
}

// UI kind (agentTypes.js key) → backend harness wire string. Wire strings are the
// `descriptor().wire` values in agent-teams core/harness/src/lib.rs, parsed by
// `parse_harness` (app/src-tauri/src/lib.rs) — NOT the CLI command name (cursor's
// cmd is "cursor-agent" but its wire string is "cursor"). Unknown kinds are
// REFUSED (see harnessWireOf) — never silently remapped to "bash".
const HARNESS_WIRE = {
  "claude-code": "claude",
  cursor: "cursor",
  codex: "codex",
  opencode: "opencode",
  commandcode: "commandcode",
  pi: "pi",
  grok: "grok",
  bash: "bash",
};

// UI kind keys AND queue-row wire strings both appear on agent.kind (poll prefers
// row.harness once list_queue surfaces the pane). Map either form to the wire
// string spawn_workspace expects; refuse unknown values rather than defaulting
// to "bash" (a wrong harness is worse than a refused restart).
function harnessWireOf(kind) {
  if (!kind || typeof kind !== "string") return null;
  if (Object.prototype.hasOwnProperty.call(HARNESS_WIRE, kind)) return HARNESS_WIRE[kind];
  for (const wire of Object.values(HARNESS_WIRE)) {
    if (wire === kind) return wire;
  }
  return null;
}

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

// A send_input reject that means "this pane's PTY is gone" (vs. a transient error).
// Ported verbatim from agent-teams app/src/main.js:82.
const DEAD_RE = /no longer alive|no such workspace|not alive/i;

// Backend cross-workspace isolation reject (phase 1): any send/broadcast/handoff
// that targets a pane outside the caller's workspace is refused with a string
// starting "CROSS_WORKSPACE:". Surface it loudly (toast + re-throw) instead of
// swallowing — the operator must see the boundary, not wonder why a keystroke
// went silent.
function isCrossWs(err) {
  return /CROSS_WORKSPACE/i.test(String(err && err.message ? err.message : err));
}

function toastCrossWs(e) {
  toast({
    title: "Blocked: cross-workspace",
    description: String(e && e.message ? e.message : e).slice(0, 240),
    variant: "destructive",
  });
}

// Backend sharing-state fetch (phase 1): per-ws allow_sharing map. Empty object
// on failure / old backend without the command — the UI then renders every ws
// as isolated, which is the safe default.
async function fetchSharingStates() {
  try {
    const m = await invoke("get_workspace_sharing_states");
    return m && typeof m === "object" ? m : {};
  } catch {
    return {};
  }
}

// Terminal REPLY traffic. xterm emits these through the SAME onData channel as
// keystrokes, but UNPROMPTED — no user key is behind them: SGR mouse reports
// (\x1b[<…M/m), focus in/out (\x1b[I / \x1b[O), and OSC query replies (\x1b]11;rgb:…,
// the background-colour probe). A reply belongs ONLY to the pane whose app asked for
// it; fanning one out types it as garbage into every sibling's input line (live-fired
// on a prod commandcode pane: "[<35;28;24M…]11;rgb:0a0a/…"). Arrow and function keys
// (\x1b[A, \x1bOP, …) do NOT match, and so still fan out.
// Ported from agent-teams app/src/main.js:636.
export function isReplyTraffic(data) {
  return /^(?:\x1b\[<|\x1b\[[IO]$|\x1b\])/.test(data);
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
    this.capacity = null;    // { max, working } from get_capacity (1s poll); null until first read
    this._capTimer = null;
    this._capDisabled = false; // backend lacks get_capacity (old build) → stop polling
    this.sharing = {};       // wsId → bool from get_workspace_sharing_states (1.5s poll); {} until first read
    this._shareTimer = null;
    this._shareDisabled = false; // backend lacks get_workspace_sharing_states (old build) → stop polling
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
    this.dead = new Set();      // pane ids known dead: backend truth ∪ deadLocal. Read by broadcastRaw.
    this.deadLocal = new Set(); // deaths a keystroke reject taught us BEFORE dead_pane_ids caught up;
                                // unioned in every poll so the next tick can't clobber them.
    this._deadBurst = new Set();
    this._deadBurstTimer = null;
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
    // Capacity (max + working) drives the HUD "N / max" + the NEW AGENT / TEMPLATES gate.
    // Polled on its own 1s timer (NOT the 120ms hot poll — working_count scans state files),
    // and folded into the normal _emit so the existing subscribe→render path picks it up.
    if (!this._capTimer && !this._capDisabled) {
      const tick = async () => {
        if (this._capDisabled) return;
        try {
          const c = await invoke("get_capacity");
          if (c && (this.capacity?.max !== c.max || this.capacity?.working !== c.working)) {
            this.capacity = c;
            this._emit();
          }
        } catch {
          this._capDisabled = true; // backend without get_capacity — don't spam
        }
      };
      tick();
      this._capTimer = setInterval(tick, 1000);
    }
    // Sharing state (per-ws allow_sharing) — mirrored from the backend on a 1.5s timer
    // (same shape as the capacity poll, NOT the 120ms hot loop). Backend is authoritative;
    // localStorage never persists this. Folded into _emit so subscribers re-render on change.
    if (!this._shareTimer && !this._shareDisabled) {
      const shareTick = async () => {
        if (this._shareDisabled) return;
        const m = await fetchSharingStates();
        // Cheap equality: only re-emit when a key changed (avoids no-op renders every 1.5s).
        const prev = this.sharing;
        const prevKeys = Object.keys(prev);
        const nextKeys = Object.keys(m);
        const changed =
          prevKeys.length !== nextKeys.length ||
          nextKeys.some((k) => !!prev[k] !== !!m[k]);
        if (changed) {
          this.sharing = m;
          this._emit();
        }
      };
      shareTick();
      this._shareTimer = setInterval(shareTick, 1500);
    }
  }

  // Latest admission cap + working count (null until the first get_capacity resolves).
  getCapacity() {
    return this.capacity;
  }

  // Latest backend sharing map (wsId → bool). Empty until the first fetch resolves; the
  // UI treats every ws as isolated in that window (safe default).
  getSharing() {
    return this.sharing;
  }

  // Toggle a workspace's allow_sharing flag on the backend. Resolves to the new value
  // the backend persisted; rejects with a string on failure. The caller is responsible for
  // optimistic UI + rollback on reject.
  async setWorkspaceSharing(wsId, enabled) {
    return await invoke("set_workspace_sharing", { ws_id: wsId, enabled });
  }

  // One-shot sharing-state fetch (the timer-driven mirror is fetchSharingStates at
  // module scope). Exposed so Home can poll on its own cadence and merge into the
  // workspaces[] shape without waiting for the bridge's next _emit.
  async fetchSharingStates() {
    return fetchSharingStates();
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
    // Backend truth, unioned with deaths a broadcast keystroke already proved (its reject
    // lands up to a tick before dead_pane_ids agrees — see _noteDeadPane). Republished on
    // `this` so broadcastRaw can skip corpses BETWEEN ticks, not just at poll time.
    const dead = new Set(await invoke("dead_pane_ids"));
    for (const id of this.deadLocal) dead.add(id);
    this.dead = dead;
    const rowById = {};
    for (const row of queue) rowById[row.id] = row;

    // 2. Read output for the UNION of queue ids and adapter-spawned ids. `list_queue`
    //    only surfaces panes that have an on-disk events.jsonl, so a state-blind pane
    //    like bash or grok (no hook, excluded from the synthetic ready event) never
    //    appears there — reading only queue ids leaves it silent forever. Spawned ids
    //    close that gap (mirrors the prod frontend reading over its own session map,
    //    not the queue).
    const ids = Array.from(new Set([...queue.map((r) => r.id), ...Object.keys(this.spawned)]));
    // Empty-raw recovery (OpenCode blank+(W) class): if a live queue pane has never
    // accumulated raw bytes, force since=0 so a desynced offset / omitted batch entry
    // cannot strand the UI on a permanent empty buffer while the PTY is full.
    for (const id of ids) {
      if (dead.has(id)) continue;
      if ((this.raw[id] || "").length > 0) continue;
      if (!rowById[id] && !this.spawned[id]) continue;
      this.offsets[id] = 0;
    }
    const reqs = ids.map((id) => ({ id, since: this.offsets[id] || 0 }));
    let deltas = reqs.length ? await invoke("read_output_delta_batch", { reqs }) : [];
    if (!Array.isArray(deltas)) deltas = [];
    // Ghost pruning: `spawned` persists across reloads, but a backend RESTART kills all
    // PTYs and wipes its registry — a persisted id the backend no longer knows returns
    // no delta entry and no queue row forever. Forget an id after ~20 consecutive
    // silent ticks (10s) with no queue row, no delta, and no dead-flag.
    const answered = new Set(deltas.map((d) => d && d.id).filter(Boolean));
    // Batch may omit a pane (try_lock skip / race). Fall back to single-pane delta so
    // OpenCode/OpenTUI never stays 0b forever while Working in the queue.
    for (const id of ids) {
      if (answered.has(id) || dead.has(id)) continue;
      if ((this.raw[id] || "").length > 0) continue;
      try {
        const d = await invoke("read_output_delta", { id, since: this.offsets[id] || 0 });
        if (d && typeof d === "object") {
          deltas.push({ id, ...d });
          answered.add(id);
        }
      } catch {
        /* pane gone / transient — next tick */
      }
    }
    this._miss = this._miss || {};
    for (const id of Object.keys(this.spawned)) {
      if (rowById[id] || answered.has(id) || dead.has(id)) { delete this._miss[id]; continue; }
      this._miss[id] = (this._miss[id] || 0) + 1;
      if (this._miss[id] > 20) { this._forget(id); delete this._miss[id]; }
    }
    for (const d of deltas) {
      if (!d || typeof d.data !== "string") continue;
      // `truncated` = the cursor fell behind the ring buffer's start: the byte window
      // was replayed from a later base, so the accumulated scrollback is stale. Reset
      // this pane's buffer and bump its generation so the terminal does a full RIS.
      if (d.truncated) {
        // Genuine backend ring-buffer eviction: replay from a later base, so the
        // accumulated scrollback is stale. Cap the window, land on a safe boundary,
        // and bump gen so the terminal does a full RIS + rewrite.
        this.raw[d.id] = trimAtSafeBoundary(d.data.slice(-160000));
        this.gen[d.id] = (this.gen[d.id] || 0) + 1;
      } else {
        // Append-only. Do NOT front-trim + gen-bump here: term.reset() is RIS and
        // wipes alt-screen / DECAWM / DECSTBM the TUI set up (ESC[?1049h, ESC[?7l,
        // …), so replaying a full-width UI into a mode-reset terminal garbles into
        // a single column. Memory is bounded by the backend RETAIN_CAP (4MB) and
        // xterm scrollback:4000 — genuine eviction arrives as d.truncated above.
        this.raw[d.id] = (this.raw[d.id] || "") + d.data;
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
    // Pane-label GC: drop hr:pane-labels keys for ids no longer in the live union.
    // Site is a *successful* _pollOnce after list_queue + dead_pane_ids + spawned
    // ghost-prune — the same set that just rebuilt `this.agents`. Not on mount, not
    // on a failed tick (those never reach here). Recompute after _forget so a ghost
    // pruned this tick is already absent.
    // Empty-list path: reconcilePaneLabels no-ops when live.size === 0 (boot / full
    // closeWorkspace / queue hiccup with empty spawned) — never wipe every custom label.
    reconcilePaneLabels(
      Array.from(new Set([...queue.map((r) => r.id), ...Object.keys(this.spawned)])),
    );
    this._emit();
  }

  // Returns `{ wsId, paneIds }` — paneIds are minteds whose spawn_workspace RESOLVED,
  // in config order. Failed / refused spawns are excluded (red raw + console.error).
  //
  // opts.assignTo (optional ws id): pin every mappable id into that workspace BEFORE the
  // first optimistic registration / poll tick can render them. Sequential spawn_workspace
  // takes seconds; without early assign, unassigned panes land in the default bucket and
  // squeeze the active grid to 1–2-col PTYs (claude scrollback garbage; opencode blank).
  // Home template path pre-mints a tab + passes assignTo + wsId so both match.
  //
  // opts.wsId (optional): reuse a caller-minted backend workspace prefix instead of
  // minting a new one (template path creates the UI tab first with this id).
  async spawnAgents(configs, _name, { assignTo, wsId: wsIdOpt } = {}) {
    // Backend groups panes into a workspace by right-splitting the id on "-p"
    // (wsNNNNNxK-pN shape) — one workspace id per spawnAgents call.
    const ws =
      typeof wsIdOpt === "string" && wsIdOpt
        ? wsIdOpt
        : "ws" + String(Math.floor(10000 + Math.random() * 90000)) + "x0";
    // Mint ALL ids up front (same wsNNNNNx0-pN scheme as before) so assignMany can pin
    // the whole batch before any per-config invoke.
    const plan = configs.map((cfg, index) => {
      const id = `${ws}-p${index}`;
      return {
        id,
        cfg,
        repo: cfg.repo || DEFAULT_REPO,
        harness: harnessWireOf(cfg.kind),
      };
    });
    // Assign only mappable kinds — refused configs never register and must not orphan.
    if (assignTo) {
      const assignable = plan.filter((p) => p.harness).map((p) => p.id);
      if (assignable.length) assignMany(assignable, assignTo);
    }
    /** @type {string[]} */
    const minted = [];
    for (const { id, cfg, repo, harness } of plan) {
      if (!harness) {
        // K4: unmapped kind = REFUSED loudly. Never HARNESS_WIRE[kind] || "bash".
        // Match restartAgents refused-harness tone: red raw + console.error, no ghost
        // registration, excluded from returned ids. (Not in assignable → no unassign.)
        const msg = "[spawn refused] unmapped kind " + String(cfg.kind) + " for " + id;
        console.error("[TauriAgentBridge]", msg);
        this.raw[id] = "\r\n\x1b[31m" + msg + "\x1b[0m\r\n";
        continue;
      }
      // Register the id up front so its output is read from the next tick (before the
      // pane ever enters list_queue) and an optimistic "starting" pane renders now.
      this.spawned[id] = { kind: cfg.kind, role: cfg.role, repo };
      this.offsets[id] = 0;
      try {
        // Capture the result: SpawnResult.queued means the global concurrent-pane cap
        // (max_concurrent) parked this spawn instead of running it. The pane stays
        // optimistically registered (renders "starting") but the operator MUST see why a
        // workspace looks stuck — historically this Ok was discarded and the pane silently
        // never appeared (the "3rd+ workspace is blank" bug).
        const res = await invoke("spawn_workspace", {
          id,
          harness,
          repo,
          role: cfg.role,
        });
        if (res && res.queued) {
          toast({
            title: "Pane queued — at the agent cap",
            description:
              `${id} is #${res.position} in line (cap ${res.max}). ` +
              "It starts when a pane frees up. Close idle panes or raise the cap (Scheduler) to run more.",
            variant: "destructive",
          });
        }
        if (cfg.role) await invoke("set_pane_roles", { roles: [[id, cfg.role]] });
        minted.push(id);
      } catch (e) {
        // A failed spawn must be visible, not a silent unhandled rejection.
        // Drop ghost registration AND any pre-assign so the id does not stick on a ws.
        delete this.spawned[id];
        if (assignTo) unassign(id);
        const errMsg = String(e && e.message ? e.message : e);
        this.raw[id] = "\r\n\x1b[31m[spawn failed] " + errMsg + "\x1b[0m\r\n";
        console.error("[TauriAgentBridge] spawn_workspace failed:", id, e);
        // The raw line above has no terminal to render into once the id is unregistered,
        // so also surface it as a toast — otherwise the failure is console-only.
        toast({
          title: "Agent failed to spawn",
          description: `${id}: ${errMsg}`,
          variant: "destructive",
        });
      }
    }
    this._saveSpawned();
    this._poll();
    return { wsId: ws, paneIds: minted };
  }

  _forget(id) {
    delete this.spawned[id];
    delete this.offsets[id];
    delete this.raw[id];
    delete this.gen[id];
    this.dead.delete(id);
    this.deadLocal.delete(id); // else a forgotten id is re-unioned as dead on every poll, forever
    this._saveSpawned();
  }

  // Early-death auto-respawn (backend arm_early_death_respawn): same pane id, brand-new PTY
  // (OpenCode/Bun OpenTUI startup segfault class). Reset delta cursor + scrollback + gen so
  // AgentPane RIS-rewrites; clear dead flags. Does NOT bypass the sized-gate.
  onEarlyRespawn(id) {
    if (!id) return;
    this.offsets[id] = 0;
    this.raw[id] = "";
    this.gen[id] = (this.gen[id] || 0) + 1;
    this.dead.delete(id);
    this.deadLocal.delete(id);
    this._poll();
  }

  sendInput(id, text) {
    return invoke("send_input", { id, data: text + "\n" }).catch((e) => {
      if (isCrossWs(e)) { toastCrossWs(e); }
      throw e;
    });
  }
  // Raw keystrokes from the xterm terminal — sent verbatim (NO trailing newline; the
  // terminal already includes \r etc.). Distinct from sendInput's line-submit path.
  sendRaw(id, data) {
    return invoke("send_input", { id, data }).catch((e) => {
      if (isCrossWs(e)) { toastCrossWs(e); }
      throw e;
    });
  }
  // NO .catch here: the rejection is the AgentPane's signal to keep its "acked
  // dims" guard open and retry — resize_pty races the multi-second spawn_workspace
  // (optimistic pane mounts first → "no such workspace"), and swallowing that left
  // the PTY at its 30×100 spawn size forever while xterm re-fit without it.
  resizePane(id, rows, cols) { return invoke("resize_pty", { id, rows, cols }); }
  delegate(id, task) {
    return this.sendInput(id, task).catch((e) => {
      if (isCrossWs(e)) toastCrossWs(e);
      throw e;
    });
  }
  broadcast(text) {
    return Promise.all(this.agents.map((a) =>
      this.sendInput(a.id, text).catch((e) => {
        if (isCrossWs(e)) toastCrossWs(e);
        throw e;
      }),
    ));
  }
  broadcastTo(ids, text) {
    return Promise.all(ids.map((id) =>
      this.sendInput(id, text).catch((e) => {
        if (isCrossWs(e)) toastCrossWs(e);
        throw e;
      }),
    ));
  }

  // Broadcast TOGGLE mode: one keystroke → every LIVE pane, verbatim (sendRaw's payload
  // shape — NO trailing "\n"; the terminal's own bytes already carry \r). Distinct from
  // broadcast(text), which is the one-shot line-submit path.
  //
  // The CALLER must keep terminal reply traffic out of here — it belongs to the focused
  // pane alone. The seam reads:
  //   broadcast && !isReplyTraffic(data) ? bridge.broadcastRaw(data) : bridge.sendRaw(id, data)
  // broadcastRaw stays deliberately dumb about it: a caller that hands it reply traffic
  // should see it fan out, not vanish silently into a filter it did not ask for.
  broadcastRaw(data) {
    const targets = this.agents.filter((a) => !this.dead.has(a.id));
    return Promise.all(targets.map((a) =>
      // A pane that died since the last keystroke rejects here — that reject IS the news.
      // Swallow it (an unhandled rejection per keystroke per corpse is noise) and let
      // _noteDeadPane both drop it from the rest of the burst and light its error state.
      this.sendRaw(a.id, data).catch((e) => {
        // Cross-workspace reject beats the dead-pane regex: isolation is a policy
        // boundary the operator MUST see, not a silent PTY death. Toast + log distinct
        // (do NOT silently swallow — the dead path would otherwise hide it).
        if (isCrossWs(e)) {
          toastCrossWs(e);
          console.warn("[TauriAgentBridge] broadcast cross-workspace blocked:", a.id);
          return;
        }
        if (DEAD_RE.test(String(e))) this._noteDeadPane(a.id);
        else console.error("[TauriAgentBridge] broadcast send_input failed:", a.id, e);
      })
    ));
  }

  // ONE pane died. Mark it so the rest of the burst skips it, flip its status through the
  // EXISTING subscribe channel (same red error state the poll's dead_pane_ids drives, just
  // without waiting for the next tick), and COALESCE the burst: a broadcast fires this once
  // per corpse per keystroke, so a message per reject would be N×keystrokes of noise.
  // Mirrors agent-teams main.js:7508 — which debounces a "N panes dead, skipped" toast.
  _noteDeadPane(id) {
    if (this.dead.has(id)) return;
    this.dead.add(id);
    this.deadLocal.add(id);
    this._deadBurst.add(id);
    // REPLACE the row, don't mutate it: _pollOnce rebuilds agents as fresh objects every
    // tick, so consumers may compare by identity — an in-place status flip could be missed.
    const i = this.agents.findIndex((a) => a.id === id);
    if (i !== -1) { this.agents[i] = { ...this.agents[i], status: "error" }; this._emit(); }
    clearTimeout(this._deadBurstTimer);
    this._deadBurstTimer = setTimeout(() => {
      const n = this._deadBurst.size;
      this._deadBurst = new Set();
      if (n) console.warn(`[TauriAgentBridge] ${n} pane${n === 1 ? "" : "s"} dead, skipped`);
    }, 400);
  }

  pauseAgents(ids) { return Promise.all(ids.map((id) => invoke("pause_pane", { id }))); }

  // Per-pane SIGCONT — the counterpart to pauseAgents, and the ONLY way out of a
  // SIGSTOP'd pane.
  //
  // STATELESS BY DESIGN. Nothing records that a pane is paused: `pause_pane`/
  // `resume_pane` are bare fire-and-forget signals, and no `paused` field exists on
  // QueueRow or anywhere in core/mcp (verified). So the UI *cannot* ask whether a pane
  // is stopped — and it does not need to. SIGCONT on an already-RUNNING child is
  // harmless (verified on darwin: the child keeps running; a child that installs a
  // SIGCONT handler merely runs it — e.g. a TUI repaint — and survives). RESUME is
  // therefore offered unconditionally and is always safe to press.
  //
  // This is precisely what keeps a pane paused BEFORE a page reload resumable: there is
  // no client-side paused-set to lose on remount, so no state can drift out of sync
  // with the real process.
  //
  // allSettled, not all: a selection that mixes live and exited panes is ordinary
  // (dead panes linger in the list as status "error"), and signal_pane Errs on an
  // exited child. One such expected failure must not surface as an unhandled rejection
  // — the live panes still resume regardless.
  resumeAgents(ids) {
    return Promise.allSettled(ids.map((id) => invoke("resume_pane", { id }))).then((rs) => {
      const failed = rs.filter((r) => r.status === "rejected");
      if (failed.length) {
        console.warn(
          `[TauriAgentBridge] resume: ${failed.length}/${ids.length} pane(s) could not be signaled`,
          failed.map((r) => String(r.reason && r.reason.message ? r.reason.message : r.reason)),
        );
      }
    });
  }

  // close_workspace then spawn_workspace with the SAME id. Capture harness/repo/role
  // BEFORE close — `_forget` and the agent-list rebuild wipe them. There is no
  // backend `restart_workspace` command (verified: only spawn_workspace / close_workspace
  // exist on the Tauri surface this bridge uses).
  async restartAgents(ids) {
    for (const id of ids) {
      const agent = this.agents.find((a) => a.id === id);
      const sp = this.spawned[id];
      // Capture identity before any destructive step.
      const kind = (sp && sp.kind) || (agent && agent.kind) || null;
      const role = sp && "role" in sp ? sp.role : agent && agent.role;
      // Repo never lives on the UI agent object. Prefer the value spawnAgents
      // persisted; else DEFAULT_REPO — this UI collects no folder in the spawn
      // form, so that is the historical spawn path for every bridge-owned pane.
      const repo = (sp && sp.repo) || DEFAULT_REPO;
      const harness = harnessWireOf(kind);

      if (!harness) {
        // Wrong harness is worse than a refused restart — surface and skip.
        const msg = "[restart refused] cannot recover harness for " + id
          + " (kind=" + String(kind) + ")";
        console.error("[TauriAgentBridge]", msg);
        this.raw[id] = (this.raw[id] || "") + "\r\n\x1b[31m" + msg + "\x1b[0m\r\n";
        const ri = this.agents.findIndex((a) => a.id === id);
        if (ri !== -1) {
          this.agents[ri] = { ...this.agents[ri], status: "error", raw: this.raw[id] };
          this._emit();
        }
        continue;
      }

      try {
        await invoke("close_workspace", { id });
      } catch (e) {
        // Surface — do not swallow. Still attempt re-spawn: a "no longer alive" /
        // "no such workspace" close is the common case for a dead pane the user is
        // trying to bring back; spawn_workspace will fail loudly if the id is still
        // live (CLOSE_PENDING / ALREADY_LIVE).
        console.error("[TauriAgentBridge] close_workspace failed on restart:", id, e);
      }

      // Fresh local state for the new PTY (same id). Do NOT call _forget — that
      // drops the id from the poll union before re-spawn registers it again.
      this.offsets[id] = 0;
      this.raw[id] = "";
      this.gen[id] = (this.gen[id] || 0) + 1;
      this.dead.delete(id);
      this.deadLocal.delete(id);
      this.spawned[id] = { kind, role, repo };
      this._saveSpawned();

      const si = this.agents.findIndex((a) => a.id === id);
      if (si !== -1) {
        this.agents[si] = {
          ...this.agents[si],
          kind,
          role,
          status: "starting",
          attention: null,
          raw: "",
          gen: this.gen[id],
        };
        this._emit();
      }

      try {
        await invoke("spawn_workspace", { id, harness, repo, role });
        if (role) await invoke("set_pane_roles", { roles: [[id, role]] });
      } catch (e) {
        // Pane is gone if close succeeded — must be visible, not a silent reject.
        delete this.spawned[id];
        this._saveSpawned();
        const detail = String(e && e.message ? e.message : e);
        this.raw[id] = "\r\n\x1b[31m[restart failed] " + detail + "\x1b[0m\r\n";
        console.error("[TauriAgentBridge] restart spawn_workspace failed:", id, e);
        const ei = this.agents.findIndex((a) => a.id === id);
        if (ei !== -1) {
          this.agents[ei] = { ...this.agents[ei], status: "error", raw: this.raw[id] };
          this._emit();
        }
      }
    }
    this._poll();
  }

  // resumeAll() removed: superseded by resumeAgents(ids) — resumeAll was exactly
  // resumeAgents(every id) — and it had no call site anywhere in the repo. Its only
  // plausible consumer was the fleet-wide Play button the operator deliberately
  // deleted; keeping it would leave that rejected global-resume model lying around as
  // a trap. Resume is per-pane now.

  // Kill the given panes for good (PTY + worktree via close_workspace) — no respawn.
  // Distinct from restartAgents (close + re-spawn same id). Used by workspace delete
  // and single-pane Close. Clears assignment map so deleted tabs leave no orphans.
  async closeAgents(ids) {
    for (const id of ids) {
      try {
        await invoke("close_workspace", { id });
      } catch (e) {
        console.error("[TauriAgentBridge] close_workspace failed:", id, e);
      }
      this._forget(id);
      unassign(id);
    }
    this.agents = this.agents.filter((a) => !ids.includes(a.id));
    this._emit();
  }

  stopAll() { return this.closeWorkspace(); }

  async closeWorkspace() {
    for (const a of this.agents) await invoke("close_workspace", { id: a.id });
    this.agents = [];
    this.offsets = {};
    this.raw = {};
    this.gen = {};
    this.spawned = {};
    this.dead = new Set();
    this.deadLocal = new Set();
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