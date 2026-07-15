//! LIVE-VERIFY (ignored by default): spawn the real `opencode` and `cline` TUIs
//! through the exact `Supervisor::spawn` path the app uses (PATH inject, cline
//! `--config` per-pane home, opencode plugin ensure) and assert the pane PAINTS —
//! i.e. the PTY streams TUI bytes, the pane is not blank.
//!
//! Needs the real binaries + a logged-in `~/.cline` on the machine, so these are
//! `#[ignore]`; run explicitly with:
//!   cargo test -p supervisor --test live_tui -- --ignored --nocapture

use std::path::PathBuf;
use std::time::{Duration, Instant};
use supervisor::*;

fn spawn_and_snapshot(harness: Harness, id: &str, wait: Duration) -> String {
    let dir = std::env::temp_dir().join(format!("at-live-{}-{}", id, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let spec = WorkspaceSpec {
        id: id.into(),
        harness,
        worktree: dir.clone(),
        session_id: None,
        resume: false,
        role: None,
        is_worker: false,
        extra_dirs: vec![],
        model: None,
    };
    let hooks = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../hooks");
    let state = std::env::temp_dir().join(format!("at-live-state-{}", std::process::id()));
    std::fs::create_dir_all(&state).unwrap();
    let sidecar = PathBuf::from("/unused/agent-teams-mcp");

    let mut sup = Supervisor::spawn(&spec, &hooks, &state, &sidecar).unwrap();
    assert!(sup.is_alive(), "{id}: child alive right after spawn");

    // Wait for the TUI to paint: alt-screen entry (CSI ?1049h) or any cursor-addressed
    // SGR paint is proof the harness started rendering — a blank pane has neither.
    let start = Instant::now();
    let mut snap = String::new();
    while start.elapsed() < wait {
        snap = sup.snapshot();
        if snap.contains("\x1b[?1049h") || snap.contains("\x1b[38;2;") {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let alive = sup.is_alive();
    sup.kill();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        alive,
        "{id}: child must still be alive after {wait:?} (exited early — spawn args wrong?); output so far: {snap:?}"
    );
    snap
}

#[test]
#[ignore = "live: needs opencode binary on this machine"]
fn opencode_tui_paints_in_supervisor_pty() {
    let snap = spawn_and_snapshot(Harness::OpenCode, "wsliveoc", Duration::from_secs(20));
    assert!(
        snap.contains("\x1b[?1049h") || snap.contains("\x1b[38;2;"),
        "opencode pane must paint TUI bytes, got {} bytes: {snap:?}",
        snap.len()
    );
}

#[test]
#[ignore = "live: needs cline binary + logged-in ~/.cline on this machine"]
fn cline_tui_paints_in_supervisor_pty() {
    let snap = spawn_and_snapshot(Harness::Cline, "wslivecl", Duration::from_secs(20));
    assert!(
        snap.contains("\x1b[?1049h") || snap.contains("\x1b[38;2;"),
        "cline pane must paint TUI bytes, got {} bytes: {snap:?}",
        snap.len()
    );
}
