# Design Brief — harness-ready, post–BridgeMind analysis

**Status:** Draft · **Date:** 2026-07-29 · **Companion docs:** `docs/PRD.md` (what & why), `docs/BUILD-PLAN.md` (how & when).
**Scope:** UX, information architecture, interaction, and visual direction for the capabilities the PRD decided to **BORROW / ADAPTER / MAKE-REAL** — *not* the SKIP list. This brief is governed by the PRD's ICP-mismatch finding: harness-ready serves terminal-native power users, so the design language stays control-dense and keyboard-first; we do **not** import BridgeMind's beginner-friendly affordances (drag-kanban, voice widget, tutorial onboarding, decorative themes).

> **Provenance notice.** BridgeMind's live UI is not fetchable (Cloudflare). Where this brief references BridgeMind's *look or flow*, it is from indexed screenshots / product-page descriptions and is **INFERRED**; harness-ready's *current* UI is read directly from `ui/` + the COMMAND/MONITORING screenshots and is **EXTRACTED**.

---

## 1. Design intent & principles

1. **Control density over hand-holding.** Surface the real levers (harness, role, autonomy ceiling, worktree, ownership) — power users want knobs, not wizards that hide them.
2. **No fake affordances (hard rule).** A control ships only when its backend exists. Precedent from our own history: the BridgeSwarm "skip-permissions" toggle was *held* because the backend flag didn't exist; the pause control is offered as a one-way RESUME because no `paused` state is recorded (`ui/HANDOFF.md`). Every new control in this brief must name its backing command/state.
3. **Honest state over flattering state.** If the registry is stale or a metric is unavailable, show that — never a simulated value presented as live (the current MONITORING simulation must read as *placeholder* until R-OBS lands, then as *real*).
4. **Local-first, BYO-key identity.** Visual cues should reinforce "this runs on your machine, your keys" — the opposite of a credit-metered cloud SaaS. No credit balances, no upsell surfaces.
5. **Semantic lanes preserved.** Per the prior retheme discipline: brand/accent may evolve, but *state* colours are sacred — `needs` amber, `success` green, `danger` red, `info` cyan. A diff `@@` header or a status pill never wears a brand colour.
6. **Keyboard-first, mouse-optional.** Every action reachable by key; the command palette (R-PALETTE) is acceleration, never the only path.

---

## 2. Current-state UX map (EXTRACTED from `ui/` + screenshots)

- **COMMAND tab** — top bar: `+ New Agent`, `Broadcast`, a voice/`mo1` control, `Delegate`, `Templates`, `Active Agents: N`, `Close Workspace`; hero empty-state ("Agent Command Center", `+ Spawn First Agent`, `Launch a Template`, `Load Demo Fleet`); `Select Workspace` (My Workspace / + New Workspace); `Supported Harnesses` grid (claude/cursor/opencode/codex/commandcode/pi/grok/bash).
- **MONITORING tab** — KPI cards (Fleet CPU / Fleet Memory / Success Rate / Tasks Completed) + "Fleet Resource Usage — Live" area chart (CPU/Memory/Network) + "Task Success Rate — Per Agent" bar chart with hover tooltip. **Currently simulated.**
- **Contract** — everything flows through `ui/src/lib/agentBridge.js`; `tauriAgentBridge.js` maps to Tauri commands; statuses `working | needs_input | blocked | error | starting | idle`; attention surfaced per-pane with an amber border + inline reply.

**Design read:** the skeleton is right for the ICP (command surface + telemetry surface). The work is (a) making telemetry real, (b) enriching the spawn/orchestrate flow with role-cast + ownership, (c) adding a context/memory view, (d) tightening honesty of states.

---

## 3. Information architecture — proposed

Keep the **COMMAND / MONITORING** split (it matches the power-user mental model: *act* vs *observe*). Add one surface; do not add the beginner-ICP ones.

| Tab / surface | Today | Proposed change | Why |
|---|---|---|---|
| **COMMAND** | spawn/broadcast/delegate/templates/workspace/harness grid | Enrich spawn + orchestrate (role cast, file-ownership, preview-before-dispatch); keep harness grid | Core power-user flow |
| **MONITORING** | simulated KPIs + charts | **Make real** (R-OBS); add per-pane resource + outcome attribution; honest empty/loading/stale states | Differentiator |
| **CONTEXT** *(new, optional)* | none | Read-only memory graph + per-project notes (R-MEM); agents read/write via MCP, humans browse | Borrow the *value* of BridgeMemory without the paid gate |
| **Templates manager** | inline | Promote to a local-store manager (R-TEMPLATES) with the same schema | Decouple from Base44 |
| **Command palette** | none | `Cmd/Ctrl+K` over existing handlers (R-PALETTE) | Power-user acceleration |
| **Recipes / Playbooks** | none | Per-harness autonomy profiles + first-run playbooks (R-ONBOARD), via the templates manager / palette (applied 2026-07-29, item 4) | Power-user onboarding — NOT beginner tutorials |
| *Not added* | — | drag-kanban tab, voice widget, **beginner video/course** onboarding, theme gallery | SKIP — beginner ICP / mirage (onboarding *recipes* are added above, not courses) |

**Navigation rule:** top-right tab toggle stays; the palette and keyboard shortcuts (`Alt+↑/↓` workspace cycle already exists per prior work) remain the fast path. No new top-bar clutter — the prior topbar-dedupe audit (13 controls → consolidated primaries + `⋯ More`) set the bar; new actions go into the palette or a `⋯` overflow, not the top row.

---

## 4. Interaction design — per borrowed capability

### 4.1 Spawn & orchestrate with role-cast + file-ownership (R-ORCH)
- **Spawn form** gains an optional **Role** segmented control (Coordinator / Builder / Scout / Reviewer / none) and, when a role implies writes, an **Owned paths** field (glob list). Both are *advisory conventions carried on the task channel* — they do not hard-enforce at the filesystem (we already isolate via worktrees); they document intent and feed the preview.
- **Orchestrate preview** (the existing `dispatch=false` preview) renders the cast as a roster: each pane shows role pill + owned-paths chip + harness + model, plus a **collision check** that warns (amber, not blocking) when two Builders' owned paths overlap. This is the power-user analogue of BridgeMind's mission tree — but text/table-first, not a decorative tree.
- **No fake "auto-ownership enforcement" toggle.** If we later hard-enforce ownership, that is a separate, gated backend feature; the UI must not imply it exists today.

### 4.2 Real monitoring (R-OBS)
- KPI cards keep their layout but bind to real sources; add a subtle **source/age** affordance (e.g. "live · 2s" vs "no data") so a reader can never mistake a flatline for a fake curve.
- Per-agent success bars derive from **task outcome events** (success/needs-human/blocked/error), not random simulation. Hover tooltip = real counts + last-event timestamp.
- **Stale/degraded state:** if the supervisor can't sample a pane (e.g. `state_blind` harness, or the liveness gate from R-GATES is mid-fix), show that pane as "telemetry unavailable" with the reason — *never* interpolate a plausible number.
- Empty fleet = the same honest empty-state grammar as COMMAND.
- **Real-signal definition (cross-LLM review, applied 2026-07-29, item 3):** per-agent series come from **process liveness + per-child CPU/mem + git state + last-tool-failure + human-gate queue depth** — honest, granular health. The differentiator is *truth + granularity*, explicitly **not** a vanity KPI dashboard; if a signal can't be measured, the pane says so.

### 4.3 Context / memory view (R-MEM)
- Read-only graph of memory notes (we already have link + suggested-connection projections). Nodes = notes; edges = links. Click → note body + provenance. This mirrors the *value* of BridgeMemory's graph without a paid gate and without pretending it's a live agent feed.
- Provide an explicit **"agents can read/write this via MCP"** caption so the human understands the dual authorship.

### 4.4 Command palette (R-PALETTE)
- Fuzzy over actions: spawn (per harness), orchestrate, focus pane, open diff, copy id/branch, toggle tab, run template. Each row shows its keybinding. Palette is a *view over existing handlers* — adding an action means adding a handler, not a palette-only shortcut (keeps the no-fake-affordance rule).

### 4.5 Templates manager (R-TEMPLATES)
- Same card/launch UX; storage badge changes from "Base44" to "Local". Import/export JSON for portability (a power-user expectation; also a quiet anti-lock-in signal).

---

## 5. Visual language

- **Keep the terminal aesthetic** (monospace headers, cyan/`--info` accents, dark surfaces) seen in the screenshots — it *signals* the ICP correctly. Do not soften it toward BridgeMind's friendlier gradient look; that look targets beginners.
- **Token strategy:** the sibling `agent-teams` app shipped a 5-skin system (Nothing/Aurora/Atelier/Phosphor/Precision) via `html[data-skin]` token swap. harness-ready's `ui/` is shadcn/Tailwind — propose mapping our palette to **CSS variables / shadcn theme tokens** so a future skin system is a token swap, not a rewrite. *Not in scope to build the skins now*; just don't hard-code colours that would block it.
- **Semantic lanes** (principle 5) get dedicated tokens (`--need/--success/--danger/--info`) that no brand override touches. The monitoring charts use these lanes (CPU=info, memory=need, success=success, error=danger), not brand colours.
- **Density & type:** small-caps section labels + tabular numerals for metrics (matches screenshots); preserve the "command-line as first-class citizen" feel (e.g. the `$ agent fleet status` motif).

---

## 6. States, edges & honesty patterns

- **Liveness-blindness UX (until R-GATES lands):** if a pane is alive to mutations but absent from the read registry, the UI must not show it as "live" in one place and "gone" in another. Prefer a single **"registry reconciling / stale"** badge sourced from one place, over two contradictory truths. This is a direct design consequence of the REQUIRES-HUMAN-DESIGN liveness note.
- **`state_blind` harnesses** (CommandCode, Pi, Grok per `core/harness` descriptors): their telemetry/queue membership is thinner by design — the UI should label the *capability* gap ("hook telemetry limited") rather than show an empty chart that looks like a bug.
- **Pause/resume:** keep RESUME-always (no toggle), per HANDOFF — there is no persisted `paused` field, so a toggle would lie.
- **Empty / error / rate-limited / needs-human:** reuse the ranked-queue semantics; the attention state (amber) is the single source of "this pane needs you".

---

## 7. Anti-patterns to avoid (explicit)

- A **drag-and-drop kanban as the dispatch surface** (beginner-ICP; we dispatch via orchestrate/CLI/task-list).
- A **voice widget** in the command center (orthogonal to the ICP).
- **Decorative onboarding / tutorial carousels / beginner video** (power users read docs; *recipes/playbooks* for activation are the allowed form — R-ONBOARD, applied 2026-07-29, item 4).
- **Credit / usage meters or upsell surfaces** (we are BYO-key/local).
- **Any toggle whose backend flag doesn't exist** (the cardinal sin — see principle 2).
- **Brand colour on state** (diff headers, status pills, KPIs).
- **Simulated data presented without a "simulated/placeholder" affordance** (the current MONITORING state must be visibly placeholder until R-OBS).

---

## 8. Accessibility & keyboard

- Full keyboard reachability for spawn/orchestrate/monitoring/context; visible focus rings (prior work added focus-trap + kanban-nav fixes in the sibling app — mirror that bar).
- Honour `_isTypingTarget`-style guards so xterm/inputs keep their keystrokes (precedent: `Alt+↑/↓` workspace cycling).
- Colour never the sole signal of state (pair the semantic lanes with icons/labels) — matters for the monitoring charts especially.
- Respect `prefers-reduced-motion` for any live-chart animation.

---

## 9. Components & tokens (shadcn)

- Build new surfaces from existing shadcn primitives in `ui/` (`components.json` present): segmented control (role cast), chip (owned paths, role pill), dialog/sheet (template manager, palette), tooltip (chart hover), badge (status/lanes), tabs (top-level).
- Add lane tokens + a `data-skin`-ready variable layer (principle 5 + §5) without building the skin switcher yet.
- Charts: keep the current chart component but feed real series + a "no-data/stale" series state.

---

## 10. Open design questions

1. **CONTEXT tab vs a dock/side-panel** for the memory graph — tab keeps IA flat; a dock keeps it available while commanding. Decide with a quick prototype; default to tab.
2. **Owned-paths collision check severity** — warn (amber) vs block (red). Lean warn: we isolate via worktrees, so overlap is a convention breach, not a catastrophe; blocking would be a fake guarantee unless a backend enforcer exists.
3. **Skin system now or later** — recommend *token-ready now, switcher later*; confirm it doesn't collide with the existing cyan brand identity the screenshots show.
4. **Monitoring sampling rate** — drives the "live · Ns" affordance; tie to whatever R-OBS's supervisor sampling costs (don't over-sample).
5. **Palette keybinding** — `Cmd/Ctrl+K` vs `Cmd/Ctrl+Shift+P` (BridgeMind uses the latter for MCP config); recommend `Cmd/Ctrl+K` to avoid clashing with editor conventions power users already muscle-memory.

---

## 11. Sources

harness-ready (EXTRACTED): `ui/HANDOFF.md`, `ui/README.md`, `ui/AGENTS.md`, `ui/src/**` (Home/Monitoring/components per HANDOFF map), COMMAND + MONITORING screenshots, `core/harness` state_blind descriptors, `docs/REQUIRES-HUMAN-DESIGN-liveness-blindness.md`.
BridgeMind (INFERRED from indexed screenshots / product pages): products/bridgespace (multi-pane + BridgeBoard + mission tree), products/bridgevoice, bridgeswarm (mission tree), themes list, command-palette usage.
Vault: 2026-06-18 bridgemind-ui-retheme handover (semantic-lane discipline, topbar-dedupe, split-tree tiling, no-fake-affordance precedent); agent-teams-entity (skins system, focus-trap/kanban-nav bar).
