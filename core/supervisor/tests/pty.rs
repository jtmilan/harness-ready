//! AC-1 (worktree) + AC-3 (PTY I/O) + AC-4 (lifecycle). Uses the `bash` test
//! harness — no agent quota burned.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use supervisor::*;

fn git(dir: &Path, args: &[&str]) {
    Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("git failed");
}

#[test]
fn worktree_add_then_remove() {
    let repo = std::env::temp_dir().join(format!("at-sup-wt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&repo);
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    std::fs::write(repo.join("README.md"), "x").unwrap();
    git(&repo, &["add", "."]);
    git(
        &repo,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "-m",
            "init",
        ],
    );

    let wt = add_worktree(&repo, "wsx").unwrap();
    assert!(wt.root.exists(), "worktree dir created");
    assert_eq!(wt.branch, "agent-teams/wsx");
    // repo-root target → cwd is the worktree root (no sparse subpath)
    assert_eq!(wt.cwd, wt.root);
    assert_eq!(
        wt.git_root,
        std::fs::canonicalize(&repo).unwrap_or(repo.clone())
    );
    // the repo-root worktree must MATERIALIZE the committed tree (a bare `git checkout`
    // repopulates the --no-checkout worktree) — guards against a future change shipping
    // empty repo-root panes, which the dir/path asserts above would not catch.
    assert!(
        wt.cwd.join("README.md").exists(),
        "repo-root worktree must materialize tracked files"
    );

    remove_worktree(&wt.git_root, "wsx", &wt.root).unwrap();
    assert!(!wt.root.exists(), "worktree removed");

    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn worktree_subfolder_is_sparse() {
    // AC-1: targeting a subfolder of a larger repo materializes ONLY that subtree.
    let repo = std::env::temp_dir().join(format!("at-sup-sparse-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&repo);
    std::fs::create_dir_all(repo.join("apps/proj/src")).unwrap();
    std::fs::create_dir_all(repo.join("other/big")).unwrap();
    git(&repo, &["init", "-q"]);
    std::fs::write(repo.join("apps/proj/src/main.txt"), "a").unwrap();
    std::fs::write(repo.join("ROOT.md"), "root").unwrap();
    for i in 0..20 {
        std::fs::write(repo.join(format!("other/big/f{i}.txt")), "filler").unwrap();
    }
    git(&repo, &["add", "."]);
    git(
        &repo,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "-m",
            "init",
        ],
    );

    let wt = add_worktree(&repo.join("apps/proj"), "wssub").unwrap();
    // agent cwd is the subfolder inside the worktree
    assert!(
        wt.cwd.ends_with("apps/proj"),
        "cwd is the subfolder: {:?}",
        wt.cwd
    );
    assert!(
        wt.cwd.join("src/main.txt").exists(),
        "subfolder file materialized"
    );
    // the unrelated big subtree must NOT be checked out
    assert!(
        !wt.root.join("other/big").exists(),
        "other/big NOT materialized (sparse)"
    );
    assert!(
        wt.root.join("ROOT.md").exists(),
        "repo-root files present (cone mode)"
    );

    remove_worktree(&wt.git_root, "wssub", &wt.root).unwrap();
    assert!(!wt.root.exists(), "worktree removed");
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn worktree_reused_when_dir_exists() {
    // AC-3 (Plan 04-02): reopening when the worktree dir still exists (e.g. kept by
    // the dirty-sweep) must REUSE it — same root/cwd, no `already exists` error.
    let repo = std::env::temp_dir().join(format!("at-sup-reuse-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&repo);
    std::fs::create_dir_all(repo.join("apps/proj")).unwrap();
    git(&repo, &["init", "-q"]);
    std::fs::write(repo.join("apps/proj/main.txt"), "a").unwrap();
    std::fs::write(repo.join("ROOT.md"), "root").unwrap();
    git(&repo, &["add", "."]);
    git(
        &repo,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "-m",
            "init",
        ],
    );

    // first call creates a sparse worktree of the subfolder
    let first = add_worktree(&repo.join("apps/proj"), "wsreuse").unwrap();
    // simulate an agent leaving an uncommitted file (dirty → sweep would keep it)
    std::fs::write(first.cwd.join("scratch.txt"), "wip").unwrap();

    // second call (reopen) must reuse the SAME worktree, not error
    let second = add_worktree(&repo.join("apps/proj"), "wsreuse").unwrap();
    assert_eq!(
        second.root, first.root,
        "reuse keeps the same worktree root"
    );
    assert_eq!(second.cwd, first.cwd, "reuse keeps the same agent cwd");
    assert!(
        second.cwd.ends_with("apps/proj"),
        "cwd still the subfolder: {:?}",
        second.cwd
    );
    assert!(
        second.cwd.join("scratch.txt").exists(),
        "uncommitted work preserved on reuse"
    );

    remove_worktree(&second.git_root, "wsreuse", &second.root).unwrap();
    let _ = std::fs::remove_dir_all(&repo);
}

// The repo-ROOT reuse cell (empty subpath → cwd == root). worktree_reused_when_dir_exists
// covers only the subfolder case; this pins the empty-subpath reuse branch.
#[test]
fn worktree_reused_at_repo_root_empty_subpath() {
    let repo = std::env::temp_dir().join(format!("at-sup-rootreuse-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&repo);
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    std::fs::write(repo.join("ROOT.md"), "root").unwrap();
    git(&repo, &["add", "."]);
    git(
        &repo,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "-m",
            "init",
        ],
    );

    // first call → repo-root worktree (no sparse subpath, so cwd == root)
    let first = add_worktree(&repo, "wsrootreuse").unwrap();
    assert_eq!(
        first.cwd, first.root,
        "repo-root target → cwd is the worktree root"
    );
    std::fs::write(first.cwd.join("scratch.txt"), "wip").unwrap();

    // second call (reopen) reuses the SAME worktree via the empty-subpath branch
    let second = add_worktree(&repo, "wsrootreuse").unwrap();
    assert_eq!(
        second.root, first.root,
        "reuse keeps the same worktree root"
    );
    assert_eq!(second.cwd, second.root, "empty-subpath reuse → cwd == root");
    assert!(
        second.cwd.join("scratch.txt").exists(),
        "uncommitted work preserved on reuse"
    );

    remove_worktree(&second.git_root, "wsrootreuse", &second.root).unwrap();
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn spawn_injects_dev_path() {
    // GUI apps get a minimal PATH; the supervisor must inject Homebrew/.local so
    // claude/cursor resolve. Spawn bash, echo $PATH, assert the prepend landed.
    let dir = std::env::temp_dir().join(format!("at-sup-path-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let spec = WorkspaceSpec {
        id: "wspath".into(),
        harness: Harness::Bash,
        worktree: dir.clone(),
        session_id: None,
        resume: false,
        role: None,
        is_worker: false,
        extra_dirs: vec![],
        model: None,
    };
    let hooks = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../hooks");
    let state = std::env::temp_dir().join("at-sup-state");
    // Bash skips injection entirely, so the sidecar path is never read (16-01).
    let sidecar = PathBuf::from("/unused/agent-teams-mcp");

    let mut sup = Supervisor::spawn(&spec, &hooks, &state, &sidecar).unwrap();
    sup.write(b"printf 'PATHIS:%s\\n' \"$PATH\"\n").unwrap();

    let start = Instant::now();
    let mut ok = false;
    while start.elapsed() < Duration::from_secs(5) {
        if sup.snapshot().contains("/opt/homebrew/bin") {
            ok = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        ok,
        "injected dev PATH must reach the child; got: {:?}",
        sup.snapshot()
    );
    // bun/pnpm bin dirs are GUARANTEED prepends (their exports live in .zshrc, which
    // the non-interactive login-shell probe never sources — a bun-installed opencode
    // was unspawnable without this).
    let snap = sup.snapshot();
    assert!(
        snap.contains("/.bun/bin"),
        "~/.bun/bin must be in the injected PATH; got: {snap:?}"
    );
    assert!(
        snap.contains("/Library/pnpm"),
        "~/Library/pnpm must be in the injected PATH; got: {snap:?}"
    );

    sup.kill();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pty_io_roundtrip_and_lifecycle() {
    let dir = std::env::temp_dir().join(format!("at-sup-pty-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let spec = WorkspaceSpec {
        id: "wsbash".into(),
        harness: Harness::Bash,
        worktree: dir.clone(),
        session_id: None,
        resume: false,
        role: None,
        is_worker: false,
        extra_dirs: vec![],
        model: None,
    };
    let hooks = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../hooks");
    let state = std::env::temp_dir().join("at-sup-state");
    // Bash skips injection entirely, so the sidecar path is never read (16-01).
    let sidecar = PathBuf::from("/unused/agent-teams-mcp");

    let mut sup = Supervisor::spawn(&spec, &hooks, &state, &sidecar).unwrap();
    assert!(sup.is_alive(), "session alive after spawn");

    // Q4 child_pid accessor: while the child is live its OS pid is Some(plausible pid)
    // — the value the daemon's registry writer captures AT SPAWN (it returns None once
    // the child is reaped, which is exactly why capture must precede any try_wait).
    let pid = sup.child_pid().expect("a live child has an OS pid");
    assert!(
        pid > 1,
        "child pid must be a plausible live pid (> 1), got {pid}"
    );

    sup.write(b"echo hello-pty-42\n").unwrap();

    let start = Instant::now();
    let mut seen = false;
    while start.elapsed() < Duration::from_secs(5) {
        if sup.snapshot().contains("hello-pty-42") {
            seen = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        seen,
        "PTY should stream echo output; got: {:?}",
        sup.snapshot()
    );

    sup.kill();
    let _ = std::fs::remove_dir_all(&dir);
}

// Gap #6 e2e (accidental-`/` cwd): a spec whose worktree is the filesystem ROOT — the
// cwd a GUI app inherits from `open`/launchd, which an unresolved repo path can pass
// straight through — must NOT start the child at `/`. The deliberate resolver
// (`resolve_spawn_cwd`) degrades it to HOME. A child at `/` slugs its claude
// transcript dir to a bare `-`, colliding across unrelated panes.
#[test]
fn spawn_never_runs_child_at_filesystem_root() {
    let spec = WorkspaceSpec {
        id: "wscwdroot".into(),
        harness: Harness::Bash,
        worktree: PathBuf::from("/"),
        session_id: None,
        resume: false,
        role: None,
        is_worker: false,
        extra_dirs: vec![],
        model: None,
    };
    let hooks = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../hooks");
    let state = std::env::temp_dir().join("at-sup-state");
    let sidecar = PathBuf::from("/unused/agent-teams-mcp");

    let mut sup = Supervisor::spawn(&spec, &hooks, &state, &sidecar).unwrap();
    // `CWD%sIS` in the typed line keeps the ECHOED input from matching the marker —
    // only the printf OUTPUT contains `CWDIS:`.
    sup.write(b"printf 'CWD%sIS:%s\\n' '' \"$PWD\"\n").unwrap();

    let start = Instant::now();
    let mut seen = false;
    while start.elapsed() < Duration::from_secs(5) {
        if sup.snapshot().contains("CWDIS:") {
            seen = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let snap = sup.snapshot();
    assert!(seen, "child must report its cwd; got: {snap:?}");
    // The PTY renders printf's \n as \r\n, so a root cwd is exactly `CWDIS:/\r` —
    // any real fallback (`CWDIS:/Users/...`) does not match this.
    assert!(
        !snap.contains("CWDIS:/\r") && !snap.trim_end().ends_with("CWDIS:/"),
        "child must NEVER run at filesystem root; got: {snap:?}"
    );

    sup.kill();
}

// Gap #6 e2e (fast path unchanged): an EXISTING worktree dir IS the child's cwd.
#[test]
fn spawn_runs_child_in_the_given_worktree_dir() {
    let dir = std::env::temp_dir().join(format!("at-sup-cwdok-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let spec = WorkspaceSpec {
        id: "wscwdok".into(),
        harness: Harness::Bash,
        worktree: dir.clone(),
        session_id: None,
        resume: false,
        role: None,
        is_worker: false,
        extra_dirs: vec![],
        model: None,
    };
    let hooks = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../hooks");
    let state = std::env::temp_dir().join("at-sup-state");
    let sidecar = PathBuf::from("/unused/agent-teams-mcp");

    let mut sup = Supervisor::spawn(&spec, &hooks, &state, &sidecar).unwrap();
    sup.write(b"printf 'CWD%sIS:%s\\n' '' \"$PWD\"\n").unwrap();

    // match on the unique BASENAME (macOS tempdirs canonicalize /var → /private/var,
    // so a full-path compare would be flaky; the nonce'd basename is unambiguous).
    let base = dir.file_name().unwrap().to_string_lossy().to_string();
    let start = Instant::now();
    let mut ok = false;
    while start.elapsed() < Duration::from_secs(5) {
        let snap = sup.snapshot();
        if snap.contains("CWDIS:") {
            ok = snap.contains(&base);
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        ok,
        "child cwd must be the worktree dir ({base}); got: {:?}",
        sup.snapshot()
    );

    sup.kill();
    let _ = std::fs::remove_dir_all(&dir);
}

// kill() must REAP the child (kill + wait), not just signal it — otherwise every closed
// pane leaves a kernel ZOMBIE until the app process exits (nothing polls is_alive on a
// pane that has left the registry).
#[test]
fn kill_reaps_the_child_no_zombie_left() {
    let dir = std::env::temp_dir().join(format!("at-sup-reap-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let spec = WorkspaceSpec {
        id: "wsreap".into(),
        harness: Harness::Bash,
        worktree: dir.clone(),
        session_id: None,
        resume: false,
        role: None,
        is_worker: false,
        extra_dirs: vec![],
        model: None,
    };
    let hooks = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../hooks");
    let state = std::env::temp_dir().join("at-sup-state");
    let sidecar = PathBuf::from("/unused/agent-teams-mcp");

    let mut sup = Supervisor::spawn(&spec, &hooks, &state, &sidecar).unwrap();
    let pid = sup.child_pid().expect("live child pid");
    sup.kill();
    // After kill() returns the child must be REAPED: ps shows NOTHING for the pid (fully
    // gone) — a zombie would still be listed with stat "Z…".
    let out = std::process::Command::new("/bin/ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    let stat = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert!(
        !stat.starts_with('Z'),
        "child must not remain a zombie after kill(); ps stat = {stat:?}"
    );
    assert!(!sup.is_alive(), "killed child is not alive");
    let _ = std::fs::remove_dir_all(&dir);
}

// Sound-guard regression: a PLAIN (non-worktree) dir squatting the worktree path
// must NOT be "reused". The old guard (root.exists() && is-inside-work-tree, which is
// true for ANY dir under the main checkout) returned a Worktree rooted at the MAIN
// checkout → a pane editing main. add_worktree must fail loud instead.
#[test]
fn add_worktree_rejects_a_plain_dir_squatting_the_path() {
    let repo = std::env::temp_dir().join(format!("at-sup-squat-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&repo);
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    std::fs::write(repo.join("README.md"), "x").unwrap();
    git(&repo, &["add", "."]);
    git(
        &repo,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "-m",
            "init",
        ],
    );

    // a plain dir (no .git, not a registered worktree) squatting the worktree path
    let squat = repo.join(".agent-teams-worktrees").join("wsx");
    std::fs::create_dir_all(&squat).unwrap();
    std::fs::write(squat.join("junk.txt"), "not a worktree").unwrap();

    let r = add_worktree(&repo, "wsx");
    assert!(
        r.is_err(),
        "must reject a plain squatting dir, not mis-reuse it (the old guard returned a main-rooted worktree)"
    );

    let _ = std::fs::remove_dir_all(&repo);
}
