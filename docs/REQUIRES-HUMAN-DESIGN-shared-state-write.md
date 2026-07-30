# REQUIRES-HUMAN-DESIGN: Shared-State Write Path (R-MCP-RW)

**Lane:** Phase 7 UI slice only — indicator ships; write path is HD  
**Date:** 2026-07-29  
**Scope:** A read-write shared task/context layer agents can write under gates. The *write path*, *gating*, and *injection-safety* cross the mutation boundary and are **not implemented here**. This note frames the open design so a future phase does not have to guess the gate.  
**Related:** [`REQUIRES-HUMAN-DESIGN-liveness-blindness.md`](./REQUIRES-HUMAN-DESIGN-liveness-blindness.md) (split-authority pattern this design must not repeat); [`RESEARCH-SYNTHESIS.md`](./RESEARCH-SYNTHESIS.md) channel-A / channel-B distinction.

---

## Observation (what ships in Phase 7 vs what does not)

**Ships (UI slice only):**

- A pure helper `ui/src/lib/sharedState.js` (`sharedStateLabel`, `isWritable`) — unit-tested, never throws, returns the safe read-only default for any unknown input.
- A **non-interactive** status chip `SharedStateBadge.jsx` (a `<span>`, not a button; no `onClick`) that renders the known default.
- The chip is wired into `TitleBar.jsx` so the mode is visible in the header.
- An honest label: the dynamic mode value is **NEEDS-BACKEND**.

**Does NOT ship (gated-OFF, HD):**

- Any write path from agents into shared state.
- Any gate logic (arm/disarm, per-op allow/deny, audit).
- Any change to `ui/src/lib/agentBridge.js` or `tauriAgentBridge.js`.
- Any Rust / `core/mcp` / `app/src-tauri` / `harness-ready-mcp/` code.
- Any new dependency.

The write path is the mutation boundary. Shipping guessed gate code would violate the boundary and would repeat the silent-lie pattern documented in the liveness-blindness note (RC1: split liveness authority).

---

## Problem

The harness-ready UI today projects **read-only** state: context graphs, liveness, monitoring, orchestration reports. Agents read projections; operators read projections; nothing in the UI invites an agent to write back into a shared layer that other agents and the operator also read.

R-MCP-RW proposes adding an *optional* read-write shared task/context layer — a place for agents to:

- Claim a task slice (`## BOUNDARIES` markdown under `<run_dir>/<id>.md` is already half of this).
- Publish structured state that survives across orchestrate→synthesize fan-in.
- Coordinate with sibling panes on a shared artifact (not just via the coordinator's fan-out text).

The moment agents can **write** into a shared layer, four failure classes open that the read-only projection does not have:

1. **Untrusted-content injection.** Agent output is untrusted. A shared layer that accepts writes without sanitization becomes an injection vector for sibling agents (prompt injection via shared-state reads) and for the operator (UI rendering of untrusted content).
2. **Gate bypass.** A write path without an explicit arm/disarm control and per-op allow/deny turns every agent bug into a shared-state mutation. The default must be gated-OFF and the arming must be a confirmed, operator-initiated action — never a silent toggle.
3. **Split authority.** If the UI thinks the gate is closed but the backend accepted a write (or vice versa), we reproduce the liveness-blindness bug at the mode layer: the indicator lies, the operator acts on a false assumption.
4. **Multi-instance / ownership collisions.** Two Agent Teams instances on the same `state_root` (see liveness-blindness RC3) can already clobber `live.json`. A shared-state write layer with no ownership model will clobber shared artifacts the same way.

---

## Open design questions (for human)

### 1. Write boundary scope

What exactly is allowed to write into the shared layer?

- **(a)** Only agents spawned by the current workspace (pane-ids under the current `wsNNNNNxK`)?
- **(b)** Any agent the operator explicitly names (cross-workspace writes)?
- **(c)** The operator, via a dedicated UI surface?
- **(d)** The coordinator pane only (single-writer, fan-in model)?

Each option has a different trust surface and a different audit shape. Option (d) composes cleanly with the existing `team_orchestrate` fan-out but centralizes authority in the coordinator persona. Option (a) is the least surprising and matches the current worktree-isolation model.

### 2. Per-op gating vs session-level arming

Is the gate **per write operation** (every write prompts/decides), or **session-armed** (operator arms the write layer once for a window, writes flow until disarm)?

- Per-op: safer, more friction, audit is trivial (each op is a decision).
- Session-armed: lower friction for long orchestrate runs, but the disarm action must be **explicit + confirmed** (R3 — never a silent toggle), and the armed window must be visible in the UI at all times.

Recommendation (not binding): session-armed with a per-op audit log, because a pure per-op gate is too noisy for orchestrate fan-out and a pure session-arm without per-op audit hides individual writes.

### 3. Injection safety / untrusted-content handling

Agent writes are untrusted input. What is the sanitization contract?

- Markdown writes: must pass through the existing markdown renderer's sanitization (no raw HTML, no `javascript:` URLs).
- Structured-state writes (JSON): schema-validated at the write boundary; writes that fail schema are rejected, not silently coerced.
- Cross-agent reads: a pane reading shared state must treat the content as **untrusted** — no eval, no dynamic code, no trust in embedded instructions.

This mirrors the broader untrusted-content question in R-MCP-RW research and is a hard prerequisite for any write path.

### 4. Default state and arming UX

The gated-OFF default is non-negotiable (per the research). The arming UX must:

- Be a distinct control from the status indicator (the indicator is **read-only**; the arming control is a separate, future, explicitly-gated surface).
- Require explicit operator confirmation (not a single click, not a keyboard shortcut that can be hit by accident).
- Be reversible (disarm at any time).
- Surface the armed window prominently while armed (the indicator's tone change from cyan to amber is the minimum; consider a persistent banner).

### 5. Audit and observability

Every write must be:

- Attributed to a pane-id + workspace-id + timestamp.
- Append-only (no silent overwrite — even "updates" are append-with-version).
- Queryable by the operator (a future `team_read_shared_state` surface, analogous to `team_read_output`).
- Included in the existing `team_audit_log` ledger shape (or a sibling ledger if the volume profile differs).

Without an audit surface, a write path is indistinguishable from a silent corruption path on day two.

### 6. Channel-A / channel-B composition

Per `RESEARCH-SYNTHESIS.md` and the polyglot-broker model:

- **Channel-A** is the persona / SSOT channel — who the agent is, what harness it runs on, identity-bearing state.
- **Channel-B** is the task / domain channel — the curated brief for the task at hand (TypeScript/React, Rust, etc.).

Shared state crosses both:

- Writes are **channel-A attributable** (this pane, this harness, this identity wrote this).
- Reads are **channel-B consumed** (a TypeScript pane reads a Rust pane's shared artifact as task content, not as persona).

The gate and the sanitization contract must respect this split. A write from channel-A must never be allowed to rewrite another pane's channel-A identity (no persona mutation via shared state). A read into channel-B must never trust channel-A-origin content as if it were operator-authored.

### 7. Multi-instance / ownership

Two Agent Teams instances on the same `state_root` (see liveness-blindness B3) must not be able to:

- Concurrently write to the same shared-state key without a defined conflict resolution.
- See each other's armed windows (arming is instance-scoped, not disk-global).
- Read stale shared state from a prior instance without an explicit staleness marker.

This likely implies per-instance shared-state namespaces (like the per-instance registry proposal in liveness-blindness option D) and is coupled to whichever option is chosen there.

### 8. Failure modes and the honest indicator

The indicator shipped in Phase 7 is honest about what it does not know:

- When the backend is unreachable or returns an unknown value, the indicator shows **read-only** (the safe default), not "unknown" or an error state.
- The indicator labels the dynamic value **NEEDS-BACKEND** so a future operator reading the UI today knows the value is not yet live.
- If a future implementation ever lies about the mode (backend says read-only but accepted a write), that is the exact split-authority bug from liveness-blindness RC1 — and must be fixed at the backend, not papered over in the UI.

---

## Proposed UI contract (Phase 7 indicator + future arm/disarm)

### Indicator (ships Phase 7)

- `SharedStateBadge` is a **status chip**, not a control. `<span role="status">`, no `onClick`, no hover affordance implying user control.
- Tone: cyan/muted for read-only (the real default); distinct amber for read-write. Neither uses a sacred health lane (`--need`, `--danger`) because mode is not health.
- Accessible: `aria-label` + `title` describe the gated-OFF default and the NEEDS-BACKEND state.
- Helper: `sharedStateLabel(mode)` + `isWritable(mode)` — pure, never throws, defaults to read-only for unknown input.

### Bridge method (NEEDS-BACKEND — NOT implemented in Phase 7)

```js
// Proposed addition to tauriAgentBridge.js + MockAgentBridge (interface parity):
//
//   /**
//    * @returns {Promise<'read-only' | 'read-write'>}
//    *   'read-only'  = write gate closed (default; R-MCP-RW gated-OFF).
//    *   'read-write' = write gate open after explicit operator arm + confirm.
//    *   Never throws; on backend error resolves to 'read-only'.
//    */
//   async getSharedStateMode() { ... }
```

### Arm/disarm control (future — out of scope for Phase 7)

- MUST be a distinct surface from the indicator (separate button / dialog, not the chip).
- MUST require explicit operator confirmation (dialog with a typed confirmation, or at minimum a second click in a confirm state — never a single click).
- MUST surface the armed window prominently while armed.
- MUST be reversible at any time (disarm).
- MUST emit an audit row per the audit contract above.

---

## What could NOT be determined (this phase)

- The backend shape of the write gate (Tauri command name, Rust handler location, whether it lives in `core/mcp` or `app/src-tauri`).
- The exact sanitization library / contract for markdown writes (candidates: `DOMPurify` on the read side; schema validation on the write side; both likely needed).
- The audit ledger format and whether shared-state writes fit in the existing `team_audit_log` or need a sibling.
- Whether the arm/disarm control lives in the TitleBar, in a dedicated Shared State panel, or in a command palette action.
- Multi-instance policy — coupled to the decision in liveness-blindness B3.

---

## Explicit design question(s) for human

1. **Write boundary scope** — option (a) pane-local, (b) named cross-workspace, (c) operator-only, or (d) coordinator-only? Or a matrix?
2. **Per-op gate vs session-arm** — which model, and if session-arm, what is the max window and the disarm trigger?
3. **Sanitization contract** — markdown-only, schema-validated JSON, or both? Which library?
4. **Arming UX** — where does the control live, and what confirmation step is required?
5. **Audit surface** — extend `team_audit_log` or sibling ledger?
6. **Channel-A/B split** — confirm the rule that writes are A-attributed and reads are B-consumed, with no cross-contamination.
7. **Multi-instance** — coupled to liveness-blindness B3; resolve that first.
8. **Indicator evolution** — when the backend ships, should the indicator auto-refresh (subscription) or poll? Subscription is preferred for truth-alignment (see liveness-blindness option A).

---

## Blast radius — what silently breaks if we guess the gate

| Surface | Risk if guessed | Mitigation |
|---------|-----------------|------------|
| `agentBridge.js` / `tauriAgentBridge.js` | New method added without backend parity → mock lies, Tauri throws | Do not add `getSharedStateMode` until both implementations are ready |
| `SharedStateBadge` | Evolving into a toggle/button → implies user control over mode | Hard rule R3 — `<span role="status">`, no `onClick`, no button role |
| Health-lane color tokens (`--need`/`--danger`) | Using them for mode makes mode look like a health state | Hard rule R5 — cyan/muted for read-only, amber for read-write |
| Cross-agent reads | Trusting shared-state content as operator-authored | Reads must treat content as untrusted (separate HD note) |
| Audit | No ledger → silent corruption on day two | Audit contract above is a hard prerequisite |
| Multi-instance | Same clobber pattern as `live.json` | Coupled to liveness-blindness B3 |
| Backend mode vs UI mode drift | Split authority → the liveness-blindness bug, again | Subscription + single source of truth (liveness-blindness option A) |

---

## BOUNDARIES

- **No write-path code ships in Phase 7.** Would be guessing the gate.
- **No edit to `ui/src/lib/agentBridge.js` or `tauriAgentBridge.js`.** Indicator calls the pure helper with `undefined` (no backend source yet).
- **No Rust / `core/mcp` / `app/src-tauri` / `harness-ready-mcp/` code.** Zero files under those trees are created or modified in this phase.
- **No new dependency.** Uses existing `lucide-react` icons.
- **Indicator is non-interactive.** `<span role="status">`, no `onClick`, no button semantics (R3).
- **Tone is not a health lane.** Cyan/muted (read-only) and amber (read-write) — neither uses `--need` / `--danger` (R5).
- **HD note only** for the write path, gating, injection-safety, audit, and multi-instance questions above.
- **Claims:** Tagged in body; this note is a design document, not an implementation.
