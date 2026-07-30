jtmilan/phase/0-unification — harness-ready-mcp/src/read_output.rs:177: SHOULD: `registry_present` set from widened `pane_is_live`, so with reconcile ON a disk-only pane returns `registry_present:true` — field contract says "present in live registry"; lies when flag armed (RC1 anti-lie pattern the phase claims to fix). Expose separate `observed_source` (or keep `registry_present` = raw `reg_row.is_some()`) and derive liveness separately.

jtmilan/phase/0-unification — harness-ready-mcp/src/read_output.rs:285: SHOULD: same widening drives `registry_fact` prose; disk-only pane (reconcile ON) emits "the live registry lists this pane" AND appended "observed=Disk (registry omits)" — contradictory in one note. Branch `registry_fact`/`divergence` on `reconcile_source`, not widened `pane_is_live`.

jtmilan/phase/0-unification — harness-ready-mcp/src/read_output.rs:145: SHOULD: reconcile block calls `recent_event_ids(state_dir, TTL)` = full `state_dir` scan per `team_read_output`, to test ONE id. Stat `<state_dir>/<id>/events.jsonl` directly (id already format-validated) instead of scanning every pane.

ui/src/lib/contextGraph.js:34: SHOULD: `countByKind` uses plain `{}` keyed by untrusted `node.kind`/`file_type`/`category`; keys `__proto__`/`constructor`/`toString` misbehave (proto accessor no-op, NaN). Use `Map` or `Object.create(null)` — memory-store content is upstream-untrusted.

jtmilan/phase/0-unification — harness-ready-mcp/src/main.rs:1352: NIT: flag-off path adds `read_mcp_config` disk read before "byte-for-byte existing logic"; invariant holds for queue compute but doc overstates — config I/O is new on every call. Reword comment: compute unchanged, one extra config read.

jtmilan/phase/0-unification — harness-ready-mcp/src/read_output.rs:300: NIT: none-note now always pastes full LIVENESS-BLINDNESS paragraph for ANY registry-absent id, incl. plain typos; old "check the id" guidance buried. Gate divergence prose to `!pane_is_live` + genuine-candidate signal, keep typo hint first.

ui/src/components/command/SharedStateBadge.jsx:88: NIT: NEEDS-BACKEND span at `text-[9px] opacity-60` — low contrast + sub-10px likely fails WCAG (R8 claim). Raise to ≥10px, drop opacity or use muted-but-passing token.

ui/src/components/command/SharedStateBadge.jsx:80: NIT: `role="status"` = implicit aria-live polite; static chip may be announced on mount. Use `role="img"` or bare `<span>` + aria-label; reserve live region for actual mode flips.

jtmilan/phase/0-unification — core/mcp/src/lib.rs:2502: NIT: `disk_recent` treats ANY future mtime as recent forever — skewed-clock dead pane stays "live" indefinitely. Cap future tolerance (e.g. `age_abs <= ttl` with bounded forward slack) or document the unbounded case as accepted.

jtmilan/phase/0-unification — harness-ready-mcp/src/main.rs:1341: NIT: phase edits sidecar (`harness-ready-mcp`) + `core/mcp`; Rust invariant says "no app/daemon edits." Confirm "daemon" excludes the MCP sidecar, else phase-0 violates scope as written. Rename invariant target or note sidecar is in-scope MCP layer.

VERDICT: 0 blocking / 4 should / 6 nit

## BOUNDARIES

- Review material truncated mid-`ui/src/lib/sharedState.test.js` (ends inside `expect(sharedStateLabel("READ-WRITE")).toEqual({`). Could not verify that test's tail or that the file parses — truncation artifact vs real syntax error undecidable from material.
- Could not read: `read_mcp_config` error/panic behavior on corrupt config, `registry_lookup`, `compute_queue_identified` internals, `read_registry` path resolution. Test `none_path_with_registry_row_says_listed_not_absent` writes registry to `root` but resolves with `root/state` — whether `read_registry` walks up is unverified.
- No `cargo` / `vitest` run; line numbers estimated from diff hunk headers, not opened source.
- SKIP-list sweep clean on visible material: no drag-kanban, voice, beginner-video, vendor-bench, themes, cloud-first. F-MEM-1 memory graph + R-MCP-RW indicator are in-scope. Shared-state write gate correctly deferred to HD note with explicit untrusted-content handling (Q3) — no write path ships, nothing violates the gate today.
- Rust invariants verified on material: no `unsafe`, no new crate dep, pure helpers, flag default `false`.
