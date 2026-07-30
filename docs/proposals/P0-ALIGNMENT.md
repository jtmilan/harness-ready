# P0 — ALIGNMENT AUDIT · harness-ready UI

**Date:** 2026-07-29 · **Scope:** `ui/` over `agentBridge.js` · **Mode:** audit only, no code
**Rules in force:** R1–R10 (docs/BASE44-PROMPTS.md, STEP 2) · **Baseline:** `npx vitest run` 46/46 PASS, `vite build` OK (2.78s) — gate green before any proposal work.

**Method:** direct read of the read-first file list (HANDOFF.md, agentBridge.js, tauriAgentBridge.js, agentTypes.js, Home.jsx, Monitoring.jsx, components/command/**, components.json, tailwind.config.js) plus index.css, App.jsx, monitorData.js, workspaceStore.js, useKeyboardShortcuts.js, templates/*; cross-checked against docs/PRD.md §6–§8, docs/DESIGN-BRIEF.md §1–§9, docs/BUILD-PLAN.md §0–§2, docs/RESEARCH-SYNTHESIS.md §5/§8/§11.

---

## 1 · ALIGNMENT MAP

One row per current screen/component. Columns: (a) governing doc section, (b) R-* satisfied/missed, (c) FAKE-AFFORDANCE found today, (d) simulated data shown without a placeholder affordance.

| Screen / component | (a) Governed by | (b) R-* status | (c) Fake affordance | (d) Unflagged simulated data |
|---|---|---|---|---|
| **Home command surface — TopBar.jsx** | DB §2 (skeleton "correct for ICP"); HANDOFF UI map | ✅ R7 partial (actions exist); ✅ R3 (every button → real `bridge.*`); ⚠️ R5 (hardcoded hex/cyan classes, no lane tokens) | none — NEW AGENT→`spawnAgents` (cap-guarded via `getCapacity`), BROADCAST→`broadcast`, DELEGATE→`delegate`, TEMPLATES→overlay, CLOSE WORKSPACE→`closeWorkspace`, ⌘⇧I→`broadcastRaw` fan-out | none (`activeCount` computed live from subscribe) |
| **Home — pane grid (AgentPane.jsx + AttentionPrompt.jsx)** | HANDOFF "Attention surfacing"; DB §6 (amber = single "needs you" source); PRD §9 | ✅ R3 (keystrokes→`sendRaw`/`broadcastRaw`, reply→`sendInput`, close→`closeAgents`, resize→`resizePane`); ✅ R4 partial (needs_input = amber border + inline reply); ⚠️ R4 (blocked/error = badge-only, no dedicated treatment; **stale not handled at all**); ⚠️ R8 (status conveyed by color+letter badge only) | none | none (raw byte count, branch, worktree all live props) |
| **Home — EmptyState.jsx** | DB §2 ("keep"); HANDOFF (fleet starts empty; `loadDemoFleet` mock-only) | ✅ R1 (spawn/template CTAs right for ICP); ❌ R3 (**LOAD DEMO FLEET button is dead** — `onLoadDemo` never passed by Home.jsx, `onClick` undefined); ⚠️ R4 (hardcoded "0 active" line, not computed) | **LOAD DEMO FLEET** — renders, does nothing | "$ agent fleet status — 0 active" — hardcoded `0` |
| **Home — NewAgentOverlay.jsx** | DB §4.1 (spawn flow target); PRD §7 R-ORCH | ✅ R3 (SPAWN→`spawnAgents`); ❌ R-ORCH not implemented (no role cast, no owned-paths, no preview roster, no collision check — role is a free-text input only) | none | none (defaults only) |
| **Home — templates (TemplatesOverlay/TemplateList/TemplateBuilder)** | HANDOFF ("swap for a local store, same schema"); PRD §7 R-TEMPLATES; DB §4.5 | ⚠️ R3 (controls are real but backing store is remote); ❌ **R6 local-first violated** — persistence is `base44.entities.AgentTemplate.{list,create,delete}`, the only Base44-entity dependency left in the UI; R-TEMPLATES not implemented | none (LAUNCH→`spawnAgents` works; save/delete work against Base44) | none; but remote store means templates vanish/change with the hosted backend — a honesty hazard for a local-first product |
| **Home — bottom rail: PerformanceWidget.jsx** | DB §2; R4 (honest monitoring) | ✅ mostly honest — trend = live count of `status==="working"` sampled 2s; status pie = live; ⚠️ R4 (no empty/no-data state when fleet empty — widget simply unmounts with the grid); ⚠️ R5 (STATUS_COLORS hardcoded) | none | none (live-derived) |
| **Home — bottom rail: SessionInfo.jsx** | no doc section sanctions this widget | ❌ R4/R9 honesty — **SESSION_ID `"00612425-38791089839"` hardcoded constant**; SESSION_START = module-load `Date.now()`; `running` prop hardcoded `true` → PAUSED branch is dead code | the RUNNING/PAUSED indicator itself (always RUNNING) | session id + uptime rendered as real, sourced from nowhere |
| **Home — bottom rail: AgentDirectory.jsx / WorkspacesPanel.jsx / WorkspaceTile.jsx** | DB §3 (workspace tabs kept); HANDOFF | ✅ R3 (sharing toggle→`setWorkspaceSharing` with confirm-on-relax; delete→`closeAgents`; backend authoritative for `allow_sharing`, stripped from localStorage) | none | none (member counts live) |
| **Home — LayoutToolbar.jsx (modes + ws tabs)** | DB §2 ("keep") | ✅ local layout only, no backend claims | none | none |
| **Home — BulkActionBar.jsx** | HANDOFF (`pauseAgents`/`resumeAgents` real SIGSTOP/SIGCONT); DB §6 (RESUME-always) | ✅ R3 all four buttons real; ✅ R6/DB §6 — RESUME always offered, never toggled (no `paused` field exists); ⚠️ R8 no post-action state feedback | none | none |
| **Home — PaneMenu.jsx** | DB §3 nav rule (overflow idiom) | ✅ all six items produce real effects (clipboard, local label store, `assign`, `closeAgents`) | none ("rename" no-op in Home is benign — AgentPane commits via `paneLabels` before notifying) | none |
| **TitleBar.jsx / nav (COMMAND · MONITORING)** | DB §3 ("top-right tab toggle stays"); App.jsx routes `/` + `/monitoring` | ✅ R7 partial; ⚠️ R5 hardcoded styling | none | none |
| **Monitoring page (Monitoring.jsx + monitor/**)** | DB §2 (**"Currently simulated"**); PRD §7 R-OBS AC; PRD §8 stop-doing; Synthesis §8 | ❌❌ **R4 worst violation in the app**: zero `bridge.subscribe`; all data from `monitorData.js` (random-walk CPU/mem/net, random success rates); agent ids `AGENT-001…012` hardcoded regardless of real fleet; no empty/loading/stale/state_blind/error states; ❌ R3 (charts claim "Live" in copy — "Fleet Resource Usage — Live"); ❌ PRD §10 success metric ("zero 'looks live but is fake' surfaces") | the whole page: "LIVE" chart, per-agent success bars for agents that may not exist, TASKS COMPLETED counter | **everything** — no placeholder affordance anywhere. PRD §8 explicitly: "the current MONITORING state must be visibly placeholder until R-OBS" |
| **Contract layer (agentBridge.js / tauriAgentBridge.js)** | HANDOFF contract table; R10 | ✅ R10 honored — singleton, adapter swap on `window.__TAURI__`, K4 unmapped-kind refusal, spawn returns `{wsId, paneIds}`, cap poll, dead-pane reconciliation, `reconcilePaneLabels` | mock-only `sendRaw`/`resizePane`/`broadcastRaw` are documented stubs (parity, not fakes) | `MockAgentBridge.start()` + `agentData.js#createAgents` are declared simulation (HANDOFF "Simulation to strip") — acceptable in web preview, must never read as live |
| **Theme layer (index.css / tailwind.config.js / components.json)** | DB §5 (lanes + data-skin); R5; R8 | ⚠️ R5 **mixed**: ✅ type pairing exists (`--font-display`: Rajdhani, `--font-body/mono`: JetBrains Mono) + ambient layer exists (`.scanlines` cyan repeating-gradient, `.crt-screen`); ❌ **no semantic lane tokens** (`--need/--success/--danger/--info` absent); ❌ dark look = hardcoded `#0D1117` + ad-hoc `cyan-*` classes while shadcn `:root` tokens still hold default **light** values; ❌ no `data-skin`-ready variable layer; ⚠️ R8 no focus-ring system visible in config | — | — |
| **Keyboard layer (useKeyboardShortcuts.js)** | R7; DB §3 (palette = fast path) | ❌ R7 thin: only **⌘⇧I** (broadcast toggle) + **⌘G** (zoom); macOS-only (`metaKey`, no Ctrl fallback); no palette; no spawn/focus/template bindings | — | — |

---

## 2 · BRIDGE CONTRACT COVERAGE (what exists vs what proposals may claim)

**EXISTS today** (proposals may bind to these freely):
`subscribe` · `start` · `sendInput` · `sendRaw` · `resizePane` · `delegate` · `broadcast` · `broadcastTo` · `broadcastRaw` · `pauseAgents` · `resumeAgents` · `restartAgents` · `closeAgents` · `closeWorkspace` · `stopAll` · `advanceStarting` · `spawnAgents(configs, name, {assignTo, wsId})` → `{wsId, paneIds}` · `getCapacity()` (Tauri: `get_capacity` poll) · `setWorkspaceSharing` / `fetchSharingStates` / `getSharing` (Tauri sharing commands) · `onEarlyRespawn` · Tauri events `maximize-pane` / `pane-early-respawn` / `spawn-cap-warning` · poll core `list_queue` + `dead_pane_ids` + `read_output_delta_batch`.

**NOT on the contract** (any proposal binding these must tag **NEEDS-BACKEND**, per R3/R10):
per-child CPU/mem sampling · git-state query · last-tool-failure event · human-gate queue depth · task-outcome events (success/blocked/error counts) · memory-graph read API · recipe/playbook store · skills-catalog source · session metadata (real id/start) · paused-state query (deliberately absent — RESUME stays one-way).

---

## 3 · CURRENT KEYBOARD MAP

| Binding | Action | Backing |
|---|---|---|
| ⌘⇧I | broadcast-mode toggle (keystrokes fan to all panes via `broadcastRaw`, reply traffic excluded) | local state + `bridge.broadcastRaw` |
| ⌘G | zoom selected/zoomed pane | `useTiling.toggleZoom` (local) |
| Tauri app menu "Toggle Pane Zoom" | same as ⌘G | `maximize-pane` event |

Everything else (spawn, delegate, templates, close, focus pane, tab switch) is mouse-only today.

---

## 4 · RANKED HIGHEST-VALUE UNIMPLEMENTED R-* ITEMS

1. **R-OBS — real monitoring** (PRD §7, Phase 1a/1b; Synthesis §5 row 1). Monitoring.jsx is the single biggest R4 violation and the product's stated differentiator ("honest, granular per-agent health from real signals"). Critical path per BUILD-PLAN: 0 → 1b. Highest value, highest doc pressure.
2. **R-GATES UI surface — one "registry reconciling / stale" badge** (DB §6 liveness-blindness UX; PRD §7 R-GATES is backend HD, but its *UI surface* is designable now, tagged NEEDS-BACKEND): today nothing prevents "live in one place, gone in another"; the worktrees-registry leak (dead rows for closed panes) is documented in agent-teams memory.
3. **R-TEMPLATES — local store** (PRD §7; DB §4.5; Phase 2, independent). Only remaining Base44-entity dependency; direct R6 violation; free parallel track, small effort, removes the last cloud tether.
4. **R-ORCH — role-cast + owned-paths in spawn/orchestrate** (PRD §7; DB §4.1; Phase 4). NewAgentOverlay has none of it; advisory-only with amber collision WARNING (no enforce toggle — DB §7.5).
5. **R-MEM — CONTEXT surface, read-only memory graph** (PRD §7; DB §4.3; Phase 3, HD-if-write-contract — read-only view is not HD). BORROW verb; agents already read/write via MCP, humans have no browse surface.
6. **Micro-honesty fixes** (R3/R4/R9, effort S, should ride with any P5): kill or wire the dead LOAD DEMO FLEET button; SessionInfo fake id/uptime/always-RUNNING; EmptyState hardcoded "0 active"; Monitoring "Live" copy.
7. **R5 lane tokens + data-skin layer** (DB §5/§9; effort S-M, prerequisite for honest monitoring charts): four lanes on dedicated tokens; charts today use hardcoded STATUS_COLORS — CPU=info/mem=need/success=success/error=danger mapping needs the tokens to exist first.
8. **R-PALETTE** (PRD §7; DB §4.4; Phase 5, opportunistic): only 2 shortcuts exist; palette over EXISTING handlers, each row shows keybinding.
9. **R-ONBOARD — recipes/playbooks** (Synthesis §11 item 4; DB §3; Phase 4b free parallel): 8 harnesses × autonomy gates = brutal activation curve; reuse templates manager; never video.
10. **R-SKILLS-OSS — catalog landing** (PRD §7; Phase 6, opportunistic): mostly docs/site, minimal ui/ surface.

**Sequencing anchor (BUILD-PLAN §1 verbatim rationale):** critical path = Phase 0 (R-GATES) → 1b (R-OBS attribution); must-haves next = R-TEMPLATES (2) / R-MEM (3) / R-ORCH (4); opportunistic = R-PALETTE (5) / R-SKILLS-OSS (6) / R-ONBOARD (4b); R-MCP-RW (7) last, gated. P1 proposals must echo this order.

---

## 5 · WHAT IS ALREADY RIGHT (do not regress in P5)

- Command-surface skeleton matches ICP (DB §2): act/observe split, harness grid, workspace tabs, `$ agent fleet status` motif.
- Attention semantics: amber border + inline reply + ranked queue ordering (HANDOFF: error=400/needs_input=300/blocked=200 + seconds waiting).
- RESUME-always, no pause toggle (DB §6; no `paused` field exists).
- Spawn cap guard + queued-spawn toast + `spawn-cap-warning` event (honest admission control).
- Sharing toggle: confirm-on-relax, backend authoritative, never persisted locally (correct trust boundary).
- Type pairing + scanline ambient layer already present (P3 extends, not replaces).
- K4 refusal of unmapped harness kinds (loud, never silent rewrite to bash).

---

## 6 · ERRATA + ADDITIONS (after independent verification by pane p6, `panes/p6.md`)

**E1 — Templates row corrected (P0 was factually wrong).** `ui/src/api/base44Client.js` is an **offline local stand-in** ("Same export shape (`base44.entities.*`, `base44.auth.*`) so call sites keep working without network"), importing `AgentTemplate` from `./localAgentTemplateStore.js` (localStorage key `agent-templates`). Templates are **already local-first** — the `base44.entities.*` call shape is a historical API silhouette, not a cloud tether. Verified by coordinator against source. Corrections:
- §1 templates row: ❌ R6 violated → **✅ R6 satisfied** (controls honest, store local). Residual hazard is naming/confusion, not cloud dependency.
- §4 ranked item #3 re-scopes: R-TEMPLATES remaining work = JSON import/export + schema pin + recipes coupling (BUILD-PLAN Phase 2 DoD), **not** "remove cloud tether".
- p5's "templates via hosted entity is peer-unusual" premise falls away; its import/export recommendation stands on its own.

**Misses P0 did not list (p6 §3, all file:line verified, folded into ranked item #6 micro-honesty):**
- **M1 (medium):** PRIORITY + AUTONOMY selects in `NewAgentOverlay.jsx:61-73` and `TemplateBuilder.jsx:31-40` have **no backend effect** — Tauri `spawn_workspace` takes only `id, harness, repo, role` (`tauriAgentBridge.js:481-486`); mock merely echoes them into a log line. Kill or tag NEEDS-BACKEND + disable. R3 violation.
- **M2 (medium):** mock bridge lacks `setWorkspaceSharing`/`fetchSharingStates`/`getSharing` while `Home.jsx:145` unconditionally awaits → TypeError + rollback toast in web preview. Add mock stubs (parity precedent: `resumeAgents`/`broadcastRaw`) or guard on method presence.
- **M3 (medium-low):** empty non-default workspace copy always blames the pane cap (`Home.jsx:732-744`) even when not at cap — gate on real `atCap`, else "no agents assigned".
- **M4 (low):** TopBar one-shot BROADCAST tooltip claims ⌘⇧I (`TopBar.jsx:26-27`) — ⌘⇧I is the broadcast-mode *toggle*, not the one-shot.
- **M5 (low):** LayoutToolbar "Single pane (⌘G)" tooltip (`LayoutToolbar.jsx:6`) — ⌘G is zoom, not a layout mode.
- **M6 (low):** auth page suite (Login/Register/Forgot/Reset) is offline-stubbed **and unrouted** in App.jsx — dead code; honesty tags required if ever remounted.
- **M7 (low):** WorkspaceTile chip mode ignores `onRename`/`onDelete` props EmptyState passes (`WorkspaceTile.jsx:120-128`).
- **M8 (low):** mock `sendRaw` no-op means web-preview keystrokes never echo (documented stub; can surprise operators).

**Confirmed by p6:** dead LOAD DEMO FLEET, SessionInfo fake constants, whole-page Monitoring simulation, thin keyboard map — all stand. Everything else in §1 verified HONEST (coverage matrix: p6 §5, 21 paths traced).

---

*Next: P1 feature proposals → P2 UI/IA redesign → P3 visual system (fan-out to ws83621x0 panes — DONE, `panes/p1..p6.md`), then P4 adversarial review (pane p4), then coordinator synthesis. Operator picks one id for P5.*
