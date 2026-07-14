//! PR step — EXTRACTED (Phase 0/P1 seam) from agent-teams `app/src-tauri/src/lib.rs` (~L9815).
//!
//! `flywheel_push_and_pr` pushes the integration branch and opens a PR via `gh` — it NEVER
//! merges (the human reviews + merges). Fully self-contained (git + gh subprocess), zero
//! AppState coupling in the source.
//!
//! Deviations: `supervisor::harness_path()` → inlined `harness_path()`; `git_capture` shells `git`.
//! Both now live in `crate::gitutil` (de-duped across orchestrate/synthesize/flywheel/apply).

use std::path::Path;

use crate::gitutil::{git_capture, harness_path};

/// Push the integration branch + open a PR (base `main`, head `branch`, body = the synthesized
/// report). NEVER merges. Returns the PR url. Idempotent within a run (reuses an open PR for the
/// same branch). Refuses if there is no `origin` remote.
pub fn flywheel_push_and_pr(
    repo: &Path,
    integ_wt: &Path,
    branch: &str,
    body_path: &Path,
    title: &str,
) -> Result<String, String> {
    // Trusted-repo / sanity: a real `origin` must exist, else there is nothing to push to.
    git_capture(repo, &["remote", "get-url", "origin"])
        .filter(|s| !s.is_empty())
        .ok_or("no 'origin' remote — push/PR refused")?;
    // Push ONLY the integration branch, explicit refspec, NEVER force. `HEAD:refs/heads/<branch>`
    // creates the remote branch from HEAD regardless of the local branch name (correct for BOTH a
    // loop run on `agent-teams/bridge-integ-<run>` shipping a `loop/<id>/<base>` name AND manual).
    let refspec = format!("HEAD:refs/heads/{branch}");
    let push = std::process::Command::new("git")
        .arg("-C")
        .arg(integ_wt)
        .args(["push", "origin", &refspec])
        .env("PATH", harness_path())
        .output()
        .map_err(|e| format!("git push spawn failed: {e}"))?;
    if !push.status.success() {
        return Err(format!(
            "git push failed: {}",
            String::from_utf8_lossy(&push.stderr).trim()
        ));
    }
    // Idempotency (within-run): an existing open PR for THIS branch → reuse it, never re-create.
    // `env_remove("GITHUB_TOKEN")`: a GH_TOKEN/GITHUB_TOKEN env var SHADOWS gh keyring scope and
    // breaks auto-PR — let `gh` use its keyring auth.
    if let Ok(list) = std::process::Command::new("gh")
        .args([
            "pr", "list", "--head", branch, "--base", "main", "--state", "open", "--json", "url",
            "--jq", ".[0].url",
        ])
        .current_dir(repo)
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_TOKEN") // GH_TOKEN outranks GITHUB_TOKEN for gh — scrub both (mirror worker.rs)
        .env("PATH", harness_path())
        .output()
    {
        if list.status.success() {
            let url = String::from_utf8_lossy(&list.stdout).trim().to_string();
            if url.starts_with("http") {
                return Ok(url);
            }
        }
    }
    // Create the PR — base main, head the integration branch, body = the synthesized report.
    let create = std::process::Command::new("gh")
        .args([
            "pr",
            "create",
            "--base",
            "main",
            "--head",
            branch,
            "--title",
            title,
            "--body-file",
        ])
        .arg(body_path)
        .current_dir(repo)
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_TOKEN") // GH_TOKEN outranks GITHUB_TOKEN for gh — scrub both (mirror worker.rs)
        .env("PATH", harness_path())
        .output()
        .map_err(|e| format!("gh pr create spawn failed: {e}"))?;
    if !create.status.success() {
        return Err(format!(
            "gh pr create failed: {}",
            String::from_utf8_lossy(&create.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&create.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // The `origin` guard fires before any network/gh: a non-git dir has no `origin` remote →
    // the call is refused with no push/PR attempted. Deterministic (no network, no gh).
    #[test]
    fn refuses_when_no_origin_remote() {
        let root =
            std::env::temp_dir().join(format!("ade-flywheel-noorigin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let body = root.join("body.md");
        std::fs::write(&body, "report").unwrap();
        let err = flywheel_push_and_pr(&root, &root, "feat/x", &body, "title").unwrap_err();
        assert!(
            err.contains("no 'origin' remote"),
            "non-git dir → refused, got: {err}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
