//! `agent-teams-daemon` binary — Phase 08 Sub-build 3 (slice 2).
//!
//! GATED OFF (bundled-but-inert): as of 08-T9 this binary IS bundled (it is in
//! `tauri.conf` `externalBin`), but it is NEVER launched by a default install — its
//! launchd LaunchAgent registration is gated behind an explicit opt-in
//! (`AGENT_TEAMS_DAEMON_LAUNCHAGENT=1`, see `scripts/install-app.sh`); without it there
//! is no plist, no bootstrap, no socket bind, so the app remains the sole socket owner.
//! The binary exists so the daemon socket SERVER compiles + tests and the launch posture
//! is real. PTY-ownership transfer (how live panes get INTO the daemon's [`DaemonSups`])
//! is design Q4 — so this serves an EMPTY map today (every `ListLive` returns no ids
//! until spawn-transfer lands).
//!
//! ⚠️ SECURITY: the coordinator-only peer-pid gate parity LANDED in `server.rs`
//! (`handle_request_line` runs `roles::coordinator_gate` on the socket-peer → owning-pane
//! role for every mutating op — the same `op_requires_mutations` SSOT + FORBIDDEN
//! message/code as the app's `gate_socket_request`, fail-closed). Still OWED before
//! flipping the `AGENT_TEAMS_DAEMON_LAUNCHAGENT` opt-in: the LaunchAgent opt-in itself +
//! the live PTY-ownership-transfer (Q4) security review. Note the gate resolves against the
//! daemon's LIVE role-tagged pane map, which is EMPTY until PTY transfer lands — so today
//! every mutating op fails closed (no coordinator pane to resolve to), which is correct.
//!
//! ## Launch posture (MF-F)
//!
//! * Production (no flag) → **A1** launchd socket-activation
//!   ([`agent_teams_daemon::launch::LaunchdSocketActivation`]). On ANY A1 error this
//!   logs and EXITS (fail loud) — it MUST NOT silently self-bind. Under the ratified
//!   D45, A1 is the only posture that lets idle-shutdown (AC-4) coexist with
//!   auto-restart; a self-bind fallback would re-introduce the break D45 ruled out.
//! * `--dev` → **A2** [`agent_teams_daemon::launch::DoubleFork`] self-bind, the explicit
//!   developer escape hatch (run the daemon without an installed launchd plist).
//!
//! The A1-vs-A2 selection + the no-fallback guarantee live in
//! [`agent_teams_daemon::launch::acquire_listener_with_posture`].

use std::sync::Arc;

use agent_teams_daemon::launch::acquire_listener_with_posture;
use agent_teams_daemon::server::serve;
use agent_teams_daemon::sups::DaemonSups;

/// Resolve the app's `state_root` so the daemon reads the SAME `mcp-config.json`
/// (sibling of `state_root`) the app writes — the source of truth for the
/// per-request `allow_mutations` gate. Mirrors the hook convention
/// (`AGENT_TEAMS_STATE_DIR`, falling back to the default Application Support path).
fn resolve_state_root() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("AGENT_TEAMS_STATE_DIR") {
        return std::path::PathBuf::from(dir);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    std::path::PathBuf::from(home).join("Library/Application Support/agent-teams")
}

fn main() {
    // A2 (self-bind) is reachable ONLY via this explicit flag — never as an A1 fallback.
    let dev = std::env::args().skip(1).any(|a| a == "--dev");
    let state_root = resolve_state_root();

    let (listener, mech) = match acquire_listener_with_posture(dev, &state_root) {
        Ok(v) => v,
        Err(e) => {
            // MF-F: fail LOUD on A1 error — do NOT fall through to a self-bind.
            eprintln!(
                "agent-teams-daemon: acquire listener failed: {e}; exiting \
                 (A1 launchd socket-activation = fail-loud, no A2 fallback). \
                 Run with --dev to self-bind outside launchd."
            );
            std::process::exit(1);
        }
    };

    // Q4 (feature `daemon-spawn` ONLY): the daemon OWNS the live registry once it is the
    // PTY owner. COLD-START ORPHAN SWEEP (AC-4 + crash recovery): kill any prior-run child
    // that is still alive (per the prior live registry, only when its writer pid is dead),
    // remove every worktree recorded in the durable `agent-teams-daemon-worktrees.json`
    // trail, then clear BOTH files before the daemon begins spawning. No-op in the default
    // build (the daemon owns no panes and the app still owns the registry).
    #[cfg(feature = "daemon-spawn")]
    agent_teams_daemon::spawn::cold_start_sweep(&state_root);

    // Empty map until a gated `Spawn` populates it (default build serves it empty exactly
    // as before). The production stored value is the real `Supervisor` (the PTY-owning pane).
    let sups: Arc<DaemonSups<supervisor::Supervisor>> = Arc::new(DaemonSups::new());
    eprintln!(
        "agent-teams-daemon: listening via {mech}; state_root={}",
        state_root.display()
    );
    serve(listener, sups, state_root);
}
