# Lane L2 — sidecar source identity (Tier A rebrand)

You are ONE lane of a 4-lane parallel rebrand. Stay INSIDE your file boundaries.
Base: branch `rebrand/harness-ready-mcp` — the crate is ALREADY renamed to
`harness-ready-mcp/` (dir + package + bin). Your work is the IDENTITY STRINGS inside it.

## YOUR files (edit ONLY under `harness-ready-mcp/`)
- `harness-ready-mcp/src/main.rs` — rmcp `ServerInfo`/server name, the developer-guide
  prompt resource text, tool registration descriptions.
- `harness-ready-mcp/src/*.rs`, `harness-ready-mcp/src/phase_b/*.rs` — every doc comment,
  log line, error detail, and tool description that says "agent-teams" / "Agent Teams"
  as a PRODUCT/SERVER identity.

## The change
1. MCP server identity (the name the sidecar announces to clients): `agent-teams` →
   `harness-ready-mcp` (or `Harness Ready` for human-facing prose — use judgment:
   machine names get `harness-ready-mcp`, prose gets `Harness Ready`).
2. Tool descriptions mentioning "Agent Teams state/queue/registry" → "Harness Ready …".
3. Developer-guide prompt text: product references → Harness Ready.
4. Do NOT change, anywhere:
   - `AGENT_TEAMS_*` ENV VAR NAMES (Tier B — they stay until a later pass with a compat shim),
   - state-file names: `agent-teams-mcp.sock`, `agent-teams-live.json`,
     `agent-teams-external-mutations.jsonl`, `agent-teams-mcp-http.*`,
   - the `bridgeagent` integration (different product),
   - any tool NAME (`team_get_queue`, `team_send_input`, …) — only descriptions.

## Gate (must be green before you commit)
```
rtk proxy cargo check -p harness-ready-mcp
rtk proxy cargo test -p harness-ready-mcp
rtk proxy cargo test -p harness-ready-mcp --features phase-b-mutations
```

## Commit
One commit on your lane branch:
`refactor(rebrand): L2 — sidecar identity strings agent-teams → Harness Ready`
End the message with: `Co-Authored-By: Claude <noreply@anthropic.com>`

## Report
End with `## BOUNDARIES` (files touched, or heading + "none"). List any identity string
you were UNSURE whether to rename (env var vs product name) — the coordinator decides.
