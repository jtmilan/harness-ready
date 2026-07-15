# REQUIRES-HUMAN-DESIGN: Liveness Blindness

**Lane:** L7 (read-only investigation)  
**Date:** 2026-07-14  
**Scope:** Why live panes accept `team_send_input` but are absent from `agent-teams-live.json`, `team_get_queue`, and `team_read_output` live scrollback.

---

## Observation

Workspace `ws29184x2` has **11 live panes** (p1–p8 Grok, p9 CommandCode, p10 cursor-agent, p11 OpenCode) with ~18h process uptime. All accepted `team_send_input` during the probe. p12 correctly returned `UNKNOWN_WORKSPACE`; p13 correctly returned `DEAD_PANE`.

Yet for p1 and p9 (representative):

| Surface | Result |
|---------|--------|
| `~/Library/Application Support/agent-teams-live.json` | No `ws29184x2-p1` … `ws29184x2-p11` |
| `team_get_queue` | No rows for those pane ids |
| `team_read_output` | `source: "none"` with note that pane is not in the live registry |

**Contradiction (VERIFIED at probe time):** Mutation path (`team_send_input`) treats panes as live; read/queue path treats the same ids as not live.

Current prod registry snapshot (during investigation): `app_pid` 32402, `live_ids` include `ws29184x2-p14` and `ws92431x3-p0`–`p4` only — not p1–p11.

Disk under prod `state_root` (`~/Library/Application Support/agent-teams/`):

- `ws29184x2-p1`: **no** `events.jsonl` (Grok / state_blind)
- `ws29184x2-p9`: **has** `events.jsonl` (CommandCode SessionStart)
- `ws29184x2-p10`, `ws29184x2-p11`: **have** `events.jsonl` (cursor-agent, OpenCode — not state_blind)

So missing queue membership is **not** explained solely by `state_blind` / missing hooks.

---

## Root cause(s) with evidence

### RC1 — Two different liveness sources (primary divergence)

**VERIFIED:** `team_send_input` and `team_read_output` do not share a liveness definition.

| Path | Liveness source | Key code |
|------|-------------------|----------|
| **Mutations** (`team_send_input`, split-write) | In-memory `DaemonSups` in the running app: `contains(id)` + `sup.is_alive()` | `core/daemon/src/handlers.rs` (~146–215): resolves pane via app socket → supervisor map |
| **Reads** (`team_read_output` live scrollback) | Disk `agent-teams-live.json` via `registry_lookup()` | `agent-teams-mcp/src/read_output.rs` (~114–119, 171–209, 219–222): `pane_is_live = reg_row.is_some()`; without registry row, live scrollback is skipped and note says pane is not in live registry |

**Exact divergence:** A pane can be alive in **Instance A’s `sups`** (send works) while **absent from `live.json`** (read/queue blind).

### RC2 — `live.json` cleared on every app startup

**VERIFIED:** On each app launch, before panes respawn:

```rust
// app/src-tauri/src/lib.rs ~13637
live_registry_write(&state_root, Some(std::process::id()), None, Vec::new());
```

This **always** writes an empty `live_ids` vector (not conditional on instance lock).

**VERIFIED:** Registry is repopulated only when panes spawn through `do_spawn` → `live_registry_update` (`app/src-tauri/src/lib.rs` ~1601–1624) → `live_registry_write` (`~2655–2674`). Path: sibling of `state_root`, `<parent>/agent-teams-live.json` (`core/mcp/src/lib.rs` ~301–317).

**INFERRED:** Long-lived PTY children from a **prior** session are not re-inserted into `live.json` after restart unless respawned through current app’s `do_spawn`.

### RC3 — Multi-instance / registry clobber

**VERIFIED:**

- `default_state_root` (prod): `~/Library/Application Support/agent-teams` (`app/src-tauri/src/lib.rs` ~2704–2709).
- Harness-ready fork uses `~/Library/Application Support/harness-ready/agent-teams` (isolated `default_state_root`).
- `instance.lock` per `state_root`; second instance may skip `remove_dir_all(state_root)` (`~13585–13608`) but **still** runs empty `live_registry_write` at startup (`~13637`).
- Socket bind: second instance may disable mutations if socket already held (`spawn_socket_listener` ~12515).

**INFERRED footgun:** Instance B starts while Instance A still holds live panes in `sups`. B overwrites `agent-teams-live.json` to empty. MCP reads B’s (or shared file’s) empty registry → queue/read blind; `team_send_input` may still reach A via A’s socket and `sups`.

### RC4 — MCP queue filtered by disk registry, not `sups`

**VERIFIED:** `identified_queue` in `agent-teams-mcp/src/main.rs` (~1020–1022) calls `compute_queue_identified`, which when registry **is present** filters to `registry.live_ids` only (`core/mcp/src/lib.rs` ~192–201). Empty or stale registry ⇒ empty or wrong MCP queue even if events exist on disk.

**VERIFIED:** In-app `list_queue` uses `state.sups.live_ids()` in-memory (`app/src-tauri/src/lib.rs` ~2535–2547), optionally enriched from registry — **not** the same filter as MCP.

**INFERRED:** GUI queue can show panes MCP `team_get_queue` omits when registry is stale.

### RC5 — `state_blind` is a separate axis (hooks / events), not registry membership

**VERIFIED:** `state_blind: true` for CommandCode, Cline, Grok (`core/harness/src/lib.rs` ~193–232). Synthetic `SessionStart` via `write_spawn_ready_event` at spawn (`core/supervisor/src/lib.rs` ~1167–1183).

**VERIFIED:** p9 (CommandCode) has `events.jsonl` but was still absent from queue when not in `live.json`. p10/p11 have events and are **not** state_blind — same absence pattern.

**Conclusion:** `state_blind` explains missing or thin hook telemetry; it does **not** explain MCP queue omission when the root issue is registry vs `sups`.

### RC6 — MCP default state dir vs prod panes (configuration footgun)

**VERIFIED:** MCP sidecar default state dir is harness-ready path unless `$AGENT_TEAMS_STATE_DIR` is set (`agent-teams-mcp/src/main.rs` ~1027–1032). Pane spawn sets `AGENT_TEAMS_STATE_DIR` to app’s `state_root` (`core/supervisor/src/lib.rs` ~1291).

**INFERRED:** External MCP clients without env alignment read wrong `live.json` / events tree — amplifies blindness but is not required to explain ws29184x2 probe (prod paths were used).

---

## One bug or several?

**Recommendation: several distinct bugs / design gaps**, not a single fix:

| ID | Issue | Coupling |
|----|--------|----------|
| **B1** | **Split liveness authority** — mutations use `sups`, reads/queue use `live.json` | Core; RC1 |
| **B2** | **Startup clears registry** without reconciling existing live children | RC2; orphan panes |
| **B3** | **Multi-instance registry write** — non-owner instance can empty shared `live.json` | RC3 |
| **B4** | **GUI vs MCP queue semantics** — in-memory vs disk filter | RC4 |
| **B5** | **`state_blind` residue** — no `events.jsonl` for some live panes; queue discovery from events alone is incomplete | RC5; orthogonal to B1 |
| **B6** | **State dir / MCP env mismatch** — wrong tree for external tools | RC6 |

Fixing B1 alone (unify liveness) does not automatically fix B5 (hook-less harnesses) or B3 (multi-instance policy).

---

## Options and trade-offs

### Option A — Single source of truth: in-memory `sups` (app-authoritative)

- MCP `read_output` / `identified_queue` query app socket for live id set (new RPC or extend existing).
- **Pros:** Matches mutation truth; fixes send vs read contradiction.
- **Cons:** MCP requires running app + socket; no offline queue; more coupling.

### Option B — Single source of truth: `live.json` (disk-authoritative)

- Reject `send_input` if id ∉ registry; periodic reconcile from `sups` → registry.
- **Pros:** MCP stays file-based; auditable snapshot.
- **Cons:** Must fix startup clear + multi-instance clobber; lag if reconcile is periodic; still wrong if registry empty but PTY alive until reconcile.

### Option C — Reconcile on startup and on instance lock

- On app start: scan `state_root` workspaces + `sups` / PTY liveness → rebuild `live.json` instead of `Vec::new()`.
- Only one writer holds `instance.lock` for registry updates.
- **Pros:** Addresses RC2/RC3 without full architecture flip.
- **Cons:** Heuristic for “orphan” PTYs; multi-instance policy still needs human decision.

### Option D — Per-instance registry files

- `agent-teams-live-<pid>.json` or nest under `state_root`; MCP merges or follows `AGENT_TEAMS_APP_PID`.
- **Pros:** Stops cross-instance clobber.
- **Cons:** Merge semantics complex; external tools need new contract.

### Option E — Document + fail loud

- `team_read_output` / queue: if send would succeed but registry missing, return explicit `LIVENESS_MISMATCH` (not `source: none`).
- **Pros:** Low code churn; surfaces B1 for operators.
- **Cons:** Does not fix blindness; shifts burden to humans.

**Trade-off summary:** Unification (A or B) is a **product decision** (what does “live” mean?). Reconcile (C) is a **minimal patch** that may paper over B1 until unified.

---

## What could NOT be determined (this investigation)

- **INFERRED:** Which exact app instance (pid / bundle: GlikaAgents vs DevTest) currently owns p1–p11 `sups` — probe confirmed send works but did not capture owning `app_pid` at send time.
- **INFERRED:** Whether a second instance startup occurred during the 18h window (would explain empty registry + live send).
- **INFERRED:** Full chronology of `live.json` writes (no append-only audit log for registry).
- Whether p1–p8 “DevTest sidecar” PIDs imply DevTest `state_root` or only DevTest-built MCP binary with prod `AGENT_TEAMS_STATE_DIR`.
- Whether `team_orchestrate` / `team_broadcast` from external MCP were exercised on p1–p11 during probe (code path uses `sups` — likely works; not re-verified in this lane).

---

## Explicit design question(s) for human

1. **What is the canonical definition of “live pane”?** PTY child running, entry in `sups`, row in `live.json`, or hook-visible session?

2. **Should MCP reads and mutations use the same gate?** If yes, which source wins when they disagree?

3. **Multi-instance policy:** Is a second Agent Teams instance on the same `state_root` supported? If not, should startup fail hard instead of clearing `live.json`?

4. **Orphan PTYs after restart:** Should the app re-adopt long-running children into registry/`sups`, or kill them on startup wipe?

5. **Queue for `state_blind` harnesses:** Should rank/queue work with zero `events.jsonl`, or is “no hooks = invisible to queue” acceptable?

6. **Operator contract:** Should `agent-teams-live.json` remain a public API for BridgeSpace / external orchestrators, and must it be strongly consistent with send?

---

## Blast radius — surfaces that silently lie

| Surface | Behavior when registry stale | Code anchor |
|---------|------------------------------|-------------|
| `team_get_queue` / `team://queue` | Empty or subset; filters to `live_ids` when registry file exists | `core/mcp/src/lib.rs` ~192–201; `agent-teams-mcp/src/main.rs` ~1020–1022 |
| `team_read_output` (live scrollback) | Skips scrollback; `source: none` + misleading “not in live registry” | `agent-teams-mcp/src/read_output.rs` ~171–209, ~219–222 |
| `agent-teams-live.json` consumers | Any tool reading sibling file without socket | `core/mcp/src/lib.rs` ~301–317 |
| In-app queue vs MCP queue | GUI may list `sups.live_ids()` while MCP lists registry only | `app/src-tauri/src/lib.rs` ~2535–2547 vs MCP path above |
| `dead_pane_ids` (GUI) | Uses `sups` + registry keys; comments note registry keys can linger | `app/src-tauri/src/lib.rs` ~2550–2579 |
| `team_orchestrate` / `team_broadcast` (MCP) | Uses app `live_pane_ctxs` from **sups** — may target panes absent from queue | `app/src-tauri/src/lib.rs` ~6597–6614 (pattern; exact MCP bridge not re-run) |
| `team_synthesize` (pane ids) | `read_output::resolve` — registry-gated like read | `agent-teams-mcp/src/read_output.rs` |
| Event-discovered queue (registry absent) | May show **stale** panes from old `events.jsonl` | `compute_queue` when registry missing — inverse failure mode |

**Silent lie pattern:** Mutations succeed → operator assumes pane is observable → read/queue/report paths return empty or “none” without a hard error that mutations would succeed.

---

## BOUNDARIES

- **Read-only:** No code, config, or behavior changes in Agent Teams, harness-ready, or prod state.
- **Deliverable only:** This document under `at-hr-liveness/docs/` on branch `docs/liveness-blindness`.
- **No push / no merge** per lane instructions.
- **Not in scope:** Fixing registry logic, adding reconcile, changing startup `live_registry_write`, or unifying MCP default state dir.
- **Claims:** Tagged VERIFIED / INFERRED in body; probe timestamps and registry snapshots reflect 2026-07-14 investigation window.
