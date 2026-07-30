# codex — testing lens (ROUND 2, GROUNDED)

Grounded via `codex exec --dangerously-bypass-approvals-and-sandbox` in a throwaway worktree
checked out at `main` (post-merge), git push neutered (per-process pushurl=/dev/null), output
captured `2>&1`. codex READ the merged tree and ran `rg`/`git` in the worktree. The run hit its
300s timeout (rc=124) mid-review; the full 10,142-line reasoning stream is preserved locally at
`/tmp/codex-raw-stream.md` (489KB, NOT committed — noisy chain-of-thought). Findings below are what
codex verified before cutoff.

## Findings

- core/mcp/src/lib.rs: SHOULD: the `reconcile_liveness` config gate has NO test asserting it
  defaults to `false`, and no round-trip test — unlike every sibling gate. The pure helpers
  (`liveness_source`, `observed_live_ids`) are well-tested, but the gate default itself is only
  protected by the explicit `Default` impl, not pinned by a test. Fix: add a default-off + round-trip
  test matching the other gates.
- ui/src/components/command/templates/templateIO.js: SHOULD: `coerceTemplateAgents` (a pure helper
  with real edge-case logic: cline→pi migration, persona-as-kind fallback, empty-kind default,
  non-array guard, schema-pin key stripping) has NO dedicated test file; only indirectly exercised by
  one `templateIO.test.js` assertion. Fix: add direct unit tests for each branch.
- ui/src/lib/commandPalette.js: NIT: `filterCommands` and `moveIndex` are dead code in production —
  referenced only by their test file; `CommandPalette.jsx` uses cmdk's own filter. Acknowledged in a
  comment ("for the dep-free path and any future palette variant"). Fix: delete or wire up.
- agent-teams-mcp/src/main.rs `recent_event_ids`: NIT (resolved): lacks a direct test BUT delegates
  the recency decision to `agent_teams_core::disk_recent`, which codex verified IS well-tested
  (6 tests: none/within/boundary/outside/future/zero-ttl) — so the thin fs-walker needs no direct test.

## VERDICT: 0 blocking / 2 should / 2 nit (incomplete — cut at 300s)

## BOUNDARIES
- Stream cut at the 300s timeout while verifying `disk_recent` test coverage; codex did NOT finish a
  final pass over the remaining UI helpers (skillsManifest, contextGraph, sharedState, monitorRows,
  orchPaths test coverage was being assessed). The findings above are verified; the rest of the lens
  is unreviewed past cutoff.
- Anti-hallucination honored: every finding cites a file codex actually opened; `disk_recent` test
  count was confirmed by `rg` before the NIT was downgraded.
