# Auto-ship runbook — delegate → Bridge → PR, with one human touch

The goal: describe a feature, let agents implement + the system verify it, and
get a **reviewable PR** — without running the gate by hand. The only manual step
is clicking **Merge** on a green PR (a PR is reversible; an unattended merge to
main is not — and the verify gate exists precisely because agent code that
*looks* fine fails the merged-tree test).

This pipeline has two halves:

| Half | Driven by | Why |
|---|---|---|
| **Front** — agents write the code | the **app UI** (delegate + Bridge) | spawning workers + dispatching to live PTY panes is GUI/PTY-bound; not scriptable |
| **Tail** — gate + ship | **`scripts/auto-ship.sh`** | the fold + `cargo test` + push + PR is pure git/CLI → fully automatable |

## Hard constraints (learned the hard way)

- **The gate is `cargo test`.** A pure UI/CSS/JS feature has zero cargo coverage →
  verdict is always HOLD → it always needs human review. For a feature that can
  *auto-pass*, give it **backend test coverage**.
- **Scope the gate to a model-free crate** (`core/*`). The integration tree is a
  fresh `git worktree add`; gitignored fixtures (e.g. `app/src-tauri/models/
  ggml-tiny.en.bin`) are absent → the app crate fails to *build* → false REJECT.
- **Confirm the Bridge preview** shows a `verify` pane (no "commit" in its task) +
  the dispatch toast says "M verify held" (M ≥ 1) — the orchestrator (haiku) is
  unreliable at honoring a verify focus, and **0 verify panes = no auto-fold**.
- **`\n` doesn't submit codex/cursor TUIs** — after dispatch, click each non-claude
  pane and press Enter, or it sits unsent.

## Front (in the app)

1. **(optional) delegate the audit.** Delegate button → goal: *"Audit <area> for
   <X>, report gaps. Change no code."* → advisory `final.md` (report-only).
2. **Bridge the fix.** Open Bridge (Cmd+Shift+O). Goal references the report's
   ABSOLUTE path and frames it as multi-writer code so two-wave engages, e.g.:
   > *"Fix the gaps in `<abs>/final.md`. Each code agent owns ONE file, fixes it,
   > commits. Keep changes inside a model-free crate (core/*)."*
   Set ONE pane's **focus** to *"ONLY verify the merged tree — do not edit any
   file."* **Check ☑ Auto-synthesize.**
3. **Synthesize → confirm the preview** (one verify pane, rest committers) → **Dispatch**.
   Submit any codex/cursor panes manually (Enter). Leave the verify pane alone.
4. The system auto-folds the committed pane branches into one integration
   worktree, runs the authoritative test, and writes `bridge/<run>/final.md`
   (PASS) or `final.HELD.md` (fail). The pane branches `agent-teams/<pane>` persist.

## Tail (the script — this is the reusable automation)

Ship the **current feature branch** (single-author case):
```bash
scripts/auto-ship.sh \
  --manifests core/ringbuf/Cargo.toml \
  --base main \
  --title "feat(ringbuf): ByteRing::evicted()"
```

Ship a **Bridge two-wave run** (fold the pane branches, then gate + PR), using
the synthesized `final.md` as the PR body:
```bash
scripts/auto-ship.sh \
  --branches agent-teams/ws4772x0-p0,agent-teams/ws4772x0-p1,agent-teams/ws4772x0-p2 \
  --manifests core/ringbuf/Cargo.toml \
  --base main \
  --title "Error-handling fixes (Bridge two-wave)" \
  --body-file bridge/<run>/final.md
```

`--dry-run` runs the gate only (no push/PR). On a RED gate the script aborts —
no branch is pushed, no PR opened. On GREEN it pushes and opens the PR, then
stops: **the merge is yours.**

### The one fragile link
The unattended `gh pr create` uses `env -u GITHUB_TOKEN` (the env token is bad;
the keyring works) and pushes via `origin`. If it still trips on auth, the gate
already passed — just run the printed `git push` + `gh pr create` yourself.
