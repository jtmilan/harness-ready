# TASK p3 — P3 VISUAL SYSTEM SPEC (harness-ready UI)

You are builder pane p3 in workspace ws83621x0. Coordinator is p0. Deliverable is a SPEC DOCUMENT — no application code.

## Required reading (absolute paths, in full)
1. /Users/jeffrymilan/Personal/harness-ready/docs/BASE44-PROMPTS.md — R5/R6/R8 (+R3), P3 prompt, AVOID list, P4 checklist
2. /Users/jeffrymilan/Personal/harness-ready/docs/DESIGN-BRIEF.md — §5 visual language (lanes, data-skin, density/type), §7 anti-patterns, §8 a11y
3. /Users/jeffrymilan/Personal/harness-ready/ui/src/index.css — current tokens (shadcn defaults still LIGHT in :root; .scanlines + .crt-screen exist; fonts: Rajdhani display / JetBrains Mono body)
4. /Users/jeffrymilan/Personal/harness-ready/ui/tailwind.config.js and /Users/jeffrymilan/Personal/harness-ready/ui/components.json
5. /Users/jeffrymilan/Personal/harness-ready/docs/proposals/P0-ALIGNMENT.md — theme-layer row (what exists vs missing)
6. Monitoring components for the before→after specs: /Users/jeffrymilan/Personal/harness-ready/ui/src/pages/Monitoring.jsx + ui/src/components/monitor/*.jsx + ui/src/components/command/EmptyState.jsx

## Deliverable
1. **Token system** — CSS variables / shadcn theme tokens with four semantic lanes on DEDICATED tokens no brand override touches: `--need` (amber), `--success` (green), `--danger` (red), `--info` (cyan / brand accent). Plus a `data-skin`-ready variable layer (token-ready now, switcher later — sibling agent-teams ships 5 skins via html[data-skin]; don't build skins, just don't block them). Chart lane mapping: CPU=info, memory=need, success=success, error=danger. Fix the light-default :root problem (dark look currently hardcoded #0D1117).
2. **Typography** — monospace/technical DISPLAY headers (keep/extend Rajdhani), small-caps section labels, tabular numerals (`font-variant-numeric: tabular-nums`) for ALL metrics; deliberate pairing with a readable body face (JetBrains Mono); bold type-scale ratios (large tabular metrics vs small-caps labels vs body).
3. **Density + spacing guidance** — control-dense terminal feel; keep the command line as first-class citizen ($ motif); spacing scale for dense instrument panels, not marketing pages.
4. **Motion rules** — durations/easings as tokens used consistently for hover/press/FLIP/chart updates; reduced-motion off-switch (prefers-reduced-motion kills all of it); purposeful, non-gratuitous.
5. **Ambient/background-layer token** — subtle grid / scanline / telemetry field (extend the existing .scanlines; NEVER aurora blobs).
6. **a11y pass** — visible focus rings system, AA contrast targets, color-never-sole-signal (pair EVERY lane color with an icon/label — critical for monitoring charts).
7. **Before→after component specs (no code)** for exactly these five: KPI cards (StatCard), fleet resource chart (ResourceChart), per-agent success bars (SuccessRateChart), supported-harnesses grid (EmptyState harness tiles), empty-state (EmptyState). Each: current look · proposed look · tokens/lanes used · honest empty/stale/no-data variant.

## Discrete visual suggestions use the OUTPUT SCHEMA with ids A-VIS-n
(id / title / implements / icp_fit / ux / backend_dep / fake_affordance_check / components / visual / effort / priority / a11y_keyboard)

## Hard rules
- Do NOT import BridgeMind's friendly pink/violet gradient look (targets beginners). The aesthetic must keep signaling "for people who live in a terminal".
- AVOID (hard): centered hero trio; 3–4 equal feature cards; gradient headlines; indigo/violet/pink gradients; blanket glassmorphism/rounded-2xl; aurora blobs; cream/terracotta/serif; near-black + single neon accent; broadsheet hairline grids.
- State colors are sacred — a diff @@ header or status pill never wears brand color (DESIGN-BRIEF principle 5).
- R6: no credit/usage/upsell surface. R3: any new control names its backing method or is NEEDS-BACKEND.

## Output
Run P4 self-review + append footer. Write the full report to /Users/jeffrymilan/Personal/harness-ready/docs/proposals/panes/p3.md (coordinator's checkout, NOT your worktree). End with `## BOUNDARIES`. Then report completion.
