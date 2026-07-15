# Handoff Spec — Agent Supervisor (macOS rebuild)

A local macOS desktop app for supervising interactive AI coding agents (Claude Code,
Cursor, OpenCode, Codex, Grok, CommandCode, Cline, or bash for smoke tests), each
isolated in its own **git worktree + PTY**, surfaced through one ranked
**"who needs you right now"** queue.

This repo is the finished UI. All backend wiring flows through one contract.

## The contract: `src/lib/agentBridge.js`

The UI only talks to the exported `bridge` singleton. Replace `MockAgentBridge`
with a real implementation (Tauri commands / Electron IPC) — nothing else in the
UI needs to change.

| Method | Real implementation |
|---|---|
| `subscribe(cb)` | Push agent snapshots on every PTY output chunk / status change |
| `sendInput(id, text)` | Write `text + "\n"` to the agent's PTY stdin |
| `delegate(id, task)` | Send a task prompt to the agent's PTY |
| `broadcast(text)` / `broadcastTo(ids, text)` | Write to many PTYs |
| `pauseAgents(ids)` / `resumeAgents(ids)` / `restartAgents(ids)` | SIGSTOP / SIGCONT / respawn PTY child (per pane) |
| `stopAll()` / `advanceStarting()` | Fleet-wide process control |
| `closeWorkspace()` | Kill all PTY children, `git worktree remove` each, emit empty fleet (UI returns to launch pad) |
| `spawnAgents(configs, name)` | `git worktree add <path> -b <branch>` then spawn `AGENT_KINDS[kind].cmd` (see `src/lib/agentTypes.js`) in a PTY cwd'd to the worktree |

**Agent shape** (what `subscribe` must emit):
```js
{ id, kind, role?, branch, worktree, status,
  attention: { reason, since } | null, output: string[] }
```
**Statuses:** `working | needs_input | blocked | error | starting | idle`

**Status detection:** parse PTY output for interactive prompts / permission
requests / merge conflicts → set `needs_input` / `blocked` / `error` with an
`attention.reason` string and `since` timestamp.

## Attention surfacing

Agents needing the operator surface directly on their pane: an amber-highlighted
border plus an inline `AttentionPrompt` (reason + reply input) whenever
`status === "needs_input"`. For the ranked "who needs you right now" ordering,
sort agents by `baseScore(status) + secondsWaiting(attention.since)` in the
backend (suggested: error=400, needs_input=300, blocked=200) and emit agents in
that order via `subscribe`.

## UI map

- `src/pages/Home.jsx` — command center; all actions call `bridge.*`
- `src/components/command/AgentPane.jsx` — per-agent PTY view + inline reply (`AttentionPrompt`)
- `src/components/command/templates/*` — team templates (save/launch); currently
  persisted via the Base44 `AgentTemplate` entity — swap for a local store
  (SQLite/JSON) in the desktop build, keeping the same schema
  (`base44/entities/AgentTemplate.jsonc`)
- `src/pages/Monitoring.jsx` — metrics dashboard; `src/lib/monitorData.js` is
  simulated — feed real per-process CPU/mem and task outcomes
- `src/components/command/NewAgentOverlay.jsx` — manual spawn form (role + harness + priority + autonomy) → `bridge.spawnAgents`
- `src/components/command/EmptyState.jsx` — zero-agent landing (fleet starts empty; `bridge.loadDemoFleet()` is mock-only)
- `src/lib/agentTypes.js` — agent kind → CLI command registry

## Wiring to jtmilan/agent-teams (Tauri + Rust)

`src/lib/tauriAgentBridge.js` is a ready adapter for the agent-teams backend.
It activates automatically when `window.__TAURI__` exists (set
`app.withGlobalTauri: true` in `tauri.conf.json`), and maps the contract to
these Tauri commands from `app/src-tauri/src/lib.rs`:

| Bridge method | Tauri command(s) |
|---|---|
| `subscribe` | polls `list_queue` + `read_output_delta_batch(reqs: [{id, since}])` + `dead_pane_ids` every 500ms |
| `spawnAgents` | `spawn_workspace(id, harness, repo)` + `set_pane_roles(roles)` |
| `sendInput` / `delegate` | `send_input(id, data)` |
| `broadcast` / `broadcastTo` | `send_input` per pane |
| `restartAgents` / `closeWorkspace` | `close_workspace(id)` per pane |
| `pauseAgents` / `resumeAgents` | `pause_pane(id)` / `resume_pane(id)` per pane — real SIGSTOP / SIGCONT |

To embed this UI in the agent-teams app: build it (`vite build`) and point
`tauri.conf.json` `frontendDist` at the output (replacing `app/src/`). Verify
the `QueueRow` field names in `_poll()` (harness/role/branch/worktree/
waiting_on_you/reason/since) against `core/daemon` — adjust the projection
there if they differ.

`pauseAgents`/`resumeAgents` are NOT no-ops: the backend exposes `pause_pane`/
`resume_pane`, which are real `SIGSTOP`/`SIGCONT` sent to the pane's PTY child.
Note that nothing records that a pane is paused — there is no `paused` field on
`QueueRow` or anywhere in `core/mcp` — so the UI cannot query pause state and
must not pretend to. RESUME is therefore always offered rather than toggled;
re-sending either signal is harmless.

## Simulation to strip

`MockAgentBridge.start()` fakes PTY output and random `needs_input` events;
`src/lib/agentData.js#createAgents` fakes the initial fleet. Delete both once
real PTYs are wired.