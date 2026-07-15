//! Diagnostic: spawn a real harness via the Supervisor and dump what the PTY
//! emits. `cargo run -p supervisor --example probe_claude -- [claude|cursor|bash]`

use std::path::PathBuf;
use std::{thread, time::Duration};
use supervisor::{Harness, Supervisor, WorkspaceSpec};

fn main() {
    let which = std::env::args().nth(1).unwrap_or_else(|| "claude".into());
    let harness = match which.as_str() {
        "cursor" => Harness::Cursor,
        "bash" => Harness::Bash,
        _ => Harness::Claude,
    };

    let dir = std::env::temp_dir().join("at-probe-wt");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let hooks = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../hooks");
    let state = std::env::temp_dir().join("at-probe-state");

    let spec = WorkspaceSpec {
        id: "probe".into(),
        harness,
        worktree: dir.clone(),
        session_id: None,
        resume: false,
        role: None,
        is_worker: false,
        extra_dirs: vec![],
        model: None,
    };
    // 16-01: point the MCP sidecar at the committed prebuilt (or AGENT_TEAMS_MCP_BIN
    // override) so a live claude/cursor probe injects the real read-only surface.
    let sidecar = std::env::var("AGENT_TEAMS_MCP_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../app/src-tauri/binaries/agent-teams-mcp-aarch64-apple-darwin")
        });
    let mut sup = Supervisor::spawn(&spec, &hooks, &state, &sidecar).expect("spawn");
    sup.resize(40, 120).expect("resize");
    if matches!(harness, Harness::Bash) {
        let _ = sup.write(b"echo PROBE_OK; ls\n");
    }
    thread::sleep(Duration::from_secs(5));

    let snap = sup.snapshot();
    let _ = std::fs::write("/tmp/at-claude-snap.txt", snap.as_bytes());
    eprintln!("=== wrote /tmp/at-claude-snap.txt ===");
    eprintln!(
        "=== harness={which} alive={} snapshot_len={} ===",
        sup.is_alive(),
        snap.len()
    );
    let head: String = snap.chars().take(1500).collect();
    eprintln!("=== first 1500 chars (escaped) ===\n{head:?}");
    sup.kill();
}
