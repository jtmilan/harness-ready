# Build Plan — harness-ready, post–BridgeMind analysis

**Status:** Draft · **Date:** 2026-07-29 · **Companion docs:** `docs/PRD.md` (what/why), `docs/DESIGN-BRIEF.md` (UX/IA/visual).
**How to read this:** each phase maps a PRD requirement to the exact crates/files it touches, in dependency order, with the test gate, an effort size, and an explicit **HD** (REQUIRES-HUMAN-DESIGN) flag where a phase crosses a mutation / schema / service boundary. Phases flagged HD **stop for a design note before any code** — that is a coordinator hard rule, not a scheduling nicety.

> **Grounding.** Crate/file references below are read from the repo as it stands: the cargo workspace `core/{state-adapter, supervisor, harness, flywheel, mcp, daemon, task, ringbuf, memory, roles, agent}` + `harness-ready-mcp`, the Tauri shell `app/src-tauri`, and the React front-end `ui/` (shadcn/Vite, over `ui/src/lib/agentBridge.js` → `tauriAgentBridge.js`). [EXTRACTED: Cargo.toml, ui/HANDOFF.md.] The repo already uses git worktrees (`.agent-teams-worktrees/` exists at root), so per-slice isolation is available.

---

## 0. Operating rules (non-negotiable)

1. **Test gate is read, never redefined.** The coordinator does not set or weaken the gate. Each phase runs the repo's *existing* canonical checks, discovered from config, not invented here: `cargo test --workspace` (core crates + `harness-ready-mcp`); the app shell's own suite (see `app/` — vitest per `app/vitest.config.js`, plus Playwright/visual tests under `app/tests-visual` + `app/playwright.config.js`); and the UI's `npx vitest run` + `vite build` (see `ui/package.json`, `ui/vitest.config.js`). Build via whatever entry the repo already documents (`app/README.md`, `ui/README.md`, `scripts/`); **do not add new build paths.** A phase is not done until the checks it affects are green *by these commands*.
2. **One logical slice per PR; human deep-review; never auto-merge; commit-on-ask.** "The AI wrote it" is not a justification — every change must be explainable in the PR.
3. **Stay in file boundaries.** Debt spotted outside a phase's owned files is *filed as a note*, not silently fixed (Boy-Scout, not bulldozer).
4. **No fake affordances** (Design Brief §1.2): ship a control only with its backing command/state.
5. **ICP-drift guard:** every PR description must answer "does this pass the PRD §6 transfer matrix?" — i.e. does it serve a power user, or is it beginner-ICP creep? Review rejects the latter.
6. **HD phases emit a `docs/REQUIRES-HUMAN-DESIGN-<slug>.md`** (matching the existing convention) and stop; they do not implement speculatively.

---

## 1. Phase map & critical path

```
Phase 0 (HD)  trust gates: liveness-blindness + branch-wire   ──┐
Phase 2       local template store (independent)              ──┤  (parallel track)
Phase 1a      real monitoring: sampling + metrics source      ──┤
Phase 1b      monitoring: queue/outcome attribution           ──┘ needs Phase 0 liveness
Phase 3       memory persistence + graph view (HD-if-contract)   after memory contract review
Phase 4       role-cast orchestration + ownership (channel-B)    independent; cleaner after Phase 0
Phase 5       command palette (optional)                         after handlers stable
Phase 6       publish skills catalog (optional, independent)
Phase 7 (HD)  read-write shared-state MCP layer                  last; biggest boundary change
```

Critical path = **0 → 1b** (honest liveness unlocks correct per-agent attribution) and **0 → 3/4** (trust + queue clarity de-risk the borrow/adapter work). Phases 2 and 6 are free parallel tracks. Phase 7 is deliberately last and gated. **Phase 4b (R-ONBOARD onboarding recipes, applied 2026-07-29, item 4) joins 2 and 6 as a free parallel track.**

---

## 2. Phases

### Phase 0 — Close the trust gates (R-GATES) · **HD** · effort L
- **Goal:** one source of liveness; branch info wires through. This unblocks honest monitoring and queue correctness.
- **Owned files (indicative):** `app/src-tauri/src/lib.rs` (startup `live_registry_write` empty-clobber; multi-instance path), `core/mcp` (registry vs `sups` filter in `compute_queue_identified`), `core/daemon` (handlers liveness resolution), `harness-ready-mcp/src/read_output.rs` + `main.rs` (live-scrollback + queue membership), and the branch wire-through surface.
- **Stop condition:** produce/refresh `docs/REQUIRES-HUMAN-DESIGN-liveness-blindness.md` with the *chosen* single-source-of-liveness design (in-memory `sups` vs disk registry vs a reconciled view) and the multi-instance rule, **then** implement. Do not code from the investigation notes alone.
- **Gate:** the existing read-only-form unit tests (`branch_name_args`, `is_readonly_git`, etc.) stay green; add a regression test that a pane alive to mutation is visible to the read/queue path; `cargo test --workspace`.
- **Risk:** multi-instance footgun (Instance B clobbering Instance A's registry). Mitigation: design the instance-lock + registry-write ordering explicitly.
- **DoD:** liveness parity proven by test; branch chip reliable; no contradictory live/gone states in UI.

### Phase 1a — Real monitoring: sampling + metrics source (R-OBS) · effort M
- **Goal:** replace simulated series with real per-process resource samples + a real outcome-event stream.
- **Owned files:** `core/supervisor` (per-PTY child CPU/mem sampling), `core/daemon` (surface samples), `ui/src/lib/agentBridge.js` + `ui/src/lib/tauriAgentBridge.js` (subscribe to real snapshots), `ui/src/lib/monitorData.js` (strip simulation from the live path — keep only as an explicit, labelled *demo* fixture if `Load Demo Fleet` needs it).
- **Gate:** `ui` vitest + `vite build`; app vitest; sampling has a documented rate/cost and a "no-data/stale" series state (Design Brief §4.2).
- **Risk:** over-sampling cost; `state_blind` harnesses (CommandCode/Pi/Grok per `core/harness`) can't yield hook telemetry — UI must show "telemetry limited", not a fake curve.
- **DoD:** KPI cards + area chart bind to real data with a visible source/age affordance; `state_blind` panes labelled honestly; **per-agent series derive from real signals — process liveness + per-child CPU/mem + git state + last-tool-failure + human-gate queue depth (cross-LLM review, applied 2026-07-29, item 3); no interpolated/simulated curves; the value is honest granular health, not a vanity dashboard.**

### Phase 1b — Monitoring: queue/outcome attribution (R-OBS) · effort S–M · **depends Phase 0**
- **Goal:** per-agent success bars reflect real task outcomes (success / needs-human / blocked / error), tie-broken/ordered exactly like the ranked queue.
- **Owned files:** `ui/src/pages/Monitoring.jsx`, the tauri adapter's `_poll()` projection (`QueueRow` fields), and whatever outcome event Phase 0 stabilises.
- **Gate:** UI vitest + build; a test that the bar for a pane matches its real outcome events, not a random value.
- **DoD:** hover tooltip shows real counts + last-event timestamp; empty/degraded states use the honest grammar.

### Phase 2 — Local template store (R-TEMPLATES) · effort S–M · independent
- **Goal:** decouple templates from Base44; keep the `AgentTemplate` schema.
- **Owned files:** `ui/src/components/command/templates/*`; a new local store module (SQLite or JSON per `ui/HANDOFF.md`); remove the Base44 entity dependency for templates.
- **Gate:** UI vitest + build; round-trip save/load/launch test; import/export JSON (portability + quiet anti-lock-in).
- **Risk:** schema drift from the Base44 `base44/entities/AgentTemplate.jsonc` — pin the schema in a test so the swap is behaviour-preserving.
- **DoD:** storage badge reads "Local"; launch flow unchanged in UX.

### Phase 3 — Memory persistence + graph view (R-MEM) · effort M · **HD if write-contract changes**
- **Goal:** compounding, linkable, always-available context (the *value* of BridgeMemory), local-first, **never tier-gated**; plus a read-only graph view.
- **Owned files:** `core/memory`, `core/mcp` (memory tools / projection), a new read-only UI graph view (nodes = notes, edges = links + suggested connections).
- **Stop condition (HD):** if this changes the memory store's *write contract* or its injection surface, write a design note first (the broker SKILL.md's channel-A/B + injection-safety rules apply). If it only adds a read view + uses existing `create/search/get` tools, no HD.
- **Gate:** `cargo test` on `core/memory` + `core/mcp`; UI build; a test that a note written in session N is retrievable in session N+1.
- **DoD:** graph browsable; agents retrieve via MCP without re-deriving; caption states dual (human+agent) authorship.

### Phase 4 — Role-cast orchestration + file-ownership (R-ORCH) · effort M · channel-B only
- **Goal:** cast panes as Coordinator/Builder/Scout/Reviewer and declare non-overlapping owned paths; show a collision *warning* in the orchestrate preview.
- **Owned files:** `core/roles` (role semantics for the cast), the orchestration prompt builder in `core/mcp` (carry role + ownership on the **task channel**), `ui` spawn/orchestrate wizard (role segmented control + owned-paths field + roster preview with overlap warn).
- **Hard constraint:** ownership/role text lives on channel B (task) **only** — never templated into the persona SSOT (`core/roles::persona()`), preserving the injection-safety property the broker SKILL.md documents. If a design needs channel A, it becomes HD and stops.
- **Gate:** `cargo test` on `core/roles`/`core/mcp` (incl. a byte-identity / no-injection regression if one exists); UI build; a preview test that overlapping owned paths warn (amber) and do not block.
- **DoD:** preview shows role pill + owned-paths chip + harness + model per pane; we already auto-isolate via worktrees, so this *documents* intent rather than faking enforcement.

### Phase 4b — Power-user onboarding recipes / playbooks (R-ONBOARD) · effort S–M · independent (applied 2026-07-29, item 4)
- **Goal:** ship per-harness **autonomy profiles + playbooks** (one-click first-run recipes) so the activation curve doesn't kill adoption; **not** beginner video courses (those stay SKIP).
- **Owned files:** `ui` (recipes surface reusing the templates manager / palette); a local recipes store (JSON/SQLite, same local-store direction as R-TEMPLATES).
- **Gate:** UI vitest + build; a recipe round-trips (load → apply gates → launch); recipes are versionable local files.
- **Risk:** scope creep into a tutorial hub — guard with the ICP test; docs/recipes only, no video/course UI.
- **DoD:** a new operator can onboard a 3rd harness via a recipe without reading source.

### Phase 5 — Command palette (R-PALETTE) · effort S–M · optional
- **Goal:** `Cmd/Ctrl+K` fuzzy over *existing* handlers (spawn per harness, orchestrate, focus pane, open diff, copy id/branch, tab toggle, run template), each row showing its keybinding.
- **Owned files:** `ui` (new palette component over the existing handler set).
- **Rule:** palette-only shortcuts are forbidden — every row maps to a real handler (no-fake-affordance).
- **Gate:** UI vitest (palette resolves each action to a handler) + build; keyboard reachability + focus-trap check.
- **DoD:** every action reachable by key with or without the palette.

### Phase 6 — Publish skills catalog (R-SKILLS-OSS) · effort S · optional, independent
- **Goal:** publicly browsable index of `.claude/skills` (reuse polyglot-expert/broker artifacts where relevant); MIT where appropriate.
- **Owned files:** a docs/site index + repo `.claude/skills`.
- **Gate:** links resolve; no secrets/PII in published bodies (treat skill text as publishable surface — review for leakage).
- **DoD:** a landing page lists skills with provenance.

### Phase 7 — Read-write shared-state MCP layer (R-MCP-RW) · **HD** · effort L · last
- **Goal:** a BridgeMCP-*like* shared task/context layer agents can read **and** write under gates, complementing the current read-only `core/mcp` projection.
- **Stop condition (HD):** this crosses the mutation boundary — design the write gate, the injection-safety of any agent-authored content, and the channel-A/B split **before** code. Mirror the existing `REQUIRES-HUMAN-DESIGN` convention and the broker's "mutations gated, read-only by default" model.
- **Owned files (after design):** `core/mcp`, `core/task`, `core/supervisor`, `harness-ready-mcp`.
- **Gate:** security-focused review (the `security` polyglot posture: enumerate untrusted inputs/sinks, fail closed); `cargo test --workspace`; no write path enabled by default.
- **DoD:** writes are gated-OFF by default and armed only by an explicit human decision; read surface unchanged for existing consumers.

---

## 3. Branching, PR & isolation strategy

- **Branch per phase** (or per slice within a phase), off current main; the repo's existing `.agent-teams-worktrees/` workflow is available if a harness implements a phase.
- **PR scope = one phase's owned files.** Out-of-boundary debt → a filed note, not a drive-by fix.
- **Review checklist (every PR):** (a) passes the ICP-drift guard (§0.5); (b) no fake affordance introduced; (c) semantic lanes intact if UI touched; (d) the phase's canonical gate green *by the repo's commands*; (e) HD phases link their design note.
- **Merge:** human only; never auto-merge; commit-on-ask (global rule).

---

## 4. Out of scope / deferred (mirrors PRD §6 + §8)

- **SKIP (beginner-ICP / mirage):** BridgeBoard drag-dispatch, BridgeVoice, **beginner video/course** hub (power-user onboarding *recipes* are NOT skipped — Phase 4b / R-ONBOARD), vendor benchmark, app-theme switcher, "three modes", cloud-first redesign.
- **DEFER (our own possible future, gated):** container/microVM sandbox per worktree — *not* a BridgeMind feature to copy (refuted), and if we ever want stronger isolation it is its own HD design, not part of this plan.
- **Not building toward the vaporware watchlist** (PRD §8): cloud-first arch, auto-worktree-per-agent, container sandbox, three modes, BridgeMind-side fleet dashboard.

---

## 5. Risks & mitigations

| Risk | Mitigation |
|---|---|
| ICP drift (shipping beginner features) | §0.5 review gate; PRD §6 matrix is the test |
| ICP hypothesis hinge (spine wrong if overlap) | treat ICP-mismatch as working hypothesis + 12-month re-review trigger; external-model confidence 42–62 (cross-LLM review, applied 2026-07-29, item 2) |
| Provenance ceiling (AMBIGUOUS specs) | verify against a real browser / Google cache *before* coding any feature that depends on an AMBIGUOUS BridgeMind detail |
| HD load stalls the plan | Phase 0 + 7 are the only hard stops; everything else proceeds; design notes are scoped, not open-ended |
| Weakening/renaming the test gate | forbidden (§0.1); gate is read from repo config |
| Monitoring over-sampling / fake-looking flatlines | documented rate + explicit no-data/stale series (Design Brief §4.2) |
| Memory/orchestration injection regressions | channel-B-only rule + byte-identity/no-injection regression tests |
| Building a mirage | §4 + PRD §8 watchlist reviewed at each phase gate |

---

## 6. Definition of done (per phase)

A phase is done when: owned files changed within boundary; the phase's canonical gate green by the repo's commands; PR carries the §3 checklist + (for HD) the design note; no fake affordance; no out-of-boundary drive-bys; and the feature visibly serves a power user (ICP guard). The whole plan is done when Phases 0–4 ship (the must-haves: trust + real monitoring + local templates + memory + role-cast), with 5–7 landing opportunistically and 7 only after its HD design.

---

## 7. Sources

Repo ground truth: `Cargo.toml` (workspace members), `ui/HANDOFF.md` (contract, simulation-to-strip, Base44 templates, pause/resume semantics), `ui/README.md` / `ui/AGENTS.md` (Vite/shadcn/Base44 build), `docs/REQUIRES-HUMAN-DESIGN-liveness-blindness.md` + `-branch-wire-through.md` (open HD gates, root-cause evidence), `core/harness` (state_blind descriptors), `app/vitest.config.js` / `app/playwright.config.js` / `ui/vitest.config.js` (gate discovery), `.agent-teams-worktrees/` (worktree workflow present).
Companion analysis: `docs/PRD.md`, `docs/DESIGN-BRIEF.md`; `~/.claude/context/2026-07-29-bridgemind-competitive-deepdive.md`; polyglot-broker `SKILL.md` + `references/domains.md` (channel-A/B + injection-safety + closed-enum rules applied to Phases 3/4/7).
