# Lane L3 — app crate + ui identity strings (Tier A rebrand)

You are ONE lane of a 4-lane parallel rebrand. Stay INSIDE your file boundaries.
Base: branch `rebrand/harness-ready-mcp` — crate/binary renames + the app's
resolve_sidecar_bin/resolve_coordinator_sidecar_bin candidate strings + the
`agent_teams_daemon::` → `harness_ready_daemon::` identifier rename are ALREADY
committed. Your work is the REMAINING identity strings in the app + ui.

## YOUR files (edit ONLY under `app/`)
- `app/src-tauri/src/*.rs` — remaining "agent-teams-mcp" / "Agent Teams" identity
  comments + strings (the skeleton sed already took the plumbing strings — find what's
  left: doc comments naming the product, log prefixes like `[agent-teams]`, any
  user-visible detail strings).
- `app/src/**` (ui JS/JSX) — any reference to the MCP server identity: tool-prefix
  strings `mcp__agent-teams__`, "Agent Teams" product copy tied to the MCP layer.

## The change
1. Product identity in comments/logs/strings: "Agent Teams" → "Harness Ready" where it
   names THIS product's MCP/app layer. Log prefix `[agent-teams]` → `[harness-ready]`.
2. ui: `mcp__agent-teams__*` permission/prefix strings → `mcp__harness-ready__*`.
3. Judgment calls — DO NOT change:
   - role/persona vocabulary (`AgentRole::Coordinator`, "coordinator pane" — these are
     role names, not product names),
   - `AGENT_TEAMS_*` env var names (Tier B),
   - state-file names (`agent-teams-mcp.sock`, `agent-teams-live.json`, …) (Tier B),
   - git branch prefixes like `agent-teams/wsx` (worktree branch naming — Tier B),
   - audit `source` values already in shipped jsonl history (`external_orchestrator` etc.).
   When unsure: leave it and report it.

## Gate (must be green before you commit)
```
cd app/src-tauri && rtk proxy cargo check
cd .. && ./node_modules/.bin/vitest run
cd ../ui && ./node_modules/.bin/vitest run
```

## Commit
One commit on your lane branch:
`refactor(rebrand): L3 — app + ui identity strings agent-teams → Harness Ready`
End the message with: `Co-Authored-By: Claude <noreply@anthropic.com>`

## Report
End with `## BOUNDARIES` (files touched, or heading + "none"). List every judgment-call
string you left unchanged and why.
