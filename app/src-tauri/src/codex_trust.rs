//! Codex CLI trust-sync — ensures every git worktree path managed by this app is
//! registered as a trusted project in `~/.codex/config.toml`, so Codex CLI uses the
//! same settings (sandbox mode, model provider, approval policy) in worktrees as in
//! the main repo.
//!
//! ## Why this exists
//!
//! Codex CLI matches project trust by **directory path**. The main repo checkout is
//! trusted, but `git worktree add` creates linked checkouts at completely different
//! paths (e.g. `~/at-w4b`, `<repo>/.agent-teams-worktrees/ws…-p0`). Without an entry
//! in `~/.codex/config.toml` for each worktree path, Codex treats them as untrusted —
//! different sandbox mode, different approval behaviour, different settings.
//!
//! ## Design
//!
//! - **Append-only, never rewrite.** We read the config, check if a path is already
//!   present, and append a `[projects."<path>"]` block if missing. We never parse and
//!   re-serialize the TOML (that would lose comments, formatting, and ordering).
//! - **Fail-soft.** Every public function returns `Result` but callers treat failure
//!   as a warning, never a fatal error. A missing `~/.codex/` or unreadable config
//!   means the user hasn't set up Codex CLI yet — nothing to sync.
//! - **Idempotent.** Safe to call on every startup and every worktree creation.

use std::path::{Path, PathBuf};

/// Returns `~/.codex/config.toml` (the Codex CLI global config).
pub fn codex_config_path() -> Option<PathBuf> {
    dirs_home().map(|h| h.join(".codex").join("config.toml"))
}

/// Cross-platform home dir without pulling in the `dirs` crate.
fn dirs_home() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| std::env::var("USERPROFILE").ok().map(PathBuf::from))
}

/// Core: ensure a single directory path is registered as trusted in the given
/// config file. Returns `Ok(true)` if a new entry was appended, `Ok(false)` if
/// already present or config doesn't exist.
fn ensure_trusted_in(config: &Path, path: &Path) -> std::io::Result<bool> {
    if !config.is_file() {
        return Ok(false); // no config yet — nothing to sync
    }
    let content = std::fs::read_to_string(config)?;
    let path_str = path.to_string_lossy();
    let needle = format!("[projects.\"{}\"]", path_str);
    if content.contains(&needle) {
        return Ok(false); // already trusted
    }
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(config)?;
    use std::io::Write;
    writeln!(f)?;
    writeln!(f, "[projects.\"{}\"]", path_str)?;
    writeln!(f, "trust_level = \"trusted\"")?;
    Ok(true)
}

/// Public wrapper: ensure a path is trusted in `~/.codex/config.toml`.
pub fn ensure_trusted(path: &Path) -> std::io::Result<bool> {
    match codex_config_path() {
        Some(config) => ensure_trusted_in(&config, path),
        None => Ok(false),
    }
}

/// Read all worktree paths from `git worktree list --porcelain` for the given
/// repo root and register each as trusted. Returns the count of newly added
/// paths.
pub fn sync_git_worktrees(repo_root: &Path) -> std::io::Result<u32> {
    let config = match codex_config_path() {
        Some(c) => c,
        None => return Ok(0),
    };
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["worktree", "list", "--porcelain"])
        .output()?;
    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "git worktree list failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut added = 0u32;
    for line in stdout.lines() {
        if let Some(path_str) = line.strip_prefix("worktree ") {
            let path = Path::new(path_str.trim());
            if path.is_dir() && ensure_trusted_in(&config, path)? {
                added += 1;
            }
        }
    }
    Ok(added)
}

/// Read the app's persistent worktree registry (JSONL) and register each
/// recorded worktree path as trusted. Returns the count of newly added paths.
pub fn sync_registry_worktrees(registry: &Path) -> std::io::Result<u32> {
    let config = match codex_config_path() {
        Some(c) => c,
        None => return Ok(0),
    };
    if !registry.is_file() {
        return Ok(0);
    }
    let content = std::fs::read_to_string(registry)?;
    let mut added = 0u32;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(wt) = val.get("worktree").and_then(|v| v.as_str()) {
                let path = Path::new(wt);
                if path.is_dir() && ensure_trusted_in(&config, path)? {
                    added += 1;
                }
            }
        }
    }
    Ok(added)
}

/// Full sync: git worktrees + registry worktrees. Returns total newly added
/// count. Fail-soft: logs warnings but never panics.
#[allow(dead_code)] // public utility — used by the shell script equivalent + future callers
pub fn sync_all(repo_root: &Path, registry: &Path) -> u32 {
    let mut total = 0u32;
    match sync_git_worktrees(repo_root) {
        Ok(n) => total += n,
        Err(e) => eprintln!("[codex-trust] git worktree sync warning: {e}"),
    }
    match sync_registry_worktrees(registry) {
        Ok(n) => total += n,
        Err(e) => eprintln!("[codex-trust] registry sync warning: {e}"),
    }
    if total > 0 {
        eprintln!("[codex-trust] registered {total} new worktree path(s) as trusted in ~/.codex/config.toml");
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-rolled scratch dir (matches core/task + core/agent pattern — no tempfile dep).
    struct Scratch {
        dir: PathBuf,
    }
    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "codex-trust-test-{}-{tag}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self { dir }
        }
        fn config(&self) -> PathBuf {
            self.dir.join("config.toml")
        }
        fn write_config(&self, content: &str) {
            std::fs::write(self.config(), content).unwrap();
        }
        fn read_config(&self) -> String {
            std::fs::read_to_string(self.config()).unwrap()
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn ensure_trusted_appends_missing_path() {
        let s = Scratch::new("append");
        s.write_config("[projects.\"/existing/repo\"]\ntrust_level = \"trusted\"\n");
        let added = ensure_trusted_in(&s.config(), Path::new("/new/worktree")).unwrap();
        assert!(added);
        let content = s.read_config();
        assert!(content.contains("[projects.\"/new/worktree\"]"));
        assert!(content.contains("[projects.\"/existing/repo\"]"));
    }

    #[test]
    fn ensure_trusted_is_idempotent() {
        let s = Scratch::new("idempotent");
        s.write_config("[projects.\"/already/trusted\"]\ntrust_level = \"trusted\"\n");
        let added = ensure_trusted_in(&s.config(), Path::new("/already/trusted")).unwrap();
        assert!(!added);
    }

    #[test]
    fn ensure_trusted_noop_when_config_absent() {
        let s = Scratch::new("absent");
        // config file doesn't exist
        let added = ensure_trusted_in(&s.config(), Path::new("/some/path")).unwrap();
        assert!(!added);
    }

    #[test]
    fn sync_registry_reads_jsonl_lines() {
        let s = Scratch::new("registry");
        s.write_config("");
        let wt1 = s.dir.join("wt1");
        let wt2 = s.dir.join("wt2");
        std::fs::create_dir_all(&wt1).unwrap();
        std::fs::create_dir_all(&wt2).unwrap();
        let registry = s.dir.join("registry.json");
        let mut f = std::fs::File::create(&registry).unwrap();
        use std::io::Write;
        writeln!(
            f,
            r#"{{"id":"ws1","repo":"/repo","worktree":"{}"}}"#,
            wt1.display()
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"id":"ws2","repo":"/repo","worktree":"{}"}}"#,
            wt2.display()
        )
        .unwrap();
        writeln!(f, "not json").unwrap();
        // Use the internal function directly with our scratch config.
        let content = std::fs::read_to_string(&registry).unwrap();
        let mut added = 0u32;
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
                if let Some(wt) = val.get("worktree").and_then(|v| v.as_str()) {
                    let path = Path::new(wt);
                    if path.is_dir() && ensure_trusted_in(&s.config(), path).unwrap() {
                        added += 1;
                    }
                }
            }
        }
        assert_eq!(added, 2);
        let config_content = s.read_config();
        assert!(config_content.contains(&format!("[projects.\"{}\"]", wt1.display())));
        assert!(config_content.contains(&format!("[projects.\"{}\"]", wt2.display())));
    }
}
