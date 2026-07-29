# TASK p1 — P1 FEATURE PROPOSALS (harness-ready UI)

You are builder pane p1 in workspace ws83621x0, working the harness-ready proposal session. Coordinator is p0. Deliverable is a PROPOSAL DOCUMENT — no application code.

## Required reading (absolute paths, read in full before writing)
1. /Users/jeffrymilan/Personal/harness-ready/docs/BASE44-PROMPTS.md — R1–R10 (non-negotiable), OUTPUT SCHEMA (P0 STEP 3), P1 prompt, P4 checklist
2. /Users/jeffrymilan/Personal/harness-ready/docs/proposals/P0-ALIGNMENT.md — the alignment audit: what EXISTS on the bridge contract, what is fake/simulated today, ranked unimplemented R-* list
3. /Users/jeffrymilan/Personal/harness-ready/docs/PRD.md — §6 transfer matrix, §7 requirements (R-OBS…R-MCP-RW), §8 vaporware watchlist
4. /Users/jeffrymilan/Personal/harness-ready/docs/BUILD-PLAN.md — §1 phasing + ordering rationale, §0 test gate
5. /Users/jeffrymilan/Personal/harness-ready/ui/HANDOFF.md — the agentBridge.js contract + "Simulation to strip"

## Deliverable
Prioritized FEATURE suggestions implementing: **R-OBS, R-TEMPLATES, R-MEM, R-ORCH, R-ONBOARD, R-PALETTE, R-SKILLS-OSS**. SKIP R-GATES and R-MCP-RW as implementation targets, but you MAY propose their future UI surface tagged NEEDS-BACKEND.

Every suggestion uses the OUTPUT SCHEMA exactly:
`id` (F-OBS-n / F-TPL-n / F-MEM-n / F-ORCH-n / F-ONB-n / F-PAL-n / F-SKL-n) · `title` · `implements` (doc section + R-*) · `icp_fit` (one line + transfer verb BORROW/ADAPTER/MAKE-REAL) · `ux` (screen + interaction + states covered) · `backend_dep` (name the agentBridge.js method / Tauri command — mark EXISTS or NEEDS-BACKEND) · `fake_affordance_check` (PASS or flagged) · `components` (shadcn primitives) · `visual` (tokens/lanes) · `effort` (S/M/L) · `priority` (P0/P1/P2) · `a11y_keyboard`

## Hard constraints
- R-OBS: bind to the real-signal list — process liveness + per-child CPU/mem + git state + last-tool-failure + human-gate queue depth (PRD §7 R-OBS AC). Define empty/loading/stale/state_blind/needs-human/error. Never propose a vanity aggregate dashboard. Never interpolate. KPI cards get a source/age affordance ("live · 2s" vs "no data") per DESIGN-BRIEF §4.2.
- R-ORCH: role + owned-paths are ADVISORY conventions on the task channel. Orchestrate preview shows a non-blocking amber collision WARNING when two Builders' owned paths overlap. Do NOT propose an auto-enforce-ownership toggle (no backend enforcer exists — DESIGN-BRIEF §4.1, §7.5).
- R-ONBOARD: recipes/playbooks (per-harness autonomy profiles + first-run steps) reusing the templates manager / palette. NOT video, NOT a beginner course (hard SKIP, R2).
- R-PALETTE: palette over EXISTING handlers only — adding an action means adding a handler, never a palette-only shortcut.
- Anything on the SKIP list (drag-kanban, voice, beginner tutorial/video, vendor benchmark, theme gallery, three modes, cloud-first) gets listed ONCE under a "Rejected + why" footer, never as a proposal.
- Every control names its backing method/state or is tagged NEEDS-BACKEND (R3). No invented BridgeMind REST/webhooks/CLI/fleet-dashboard (R9). No credit/usage/upsell surfaces (R6).

## End with
1. A one-paragraph **sequencing note** consistent with BUILD-PLAN.md §1: R-OBS + R-GATES first (critical path 0→1b); R-TEMPLATES/R-MEM/R-ORCH next (must-haves); R-PALETTE/R-SKILLS-OSS/R-ONBOARD opportunistic parallel tracks; R-MCP-RW last and gated.
2. The **P4 self-review checklist** run against every item (all ten R-* tests) + a one-line footer: how many items you dropped and why.

## Output
Write the full report to /Users/jeffrymilan/Personal/harness-ready/docs/proposals/panes/p1.md (absolute path — this is the coordinator's checkout, NOT your worktree). End with a `## BOUNDARIES` section listing what you did NOT touch. Then report completion in your pane.
