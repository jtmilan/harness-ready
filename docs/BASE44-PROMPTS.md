# Base44 Prompt Pack — harness-ready feature / UI / aesthetics suggestions

**Purpose.** Paste these into Base44 (the builder behind the `ui/` app) so it reads the four research docs you produced and returns *grounded* feature suggestions, UI/IA redesigns, and aesthetic improvements for **harness-ready** — without re-introducing the things the research said to skip, and without inventing controls that have no backend.

**How to use.**

- Run **P0 first** (it loads the docs + repo and locks the rules + output format). Then run **P1**, **P2**, **P3** (separate Base44 messages or sessions — each is self-contained). Run **P4** as a final self-review pass over whatever P1–P3 produced. Use **P5** to implement one chosen suggestion.
- Each prompt is self-contained: copy everything between the `COPY FROM HERE` / `COPY TO HERE` markers.
- **First pass = proposals only** (P1–P3). Do not let Base44 write code until you pick an item and run P5.
- Paths below are **repo-root-relative**. The four docs are the source of truth; the `ui/` source is the thing being improved.

**Read-first file list (Base44 should open these):**
`docs/RESEARCH-SYNTHESIS.md` (the consensus + the cross-LLM punch-list), `docs/PRD.md` (requirements + the feature spec + the transfer matrix), `docs/DESIGN-BRIEF.md` (UX/IA/visual direction), `docs/BUILD-PLAN.md` (phasing + the test gate), then the app: `ui/HANDOFF.md` (the `agentBridge.js` contract + statuses + what is simulated), `ui/src/lib/agentBridge.js`, `ui/src/lib/tauriAgentBridge.js`, `ui/src/lib/agentTypes.js`, `ui/src/pages/Home.jsx`, `ui/src/pages/Monitoring.jsx`, `ui/src/components/command/**`, `ui/components.json`, `ui/tailwind.config.js`. If Base44 can ingest images, also give it the COMMAND-tab and MONITORING-tab screenshots; otherwise rely on `DESIGN-BRIEF.md` §2 (current-state map).

---

## P0 — Ingest, lock the rules, set the output format



You are a senior product + UI engineer for **harness-ready**, a macOS "Agent Command Center" that supervises interactive CLI coding-agents (Claude Code, Cursor, OpenCode, Codex, CommandCode, Pi, Grok, bash), each in its own git-worktree PTY, surfaced through a ranked "who-needs-you" queue. The front-end is this Base44/Vite/shadcn app over a single `agentBridge.js` contract.

STEP 1 — READ these repo files in full and treat them as the source of truth (quote section ids when you cite them): docs/RESEARCH-SYNTHESIS.md, docs/PRD.md, docs/DESIGN-BRIEF.md, docs/BUILD-PLAN.md, ui/HANDOFF.md, ui/src/lib/agentBridge.js, ui/src/lib/tauriAgentBridge.js, ui/src/lib/agentTypes.js, ui/src/pages/Home.jsx, ui/src/pages/Monitoring.jsx, ui/src/components/command/**, ui/components.json, ui/tailwind.config.js.

STEP 1b — TRANSFER CONTEXT: these prompts run in the Base44 web builder (app.base44.com), but every result is hand-transferred by a human into the LOCAL harness-ready ui/ checkout. Therefore keep all paths ui/-relative, keep diffs minimal and portable, prefer editing existing files/components over inventing new structures, and never reference a file that isn't in the read-first list without explicitly proposing to create it. When you later implement (P5), your output must map 1:1 to the local ui/ tree so it pastes cleanly.

STEP 2 — Adopt these RULES (non-negotiable; they come from the product's own research; never relax them):
R1 ICP = terminal-native POWER USERS who orchestrate agents — NOT non-coder MVP builders. Test every idea: "does a terminal-dwelling power user want this?" If it only helps a beginner, reject it.
R2 Honor the PRD §6 / Synthesis §3 transfer matrix: BORROW the memory-graph idea + the OSS-skills posture; ADAPTER role-cast orchestration + multi-pane UX; MAKE-REAL monitoring; SKIP drag-dispatch kanban, voice dictation, beginner tutorial/video hub, vendor benchmark, app-theme gallery, "three modes", cloud-first. NEVER propose a SKIP item.
R3 No fake affordances: every control you propose must name the backing agentBridge.js method / Tauri command / state field. If none exists, label it "NEEDS-BACKEND — do not ship this control until the backend lands" (precedent: a skip-permissions toggle was held for exactly this; RESUME is one-way because no paused state is recorded).
R4 Monitoring = honest, granular per-agent health from REAL signals (process liveness + per-child CPU/mem + git state + last-tool-failure + human-gate queue depth), NOT a vanity KPI dashboard. Define explicit empty / loading / stale / state_blind / needs-human / error states; never interpolate or show simulated data as live.
R5 Visual: keep the dark, monospace, cyan/`--info` terminal aesthetic; map colors to CSS variables / shadcn theme tokens (so a future skin is a token swap); preserve semantic lanes — `--need` amber, `--success` green, `--danger` red, `--info` cyan (brand/accent may move, state colors never); tabular numerals for metrics; small-caps labels. Do NOT import BridgeMind's friendly pink/violet gradient look (it targets beginners).
R6 Local-first / BYO-key identity: no credit meters, no usage counters, no upsell surfaces, no cloud-account framing.
R7 Keyboard-first: every action reachable by key; the palette accelerates, never the only path; honor typing-target guards so terminals/inputs keep their keystrokes.
R8 a11y: visible focus rings; color never the sole signal of state; respect prefers-reduced-motion; AA contrast.
R9 Honesty: treat BridgeMind marketing as unverified; do NOT assume a REST API, webhooks, standalone CLI, or fleet-KPI dashboard exist (they don't).
R10 Build inside ui/ over agentBridge.js; do not redefine the backend contract — if a feature needs a new contract method, propose the method signature as a NOTE, do not fake it.

STEP 3 — Adopt this OUTPUT SCHEMA and use it for every suggestion in later prompts:

- id (e.g. F-OBS-1 / U-CMD-2 / A-VIS-1)
- title
- implements: <doc section and/or R-* requirement>
- icp_fit: <one line: why a power user wants this + which transfer-matrix verb (BORROW/ADAPTER/MAKE-REAL)>
- ux: <screen + interaction + the states you cover>
- backend_dep: <agentBridge.js method(s) / Tauri command(s); mark EXISTS or NEEDS-BACKEND>
- fake_affordance_check: PASS or flagged (and why)
- components: 
- visual: <tokens / semantic lanes used>
- effort: S / M / L
- priority: P0 / P1 / P2
- a11y_keyboard: 

STEP 4 — Produce an ALIGNMENT MAP (this prompt's only deliverable): a table with one row per current ui/ screen/component (Home command surface, Monitoring, empty-state, NewAgentOverlay, templates, agent panes) stating (a) which doc section governs it, (b) which R-* it currently satisfies or misses, (c) any FAKE-AFFORDANCE you find today (a control with no backing method/state), (d) which simulated data is presented without a "placeholder" affordance. End with a short list of the highest-value unimplemented R-* items. Do not write any application code yet.



---



## P1 — Feature suggestions (PRD-driven, ICP-safe)



Context: you have read the four docs + the ui/ source (see P0). Re-apply RULES R1–R10 from P0 in full.

Produce a prioritized list of FEATURE suggestions that implement these PRD requirements in the ui/ app: R-OBS (real monitoring), R-TEMPLATES (local template store UX), R-MEM (memory graph view), R-ORCH (role-cast + file-ownership in spawn/orchestrate), R-ONBOARD (per-harness onboarding recipes/playbooks), R-PALETTE (command palette), R-SKILLS-OSS (skills catalog surface). SKIP R-GATES and R-MCP-RW (those are backend human-design items, not UI-only work) — but you MAY propose the UI surface they will eventually need, clearly tagged NEEDS-BACKEND.

For each suggestion use the P0 OUTPUT SCHEMA. Extra constraints:

- Every proposed control passes the fake-affordance check (R3). For monitoring (R-OBS) bind to the real-signal list in R4 and define the stale/state_blind/empty states; do not propose a vanity aggregate dashboard.
- For R-ORCH, role + owned-paths are ADVISORY conventions on the task channel; the orchestrate preview must show a non-blocking collision WARNING when two Builders' owned paths overlap; do NOT propose an "auto-enforce ownership" toggle (no backend enforcer exists).
- For R-ONBOARD, propose recipes/playbooks (per-harness autonomy profiles + first-run steps) reusing the templates manager / palette — NOT video or a beginner course UI (that is a hard SKIP).
- Do NOT propose anything on the SKIP list (R2). If a tempting idea is on it, list it once under a "rejected + why" footer instead.
End with a one-paragraph sequencing note consistent with BUILD-PLAN.md (R-OBS + R-GATES first; R-TEMPLATES/R-MEM/R-ORCH next; R-PALETTE/R-SKILLS-OSS opportunistic). No application code yet.



---



## P2 — UI / information-architecture redesign (Design-Brief-driven)



Context: you have read the four docs + the ui/ source (see P0). Re-apply RULES R1–R10 from P0 in full, especially R3 (no fake affordances), R4 (honest monitoring states), R5 (lanes + terminal aesthetic), R7 (keyboard-first).

Produce UI/IA redesign proposals per DESIGN-BRIEF.md §3–§4. Cover: (1) the COMMAND / MONITORING split and the optional new CONTEXT surface (read-only memory graph); (2) the spawn + orchestrate flow with role-cast segmented control, owned-paths field, and the preview roster with collision warning; (3) the command palette over EXISTING handlers only (no palette-only shortcuts); (4) the Recipes/Playbooks manager (R-ONBOARD) reusing the templates manager; (5) the attention/queue semantics and per-pane inline reply.

For each screen/flow use the P0 OUTPUT SCHEMA and ADD: a complete STATES list (empty / loading / working / needs-human / blocked / error / stale / state_blind / registry-reconciling), and a KEYBOARD map. Component-level: name the shadcn primitives (segmented control, chip, dialog/sheet, tooltip, badge, tabs) per DESIGN-BRIEF.md §9. Honor the navigation rule (no new top-bar clutter; new actions go to the palette or a `⋯` overflow). Honor the pause/resume rule (RESUME-always, no toggle, because no paused state exists).

LIVING, CRAFTED, NOT GENERIC (apply to every screen you propose): this is instrumentation for people who live in terminals — it must read as a command center that is *running*, not a marketing page. Give it perceptible life: a layered/ambient background (a faint technical grid, scanline, topographic contour, or slow telemetry field — NOT aurora blobs), live micro-interactions (hover/press/focus feedback on panes, buttons, queue rows, harness tiles; FLIP/animated transitions when the roster or queue reorders; purposeful, non-gratuitous chart updates), and strong contrast in type size/weight (a distinctive technical/monospace DISPLAY face for headers, section labels, and metrics, paired with a readable body face — never a single Inter/Geist/Roboto/Arial family everywhere). All motion honors `prefers-reduced-motion`. Open each screen with whatever is most characteristic of a fleet-control surface (the live queue / the fleet / the terminals), not a generic hero.
AVOID these generic AI-UI defaults unless the user asks: a centered hero trio (headline + subtitle + CTA stacked/centered); a row of 3–4 equal feature cards; gradient-painted headlines; indigo/violet/pink gradients; site-wide glassmorphism; blanket `rounded-2xl`; aurora-blob backgrounds; cream/beige palettes with terracotta + serif type; near-black backgrounds with a single neon/acid accent; dense broadsheet column grids with hairline rules.

No application code yet.



---



## P3 — Aesthetics / visual system



Context: you have read the four docs + the ui/ source (see P0). Re-apply RULES R5, R6, R8 (and R3) from P0 in full.

Produce a VISUAL-SYSTEM spec and a concrete restyle plan for the current COMMAND and MONITORING screens, per DESIGN-BRIEF.md §5–§6. Deliver: (1) a token system — CSS variables / shadcn theme tokens with the four semantic lanes (`--need/--success/--danger/--info`) on dedicated tokens that no brand override touches, plus a `data-skin`-ready variable layer (token-ready now, switcher later); (2) typography — monospace headers, small-caps section labels, tabular numerals for all metrics; (3) density + spacing guidance that keeps the control-dense terminal feel; (4) motion rules honoring prefers-reduced-motion; (5) an a11y pass (focus rings, AA contrast, color-never-sole-signal — pair every lane color with an icon/label, critical for the monitoring charts).

Then give before→after component specs (no code, just spec) for: the KPI cards, the fleet resource chart, the per-agent success bars, the supported-harnesses grid, and the empty-state — each stating current look, proposed look, tokens/lanes used, and the honest empty/stale/no-data variant. EXPLICITLY do not import BridgeMind's friendly gradient look; the aesthetic must keep signaling "this is for people who live in a terminal". Make the visual system carry life: define an ambient/background-layer token (subtle grid/scanline/telemetry field — never aurora blobs), motion tokens (durations/easings) used consistently for hover/press/FLIP/chart updates with a reduced-motion off-switch, and a deliberate type pairing — a distinctive technical/monospace DISPLAY face for headers/labels/metrics plus a readable body face — with bold type-scale ratios (large tabular metrics vs small-caps labels vs body). Apply the same AVOID list as P2 (no centered hero trio, no 3–4 equal feature cards, no gradient headlines, no indigo/violet/pink gradients, no blanket glassmorphism/`rounded-2xl`, no aurora blobs, no cream/terracotta/serif, no near-black + single neon, no broadsheet hairline grids). The result should feel crafted and alive — a portfolio-grade command center — while staying unmistakably terminal-native. Use the P0 OUTPUT SCHEMA for any discrete visual suggestion (ids prefixed A-VIS-). No application code yet.



---



## P4 — Self-review checklist (run before you finalize any output)



Before you return ANY suggestion list (from P1, P2, or P3), run this checklist against every item and FIX or DROP violations; then append a one-line "self-review" footer reporting how many items you dropped and why.
[ ] ICP test (R1): would a terminal-native power user want this, or only a beginner? Drop if beginner-only.
[ ] Transfer-matrix test (R2): is this on the SKIP list (drag-kanban, voice, beginner tutorial/video, vendor benchmark, theme gallery, three modes, cloud-first)? Drop if yes.
[ ] Fake-affordance test (R3): does every control name a backing agentBridge.js method / Tauri command / state? If not, tag NEEDS-BACKEND or drop the control.
[ ] Monitoring-honesty test (R4): any monitoring item uses the real-signal list and defines stale/state_blind/empty? Any simulated data shown without a placeholder affordance? Fix.
[ ] Visual test (R5): semantic lanes intact (state colors never wear brand)? terminal aesthetic kept (no friendly gradient import)? tokens/lanes named?
[ ] Local-first test (R6): no credit/usage/upsell/cloud-account surface introduced?
[ ] Keyboard test (R7): every action key-reachable; palette-only shortcuts absent; typing-target guards honored?
[ ] a11y test (R8): focus rings, AA contrast, color-not-sole-signal, reduced-motion?
[ ] Honesty test (R9): no invented BridgeMind REST/webhooks/CLI/fleet-dashboard?
[ ] Boundary test (R10): no backend contract redefined; new contract needs proposed as notes only?
Return the filtered list + the footer. No application code.



---



## P5 — Implement one chosen suggestion (run after you pick an id)



Implement exactly ONE suggestion, id = [PASTE THE id HERE, e.g. F-OBS-1], from the lists produced under the P0 OUTPUT SCHEMA. Re-apply RULES R1–R10 from P0 in full.

Constraints:

- Build only inside ui/ over agentBridge.js; do not change the backend contract. If the chosen item's backend_dep is NEEDS-BACKEND, STOP and instead deliver (a) the proposed agentBridge.js / Tauri method signature as a note and (b) a fully-built UI behind a clearly-labelled "pending backend" state — do not fake the data.
- Use the shadcn primitives and tokens/lanes the item's `components` and `visual` fields specify; cover every state in its `ux` states list; wire the keyboard/a11y notes.
- Keep the diff minimal and within the files the item touches; do not drive-by refactor; preserve semantic lanes and the terminal aesthetic.
- Match existing conventions in ui/ (naming, component structure, the agentBridge singleton pattern in ui/HANDOFF.md).
- Before finishing, run the repo's existing checks for the touched package (see BUILD-PLAN.md §0 — `npx vitest run` + `vite build` for ui/, plus any Playwright/visual tests), add/extend a test for the new behavior, and report the exact commands + results. Do not redefine or weaken the test gate.
- Output: the changed files, a short explanation tying each change to the item's fields and to the doc section it implements, and the test command + result.
- Transfer-ready: a human pastes your changes into the LOCAL harness-ready ui/ checkout, so every path you touch must be ui/-relative and match the local tree 1:1; list the exact target files up front; keep the diff minimal and portable; do not depend on any file or component that isn't in the read-first list unless you create it and name it.



---

