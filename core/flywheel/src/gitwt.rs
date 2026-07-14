//! Shared git-worktree primitives — single home for the de-duped Worktree/add_worktree/remove_worktree
//! (was duplicated in runctx.rs + synthesize::fold_support).

use std::path::{Path, PathBuf};

/// An isolated git worktree off a parent repo.
#[allow(dead_code)] // `branch` is recorded for parity with the source; not read by current callers.
pub(crate) struct Worktree {
    pub cwd: PathBuf,
    pub root: PathBuf,
    pub git_root: PathBuf,
    pub branch: String,
}

fn git_out(dir: &Path, args: &[&str]) -> std::io::Result<std::process::Output> {
    std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
}
fn io_err<E: std::fmt::Display>(e: E) -> std::io::Error {
    std::io::Error::other(e.to_string())
}

/// Create an isolated git worktree for `selected` on branch `agent-teams/<id>` (no-checkout →
/// sparse/checkout). Mirror of `supervisor::add_worktree`.
pub(crate) fn add_worktree(selected: &Path, id: &str) -> std::io::Result<Worktree> {
    let branch = format!("agent-teams/{id}");
    let top = git_out(selected, &["rev-parse", "--show-toplevel"])?;
    if !top.status.success() {
        return Err(io_err(format!(
            "not a git repo: {}",
            String::from_utf8_lossy(&top.stderr)
        )));
    }
    let git_root = PathBuf::from(String::from_utf8_lossy(&top.stdout).trim().to_string());

    let canon_sel = std::fs::canonicalize(selected).unwrap_or_else(|_| selected.to_path_buf());
    let canon_root = std::fs::canonicalize(&git_root).unwrap_or_else(|_| git_root.clone());
    let subpath = canon_sel
        .strip_prefix(&canon_root)
        .map(|p| p.to_path_buf())
        .unwrap_or_default();

    let root = git_root.join(".agent-teams-worktrees").join(id);
    if root.exists() {
        let canon_wt = std::fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
        let is_genuine = git_out(&root, &["rev-parse", "--show-toplevel"])
            .ok()
            .filter(|o| o.status.success())
            .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim().to_string()))
            .and_then(|tl| std::fs::canonicalize(&tl).ok())
            .map(|tl| tl == canon_wt)
            .unwrap_or(false);
        if is_genuine {
            // GENUINE REUSE HYGIENE: a pre-existing worktree may carry a stale dirty tree from a
            // crashed prior run — adopting it as-is means the autocommit safety net ships those
            // leftovers. Scrub back to a pristine checkout of the repo's CURRENT HEAD:
            // reset --hard (index/tracked) + clean -fd (untracked) + checkout -B <branch> <HEAD>.
            let head = git_out(&git_root, &["rev-parse", "HEAD"])
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| io_err("worktree reuse: could not resolve repo HEAD"))?;
            let scrub = |args: &[&str]| -> std::io::Result<()> {
                let r = git_out(&root, args)?;
                if !r.status.success() {
                    return Err(io_err(format!(
                        "worktree reuse scrub failed (git {args:?}): {}",
                        String::from_utf8_lossy(&r.stderr)
                    )));
                }
                Ok(())
            };
            scrub(&["reset", "--hard", "--quiet"])?;
            scrub(&["clean", "-fdq"])?;
            scrub(&["checkout", "-q", "-B", &branch, &head])?;
            let cwd = if subpath.as_os_str().is_empty() {
                root.clone()
            } else {
                root.join(&subpath)
            };
            return Ok(Worktree {
                cwd,
                root,
                git_root,
                branch,
            });
        }
        return Err(io_err(format!(
            "worktree path occupied by a non-worktree dir: {} (remove it or use another id)",
            root.display()
        )));
    }
    if let Some(parent) = root.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let add = git_out(
        &git_root,
        &[
            "worktree",
            "add",
            "--no-checkout",
            "-B",
            &branch,
            &root.to_string_lossy(),
        ],
    )?;
    if !add.status.success() {
        if let Some(parent) = root.parent() {
            let _ = std::fs::remove_dir(parent);
        }
        return Err(io_err(format!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&add.stderr)
        )));
    }
    let run = |args: &[&str]| -> std::io::Result<()> {
        let r = git_out(&root, args)?;
        if !r.status.success() {
            let _ = remove_worktree(&git_root, id, &root);
            return Err(io_err(format!(
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&r.stderr)
            )));
        }
        Ok(())
    };
    let cwd = if subpath.as_os_str().is_empty() {
        run(&["checkout"])?;
        root.clone()
    } else {
        let sub = subpath.to_string_lossy().to_string();
        run(&["sparse-checkout", "init", "--cone"])?;
        run(&["sparse-checkout", "set", sub.as_str()])?;
        run(&["checkout"])?;
        root.join(&subpath)
    };
    Ok(Worktree {
        cwd,
        root,
        git_root,
        branch,
    })
}

/// Remove a worktree and its `agent-teams/<id>` branch.
///
/// # Why the status is checked (data-loss / orphan-leak fix)
/// The old body called `.output()?` but NEVER inspected `status.success()`, so a git
/// that *refused* to remove the worktree (it is `git worktree lock`ed, its dir is a
/// live process cwd, an admin record is wedged, …) still returned `Ok(())`. The
/// callers (`sweep_manifest`, `RunContext::drop`) treat `Ok` as "reclaimed" and then
/// delete the manifest/registry line — so a worktree git could NOT remove is FORGOTTEN
/// forever (leaked disk + a dangling `agent-teams/<id>` branch, with no record left to
/// reclaim it later).
///
/// Now: force-remove, and on failure `git worktree prune` (clears stale admin records
/// for dirs that are already gone) then retry once. Only if the worktree PATH still
/// exists after that do we return a real `Err`, so the caller can KEEP its tracking
/// record and retry on a later sweep. We deliberately do NOT escalate to a double
/// `--force` (which would blow past a deliberate `git worktree lock`); a genuinely
/// held worktree is preserved-and-reported, not nuked. Branch deletion is best-effort
/// (an orphan branch ref is a trivial leak next to a live worktree, and the branch may
/// be checked out elsewhere or already gone).
pub(crate) fn remove_worktree(repo: &Path, id: &str, path: &Path) -> std::io::Result<()> {
    let branch = format!("agent-teams/{id}");
    let path_str = path.to_string_lossy().to_string();

    let first = git_out(repo, &["worktree", "remove", "--force", &path_str])?;
    if !first.status.success() {
        // Stale admin record for an already-deleted dir → prune reconciles it. Then
        // retry the forced removal once.
        let _ = git_out(repo, &["worktree", "prune"]);
        let retry = git_out(repo, &["worktree", "remove", "--force", &path_str])?;
        if !retry.status.success() && path.exists() {
            // Genuinely un-removable (locked / held): report so the caller keeps the record.
            return Err(io_err(format!(
                "git worktree remove --force failed for {} (kept for later reclaim): {} | {}",
                path.display(),
                String::from_utf8_lossy(&first.stderr).trim(),
                String::from_utf8_lossy(&retry.stderr).trim(),
            )));
        }
    }
    // Best-effort branch cleanup (never fatal to the worktree reclaim).
    let _ = git_out(repo, &["branch", "-D", &branch]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_repo(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("ade-gitwt-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .current_dir(&root)
                .args(args)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?} failed");
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(root.join("README.md"), "seed\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "seed"]);
        root
    }

    // REUSE HYGIENE: a genuinely-reused worktree carrying stale dirty state (tracked edit +
    // untracked leftover + a stale commit on the branch) must come back PRISTINE at the repo's
    // current HEAD — else the apply autocommit safety net ships the leftovers.
    #[test]
    fn reused_worktree_is_scrubbed_to_pristine_current_head() {
        let repo = temp_repo("reuse");
        let wt1 = add_worktree(&repo, "wreuse").expect("first add");
        // simulate a crashed prior run: dirty tracked file + untracked leftover + a stale commit.
        std::fs::write(wt1.root.join("README.md"), "stale edit\n").unwrap();
        std::fs::write(wt1.root.join("LEFTOVER.txt"), "junk\n").unwrap();
        let g = |dir: &Path, args: &[&str]| {
            assert!(std::process::Command::new("git")
                .current_dir(dir)
                .args(args)
                .output()
                .unwrap()
                .status
                .success());
        };
        g(
            &wt1.root,
            &[
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "add",
                "README.md",
            ],
        );
        g(
            &wt1.root,
            &[
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-qm",
                "stale",
            ],
        );
        std::fs::write(wt1.root.join("DIRTY.txt"), "dirty\n").unwrap();

        // reuse the SAME id → must scrub, not adopt.
        let wt2 = add_worktree(&repo, "wreuse").expect("reuse add");
        assert_eq!(wt2.root, wt1.root);
        assert_eq!(
            std::fs::read_to_string(wt2.root.join("README.md")).unwrap(),
            "seed\n",
            "tracked edit reset"
        );
        assert!(
            !wt2.root.join("LEFTOVER.txt").exists(),
            "untracked leftover cleaned"
        );
        assert!(!wt2.root.join("DIRTY.txt").exists(), "dirty file cleaned");
        // HEAD == repo HEAD (stale commit discarded via checkout -B)
        let head = |dir: &Path| {
            let o = std::process::Command::new("git")
                .current_dir(dir)
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap();
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        };
        assert_eq!(
            head(&wt2.root),
            head(&repo),
            "reused worktree rebased onto current repo HEAD"
        );
        // status clean
        let st = std::process::Command::new("git")
            .current_dir(&wt2.root)
            .args(["status", "--porcelain"])
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&st.stdout).trim().is_empty(),
            "pristine after reuse"
        );
        let _ = std::fs::remove_dir_all(&repo);
    }

    /// R3 orphan-leak fix: a worktree git genuinely REFUSES to remove (here: a
    /// `git worktree lock`, which a single `--force` cannot override) must return
    /// `Err` — NOT a silent `Ok` that would let the caller delete the reclaim record
    /// and leak the worktree forever. After the failed removal the worktree must
    /// still exist (its record is preserved for a later sweep).
    #[test]
    fn locked_worktree_removal_returns_err_not_silent_ok() {
        let repo = temp_repo("locked");
        let wt = add_worktree(&repo, "wlock").expect("add");
        // Lock it: a single `git worktree remove --force` refuses a locked worktree.
        let lock = std::process::Command::new("git")
            .current_dir(&repo)
            .args(["worktree", "lock"])
            .arg(&wt.root)
            .output()
            .unwrap();
        assert!(lock.status.success(), "git worktree lock must succeed");

        let err = remove_worktree(&wt.git_root, "wlock", &wt.root)
            .expect_err("a locked worktree must NOT return silent Ok");
        assert!(
            err.to_string().contains("kept for later reclaim"),
            "error explains the record is preserved: {err}"
        );
        assert!(
            wt.root.exists(),
            "the un-removable worktree must still be on disk (record kept)"
        );

        // Cleanup: unlock + force-remove for a tidy temp dir.
        let _ = std::process::Command::new("git")
            .current_dir(&repo)
            .args(["worktree", "unlock"])
            .arg(&wt.root)
            .output();
        let _ = remove_worktree(&wt.git_root, "wlock", &wt.root);
        let _ = std::fs::remove_dir_all(&repo);
    }

    /// The happy path still works AND now returns a meaningful `Ok`: a normal
    /// worktree is removed, its dir gone, and the branch deleted.
    #[test]
    fn normal_worktree_removal_succeeds_and_cleans_branch() {
        let repo = temp_repo("normalrm");
        let wt = add_worktree(&repo, "wnorm").expect("add");
        assert!(wt.root.exists());
        remove_worktree(&wt.git_root, "wnorm", &wt.root).expect("normal removal must succeed");
        assert!(!wt.root.exists(), "worktree dir removed");
        let branches = std::process::Command::new("git")
            .current_dir(&repo)
            .args(["branch", "--list", "agent-teams/wnorm"])
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&branches.stdout).trim().is_empty(),
            "the agent-teams/<id> branch was deleted"
        );
        let _ = std::fs::remove_dir_all(&repo);
    }
}
