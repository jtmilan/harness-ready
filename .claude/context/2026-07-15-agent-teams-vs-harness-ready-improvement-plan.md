# Context Brief — agent-teams vs harness-ready comparison + improvement plan

**Date:** 2026-07-15
**Intent:** plan — comprehensive review comparing `~/Personal/harness-ready` (fork) against `~/Personal/agent-teams` (upstream), scout features worth porting.

## Topic / Intent

harness-ready is a fork of agent-teams (EXTRACTED — `personal/daily/2026-07-14.md`: "Harness Ready fork: grok harness, coordinator prompting, resize bugs, stable-cert install shipped"). Upstream agent-teams is at v0.9.0 with a long feature lead. Goal: gap analysis + prioritized port/improvement plan.

## Best Practices Found

- **6-component harness frame** (EXTRACTED — `.bridgememory/Coding Harness.md`, canonical `shared/topics/coding-harness-components`): tool use, planning, memory, reflection, multi-agent orchestration, environment isolation. Evaluate every feature port against which component it strengthens.
- **Hook-driven state, no PTY scraping** (EXTRACTED — `personal/decisions/2026-06-02-agent-teams-architecture-locks.md`): agent state (`working` / `waiting{approval|question|turn_end}` / `error` / `rate_limited`) comes from structured hook JSON. Locked decision; any harness-ready feature must respect it.
- **Structured return contract** (EXTRACTED — `.bridgememory/agent-teams-orchestration-research.md`): #1 leverage = `status` (`done`|`blocked`|`needs-input`) + mandatory `## BOUNDARIES` on worker reports, through append-only task board. Priority roadmap: return contract → delegation via task board → artifact bus (reply = path + status) → fan-in completeness gate → typed SharedState → `depends_on` DAG → RetryPolicy.
- **Coordinator + isolated worktree panes = Claude Agent SDK model** (EXTRACTED — same card): formalize, don't adopt new orchestration runtimes.
- **Ranked "who-needs-me" queue** as the MVP wedge (EXTRACTED — architecture-locks decision).

## Industry-Standard Architecture / Patterns

- Fan-out: AutoGen pub/sub ↔ `team_orchestrate`; fan-in: CrewAI task context ↔ `team_synthesize` → `final.md`; control edges: LangGraph `Command(goto)` ↔ `task_create`/`task_transition`; lifecycle: Temporal states ↔ kanban (EXTRACTED — orchestration research card).
- Dispatch gates OFF by default; triple-gate for autonomy (mutations + autonomy + delegate-live) (EXTRACTED — `personal/entities/agent-teams-entity.md`, Flywheel v0.6.6).
- Per-pane harness/model selection across 5 harnesses (claude/cursor/codex/commandcode/opencode), all validated 2026-06-08 (EXTRACTED — entity page).

## Anti-Patterns

- **Never `git pull` on multi-agent fork repos** — breaks fork invariant; use explicit `git merge` (EXTRACTED — `personal/topics/multi-agent-lane-gotchas.md`, Phase 27 incident). Directly applicable: harness-ready is exactly such a fork.
- Hard-coded pane caps → false spawn failures; cap must be configurable, dispatch gate explicit (EXTRACTED — lane-gotchas).
- Account drift on `gh pr merge` across sessions (EXTRACTED — `personal/topics/agent-teams-github-merge-gotchas.md` ref; merge as jtmilan).
- Interactive TUI as state source (PTY scraping) — the original RETHINK blocker (EXTRACTED — architecture-locks).

## Open Questions

- AMBIGUOUS: how far has harness-ready diverged from upstream (fork point, cherry-picks)? Scout agents mapping both repos now; git merge-base will confirm.
- AMBIGUOUS: whether upstream v0.8–v0.9 skins/Settings/Flywheel features are wanted in harness-ready or the fork intends a leaner scope (user preference — surface in plan).
- INFERRED: harness-ready active WIP (ws60935x4 round 2/3, ws92431x3 UI-parity lanes) already ports upstream UI parity — plan must not collide with in-flight lanes.

## Scout Findings (2026-07-15, two Explore agents + tree diff)

- EXTRACTED (tree diff): repos share identical `core/*` crate layout; only ~27 source files differ; `git merge-base` empty — **unrelated git histories**, so ports are manual re-implementation, never cherry-pick/pull.
- EXTRACTED (scout): harness-ready ships a NEW React frontend (`ui/`, Base44/shadcn/xterm, 106 files) wired through `ui/src/lib/tauriAgentBridge.js`; upstream ships vanilla `app/src/main.js` (~507 KB). Backend exposes **155 Tauri commands**; `ui/` wires only ~10 (spawn, close, send_input, resize, read_output_delta_batch, list_queue, dead_pane_ids, pause/resume, set_pane_roles).
- EXTRACTED: harness-ready-only additions — grok as 8th harness, PTY-resize/narrow-wrap fixes, tiling-tree full-id serialization, template→workspace tabs, instant pane close, delete-workspace.
- EXTRACTED: upstream-only since fork — memory global scope + run-capture flywheel (#288), Second Brain governance + planetary graph (#287), external-spawn cap threading (#289), Cline state notes, Sentry (#266), skins, Settings→Trusted Repos UI, dashboard/, docs/, prototype/, tests/, bridge/, `.github` CI, README/LICENSE.
- EXTRACTED: fork residue — `DEFAULT_REPO` hardcoded `/Users/jeffrymilan/agent-teams-devapp-workspace` (`ui/src/lib/tauriAgentBridge.js:78`), `SESSION_ID` hardcoded (`Home.jsx:21`), sidecars/state-root still named `agent-teams`, no `.github/workflows/` (test-all.sh references missing ci.yml), `ui/` has only 2 test files vs 25 legacy suites.
- EXTRACTED: existing parity plan at `ui/.claude/context/2026-07-13-feature-parity-plan.md` (lanes L1–L7, mostly unbuilt).

## Sources

- `~/Memory/personal/entities/agent-teams-entity.md`
- `~/Memory/personal/decisions/2026-06-02-agent-teams-architecture-locks.md`
- `~/Memory/personal/topics/multi-agent-lane-gotchas.md`
- `~/Memory/personal/daily/2026-07-14.md`
- `~/Memory/.bridgememory/Coding Harness.md`
- `~/Memory/.bridgememory/agent-teams-orchestration-research.md`
- graphify vault graph query (381-node BFS neighborhood, 2026-07-15)
