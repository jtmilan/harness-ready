# Harness Ready — Prioritized Improvement Plan (vs agent-teams upstream)

Date: 2026-07-15. Companion brief: `2026-07-15-agent-teams-vs-harness-ready-improvement-plan.md`.
Ground truth: same 155-command Rust backend; delta is almost entirely **frontend surface coverage** (`ui/` wires ~10 of 155 commands) plus repo hygiene.

## Guardrails (from vault, non-negotiable)

- Never `git pull`/cherry-pick across the two repos — unrelated histories; port by hand (`multi-agent-lane-gotchas.md`).
- Hook-driven state stays; no PTY scraping (architecture-locks decision).
- Every autonomy surface ships gated OFF by default (upstream triple-gate pattern).
- Don't collide with in-flight lanes: ws60935x4 (round 3: native macOS menu bar) and ws92431x3 (UI parity: maximize, pane menu, shortcuts, queue-order) own their files until merged.

## P0 — Hygiene / foundations (small, unblocks everything)

| Item | Why | Where |
|---|---|---|
| Restore CI: port `.github/workflows/ci.yml` from upstream | `scripts/test-all.sh` mirrors a ci.yml that doesn't exist here; zero CI on fork | upstream `.github/` → fork |
| Repo picker in spawn form; kill `DEFAULT_REPO` hardcode | spawns land in `/Users/jeffrymilan/agent-teams-devapp-workspace` — wrong repo for a standalone product | `ui/src/lib/tauriAgentBridge.js:78`, `NewAgentOverlay.jsx` |
| Kill `SESSION_ID` hardcode | fork residue | `ui/src/pages/Home.jsx:21` |
| README + LICENSE | dropped in fork | root |
| Untrack `clipboard-*.png`, in-tree worktrees; gitignore | repo hygiene | root |
| Decide state-root/sidecar rebrand (`agent-teams-*` → `harness-ready-*`) | collision risk with prod agent-teams documented open question | `2026-07-14-harness-ready-standalone.md` |

## P1 — UI feature parity (the big win: backend already done)

Execute existing lane plan `ui/.claude/context/2026-07-13-feature-parity-plan.md` (L1–L7). Order by user value:

1. **Per-harness model picker** — `list_harness_models` wired into NewAgentOverlay.
2. **Diff viewer** — `diff_changed_files`/`diff_unified`/`pane_branch`; port `app/src/diff-view-core.js` logic. Reviewing lane output is the core loop.
3. **Kanban task board** — `*_task_kanban` commands; port `board-core.js`/`kanban-core.js`.
4. **Orchestrate UI** — goal → per-pane task preview (`dispatch=false`) → dispatch; port from legacy main.js bridge-chat.
5. **Delegate (real)** — replace shallow `send_input` fan-out with gated `delegate*` + autonomy/trust surface (`delegate_gate_status`, `delegate_trust_repo`, Settings→Trusted Repos parity).
6. **Runs + loops** — `run_now`, `set_max_concurrent`, `loop_*` CRUD + run history.
7. **Flywheel/Bridge pipeline UI** — `bridge_*` stage progress, verdict cards (upstream Delegations panel as reference).
8. **Memory graph** — `memory_*` + `memory_graph`; `lightning-core.js`/`memory-lightning.js` already present in repo, need React host.
9. **Voice** — dictation/`speak` buttons (whisper sidecar already bundled).

## P2 — Backport upstream commits since fork (manual, diff-guided)

- #288 memory global scope + run-capture flywheel + relevance floor
- #289 config-resolved external-spawn cap threaded to frontend
- #287 Second Brain continuous learning + prompt governance (evaluate — may be out of scope for a leaner fork)
- Sentry crash monitoring (#266) — optional, default-off

Method: `diff` the ~27 divergent backend files each round; port hunks, never merge histories.

## P3 — Quality depth

- `ui/` test buildout: mirror upstream pattern (every lib/component core gets a vitest suite; currently 2 files vs legacy 25).
- Playwright visual baseline for `ui/` (upstream has webkit visual regression on legacy app only).
- Resolve harness descriptor UNVERIFIED marks (cline/grok/opencode headless behavior), cursor `rate_limit` stderr detection, claude settings merge-not-clobber.
- Daemon-owned pane resize routed through daemon (known residual from resize fix round).

## P4 — Orchestration formalization (vault research roadmap)

From `agent-teams-orchestration-research`: 1) structured return contract (`status` + BOUNDARIES) 2) all delegation through task board 3) artifact bus (reply = path + status) 4) fan-in completeness gate 5) typed SharedState 6) `depends_on` DAG 7) RetryPolicy. Applies once P1 items 3–7 exist in UI.

## Suggested lane decomposition (when dispatching)

P0 = one hygiene lane (mechanical). P1 items are file-disjoint per feature (each = new `ui/src/components/features/<x>/` + bridge methods) — safe 3–5-pane fan-out. P2 backend ports must be solo-lane (touch shared `lib.rs`/`core/*`).
