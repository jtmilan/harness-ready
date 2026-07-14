# Plan — Replicate agent-teams workflow LOGIC into the new UI (not the visual design)

## Topic/Intent
The new Base44 "Agent Command Center" UI ($UI = ~/Personal/nexus-agents) now spawns panes and
streams real terminals (xterm + union-read fix, verified). It is MISSING the prod app's
orchestration workflow surface. Replicate the LOGIC/WORKFLOW (not the prod look): window
auto-tile / resize / merge, Orchestrate, Flywheel, Loops (routines), Runs (history), Delegate +
concurrency. Wire the new UI's bridge to the EXISTING Tauri commands — do not reimplement backend.

## Ground truth — Tauri commands ALREADY exposed (agent-teams/app/src-tauri/src/lib.rs generate_handler!)
- Orchestrate: `orchestrate` (goal → per-pane tasks; dispatch=false previews, true fans out)
- Delegate: `delegate`, `delegate_stop`, `delegate_set_autonomy`, `delegate_gate_status`,
  `delegate_run_history`, `delegate_trust_repo`, `delegate_is_repo_trusted`, `list_trusted_repos`
- Flywheel/Bridge: `bridge_new_run`, `bridge_write_manifest`, `bridge_ready`, `bridge_verify`,
  `bridge_synthesize`, `bridge_assemble_integration`, `bridge_open_pr`
- Loops/Routines: `loop_list`, `loop_create`, `loop_update`, `loop_delete`, `loop_set_enabled`,
  `loop_run_now`, `loop_run_history`
- Runs / concurrency: `run_now`, `set_max_concurrent`, `reconcile_daemon_panes`, `set_pane_roles`
- (already wired by this session: spawn_workspace, close_workspace, send_input, resize_pty,
  read_output_delta_batch, list_queue, dead_pane_ids, pause_pane, resume_pane, default_folder)

INFERRED: some commands are gated (`delegate-live` feature, `allow_mutations`, `loop_autonomy`).
Adapter calls will surface gate errors as `DELEGATE_UNAVAILABLE` / refusals — UI must show them,
not swallow. EXTRACTED from the handler-block comments at lib.rs:13886-13902.

## Architecture for PARALLEL, collision-free build
Prod drives these from one monolith (app/src/main.js). To parallelize without merge conflicts on
shared files (tauriAgentBridge.js, Home.jsx, TopBar.jsx), decompose so each lane owns DISJOINT
NEW files, and ONE serial integration pass wires them:

- **Feature module** per feature: `src/lib/features/<feature>.js` — pure functions taking `invoke`
  (e.g. `export async function orchestrate(invoke, {goal, dispatch})`). No shared-file edits.
- **Overlay component** per feature: `src/components/command/<Feature>Overlay.jsx` — self-contained,
  props-driven, matches existing overlay style (see CommandOverlay/TemplatesOverlay). No shared edits.
- **Serial integration (coordinator, last):** add thin adapter methods delegating to the feature
  modules, TopBar buttons, and Home overlay-state — the only edits to the 3 shared files, done once.

Window layout is the exception (it edits Home grid + AgentPane): its own lane OWNS Home.jsx layout
+ a `src/lib/layout.js`; integration of feature-overlay triggers happens AFTER, by the coordinator.

## Lanes (disjoint; each writes a report to scratchpad)
- **L1 layout (typescript):** auto-tile modes (single / grid / 2-col / focus-split), per-pane
  drag-resize, "merge" (group panes into one workspace tab / combine two workspaces). Owns Home.jsx
  grid region + new `src/lib/layout.js`. Mirror prod layout modes (main.js layout toggle: the
  4 header icons). Persist layout choice in localStorage.
- **L2 orchestrate:** `features/orchestrate.js` + `OrchestrateOverlay.jsx` — goal box → preview
  per-pane task mapping (dispatch=false) → confirm → dispatch (dispatch=true).
- **L3 loops:** `features/loops.js` + `LoopsOverlay.jsx` — list/create/edit/enable-toggle/run-now/
  view history over loop_* commands.
- **L4 runs:** `features/runs.js` + `RunsOverlay.jsx` — delegate_run_history + loop_run_history +
  run dirs; read-only history browser.
- **L5 flywheel:** `features/flywheel.js` + `FlywheelOverlay.jsx` — goal → bridge_new_run →
  write_manifest → (delegate) → bridge_ready → bridge_verify → bridge_synthesize →
  bridge_assemble_integration → bridge_open_pr; pipeline-stage view. Surface gate errors.
- **L6 delegate+concurrency:** `features/delegate.js` + wire max-concurrent control — delegate,
  delegate_set_autonomy, delegate_gate_status, set_max_concurrent. Autonomy + trust-repo aware.
- **L7 backend audit (rust, read-only):** verify each command's arg/return shape vs what the
  feature modules assume; file mismatches; propose minimal ADDITIVE fixes only (no struct changes).

## Contracts (pin BEFORE fan-out)
- Adapter method names the integration will expose (lanes target these): `orchestrate(goal,dispatch)`,
  `loopList()/loopCreate(cfg)/loopUpdate(id,cfg)/loopDelete(id)/loopSetEnabled(id,on)/loopRunNow(id)/
  loopRunHistory(id)`, `runHistory()`, `flywheelRun(goal,opts)`, `delegate(goal,opts)/
  delegateSetAutonomy(level)/delegateGateStatus()/setMaxConcurrent(n)`.
- Overlay props: `{open, onClose, bridge}` (bridge passed from Home, same as existing overlays).
- Feature modules are `invoke`-injected (testable, no window coupling).
- Every adapter call wraps invoke in try/catch and returns `{ok,data,error}` so overlays render
  gate/refusal errors (DELEGATE_UNAVAILABLE etc.) instead of dying.

## Anti-Patterns (avoid)
- Reimplementing backend logic in JS (the commands exist — call them). EXTRACTED from handler list.
- Editing the shared 3 files from parallel lanes (merge hell) — hence the module+overlay split.
- Swallowing gate errors (prod surfaces them; so must we).
- Touching the prod app/src or Rust structs (handoff constraint).

## Open Questions (AMBIGUOUS — surface to user / resolve in L7)
- `orchestrate` preview return shape (per-pane task list JSON?) — L7 confirms from lib.rs:3281.
- `loop_create` config schema (cron? interval? command?) — L7 confirms from LoopConfig struct.
- "merge windows" exact prod semantics — combining two workspaces vs grouping panes. INFERRED as
  workspace-merge (move panes of ws B into ws A). Confirm against main.js layout code.
- Flywheel gating: needs `delegate-live` build + allow_mutations; on a default build it self-refuses.
  UI shows the refusal; actual runs need the operator to arm gates. EXTRACTED from lib.rs comments.

## Sources
- agent-teams/app/src-tauri/src/lib.rs (generate_handler! 13876-13990; orchestrate 3281; bridge_* 3355,4808,4980,5069,5110,5837,5916; loop_* 7828+; delegate*), app/src/main.js (layout + all workflow driving)
- nexus-agents current: src/components/command/*Overlay.jsx, Home.jsx, src/lib/tauriAgentBridge.js
- This session: DD-A/DD-B deep-dive reports (scratchpad); nexus-agents-ui-embed memory

## Conformance
Approach conforms to industry-standard ports-and-adapters + parallel-safe module decomposition:
UI calls existing backend commands through injectable feature modules; no backend reimplementation;
shared-file edits serialized to one integration pass. AMBIGUOUS items routed to the L7 backend audit
before the dependent lanes wire their calls.
