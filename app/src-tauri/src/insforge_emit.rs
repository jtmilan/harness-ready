//! P10.1 / P10.3 — InsForge dashboard emitter (the ONLY new surface for the ACP
//! observability/persistence layer; see `.paul/analysis/control-plane-integration/02-integration.md`).
//!
//! ## Contract — mirrors `emit_consolidated_artifact`'s best-effort discipline exactly
//!
//! This module writes loop/run/event rows to InsForge (a Postgres BaaS) at existing
//! flywheel lifecycle points so the operator can track loops *after they submit them*.
//! It is built to the same four invariants the artifact emitter documents (`lib.rs`
//! `emit_consolidated_artifact`):
//!
//! 1. **Default-OFF flag.** Every entry point first checks the `insforge_dashboard`
//!    flag in `mcp-config.json` (read with the SAME path policy as `read_mcp_config`).
//!    Off → the function returns immediately. No thread is spawned, no network call is
//!    made. The flag is file-only / operator-edited, never LLM-settable, and absent ⇒
//!    OFF (fail safe) — identical posture to `flywheel_apply` / `flywheel_ship`.
//! 2. **Env-gated.** The InsForge base URL + the server-write key come from env
//!    (`INSFORGE_URL` + `INSFORGE_ADMIN_TOKEN`). If either is unset/empty → silent
//!    no-op (no thread, no call). The key is NEVER logged and NEVER hardcoded.
//! 3. **Fire-and-forget on a background thread.** Each emit spawns one `std::thread`
//!    that shells out to `curl` (the app already shells out to `git`/`gh`; this adds
//!    NO new crate — `Cargo.toml` is untouched). A slow/unreachable InsForge can never
//!    block the controller's hot path.
//! 4. **Failure-swallowed + write-only w.r.t. the engine.** `let _ = curl(...)`; any
//!    IO/HTTP failure is logged (without the key) and skipped, never `?`-propagated.
//!    The emitter NEVER reads InsForge back to influence a decision — the verdict,
//!    gates, ARM check, credential-strip, and never-merge-main path are untouched.
//!    InsForge holds a *copy* of the verdict word; it can never *set* one.
//!
//! The whole module is `#[cfg(feature = "delegate-live")]` (gated at the `mod`
//! declaration in `lib.rs`) so a non-delegate build never references it.
//!
//! ## Transport
//!
//! PostgREST-over-Postgres via the InsForge REST mount (`/api/database/records/{table}`):
//! - INSERT / UPSERT → `POST` with a JSON **array** body. Upsert uses
//!   `Prefer: resolution=merge-duplicates` (conflict on the table's PK).
//! - UPDATE → `PATCH …?{pk}=eq.{id}` with a JSON object body.
//! Auth is `Authorization: Bearer {INSFORGE_ADMIN_TOKEN}`.
//!
//! ## Column contract
//!
//! Column names match the live InsForge `acp_loops` / `acp_runs` / `acp_run_events`
//! tables verbatim (verified via the InsForge MCP `get-table-schema`, which matches §4
//! of the design doc). The verdict word is mapped host→schema at the call site
//! (`pass`→`PASS`, `hold`→`HELD`, `reject`→`FAIL`, `advisory`→`PASS`).

use serde_json::{json, Value};
use std::path::Path;
use std::process::{Command, Stdio};

use agent_teams_core::mcp_config_path;

/// Env var holding the InsForge base URL, e.g. `https://insforge-sandbox.up.railway.app`.
/// Unset/empty ⇒ the emitter is a silent no-op.
const ENV_URL: &str = "INSFORGE_URL";
/// Env var holding the server-write key (InsForge admin token / Bearer). NEVER
/// logged, NEVER hardcoded. Unset/empty ⇒ the emitter is a silent no-op.
const ENV_KEY: &str = "INSFORGE_ADMIN_TOKEN";

/// The default-OFF gate key inside `mcp-config.json`. Mirrors the `flywheel_*` flags:
/// file-only, operator-edited, absent ⇒ OFF.
const FLAG: &str = "insforge_dashboard";

/// Resolved, validated emit configuration. `Some` ONLY when the flag is ON *and* both
/// env vars are present + non-empty — otherwise every public fn short-circuits to a
/// no-op before spawning anything.
struct EmitCfg {
    base: String,
    key: String,
}

/// Is the `insforge_dashboard` flag ON in `mcp-config.json`?
///
/// Read DIRECTLY from the same file `read_mcp_config` reads (`<state_root>/../mcp-config.json`,
/// via the shared `mcp_config_path` path policy) and parse JUST the one bool key. We do NOT
/// route through the typed `McpConfig` struct: that would require adding a field to the core
/// crate (and recompiling the dep tree under the running app); reading the raw JSON keeps the
/// change confined to this module + `lib.rs` and `Cargo.toml` untouched. Fail-SAFE exactly like
/// `read_mcp_config`: an absent / unreadable / malformed file ⇒ flag OFF (never fail open).
fn flag_on(state_root: &Path) -> bool {
    let Some(path) = mcp_config_path(state_root) else {
        return false;
    };
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|body| serde_json::from_str::<Value>(&body).ok())
        .and_then(|v| v.get(FLAG).and_then(|b| b.as_bool()))
        .unwrap_or(false)
}

/// Resolve the active config, or `None` (⇒ no-op) if the flag is off or either env var
/// is unset/empty. This is the single chokepoint every emit fn passes through BEFORE it
/// spawns a thread — so flag-OFF / env-unset is byte-for-byte unchanged behavior (no
/// thread, no network).
fn resolve(state_root: &Path) -> Option<EmitCfg> {
    if !flag_on(state_root) {
        return None;
    }
    let base = std::env::var(ENV_URL).ok().filter(|s| !s.trim().is_empty())?;
    let key = std::env::var(ENV_KEY).ok().filter(|s| !s.trim().is_empty())?;
    Some(EmitCfg {
        base: base.trim_end_matches('/').to_string(),
        key,
    })
}

/// Fire-and-forget one curl POST/PATCH on a background thread. Returns immediately; the
/// thread swallows + logs (without the key) any failure. NEVER blocks the caller, NEVER
/// returns an error to the caller.
fn fire(cfg: EmitCfg, method: &'static str, path: String, body: Value) {
    let url = format!("{}{}", cfg.base, path);
    let key = cfg.key;
    let payload = body.to_string();
    // One detached worker per emit. The emit cadence is low (a handful per run), so a
    // thread-per-call is simpler than a channel+drainer and just as fail-soft. If thread
    // spawning itself fails, that error is swallowed too — the run is never affected.
    let _ = std::thread::Builder::new()
        .name("insforge-emit".into())
        .spawn(move || {
            let out = Command::new("curl")
                .arg("-sS")
                .args(["--max-time", "8"]) // never hang a worker thread on a dead endpoint
                .args(["-X", method])
                .arg("-H")
                // The key is passed as a curl header arg (process argv), NOT logged. We
                // never print `key` anywhere in this module.
                .arg(format!("Authorization: Bearer {key}"))
                .args(["-H", "Content-Type: application/json"])
                // resolution=merge-duplicates ⇒ POST acts as UPSERT on the PK; harmless on
                // a pure INSERT path (no conflict ⇒ plain insert). return=minimal keeps the
                // response small (we never read the body to decide anything).
                .args(["-H", "Prefer: resolution=merge-duplicates,return=minimal"])
                .args(["--data-binary", &payload])
                // Append the HTTP status as the final stdout line. curl WITHOUT `-f` exits 0
                // even on a 4xx/5xx (e.g. the `permission denied for sequence` 403 that
                // silently dropped every event before this guard), so the process exit code
                // alone can't tell success from an HTTP error — we must inspect the code.
                .args(["-w", "\n%{http_code}"])
                .arg(&url)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output();
            match out {
                Ok(o) if o.status.success() => {
                    // curl ran. Split the trailing status line off stdout and check it's 2xx.
                    let stdout = String::from_utf8_lossy(&o.stdout);
                    let (body, code) = stdout.rsplit_once('\n').unwrap_or(("", stdout.trim()));
                    let code = code.trim();
                    if !code.starts_with('2') {
                        // HTTP error that curl masked. The response body is InsForge's error
                        // JSON — it carries NO secret (the bearer key lives only in the
                        // request header, never echoed to stdout). Cap the snippet so a
                        // verbose error can't flood the log.
                        let snippet: String = body.trim().chars().take(300).collect();
                        eprintln!("[insforge_emit] {method} {url} → HTTP {code}: {snippet}");
                    }
                }
                Ok(o) => {
                    // Log the failure WITHOUT the key or the payload (payload may carry the
                    // goal text; the URL carries no secret). Best-effort observability only.
                    let err = String::from_utf8_lossy(&o.stderr);
                    eprintln!(
                        "[insforge_emit] {method} {url} → curl exit {:?}: {}",
                        o.status.code(),
                        err.trim()
                    );
                }
                Err(e) => eprintln!("[insforge_emit] {method} {url} → curl spawn failed: {e}"),
            }
        });
}

/// REST path for a table resource on the InsForge PostgREST mount.
fn table_path(table: &str) -> String {
    format!("/api/database/records/{table}")
}

// ─────────────────────────── emit points (§5) ───────────────────────────
//
// Every fn: take `state_root` (to read the flag) → `resolve` (flag+env) → if `None`,
// return (no-op) → else build the row and `fire` a detached curl. No `?`, no unwrap on
// the network path, never affects control flow.

/// UPSERT one `acp_loops` row (loop saved / updated). PK = the stable `loop_id`, so a
/// re-save merges in place — matching the host's one-loop-one-identity invariant.
#[allow(clippy::too_many_arguments)]
pub fn upsert_loop(
    state_root: &Path,
    loop_id: &str,
    name: &str,
    goal: &str,
    repo: &str,
    workspace_id: &str,
    harness: &str,
    model: Option<&str>,
    workers: u32,
    merge_target: &str,
    ship_request: bool,
    schedule: &Value,
    enabled: bool,
) {
    let Some(cfg) = resolve(state_root) else { return };
    let row = json!({
        "loop_id": loop_id,
        "name": name,
        "goal": goal,
        "repo": repo,
        "workspace_id": workspace_id,
        "harness": harness,
        "model": model,
        "workers": workers,
        "merge_target": merge_target,
        "ship_request": ship_request,
        "schedule": schedule,
        "enabled": enabled,
    });
    // POST a single-element array (InsForge requires array bodies for create/upsert).
    fire(cfg, "POST", table_path("acp_loops"), json!([row]));
}

/// INSERT one `acp_runs` row at loop-fire start (controller, after run_id minted + repo
/// validated). phase = 'validating'. `loop_id` empty ⇒ a one-shot manual run (no loop scope).
pub fn insert_run(state_root: &Path, run_id: &str, loop_id: &str, repo: &str, goal: &str) {
    let Some(cfg) = resolve(state_root) else { return };
    let row = json!({
        "run_id": run_id,
        // NULL (not "") when there is no loop, so the acp_loops FK is satisfied (a manual
        // run has no acp_loops row to reference).
        "loop_id": if loop_id.trim().is_empty() { Value::Null } else { json!(loop_id) },
        "repo": repo,
        "goal": goal,
        "phase": "validating",
    });
    fire(cfg, "POST", table_path("acp_runs"), json!([row]));
    // Open the timeline with the first phase event.
    emit_phase_event(state_root, run_id, loop_id, "validating");
}

/// UPDATE `acp_runs` after orchestration returns: record the decomposed subtasks, the
/// dispatched worker pane ids, the orchestrator token spend, and advance phase →
/// 'dispatched'.
pub fn update_run_dispatch(
    state_root: &Path,
    run_id: &str,
    decompose_tasks: &Value,
    worker_panes: &[String],
    input_tokens: u64,
    output_tokens: u64,
) {
    let Some(cfg) = resolve(state_root) else { return };
    let patch = json!({
        "decompose_tasks": decompose_tasks,
        "worker_panes": worker_panes,
        "phase": "dispatched",
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
    });
    fire(
        cfg,
        "PATCH",
        format!("{}?run_id=eq.{}", table_path("acp_runs"), run_id),
        patch,
    );
}

/// Phase change (the live feed). UPDATE `acp_runs.phase` AND append one `acp_run_events`
/// row of type 'phase'. THROTTLED by the caller — call ONLY when the phase string actually
/// changes (the `beat_in_flight` wiring compares against the prior in-flight phase), so a
/// 1.2s heartbeat tick does not spam events.
pub fn emit_phase(state_root: &Path, run_id: &str, loop_id: &str, phase: &str) {
    let Some(cfg) = resolve(state_root) else { return };
    fire(
        cfg,
        "PATCH",
        format!("{}?run_id=eq.{}", table_path("acp_runs"), run_id),
        json!({ "phase": phase }),
    );
    emit_phase_event(state_root, run_id, loop_id, phase);
}

/// Append one `acp_run_events(event_type='phase')` row. Internal helper shared by
/// `insert_run` (opening event) + `emit_phase`.
fn emit_phase_event(state_root: &Path, run_id: &str, loop_id: &str, phase: &str) {
    let Some(cfg) = resolve(state_root) else { return };
    let row = event_row(run_id, loop_id, "phase", json!({ "phase": phase }));
    fire(cfg, "POST", table_path("acp_run_events"), json!([row]));
}

/// Append one worker lifecycle event ('worker_start' | 'worker_end'). `kind` is the
/// event_type; `wid` is the worker pane id.
pub fn emit_worker(state_root: &Path, run_id: &str, loop_id: &str, wid: &str, kind: &str) {
    let Some(cfg) = resolve(state_root) else { return };
    let row = event_row(run_id, loop_id, kind, json!({ "wid": wid }));
    fire(cfg, "POST", table_path("acp_run_events"), json!([row]));
}

/// Finish a run: UPDATE `acp_runs` with the decided verdict + provenance (read straight
/// off the SAME values that feed `emit_consolidated_artifact`) and append a 'verdict'
/// event. `verdict` is the SCHEMA word (PASS|HELD|FAIL) — map at the call site.
#[allow(clippy::too_many_arguments)]
pub fn finish_run(
    state_root: &Path,
    run_id: &str,
    loop_id: &str,
    verdict: &str,
    held_kind: Option<&str>,
    calibrated: Option<bool>,
    review_verdict: Option<&str>,
    base_sha: Option<&str>,
    head_sha: Option<&str>,
    diff_bytes: Option<u64>,
    input_tokens: u64,
    output_tokens: u64,
    cost_usd: f64,
    report_name: &str,
) {
    let Some(cfg) = resolve(state_root) else { return };
    let patch = json!({
        "verdict": verdict,
        "held_kind": held_kind,
        "calibrated": calibrated,
        "review_verdict": review_verdict,
        "base_sha": base_sha,
        "head_sha": head_sha,
        "diff_bytes": diff_bytes,
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "cost_usd": cost_usd,
        "report_name": report_name,
        "phase": "done",
        // PostgREST coerces this ISO-8601 string to timestamptz. Recording the close time
        // is purely observational; a parse failure on the DB side just leaves it null.
        "ended_at": iso_now(),
    });
    fire(
        cfg,
        "PATCH",
        format!("{}?run_id=eq.{}", table_path("acp_runs"), run_id),
        patch,
    );
    // The verdict event carries the same words for the append-only timeline. `cfg` was
    // moved into the PATCH `fire` above; re-`resolve()` for the second fire (cheap; keeps
    // the key from lingering in a Clone).
    let Some(cfg2) = resolve(state_root) else { return };
    let row = event_row(
        run_id,
        loop_id,
        "verdict",
        json!({ "verdict": verdict, "held_kind": held_kind, "report_name": report_name }),
    );
    fire(cfg2, "POST", table_path("acp_run_events"), json!([row]));
}

/// Set the PR url after `flywheel_push_and_pr` returns one: UPDATE `acp_runs.pr_url` +
/// append a 'pr_open' event.
pub fn set_pr_url(state_root: &Path, run_id: &str, loop_id: &str, pr_url: &str) {
    let Some(cfg) = resolve(state_root) else { return };
    fire(
        cfg,
        "PATCH",
        format!("{}?run_id=eq.{}", table_path("acp_runs"), run_id),
        json!({ "pr_url": pr_url }),
    );
    let Some(cfg2) = resolve(state_root) else { return };
    let row = event_row(run_id, loop_id, "pr_open", json!({ "pr_url": pr_url }));
    fire(cfg2, "POST", table_path("acp_run_events"), json!([row]));
}

/// Build one `acp_run_events` row. `loop_id` empty ⇒ NULL (the events table's loop_id is
/// a plain column, but keeping it NULL on a manual run is cleaner than "").
fn event_row(run_id: &str, loop_id: &str, event_type: &str, detail: Value) -> Value {
    json!({
        "run_id": run_id,
        "loop_id": if loop_id.trim().is_empty() { Value::Null } else { json!(loop_id) },
        "event_type": event_type,
        "detail": detail,
    })
}

/// Best-effort ISO-8601 UTC timestamp for `ended_at`, with no chrono dependency (Cargo.toml
/// stays untouched). Computed from the Unix epoch via a small civil-date conversion. Never
/// panics; on a clock error returns the epoch. Observational only.
fn iso_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// Howard Hinnant's `civil_from_days` algorithm (public domain): days-since-Unix-epoch →
/// (year, month, day). Pure + total.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod smoke {
    use super::*;

    /// LIVE write-path smoke. `#[ignore]` so it never runs in CI / normal `cargo test` —
    /// it makes real HTTP writes to the configured InsForge instance. Run explicitly with
    /// the flag's two env vars set:
    ///
    /// ```text
    /// INSFORGE_URL=... INSFORGE_ADMIN_TOKEN=... \
    ///   cargo test --no-default-features --features delegate-live \
    ///   --manifest-path app/src-tauri/Cargo.toml live_write_path_smoke \
    ///   -- --ignored --nocapture
    /// ```
    ///
    /// It writes a `p10-smoke-<pid>` loop + run via the REAL emitter fns (same curl the
    /// running binary fires), then sleeps for the detached threads to complete. The caller
    /// verifies the rows landed (and cleans them up) out-of-band via the InsForge admin tool.
    #[test]
    #[ignore]
    fn live_write_path_smoke() {
        // Fail loud if the env isn't wired — otherwise `resolve()` would silently no-op and
        // the test would "pass" having written nothing.
        let url = std::env::var(ENV_URL).unwrap_or_default();
        let key = std::env::var(ENV_KEY).unwrap_or_default();
        assert!(
            !url.trim().is_empty() && !key.trim().is_empty(),
            "set {ENV_URL} + {ENV_KEY} in the env before running this smoke"
        );

        // Isolated state_root with the flag ON in its sibling mcp-config.json — never the
        // live config. `<tmp>/state` ⇒ flag read from `<tmp>/mcp-config.json`.
        let tmp = std::env::temp_dir().join(format!("p10-smoke-{}", std::process::id()));
        let state_root = tmp.join("state");
        std::fs::create_dir_all(&state_root).expect("mk state_root");
        std::fs::write(
            tmp.join("mcp-config.json"),
            r#"{"insforge_dashboard": true}"#,
        )
        .expect("write flag config");
        // Sanity: the flag must resolve ON from this isolated root.
        assert!(flag_on(&state_root), "flag should be ON in the temp config");

        let run_id = format!("p10-smoke-{}", std::process::id());
        let loop_id = format!("loop-p10smoke-{}", std::process::id());
        println!("[smoke] run_id={run_id} loop_id={loop_id}");

        // Space the emits ~900ms apart so each detached curl commits before the next fires —
        // mimics the real flywheel's seconds-to-minutes spacing between lifecycle points and
        // isolates ordering from the emitter's correctness.
        let gap = || std::thread::sleep(std::time::Duration::from_millis(900));

        // Drive the real emit points in lifecycle order.
        upsert_loop(
            &state_root,
            &loop_id,
            "p10 smoke loop",
            "smoke: prove the binary POSTs",
            "/Users/jeffrymilan/flywheel-verify",
            "ws-smoke",
            "claude",
            Some("claude"),
            1,
            "loop-integration",
            false,
            &json!({}),
            false,
        );
        gap();
        insert_run(
            &state_root,
            &run_id,
            &loop_id,
            "/Users/jeffrymilan/flywheel-verify",
            "smoke: prove the binary POSTs",
        );
        gap();
        update_run_dispatch(
            &state_root,
            &run_id,
            &json!([{ "task": "smoke subtask" }]),
            &["run-w0".to_string()],
            1234,
            567,
        );
        gap();
        emit_worker(&state_root, &run_id, &loop_id, "run-w0", "worker_start");
        gap();
        emit_worker(&state_root, &run_id, &loop_id, "run-w0", "worker_end");
        gap();
        finish_run(
            &state_root,
            &run_id,
            &loop_id,
            "PASS",
            None,
            Some(true),
            Some("approve"),
            Some("abc1234"),
            Some("def5678"),
            Some(42),
            1234,
            567,
            0.0,
            "final.md",
        );
        gap();
        set_pr_url(
            &state_root,
            &run_id,
            &loop_id,
            "https://github.com/jtmilan/flywheel-verify/pull/999",
        );

        // The emits are fire-and-forget on detached threads; curl has --max-time 8. Hold the
        // process open so they complete before the test runner exits.
        std::thread::sleep(std::time::Duration::from_secs(9));
        println!("[smoke] fired all emits for run_id={run_id} — verify rows out-of-band");
    }
}
