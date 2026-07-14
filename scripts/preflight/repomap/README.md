# Pre-flight repo-map generator (DESIGN §3.9-A)

Standalone, **LLM-free**, deterministic ranked symbol map for prepending to
worker prompts. This is the "map before agents look" orientation layer: agents
navigate instead of grep-thrashing.

Vendors a compact RepoMapper-style pipeline (no aider runtime):

```
git-tracked Rust + JS files
  -> tree-sitter symbol extraction (definitions + references)
  -> file/symbol reference graph
  -> PageRank ranking (pure-python, no scipy/numpy)
  -> binary-search token budget (default 1500)
  -> ranked <repo-map> text to stdout
```

## Usage

```bash
# uv handles the (cached) dependency env via PEP 723 inline metadata.
uv run --no-project --script repomap.py <repo_path> [--max-tokens N] [flags]

# or, since the shebang is `#!/usr/bin/env -S uv run --no-project --script`:
./repomap.py <repo_path> --max-tokens 1500
```

Flags:

| Flag | Default | Meaning |
|---|---|---|
| `--max-tokens N` | `1500` | Explicit token budget. **No aider no-files auto-inflation** — the budget is fixed regardless of chat-files. |
| `--lang rust,js` | both | Restrict to languages (`rust`/`rs`, `js`/`javascript`). |
| `--no-cache` | off | Skip the SHA-keyed cache (always recompute). |
| `--cache-dir DIR` | `<repo>/.git/repomap-cache` | Where the diskcache lives. Default keeps it inside `.git/`, untracked. |
| `--stats` | off | Print scan stats + cache state to **stderr** (map still goes to stdout). |

## Caching

Keyed by `git rev-parse HEAD` + a porcelain-dirty hash (`git status --porcelain`)
+ the token budget + the language set. Warm re-runs are a cache hit (~0.2s) and
re-emit the byte-identical stored map. Any commit, any uncommitted edit, or a
budget/lang change invalidates the entry and recomputes. The stable block pairs
well with Anthropic prompt caching on the worker side.

## Output shape

```
<repo-map repo="agent-teams" head="d8f490538a7e" max-tokens="1500" langs="javascript,rust">
core/mcp/src/lib.rs:
│  QueueRow
│  compute_queue
│  get_workspace
...
app/src/main.js:
│  ...
</repo-map>
```

Files are ordered by PageRank importance; within a file, symbols are ordered by
source line. Vendored / minified / generated files (`/vendor/`, `*.min.js`,
`/node_modules/`, `/dist/`, `/target/`, or a long-average-line minification
heuristic) are excluded so the budget goes to real source.

## Integration (loop DEFINE enrichment)

Prepend the emitted block to a worker's investigate prompt. Because it's cached
by repo SHA, each loop iteration's pre-flight cost is ~0 once warm; regenerate
only on SHA change. A top-N CRAP hotspot list (§3.10) can be appended to the
same `<repo-map>` block by a sibling component.

## Dependencies (pinned, auto-installed by uv)

- `grep-ast>=0.5` — filename→lang detection
- `tree-sitter-language-pack==0.9.0` — the standard py-tree-sitter binding
  (pinned; 1.9.x ships an incompatible Rust-native binding with no `Query` API)
- `tree-sitter>=0.23,<0.25`
- `networkx>=3.0` — graph container (PageRank is pure-python in this script)
- `diskcache>=5.6` — SHA-keyed result cache

## Self-test (observed)

Run against `/Users/jeffrymilan/Personal/agent-teams` (real Rust + vanilla-JS):

- cold run ~1.3s: 59 files scanned, 2561 defs, 36089 refs, 1876 ranked symbols
- emitted ~1500 tokens at `--max-tokens 1500` (binary-search fit; budgets
  300/800/1500/3000 all respected)
- 29 Rust files + 15 JS files in the map; real symbols
  (`harness_path`, `Harness`, `compute_queue`, `LiveRegistry`, `get_workspace`)
- warm cache hit ~0.2s, byte-identical to cold; two `--no-cache` runs identical
- `--lang rust` emits 0 JS files; `--lang js` emits 0 Rust files
- non-git dir / missing arg exit 2 with a clean stderr message
