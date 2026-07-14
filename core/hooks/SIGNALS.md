# SIGNALS.md — verified per-harness block-signal contract

> Task 1 (recon) output for Plan 01-01. This is the contract `normalize()` (Task 2) encodes.
> **Evidence tiers:** `[installed]` = read from a real installed config on this machine (primary source); `[spike]` = observed in the 2026-06-02 cursor-agent run (`spike-evidence/`); `[by-design]` = forced by documented CLI flag semantics; `[AC-6]` = to be live-confirmed at the human-verify checkpoint.

## The core correction (D9)

`beforeShellExecution` / `PreToolUse` fire before **every** command — they are *interceptions*, not *blocks*. Whether a given interception is a "needs-you" block is a function of the **policy decision the hook returns** (allow ⇒ routine/working; defer/ask ⇒ the harness shows its native prompt ⇒ waiting). The writer records that decision in the event; the normalizer reads `{event, decision}`. Auto-allow-everything ⇒ no waits ⇒ empty queue ⇒ no product.

## Response model = Model A (MVP)

Hook **detects + defers**; the harness shows its **own native TUI prompt**; the human answers in the **live PTY** (FR-3); the app only routes attention (FR-6). No app-side approval UI until Phase 02+.
- cursor: native prompting is the default — `--force`/`--yolo` (force-allow) and `--trust` exist precisely to *bypass* it, and `--trust` only works in `--print`/headless `[by-design]`. So an interactive (non-`--force`) session prompts natively when the hook doesn't return allow.
- claude: `PreToolUse` can return `{"hookSpecificOutput":{"permissionDecision":"allow|deny|ask"}}` `[installed]` (the SOUL.md guard in `~/.claude/settings.json` returns `deny` — proves the schema). `ask` ⇒ native prompt.

---

## CLAUDE — signal map  `[installed: ~/.claude/settings.json + ~/.vibeyard/run/, the canonical reference (D3)]`

Vibeyard writes `<Event>:<status>` to a per-session status file. Its mapping (primary source):

| Claude hook event | Vibeyard status | → AgentState `{state, waiting_reason, needs_human}` | Notes |
|---|---|---|---|
| `SessionStart` | `waiting` | `{working, –, false}` | session up, awaiting first prompt; not a block |
| `UserPromptSubmit` | `working` | `{working, –, false}` | |
| `PreToolUse` | (logged) | `{working, –, false}` | interception, NOT a block on its own (D9) |
| `PostToolUse` | `working` | `{working, –, false}` | tool ran → resolved |
| **`PermissionRequest`** | **`input`** | **`{waiting, approval, true}`** | **THE BLOCK** — claude awaiting your approval |
| `PermissionDenied` | (logged) | `{working, –, false}` | denial processed → resumes |
| `Notification` | — | `{waiting, question, true}` *iff* msg = needs-input | secondary/soft; PermissionRequest is primary |
| `Stop` | `completed` | `{done, turn_end, false}` | turn-done, **NOT blocked** (D4) |
| `StopFailure` | `waiting` | `{error, –, false}` | failed turn; surface as error |
| `SubagentStop` | (logged) | unchanged | sub-turn boundary |

**Claude block signal = `PermissionRequest`** (Vibeyard → `input`). This resolves the open recon question: claude exposes a dedicated permission-wait hook event; we do not depend on scraping or on `Stop`.

---

## CURSOR — signal map  `[spike + installed ~/.cursor/hooks.json + by-design flags]`

Spike fired (allowed path): `sessionStart`, `beforeShellExecution`×2 (payload carried `command`), `afterShellExecution`, then errored on `resource_exhausted` (no `stop`). Global `~/.cursor/hooks.json` uses `preToolUse` (matcher `Shell`) `[installed]`.

| Cursor hook event | + writer decision | → AgentState | Notes |
|---|---|---|---|
| `sessionStart` | — | `{working, –, false}` | `[spike]` carries conversation_id + model |
| `beforeShellExecution` / `preToolUse` | **`allow`** | `{working, –, false}` | routine — NOT a block (allowlisted/safe) |
| `beforeShellExecution` / `preToolUse` | **`defer`/`ask`** | **`{waiting, approval, true}`** | **THE BLOCK** — cursor shows native prompt `[AC-6]` |
| `afterShellExecution` | — | `{working, –, false}` | `[spike]` command ran → resolved |
| `stop` (clean finish) | — | `{done, turn_end, false}` | `[AC-6]` spike couldn't confirm — it errored first |
| `resource_exhausted` (stderr, **NOT a hook**) | — | `{waiting, rate_limit, false}` | ⚠ surfaced via output text, not a hook → Phase-02 supervisor tails stderr; **out of Phase-01 hook-only scope** |

**Cursor block signal = a `beforeShellExecution`/`preToolUse` the writer chose to DEFER** (so cursor prompts natively). The allow-vs-defer decision is the writer's allowlist policy (Task 3), recorded in the event.

---

## What this confirms / leaves open

- ✅ **Block signal established for both harnesses** from primary sources — claude `PermissionRequest` `[installed]`, cursor deferred-`beforeShellExecution` `[spike+by-design]`. AC-0 met for the normalizer's purposes.
- ✅ `Stop`/`stop` = done, never blocked (D4) — confirmed `[installed]`.
- 🔶 **`[AC-6]` live confirmations** (the human-verify checkpoint, not blocking the normalizer):
  1. cursor `beforeShellExecution` returning defer/ask in a **true interactive** (non-`--force`) session shows the native prompt and does **not** hang.
  2. cursor `stop` fires on a **clean** finish (spike errored before it could).
- ⚠ **rate_limit is not hook-derivable for cursor** (stderr text) → Phase-02 supervisor concern; Phase-01 adapter maps it only if a wrapper feeds it the line.

## Per-harness asymmetry (for the normalizer)

| | claude | cursor |
|---|---|---|
| genuine block event | `PermissionRequest` | `beforeShellExecution`/`preToolUse` + decision=defer |
| resolved event | `PermissionDenied` / next `PostToolUse` | `afterShellExecution` |
| done | `Stop` | `stop` |
| error | `StopFailure` | (stderr `resource_exhausted` — non-hook) |
| decision channel | `PreToolUse` `permissionDecision: allow\|deny\|ask` | hook return `{"permission":"allow"}` / defer |
