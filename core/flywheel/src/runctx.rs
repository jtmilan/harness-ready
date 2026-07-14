//! P1 — owned run state for the headless single-worker spine.
//!
//! This is the de-Tauri of the agent-teams `delegate_controller` (the design's "biggest risk").
//! It replaces the four AppState/AppHandle couplings with owned, GUI-free primitives:
//!
//! | Tauri primitive | CLI replacement (here) |
//! |---|---|
//! | `app: AppHandle` arg | owned [`RunContext`] |
//! | `delegate_cancel: Arc<AtomicBool>` (SeqCst kill-switch) | [`RunContext::cancel`] (`Arc<AtomicBool>`, SeqCst — verbatim semantics) |
//! | `headless_workers`/`worker_registry` + `DelegateGuard` drop | [`RunContext`]'s `Drop` (RAII): kill pgid → wait → remove worktree → unlink manifest |
//! | `AppHandle::emit` | [`DynEmitter`] (stdout default) |
//!
//! **Biggest risk = `Drop` correctness.** Mitigations applied: (1) the on-disk worktree manifest
//! is written BEFORE the in-memory worker push, so a crash between worktree-create and register
//! still leaves a sweepable record ([`sweep_manifest`] reconciles it on the next run); (2) `Drop`
//! is *saturated* and fail-soft — every step logs + continues, never `?`, never panics the sweep;
//! (3) the kill targets the whole process GROUP (`/bin/kill -9 -<pgid>`, the worker is a group
//! leader via `process_group(0)`) so forked grandchildren are reaped, matching the source §9.4.
//!
//! `spawn_worker` takes a caller-built `Command` (the production claude/cursor/codex invocation, OR
//! a mock `sh` in tests) so the keystone is testable WITHOUT a live harness.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

// ===================== emitter (replaces AppHandle::emit) =====================

/// Where run/worker progress goes. The GUI routes these to the webview; the CLI prints them.
pub trait DynEmitter: Send {
    fn emit_phase(&mut self, run_id: &str, phase: &str);
    fn emit_worker(&mut self, run_id: &str, wid: &str, event: &str);
    fn log(&mut self, msg: &str);
    /// A user-facing progress NOTICE (vs `log`, which is a diagnostic). Hosts with a human-visible
    /// surface (the GUI's parent PTY pane) override this to route there so the operator sees it; the
    /// default forwards to `log`, so non-GUI hosts (the CLI's `StderrEmitter`) keep stderr behavior.
    fn notice(&mut self, msg: &str) {
        self.log(msg);
    }
}

/// Default CLI emitter — everything to stderr (stdout stays reserved for machine output / verdict).
pub struct StderrEmitter;
impl DynEmitter for StderrEmitter {
    fn emit_phase(&mut self, run_id: &str, phase: &str) {
        eprintln!("[{run_id}] phase: {phase}");
    }
    fn emit_worker(&mut self, run_id: &str, wid: &str, event: &str) {
        eprintln!("[{run_id}] worker {wid}: {event}");
    }
    fn log(&mut self, msg: &str) {
        eprintln!("{msg}");
    }
}

// ===================== production worktree helpers =====================
// Worktree/add_worktree/remove_worktree now live in `crate::gitwt` (de-duped vs synthesize::fold_support).
use crate::gitwt::{add_worktree, remove_worktree};

// ===================== owned run state =====================

/// One live worker the controller owns end-to-end (replaces `HeadlessWorker` in `AppState`).
struct OwnedWorker {
    id: String,
    child: std::process::Child,
    git_root: PathBuf,
    worktree: PathBuf,
    pgid: u32,
    /// The worker's stream log (if any), so a progress-aware settle can detect "still streaming"
    /// from its mtime. `None` when no log was requested (mock workers / silent harnesses).
    log_path: Option<PathBuf>,
}

/// Owns 100% of a delegate run's mutable state. Drop reaps every worker + worktree.
pub struct RunContext {
    run_id: String,
    cancel: Arc<AtomicBool>,
    workers: Vec<OwnedWorker>,
    /// Durable sweep list: every worktree path, written BEFORE the in-memory push.
    manifest: PathBuf,
    emitter: Box<dyn DynEmitter>,
    repo_root: PathBuf,
}

/// Tunables for [`RunContext::settle_streaming`] — the progress-aware settle that mirrors the GUI
/// delegate controller's deadline policy (base wait, extended while a worker is still streaming,
/// bounded by an absolute backstop). All durations; `poll` is the loop cadence + kill-switch check.
#[derive(Clone, Copy, Debug)]
pub struct SettleStreaming {
    /// Base wait before the first kill decision (a SILENT hang dies here).
    pub base: Duration,
    /// Granted per extension when a worker is still streaming at the deadline.
    pub extend: Duration,
    /// Absolute backstop — no settle waits past this, streaming or not.
    pub hard_cap: Duration,
    /// A worker counts as "streaming" if its log mtime is within this window.
    pub stream_window: Duration,
    /// Poll cadence (also the cancel-flag check interval).
    pub poll: Duration,
}

impl RunContext {
    /// `repo_root` = the target repo; `run_id` labels the run. The manifest lives under the repo's
    /// `.agent-teams-worktrees/<run_id>.manifest` so a startup [`sweep_manifest`] can find it.
    pub fn new(
        run_id: impl Into<String>,
        repo_root: impl Into<PathBuf>,
        emitter: Box<dyn DynEmitter>,
    ) -> Self {
        let run_id = run_id.into();
        let repo_root = repo_root.into();
        let manifest = repo_root
            .join(".agent-teams-worktrees")
            .join(format!("{run_id}.manifest"));
        RunContext {
            run_id,
            cancel: Arc::new(AtomicBool::new(false)),
            workers: Vec::new(),
            manifest,
            emitter,
            repo_root,
        }
    }

    /// Like [`new`](Self::new) but adopts a CALLER-owned cancel flag instead of minting one — so a
    /// host (e.g. the GUI's `AppState.delegate_cancel`, flipped by a `delegate_stop` command) and
    /// this context share ONE kill-switch. `cancel_handle()` then returns a clone of that same Arc.
    pub fn new_with_cancel(
        run_id: impl Into<String>,
        repo_root: impl Into<PathBuf>,
        emitter: Box<dyn DynEmitter>,
        cancel: Arc<AtomicBool>,
    ) -> Self {
        let mut ctx = Self::new(run_id, repo_root, emitter);
        ctx.cancel = cancel;
        ctx
    }

    /// A clone of the kill-switch the caller can flip (SIGINT handler, deadline watchdog).
    pub fn cancel_handle(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// Spawn ONE headless worker. The caller supplies a `build` closure that — given the worktree
    /// checkout dir — returns the per-harness invocation as a [`crate::worker::WorkerSpec`] (program/
    /// args/env, or a mock). We create the isolated worktree, build the spec (so opencode's `--dir`
    /// is baked against the real cwd), append the prompt positionally for non-stdin harnesses, set
    /// cwd, make it a process-group leader, drain stdout/stderr to `log_path` (if any), record it in
    /// the manifest BEFORE the in-memory push, and (for claude / `prompt_via_stdin`) deliver
    /// `prompt_stdin` on stdin. Returns the worker id.
    pub fn spawn_worker(
        &mut self,
        wid: &str,
        build: impl FnOnce(&Path) -> crate::worker::WorkerSpec,
        prompt_stdin: Option<&str>,
        log_path: Option<PathBuf>,
    ) -> Result<(), String> {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::process::CommandExt;
        use std::process::Stdio;

        let wt = add_worktree(&self.repo_root, wid)
            .map_err(|e| format!("worktree create failed: {e}"))?;

        // MANIFEST-FIRST: record the worktree path before anything that could crash, so a mid-spawn
        // failure leaves a record sweep_manifest can reconcile (the Drop-correctness mitigation).
        self.record_manifest(&wt.root);

        // Build the per-harness spec AGAINST the real worktree cwd (opencode's `--dir <cwd>` is baked
        // here). claude → prompt on stdin; cursor/codex/opencode → prompt appended as the trailing
        // positional arg below (before the cwd/stdio block).
        let crate::worker::WorkerSpec {
            mut cmd,
            prompt_via_stdin,
            end_of_options_supported,
        } = build(&wt.cwd);
        if !prompt_via_stdin {
            if let Some(p) = prompt_stdin {
                if p.starts_with('-') {
                    // A prompt that begins with `-`/`--` would be mis-parsed as a flag by the
                    // non-claude CLIs (commander.js/clap) → the task is silently dropped. Only pass
                    // it positionally behind a `--` end-of-options marker when the harness is
                    // live-verified to honor `--`; otherwise REJECT (no harness is verified today).
                    if end_of_options_supported {
                        cmd.arg("--");
                        cmd.arg(p);
                    } else {
                        // Same cleanup as the spawn-fail path below: remove the worktree + branch and
                        // unrecord the manifest so a rejected spawn leaves no orphan for the sweep.
                        let _ = remove_worktree(&wt.git_root, wid, &wt.root);
                        self.unrecord_manifest(&wt.root);
                        return Err(format!(
                            "prompt begins with '-' and harness has no verified end-of-options marker (would be mis-parsed as a flag): {:?}",
                            p.chars().take(48).collect::<String>()
                        ));
                    }
                } else {
                    cmd.arg(p);
                }
            }
        }

        cmd.current_dir(&wt.cwd)
            // Resolve harness binaries via the parity'd PATH (login-shell + Homebrew/cargo/local-bin),
            // NOT a bare inherited PATH — a GUI host launched from the macOS dock has a minimal launchd
            // PATH that would not find claude/git/gh/cursor (the same fix synthesize's escalation needs).
            .env("PATH", crate::gitutil::harness_path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // §9.4: process-GROUP leader (pgid == pid) so the kill-switch reaps the whole tree.
        cmd.process_group(0);

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let _ = remove_worktree(&wt.git_root, wid, &wt.root);
                self.unrecord_manifest(&wt.root);
                return Err(format!("worker spawn failed: {e}"));
            }
        };
        let pgid = child.id(); // process_group(0) → child IS the group leader → pgid == pid

        // Drain stdout/stderr on reader threads FIRST (deadlock-proof) — to a log file if given.
        let drain = |stream: Option<Box<dyn std::io::Read + Send>>,
                     lp: Option<PathBuf>,
                     prefix: &'static str| {
            if let Some(s) = stream {
                std::thread::spawn(move || {
                    // 0600: worker stdout/stderr can carry secrets — never world-readable.
                    use std::os::unix::fs::OpenOptionsExt;
                    let mut logf = lp.and_then(|p| {
                        std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .mode(0o600)
                            .open(p)
                            .ok()
                    });
                    for line in BufReader::new(s).lines().map_while(Result::ok) {
                        if let Some(f) = logf.as_mut() {
                            let _ = writeln!(f, "{prefix}{line}");
                        }
                    }
                });
            }
        };
        let worker_log = log_path.clone();
        drain(
            child
                .stdout
                .take()
                .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
            log_path.clone(),
            "",
        );
        drain(
            child
                .stderr
                .take()
                .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
            log_path,
            "[stderr] ",
        );

        // Prompt delivery: claude (prompt_via_stdin) reads it on stdin; non-stdin harnesses already
        // got it as a trailing positional above. Close stdin UNCONDITIONALLY (EOF) so a child that
        // waits on stdin can't hang — but only WRITE the prompt for the stdin harnesses. Readers
        // above are already draining → no deadlock.
        if let Some(mut stdin) = child.stdin.take() {
            if prompt_via_stdin {
                if let Some(p) = prompt_stdin {
                    let _ = stdin.write_all(p.as_bytes());
                    let _ = stdin.flush();
                }
            }
        }

        self.emitter.emit_worker(&self.run_id, wid, "spawned");
        self.workers.push(OwnedWorker {
            id: wid.to_string(),
            child,
            git_root: wt.git_root,
            worktree: wt.root,
            pgid,
            log_path: worker_log,
        });
        Ok(())
    }

    /// Wait for every worker to exit, or until `timeout`, or until cancelled (cancel → kill all).
    /// Returns (wid, exited-cleanly?) per worker. Workers stay owned for `Drop` to clean worktrees.
    pub fn settle(&mut self, timeout: Duration) -> Vec<(String, bool)> {
        let deadline = Instant::now() + timeout;
        let mut done: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
        loop {
            if self.is_cancelled() {
                self.emitter.log("[settle] cancelled — killing workers");
                for w in &self.workers {
                    kill_group(w.pgid);
                }
                break;
            }
            let mut all_exited = true;
            for w in &mut self.workers {
                if done.contains_key(&w.id) {
                    continue;
                }
                match w.child.try_wait() {
                    Ok(Some(st)) => {
                        done.insert(w.id.clone(), st.success());
                    }
                    Ok(None) => all_exited = false,
                    Err(_) => {
                        done.insert(w.id.clone(), false);
                    }
                }
            }
            if all_exited || Instant::now() >= deadline {
                if !all_exited {
                    self.emitter.log("[settle] deadline — killing stragglers");
                    for w in &self.workers {
                        if !done.contains_key(&w.id) {
                            kill_group(w.pgid);
                        }
                    }
                }
                break;
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        self.workers
            .iter()
            .map(|w| (w.id.clone(), *done.get(&w.id).unwrap_or(&false)))
            .collect()
    }

    /// Progress-aware settle (the GUI delegate-controller policy, faithfully): a worker is DONE when
    /// `report_done(wid)` is true (its fan-in report carries the completion sentinel) OR its child
    /// exits. Waits `opts.base`; while any not-yet-done worker is still STREAMING (its log mtime is
    /// within `opts.stream_window`) the deadline is extended by `opts.extend`, bounded by the absolute
    /// `opts.hard_cap` backstop — so a worker actively producing output is never killed mid-work, while
    /// a SILENT hang still dies at `base`. Cancel → kill all. Returns (wid, settled?) — `false` only
    /// for a straggler killed at the cap. `report_done` is a caller closure (RunContext is agnostic to
    /// the fan-in report path convention, e.g. `<run_dir>/<wid>.md`).
    ///
    /// `should_cancel` is an EXTRA caller predicate OR-ed into the per-tick kill check alongside the
    /// shared `delegate_cancel` flag — the GUI passes its `autonomy_ceiling → L0` re-read here so the
    /// FR-14 autonomy halt stops in-flight workers mid-settle (not just at the settle boundary). The
    /// CLI passes `|| false`.
    pub fn settle_streaming(
        &mut self,
        opts: SettleStreaming,
        report_done: impl Fn(&str) -> bool,
        should_cancel: impl Fn() -> bool,
    ) -> Vec<(String, bool)> {
        let start = Instant::now();
        let mut deadline = start + opts.base;
        let cap = start + opts.hard_cap;
        let mut done: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
        loop {
            if self.is_cancelled() || should_cancel() {
                self.emitter.log("[settle] cancelled — killing workers");
                for w in &self.workers {
                    kill_group(w.pgid);
                }
                break;
            }
            // A worker settles on its report sentinel OR a clean child exit.
            for w in &mut self.workers {
                if done.contains_key(&w.id) {
                    continue;
                }
                if report_done(&w.id) {
                    done.insert(w.id.clone(), true);
                    continue;
                }
                match w.child.try_wait() {
                    Ok(Some(st)) => {
                        done.insert(w.id.clone(), st.success());
                    }
                    Ok(None) => {}
                    Err(_) => {
                        done.insert(w.id.clone(), false);
                    }
                }
            }
            let all_done = self.workers.iter().all(|w| done.contains_key(&w.id));
            if all_done {
                break;
            }
            let now = Instant::now();
            if now >= deadline {
                // Progress-aware extension: any not-yet-done worker still streaming → grant more
                // time, bounded by the hard cap. Else kill the stragglers and stop.
                let streaming = self.workers.iter().any(|w| {
                    !done.contains_key(&w.id) && Self::worker_streaming(w, opts.stream_window)
                });
                if streaming && now < cap {
                    deadline = (now + opts.extend).min(cap);
                    // User-facing NOTICE (→ parent pane in the GUI), restoring the pre-RunContext
                    // "deadline reached — extending while progress continues" line the operator saw.
                    self.emitter.notice(&format!(
                        "[delegate] {}m deadline reached but worker(s) still streaming — extending while progress continues (hard cap {}m)",
                        opts.base.as_secs() / 60,
                        opts.hard_cap.as_secs() / 60,
                    ));
                } else {
                    if now >= cap {
                        self.emitter.log("[settle] hard cap — killing stragglers");
                    } else {
                        self.emitter.log("[settle] deadline — killing stragglers");
                    }
                    for w in &self.workers {
                        if !done.contains_key(&w.id) {
                            kill_group(w.pgid);
                            done.insert(w.id.clone(), false);
                        }
                    }
                    break;
                }
            }
            std::thread::sleep(opts.poll);
        }
        self.workers
            .iter()
            .map(|w| (w.id.clone(), *done.get(&w.id).unwrap_or(&false)))
            .collect()
    }

    /// A worker counts as "streaming" if its stream log was modified within `window`. Pure FS read.
    fn worker_streaming(w: &OwnedWorker, window: Duration) -> bool {
        let Some(lp) = w.log_path.as_ref() else {
            return false;
        };
        std::fs::metadata(lp)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| std::time::SystemTime::now().duration_since(t).ok())
            .map(|age| age < window)
            .unwrap_or(false)
    }

    /// The worktree path of worker `wid` (callers fold / read the diff from here).
    pub fn worktree_of(&self, wid: &str) -> Option<&Path> {
        self.workers
            .iter()
            .find(|w| w.id == wid)
            .map(|w| w.worktree.as_path())
    }

    fn record_manifest(&self, wt: &Path) {
        if let Some(parent) = self.manifest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.manifest)
        {
            use std::io::Write;
            let _ = writeln!(f, "{}", wt.display());
        }
    }
    fn unrecord_manifest(&self, wt: &Path) {
        if let Ok(body) = std::fs::read_to_string(&self.manifest) {
            let kept: Vec<&str> = body
                .lines()
                .filter(|l| *l != wt.to_string_lossy())
                .collect();
            let _ = std::fs::write(&self.manifest, kept.join("\n"));
        }
    }
}

impl Drop for RunContext {
    fn drop(&mut self) {
        // SATURATED + FAIL-SOFT: every step logs + continues, never panics the sweep.
        let workers = std::mem::take(&mut self.workers);
        for mut w in workers {
            kill_group(w.pgid); // SIGKILL the whole group (forked grandchildren too)
            let _ = w.child.wait();
            if let Err(e) = remove_worktree(&w.git_root, &w.id, &w.worktree) {
                self.emitter
                    .log(&format!("[drop] remove_worktree {} failed: {e}", w.id));
            }
        }
        if let Err(e) = std::fs::remove_file(&self.manifest) {
            if e.kind() != std::io::ErrorKind::NotFound {
                self.emitter
                    .log(&format!("[drop] manifest unlink failed: {e}"));
            }
        }
        if !self.is_cancelled() {
            self.emitter.emit_phase(&self.run_id, "done");
        }
    }
}

/// SIGKILL a whole process group (`/bin/kill -9 -<pgid>`). Best-effort; a dead group is a no-op.
fn kill_group(pgid: u32) {
    let _ = std::process::Command::new("/bin/kill")
        .args(["-9", &format!("-{pgid}")])
        .output();
}

/// Startup reconciliation: remove any worktrees listed in a leftover manifest (a previous run that
/// crashed before Drop). Returns the count cleaned. The manifest is removed after.
pub fn sweep_manifest(repo_root: &Path, manifest: &Path) -> usize {
    let Ok(body) = std::fs::read_to_string(manifest) else {
        return 0;
    };
    let mut n = 0;
    for line in body.lines().filter(|l| !l.trim().is_empty()) {
        let wt = Path::new(line);
        // id = the worktree dir name (.agent-teams-worktrees/<id>)
        if let Some(id) = wt.file_name().and_then(|s| s.to_str()) {
            if remove_worktree(repo_root, id, wt).is_ok() {
                n += 1;
            }
        }
    }
    let _ = std::fs::remove_file(manifest);
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    // Build a real temp git repo with one commit (worktree add needs a base commit).
    fn temp_git_repo() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "ade-runctx-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
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
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(root.join("README.md"), "seed\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "seed"]);
        root
    }

    // THE KEYSTONE TEST: spawn a worker that commits in its isolated worktree, settle, then Drop
    // must remove the worktree + its branch + the manifest. Proves spawn→commit→RAII-cleanup.
    #[test]
    fn worker_commits_in_isolated_worktree_and_drop_cleans_everything() {
        let repo = temp_git_repo();
        let wid = "w0";
        let wt_path = repo.join(".agent-teams-worktrees").join(wid);
        let manifest;
        {
            let mut ctx = RunContext::new("runA", &repo, Box::new(StderrEmitter));
            manifest = repo.join(".agent-teams-worktrees").join("runA.manifest");

            // mock harness: write a file + commit, inside the worktree cwd.
            let mut cmd = std::process::Command::new("sh");
            cmd.args(["-c", "echo work > worker.txt && git -c user.email=w@w -c user.name=w add -A && git -c user.email=w@w -c user.name=w commit -q -m worker"]);
            ctx.spawn_worker(
                wid,
                |_wt| crate::worker::WorkerSpec {
                    cmd,
                    prompt_via_stdin: true,
                    end_of_options_supported: false,
                },
                None,
                None,
            )
            .expect("spawn");

            // manifest recorded the worktree BEFORE the in-memory push
            let mbody = std::fs::read_to_string(&manifest).unwrap();
            assert!(
                mbody.contains(wt_path.to_string_lossy().as_ref()),
                "manifest records the worktree"
            );

            let res = ctx.settle(Duration::from_secs(20));
            assert_eq!(res, vec![("w0".to_string(), true)], "worker exited cleanly");

            // the worker's commit landed on agent-teams/w0 in the worktree
            assert!(
                wt_path.join("worker.txt").exists(),
                "worker wrote into its worktree"
            );
            let log = std::process::Command::new("git")
                .current_dir(&wt_path)
                .args(["log", "-1", "--pretty=%s"])
                .output()
                .unwrap();
            assert_eq!(
                String::from_utf8_lossy(&log.stdout).trim(),
                "worker",
                "worker committed"
            );
        } // ← Drop runs here

        // Drop cleaned: worktree gone, branch gone, manifest gone.
        assert!(!wt_path.exists(), "Drop removed the worktree dir");
        assert!(!manifest.exists(), "Drop removed the manifest");
        let branches = std::process::Command::new("git")
            .current_dir(&repo)
            .args(["branch", "--list", "agent-teams/w0"])
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&branches.stdout).trim().is_empty(),
            "Drop deleted the worker branch"
        );

        let _ = std::fs::remove_dir_all(&repo);
    }

    // CANCEL/KILL path: a long-sleeping worker + cancel → settle kills it → Drop still cleans the
    // worktree (the kill is best-effort; the worktree removal is the invariant we assert).
    #[test]
    fn cancel_kills_worker_and_drop_still_cleans_worktree() {
        let repo = temp_git_repo();
        let wid = "w1";
        let wt_path = repo.join(".agent-teams-worktrees").join(wid);
        {
            let mut ctx = RunContext::new("runB", &repo, Box::new(StderrEmitter));
            let mut cmd = std::process::Command::new("sh");
            cmd.args(["-c", "sleep 60"]);
            ctx.spawn_worker(
                wid,
                |_wt| crate::worker::WorkerSpec {
                    cmd,
                    prompt_via_stdin: true,
                    end_of_options_supported: false,
                },
                None,
                None,
            )
            .expect("spawn");
            assert!(wt_path.exists(), "worktree created");
            ctx.cancel();
            let res = ctx.settle(Duration::from_secs(10));
            assert_eq!(
                res,
                vec![("w1".to_string(), false)],
                "cancelled worker did not exit cleanly"
            );
        } // ← Drop reaps the (killed) worker + worktree
        assert!(
            !wt_path.exists(),
            "Drop removed the worktree even after cancel/kill"
        );
        let _ = std::fs::remove_dir_all(&repo);
    }

    // settle_streaming: a worker whose fan-in report carries the sentinel settles IMMEDIATELY
    // (true), before its (still-running) child exits — proves the report-done short-circuit.
    #[test]
    fn settle_streaming_settles_on_report_sentinel_before_child_exit() {
        let repo = temp_git_repo();
        {
            let mut ctx = RunContext::new("runS1", &repo, Box::new(StderrEmitter));
            let mut cmd = std::process::Command::new("sh");
            cmd.args(["-c", "sleep 30"]); // silent + long-running; would never exit within the test
            ctx.spawn_worker(
                "w0",
                |_wt| crate::worker::WorkerSpec {
                    cmd,
                    prompt_via_stdin: true,
                    end_of_options_supported: false,
                },
                None,
                None,
            )
            .expect("spawn");
            let opts = SettleStreaming {
                base: Duration::from_secs(20),
                extend: Duration::from_secs(5),
                hard_cap: Duration::from_secs(30),
                stream_window: Duration::from_secs(5),
                poll: Duration::from_millis(50),
            };
            // report "done" immediately → must NOT wait on the sleeping child.
            let t0 = Instant::now();
            let res = ctx.settle_streaming(opts, |_wid| true, || false);
            assert_eq!(
                res,
                vec![("w0".to_string(), true)],
                "settled true on the report sentinel"
            );
            assert!(
                t0.elapsed() < Duration::from_secs(5),
                "short-circuited, did not wait the base"
            );
        }
        let _ = std::fs::remove_dir_all(&repo);
    }

    // settle_streaming: a SILENT worker (no report, no streaming log) is killed at the base deadline
    // (false) — the progress-aware extension must NOT save a worker producing nothing.
    #[test]
    fn settle_streaming_kills_silent_worker_at_base() {
        let repo = temp_git_repo();
        let wt_path = repo.join(".agent-teams-worktrees").join("w1");
        {
            let mut ctx = RunContext::new("runS2", &repo, Box::new(StderrEmitter));
            let mut cmd = std::process::Command::new("sh");
            cmd.args(["-c", "sleep 60"]);
            // no log_path → worker_streaming() is false → no extension.
            ctx.spawn_worker(
                "w1",
                |_wt| crate::worker::WorkerSpec {
                    cmd,
                    prompt_via_stdin: true,
                    end_of_options_supported: false,
                },
                None,
                None,
            )
            .expect("spawn");
            let opts = SettleStreaming {
                base: Duration::from_millis(300),
                extend: Duration::from_millis(300),
                hard_cap: Duration::from_secs(2),
                stream_window: Duration::from_millis(200),
                poll: Duration::from_millis(50),
            };
            let res = ctx.settle_streaming(opts, |_wid| false, || false);
            assert_eq!(
                res,
                vec![("w1".to_string(), false)],
                "silent worker killed at base → false"
            );
        }
        assert!(!wt_path.exists(), "Drop cleaned the worktree");
        let _ = std::fs::remove_dir_all(&repo);
    }

    // settle_streaming: the EXTRA `should_cancel` predicate (the GUI's autonomy_ceiling→L0 re-read)
    // halts in-flight workers mid-settle, BEFORE the base deadline — the regression #248 introduced
    // (settle then watched only the delegate_cancel Arc) and this fix restores.
    #[test]
    fn settle_streaming_honors_should_cancel_predicate() {
        let repo = temp_git_repo();
        let wt_path = repo.join(".agent-teams-worktrees").join("w2");
        {
            let mut ctx = RunContext::new("runS3", &repo, Box::new(StderrEmitter));
            let mut cmd = std::process::Command::new("sh");
            cmd.args(["-c", "sleep 60"]);
            ctx.spawn_worker(
                "w2",
                |_wt| crate::worker::WorkerSpec {
                    cmd,
                    prompt_via_stdin: true,
                    end_of_options_supported: false,
                },
                None,
                None,
            )
            .expect("spawn");
            // A LONG base so the worker would otherwise stream/sleep for a full minute; the external
            // predicate must kill it on the first tick regardless of the shared `delegate_cancel` flag.
            let opts = SettleStreaming {
                base: Duration::from_secs(60),
                extend: Duration::from_secs(60),
                hard_cap: Duration::from_secs(120),
                stream_window: Duration::from_millis(200),
                poll: Duration::from_millis(50),
            };
            let t0 = Instant::now();
            let res = ctx.settle_streaming(opts, |_wid| false, || true);
            assert_eq!(
                res,
                vec![("w2".to_string(), false)],
                "should_cancel killed the worker → false"
            );
            assert!(
                t0.elapsed() < Duration::from_secs(5),
                "halted on should_cancel, did not wait the base"
            );
        }
        assert!(!wt_path.exists(), "Drop cleaned the worktree");
        let _ = std::fs::remove_dir_all(&repo);
    }

    // A positional-delivery worker (`prompt_via_stdin: false`) with a `-`-leading prompt and a
    // harness that does NOT support `--` (end_of_options_supported: false — the default for every
    // harness today) must be REJECTED, and the rejection must clean up like the spawn-fail path:
    // no leftover worktree dir and no manifest entry for it.
    #[test]
    fn spawn_worker_rejects_dash_prompt_without_end_of_options_support() {
        let repo = temp_git_repo();
        let wid = "wdash";
        let wt_path = repo.join(".agent-teams-worktrees").join(wid);
        let manifest = repo.join(".agent-teams-worktrees").join("runDash.manifest");
        {
            let mut ctx = RunContext::new("runDash", &repo, Box::new(StderrEmitter));
            let mut cmd = std::process::Command::new("sh");
            cmd.args(["-c", "true"]);
            let res = ctx.spawn_worker(
                wid,
                |_wt| crate::worker::WorkerSpec {
                    cmd,
                    prompt_via_stdin: false, // positional delivery → the `-` prompt would be a flag
                    end_of_options_supported: false, // no harness is verified for `--`
                },
                Some("--dangerously-do-a-thing"),
                None,
            );
            assert!(
                res.is_err(),
                "a '-'-leading prompt must be rejected when `--` is unverified"
            );
            // Cleanup ran: the worktree dir was removed and the manifest carries no entry for it.
            assert!(!wt_path.exists(), "rejection removed the worktree dir");
            if let Ok(body) = std::fs::read_to_string(&manifest) {
                assert!(
                    !body.contains(wt_path.to_string_lossy().as_ref()),
                    "rejection unrecorded the worktree from the manifest"
                );
            }
            // No live worker was registered → nothing for Drop to reap.
            assert!(
                ctx.worktree_of(wid).is_none(),
                "no worker registered after a rejected spawn"
            );
        }
        let _ = std::fs::remove_dir_all(&repo);
    }

    // sweep_manifest reconciles a leftover worktree from a crashed prior run.
    #[test]
    fn sweep_manifest_reconciles_orphan_worktree() {
        let repo = temp_git_repo();
        // simulate a crashed run: a real worktree exists, recorded in a manifest, but no RunContext.
        let wt = add_worktree(&repo, "orphan").expect("worktree");
        let manifest = repo.join(".agent-teams-worktrees").join("crashed.manifest");
        std::fs::write(&manifest, format!("{}\n", wt.root.display())).unwrap();
        assert!(wt.root.exists());

        let n = sweep_manifest(&repo, &manifest);
        assert_eq!(n, 1, "swept one orphan");
        assert!(!wt.root.exists(), "orphan worktree removed");
        assert!(!manifest.exists(), "manifest consumed");
        let _ = std::fs::remove_dir_all(&repo);
    }
}
