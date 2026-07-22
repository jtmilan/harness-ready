//! LIVE-VERIFY (ignored by default): spawn the real `opencode` and `pi` TUIs
//! through the exact `Supervisor::spawn` path the app uses (PATH inject, opencode
//! plugin ensure) and assert the pane PAINTS — i.e. the PTY streams TUI bytes,
//! the pane is not blank.
//!
//! Needs the real binaries on the machine, so these are `#[ignore]`; run
//! explicitly with:
//!   cargo test -p supervisor --test live_tui -- --ignored --nocapture

use std::path::PathBuf;
use std::time::{Duration, Instant};
use supervisor::*;

/// Wait until `pred(snapshot)` or `wait` elapses. Does NOT kill early on first probe
/// byte — OpenTUI emits alt-screen / capability queries first (~373B), then needs
/// host probe answers before truecolor SGR paint (~8KB @ ~3s).
fn spawn_and_wait(
    harness: Harness,
    id: &str,
    wait: Duration,
    pred: impl Fn(&str) -> bool,
) -> (String, bool) {
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

    let start = Instant::now();
    let mut snap = String::new();
    while start.elapsed() < wait {
        snap = sup.snapshot();
        if pred(&snap) {
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
    (snap, alive)
}

#[test]
#[ignore = "live: needs opencode binary on this machine"]
fn opencode_tui_paints_in_supervisor_pty() {
    // First-paint gate: any alt-screen entry OR truecolor SGR (probe phase counts).
    let (snap, _) = spawn_and_wait(
        Harness::OpenCode,
        "wsliveoc",
        Duration::from_secs(20),
        |s| s.contains("\x1b[?1049h") || s.contains("\x1b[38;2;") || s.contains("\x1b[?2031h"),
    );
    assert!(
        snap.contains("\x1b[?1049h")
            || snap.contains("\x1b[38;2;")
            || snap.contains("\x1b[?2031h")
            || snap.len() > 200,
        "opencode pane must paint TUI bytes, got {} bytes: {snap:?}",
        snap.len()
    );
}

#[test]
#[ignore = "live: needs opencode binary on this machine"]
fn opencode_tui_paints_substantial_sgr_after_auto_answer() {
    // Must wait past the ~373B probe phase; host auto-answer unlocks full SGR ~3s later.
    let (snap, _) = spawn_and_wait(
        Harness::OpenCode,
        "wsliveoc2",
        Duration::from_secs(15),
        |s| {
            (s.contains("\x1b[38;2;") || s.contains("\x1b[48;2;")) && s.len() > 2000
        },
    );
    let has_sgr = snap.contains("\x1b[38;2;") || snap.contains("\x1b[48;2;");
    assert!(
        has_sgr && snap.len() > 2000,
        "expected substantial OpenTUI paint after probe auto-answer; bytes={} sgr={} head={:?}",
        snap.len(),
        has_sgr,
        snap.chars().take(120).collect::<String>()
    );
}

#[test]
#[ignore = "live: needs pi binary on this machine"]
fn pi_tui_paints_in_supervisor_pty() {
    let (snap, _) = spawn_and_wait(
        Harness::Pi,
        "wslivepi",
        Duration::from_secs(20),
        |s| s.contains("\x1b[?1049h") || s.contains("\x1b[38;2;") || s.len() > 200,
    );
    assert!(
        snap.contains("\x1b[?1049h") || snap.contains("\x1b[38;2;") || snap.len() > 200,
        "pi pane must paint TUI bytes, got {} bytes: {snap:?}",
        snap.len()
    );
}
