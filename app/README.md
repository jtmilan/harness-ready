# Agent Teams (MVP)

Local macOS app: a **team of interactive coding agents** (claude / cursor / opencode / codex / commandcode / cline, or `bash` to smoke-test), each in its own git worktree + PTY, surfaced through **one ranked queue of which agent needs you right now**.

## Run (dev)

Use the isolated dev launcher — it gives the dev instance its OWN identifier,
state dir, socket, and registry so it can NEVER clobber the installed
/Applications app or its data (the fixed-name socket + live-registry are
siblings of the state dir, so pointing a dev run at the production state dir
steals production's socket):

```bash
cd agent-teams/app && bun install   # first time only
../scripts/dev-isolated.sh          # isolated dev instance (safe next to prod)
```

## Use

- **“+ New” (⌘N)** — pick a harness (claude / cursor / opencode / codex / commandcode / cline, or `bash` to smoke-test), a repo path, and an optional initial prompt → spawns the agent in a `git worktree` with hooks injected.
- **Left rail** — the ranked *who-needs-you* queue. `►` marks an agent **waiting on you** (approval/question), sorted above working/done. Click a row to focus its terminal.
- **⌘⇧J** — OS-global hotkey (via `tauri-plugin-global-shortcut`): raises the window and jumps to the top of the queue, even when the app is in the background.
- **Main pane** — the focused agent's live PTY. Type to drive it; answer approval prompts in place (Model A).

## Architecture

| Crate | Role |
|---|---|
| `core/state-adapter` | normalize harness hook events → `{state, waiting_reason, needs_human}` + ranking (Phase 01) |
| `core/supervisor` | spawn a harness in a PTY inside a git worktree; stream I/O; lifecycle (Phase 02-01) |
| `app` | Tauri v2 shell + xterm.js terminal + ranked queue (Phase 02-02/03) |

Full design + decision log: `../PRD.md`, `../SUMMARY.md`, `../.paul/`.

## Known follow-ons (post-MVP / v1)

- cursor `rate_limit` detection (stderr tail, not a hook) → v1 scheduler.
- claude `.claude/settings.local.json` merge (not clobber) for repos that already have one.
