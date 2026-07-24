//! The daemon's writer for `agent-teams-live.json` (Phase 08 Sub-build 2 / 08-T4).
//!
//! Today the GUI app owns the live registry (`app/src-tauri/src/lib.rs`'s
//! `live_registry_write`): it stamps `app_pid = <GUI pid>` and a workspace set with
//! `pid: None` (the GUI has no per-pane child-pid accessor). When the daemon becomes
//! the PTY owner (Sub-build 3 wires the socket server), the DAEMON writes this file
//! instead — with its OWN pid as `app_pid` and each pane's real child pid. This module
//! is that writer: built now (the machinery), driven by the daemon's request handlers
//! in Sub-build 3.
//!
//! SSOT: the path + types come from `agent_teams_core` ([`registry_path`] /
//! [`LiveRegistry`] / [`LiveWorkspace`] / [`LIVE_REGISTRY_SCHEMA`]) — never redefined
//! here, so the GUI writer, the daemon writer, and the sidecar reader can never drift.

use agent_teams_core::{registry_path, LiveRegistry, LiveWorkspace, LIVE_REGISTRY_SCHEMA};
use std::collections::HashMap;
use std::path::Path;

/// Unix-millis wall clock for `updated_at` (the registry's own clock; no chrono dep —
/// `agent_teams_core::LiveRegistry::updated_at` is `Option<u64>` millis, not a string).
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Build the daemon's view of the live registry: the daemon's pid as `app_pid` and one
/// [`LiveWorkspace`] per `(id → child_pid)`, each with its child pid POPULATED (the GUI
/// writer left it `None`). PURE over its inputs so it is unit-tested without disk I/O.
pub fn build_registry(daemon_pid: u32, workspace_pids: &HashMap<String, u32>) -> LiveRegistry {
    let mut workspaces: Vec<LiveWorkspace> = workspace_pids
        .iter()
        .map(|(id, child_pid)| LiveWorkspace {
            id: id.clone(),
            pid: Some(*child_pid),
            harness: None,
            repo: None,
            // The daemon's pid map carries no spawn identity — role/tag stay None
            // (the GUI writer records them; serde-additive so both writers coexist).
            role: None,
            tag: None,
            session_id: None,
            spawned_at: None,
            allow_sharing: false,
        })
        .collect();
    // Deterministic order (HashMap iteration is unordered) so the on-disk file is stable
    // across rewrites with the same set — easier to diff, and the test can assert it.
    workspaces.sort_by(|a, b| a.id.cmp(&b.id));
    LiveRegistry {
        schema: LIVE_REGISTRY_SCHEMA,
        app_pid: Some(daemon_pid),
        updated_at: Some(now_millis()),
        active: None, // the daemon lane does not track the frontend's active workspace
        workspaces,
    }
}

/// Overwrite the live registry (sibling of `state_root`) with the daemon's pid + the
/// current `id → child_pid` set. Best-effort: a missing parent or a write error is
/// swallowed (a failed registry write must never crash the daemon — the sidecar simply
/// falls back to the discovered superset, exactly as it does when the app is down).
pub fn write_live_registry(state_root: &Path, workspace_pids: &HashMap<String, u32>) {
    let Some(path) = registry_path(state_root) else {
        return;
    };
    let registry = build_registry(std::process::id(), workspace_pids);
    if let Ok(json) = serde_json::to_string(&registry) {
        // tmp+rename so a concurrent reader never sees a torn/partial registry.
        let _ = crate::fsutil::write_atomic(&path, json.as_bytes());
    }
}

/// Clear the live registry — an EMPTY workspace set with no `app_pid` — called on the
/// daemon's cold start (AC-4) so stale entries from a prior run never linger. Mirrors
/// the GUI's startup stamp, but the daemon owns it once it is the PTY owner.
pub fn clear_live_registry(state_root: &Path) {
    let Some(path) = registry_path(state_root) else {
        return;
    };
    let registry = LiveRegistry {
        schema: LIVE_REGISTRY_SCHEMA,
        app_pid: None,
        updated_at: Some(now_millis()),
        active: None,
        workspaces: Vec::new(),
    };
    if let Ok(json) = serde_json::to_string(&registry) {
        // tmp+rename so a concurrent reader never sees a torn/partial registry.
        let _ = crate::fsutil::write_atomic(&path, json.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_teams_core::read_registry;

    #[test]
    fn build_registry_populates_pid_and_is_deterministic() {
        let mut pids = HashMap::new();
        pids.insert("ws-2".to_string(), 222u32);
        pids.insert("ws-1".to_string(), 111u32);

        let reg = build_registry(999, &pids);
        assert_eq!(reg.schema, LIVE_REGISTRY_SCHEMA);
        assert_eq!(
            reg.app_pid,
            Some(999),
            "daemon pid is app_pid (not the GUI's)"
        );
        // sorted by id → deterministic on-disk order
        assert_eq!(reg.workspaces.len(), 2);
        assert_eq!(reg.workspaces[0].id, "ws-1");
        assert_eq!(
            reg.workspaces[0].pid,
            Some(111),
            "child pid POPULATED (GUI left None)"
        );
        assert_eq!(reg.workspaces[1].id, "ws-2");
        assert_eq!(reg.workspaces[1].pid, Some(222));
    }

    #[test]
    fn write_then_read_round_trips_via_core() {
        // a state_root with a parent → registry_path resolves to the sibling file
        let tmp = std::env::temp_dir().join(format!(
            "at-regwriter-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let state_root = tmp.join("agent-teams");
        std::fs::create_dir_all(&state_root).unwrap();

        let mut pids = HashMap::new();
        pids.insert("ws-1".to_string(), 4242u32);
        write_live_registry(&state_root, &pids);

        // the SSOT reader sees what the daemon wrote (sibling-of-state_root location)
        let back = read_registry(&state_root).expect("registry written + readable");
        assert_eq!(back.app_pid, Some(std::process::id()));
        assert_eq!(back.workspaces.len(), 1);
        assert_eq!(back.workspaces[0].id, "ws-1");
        assert_eq!(back.workspaces[0].pid, Some(4242));

        // clear → empty set, no app_pid
        clear_live_registry(&state_root);
        let cleared = read_registry(&state_root).expect("cleared registry readable");
        assert!(cleared.app_pid.is_none());
        assert!(cleared.workspaces.is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn no_parent_state_root_is_a_silent_noop() {
        // "/" has no parent → registry_path None → write/clear are no-ops, never panic
        write_live_registry(Path::new("/"), &HashMap::new());
        clear_live_registry(Path::new("/"));
    }
}
