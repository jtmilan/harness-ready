/**
 * AgentBridge — the wiring contract between this UI and a local backend.
 *
 * The UI only ever talks to the exported `bridge` singleton. To rebuild this
 * as a native macOS app (Tauri / Electron), replace MockAgentBridge with an
 * implementation that drives real PTYs and git worktrees:
 *
 *   - spawnAgents(configs): for each config, `git worktree add <path> -b <branch>`
 *     then spawn AGENT_KINDS[kind].cmd in a PTY (portable-pty / node-pty) cwd'd there.
 *   - subscribe(cb): push a full agent-state snapshot on every PTY output chunk
 *     or status change. cb receives the agents array.
 *   - sendInput(id, text): write `text + "\n"` to that agent's PTY stdin.
 *   - status detection: parse PTY output for interactive prompts / permission
 *     requests → set status "needs_input" with attention {reason, since}.
 *   - pause/restart/stop: SIGSTOP / respawn / SIGTERM the PTY child process.
 *
 * Agent shape:
 *   { id, kind, role?, branch, worktree, status,
 *     attention: { reason, since } | null, output: string[] }
 * Statuses: working | needs_input | blocked | error | starting | idle
 */
import { createAgents, randomLine, ATTENTION_REASONS } from "@/lib/agentData";
import { TauriAgentBridge, isTauri } from "@/lib/tauriAgentBridge";
import { randomAgentName } from "@/lib/agentNames";

class MockAgentBridge {
  constructor() {
    this.agents = []; // fleet starts empty — spawn via UI or loadDemoFleet()
    this.listeners = new Set();
    this.running = true;
    this.timer = null;
  }

  start() {
    if (this.timer) return;
    this.timer = setInterval(() => {
      if (!this.running) return;
      let changed = false;
      this.agents = this.agents.map((a) => {
        if (a.status !== "working" || Math.random() <= 0.4) return a;
        changed = true;
        if (Math.random() < 0.03) {
          const reason = ATTENTION_REASONS[Math.floor(Math.random() * ATTENTION_REASONS.length)];
          return { ...a, status: "needs_input", attention: { reason, since: Date.now() }, output: [...a.output.slice(-40), ">> AWAITING OPERATOR INPUT"] };
        }
        return { ...a, output: [...a.output.slice(-40), randomLine()] };
      });
      if (changed) this._emit();
    }, 900);
  }

  setRunning(v) { this.running = v; }

  subscribe(cb) {
    this.listeners.add(cb);
    cb(this.agents);
    return () => this.listeners.delete(cb);
  }

  _emit() {
    this.agents = [...this.agents];
    this.listeners.forEach((cb) => cb(this.agents));
  }

  _patch(filter, fn) {
    this.agents = this.agents.map((a) => (filter(a) ? fn(a) : a));
    this._emit();
  }

  _append(a, line, extra = {}) {
    return { ...a, ...extra, output: [...a.output.slice(-40), line] };
  }

  // --- operator actions (PTY stdin writes in the real backend) ---
  sendInput(id, text) {
    this._patch((a) => a.id === id, (a) => this._append(a, `> ${text}`, { status: "working", attention: null }));
  }

  // Web-preview stubs so the xterm-based AgentPane can call these unconditionally.
  // The real terminal (raw byte input + PTY resize) exists only in the Tauri build.
  sendRaw(_id, _data) {}
  resizePane(_id, _rows, _cols) {}

  delegate(id, task) {
    this._patch((a) => a.id === id, (a) => this._append(a, `>> TASK DELEGATED: ${task}`, { status: "working", attention: null }));
  }

  broadcast(text) {
    this._patch(() => true, (a) => this._append(a, `>> BROADCAST: ${text}`));
  }

  broadcastTo(ids, text) {
    this._patch((a) => ids.includes(a.id), (a) => this._append(a, `>> BROADCAST: ${text}`));
  }

  // --- process control (signals in the real backend) ---
  pauseAgents(ids) {
    this._patch((a) => ids.includes(a.id), (a) => this._append(a, ">> PAUSED BY OPERATOR", { status: "idle", attention: null }));
  }

  restartAgents(ids) {
    this._patch((a) => ids.includes(a.id), (a) => this._append(a, ">> AGENT RESTARTED", { status: "working", attention: null }));
  }

  // Real backend: kill each PTY child + `git worktree remove`, then drop the panes.
  closeAgents(ids) {
    this.agents = this.agents.filter((a) => !ids.includes(a.id));
    this._emit();
  }

  resumeAll() {
    this._patch((a) => a.status === "idle", (a) => ({ ...a, status: "working" }));
  }

  stopAll() {
    this._patch(() => true, (a) => this._append(a, ">> STOPPED BY OPERATOR", { status: "idle", attention: null }));
  }

  // Real backend: kill all PTY children, `git worktree remove` each, then emit []
  closeWorkspace() {
    this.agents = [];
    this._emit();
  }

  advanceStarting() {
    this._patch((a) => a.status === "starting", (a) => ({ ...a, status: "working" }));
  }

  loadDemoFleet() {
    this.agents = createAgents(12);
    this._emit();
  }

  // --- lifecycle (git worktree add + PTY spawn in the real backend) ---
  // Returns `{ wsId, paneIds }` — same shape as TauriAgentBridge.spawnAgents so Home can
  // create a UI workspace tab and assign panes without default-bucket pooling.
  spawnAgents(configs, templateName) {
    const base = this.agents.length;
    const paneIds = [];
    // Mint a backend-shaped workspace id so mock + real share the same call contract.
    const wsId = "ws" + String(Math.floor(10000 + Math.random() * 90000)) + "x0";
    configs.forEach((cfg, i) => {
      const num = String(base + i + 1).padStart(3, "0");
      const id = `AGENT-${num}`;
      paneIds.push(id);
      this.agents.push({
        id,
        name: randomAgentName(),
        kind: cfg.kind || "bash",
        role: cfg.role,
        branch: `feat/${(cfg.role || cfg.kind || "task").toLowerCase().replace(/\s+/g, "-")}-${num}`,
        worktree: `~/worktrees/agent-${num}`,
        status: "starting",
        attention: null,
        output: [
          `>> LAUNCHED FROM TEMPLATE: ${templateName}`,
          `>> ROLE: ${(cfg.role || "").toUpperCase()} | AGENT: ${(cfg.kind || "bash").toUpperCase()} | PRIORITY: ${(cfg.priority || "normal").toUpperCase()} | AUTONOMY: ${(cfg.autonomy || "semi").toUpperCase()}`,
          "$ git worktree add ... && agent init",
        ],
      });
    });
    this._emit();
    setTimeout(() => this._patch((a) => paneIds.includes(a.id) && a.status === "starting", (a) => ({ ...a, status: "working" })), 3000);
    return { wsId, paneIds };
  }
}

// Inside the Tauri shell (jtmilan/agent-teams) the real backend bridge is used;
// in the hosted web preview the mock keeps the UI fully explorable.
export const bridge = isTauri() ? new TauriAgentBridge() : new MockAgentBridge();