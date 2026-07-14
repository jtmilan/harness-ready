# Context Brief — Embed Base44 "Agent Command Center" UI as Tauri frontend

(Recreated: original brief lived in agent-teams worktree ws28151x0-p0, deleted by the
dev-instance state-dir leak reap. Work completed 2026-07-13.)

## Topic/Intent
Embed this repo ($UI, standalone — HANDOFF.md's "same repo" claim is stale) as the frontend of
the agent-teams Tauri v2 app; fix bridge adapter to the real Rust wire contract; strip Base44;
build + run `cargo tauri dev`; smoke test; add pause/resume. Intent: implement.

## Best Practices Found
- EXTRACTED (agent-teams core/mcp/src/lib.rs:54): QueueRow wire = `{id, harness, state, reason,
  needs_human, since, role?, tag?, workspace?}` — adapter consumes these names; Rust untouched.
- EXTRACTED (agent-teams app/src-tauri/src/lib.rs:2286,2438): delta = `{base,next,data RAW
  string,truncated}`; no base64; cursor = `next`.
- EXTRACTED (app/src/main.js:3643): Tauri v2 invoke keys are camelCase for multiword Rust args
  (`sessionId`) — ground truth for arg casing.
- EXTRACTED (core/harness/src/lib.rs): wire strings claude/cursor/bash/codex/commandcode/
  opencode/cline; UI kind `claude-code`→`claude`; grok has no harness → bash fallback.
- EXTRACTED (auto-memory dev-instance-isolation + this run): second app instance needs EXPLICIT
  `AGENT_TEAMS_STATE_DIR` override — `${VAR:-default}` is defeated inside app-spawned panes.
- EXTRACTED (auto-memory second-brain-graph-base44-port): Base44→local swap = same-schema
  localStorage store behind the unchanged import path; Chrome/agent-browser cannot attach to
  Tauri webview.

## Industry-Standard Architecture/Patterns
- EXTRACTED (HANDOFF.md): ports-and-adapters — UI talks only to the bridge singleton; all fixes
  confined to the adapter. Conformed.
- INFERRED: offline-first desktop = stub remote SDK with Promise-shaped local store, bypass auth
  at the router root, drop build-time platform plugins.

## Anti-Patterns
- Editing Rust structs to fit the UI (forbidden by handoff). Not done.
- Second app instance sharing prod state dir (fleet reap hazard — happened once this run).
- Refactoring UI components/design system (out of scope). Not done.

## Open Questions
- `truncated` delta flag ignored by adapter (scrollback gap after ring eviction) — filed, minor.
- `kind: row.harness` wire string isn't an AGENT_KINDS key — cosmetic label misses; filed.
- In-Tauri GUI click-through unverified (no Screen Recording TCC); UI-layer 5/5 in Chrome mock.

## Sources
- HANDOFF.md, src/lib/tauriAgentBridge.js, src/api/base44Client.js (this repo)
- agent-teams: app/src-tauri/src/lib.rs, core/mcp/src/lib.rs, core/harness/src/lib.rs, app/src/main.js
- Lane reports: scratchpad L1/L2/L3/L6/L7 reports (session 50452cc3, 2026-07-13)
