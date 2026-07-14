//! 16-01 (item 1) Task 5 — END-TO-END pane MCP probes (D56 / AC-4).
//!
//! An agent inside a real worktree, spawned through the SAME `inject_mcp_config`
//! path the supervisor uses, must list+call the read-only `team_get_queue` tool
//! and return the seeded workspace id. Two legs (DIFFERENT mechanisms):
//!
//! - **claude** — `--mcp-config <staged abs file>` (ADDITIVE, no `--strict`) +
//!   `--allowedTools mcp__agent-teams__team_get_queue`. The `-p` mode has TWO
//!   independent gates: (a) MCP-SERVER trust and (b) per-TOOL permission. The
//!   `--allowedTools` flag pre-allows the tool so the probe isolates the
//!   SERVER-TRUST gate — that is what D56's additive-vs-strict question measures.
//!   Without it the probe false-negatives and would wrongly push us to `--strict`.
//! - **cursor** — in-worktree `.cursor/mcp.json` (discovered by path) +
//!   `--approve-mcps` (server trust) + `--force` (tool permission), `-p`.
//!
//! BOTH legs are `#[ignore]` + env-gated (`AGENT_TEAMS_E2E_MCP=1`) so CI without
//! the CLIs / network stays green. The sidecar binary is the committed prebuilt
//! (or `AGENT_TEAMS_MCP_BIN`).
//!
//! ARBITER RESULT (run live in this environment, 2026-06-05): the ADDITIVE claude
//! leg returned the seeded row with NO blocking server-trust prompt — additive is
//! CONFIRMED, no `--strict-mcp-config` fallback needed. The cursor leg returned the
//! seeded row via `.cursor/mcp.json` + `--approve-mcps`. Captured in 16-01-SUMMARY.
//!
//! Manual run:
//!   AGENT_TEAMS_E2E_MCP=1 cargo test -p supervisor --test mcp_e2e -- --ignored --nocapture

use state_adapter::inject::{inject_mcp_config, InjectConfig, InjectHarness};
use std::path::PathBuf;
use std::process::Command;

fn sidecar_bin() -> PathBuf {
    std::env::var("AGENT_TEAMS_MCP_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../app/src-tauri/binaries/agent-teams-mcp-aarch64-apple-darwin")
        })
}

fn hooks_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../hooks")
}

/// A scratch (root, state_dir, worktree) seeded with one workspace whose latest
/// event makes a recognizable queue row. Returns (root, state, worktree).
fn scratch(tag: &str, ws_id: &str, event_line: &str) -> (PathBuf, PathBuf, PathBuf) {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let root = std::env::temp_dir().join(format!("at-e2e-mcp-{tag}-{nonce}"));
    let _ = std::fs::remove_dir_all(&root);
    let state = root.join("state");
    let wt = root.join("worktree");
    std::fs::create_dir_all(state.join(ws_id)).unwrap();
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(
        state.join(ws_id).join("events.jsonl"),
        format!("{event_line}\n"),
    )
    .unwrap();
    (root, state, wt)
}

fn staged_dir(root: &std::path::Path) -> PathBuf {
    let d = root.join("staged-hooks");
    std::fs::create_dir_all(&d).unwrap();
    // stage the real claude template so inject_mcp_config can render it
    std::fs::copy(
        hooks_dir().join("claude-mcp.tmpl.json"),
        d.join("claude-mcp.tmpl.json"),
    )
    .unwrap();
    std::fs::copy(
        hooks_dir().join("cursor-mcp.tmpl.json"),
        d.join("cursor-mcp.tmpl.json"),
    )
    .unwrap();
    d
}

// AC-4 (claude): additive --mcp-config, tool pre-allowed → the agent lists+calls
// team_get_queue and the seeded id appears in the output, no server-trust prompt.
#[test]
#[ignore = "needs claude CLI + network; run with AGENT_TEAMS_E2E_MCP=1 -- --ignored"]
fn claude_pane_lists_and_calls_team_get_queue() {
    if std::env::var("AGENT_TEAMS_E2E_MCP").is_err() {
        eprintln!("skipping: set AGENT_TEAMS_E2E_MCP=1 to run the live claude probe");
        return;
    }
    let (root, state, wt) = scratch(
        "claude",
        "e2e-ws",
        r#"{"harness":"claude","event":"PermissionRequest","ts":1000}"#,
    );
    let staged = staged_dir(&root);

    let cfg = InjectConfig {
        workspace_id: "e2e".into(),
        repo_dir: wt.clone(),
        writer_path: staged.join("state-writer.sh"),
        templates_dir: staged.clone(),
        state_root: state.clone(),
        pane_id: "e2e-p0".into(),
        repo_key: "e2e".into(),
        task_scope: "e2e-p0".into(),
    };
    let mcp_cfg = inject_mcp_config(&cfg, InjectHarness::Claude, &sidecar_bin())
        .expect("inject claude")
        .expect("claude returns a staged config path");

    let out = Command::new("claude")
        .current_dir(&wt)
        .args([
            "--mcp-config",
            mcp_cfg.to_str().unwrap(),
            "--allowedTools",
            "mcp__agent-teams__team_get_queue",
            "-p",
            "Call the team_get_queue tool and print its JSON result verbatim. No commentary.",
        ])
        .output()
        .expect("run claude");
    let stdout = String::from_utf8_lossy(&out.stdout);
    eprintln!("claude -p stdout:\n{stdout}");
    assert!(
        stdout.contains("e2e-ws"),
        "seeded workspace id must appear in the tool result (additive server-trust + pre-allowed tool); got: {stdout}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

// AC-4 (cursor): in-worktree .cursor/mcp.json + --approve-mcps → the agent
// lists+calls team_get_queue and the seeded id appears in the output.
#[test]
#[ignore = "needs cursor-agent CLI + network; run with AGENT_TEAMS_E2E_MCP=1 -- --ignored"]
fn cursor_pane_lists_and_calls_team_get_queue() {
    if std::env::var("AGENT_TEAMS_E2E_MCP").is_err() {
        eprintln!("skipping: set AGENT_TEAMS_E2E_MCP=1 to run the live cursor probe");
        return;
    }
    let (root, state, wt) = scratch(
        "cursor",
        "cur-ws",
        r#"{"harness":"cursor","event":"stop","ts":2000}"#,
    );
    let staged = staged_dir(&root);

    let cfg = InjectConfig {
        workspace_id: "e2e".into(),
        repo_dir: wt.clone(),
        writer_path: staged.join("state-writer.sh"),
        templates_dir: staged.clone(),
        state_root: state.clone(),
        pane_id: "e2e-p0".into(),
        repo_key: "e2e".into(),
        task_scope: "e2e-p0".into(),
    };
    // cursor injection writes <wt>/.cursor/mcp.json and returns None
    let none =
        inject_mcp_config(&cfg, InjectHarness::Cursor, &sidecar_bin()).expect("inject cursor");
    assert!(none.is_none(), "cursor config is discovered by path → None");
    assert!(
        wt.join(".cursor/mcp.json").exists(),
        "cursor config written in worktree"
    );

    let out = Command::new("cursor-agent")
        .current_dir(&wt)
        .args([
            "--approve-mcps",
            "--force",
            "-p",
            "--output-format",
            "text",
            "Call the team_get_queue MCP tool from the agent-teams server and print its JSON result verbatim. No commentary.",
        ])
        .output()
        .expect("run cursor-agent");
    let stdout = String::from_utf8_lossy(&out.stdout);
    eprintln!("cursor-agent -p stdout:\n{stdout}");
    assert!(
        stdout.contains("cur-ws"),
        "seeded workspace id must appear in the tool result; got: {stdout}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
