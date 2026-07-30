Reading the full review material from the offloaded prompt file.ui/src/components/command/templates/templateIO.js:317: SHOULD: Untrusted import does `JSON.parse(text)` with no pre-parse size bound (only post-parse `MAX_IMPORT=500` on array length) — a multi‑MB/GB file freezes the renderer / can thrash localStorage (CWE-400, OWASP ASVS V5.1.5). Fix: refuse `file.size` / `text.length` above a hard cap (e.g. 1–2 MB) before parse; keep `MAX_IMPORT`.

ui/src/components/command/templates/templateIO.js:302: SHOULD: `validateOneTemplate` caps `name`/`description` but passes `agents` through `coerceTemplateAgents` with no max agent count or role length — 500 templates × huge agent arrays still DoS localStorage (CWE-400). Fix: cap `agents.length` (e.g. ≤50) and `role` length (e.g. ≤200) before accept.

ui/src/components/command/templates/templateAgents.js:35: SHOULD: Schema pin drops unknown keys (good) but `priority`/`autonomy` are copied with no type/allowlist — import can store objects/huge strings into localStorage and into launch payload (CWE-20). Fix: keep only allowlisted string enums (or drop non-string / unknown).

agent-teams-mcp/src/main.rs:1431: SHOULD: `recent_event_ids` treats any `read_dir` child as a pane id: `is_dir()`/`metadata` follow symlinks and ids are not filtered by `validate_spawn_id` — no canonical prefix-check that `events.jsonl` stays under `state_dir` (CWE-22, CWE-59). Fix: require `validate_spawn_id(id)`; use non-follow metadata (or canonicalize + `starts_with(state_dir)`); skip symlink dirs.

agent-teams-mcp/src/read_output.rs:177: SHOULD: `registry_present` is set from post-reconcile `pane_is_live`, so with `reconcile_liveness` ON a disk-only pane reports `registry_present: true` while the field claims registry membership — false authz/liveness signal (CWE-451, fail-closed). Fix: `registry_present = reg_row.is_some()`; expose reconcile source separately.

agent-teams-mcp/src/read_output.rs:145: SHOULD: Flag-ON path calls `recent_event_ids(state_dir, …)` (full tree scan) to decide liveness of one already-validated `id` — work scales with state_dir width (CWE-400). Fix: `metadata(state_dir.join(id).join("events.jsonl"))` only.

ui/src/lib/contextGraph.js:32: SHOULD: `countByKind` uses plain `{}` and indexes with untrusted `kind`/`file_type`/`category` — `__proto__`/`constructor` keys corrupt the bag / prototype (CWE-1321). Fix: `Object.create(null)` or `Map`.

VERDICT: 0 blocking / 7 should / 0 nit

## BOUNDARIES
- Skills manifest: phase/6-skills material only shows `/skills` route + empty `ui/src/data/skills-raw.json` (`[]`). No `Skills.jsx`, no manifest parser/validator in the inlined diffs — **could not review skills-manifest parsing**.
- Shared-state namespace gate: no write path or namespace isolation code ships; only fail-closed UI helpers + HD note. Nothing to flag as an open write/namespace hole in code.
- Review material ends mid-`sharedState.test.js` (`expect(sharedStateLabel("READ-WRITE")).toEqual({`); could not verify the remainder of that test file.
- Did not run builds/tests; line numbers taken from full new-file bodies or diff hunks in the provided material only.
