# serena per-worktree LSP-MCP (DESIGN §3.9-B)

`serena-worktree.sh` starts **one** [serena](https://github.com/oraios/serena)
Language-Server-Protocol MCP server, **project-pinned to a single worktree**.

serena exposes a real Language Server as MCP tools:

- **Navigation (token-cheap):** `get_symbols_overview`, `find_symbol`,
  `find_referencing_symbols` — pull ONE symbol body, not a whole file.
- **Symbol-accurate edits (no line-offset drift):** `replace_symbol_body`,
  `insert_after_symbol`.

It is the on-demand *semantic drill-down* half of pre-flight. The cheap
deterministic *orientation* half is the aider-style repo-map (§3.9-A); map
points, serena drills.

## The one-server-per-worktree rule (non-negotiable)

> **serena is ONE active project per server process.**

A single global server does **NOT** fan out across worktrees. Concurrent
worktrees pointed at one shared server **thrash and contaminate** each other's
active project + LSP state.

```
RULE: one serena server PER worktree, project-pinned.
      NEVER a shared server multiplexing projects.
```

The *skill* ("how to wire serena") is global and lives in `~/.claude/skills`
(linked into each worktree by `../skills/link-skills.sh`, §3.8). The *instance*
is per-worktree: a stdio subprocess spawned by the MCP client that **dies with
the session**.

Because rust-analyzer is RAM-hungry, **cap concurrency** — tie the number of
live serena servers to the §3.1 `concurrency` selector.

## Backends

serena selects the language server per project language:

| Language | Backend                      | Notes                                  |
|----------|------------------------------|----------------------------------------|
| Rust     | `rust-analyzer`              | RAM-hungry; slow cold-start; cap fan-out|
| JS / TS  | `typescript-language-server` | lighter than rust-analyzer             |

serena manages backend acquisition; you generally do not install them by hand,
but a system `rust-analyzer` / `typescript-language-server` on `PATH` is honored.

## Warm cold-start with `serena project index`

rust-analyzer's first activation can take tens of seconds to minutes. Warm the
index once per worktree so the first real query is fast:

```sh
./serena-worktree.sh --index <worktree-path>
# == uvx --from git+https://github.com/oraios/serena serena project index <path>
```

## Usage

```sh
# Exec the stdio MCP server, pinned to one worktree (client owns lifecycle):
./serena-worktree.sh <worktree-path>

# Warm the symbol index (hide rust-analyzer cold-start), then exit:
./serena-worktree.sh --index <worktree-path>

# Print the exact command without exec (for wiring / debugging):
./serena-worktree.sh --print-cmd <worktree-path>

# Prove uvx can fetch serena, then exit (no server left running):
./serena-worktree.sh --check
```

### Wiring into an MCP client (per worktree)

```sh
# From inside the worktree, register a serena server scoped to THIS project:
claude mcp add serena -- \
  uvx --from git+https://github.com/oraios/serena \
  serena start-mcp-server --context ide-assistant --project "$(pwd)"
```

Or point the client at this wrapper so the path resolution / one-per-worktree
discipline lives in one place:

```sh
claude mcp add serena -- /abs/path/to/serena-worktree.sh "$(pwd)"
```

The server speaks MCP over **stdio** — it is meant to be spawned *by* the
client, not launched standalone. The wrapper `exec`s serena so the client owns
the process lifecycle and the server dies with the session.

## Requirements

- `uvx` (from [uv](https://docs.astral.sh/uv/)) on `PATH`. The wrapper fetches
  serena straight from git via `uvx --from git+https://github.com/oraios/serena`
  — no separate `pip install` step. First fetch downloads + caches; subsequent
  runs are fast.

## Related: ponytail skill install (§3.8) — needs the USER

Fix-workers reference the **ponytail** YAGNI minimal-diff skill (installed once
into the global `~/.claude/skills`, then reached via the §3.8 worktree symlink).
ponytail is distributed as a Claude Code **plugin**, and its install is
**interactive** (a plugin marketplace prompt), so it cannot be scripted here —
it must be run by a human in an interactive Claude Code session:

```
/plugin marketplace add DietrichGebert/ponytail
/plugin install ponytail@ponytail
```

After install, ponytail is available globally and is linked into each worktree
by `../skills/link-skills.sh` like every other skill (one source of truth).

> This step is left to the user — see `needs_from_user` in the build report.
