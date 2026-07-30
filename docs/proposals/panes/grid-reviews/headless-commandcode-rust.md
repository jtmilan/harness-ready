I've read all four files. Here's my review:

**core/mcp/src/lib.rs**: No findings.

**agent-teams-mcp/src/read_output.rs**: No findings.

**agent-teams-mcp/src/main.rs**: No findings.

**core/daemon/src/spawn.rs**: 
- `core/daemon/src/spawn.rs:705,742,743`: NIT: `unsafe { libc::kill(…) }` — the only `unsafe` blocks across all four files. They're documented, necessary for process signaling (no safe Rust equivalent for kill(2)), and guarded by the `daemon-spawn` feature. Acceptable for systems-level daemon code, but flagged since the review criterion is "no unsafe."

VERDICT: b/s/n

## BOUNDARIES
- `reconcile_liveness: false` default verified in `McpConfig::default()` (core/mcp/src/lib.rs:2047). Flag-off path in `agent-teams-mcp/src/main.rs:1358-1362` calls `compute_queue_identified(state_dir, registry.as_ref())` — byte-for-byte the prior logic. No discrepancy.
- `daemon_spawn_enabled: false` default verified (core/mcp/src/lib.rs:2040). The entire `core/daemon/src/spawn.rs` is `#[cfg(any(test, feature = "daemon-spawn"))]` — absent from default builds, `server::route_request` falls through to `SPAWN_UNAVAILABLE`.
- No `unwrap()` on IO operations across any of the four files. All fallible IO uses `.ok()?`, `if let Ok(…)`, `continue`/`None`, or `unwrap_or_else(|_| fallback)`. Test code uses `.unwrap()` on test assertions only — standard and acceptable.
- `recent_event_ids` (main.rs:1429-1453) is confined to `state_dir/*/events.jsonl` — paths come from `read_dir` on the caller-provided `state_dir`, never from caller-supplied strings. `newest_report_md` and transcript locators build paths from registry-sourced/controlled roots only. All id-to-path steps are `validate_spawn_id`-gated.
- `unsafe` is confined to `libc::kill(…)` in two locations in `spawn.rs`, both necessary for process management and documented.
- No new external crate deps introduced. All imports are from `std`, `serde`, `rmcp`, `schemars`, `tokio`, `hmac`/`sha2`/`hex`, `anyhow`, or sibling crates (`agent_teams_core`, `supervisor`, `roles`, `state_adapter`).
- `core/mcp/src/lib.rs` pure decision helpers are unit-tested in the `#[cfg(test)] mod` — `normalize_input`, `validate_spawn_id`, `validate_session_id`, `validate_model`, `extra_dir_in_repo_scope`, `ws_of_pane`, `sharing_enabled`, `authorize_cross`, `read_output_cap`, `strip_ansi_codes`, `external_spawn_*` allowlists, `op_*` classifiers, `liveness_source`, `observed_live_ids`, `disk_recent`, constant-time equals, HMAC primitives, etc.
