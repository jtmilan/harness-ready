# Brief — `~/Personal/harness-ready`: standalone clean copy of the app (no agent-teams dependency)

## Topic/Intent
Create `~/Personal/harness-ready` as a SELF-CONTAINED copy of the Agent Teams app (Tauri Rust
backend + the new frontend with this session's xterm/adapter fixes). Zero filesystem dependency on
`~/Personal/agent-teams` — no absolute agent-teams paths, no `../../` escaping the harness-ready
root. Independent build (`cargo tauri dev/build`). Own identifier + window title. Parked: the
feature-parity plan (2026-07-13-feature-parity-plan.md) resumes after this.

## Ground truth (EXTRACTED — agent-teams layout)
- Cargo workspace root `Cargo.toml`: members = core/state-adapter, core/supervisor, core/harness,
  core/flywheel, core/mcp, agent-teams-mcp, core/daemon, core/task, core/ringbuf, core/memory,
  core/roles, core/agent; `exclude = ["app"]` (app is a standalone crate).
- `app/src-tauri/Cargo.toml` path deps: `../../core/{supervisor,state-adapter,flywheel,mcp,roles,
  task,memory,agent}`, `../../core/daemon`. Copying the tree at the SAME relative layout keeps
  these resolving INSIDE harness-ready (app at harness-ready/app, core at harness-ready/core).
- Bundle assets (all under `app/src-tauri/`, NOT under target/): `binaries/` (prebuilt
  agent-teams-mcp, -coordinator, -daemon, whisper-cli — aarch64-apple-darwin), `models/ggml-tiny.en.bin`
  (77MB), bundle `resources` point at `../../core/hooks/*`.
- `target/` is 27GB (app) + 7GB (root) — MUST be excluded from the copy (rebuild fresh).
- Current tauri.conf couplings to agent-teams: `beforeBuildCommand` chains
  `bash ../scripts/fetch-whisper-model.sh && npm --prefix /Users/jeffrymilan/Personal/nexus-agents run build`;
  `frontendDist` = absolute nexus-agents/dist. These are the agent-teams dependencies to sever.

## Target layout (INFERRED — standard Tauri + sibling frontend)
```
~/Personal/harness-ready/
  Cargo.toml            # workspace (same members, exclude app)
  core/  agent-teams-mcp/  scripts/
  app/src-tauri/        # Tauri crate; path deps ../../core/* resolve within root
    binaries/  models/  tauri.conf.json  tauri.dev.conf.json
  ui/                   # the frontend (copied from nexus-agents working tree)
```

## Pinned tauri.conf.json changes (Agent A owns the file; contract for both lanes)
- `identifier`: `com.jeffrymilan.harnessready` (+ tauri.dev.conf.json productName "Harness Ready Dev")
- `productName`: "Harness Ready"; `app.windows[0].title`: "Harness Ready"
- `build.frontendDist`: `../../ui/dist` (relative to app/src-tauri)
- `build.devUrl`: `http://localhost:5173`
- `build.beforeDevCommand`: `npm --prefix ../../ui run dev`
- `build.beforeBuildCommand`: `bash ../scripts/fetch-whisper-model.sh && npm --prefix ../../ui run build`
- Keep `bundle.externalBin` names + `bundle.resources` `../../core/hooks/*` (internal — copied).
  Binary names stay `agent-teams-mcp` etc. (internal identifiers, not a path dependency).

## Best Practices / Anti-Patterns
- EXTRACTED: preserve internal relative layout so path deps hold — a faithful `rsync -a --exclude`
  copy, not a hand-rebuilt tree. ANTI: rewriting each Cargo.toml path by hand (error-prone).
- INFERRED: exclude `target/`, `node_modules/`, `.git/`, `dist/` from the copy (rebuild clean).
- ANTI: absolute `~/Personal/agent-teams/...` paths anywhere in harness-ready (defeats the purpose).
- ANTI: renaming internal sidecar binaries (breaks `sidecar_bin` name resolution in Rust).
- State isolation: harness-ready should run with its OWN `AGENT_TEAMS_STATE_DIR` (dev script) so it
  never shares state/socket with agent-teams. INFERRED from the multi-instance collisions this
  session (memory: dev-isolated-env-leak-fleet-reap).

## Open Questions (AMBIGUOUS)
- Does core hardcode the state-dir name "agent-teams" (so two apps still collide on default)? Agent A
  greps `agent-teams-live.json`/`registry_path`; if hardcoded, note it — full state-name rebrand is a
  follow-up, mitigated by explicit AGENT_TEAMS_STATE_DIR at launch.
- Whisper `fetch-whisper-model.sh`: idempotent when model already present? If it re-downloads, keep;
  else the bundled model suffices. Agent A verifies.

## Sources
- agent-teams/Cargo.toml, app/src-tauri/{Cargo.toml,tauri.conf.json,tauri.dev.conf.json,binaries/,models/}, core/hooks/, scripts/
- nexus-agents working tree (uncommitted xterm/adapter fixes this session)
- memory: nexus-agents-ui-embed, dev-isolated-env-leak-fleet-reap

## Conformance
Faithful tree-copy preserving relative layout (deps stay internal) + a single pinned tauri.conf
rewrite to sever the two agent-teams couplings (frontend path, build command). Conforms to the
self-contained-app standard. AMBIGUOUS state-name coupling flagged for the backend lane.
