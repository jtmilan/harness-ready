# Worktree global-skills wiring (DESIGN §3.8)

`link-skills.sh` symlinks the global Claude Code skills dir into a worktree so a
headless `claude -p` loop worker can **reach for real skills** instead of
reinventing them:

```
<worktree>/.claude/skills  ->  ~/.claude/skills
```

This is the **Skill Symlink Convention**: one source of truth, *referenced not
copied*. Skills live once in `~/.claude/skills`; every worktree links to them.
Updating a skill globally instantly updates it for all worktrees.

Workers reach for, e.g., `cavecrew-builder` / `ponytail` (surgical, minimal
diffs → cheaper review), `cavecrew-reviewer` / `code-review` (gate-2 verdict),
and `verify` (runtime observation).

## Usage

```sh
./link-skills.sh <worktree-path>              # link (idempotent)
./link-skills.sh <worktree-path> --quiet      # no chatter (for the loop)
./link-skills.sh <worktree-path> --force      # replace a real dir occupying the slot
./link-skills.sh <worktree-path> --skills-dir /custom/skills
```

Override the source via `CLAUDE_SKILLS_DIR` env var (default `~/.claude/skills`).

## Behavior (idempotent + safe)

| Slot state                              | Action                                  |
|-----------------------------------------|-----------------------------------------|
| missing                                 | create symlink                          |
| symlink already -> global skills        | no-op (exit 0)                          |
| stale/wrong symlink                     | repair (relink)                         |
| real file/dir present, no `--force`     | **refuse** (exit 2), never clobber      |
| real file/dir present, `--force`        | remove + relink                         |

Exit codes: `0` ok · `1` usage/arg error · `2` refused non-symlink · `3` link failed.
