# TASK p6 — INDEPENDENT P0 VERIFICATION (harness-ready UI)

You are pane p6 in workspace ws83621x0. Coordinator is p0. Deliverable is a VERIFICATION REPORT — no application code, read-only audit.

## Mission
Independently re-audit the harness-ready UI for (a) FAKE AFFORDANCES — any control with no backing agentBridge.js method/state — and (b) SIMULATED DATA presented without a placeholder affordance. Then check the coordinator's P0 audit for misses.

## Required reading
1. /Users/jeffrymilan/Personal/harness-ready/docs/proposals/P0-ALIGNMENT.md — the audit you are verifying (agreement AND disagreement both valuable)
2. /Users/jeffrymilan/Personal/harness-ready/ui/HANDOFF.md — the contract + "Simulation to strip"
3. /Users/jeffrymilan/Personal/harness-ready/docs/BASE44-PROMPTS.md — R1–R10 (R3, R4, R9 especially)

## Audit method (do it yourself, don't trust P0)
- Read every file under /Users/jeffrymilan/Personal/harness-ready/ui/src/components/command/** and ui/src/pages/*.jsx.
- For every button/menu item/input: trace the handler to a bridge.* call, a local-state effect, or NOTHING (fake). Report controls whose only effect is a toast/console.log.
- For every rendered number/label/chart series: trace to subscribe data, a computed prop, or a hardcoded/random source. Report anything simulated rendered as live.
- Check handlers that reference bridge methods MISSING from the mock (TypeError risk in web preview) or Tauri-only commands used unguarded.
- Cross-check localStorage surfaces (workspaceStore acc-workspaces, paneLabels hr:pane-labels, workspaceAssign) for state the UI treats as truth but the backend never confirms.

## Report format
1. **Confirmed findings** — table: file:line · control/data · verdict (FAKE / SIMULATED-UNFLAGGED / HONEST) · suggested disposition (wire it / kill it / NEEDS-BACKEND tag / add placeholder affordance).
2. **P0 disagreements** — anywhere P0-ALIGNMENT.md is wrong or too soft/too harsh, with evidence.
3. **P0 misses** — findings P0 did not list.
4. **Clean bill** — components you verified as fully honest (so the coordinator knows coverage is complete).

## Rules
- Read-only. Do not edit any file. Do not run builds.
- Precision over volume: every finding cites file:line. No speculation — if you can't trace it, label UNVERIFIED.
- The dead LOAD DEMO FLEET button, SessionInfo fake constants, monitorData.js page, and templates' base44 entity are KNOWN — confirm or refute them, then focus energy on finding NEW issues.

## Output
Write the full report to /Users/jeffrymilan/Personal/harness-ready/docs/proposals/panes/p6.md (coordinator's checkout, NOT your worktree). End with `## BOUNDARIES`. Then report completion.
