//! Re-attach partition — the pure decision behind Sub-build 3's anti-double-spawn
//! guarantee (AC-2 / AC-6), landed ahead of the socket server it will drive.
//!
//! When the GUI relaunches it holds a list of stored workspace ids. For each, it
//! must decide: is this pane still LIVE in the daemon (→ re-attach via the streaming
//! subscription, NEVER spawn — AC-6), or is it merely stored-but-dead (→ spawn +
//! conversation-resume, the D19/D20 Tier-2 fallback)? That decision is a pure set
//! operation over the daemon-written [`agent_teams_core::LiveRegistry`]; it needs no
//! socket, clock, or PTY, so it lands + unit-tests now. Sub-build 3 will call this
//! the moment the GUI connects, before any spawn.

use agent_teams_core::LiveRegistry;
use std::collections::HashMap;

/// What the GUI should do with one stored workspace id on relaunch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReattachDecision {
    /// The id is live in the daemon's registry → re-attach via the streaming
    /// subscription (snapshot + deltas). The GUI MUST NOT spawn it (AC-6: exactly
    /// one harness child per live id). `owner_pid` is the registry's recorded owner
    /// (the daemon, once Sub-build 2 moves PTY ownership; the app today).
    Reattach { owner_pid: u32 },
    /// The id is stored by the GUI but NOT live → spawn + conversation-resume
    /// (Tier 2, `--resume` / `--continue`, D19/D20).
    ColdResume,
}

/// Partition the GUI's `stored_ids` into re-attach vs cold-resume by consulting the
/// daemon-written `registry`. Pure: no IO, no clock, no socket.
///
/// Rules (AC-2 / AC-3 / AC-6):
/// - The registry records an owner pid AND lists the id as live → [`ReattachDecision::Reattach`].
/// - Otherwise → [`ReattachDecision::ColdResume`].
/// - No owner pid recorded at all (no daemon) → EVERY id is cold-resume.
///
/// Duplicate ids in `stored_ids` collapse (the result is keyed by id); an id listed
/// in the registry but not stored by the GUI is ignored (the GUI only acts on what
/// it remembers).
pub fn partition_reattach(
    registry: &LiveRegistry,
    stored_ids: &[String],
) -> HashMap<String, ReattachDecision> {
    let owner_pid = registry.app_pid;
    stored_ids
        .iter()
        .map(|id| {
            let decision = match owner_pid {
                Some(pid) if registry.workspaces.iter().any(|ws| &ws.id == id) => {
                    ReattachDecision::Reattach { owner_pid: pid }
                }
                _ => ReattachDecision::ColdResume,
            };
            (id.clone(), decision)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Build a LiveRegistry from JSON (the structs don't derive Default, and only
    // `id` is required on a LiveWorkspace — serde fills the rest). This mirrors how a
    // real registry is read off disk.
    fn registry(json: &str) -> LiveRegistry {
        serde_json::from_str(json).expect("valid LiveRegistry json")
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn live_ids_reattach_with_the_owner_pid() {
        let reg =
            registry(r#"{"schema":1,"app_pid":4242,"workspaces":[{"id":"ws-p0"},{"id":"ws-p1"}]}"#);
        let out = partition_reattach(&reg, &ids(&["ws-p0", "ws-p1"]));
        assert_eq!(out["ws-p0"], ReattachDecision::Reattach { owner_pid: 4242 });
        assert_eq!(out["ws-p1"], ReattachDecision::Reattach { owner_pid: 4242 });
    }

    #[test]
    fn stored_but_not_live_is_cold_resume() {
        let reg = registry(r#"{"schema":1,"app_pid":7,"workspaces":[{"id":"ws-p0"}]}"#);
        let out = partition_reattach(&reg, &ids(&["ws-p0", "ws-ghost"]));
        assert_eq!(out["ws-p0"], ReattachDecision::Reattach { owner_pid: 7 });
        assert_eq!(
            out["ws-ghost"],
            ReattachDecision::ColdResume,
            "stored but absent → cold"
        );
    }

    #[test]
    fn no_owner_pid_means_no_daemon_so_everything_is_cold() {
        // app_pid omitted → even ids the registry happens to list are cold-resume
        // (no live owner to attach to).
        let reg = registry(r#"{"schema":1,"workspaces":[{"id":"ws-p0"}]}"#);
        let out = partition_reattach(&reg, &ids(&["ws-p0"]));
        assert_eq!(out["ws-p0"], ReattachDecision::ColdResume);
    }

    #[test]
    fn empty_registry_and_empty_input() {
        let reg = registry(r#"{"schema":1,"app_pid":1,"workspaces":[]}"#);
        assert!(partition_reattach(&reg, &[]).is_empty());
        let out = partition_reattach(&reg, &ids(&["x"]));
        assert_eq!(
            out["x"],
            ReattachDecision::ColdResume,
            "owner present but no live panes → cold"
        );
    }

    #[test]
    fn mixed_live_and_dead_partitions_correctly() {
        let reg = registry(r#"{"schema":1,"app_pid":99,"workspaces":[{"id":"a"},{"id":"c"}]}"#);
        let out = partition_reattach(&reg, &ids(&["a", "b", "c"]));
        assert_eq!(out["a"], ReattachDecision::Reattach { owner_pid: 99 });
        assert_eq!(out["b"], ReattachDecision::ColdResume);
        assert_eq!(out["c"], ReattachDecision::Reattach { owner_pid: 99 });
        assert_eq!(out.len(), 3);
    }
}
