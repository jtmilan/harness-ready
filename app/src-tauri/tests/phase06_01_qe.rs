//! QE (06-01): lock `agent_teams_core` wire projection + rank ordering, live-registry
//! round-trip, and Phase-B inert build — without touching `lib.rs`.

use agent_teams_core::{
    compute_queue, read_registry, registry_path, LiveRegistry, LiveWorkspace, LIVE_REGISTRY_SCHEMA,
};
use state_adapter::watch::{current_states, discover};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Scratch {
    root: PathBuf,
    state: PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!("at-qe-06-01-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let state = root.join("state");
        fs::create_dir_all(&state).unwrap();
        Scratch { root, state }
    }

    fn workspace(&self, id: &str, line: &str) {
        let wsdir = self.state.join(id);
        fs::create_dir_all(&wsdir).unwrap();
        fs::write(wsdir.join("events.jsonl"), format!("{line}\n")).unwrap();
    }

    fn write_registry(&self, json: &str) {
        fs::write(registry_path(&self.state).unwrap(), json).unwrap();
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

/// Adapter / §5.7 wire strings — NOT the legacy `turnend` / `ratelimit` bug.
#[test]
fn ac1_core_projection_uses_canonical_turn_end_and_rate_limit() {
    let s = Scratch::new("wire");
    s.workspace(
        "rate",
        r#"{"harness":"cursor","event":"resource_exhausted","ts":3000}"#,
    );
    s.workspace("finished", r#"{"harness":"cursor","event":"stop","ts":2000}"#);

    let rate = compute_queue(s.path(), None)
        .into_iter()
        .find(|r| r.id == "rate")
        .expect("rate row");
    assert_eq!(rate.reason.as_deref(), Some("rate_limit"));
    assert!(!rate.needs_human, "rate_limit routes to scheduler, not the human queue");

    let done = compute_queue(s.path(), None)
        .into_iter()
        .find(|r| r.id == "finished")
        .expect("done row");
    assert_eq!(done.reason.as_deref(), Some("turn_end"));
    assert!(!done.needs_human);

    for r in compute_queue(s.path(), None) {
        let reason = r.reason.as_deref().unwrap_or("");
        assert_ne!(reason, "turnend", "must not emit legacy Debug-lowercased form");
        assert_ne!(reason, "ratelimit");
    }
}

/// Same ordering as `state_adapter::rank`: needs_human first, then
/// approval > question > turn_end > rate_limit, tie-broken by `since`.
#[test]
fn ac2_needs_human_and_reason_priority_matches_single_source_rank() {
    let s = Scratch::new("rank");
    s.workspace("working", r#"{"harness":"claude","event":"SessionStart","ts":1}"#);
    s.workspace("finished", r#"{"harness":"cursor","event":"stop","ts":2}"#);
    s.workspace(
        "rate",
        r#"{"harness":"cursor","event":"resource_exhausted","ts":3}"#,
    );
    s.workspace(
        "question",
        r#"{"harness":"claude","event":"Notification","ts":10}"#,
    );
    s.workspace(
        "approval_old",
        r#"{"harness":"claude","event":"PermissionRequest","ts":50}"#,
    );
    s.workspace(
        "approval_new",
        r#"{"harness":"claude","event":"PermissionRequest","ts":100}"#,
    );

    let core_ids: Vec<String> = compute_queue(s.path(), None)
        .iter()
        .map(|r| r.id.clone())
        .collect();

    let adapter_ids: Vec<String> = current_states(&discover(s.path()))
        .into_iter()
        .map(|(id, _, _)| id)
        .collect();

    assert_eq!(
        core_ids,
        adapter_ids,
        "compute_queue must preserve state_adapter rank order"
    );
    assert_eq!(
        core_ids,
        vec![
            "approval_old",
            "approval_new",
            "question",
            "finished",
            "rate",
            "working"
        ]
    );
    assert!(compute_queue(s.path(), None)[0].needs_human);
    assert_eq!(
        compute_queue(s.path(), None)[2].reason.as_deref(),
        Some("question")
    );
    assert_eq!(
        compute_queue(s.path(), None)[3].reason.as_deref(),
        Some("turn_end")
    );
    assert_eq!(
        compute_queue(s.path(), None)[4].reason.as_deref(),
        Some("rate_limit")
    );
}

#[test]
fn ac3_live_registry_format_roundtrip() {
    let s = Scratch::new("registry-rt");
    let json = r#"{
        "schema": 1,
        "app_pid": 424242,
        "updated_at": 1735689600123,
        "workspaces": [
            {
                "id": "ws-alpha",
                "pid": 5001,
                "harness": "claude",
                "repo": "/tmp/agent-teams/repo",
                "spawned_at": 1735689600456
            }
        ]
    }"#;
    s.write_registry(json);

    let reg = read_registry(s.path()).expect("registry parses");
    assert_eq!(reg.schema, LIVE_REGISTRY_SCHEMA);
    assert_eq!(reg.app_pid, Some(424242));
    assert_eq!(reg.updated_at, Some(1735689600123));
    assert_eq!(reg.workspaces.len(), 1);
    let w = &reg.workspaces[0];
    assert_eq!(w.id, "ws-alpha");
    assert_eq!(w.pid, Some(5001));
    assert_eq!(w.harness.as_deref(), Some("claude"));
    assert_eq!(w.repo.as_deref(), Some("/tmp/agent-teams/repo"));
    assert_eq!(w.spawned_at, Some(1735689600456));

    let roundtrip_json = serde_json::to_string(&reg).unwrap();
    let back: LiveRegistry = serde_json::from_str(&roundtrip_json).unwrap();
    assert_eq!(back, reg);

    let expected = LiveRegistry {
        schema: 1,
        app_pid: Some(424242),
        updated_at: Some(1735689600123),
        active: None,
        workspaces: vec![LiveWorkspace {
            id: "ws-alpha".into(),
            pid: Some(5001),
            harness: Some("claude".into()),
            repo: Some("/tmp/agent-teams/repo".into()),
            role: None,
            tag: None,
            session_id: None,
            spawned_at: Some(1735689600456),
        }],
    };
    assert_eq!(back, expected);

    let live: HashSet<String> = back.live_ids();
    assert_eq!(live.len(), 1);
    assert!(live.contains("ws-alpha"));
}

#[test]
fn ac4_registry_path_is_sibling_not_under_state_root() {
    let s = Scratch::new("sibling");
    let reg = registry_path(s.path()).unwrap();
    assert_eq!(reg, s.root.join("agent-teams-live.json"));
    assert!(!reg.starts_with(s.path()));
}

#[test]
fn ac5_agent_teams_mcp_phase_b_mutations_builds_inert() {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let status = Command::new("cargo")
        .args([
            "build",
            "-p",
            "agent-teams-mcp",
            "--features",
            "phase-b-mutations",
            "--quiet",
        ])
        .current_dir(&repo_root)
        .status()
        .expect("spawn cargo build");
    assert!(
        status.success(),
        "agent-teams-mcp with phase-b-mutations must compile (inert stubs)"
    );
}
