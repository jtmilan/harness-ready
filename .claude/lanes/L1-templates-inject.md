# Lane L1 — MCP templates + injection test assertions (Tier A rebrand)

You are ONE lane of a 4-lane parallel rebrand. Stay INSIDE your file boundaries.
Base: branch `rebrand/harness-ready-mcp` — the crate/binary renames (agent-teams-mcp →
harness-ready-mcp, agent-teams-daemon → harness-ready-daemon) are ALREADY committed there.
Your work is identity STRINGS only.

## YOUR files (edit ONLY these)
- `core/hooks/claude-mcp.tmpl.json`
- `core/hooks/cursor-mcp.tmpl.json`
- `core/hooks/commandcode-mcp.tmpl.json`
- `core/hooks/opencode-mcp.tmpl.json`
- `core/hooks/cline-mcp.tmpl.json` (if it has the server key)
- `core/state-adapter/src/inject.rs` — TEST assertions only (the `v["mcpServers"]["agent-teams"]` lookups)
- `core/supervisor/tests/*.rs` — test assertions on the rendered server key ONLY (binary-path renames are already done)

## The change
1. In every template: the mcpServers KEY `"agent-teams"` → `"harness-ready"`.
   Everything else in the templates stays BYTE-IDENTICAL: `{{SIDECAR}}`, `{{STATE_DIR}}`,
   the `AGENT_TEAMS_*` env var NAMES (Tier B — do NOT rename env vars), the
   `{{BRIDGEAGENT}}` fragment and the `bridgeagent` server key (different product — do NOT touch).
2. Update every test assertion that looks up the rendered server key to `"harness-ready"`.
3. Do NOT touch: env var names (`AGENT_TEAMS_STATE_DIR` etc.), state-file names
   (`agent-teams-mcp.sock`, `agent-teams-live.json`), the bridgeagent fragment.

## Gate (must be green before you commit)
```
rtk proxy cargo test -p state-adapter -p supervisor
```

## Commit
One commit on your lane branch:
`refactor(rebrand): L1 — MCP server key agent-teams → harness-ready in templates + test assertions`
End the message with: `Co-Authored-By: Claude <noreply@anthropic.com>`

## Report
Write your report ending with a `## BOUNDARIES` section listing every file you touched
(empty section = heading + "none"). Note any server-key reference you found OUTSIDE your
boundaries (do not edit it — report it).
