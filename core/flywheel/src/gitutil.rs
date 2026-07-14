//! Shared git/path helpers for the headless core (de-Tauri extraction).
//!
//! `harness_path()` is the inlined replacement for the old `supervisor::harness_path()` — the CLI
//! inherits the user's PATH (where the harness binaries already live). `git_capture()` shells `git`
//! and returns trimmed stdout on success. Pure git/path helpers — NOT LLM calls — so the
//! `orchestrate::run_claude_capture` LLM seam is not implicated.

use std::path::Path;
use std::sync::OnceLock;

/// Resolve the PATH the harness binaries (`claude`/`git`/`gh`/cursor/…) live on. Ported from
/// `supervisor::harness_path()` so a process launched WITHOUT a login shell — notably the GUI app
/// launched from the macOS dock with a minimal PATH — still finds the harness binaries. Sources the
/// user's login shell (so Homebrew's `shellenv` etc. apply) and prepends the common bin dirs. A
/// sentinel brackets the value so any profile chatter on stdout is ignored. Memoized (the login
/// shell spawns at most once per process). Falls back to the inherited PATH if the shell probe fails.
pub(crate) fn harness_path() -> String {
    static PATH: OnceLock<String> = OnceLock::new();
    PATH.get_or_init(|| {
        let home = std::env::var("HOME").unwrap_or_default();
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
        let from_shell = std::process::Command::new(&shell)
            .args(["-lc", "printf '___ATPATH___%s___ATPATH___' \"$PATH\""])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .split("___ATPATH___")
                    .nth(1)
                    .map(str::to_string)
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| std::env::var("PATH").unwrap_or_default());
        format!("/opt/homebrew/bin:/usr/local/bin:{home}/.local/bin:{home}/.cargo/bin:{from_shell}")
    })
    .clone()
}

/// `git -C <dir> <args>` → trimmed stdout on success, else None. PATH pinned to the inherited
/// harness PATH (a no-op vs default inheritance, kept explicit for parity with the harness env).
pub(crate) fn git_capture(dir: &Path, args: &[&str]) -> Option<String> {
    let o = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("PATH", harness_path())
        .output()
        .ok()?;
    o.status
        .success()
        .then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
}
