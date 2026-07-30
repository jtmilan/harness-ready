# TASK p2 — P2 UI / IA REDESIGN PROPOSALS (harness-ready UI)

You are builder pane p2 in workspace ws83621x0. Coordinator is p0. Deliverable is a PROPOSAL DOCUMENT — no application code.

## Required reading (absolute paths, in full)
1. /Users/jeffrymilan/Personal/harness-ready/docs/BASE44-PROMPTS.md — R1–R10, OUTPUT SCHEMA, P2 prompt (incl. LIVING/CRAFTED mandate + AVOID list), P4 checklist
2. /Users/jeffrymilan/Personal/harness-ready/docs/DESIGN-BRIEF.md — §1 principles, §2 current-state map, §3 IA, §4 interactions, §6 honesty patterns, §8 a11y/keyboard, §9 shadcn primitives
3. /Users/jeffrymilan/Personal/harness-ready/docs/proposals/P0-ALIGNMENT.md — current keyboard map (only ⌘⇧I + ⌘G exist), fake affordances, contract coverage
4. /Users/jeffrymilan/Personal/harness-ready/docs/PRD.md §6–§7 and /Users/jeffrymilan/Personal/harness-ready/ui/HANDOFF.md

## Deliverable — five redesign proposals, each a full section
1. **COMMAND / MONITORING split + optional CONTEXT surface** (read-only memory graph, R-MEM) — keep the act/observe split (DESIGN-BRIEF §3); CONTEXT is new top-level surface, read-only, dual-authorship caption "agents can read/write this via MCP".
2. **Spawn + orchestrate flow** — role-cast segmented control (Coordinator/Builder/Scout/Reviewer/none), owned-paths glob field, preview roster (role pill + owned-paths chip + harness + model) with non-blocking amber collision WARNING (DESIGN-BRIEF §4.1). "The power-user analogue of the mission tree — text/table-first, not decorative."
3. **Command palette over EXISTING handlers only** — Cmd/Ctrl+K fuzzy over spawn (per harness), orchestrate, focus pane, open diff, copy id/branch, toggle tab, run template; each row shows its keybinding; no palette-only shortcuts (DESIGN-BRIEF §4.4).
4. **Recipes/Playbooks manager (R-ONBOARD)** reusing the templates manager — per-harness autonomy profiles + first-run playbooks; versionable as local files; NOT video/courses.
5. **Attention / queue semantics + per-pane inline reply** — ranked who-needs-you (needs_human first → turn_end > rate_limit → longest wait); amber as the single "needs you" signal; RESUME-always, no toggle (no paused state exists).

## Every proposal must contain
- The full OUTPUT SCHEMA (ids U-CMD-n / U-ORCH-n / U-PAL-n / U-ONB-n / U-QUE-n): id / title / implements / icp_fit / ux / backend_dep (EXISTS or NEEDS-BACKEND) / fake_affordance_check / components / visual / effort / priority / a11y_keyboard
- A complete **STATES list**: empty / loading / working / needs-human / blocked / error / stale / state_blind / registry-reconciling — say what the user sees in each
- A **KEYBOARD map**: every action key-reachable; palette accelerates, never sole path; honor typing-target guards so xterm/inputs keep keystrokes (DESIGN-BRIEF §8)
- Named shadcn primitives from DESIGN-BRIEF §9: segmented control, chip, dialog/sheet, tooltip, badge, tabs

## Navigation + honesty rules
- No new top-bar clutter — new actions go to the palette or a `⋯` overflow (the topbar-dedupe audit set the bar).
- RESUME-always, never a pause toggle.
- LIVING/CRAFTED mandate on every screen: layered ambient background (faint technical grid / scanline / topographic contour / slow telemetry field — NOT aurora blobs); live micro-interactions (hover/press/focus on panes, buttons, queue rows, harness tiles; FLIP transitions on roster/queue reorder; purposeful chart updates); strong type contrast (distinctive monospace/technical DISPLAY face for headers/labels/metrics + readable body face). Open each screen with what is most characteristic of fleet control (live queue / fleet / terminals), never a generic hero. All motion honors prefers-reduced-motion.
- AVOID (hard): centered hero trio; row of 3–4 equal feature cards; gradient headlines; indigo/violet/pink gradients; site-wide glassmorphism; blanket rounded-2xl; aurora blobs; cream/beige/terracotta/serif; near-black + single neon accent; dense broadsheet hairline grids.
- R3: every control names its backing bridge method/state or is tagged NEEDS-BACKEND.

## Output
Run the P4 self-review checklist over every item + append the self-review footer. Write the full report to /Users/jeffrymilan/Personal/harness-ready/docs/proposals/panes/p2.md (coordinator's checkout, NOT your worktree). End with `## BOUNDARIES`. Then report completion.
