//! The daemon's durable **run-state** file — a SIBLING of `state_root`.
//!
//! Per `08-01-PLAN` hard-problem #5, every durable file the daemon owns lives
//! *beside* `state_root`, not inside it, because `state_root` is wiped on cold
//! start. This file is the daemon's own minimal run record (its pid + last-touch),
//! distinct from the app-owned [`agent_teams_core::LiveRegistry`]
//! (`agent-teams-live.json`) and the IPC socket — different file, different owner.
//!
//! [`daemon_runstate_path`] MIRRORS [`agent_teams_core::registry_path`] exactly
//! (`state_root.parent().map(|p| p.join(FILE))`) so daemon run state and the live
//! registry co-locate and follow the same wipe-survival convention. The filename
//! [`DAEMON_RUNSTATE_FILE`] is chosen NOT to collide with any existing sibling
//! (`agent-teams-live.json`, `agent-teams-mcp.sock`, `mcp-config.json`,
//! `agent-teams-mcp-http.{token,port}`).
//!
//! This module does NOT wipe `state_root` — that move (D20 → daemon cold start) is
//! a later sub-build. Here we only read/write the run-state JSON.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Filename of the daemon run-state file (sibling of `state_root`). Distinct from
/// every other sibling so the daemon's record never collides with the live
/// registry or the IPC files.
pub const DAEMON_RUNSTATE_FILE: &str = "agent-teams-daemon.json";

/// Current `schema` version of [`DaemonRunState`]. Bump on any breaking shape
/// change (mirrors `agent_teams_core::LIVE_REGISTRY_SCHEMA`).
pub const DAEMON_RUNSTATE_SCHEMA: u32 = 1;

/// The daemon's minimal durable run record. Forward/backward tolerant: unknown
/// fields are ignored on read (serde default) and `None` optionals are omitted on
/// write (`skip_serializing_if`), so an older/newer writer round-trips cleanly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonRunState {
    /// Format version. Readers tolerate `schema > DAEMON_RUNSTATE_SCHEMA` leniently
    /// (serde already ignores unknown fields).
    pub schema: u32,
    /// PID of the running daemon, if recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daemon_pid: Option<u32>,
    /// When the run-state was last rewritten (unix millis), if recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<u64>,
}

impl Default for DaemonRunState {
    /// A fresh record at the current schema with no pid / timestamp yet.
    fn default() -> Self {
        Self {
            schema: DAEMON_RUNSTATE_SCHEMA,
            daemon_pid: None,
            updated_at: None,
        }
    }
}

/// The daemon run-state path: `<state_root>/../agent-teams-daemon.json` (sibling of
/// `state_root`). `None` if `state_root` has no parent. MIRRORS
/// [`agent_teams_core::registry_path`] so the two siblings can never drift apart.
pub fn daemon_runstate_path(state_root: &Path) -> Option<PathBuf> {
    state_root
        .parent()
        .map(|parent| parent.join(DAEMON_RUNSTATE_FILE))
}

/// Write the run-state as JSON to [`daemon_runstate_path`]. Errors if `state_root`
/// has no parent (no sibling location) or the write fails. Modeled on the live
/// registry's write side; does NOT touch `state_root` itself.
pub fn write_runstate(state_root: &Path, runstate: &DaemonRunState) -> std::io::Result<()> {
    let path = daemon_runstate_path(state_root).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "state_root has no parent — no sibling location for daemon run state",
        )
    })?;
    let body = serde_json::to_string(runstate).map_err(std::io::Error::other)?;
    // tmp+rename so a concurrent reader never sees a torn/partial run-state file.
    crate::fsutil::write_atomic(&path, body.as_bytes())
}

/// Read + parse the run-state, or `None` if it is absent, unreadable, or malformed.
/// Mirrors [`agent_teams_core::read_registry`]: a parse error is treated as "no
/// run state," never a panic.
pub fn read_runstate(state_root: &Path) -> Option<DaemonRunState> {
    let path = daemon_runstate_path(state_root)?;
    let body = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&body).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A unique scratch dir, cleaned at drop. `state` is nested under a private
    /// root (`<root>/state`) so the run-state file — a SIBLING of the state dir
    /// (`<root>/agent-teams-daemon.json`) — is isolated per test rather than
    /// colliding in the shared system temp dir. Mirrors `core/mcp/src/lib.rs:625`.
    struct Scratch {
        root: PathBuf,
        state: PathBuf,
    }
    impl Scratch {
        fn new(tag: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("at-daemon-{}-{}", tag, std::process::id()));
            let _ = fs::remove_dir_all(&root);
            let state = root.join("state");
            fs::create_dir_all(&state).unwrap();
            Scratch { root, state }
        }
        fn path(&self) -> &Path {
            &self.state
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn daemon_runstate_path_is_sibling_of_state_root() {
        let p = daemon_runstate_path(Path::new("/var/app/agent-teams")).unwrap();
        assert_eq!(p, PathBuf::from("/var/app/agent-teams-daemon.json"));
        // Mirrors registry_path: the daemon run-state sits in the SAME directory as
        // the live registry (both siblings of state_root) — proving the mirror is
        // faithful and exercising the core path-dep.
        let reg = agent_teams_core::registry_path(Path::new("/var/app/agent-teams")).unwrap();
        assert_eq!(p.parent(), reg.parent());
        // Root has no parent → None (never panics), exactly like registry_path.
        assert!(daemon_runstate_path(Path::new("/")).is_none());
        assert!(agent_teams_core::registry_path(Path::new("/")).is_none());
    }

    #[test]
    fn daemon_runstate_roundtrips() {
        let s = Scratch::new("runstate");
        // Absent → None (no run state yet).
        assert!(read_runstate(s.path()).is_none());

        let rs = DaemonRunState {
            schema: DAEMON_RUNSTATE_SCHEMA,
            daemon_pid: Some(4242),
            updated_at: Some(1_700_000_000),
        };
        write_runstate(s.path(), &rs).unwrap();
        let back = read_runstate(s.path()).expect("present run state parses");
        assert_eq!(back, rs);

        // Forward-compat: an unknown field a newer writer adds is ignored on read.
        let path = daemon_runstate_path(s.path()).unwrap();
        fs::write(
            &path,
            r#"{"schema":2,"daemon_pid":7,"updated_at":99,"future_field":true}"#,
        )
        .unwrap();
        let tolerant =
            read_runstate(s.path()).expect("parses despite an unknown field + newer schema");
        assert_eq!(tolerant.schema, 2);
        assert_eq!(tolerant.daemon_pid, Some(7));

        // Malformed → None (lenient, never panics).
        fs::write(&path, "{ not json").unwrap();
        assert!(read_runstate(s.path()).is_none());
    }

    /// The documented omit-on-write contract: a DEFAULT record (both optionals None)
    /// serializes to JUST `{"schema":1}` (skip_serializing_if drops the Nones) and reads
    /// back as default. The both-Some roundtrip above never triggers the omit path, so a
    /// serde-attribute regression on either optional would escape it.
    #[test]
    fn default_runstate_omits_none_optionals_and_roundtrips() {
        assert_eq!(
            serde_json::to_string(&DaemonRunState::default()).unwrap(),
            r#"{"schema":1}"#,
            "None optionals must be omitted on write"
        );
        let s = Scratch::new("default");
        write_runstate(s.path(), &DaemonRunState::default()).unwrap();
        let back = read_runstate(s.path()).expect("default run state parses");
        assert_eq!(
            back,
            DaemonRunState::default(),
            "omitted fields default back to None"
        );
    }
}
