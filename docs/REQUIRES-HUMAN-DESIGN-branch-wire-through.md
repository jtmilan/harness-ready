# REQUIRES HUMAN DESIGN — branch / worktree wire-through

**Lane:** W3-p8 (investigation only; no behavior change)  
**Worktree:** `/Users/jeffrymilan/at-w3/p8` · branch `lane/w3-p8`  
**Date:** 2026-07-14  

---

## Recommendation (one line)

**Do not extend `QueueRow` for the React UI alone — either (preferred for honesty) delete the permanently empty sub-header + always-disabled "Copy branch" item, or (if the chrome is product-desired) mirror the production app's one-shot `pane_branch` path and expose worktree path from in-process `AppState.meta` without touching the public MCP contract.** Extend `QueueRow` only if external MCP consumers need these fields; that is a separate product decision.

**Strongest evidence:** the production GUI already resolves branch via a dedicated Tauri command (`pane_branch` → `git rev-parse --abbrev-ref HEAD` on `AppState.meta`'s worktree root) and never put `branch`/`worktree` on `QueueRow` — VERIFIED at `app/src/main.js:531-554` and `app/src-tauri/src/lib.rs:3684-3703`. The React bridge self-propagates empty strings instead of calling that path — VERIFIED at `ui/src/lib/tauriAgentBridge.js:211-212`.

---

## Observation (re-verified)

| Claim | Tag | Evidence |
| --- | --- | --- |
| `QueueRow` has no `branch` / `worktree` fields | **VERIFIED** | `core/mcp/src/lib.rs:55-75` — fields are `id, harness, state, reason, needs_human, since, role?, tag?, workspace?` |
| Tauri bridge never sources branch/worktree from the queue | **VERIFIED** | `ui/src/lib/tauriAgentBridge.js:211-212` — `branch: prev?.branch \|\| ""`, `worktree: prev?.worktree \|\| ""`; no read of `row.branch` / `row.worktree` |
| Self-propagation of `""` forever | **VERIFIED** | Same lines: on tick 1 `prev` is missing → `""`; every later tick copies `prev.branch` which remains `""` |
| Pane sub-header always renders empty under Tauri | **VERIFIED** | `ui/src/components/command/AgentPane.jsx:400-401` — `⎇ {agent.branch} · {agent.worktree}` with empty strings → literal `⎇  · ` |
| "Copy branch" permanently disabled under Tauri | **VERIFIED** | `AgentPane.jsx:397` `hasBranch={!!agent.branch}` + `PaneMenu.jsx:117` `disabled={!hasBranch}` |
| Mock path fakes both fields (web-preview looks fine) | **VERIFIED** | `ui/src/lib/agentBridge.js:140-141`, `ui/src/lib/agentData.js:70-71` |
| `list_queue` returns core `QueueRow` and only enriches `role`/`tag` | **VERIFIED** | `app/src-tauri/src/lib.rs:2535-2547` |

Stale / aspirational doc (not a wire fact): `ui/HANDOFF.md:78-80` lists `branch`/`worktree` among `QueueRow` field names to verify — **VERIFIED** that the handoff text says this; **VERIFIED** that the actual struct does not have those fields.

---

## What the backend does / doesn't know

### Known at spawn (ephemeral return value)

**VERIFIED.** `supervisor::add_worktree` builds:

- **branch name:** `format!("agent-teams/{id}")` — `core/supervisor/src/lib.rs:366-367`
- **worktree root:** `{git_root}/.agent-teams-worktrees/{id}` — `:387`
- **cwd:** worktree root or sparse subpath inside it — returned as `Worktree { cwd, root, git_root, branch }` — `:279-288`, `:483-486`

`do_spawn` calls `add_worktree(&repo_path, &ps.id).ok()` — `app/src-tauri/src/lib.rs:1430-1433`. On failure, the pane falls back to running in the selected folder (no git worktree) unless `require_worktree` forces failure (`:1439-1444`).

### Retained after spawn

| Store | Path / key | What it holds | Branch name? | Worktree path? |
| --- | --- | --- | --- | --- |
| In-process meta map | `AppState.meta: id → Option<(git_root, worktree_root)>` | cleanup + git helpers | **No** | **Yes** (when worktree existed) |
| Persistent worktree registry (JSONL) | sibling of `state_root` (`WtEntry`) | crash-sweep orphans | **No** | **Yes** (`worktree`, `repo`) |
| Live registry (`agent-teams-live.json`) | `LiveWorkspace` | MCP live set + identity join | **No** | **No** — only `repo: Option<String>` = **source** `ps.repo`, not the isolated worktree |
| `QueueRow` / events.jsonl | state adapter projection | rank / status | **No** | **No** |

**VERIFIED** meta insert: `lib.rs:1568-1582` stores `wt.map(|w| (w.git_root, w.root))` — branch string is dropped after spawn.  
**VERIFIED** live registry write: `lib.rs:1604-1623` — `repo: Some(ps.repo.clone())`, role/tag/session_id; no worktree path, no branch.  
**VERIFIED** `LiveWorkspace.repo` doc: "Absolute repo/worktree path backing this workspace, if recorded" (`core/mcp/src/lib.rs:318-320`) — **INFERRED** naming is ambiguous; the writer uses `ps.repo` (the selected source folder), not `w.root` / cwd.

### Live branch resolution already exists (app-only)

**VERIFIED.** `pane_branch` (`lib.rs:3690-3703`):

1. Looks up worktree root via `worktree_from_meta` (`:3545-3554`)
2. Runs `git rev-parse --abbrev-ref HEAD` (`branch_name_args` at `:3686-3688`)
3. Errors cleanly when id unknown or pane has no worktree (`Some(None)`)

Production frontend uses this **once per pane head build** (with one delayed retry), not on the queue poll — `app/src/main.js:531-554`. There is **no** symmetric `pane_worktree` / `pane_cwd` Tauri command — **VERIFIED** by search of `fn pane_*` / `invoke("pane_` under the repo (only `pane_branch` is a frontend-facing branch/path command).

### Does the backend "know" branch without git?

**INFERRED (partially).** At creation time the name is always `agent-teams/{id}`. After spawn an agent (or freshen / human) can move HEAD; production deliberately re-reads via git rather than trusting the spawn string. Stamping only the spawn-time string onto a long-lived row would drift from HEAD — **INFERRED** from the existence and comments of `pane_branch` (`lib.rs:3690-3694`).

---

## Precedent — extending `QueueRow` additively

**VERIFIED pattern** (`core/mcp/src/lib.rs:47-52, 62-74, 250-261`):

```text
Identity fields (gap #4): role, tag, workspace are OPTIONAL + serde-additive
so rows for panes spawned before they existed still parse/serialize
(absent ⇒ omitted from the wire). role/tag are joined from the live registry
(enrich_queue) — the app records them at spawn.
```

Concrete mechanics:

1. **Struct fields** on `QueueRow` with  
   `#[serde(default, skip_serializing_if = "Option::is_none")]`  
   — `role` / `tag` / `workspace` at `:65-74`.
2. **Projection** leaves join fields `None` (`project` at `:129-141`); `workspace` is pure id-derivation (cheap).
3. **Join** in `enrich_queue` copies from `LiveRegistry` / `LiveWorkspace` by id (`:255-261`) — string clones only; no git.
4. **Writer** records values at spawn in `LiveWorkspace` (`lib.rs:1606-1622`).
5. **Tests** assert unset fields are **omitted**, not null (`core/mcp/src/lib.rs:2365-2370`); set fields appear when present (`:2373-2390`).

**Would `branch` / `worktree` fit?**  
**INFERRED yes, as a shape:** same optional + `skip_serializing_if` + enrich join.  
**VERIFIED gap today:** `LiveWorkspace` has neither field, and `enrich_queue` only copies `role` and `tag`. Fitting the pattern requires **writer + registry shape + enrich + QueueRow** — not a one-line field add. `workspace` is a weaker precedent for worktree path: it is derived from `id`, not joined from registry.

`QueueRow` is **`Serialize` only** (no `Deserialize` on the struct) — **VERIFIED** `:53-54`. Wire-compat for *emitters* is additive JSON keys; strict schema clients using `schemars` (MCP tool schemas) would see an expanded schema when the `schema` feature is on.

---

## Consumers + wire-compat

| Consumer | How it sees rows | Breaks if optional `branch`/`worktree` added? |
| --- | --- | --- |
| MCP `team_get_queue` | `identified_queue` → `Vec<QueueRow>` — `agent-teams-mcp/src/main.rs:222-226, 1020-1022` | **INFERRED no** for ignore-unknown JSON clients; **schema/docs** would need update (tool description currently lists optional `role`/`tag`/`workspace` only — `:209-220`) |
| MCP `get_workspace` | same row shape — `:239-254` | same |
| MCP resources `team://queue`, `team://workspace/{id}` | pretty-printed same rows — ~`:968-972` | same |
| App `list_queue` | `compute_queue` + `enrich_queue` — `lib.rs:2535-2547` | **INFERRED no** for Tauri serde to JS (extra keys appear on objects) |
| App board / rail / HUD | consume status/rank fields — `app/src/board-core.js`, `main.js` poll | **INFERRED no** if fields optional and unused |
| React `tauriAgentBridge` | maps known keys; does not read branch from row today | **Would not light up** unless bridge mapping is also changed |
| Mock bridge / demo data | local fake agents, not `QueueRow` | N/A |
| External MCP clients (unknown) | public contract | **COULD NOT DETERMINE** who is in the wild; any client that **rejects unknown keys** or pins a closed schema could break — reason this is a human decision |

**Wire-compat story if additive optional fields are chosen:**

- **Serialize path:** absent → key omitted (matches role/tag tests).
- **Old writers / app-down:** rows keep `None` → omitted; no false claims.
- **Schema feature / MCP tool text:** must be updated deliberately (docs lag would mislead agents).
- **Not a silent freebie for the React UI:** bridge still must assign `row.branch` / `row.worktree` (or UI must call another API).

---

## Cost of the join

`list_queue` / MCP queue are on a **poll path** (React bridge ~120–500ms tick comments in `tauriAgentBridge.js`; app also polls). Cost classes:

| Approach | IO per poll | Data source today? | Fit |
| --- | --- | --- | --- |
| **A. Registry join (like role/tag)** | O(rows) string clone after one registry read | **No** — must stamp `worktree` (and optionally spawn-time `branch`) onto `LiveWorkspace` at `do_spawn` | Cheap once stamped; correct for **path**; branch may go stale vs HEAD |
| **B. Per-row `git rev-parse` inside `compute_queue` / enrich** | N git subprocesses per tick | `pane_branch` proves the command works, but production keeps it **off** the poll path | Expensive; worst design for rank projection |
| **C. App-only: use `AppState.meta` in `list_queue`** | memory map lookup | **Yes** for worktree path; branch still needs git or derivation | Does **not** help MCP (no `AppState` in sidecar); splits app vs MCP row shape unless core stays ignorant |
| **D. UI one-shot `pane_branch` (+ optional worktree command)** | 1 git per pane **once** (prod pattern) | **Yes** for branch; path needs new command or registry stamp | No `QueueRow` change; React parity with `main.js` |
| **E. Derive without storage** | free | branch ≈ `agent-teams/{id}`; path ≈ need git_root | Branch derivation **drifts** if HEAD moves; path needs git_root which is **not** on live registry today |

**VERIFIED distinction:** role/tag join is cheap because values are **already in** `LiveWorkspace`. Branch/worktree are **not**. Putting git on `compute_queue` would be a category error for a ranking projection that today only reads `events.jsonl` + optional registry JSON.

---

## Options and trade-offs

### (a) Wire it through properly (public `QueueRow` + live registry)

**Steps (INFERRED implementation sketch, not done):**

1. Add optional `worktree: Option<String>` (and optionally `branch: Option<String>`) to `LiveWorkspace` and write them in `do_spawn` from `w.root` / `w.branch` (or leave branch unstamped and keep live git elsewhere).
2. Add matching optional fields on `QueueRow` with the role/tag serde attributes.
3. Extend `enrich_queue` to copy them.
4. Update MCP tool descriptions / schema tests.
5. Map fields in `tauriAgentBridge.js` (stop the `prev?.branch || ""` trap).
6. Decide branch freshness: spawn-time stamp vs still calling `pane_branch` for HEAD truth.

**Pros:** one wire shape for app + MCP; external orchestrators can name "which worktree"; matches gap-#4 identity precedent.  
**Cons:** **public contract change**; branch stamp may lie about current HEAD; requires coordinated writer/enrich/UI work; overkill if only the React chrome needs it.

### (a′) UI / app-local wire-through (no `QueueRow` change)

**Steps (INFERRED):**

1. On pane mount (or first poll), `invoke("pane_branch", { id })` like `main.js:542`.
2. Add a small `pane_worktree` (or return path from an existing meta helper) for the sub-header path string.
3. Cache on the agent object; do not re-git every 120ms unless product wants live HEAD in the chrome.

**Pros:** zero MCP/compat risk; reuses proven prod path; worktree path is already in `AppState.meta`.  
**Cons:** React UI diverges from MCP row shape; two APIs for pane identity; still app-up only (same as today's role/tag when registry absent).

### (b) Delete the dead UI

Remove or gate:

- Sub-header bar `AgentPane.jsx:400-401`
- "Copy branch" menu item / `hasBranch` plumbing (`PaneMenu.jsx:113-119`, `AgentPane.jsx:397`)

**Pros:** stops lying; smallest honest fix; no contract change; mock-only fields can stay mock-only.  
**Cons:** loses chrome that product may still want; mock preview and Tauri diverge less only because both stop showing empty chrome (mock currently shows fake branch).

### (c) Leave as-is

**Pros:** zero work.  
**Cons:** permanent `⎇  · ` bar and a forever-disabled menu item in the real app; mock continues to hide the bug.

---

## What could NOT be determined

1. **Whether any external MCP client requires branch/worktree on the queue** — no in-repo consumer of those fields on `QueueRow` was found; out-of-repo clients are unknown.
2. **Whether product wants the sub-header as operator UX** vs decorative parity with mock — no product spec found in-lane.
3. **Exact live registry `repo` semantics for non-git / sparse cwd** — writer uses `ps.repo`; agent cwd may be `w.cwd` (subpath) while meta stores `w.root` — which string the UI should show was not product-specified.
4. **Daemon-owned panes** (`daemon_spawn` flag, default OFF) — whether meta / live registry / worktree registry stay complete for daemon-owned ids was not fully traced end-to-end in this investigation; default builds use in-process `do_spawn` (**VERIFIED** routing comments at `lib.rs:1407-1415`).
5. **Whether freshen / agent checkout changes the desired displayed branch** for the React chrome — production shows live HEAD; a spawn-time stamp would not.

---

## Explicit design questions for the human

1. **Is branch/worktree chrome a product requirement for the React Command UI, or is deleting the empty bar + disabled menu item acceptable?**  
   (If delete is OK → choose **(b)**; stop here.)

2. **Do external MCP consumers (`team_get_queue` / `get_workspace` / `team://queue`) need `branch` and/or `worktree` on the public wire?**  
   - **No** → prefer **(a′)** UI/app-local (or **(b)**); do **not** burn a `QueueRow` change on a React-only bug.  
   - **Yes** → choose **(a)** and specify: spawn-time path only, or live HEAD for branch?

3. **If (a): should `branch` mean (i) spawn label `agent-teams/{id}`, or (ii) current `HEAD`?**  
   (ii) implies either poll-time git — expensive — or a separate live command, which undermines putting it on the queue row.

4. **If (a): is `LiveWorkspace.repo` (source folder) enough for callers, or must the isolated path `{git_root}/.agent-teams-worktrees/{id}` be a new field?**  
   Today the live registry does **not** record the isolated path — **VERIFIED**.

5. **Accept additive optional fields on the public MCP contract** (schema + tool prose + any closed-schema clients), knowing role/tag already established that precedent?

---

## BOUNDARIES

Seen during investigation; **not fixed** (out of lane / behavior change forbidden):

- React bridge empty self-propagation (`tauriAgentBridge.js:211-212`) — fix is a later implementation lane after this design decision.
- Dead UI chrome (`AgentPane.jsx:400-401`, `PaneMenu.jsx:117`) — same.
- `ui/HANDOFF.md:78-80` claims `QueueRow` has `branch`/`worktree` — doc drift.
- `LiveWorkspace.repo` doc says "repo/worktree path" but writer stores source `ps.repo` — naming/docs ambiguity.
- Production already has working branch chip; React UI did not port it — product/engineering gap across frontends.
- `restartAgents` / bridge stubs / `Home.jsx` — known wave issues; other lanes.
- No change to `QueueRow`, registry, or UI in this lane — document only.
