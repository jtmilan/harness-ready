//! AC-2: inject() writes a valid per-workspace config and leaves the repo's
//! tracked tree clean. Shells out to `git` (no network).

use state_adapter::inject::{inject, InjectConfig, InjectHarness};
use std::path::PathBuf;
use std::process::Command;

fn hooks_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../hooks")
}

#[test]
fn cursor_inject_writes_config_and_keeps_git_clean() {
    let tmp = std::env::temp_dir().join(format!("at-inject-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let ok = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&tmp)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    assert!(ok, "git init failed");

    let cfg = InjectConfig {
        workspace_id: "ws-test".into(),
        repo_dir: tmp.clone(),
        writer_path: hooks_dir().join("state-writer.sh"),
        templates_dir: hooks_dir(),
        state_root: std::env::temp_dir().join("at-inject-state"),
        pane_id: "ws-test-p0".into(),
        repo_key: "ws-test".into(),
        task_scope: "ws-test-p0".into(),
    };
    let out = inject(&cfg, InjectHarness::Cursor, "").unwrap();

    let rendered = std::fs::read_to_string(&out).unwrap();
    assert!(rendered.contains("ws-test"), "WSID rendered");
    assert!(rendered.contains("beforeShellExecution"), "events wired");
    assert!(rendered.contains("state-writer.sh"), "writer path rendered");

    let exclude = std::fs::read_to_string(tmp.join(".git/info/exclude")).unwrap();
    assert!(
        exclude.contains(".cursor/hooks.json"),
        "config excluded from git"
    );

    let status = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&tmp)
        .output()
        .unwrap();
    let s = String::from_utf8_lossy(&status.stdout);
    assert!(
        !s.contains(".cursor"),
        "injected config must NOT appear in git status, got: {s:?}"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

fn git_init(dir: &std::path::Path) {
    let ok = Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    assert!(ok, "git init failed");
}

fn cfg_for(tmp: &std::path::Path) -> InjectConfig {
    InjectConfig {
        workspace_id: "ws-test".into(),
        repo_dir: tmp.to_path_buf(),
        writer_path: hooks_dir().join("state-writer.sh"),
        templates_dir: hooks_dir(),
        state_root: std::env::temp_dir().join("at-inject-state"),
        pane_id: "ws-test-p0".into(),
        repo_key: "ws-test".into(),
        task_scope: "ws-test-p0".into(),
    }
}

// The CLAUDE injection path: distinct template (.claude/settings.local.json,
// PermissionRequest events) — previously only the Cursor arm was tested.
#[test]
fn claude_inject_writes_config_and_keeps_git_clean() {
    let tmp = std::env::temp_dir().join(format!("at-inject-claude-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    git_init(&tmp);

    let out = inject(&cfg_for(&tmp), InjectHarness::Claude, "").unwrap();
    assert!(
        out.ends_with(".claude/settings.local.json"),
        "claude target path: {out:?}"
    );
    let rendered = std::fs::read_to_string(&out).unwrap();
    assert!(rendered.contains("ws-test"), "WSID rendered");
    assert!(
        rendered.contains("PermissionRequest"),
        "claude-specific event wired"
    );
    assert!(rendered.contains("state-writer.sh"), "writer path rendered");

    let exclude = std::fs::read_to_string(tmp.join(".git/info/exclude")).unwrap();
    assert!(
        exclude.contains(".claude/settings.local.json"),
        "config excluded from git"
    );
    let status = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&tmp)
        .output()
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&status.stdout).contains(".claude"),
        "config must NOT appear in git status"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

// exclude_from_git's not-a-git-repo early return: inject still writes the config + Ok.
#[test]
fn inject_into_non_git_dir_writes_config_and_is_ok() {
    let tmp = std::env::temp_dir().join(format!("at-inject-nogit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    // no `git init` → the .git/info dir is absent
    let out = inject(&cfg_for(&tmp), InjectHarness::Cursor, "")
        .expect("inject Ok even without a git repo");
    assert!(
        std::fs::read_to_string(&out).unwrap().contains("ws-test"),
        "config still written"
    );
    assert!(
        !tmp.join(".git/info/exclude").exists(),
        "exclude step skipped (no .git)"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

// exclude_from_git is idempotent: re-injecting (Supervisor::spawn does this per restart)
// must not duplicate the exclude entry.
#[test]
fn repeat_inject_does_not_duplicate_exclude_entry() {
    let tmp = std::env::temp_dir().join(format!("at-inject-repeat-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    git_init(&tmp);
    inject(&cfg_for(&tmp), InjectHarness::Cursor, "").unwrap();
    inject(&cfg_for(&tmp), InjectHarness::Cursor, "").unwrap();
    let exclude = std::fs::read_to_string(tmp.join(".git/info/exclude")).unwrap();
    assert_eq!(
        exclude.matches(".cursor/hooks.json").count(),
        1,
        "exclude entry not duplicated on re-inject"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

// the trailing-newline prepend: a prior exclude lacking a final newline must not get the
// rel glued onto its last line.
#[test]
fn inject_separates_exclude_entry_when_prior_content_lacks_newline() {
    let tmp = std::env::temp_dir().join(format!("at-inject-nonl-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    git_init(&tmp);
    std::fs::write(tmp.join(".git/info/exclude"), "*.log").unwrap(); // NO trailing newline
    inject(&cfg_for(&tmp), InjectHarness::Cursor, "").unwrap();
    let exclude = std::fs::read_to_string(tmp.join(".git/info/exclude")).unwrap();
    assert!(
        exclude.contains("*.log\n.cursor/hooks.json"),
        "rel must land on its own line, got: {exclude:?}"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

// Fix-B: a non-empty `env_block` is spliced into the claude template's {{ENV_BLOCK}}
// slot and the result is still valid JSON carrying the env (the Bedrock-merge surface).
#[test]
fn claude_inject_with_env_block_renders_valid_json_with_env() {
    let tmp = std::env::temp_dir().join(format!("at-inject-env-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    git_init(&tmp);

    // exactly what the supervisor's env-merge helper renders: a leading-comma `"env"` frag
    let env_block = ",\n  \"env\": {\n    \"CLAUDE_CODE_USE_BEDROCK\": \"1\",\n    \"AWS_PROFILE\": \"staging\"\n  }";
    let out = inject(&cfg_for(&tmp), InjectHarness::Claude, env_block).unwrap();

    let rendered = std::fs::read_to_string(&out).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&rendered).expect("merged settings.local.json must be valid JSON");
    // hooks survive AND the env block merged in
    assert!(v.get("hooks").is_some(), "hooks preserved");
    assert_eq!(
        v["env"]["CLAUDE_CODE_USE_BEDROCK"], "1",
        "bedrock switch merged"
    );
    assert_eq!(v["env"]["AWS_PROFILE"], "staging", "aws profile merged");
    let _ = std::fs::remove_dir_all(&tmp);
}

// Fix-B: the empty `env_block` (the default / no-env case) must still produce valid JSON —
// the {{ENV_BLOCK}} token vanishes cleanly, leaving the hooks-only object.
#[test]
fn claude_inject_empty_env_block_is_valid_json() {
    let tmp = std::env::temp_dir().join(format!("at-inject-noenv-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    git_init(&tmp);

    let out = inject(&cfg_for(&tmp), InjectHarness::Claude, "").unwrap();
    let rendered = std::fs::read_to_string(&out).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&rendered).expect("hooks-only settings.local.json must be valid JSON");
    assert!(v.get("hooks").is_some(), "hooks present");
    assert!(v.get("env").is_none(), "no env key when env_block empty");
    let _ = std::fs::remove_dir_all(&tmp);
}
