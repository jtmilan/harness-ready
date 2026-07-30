# Lane L4 — core crate identity + docs + audit script (Tier A rebrand)

You are ONE lane of a 4-lane parallel rebrand. Stay INSIDE your file boundaries.
Base: branch `rebrand/harness-ready-mcp` — crate/binary renames are ALREADY committed
(agent-teams-daemon → harness-ready-daemon incl. lib name; the app-side identifiers are
done). Your work: identity strings in the CORE crates + docs + the audit script.

## YOUR files (edit ONLY these areas)
- `core/supervisor/src/lib.rs` — the `mcp__agent-teams__*` tool-prefix allowlists in
  `worker_args` (→ `mcp__harness-ready__*`) and any product-identity comments.
- `core/harness/src/**` — same prefix allowlists if present + identity strings.
- `core/daemon/src/**` — product-identity strings/descriptions (the crate's `Agent Teams —`
  description lines → `Harness Ready —`; log/comment product references).
- `core/mcp/src/lib.rs`, `core/{memory,agent,task,ringbuf,flywheel,roles}/src/**` —
  product-identity comments/descriptions ONLY.
- `README.md`, `docs/**/*.md` — MCP identity references (the MCP server is now
  `harness-ready-mcp`; product prose → Harness Ready).
- NEW FILE `scripts/audit-rebrand.sh` (see below).

## The change
1. `mcp__agent-teams__` → `mcp__harness-ready__` in EVERY allowlist/permission string
   (worker_args etc.) — this is load-bearing: workers' tool permissions must match the
   new server key the templates now emit (Lane L1 owns the templates; you own the
   allowlists that must agree with them).
2. Crate `description = "Agent Teams — …"` fields → `"Harness Ready — …"`.
3. Docs: the MCP server identity + install/config snippets (the server key in any
   `~/.mcp.json` example → `harness-ready`).
4. DO NOT change (Tier B — the audit script allowlists these):
   - `AGENT_TEAMS_*` env var names,
   - state-file/socket constants: `agent-teams-mcp.sock`, `agent-teams-live.json`,
     `agent-teams-external-mutations.jsonl`, `agent-teams-mcp-http.token/port`,
     `agent-teams-orchestrate`, `agent-teams-bridge`, `LIVE_REGISTRY_FILE`, `SOCKET_FILE`,
   - `agent-teams/<id>` worktree branch prefixes,
   - the `core/mcp` PACKAGE name `agent-teams-core` (Tier B crate rename).
5. Write `scripts/audit-rebrand.sh`: grep the tree (excluding target/, node_modules/,
   .git/, binaries/) for `agent-teams-mcp`, `mcp__agent-teams`, and `"agent-teams"`
   server-key patterns; ALLOWLIST the Tier-B residue above (env var names, state-file
   constants, package name agent-teams-core, branch prefixes); print any UNEXPECTED
   residue and exit 1 if found, exit 0 when clean.

## Gate (must be green before you commit)
```
rtk proxy cargo test -p supervisor -p harness -p agent-teams-daemon -p agent-teams-core
bash scripts/audit-rebrand.sh
```

## Commit
One commit on your lane branch:
`refactor(rebrand): L4 — core identity strings + docs + audit-rebrand.sh`
End the message with: `Co-Authored-By: Claude <noreply@anthropic.com>`

## Report
End with `## BOUNDARIES` (files touched, or heading + "none") and PASTE the final
`scripts/audit-rebrand.sh` output (the residue list it reports on the tree WITH the
other 3 lanes NOT applied — note which residues belong to L1/L2/L3).
