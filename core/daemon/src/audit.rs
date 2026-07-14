//! Q4 daemon-spawn audit log (must-fix E1) — append-only operability trail.
//!
//! Because a daemon-owned pane SURVIVES app quit, the app's in-process panic-button is
//! gone for it. This append-only JSONL log (a SIBLING of `state_root`, surviving the
//! cold-start wipe like the live registry + run-state) records every `Spawn`/`Close`/
//! `reap`/`kill_all` the daemon performs, so an operator can see what now-detached agents
//! exist and which worktrees they own. Best-effort: a log-write failure is swallowed (it
//! must never crash or fail a spawn).
//!
//! Compiled only under `cfg(any(test, feature = "daemon-spawn"))` — absent from the
//! default build (the Q4 handler that drives it is likewise compiled out).

use std::io::Write;
use std::path::{Path, PathBuf};

/// Filename of the daemon spawn audit log (sibling of `state_root`). Distinct from the
/// live registry / run-state / IPC siblings.
pub const DAEMON_AUDIT_FILE: &str = "agent-teams-daemon-audit.jsonl";

/// The audit-log path: `<state_root>/../agent-teams-daemon-audit.jsonl` (sibling of
/// `state_root`, mirroring `agent_teams_core::registry_path`). `None` if no parent.
pub fn audit_path(state_root: &Path) -> Option<PathBuf> {
    state_root.parent().map(|p| p.join(DAEMON_AUDIT_FILE))
}

/// One audit record's fields (the daemon serializes them into a flat JSON line). Kept as
/// explicit args (not a struct) so the call sites read as a checklist of what is recorded.
#[allow(clippy::too_many_arguments)]
fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Append a `Spawn` audit record. `extra_dirs` is recorded as a count (the dirs
/// themselves can be long + are already repo-scoped) alongside the identifying fields.
pub fn audit_spawn(
    state_root: &Path,
    id: &str,
    harness: &str,
    repo: &str,
    is_worker: bool,
    extra_dirs: usize,
    child_pid: Option<u32>,
) {
    let line = serde_json::json!({
        "ts": unix_millis(),
        "event": "spawn",
        "id": id,
        "harness": harness,
        "repo": repo,
        "is_worker": is_worker,
        "extra_dirs": extra_dirs,
        "child_pid": child_pid,
    })
    .to_string();
    append_line(state_root, &line);
}

/// Append a lifecycle record for `event` (`"close"` / `"reap"` / `"kill_all"`) on `id`.
pub fn audit_event(state_root: &Path, event: &str, id: &str) {
    let line = serde_json::json!({
        "ts": unix_millis(),
        "event": event,
        "id": id,
    })
    .to_string();
    append_line(state_root, &line);
}

/// Best-effort append of one JSON line + `\n`. A missing parent / write error is
/// swallowed — auditing must never crash the daemon or fail a spawn.
fn append_line(state_root: &Path, line: &str) {
    let Some(path) = audit_path(state_root) else {
        return;
    };
    // 0600: the audit trail records repo paths / ids of detached agents — owner-only.
    use std::os::unix::fs::OpenOptionsExt;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&path)
    {
        let _ = writeln!(f, "{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch {
        root: PathBuf,
        state: PathBuf,
    }
    impl Scratch {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "at-audit-{}-{}-{}",
                tag,
                std::process::id(),
                unix_millis()
            ));
            let _ = std::fs::remove_dir_all(&root);
            let state = root.join("state");
            std::fs::create_dir_all(&state).unwrap();
            Scratch { root, state }
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn audit_path_is_sibling_of_state_root() {
        let p = audit_path(Path::new("/var/app/agent-teams")).unwrap();
        assert_eq!(p, PathBuf::from("/var/app/agent-teams-daemon-audit.jsonl"));
        assert!(audit_path(Path::new("/")).is_none());
    }

    #[test]
    fn spawn_and_event_records_append_one_line_each() {
        let s = Scratch::new("append");
        audit_spawn(&s.state, "ws-1", "claude", "/repo", false, 0, Some(4242));
        audit_event(&s.state, "close", "ws-1");
        let body = std::fs::read_to_string(audit_path(&s.state).unwrap()).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 2, "two appended records");
        let spawn: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(spawn["event"], "spawn");
        assert_eq!(spawn["id"], "ws-1");
        assert_eq!(spawn["child_pid"], 4242);
        let close: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(close["event"], "close");
    }

    #[test]
    fn no_parent_state_root_is_a_silent_noop() {
        // "/" has no parent → audit_path None → append is a no-op, never panics.
        audit_spawn(Path::new("/"), "ws", "bash", "/r", false, 0, None);
        audit_event(Path::new("/"), "reap", "ws");
    }
}
