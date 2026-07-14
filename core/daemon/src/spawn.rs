//! Q4 daemon-spawns-on-behalf (approach B) — the gated `handle_spawn`/`handle_close`
//! handler, the daemon-side spawn lifecycle (reaper sweep + TTL + max-cap + in-flight
//! hold + kill-all), and the durable worktree registry + audit-log wiring.
//!
//! ## Compilation posture (default-OFF inertness)
//!
//! The whole module is `#[cfg(any(test, feature = "daemon-spawn"))]`:
//! * `cargo build --release` (no feature, no test) → the module is ABSENT — `handle_spawn`
//!   / the reaper / the spawn-state / the primed set DO NOT EXIST in the default binary,
//!   and `server::route_request` answers [`response_code::SPAWN_UNAVAILABLE`].
//! * `cargo test` → compiled under `cfg(test)` so the unit tests below exercise the gate
//!   model, every validation rejection, reject-over-live-id, the cap, TTL expiry, the
//!   reaper, and kill-all against a [`DaemonSups`]`<FakePane>` + a `FakeExec` double (a
//!   real `Supervisor` owns a spawned PTY and cannot be built in a unit test).
//!
//! The LIVE production wiring ([`DaemonSpawnRoutable`] for [`supervisor::Supervisor`] +
//! [`RealSpawnExec`]) is `#[cfg(feature = "daemon-spawn")]` only — it is what
//! `server::SupRouter` calls, and it is compiled out by default.
//!
//! ## The double-gate (decision 1)
//!
//! A `Spawn` reaches [`handle_spawn`] only AFTER the accept loop's euid gate and the FRESH
//! per-request `allow_mutations` gate (`server.rs`); `handle_spawn` then checks the
//! SEPARATE `daemon_spawn_enabled` flag (read FRESH). Without BOTH `allow_mutations=true`
//! AND `daemon_spawn_enabled=true` (and the compiled-in feature), a `Spawn` is refused and
//! spawns nothing. The gates answer WHO; the daemon then independently RE-VALIDATES the
//! whole wire spec (WHAT) — id (C1), is_worker (C4), harness, extra_dirs scope (C3),
//! reject-over-live-id (D5), worktree path (C2), and force_fresh refusal (C5).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use agent_teams_core::{
    extra_dir_in_repo_scope, read_mcp_config, response_code, validate_model, validate_session_id,
    validate_spawn_id, SocketResponse, SpawnSpec,
};

use crate::audit;
use crate::handlers::PaneWrite;
use crate::registry_writer::{clear_live_registry, write_live_registry};
use crate::sups::DaemonSups;

/// Max concurrently live daemon-owned panes (decision 4 / panic containment). A `Spawn`
/// at the cap is refused with [`response_code::CAP_EXCEEDED`] BEFORE any worktree op, so a
/// same-user flood cannot grow daemon panes/worktrees without bound.
pub const MAX_DAEMON_PANES: usize = 16;

/// Per-pane time-to-live (decision 4). The reaper kills + removes any daemon-owned pane
/// older than this even if still alive — a hard ceiling on how long a now-app-quit-
/// surviving agent may run unattended before the daemon reclaims it.
pub const DAEMON_PANE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Filename of the durable per-id worktree registry (sibling of `state_root`) — a
/// daemon crash leaves a sweepable trail of `(id, git_root, root)` so orphaned worktrees
/// can be reclaimed. Distinct from every other sibling.
pub const DAEMON_WORKTREES_FILE: &str = "agent-teams-daemon-worktrees.json";

/// Unix-millis wall clock (the registry/TTL clock; no chrono dep).
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The capability a daemon-owned pane value must offer beyond [`PaneWrite`]: a kernel-tied
/// `kill` (the owned `Child`, PID-reuse-safe — must-fix A2) and the spawn-time `child_pid`
/// (D3). Implemented for the real [`supervisor::Supervisor`] (production) and a test
/// double. Kept separate from `PaneWrite` so the slice-1/2/3 handlers stay generic over
/// `PaneWrite` alone — only the Q4 spawn lifecycle needs `DaemonPane`.
pub trait DaemonPane: PaneWrite {
    /// Kill the owned child via the kernel-tied `Child::kill` (reuse-safe), NOT a pgid kill.
    fn kill(&mut self);
    /// The spawned child's OS pid, captured AT SPAWN (`None` after reap — D3).
    fn child_pid(&self) -> Option<u32>;
}

#[cfg(feature = "daemon-spawn")]
impl DaemonPane for supervisor::Supervisor {
    fn kill(&mut self) {
        supervisor::Supervisor::kill(self)
    }
    fn child_pid(&self) -> Option<u32> {
        supervisor::Supervisor::child_pid(self)
    }
}

/// One durable worktree record so Close/reap know which worktree to remove and a daemon
/// crash leaves a reclaimable trail (design §7 step 12). Serializable for the sibling file.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorktreeRecord {
    // `#[serde(default)]` on every field: a record written by a FUTURE daemon that adds
    // or renames a field still deserializes here (missing fields fall back to their type
    // default) rather than failing the WHOLE trail parse. That matters because the trail
    // is the sole crash-reclaim record — see `cold_start_sweep`, which now also refuses
    // to DELETE the trail file when it fails to parse (so a corrupt/newer trail is never
    // silently forgotten).
    #[serde(default)]
    pub git_root: PathBuf,
    #[serde(default)]
    pub root: PathBuf,
    #[serde(default)]
    pub spawned_at: u64,
}

/// The outcome of [`SpawnExec::prepare_worktree`] — the worktree the daemon will spawn
/// into, plus the reuse/dirty facts the C2/C5 checks need. Production fills this from
/// `supervisor::add_worktree` + `git status`; the test `FakeExec` synthesizes it.
pub struct PreparedWorktree {
    /// The directory the agent runs in (worktree cwd, or the bare repo fallback).
    pub cwd: PathBuf,
    /// The worktree root (`git worktree remove` operates on this).
    pub root: PathBuf,
    /// The repo's git toplevel.
    pub git_root: PathBuf,
    /// Whether a real isolated worktree was created/reused (vs the bare-repo fallback).
    pub has_worktree: bool,
    /// Whether an EXISTING worktree was reused (vs freshly created).
    pub reused: bool,
    /// Whether a reused worktree has uncommitted changes (drives the C5 UNCOMMITTED_WORK).
    pub dirty: bool,
}

/// The seam that abstracts the live worktree + PTY-spawn calls so [`handle_spawn`]'s
/// validation / lifecycle is unit-testable without a real `Supervisor` (which owns a
/// spawned child). Production is [`RealSpawnExec`] (calls `supervisor::add_worktree` /
/// `Supervisor::spawn` / `supervisor::remove_worktree`); tests impl it to return a
/// `FakePane`. Mirrors the `ConnRouter`/`PaneWrite` test-double pattern.
pub trait SpawnExec {
    /// The owned-pane value produced (a `Supervisor` in production).
    type Pane: DaemonPane;
    /// Create or reuse the per-id worktree under `repo` (already canonicalized).
    fn prepare_worktree(&self, repo: &Path, id: &str) -> Result<PreparedWorktree, String>;
    /// Build the harness argv + open the PTY + spawn the child into `wt` → an owned pane.
    fn spawn(&self, spec: &SpawnSpec, wt: &PreparedWorktree) -> Result<Self::Pane, String>;
    /// Remove the worktree (spawn-error rollback + Close/reap). Best-effort.
    fn remove_worktree(&self, git_root: &Path, id: &str, root: &Path);
}

/// The daemon's Q4 spawn runtime state — the V-independent bookkeeping that lives beside
/// [`DaemonSups`]: the per-pane claude prime set (C7), the in-flight-spawn counter (D4),
/// the id→child-pid map (D3, the live-registry value), and the durable worktree registry.
pub struct DaemonSpawnState {
    /// Per-pane claude banner-prime tracking (C7): an id is present once primed; reset on
    /// (re)spawn so a fresh pane at the same id re-primes (mirrors `do_spawn:984`).
    primed: Mutex<HashSet<String>>,
    /// In-flight spawn count (D4): `> 0` holds the idle tick open across a mid-spawn
    /// window (PTY open + fork/exec) so a first-spawn-after-grace tick can't exit and
    /// SIGHUP the fresh agent.
    pending: AtomicUsize,
    /// id → real child pid captured AT SPAWN (D3) — what `write_live_registry` stamps so
    /// `partition_reattach` fires for daemon-owned panes.
    child_pids: Mutex<HashMap<String, u32>>,
    /// Durable per-id worktree record (id → `(git_root, root, spawned_at)`).
    worktrees: Mutex<HashMap<String, WorktreeRecord>>,
    /// Per-id spawn/reap/close SERIALIZATION set (anti-double-spawn TOCTOU, must-fix). An id
    /// present here is being SPAWNED, REAPED, or CLOSED by some connection/reaper thread; any
    /// concurrent op on the same id must defer. This is the per-id MUTEX the review requires
    /// spanning two critical sections that the per-connection-thread model otherwise races:
    /// * spawn — held from the liveness check THROUGH the map insert, so two concurrent
    ///   `Spawn{id}` can no longer BOTH pass `contains()`, double fork/exec, and then kill the
    ///   loser (the reject-over-live-id D5 / no-double-spawn invariant, which §6 wrongly claimed
    ///   held "by construction").
    /// * reap/close — held from the map removal THROUGH the worktree removal, so a concurrent
    ///   `Spawn{id}` cannot re-claim the just-freed id and reuse the on-disk worktree while the
    ///   reaper/close is still `git worktree remove`-ing it (the reaper-vs-respawn corruption
    ///   race). Distinct from `pending` (the GLOBAL in-flight COUNT for the idle hold — D4).
    inflight_ids: Mutex<HashSet<String>>,
}

impl Default for DaemonSpawnState {
    fn default() -> Self {
        Self::new()
    }
}

impl DaemonSpawnState {
    pub fn new() -> Self {
        Self {
            primed: Mutex::new(HashSet::new()),
            pending: AtomicUsize::new(0),
            child_pids: Mutex::new(HashMap::new()),
            worktrees: Mutex::new(HashMap::new()),
            inflight_ids: Mutex::new(HashSet::new()),
        }
    }

    /// Current in-flight spawn count (D4) — ORed into the idle-tick hold so a mid-spawn
    /// tick never sees 0 live and exits.
    pub fn pending_count(&self) -> usize {
        self.pending.load(Ordering::SeqCst)
    }

    /// Atomically CLAIM `id` for a SPAWN (anti-double-spawn TOCTOU, must-fix). Returns an
    /// [`InflightGuard`] iff `id` is NEITHER already live (`sups.contains`) NOR already
    /// claimed by a concurrent spawn/reap/close. The check + the set-insert happen under ONE
    /// `inflight_ids` lock acquisition, and the returned guard is held by `handle_spawn`
    /// across the whole worktree-prep + fork/exec + map-insert window — so two concurrent
    /// `Spawn{id}` can never both observe the id absent, double fork/exec, and kill the loser
    /// (the precise D5 DoS the §6 "anti-double-spawn correct BY CONSTRUCTION" claim missed).
    /// Lock order: `inflight_ids` → `sups` map (NEVER the reverse — see `try_claim_id`).
    fn claim_spawn_id<'a, V>(
        &'a self,
        id: &str,
        sups: &DaemonSups<V>,
    ) -> Option<InflightGuard<'a>> {
        let mut set = self.inflight_ids.lock().unwrap_or_else(|e| e.into_inner());
        if set.contains(id) || sups.contains(id) {
            return None;
        }
        set.insert(id.to_string());
        Some(InflightGuard {
            set: &self.inflight_ids,
            id: id.to_string(),
        })
    }

    /// Atomically CLAIM `id` for a REAP/CLOSE — an op on an ALREADY-LIVE id (so, unlike
    /// [`claim_spawn_id`], liveness is NOT a bar). Returns a guard iff `id` is not already
    /// claimed by a concurrent spawn/reap/close. Held across the map-removal + worktree-removal
    /// so a concurrent `Spawn{id}` (which calls `claim_spawn_id` and sees the id still claimed)
    /// cannot re-claim the freed id and reuse the worktree mid-cleanup. Locks ONLY `inflight_ids`
    /// (never the map) so it can be called WHILE the map lock is free, keeping the single global
    /// order `inflight_ids → map` (no deadlock with `claim_spawn_id`).
    fn try_claim_id<'a>(&'a self, id: &str) -> Option<InflightGuard<'a>> {
        let mut set = self.inflight_ids.lock().unwrap_or_else(|e| e.into_inner());
        if set.contains(id) {
            return None;
        }
        set.insert(id.to_string());
        Some(InflightGuard {
            set: &self.inflight_ids,
            id: id.to_string(),
        })
    }

    /// The primed-set handle for the C7 banner-prime ([`crate::handlers::maybe_prime_claude`]).
    pub fn primed_handle(&self) -> &Mutex<HashSet<String>> {
        &self.primed
    }

    /// Forget any primed flag for `id` (a fresh pane at this id must re-prime; on close
    /// the entry is dropped). Mirrors `do_spawn:984`.
    fn forget_primed(&self, id: &str) {
        self.primed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
    }

    fn set_child_pid(&self, id: &str, pid: u32) {
        self.child_pids
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.to_string(), pid);
    }
    fn remove_child_pid(&self, id: &str) {
        self.child_pids
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
    }
    /// A snapshot clone of the id→child-pid map (what `write_live_registry` consumes).
    pub fn child_pid_map(&self) -> HashMap<String, u32> {
        self.child_pids
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn record_worktree(&self, id: &str, rec: WorktreeRecord) {
        self.worktrees
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.to_string(), rec);
    }
    fn take_worktree(&self, id: &str) -> Option<WorktreeRecord> {
        self.worktrees
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id)
    }
    /// id → `spawned_at` (millis) snapshot, read OUTSIDE the sups map lock so the reaper
    /// never nests the worktrees lock under the map lock.
    fn spawned_at_times(&self) -> HashMap<String, u64> {
        self.worktrees
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|(id, rec)| (id.clone(), rec.spawned_at))
            .collect()
    }

    /// The durable worktree-registry path (sibling of `state_root`). `None` if no parent.
    pub fn worktrees_path(state_root: &Path) -> Option<PathBuf> {
        state_root.parent().map(|p| p.join(DAEMON_WORKTREES_FILE))
    }

    /// Best-effort persist of the in-memory worktree map to the sibling file (crash-trail).
    fn persist_worktrees(&self, state_root: &Path) {
        let Some(path) = Self::worktrees_path(state_root) else {
            return;
        };
        let map = self
            .worktrees
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Ok(json) = serde_json::to_string(&map) {
            // tmp+rename so a crash-recovery reader never sees a torn/partial trail.
            let _ = crate::fsutil::write_atomic(&path, json.as_bytes());
        }
    }
}

/// RAII in-flight-spawn hold (D4): increments `pending` on construction, decrements on
/// drop, so EVERY exit path of [`handle_spawn`] (gate refusal, validation reject, spawn
/// error, success) releases the hold.
struct PendingGuard<'a>(&'a AtomicUsize);
impl<'a> PendingGuard<'a> {
    fn new(state: &'a DaemonSpawnState) -> Self {
        state.pending.fetch_add(1, Ordering::SeqCst);
        PendingGuard(&state.pending)
    }
}
impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// RAII per-id claim release (anti-double-spawn TOCTOU, must-fix). Removes `id` from the
/// `inflight_ids` set on drop, so EVERY exit path of the claiming handler (gate refusal,
/// validation reject, spawn error, success, or a reaper `continue`) frees the claim. Locks
/// ONLY `inflight_ids` on drop (never the map) — preserves the `inflight_ids → map` order.
struct InflightGuard<'a> {
    set: &'a Mutex<HashSet<String>>,
    id: String,
}
impl Drop for InflightGuard<'_> {
    fn drop(&mut self) {
        self.set
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
    }
}

/// The single atomic worktree+spawn+rollback handler (decision 1 + must-fixes). Returns a
/// refusal [`SocketResponse`] and spawns NOTHING unless every gate + re-validation passes.
pub fn handle_spawn<X: SpawnExec>(
    sups: &DaemonSups<X::Pane>,
    state: &DaemonSpawnState,
    state_root: &Path,
    spec: SpawnSpec,
    exec: &X,
) -> SocketResponse {
    // 1. DEDICATED daemon-spawn gate (decision 1 / B2), read FRESH like `allow_mutations`.
    //    `allow_mutations=true` alone is NOT sufficient; absent/malformed config ⇒ OFF.
    if !read_mcp_config(state_root).daemon_spawn_enabled {
        return SocketResponse::err(
            response_code::SPAWN_DISABLED,
            "daemon_spawn_enabled is false in mcp-config.json",
        );
    }
    // 2. IN-FLIGHT HOLD (D4) — held across the whole handler, released on every exit.
    let _pending = PendingGuard::new(state);
    // 3. VALIDATE ID (C1) — before it keys the worktree dir / write-lock / registry.
    if !validate_spawn_id(&spec.id) {
        return SocketResponse::err(
            response_code::SPAWN_REJECTED,
            "invalid id (charset/length/separator/traversal)",
        );
    }
    // 4. is_worker FORBIDDEN over the wire (decision 3 / C4). Workers spawn LOCALLY in-app.
    if spec.is_worker {
        return SocketResponse::err(
            response_code::SPAWN_REJECTED,
            "is_worker is not permitted over the wire (delegate/flywheel workers spawn locally)",
        );
    }
    // 5. HARNESS — validated via the shared wire→enum map (no app reach-in).
    if supervisor::Harness::from_wire(&spec.harness).is_none() {
        return SocketResponse::err(response_code::SPAWN_REJECTED, "unknown harness");
    }
    // 5b. session_id / model — re-validated INDEPENDENTLY (C6, the WHO-not-WHAT lesson
    //     extended past id/extra_dirs to the two fields that flow VERBATIM into the harness
    //     argv). A leading-`-` session_id on claude's optional-value `--resume` path injects
    //     a standalone flag (`--resume --dangerously-skip-permissions` → YOLO mode), nullifying
    //     the C3 repo-scope; a leading-`-` model is the same class. Both are rejected here so
    //     the daemon never maps an injected flag through `RealSpawnExec::spawn`.
    if let Some(sid) = spec.session_id.as_deref() {
        if !validate_session_id(sid) {
            return SocketResponse::err(
                response_code::SPAWN_REJECTED,
                "invalid session_id (charset/length or leading '-' flag-injection)",
            );
        }
    }
    if let Some(m) = spec.model.as_deref() {
        // An empty model is "account default" (mapped to no flag) — only a non-empty value
        // reaches the harness argv, so only that is validated.
        if !m.is_empty() && !validate_model(m) {
            return SocketResponse::err(
                response_code::SPAWN_REJECTED,
                "invalid model (whitespace/control or leading '-' flag-injection)",
            );
        }
    }
    // 6. REJECT-OVER-LIVE-ID (D5) + ANTI-DOUBLE-SPAWN HOLD: claim the id ATOMICALLY (one
    //    `inflight_ids` lock spans the liveness check AND the set-insert) and HOLD the claim
    //    through the map insert below. This closes the TOCTOU the per-connection-thread model
    //    opened — two concurrent `Spawn{id}` can no longer both pass `contains()`, fork/exec
    //    two children, and have the loser's insert kill the winner's running agent (the precise
    //    D5 DoS). Released on EVERY exit path via the guard's Drop. A claim failure means the id
    //    is already live OR a concurrent spawn is mid-flight for it → reject (NEVER kill).
    let _id_claim = match state.claim_spawn_id(&spec.id, sups) {
        Some(g) => g,
        None => {
            return SocketResponse::err(
                response_code::ALREADY_LIVE,
                "id already live or a spawn is in flight (Close then Spawn to reopen)",
            )
        }
    };
    // 7. MAX-CAP (decision 4) — refuse BEFORE any worktree op.
    if sups.count_live() >= MAX_DAEMON_PANES {
        return SocketResponse::err(response_code::CAP_EXCEEDED, "daemon live-pane cap reached");
    }
    // 8. extra_dirs REPO-SCOPE (C3 / decision 2). Canonicalize both sides so a symlink
    //    can't escape the lexical containment; an out-of-repo dir is rejected.
    let repo_root = std::fs::canonicalize(&spec.repo).unwrap_or_else(|_| PathBuf::from(&spec.repo));
    for d in &spec.extra_dirs {
        let dp = std::fs::canonicalize(d).unwrap_or_else(|_| PathBuf::from(d));
        if !extra_dir_in_repo_scope(&repo_root, &dp) {
            return SocketResponse::err(
                response_code::SPAWN_REJECTED,
                "extra_dir is outside the spawn's repo scope",
            );
        }
    }
    // 9. WORKTREE PREP (C2/C5). On failure: a required worktree → reject; the daemon never
    //    spawns into the bare repo root (it would edit the user's checkout), so any prep
    //    failure is a reject.
    let wt = match exec.prepare_worktree(&repo_root, &spec.id) {
        Ok(w) => w,
        Err(e) => {
            return SocketResponse::err(
                response_code::SPAWN_REJECTED,
                format!("worktree preparation failed: {e}"),
            )
        }
    };
    if spec.require_worktree && !wt.has_worktree {
        return SocketResponse::err(
            response_code::SPAWN_REJECTED,
            "an isolated worktree is required but could not be created",
        );
    }
    // C2: the worktree root MUST be the sanctioned per-id path derived from id+repo.
    if wt.has_worktree {
        let sanctioned = wt.git_root.join(".agent-teams-worktrees").join(&spec.id);
        if wt.root != sanctioned {
            exec.remove_worktree(&wt.git_root, &spec.id, &wt.root);
            return SocketResponse::err(
                response_code::SPAWN_REJECTED,
                "worktree root is not the sanctioned per-id path",
            );
        }
    }
    // C5 (decision 5): a reused, DIRTY worktree + fresh_from_main → refuse with the
    // sentinel; the daemon NEVER runs the destructive freshen. Leave the reused tree
    // intact (the GUI confirms + cleans app-side, then re-Spawns). NEVER call freshen_worktree.
    if spec.fresh_from_main && wt.reused && wt.dirty {
        return SocketResponse::err(
            response_code::UNCOMMITTED_WORK,
            format!(
                "UNCOMMITTED_WORK:{}:reused worktree has uncommitted changes",
                spec.id
            ),
        );
    }
    // 11. BUILD argv + open PTY + spawn (the daemon is the parent from birth — approach B).
    //     On error, roll back the just-created worktree (no leak).
    let pane = match exec.spawn(&spec, &wt) {
        Ok(p) => p,
        Err(e) => {
            if wt.has_worktree {
                exec.remove_worktree(&wt.git_root, &spec.id, &wt.root);
            }
            return SocketResponse::err(
                response_code::SPAWN_REJECTED,
                format!("spawn failed: {e}"),
            );
        }
    };
    // 13a. CAPTURE child_pid BEFORE insert (D3) — `process_id` is None after reap.
    let child_pid = pane.child_pid();
    // 12. INSERT (id-absence guaranteed: the step-6 `_id_claim` is STILL HELD here, so no
    //     concurrent spawn could have claimed+inserted this id, and only `handle_spawn`
    //     inserts) + reset the primed set (C7).
    if let Some(mut old) = sups.insert(spec.id.clone(), pane) {
        // Truly unreachable now that the per-id claim spans contains..insert, but kept
        // defensive: never orphan a live child if that invariant ever regresses.
        old.kill();
    }
    state.forget_primed(&spec.id);
    // 12b. DURABLE worktree registry (design §7 step 12) + child-pid (D3).
    if wt.has_worktree {
        state.record_worktree(
            &spec.id,
            WorktreeRecord {
                git_root: wt.git_root.clone(),
                root: wt.root.clone(),
                spawned_at: now_millis(),
            },
        );
    }
    if let Some(pid) = child_pid {
        state.set_child_pid(&spec.id, pid);
    }
    // 13b. LIVE REGISTRY (D2/D3) — daemon pid as app_pid + the real child pid (makes
    //      partition_reattach fire) — and the durable worktree mirror.
    write_live_registry(state_root, &state.child_pid_map());
    state.persist_worktrees(state_root);
    // 15. AUDIT (E1).
    audit::audit_spawn(
        state_root,
        &spec.id,
        &spec.harness,
        &spec.repo,
        spec.is_worker,
        spec.extra_dirs.len(),
        child_pid,
    );
    SocketResponse::ok("spawned")
}

/// Close a daemon-owned pane (§3.3): remove from the map, kill the child (kernel-tied),
/// remove the worktree, rewrite the live registry, drop the bookkeeping, audit-log.
/// Idempotent-OK for an absent id (like `Detach`).
pub fn handle_close<X: SpawnExec>(
    sups: &DaemonSups<X::Pane>,
    state: &DaemonSpawnState,
    state_root: &Path,
    id: &str,
    exec: &X,
) -> SocketResponse {
    // Serialize against a concurrent Spawn/reap of the same id (same claim as the reaper): hold
    // the per-id claim across the map removal + worktree removal so a racing `Spawn{id}` cannot
    // re-claim the freed id and reuse the worktree mid-cleanup. A claim failure means a spawn/
    // reap/close is mid-flight for this id → answer a DISTINCT pending error, NOT ok("closed"):
    // a false ok would let the caller drop its anchor while the in-flight spawn lands a live
    // pane moments later (orphaned from the app's view). The caller retries once it settles.
    let _claim = match state.try_claim_id(id) {
        Some(g) => g,
        None => return SocketResponse::err(
            response_code::CLOSE_PENDING,
            "a spawn/reap/close is in flight for this id — nothing closed; retry once it settles",
        ),
    };
    match sups.remove(id) {
        Some(mut pane) => {
            pane.kill();
            if let Some(rec) = state.take_worktree(id) {
                exec.remove_worktree(&rec.git_root, id, &rec.root);
            }
            state.remove_child_pid(id);
            state.forget_primed(id);
            write_live_registry(state_root, &state.child_pid_map());
            state.persist_worktrees(state_root);
            audit::audit_event(state_root, "close", id);
            SocketResponse::ok("closed")
        }
        None => SocketResponse::ok("closed"),
    }
}

/// Reaper sweep (D1/D2 + TTL): remove every pane that is DEAD (`is_alive` = `try_wait`,
/// which REAPS the zombie) OR older than `ttl`, kill an expired-but-alive child, remove
/// each worktree, drop bookkeeping, then rewrite the live registry ONCE. Relocates the
/// app's frontend `dead_pane_ids` sweep daemon-side so an exited/expired agent actually
/// drops `count_live` (else idle-shutdown never fires) and a relaunched GUI's
/// `partition_reattach` never returns a corpse. `now_ms`/`ttl` are params for testability.
pub fn reaper_sweep<X: SpawnExec>(
    sups: &DaemonSups<X::Pane>,
    state: &DaemonSpawnState,
    state_root: &Path,
    exec: &X,
    now_ms: u64,
    ttl: Duration,
) {
    let ttl_ms = ttl.as_millis() as u64;
    // Snapshot spawn times OUTSIDE the map lock (no nested locks).
    let times = state.spawned_at_times();
    let is_doomed = |id: &str, pane: &mut X::Pane| -> bool {
        let dead = !pane.is_alive();
        let expired = times
            .get(id)
            .map(|t| now_ms.saturating_sub(*t) >= ttl_ms)
            .unwrap_or(false);
        dead || expired
    };
    // Phase 1 (map lock): IDENTIFY dead/expired candidate ids — do NOT remove them yet.
    // Removing-then-cleaning-up-unlocked is the reaper-vs-respawn race: a `Spawn{id}` in the
    // window between the map removal and `take_worktree(id)` re-claims the freed id, reuses
    // the on-disk worktree, and inserts a fresh live pane — which the reaper then
    // `git worktree remove --force`s out from under (orphan + registry desync). We close it
    // by holding the per-id claim ACROSS removal + cleanup (Phase 2), exactly as `handle_spawn`
    // holds it across contains..insert.
    let candidates: Vec<String> = sups.with_map_mut(|m| {
        m.iter_mut()
            .filter_map(|(id, pane)| is_doomed(id, pane).then(|| id.clone()))
            .collect()
    });
    if candidates.is_empty() {
        return;
    }
    let mut any_removed = false;
    for id in candidates {
        // Claim the id so a concurrent `Spawn{id}` (which would see it claimed in
        // `claim_spawn_id`) cannot re-claim + reuse the worktree while we remove it. If a spawn
        // already holds the claim, the id is becoming a FRESH live pane — skip reaping it this
        // sweep (a later sweep reaps it if it is still dead/expired). The guard frees the claim
        // on each loop iteration (drop at end of the iteration scope).
        let _claim = match state.try_claim_id(&id) {
            Some(g) => g,
            None => continue,
        };
        // Phase 2 (map lock, UNDER the claim): re-confirm still-present AND still-doomed, then
        // remove + take the owned pane. Nothing can have re-spawned the id (we hold the claim);
        // a `Close`/`kill_all` may have removed it (→ None) or it may have produced output and
        // gone live-again is impossible without a spawn — so the re-check only guards Close.
        let removed = sups.with_map_mut(|m| {
            let still_doomed = m
                .get_mut(&id)
                .map(|pane| is_doomed(&id, pane))
                .unwrap_or(false);
            if still_doomed {
                m.remove(&id)
            } else {
                None
            }
        });
        let Some(mut pane) = removed else { continue };
        // Kill an expired-but-alive child (a no-op error on an already-dead one, ignored).
        pane.kill();
        if let Some(rec) = state.take_worktree(&id) {
            exec.remove_worktree(&rec.git_root, &id, &rec.root);
        }
        state.remove_child_pid(&id);
        state.forget_primed(&id);
        audit::audit_event(state_root, "reap", &id);
        any_removed = true;
    }
    // Rewrite the registry to exclude the reaped ids (D2) + the durable mirror.
    if any_removed {
        write_live_registry(state_root, &state.child_pid_map());
        state.persist_worktrees(state_root);
    }
}

/// Production reaper entry: sweep with the real clock + [`DAEMON_PANE_TTL`].
pub fn reap_dead_and_expired<X: SpawnExec>(
    sups: &DaemonSups<X::Pane>,
    state: &DaemonSpawnState,
    state_root: &Path,
    exec: &X,
) {
    reaper_sweep(sups, state, state_root, exec, now_millis(), DAEMON_PANE_TTL);
}

/// Kill-all entry point (decision 4): kill EVERY daemon-owned child, remove each worktree,
/// clear the live registry + the durable worktree registry, audit-log. The GUI panic
/// button that calls this is DEFERRED (the entry point exists now).
pub fn kill_all<X: SpawnExec>(
    sups: &DaemonSups<X::Pane>,
    state: &DaemonSpawnState,
    state_root: &Path,
    exec: &X,
) {
    let all: Vec<(String, X::Pane)> = sups.with_map_mut(|m| m.drain().collect());
    for (id, mut pane) in all {
        pane.kill();
        if let Some(rec) = state.take_worktree(&id) {
            exec.remove_worktree(&rec.git_root, &id, &rec.root);
        }
        state.remove_child_pid(&id);
        state.forget_primed(&id);
        audit::audit_event(state_root, "kill_all", &id);
    }
    clear_live_registry(state_root);
    state.persist_worktrees(state_root);
}

/// `kill(pid, 0)` liveness probe. `false` for pid 0 / a dead pid / EPERM (exists but is
/// another user's process — NOT ours to kill, so it counts as "not sweepable").
fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// COLD-START ORPHAN SWEEP (crash recovery — the reader half of the durable trail).
/// A daemon that crashed / was SIGKILLed left behind (a) the durable worktree trail
/// (`agent-teams-daemon-worktrees.json`, previously written but NEVER read) and (b) a live
/// registry stamped with its pid + its children's pids. On the next cold start:
/// kill any recorded child pid that is still alive, remove every recorded worktree, then
/// clear BOTH files. Best-effort throughout — a sweep failure never aborts startup.
///
/// GUARD: the live registry file is SHARED with the GUI app. If the prior registry's
/// `app_pid` is still ALIVE, its writer (the GUI, or another daemon instance) is running
/// and OWNS those panes — their pids are NOT orphans and are never killed. The daemon's
/// own worktree trail is still swept (only the daemon writes it).
pub fn cold_start_sweep(state_root: &Path) {
    // Resolve the SHARED live-registry writer's liveness ONCE up front. The registry file
    // is co-owned with the GUI app; `writer_alive` gates BOTH the orphan-kill (a) and the
    // registry clear (c) so a daemon cold-start that races a LIVE GUI never touches the
    // GUI's panes.
    let reg = agent_teams_core::read_registry(state_root);
    let writer_alive = reg
        .as_ref()
        .and_then(|r| r.app_pid)
        .map(pid_alive)
        .unwrap_or(false);

    // (a) prior live registry → kill orphaned children (only when the writer is DEAD).
    if !writer_alive {
        if let Some(reg) = reg.as_ref() {
            for ws in &reg.workspaces {
                if let Some(pid) = ws.pid.filter(|p| *p > 0) {
                    if pid_alive(pid) {
                        // No cheap start-time check without proc-table parsing → kill-if-exists.
                        // Accepted pid-reuse window: the pid belonged to our own same-euid agent
                        // child; a reused pid owned by another user fails EPERM harmlessly (and
                        // `pid_alive` already returned false for it).
                        audit::audit_event(state_root, "cold_start_kill", &ws.id);
                        unsafe {
                            libc::kill(pid as libc::pid_t, libc::SIGKILL);
                        }
                    }
                }
            }
        }
    }

    // (b) the durable worktree trail → remove every recorded worktree. Distinguish
    // "absent/empty" (nothing to reclaim) from "present-but-CORRUPT": on a parse error we
    // must NOT delete the file (see (c)), else the sole crash-reclaim record for worktrees
    // we could not parse+remove is destroyed and the worktrees leak forever.
    let wt_path = DaemonSpawnState::worktrees_path(state_root);
    let trail_body = wt_path
        .as_deref()
        .and_then(|p| std::fs::read_to_string(p).ok());
    let parsed: Result<HashMap<String, WorktreeRecord>, ()> = match trail_body.as_deref() {
        // Absent, or present-but-empty → an empty map is the correct, non-corrupt read.
        None => Ok(HashMap::new()),
        Some(body) if body.trim().is_empty() => Ok(HashMap::new()),
        Some(body) => serde_json::from_str(body).map_err(|_| ()),
    };
    let trail_ok = parsed.is_ok();
    let records = parsed.unwrap_or_default();
    for (id, rec) in &records {
        let _ = supervisor::remove_worktree(&rec.git_root, id, &rec.root);
        audit::audit_event(state_root, "cold_start_sweep", id);
    }

    // (c) consume the reclaim records — but each only when it is SAFE to:
    //   - the worktree trail is deleted ONLY if it parsed cleanly (a corrupt/newer trail
    //     is LEFT so its records aren't silently forgotten);
    //   - the shared live registry is cleared ONLY when its writer (the GUI) is DEAD —
    //     clearing it under a live GUI would erase that GUI's live panes.
    if trail_ok {
        if let Some(p) = wt_path {
            let _ = std::fs::remove_file(&p);
        }
    }
    if !writer_alive {
        clear_live_registry(state_root);
    }
}

// ───────────────────────── live production wiring (feature only) ────────────────────────
//
// The real exec + the routing trait `server::SupRouter` calls. Compiled out by default —
// only `cargo build --features daemon-spawn` (post-security-review) pulls it in.

/// Resolve the daemon's hooks dir + sidecar bin from its bundle layout / env, best-effort
/// (a wrong path degrades MCP/persona injection but never FAILS a spawn — AC-6).
#[cfg(feature = "daemon-spawn")]
pub struct RealSpawnExec {
    pub hooks_dir: PathBuf,
    pub state_root: PathBuf,
    pub sidecar_bin: PathBuf,
    /// The phase-b (mutation) sidecar handed ONLY to Coordinator-role panes — the daemon-side
    /// mirror of the app's capability-by-role gate (`resolve_coordinator_sidecar_bin` /
    /// `chosen_sidecar` in `app/src-tauri/src/lib.rs`). Without this, a daemon `Spawn` that
    /// parses `role=coordinator` would silently hand the coordinator the READ-ONLY sidecar.
    pub coordinator_sidecar_bin: PathBuf,
}

#[cfg(feature = "daemon-spawn")]
impl RealSpawnExec {
    /// Resolve paths daemon-side. `AGENT_TEAMS_HOOKS_DIR` / `AGENT_TEAMS_SIDECAR_BIN` /
    /// `AGENT_TEAMS_MCP_COORDINATOR_BIN` override; otherwise fall back to siblings of the
    /// daemon binary. Best-effort. The coordinator sidecar FAILS SAFE (mirror of the app's
    /// `resolve_coordinator_sidecar_bin`): if the separate `agent-teams-mcp-coordinator`
    /// binary is absent, coordinator panes get the read-only sidecar — a coordinator on a
    /// build that never bundled the mutation binary simply cannot broadcast (never a silent
    /// grant, never a spawn failure).
    pub fn resolve(state_root: &Path) -> Self {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        let hooks_dir = std::env::var("AGENT_TEAMS_HOOKS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| exe_dir.join("hooks"));
        let sidecar_bin = std::env::var("AGENT_TEAMS_SIDECAR_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|_| exe_dir.join("agent-teams-mcp"));
        let coordinator_sidecar_bin = std::env::var("AGENT_TEAMS_MCP_COORDINATOR_BIN")
            .map(PathBuf::from)
            .ok()
            .or_else(|| {
                let cand = exe_dir.join("agent-teams-mcp-coordinator");
                cand.exists().then_some(cand)
            })
            .unwrap_or_else(|| sidecar_bin.clone());
        Self {
            hooks_dir,
            state_root: state_root.to_path_buf(),
            sidecar_bin,
            coordinator_sidecar_bin,
        }
    }
}

#[cfg(feature = "daemon-spawn")]
impl SpawnExec for RealSpawnExec {
    type Pane = supervisor::Supervisor;

    fn prepare_worktree(&self, repo: &Path, id: &str) -> Result<PreparedWorktree, String> {
        // Was the per-id worktree already on disk (reuse path) BEFORE add_worktree?
        let sanctioned = std::process::Command::new("git")
            .current_dir(repo)
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| {
                PathBuf::from(String::from_utf8_lossy(&o.stdout).trim().to_string())
                    .join(".agent-teams-worktrees")
                    .join(id)
            });
        let reused = sanctioned.as_ref().map(|p| p.exists()).unwrap_or(false);
        let wt = supervisor::add_worktree(repo, id).map_err(|e| e.to_string())?;
        // A reused worktree may be dirty (uncommitted prior-run state) — drives C5.
        let dirty = if reused {
            std::process::Command::new("git")
                .current_dir(&wt.root)
                .args(["status", "--porcelain"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| !o.stdout.is_empty())
                .unwrap_or(false)
        } else {
            false
        };
        Ok(PreparedWorktree {
            cwd: wt.cwd,
            root: wt.root,
            git_root: wt.git_root,
            has_worktree: true,
            reused,
            dirty,
        })
    }

    fn spawn(&self, spec: &SpawnSpec, wt: &PreparedWorktree) -> Result<Self::Pane, String> {
        // Map the wire SpawnSpec → a supervisor WorkspaceSpec (the daemon never accepts
        // is_worker over the wire, so this is always a human-style pane: is_worker=false,
        // extra_dirs empty after the repo-scope check — workers stay local).
        let harness = supervisor::Harness::from_wire(&spec.harness).ok_or("unknown harness")?;
        let role = spec
            .role
            .as_deref()
            .and_then(|r| r.parse::<roles::AgentRole>().ok());
        // Capability-by-role (mirror of app lib.rs `chosen_sidecar`): ONLY a Coordinator-role
        // pane gets the phase-b mutation sidecar; every other pane gets the read-only sidecar
        // and has no broadcast/send-input tool to call (least-privilege — the capability is
        // absent, not merely refused at the socket gate).
        let sidecar = if matches!(role, Some(roles::AgentRole::Coordinator)) {
            &self.coordinator_sidecar_bin
        } else {
            &self.sidecar_bin
        };
        let ws = supervisor::WorkspaceSpec {
            id: spec.id.clone(),
            harness,
            // Gap #6: deliberate cwd — the prepared worktree when it exists (it was
            // just minted, but a racing reap could have removed it), else the wire
            // repo, else HOME. Never an inherited `/` (the daemon is launchd-spawned
            // with cwd `/`, and a `/` cwd slugs the claude transcript dir to bare `-`).
            worktree: supervisor::resolve_spawn_cwd(Some(&wt.cwd), Some(Path::new(&spec.repo))),
            session_id: spec.session_id.clone(),
            resume: spec.resume,
            role,
            is_worker: false,
            extra_dirs: spec.extra_dirs.iter().map(PathBuf::from).collect(),
            model: spec.model.clone(),
        };
        supervisor::Supervisor::spawn(&ws, &self.hooks_dir, &self.state_root, sidecar)
            .map_err(|e| e.to_string())
    }

    fn remove_worktree(&self, git_root: &Path, id: &str, root: &Path) {
        let _ = supervisor::remove_worktree(git_root, id, root);
    }
}

/// The routing capability `server::SupRouter` dispatches `Spawn`/`Close` through under the
/// feature (so the generic `SupRouter<V>` need not be specialized to `Supervisor`). Only
/// `Supervisor` (production) gets the REAL impl; the daemon's streaming test doubles get a
/// trivial impl under `cfg(test)` so the feature build's tests still compile.
#[cfg(feature = "daemon-spawn")]
pub trait DaemonSpawnRoutable: DaemonPane + crate::handlers::PaneStream + Sized {
    fn route_spawn(
        sups: &DaemonSups<Self>,
        state: &DaemonSpawnState,
        state_root: &Path,
        spec: SpawnSpec,
    ) -> SocketResponse;
    fn route_close(
        sups: &DaemonSups<Self>,
        state: &DaemonSpawnState,
        state_root: &Path,
        id: &str,
    ) -> SocketResponse;
    /// Drive one reaper sweep (D1/D2 + TTL) — the periodic daemon-side dead/expired sweep
    /// that relocates the app's `dead_pane_ids` sweep into the daemon.
    fn route_reap(sups: &DaemonSups<Self>, state: &DaemonSpawnState, state_root: &Path);
    /// Kill EVERY daemon-owned pane + remove its worktree + clear both registries — the
    /// graceful-termination path (`serve`'s SIGTERM watcher calls this before exiting so a
    /// launchd unload/shutdown never orphans detached agents or their worktrees).
    fn route_kill_all(sups: &DaemonSups<Self>, state: &DaemonSpawnState, state_root: &Path);
}

#[cfg(feature = "daemon-spawn")]
impl DaemonSpawnRoutable for supervisor::Supervisor {
    fn route_spawn(
        sups: &DaemonSups<Self>,
        state: &DaemonSpawnState,
        state_root: &Path,
        spec: SpawnSpec,
    ) -> SocketResponse {
        let exec = RealSpawnExec::resolve(state_root);
        handle_spawn(sups, state, state_root, spec, &exec)
    }
    fn route_close(
        sups: &DaemonSups<Self>,
        state: &DaemonSpawnState,
        state_root: &Path,
        id: &str,
    ) -> SocketResponse {
        let exec = RealSpawnExec::resolve(state_root);
        handle_close(sups, state, state_root, id, &exec)
    }
    fn route_reap(sups: &DaemonSups<Self>, state: &DaemonSpawnState, state_root: &Path) {
        let exec = RealSpawnExec::resolve(state_root);
        reap_dead_and_expired(sups, state, state_root, &exec);
    }
    fn route_kill_all(sups: &DaemonSups<Self>, state: &DaemonSpawnState, state_root: &Path) {
        let exec = RealSpawnExec::resolve(state_root);
        kill_all(sups, state, state_root, &exec);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    // ── test pane double + exec double ──

    /// A cheap daemon-pane stand-in: a real `Supervisor` owns a spawned PTY and can't be
    /// built in a unit test, so this records kills + exposes liveness/harness/child_pid —
    /// the exact [`DaemonPane`] surface, so the whole spawn lifecycle runs against
    /// `DaemonSups<FakePane>`.
    #[derive(Clone)]
    struct FakePane {
        harness: &'static str,
        alive: Arc<std::sync::atomic::AtomicBool>,
        killed: Arc<std::sync::atomic::AtomicBool>,
        pid: Option<u32>,
    }
    impl FakePane {
        fn new(harness: &'static str) -> Self {
            FakePane {
                harness,
                alive: Arc::new(std::sync::atomic::AtomicBool::new(true)),
                killed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                pid: Some(4242),
            }
        }
        #[allow(dead_code)] // test-fixture constructor kept for future dead-pane cases
        fn dead(harness: &'static str) -> Self {
            let p = Self::new(harness);
            p.alive.store(false, Ordering::SeqCst);
            p
        }
        fn was_killed(&self) -> bool {
            self.killed.load(Ordering::SeqCst)
        }
    }
    impl PaneWrite for FakePane {
        fn is_alive(&mut self) -> bool {
            self.alive.load(Ordering::SeqCst)
        }
        fn write(&mut self, _data: &[u8]) -> std::io::Result<()> {
            Ok(())
        }
        fn harness_wire(&self) -> &'static str {
            self.harness
        }
    }
    impl DaemonPane for FakePane {
        fn kill(&mut self) {
            self.killed.store(true, Ordering::SeqCst);
            self.alive.store(false, Ordering::SeqCst);
        }
        fn child_pid(&self) -> Option<u32> {
            self.pid
        }
    }

    /// A `SpawnExec` double: records the worktrees it removed, and lets a test force a
    /// prepare/spawn error or a reused-dirty worktree. Never touches git or a PTY.
    struct FakeExec {
        git_root: PathBuf,
        reused: bool,
        dirty: bool,
        prepare_err: Option<String>,
        spawn_err: Option<String>,
        has_worktree: bool,
        // mismatched root to drive the C2 sanctioned-path reject (None → use sanctioned).
        root_override: Option<PathBuf>,
        removed: Arc<Mutex<Vec<String>>>,
        pane_harness: &'static str,
    }
    impl FakeExec {
        fn ok() -> Self {
            FakeExec {
                git_root: PathBuf::from("/repo"),
                reused: false,
                dirty: false,
                prepare_err: None,
                spawn_err: None,
                has_worktree: true,
                root_override: None,
                removed: Arc::new(Mutex::new(Vec::new())),
                pane_harness: "claude",
            }
        }
        fn removed_ids(&self) -> Vec<String> {
            self.removed.lock().unwrap().clone()
        }
    }
    impl SpawnExec for FakeExec {
        type Pane = FakePane;
        fn prepare_worktree(&self, _repo: &Path, id: &str) -> Result<PreparedWorktree, String> {
            if let Some(e) = &self.prepare_err {
                return Err(e.clone());
            }
            let root = self
                .root_override
                .clone()
                .unwrap_or_else(|| self.git_root.join(".agent-teams-worktrees").join(id));
            Ok(PreparedWorktree {
                cwd: root.clone(),
                root,
                git_root: self.git_root.clone(),
                has_worktree: self.has_worktree,
                reused: self.reused,
                dirty: self.dirty,
            })
        }
        fn spawn(&self, _spec: &SpawnSpec, _wt: &PreparedWorktree) -> Result<FakePane, String> {
            if let Some(e) = &self.spawn_err {
                return Err(e.clone());
            }
            Ok(FakePane::new(self.pane_harness))
        }
        fn remove_worktree(&self, _git_root: &Path, id: &str, _root: &Path) {
            self.removed.lock().unwrap().push(id.to_string());
        }
    }

    // ── temp state_root + config helpers ──

    struct Scratch {
        root: PathBuf,
        state: PathBuf,
    }
    impl Scratch {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "at-spawn-{}-{}-{}",
                tag,
                std::process::id(),
                now_millis()
            ));
            let _ = std::fs::remove_dir_all(&root);
            let state = root.join("state");
            std::fs::create_dir_all(&state).unwrap();
            Scratch { root, state }
        }
        /// Write the mcp-config sibling with the two Q4-relevant gates.
        fn set_gates(&self, allow_mutations: bool, daemon_spawn_enabled: bool) {
            let p = agent_teams_core::mcp_config_path(&self.state).unwrap();
            std::fs::write(
                &p,
                format!(
                    r#"{{"allow_mutations":{allow_mutations},"daemon_spawn_enabled":{daemon_spawn_enabled}}}"#
                ),
            )
            .unwrap();
        }
        fn state_root(&self) -> &Path {
            &self.state
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn spec(id: &str) -> SpawnSpec {
        SpawnSpec {
            id: id.into(),
            harness: "claude".into(),
            repo: "/repo".into(),
            session_id: None,
            resume: false,
            role: None,
            is_worker: false,
            extra_dirs: vec![],
            model: None,
            fresh_from_main: false,
            require_worktree: false,
        }
    }

    // ── gate model (decision 1) ──

    #[test]
    fn gate_off_returns_spawn_disabled_and_spawns_nothing() {
        // daemon_spawn_enabled=false → SPAWN_DISABLED; the map stays empty and no worktree
        // op happens (the gate fires before prepare_worktree).
        let s = Scratch::new("gate-off");
        s.set_gates(true, false); // mutations on, daemon-spawn OFF → still refused
        let sups: DaemonSups<FakePane> = DaemonSups::new();
        let state = DaemonSpawnState::new();
        let exec = FakeExec::ok();
        let r = handle_spawn(&sups, &state, s.state_root(), spec("ws-1"), &exec);
        assert!(!r.ok);
        assert_eq!(r.code, response_code::SPAWN_DISABLED);
        assert_eq!(sups.count_live(), 0, "nothing inserted");
        assert!(
            exec.removed_ids().is_empty(),
            "no worktree op on a gated-off spawn"
        );
        assert_eq!(
            state.pending_count(),
            0,
            "in-flight hold released on every exit"
        );
    }

    #[test]
    fn double_gate_only_proceeds_with_both_on() {
        // The dedicated gate is the ONLY one checked inside handle_spawn (allow_mutations is
        // enforced upstream in the accept loop); with daemon-spawn ON the spawn proceeds.
        let s = Scratch::new("double-gate");
        s.set_gates(false, true); // allow_mutations is the accept-loop's job; the dedicated gate is ON
        let sups: DaemonSups<FakePane> = DaemonSups::new();
        let state = DaemonSpawnState::new();
        let exec = FakeExec::ok();
        let r = handle_spawn(&sups, &state, s.state_root(), spec("ws-1"), &exec);
        assert!(r.ok, "dedicated gate ON → proceeds: {r:?}");
        assert_eq!(sups.count_live(), 1);
        // and with the dedicated gate OFF it is refused regardless.
        s.set_gates(true, false);
        let r2 = handle_spawn(&sups, &state, s.state_root(), spec("ws-2"), &exec);
        assert_eq!(r2.code, response_code::SPAWN_DISABLED);
        assert_eq!(
            sups.count_live(),
            1,
            "the refused second spawn added nothing"
        );
    }

    // ── re-validation rejections (WHAT) ──

    #[test]
    fn is_worker_true_over_the_wire_is_rejected() {
        let s = Scratch::new("is-worker");
        s.set_gates(true, true);
        let sups: DaemonSups<FakePane> = DaemonSups::new();
        let state = DaemonSpawnState::new();
        let exec = FakeExec::ok();
        let mut sp = spec("ws-1");
        sp.is_worker = true;
        let r = handle_spawn(&sups, &state, s.state_root(), sp, &exec);
        assert!(!r.ok);
        assert_eq!(r.code, response_code::SPAWN_REJECTED);
        assert_eq!(
            sups.count_live(),
            0,
            "a worker spawn over the wire inserts nothing"
        );
    }

    #[test]
    fn bad_id_is_rejected_before_any_worktree_touch() {
        let s = Scratch::new("bad-id");
        s.set_gates(true, true);
        let sups: DaemonSups<FakePane> = DaemonSups::new();
        let state = DaemonSpawnState::new();
        let exec = FakeExec::ok();
        for bad in ["../escape", "a/b", "has space", ""] {
            let r = handle_spawn(&sups, &state, s.state_root(), spec(bad), &exec);
            assert_eq!(
                r.code,
                response_code::SPAWN_REJECTED,
                "id {bad:?} must be rejected"
            );
        }
        assert!(
            exec.removed_ids().is_empty(),
            "no worktree op for a rejected id"
        );
        assert_eq!(sups.count_live(), 0);
    }

    #[test]
    fn unknown_harness_is_rejected() {
        let s = Scratch::new("bad-harness");
        s.set_gates(true, true);
        let sups: DaemonSups<FakePane> = DaemonSups::new();
        let state = DaemonSpawnState::new();
        let exec = FakeExec::ok();
        let mut sp = spec("ws-1");
        sp.harness = "not-a-harness".into();
        let r = handle_spawn(&sups, &state, s.state_root(), sp, &exec);
        assert_eq!(r.code, response_code::SPAWN_REJECTED);
    }

    #[test]
    fn out_of_scope_extra_dir_is_rejected() {
        let s = Scratch::new("extra-dir");
        s.set_gates(true, true);
        let sups: DaemonSups<FakePane> = DaemonSups::new();
        let state = DaemonSpawnState::new();
        let exec = FakeExec::ok();
        let mut sp = spec("ws-1");
        // /etc is outside the repo /repo → rejected (decision 2 / C3).
        sp.extra_dirs = vec!["/etc".into()];
        let r = handle_spawn(&sups, &state, s.state_root(), sp, &exec);
        assert_eq!(r.code, response_code::SPAWN_REJECTED);
        assert_eq!(sups.count_live(), 0);
    }

    #[test]
    fn force_fresh_is_unrepresentable_and_dirty_reuse_returns_uncommitted_work() {
        // decision 5 / C5: there is no force_fresh field, and a reused+dirty worktree with
        // fresh_from_main set returns UNCOMMITTED_WORK — the destructive freshen is NEVER
        // run (the FakeExec has no freshen op; the reused tree is left intact, not removed).
        let s = Scratch::new("uncommitted");
        s.set_gates(true, true);
        let sups: DaemonSups<FakePane> = DaemonSups::new();
        let state = DaemonSpawnState::new();
        let mut exec = FakeExec::ok();
        exec.reused = true;
        exec.dirty = true;
        let mut sp = spec("ws-1");
        sp.fresh_from_main = true;
        let r = handle_spawn(&sups, &state, s.state_root(), sp, &exec);
        assert_eq!(r.code, response_code::UNCOMMITTED_WORK);
        assert!(
            r.detail.starts_with("UNCOMMITTED_WORK:ws-1:"),
            "sentinel detail: {}",
            r.detail
        );
        assert_eq!(
            sups.count_live(),
            0,
            "no pane spawned over uncommitted work"
        );
        assert!(
            exec.removed_ids().is_empty(),
            "the reused dirty tree is left intact, never freshened/removed"
        );
    }

    #[test]
    fn sanctioned_path_mismatch_is_rejected_and_rolls_back() {
        // C2: a worktree root that is NOT the sanctioned per-id path is refused, and the
        // unexpected worktree is removed (no leak).
        let s = Scratch::new("c2-path");
        s.set_gates(true, true);
        let sups: DaemonSups<FakePane> = DaemonSups::new();
        let state = DaemonSpawnState::new();
        let mut exec = FakeExec::ok();
        exec.root_override = Some(PathBuf::from("/repo/.agent-teams-worktrees/SOMEONE-ELSE"));
        let r = handle_spawn(&sups, &state, s.state_root(), spec("ws-1"), &exec);
        assert_eq!(r.code, response_code::SPAWN_REJECTED);
        assert_eq!(
            exec.removed_ids(),
            vec!["ws-1".to_string()],
            "mismatched worktree rolled back"
        );
        assert_eq!(sups.count_live(), 0);
    }

    #[test]
    fn require_worktree_unmet_is_rejected() {
        let s = Scratch::new("require-wt");
        s.set_gates(true, true);
        let sups: DaemonSups<FakePane> = DaemonSups::new();
        let state = DaemonSpawnState::new();
        let mut exec = FakeExec::ok();
        exec.has_worktree = false;
        let mut sp = spec("ws-1");
        sp.require_worktree = true;
        let r = handle_spawn(&sups, &state, s.state_root(), sp, &exec);
        assert_eq!(r.code, response_code::SPAWN_REJECTED);
    }

    #[test]
    fn spawn_error_rolls_back_the_worktree() {
        let s = Scratch::new("spawn-err");
        s.set_gates(true, true);
        let sups: DaemonSups<FakePane> = DaemonSups::new();
        let state = DaemonSpawnState::new();
        let mut exec = FakeExec::ok();
        exec.spawn_err = Some("pty open failed".into());
        let r = handle_spawn(&sups, &state, s.state_root(), spec("ws-1"), &exec);
        assert_eq!(r.code, response_code::SPAWN_REJECTED);
        assert_eq!(
            exec.removed_ids(),
            vec!["ws-1".to_string()],
            "worktree removed on spawn error (no leak)"
        );
        assert_eq!(sups.count_live(), 0);
    }

    // ── reject-over-live-id (D5) ──

    #[test]
    fn reject_over_live_id_does_not_kill_the_existing_pane() {
        let s = Scratch::new("already-live");
        s.set_gates(true, true);
        let sups: DaemonSups<FakePane> = DaemonSups::new();
        let state = DaemonSpawnState::new();
        let existing = FakePane::new("claude");
        sups.insert("ws-1", existing.clone());
        let exec = FakeExec::ok();
        let r = handle_spawn(&sups, &state, s.state_root(), spec("ws-1"), &exec);
        assert_eq!(r.code, response_code::ALREADY_LIVE);
        assert_eq!(sups.count_live(), 1, "the existing pane stays");
        assert!(
            !existing.was_killed(),
            "reject-over-live-id must NOT kill the running pane"
        );
    }

    // ── anti-double-spawn TOCTOU (D5 hardening) + flag-injection (C6) ──

    #[test]
    fn concurrent_same_id_spawn_is_rejected_not_double_spawned() {
        // While one spawn HOLDS the per-id claim (the window between the liveness check and the
        // map insert), a SECOND `Spawn{id}` for the same id must be refused ALREADY_LIVE — never
        // fork a second child or kill the first. This is the TOCTOU the per-connection-thread
        // model opened (`contains` and `insert` are separate map locks); the claim closes it.
        let s = Scratch::new("double-spawn");
        s.set_gates(true, true);
        let sups: DaemonSups<FakePane> = DaemonSups::new();
        let state = DaemonSpawnState::new();
        let exec = FakeExec::ok();
        // Hold the claim for "ws-1" (models the in-flight first spawn mid-fork/exec).
        let held = state
            .claim_spawn_id("ws-1", &sups)
            .expect("first claim succeeds");
        let r = handle_spawn(&sups, &state, s.state_root(), spec("ws-1"), &exec);
        assert_eq!(r.code, response_code::ALREADY_LIVE);
        assert_eq!(sups.count_live(), 0, "the racing spawn inserted no child");
        assert!(
            exec.removed_ids().is_empty(),
            "no worktree op for the racing spawn"
        );
        // Once the first spawn's claim releases, a fresh Spawn for ws-1 proceeds.
        drop(held);
        let r2 = handle_spawn(&sups, &state, s.state_root(), spec("ws-1"), &exec);
        assert!(r2.ok, "{r2:?}");
        assert_eq!(sups.count_live(), 1);
    }

    #[test]
    fn flag_injection_session_id_and_model_are_rejected() {
        // C6: session_id / model flow VERBATIM into the harness argv. A leading-`-` value on
        // claude's optional-value `--resume` path (or `--model`) injects a standalone flag
        // (`--resume --dangerously-skip-permissions` → permission-bypass), nullifying the C3
        // repo-scope. The daemon re-validates both fields and rejects the injection.
        let s = Scratch::new("flag-inj");
        s.set_gates(true, true);
        let sups: DaemonSups<FakePane> = DaemonSups::new();
        let state = DaemonSpawnState::new();
        let exec = FakeExec::ok();
        let mut sp = spec("ws-1");
        sp.session_id = Some("--dangerously-skip-permissions".into());
        let r = handle_spawn(&sups, &state, s.state_root(), sp, &exec);
        assert_eq!(
            r.code,
            response_code::SPAWN_REJECTED,
            "leading-'-' session_id rejected"
        );
        assert_eq!(sups.count_live(), 0);
        let mut sp2 = spec("ws-2");
        sp2.model = Some("--dangerously-skip-permissions".into());
        let r2 = handle_spawn(&sups, &state, s.state_root(), sp2, &exec);
        assert_eq!(
            r2.code,
            response_code::SPAWN_REJECTED,
            "leading-'-' model rejected"
        );
        assert_eq!(sups.count_live(), 0);
        // A legit UUID session_id + a real model id proceed (no false-positive).
        let mut sp3 = spec("ws-3");
        sp3.session_id = Some("3f2504e0-4f89-41d3-9a0c-0305e82c3301".into());
        sp3.model = Some("claude-haiku-4-5".into());
        let r3 = handle_spawn(&sups, &state, s.state_root(), sp3, &exec);
        assert!(r3.ok, "{r3:?}");
        assert_eq!(sups.count_live(), 1);
    }

    // ── max-cap (decision 4) ──

    #[test]
    fn cap_exceeded_when_at_max_panes() {
        let s = Scratch::new("cap");
        s.set_gates(true, true);
        let sups: DaemonSups<FakePane> = DaemonSups::new();
        let state = DaemonSpawnState::new();
        for i in 0..MAX_DAEMON_PANES {
            sups.insert(format!("pre-{i}"), FakePane::new("claude"));
        }
        let exec = FakeExec::ok();
        let r = handle_spawn(&sups, &state, s.state_root(), spec("over-cap"), &exec);
        assert_eq!(r.code, response_code::CAP_EXCEEDED);
        assert_eq!(
            sups.count_live(),
            MAX_DAEMON_PANES,
            "cap holds, nothing added"
        );
    }

    // ── happy path: insert + registry + child-pid + worktree record ──

    #[test]
    fn happy_path_inserts_records_pid_worktree_and_writes_registry() {
        let s = Scratch::new("happy");
        s.set_gates(true, true);
        let sups: DaemonSups<FakePane> = DaemonSups::new();
        let state = DaemonSpawnState::new();
        let exec = FakeExec::ok();
        let r = handle_spawn(&sups, &state, s.state_root(), spec("ws-1"), &exec);
        assert!(r.ok, "{r:?}");
        assert_eq!(sups.count_live(), 1);
        // child pid captured at spawn → in the registry map.
        assert_eq!(state.child_pid_map().get("ws-1"), Some(&4242));
        // live registry written with the daemon pid + child pid.
        let reg = agent_teams_core::read_registry(s.state_root()).expect("registry written");
        assert_eq!(reg.app_pid, Some(std::process::id()));
        assert_eq!(
            reg.workspaces
                .iter()
                .find(|w| w.id == "ws-1")
                .and_then(|w| w.pid),
            Some(4242)
        );
        // durable worktree registry mirror written.
        let wt_path = DaemonSpawnState::worktrees_path(s.state_root()).unwrap();
        assert!(wt_path.exists(), "durable worktree registry persisted");
    }

    // ── close ──

    #[test]
    fn close_kills_removes_worktree_and_is_idempotent() {
        let s = Scratch::new("close");
        s.set_gates(true, true);
        let sups: DaemonSups<FakePane> = DaemonSups::new();
        let state = DaemonSpawnState::new();
        let exec = FakeExec::ok();
        assert!(handle_spawn(&sups, &state, s.state_root(), spec("ws-1"), &exec).ok);
        let pane = sups.with_snapshot("ws-1", |p| p.clone()).unwrap();
        let r = handle_close(&sups, &state, s.state_root(), "ws-1", &exec);
        assert!(r.ok);
        assert_eq!(sups.count_live(), 0);
        assert!(pane.was_killed(), "close kills the child");
        assert_eq!(
            exec.removed_ids(),
            vec!["ws-1".to_string()],
            "close removes the worktree"
        );
        assert!(
            !state.child_pid_map().contains_key("ws-1"),
            "child-pid dropped"
        );
        // idempotent-OK on an absent id.
        let r2 = handle_close(&sups, &state, s.state_root(), "ghost", &exec);
        assert!(r2.ok, "closing an absent id is a no-op OK");
    }

    #[test]
    fn close_during_inflight_spawn_returns_close_pending_not_false_ok() {
        // A `Close` that races a still-in-flight `Spawn` on the same id must NOT answer
        // ok("closed") — the caller would drop its anchor while the spawn lands a live pane
        // moments later. It answers the distinct CLOSE_PENDING busy error instead.
        let s = Scratch::new("close-pending");
        s.set_gates(true, true);
        let sups: DaemonSups<FakePane> = DaemonSups::new();
        let state = DaemonSpawnState::new();
        let exec = FakeExec::ok();
        // Model the in-flight spawn holding the per-id claim.
        let held = state.claim_spawn_id("ws-1", &sups).expect("claim");
        let r = handle_close(&sups, &state, s.state_root(), "ws-1", &exec);
        assert!(!r.ok, "in-flight id must not be falsely closed: {r:?}");
        assert_eq!(r.code, response_code::CLOSE_PENDING);
        // Once the spawn settles (claim released), Close works normally.
        drop(held);
        assert!(handle_spawn(&sups, &state, s.state_root(), spec("ws-1"), &exec).ok);
        let r2 = handle_close(&sups, &state, s.state_root(), "ws-1", &exec);
        assert!(r2.ok, "{r2:?}");
        assert_eq!(sups.count_live(), 0);
    }

    // ── cold-start orphan sweep (crash recovery) ──

    /// Spawn a throwaway `sleep 60` child and return it (the "orphaned agent" stand-in).
    fn spawn_sleeper() -> std::process::Child {
        std::process::Command::new("sleep")
            .arg("60")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn sleeper")
    }

    /// A pid guaranteed dead: spawn `true`, wait it out, return its pid.
    fn dead_pid() -> u32 {
        let mut c = std::process::Command::new("true")
            .spawn()
            .expect("spawn true");
        let pid = c.id();
        let _ = c.wait();
        pid
    }

    fn write_prior_registry(state_root: &Path, app_pid: Option<u32>, panes: &[(&str, u32)]) {
        let reg = agent_teams_core::LiveRegistry {
            schema: agent_teams_core::LIVE_REGISTRY_SCHEMA,
            app_pid,
            updated_at: Some(now_millis()),
            active: None,
            workspaces: panes
                .iter()
                .map(|(id, pid)| agent_teams_core::LiveWorkspace {
                    id: id.to_string(),
                    pid: Some(*pid),
                    harness: None,
                    repo: None,
                    spawned_at: None,
                    role: None,
                    session_id: None,
                    tag: None,
                })
                .collect(),
        };
        let path = agent_teams_core::registry_path(state_root).unwrap();
        std::fs::write(&path, serde_json::to_string(&reg).unwrap()).unwrap();
    }

    #[test]
    fn cold_start_sweep_kills_orphans_and_clears_both_files() {
        let s = Scratch::new("cold-sweep");
        // A crashed prior daemon: its app_pid is DEAD, its child is still alive (orphan).
        let mut orphan = spawn_sleeper();
        write_prior_registry(
            s.state_root(),
            Some(dead_pid()),
            &[("ws-orphan", orphan.id())],
        );
        // …and it left a durable worktree record.
        let wt_path = DaemonSpawnState::worktrees_path(s.state_root()).unwrap();
        let mut records = HashMap::new();
        records.insert(
            "ws-orphan".to_string(),
            WorktreeRecord {
                git_root: s.root.join("no-such-repo"),
                root: s.root.join("no-such-repo/.agent-teams-worktrees/ws-orphan"),
                spawned_at: now_millis(),
            },
        );
        std::fs::write(&wt_path, serde_json::to_string(&records).unwrap()).unwrap();

        cold_start_sweep(s.state_root());

        // the orphaned child was SIGKILLed (reaped within a bounded wait).
        let mut reaped = false;
        for _ in 0..40 {
            if orphan.try_wait().unwrap().is_some() {
                reaped = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(reaped, "orphaned child killed by the cold-start sweep");
        // both files cleared: the trail is gone, the registry is empty.
        assert!(!wt_path.exists(), "durable worktree trail consumed");
        let reg = agent_teams_core::read_registry(s.state_root()).expect("registry rewritten");
        assert!(
            reg.workspaces.is_empty() && reg.app_pid.is_none(),
            "registry cleared"
        );
    }

    #[test]
    fn cold_start_sweep_never_kills_panes_of_a_live_writer() {
        let s = Scratch::new("cold-sweep-live");
        // The prior registry's writer is STILL ALIVE (this test process) → its panes are
        // NOT orphans; the sweep must not kill them (they belong to a running GUI/daemon).
        let mut child = spawn_sleeper();
        write_prior_registry(
            s.state_root(),
            Some(std::process::id()),
            &[("ws-live", child.id())],
        );

        cold_start_sweep(s.state_root());

        std::thread::sleep(Duration::from_millis(200));
        assert!(
            child.try_wait().unwrap().is_none(),
            "live writer's pane NOT killed"
        );
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn cold_start_sweep_does_not_clear_a_live_writers_registry() {
        // R3 fix: `clear_live_registry` moved INSIDE the `!writer_alive` guard. A daemon
        // cold-start while the GUI (this process) is ALIVE must LEAVE the shared registry
        // intact — clearing it would erase the live GUI's panes.
        let s = Scratch::new("cold-sweep-preserve");
        let mut child = spawn_sleeper();
        write_prior_registry(
            s.state_root(),
            Some(std::process::id()), // writer alive
            &[("ws-live", child.id())],
        );

        cold_start_sweep(s.state_root());

        let reg = agent_teams_core::read_registry(s.state_root())
            .expect("registry must NOT be cleared under a live writer");
        let ids: Vec<&str> = reg.workspaces.iter().map(|w| w.id.as_str()).collect();
        assert!(
            ids.contains(&"ws-live") && reg.app_pid == Some(std::process::id()),
            "the live GUI's registry entries survive the daemon cold-start: {ids:?}"
        );
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn cold_start_sweep_keeps_a_corrupt_worktree_trail() {
        // R3 fix: a trail that FAILS to parse must NOT be deleted — it is the sole
        // crash-reclaim record for worktrees the sweep could not parse+remove.
        let s = Scratch::new("cold-sweep-corrupt-trail");
        // Dead prior writer so the sweep runs its full body.
        write_prior_registry(s.state_root(), Some(dead_pid()), &[]);
        let wt_path = DaemonSpawnState::worktrees_path(s.state_root()).unwrap();
        std::fs::write(&wt_path, "{ not valid json at all").unwrap();

        cold_start_sweep(s.state_root());

        assert!(
            wt_path.exists(),
            "a corrupt worktree trail must be LEFT for later reclaim, not silently deleted"
        );
        assert_eq!(
            std::fs::read_to_string(&wt_path).unwrap(),
            "{ not valid json at all",
            "the trail bytes are preserved untouched"
        );

        // A CLEAN (parseable, empty-map) trail is still consumed as before.
        std::fs::write(&wt_path, "{}").unwrap();
        cold_start_sweep(s.state_root());
        assert!(
            !wt_path.exists(),
            "a clean trail is still deleted after the sweep"
        );
    }

    // ── reaper (D1/D2) ──

    #[test]
    fn reaper_removes_dead_panes_and_rewrites_the_registry() {
        let s = Scratch::new("reaper");
        s.set_gates(true, true);
        let sups: DaemonSups<FakePane> = DaemonSups::new();
        let state = DaemonSpawnState::new();
        let exec = FakeExec::ok();
        // two live spawns, then one dies.
        assert!(handle_spawn(&sups, &state, s.state_root(), spec("live"), &exec).ok);
        assert!(handle_spawn(&sups, &state, s.state_root(), spec("doomed"), &exec).ok);
        sups.with_mut("doomed", |p| p.alive.store(false, Ordering::SeqCst));
        // sweep at now=spawn-time so NOTHING is TTL-expired; only the dead one is reaped.
        reaper_sweep(
            &sups,
            &state,
            s.state_root(),
            &exec,
            now_millis(),
            DAEMON_PANE_TTL,
        );
        assert_eq!(sups.count_live(), 1);
        assert!(sups.contains("live") && !sups.contains("doomed"));
        // the registry no longer lists the reaped id.
        let reg = agent_teams_core::read_registry(s.state_root()).unwrap();
        let ids: Vec<&str> = reg.workspaces.iter().map(|w| w.id.as_str()).collect();
        assert!(
            ids.contains(&"live") && !ids.contains(&"doomed"),
            "registry rewritten: {ids:?}"
        );
        assert!(
            exec.removed_ids().contains(&"doomed".to_string()),
            "dead pane's worktree removed"
        );
    }

    #[test]
    fn reaper_kills_and_removes_ttl_expired_live_panes() {
        let s = Scratch::new("ttl");
        s.set_gates(true, true);
        let sups: DaemonSups<FakePane> = DaemonSups::new();
        let state = DaemonSpawnState::new();
        let exec = FakeExec::ok();
        assert!(handle_spawn(&sups, &state, s.state_root(), spec("old"), &exec).ok);
        let pane = sups.with_snapshot("old", |p| p.clone()).unwrap();
        assert!(
            sups.with_mut("old", |p| p.is_alive()).unwrap(),
            "still alive pre-sweep"
        );
        // sweep with a now FAR past the pane's spawn time and a tiny TTL → expired.
        reaper_sweep(
            &sups,
            &state,
            s.state_root(),
            &exec,
            now_millis() + 10_000,
            Duration::from_millis(1),
        );
        assert_eq!(sups.count_live(), 0, "TTL-expired pane removed");
        assert!(pane.was_killed(), "an expired-but-alive pane is killed");
        assert!(exec.removed_ids().contains(&"old".to_string()));
    }

    #[test]
    fn reaper_defers_an_id_held_by_a_concurrent_claim() {
        // Reaper-vs-respawn race: if a Spawn/Close holds the per-id claim, the reaper must NOT
        // remove that id's pane + worktree out from under it (else it `git worktree remove`s a
        // just-respawned live pane's cwd). It defers to a later sweep.
        let s = Scratch::new("reaper-claim");
        s.set_gates(true, true);
        let sups: DaemonSups<FakePane> = DaemonSups::new();
        let state = DaemonSpawnState::new();
        let exec = FakeExec::ok();
        assert!(handle_spawn(&sups, &state, s.state_root(), spec("ws-1"), &exec).ok);
        sups.with_mut("ws-1", |p| p.alive.store(false, Ordering::SeqCst)); // looks dead
                                                                           // Hold the per-id claim (models a concurrent spawn/close mid-flight for ws-1).
        let held = state.try_claim_id("ws-1").expect("claim a free id");
        reaper_sweep(
            &sups,
            &state,
            s.state_root(),
            &exec,
            now_millis(),
            DAEMON_PANE_TTL,
        );
        assert_eq!(sups.count_live(), 1, "a claimed id is NOT reaped");
        assert!(
            exec.removed_ids().is_empty(),
            "a claimed id's worktree is NOT removed"
        );
        // Once the claim releases, the next sweep reaps the dead pane normally.
        drop(held);
        reaper_sweep(
            &sups,
            &state,
            s.state_root(),
            &exec,
            now_millis(),
            DAEMON_PANE_TTL,
        );
        assert_eq!(
            sups.count_live(),
            0,
            "dead pane reaped after the claim releases"
        );
        assert!(exec.removed_ids().contains(&"ws-1".to_string()));
    }

    // ── kill-all (decision 4) ──

    #[test]
    fn kill_all_empties_the_map_and_clears_both_registries() {
        let s = Scratch::new("kill-all");
        s.set_gates(true, true);
        let sups: DaemonSups<FakePane> = DaemonSups::new();
        let state = DaemonSpawnState::new();
        let exec = FakeExec::ok();
        for id in ["a", "b", "c"] {
            assert!(handle_spawn(&sups, &state, s.state_root(), spec(id), &exec).ok);
        }
        let panes: Vec<FakePane> = ["a", "b", "c"]
            .iter()
            .map(|id| sups.with_snapshot(id, |p| p.clone()).unwrap())
            .collect();
        kill_all(&sups, &state, s.state_root(), &exec);
        assert_eq!(sups.count_live(), 0, "kill_all empties the map");
        assert!(panes.iter().all(|p| p.was_killed()), "every child killed");
        assert!(state.child_pid_map().is_empty(), "child-pid map cleared");
        // live registry cleared (no live workspaces, no app_pid).
        let reg = agent_teams_core::read_registry(s.state_root()).unwrap();
        assert!(reg.workspaces.is_empty() && reg.app_pid.is_none());
        let mut removed = exec.removed_ids();
        removed.sort();
        assert_eq!(
            removed,
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }

    // ── in-flight hold (D4) ──

    #[test]
    fn pending_counter_is_released_on_every_exit_path() {
        let s = Scratch::new("pending");
        let sups: DaemonSups<FakePane> = DaemonSups::new();
        let state = DaemonSpawnState::new();
        let exec = FakeExec::ok();
        // gated-off exit, rejected exit, and a happy exit must all leave pending at 0.
        s.set_gates(true, false);
        let _ = handle_spawn(&sups, &state, s.state_root(), spec("ws-1"), &exec);
        assert_eq!(state.pending_count(), 0);
        s.set_gates(true, true);
        let mut bad = spec("ws-2");
        bad.is_worker = true;
        let _ = handle_spawn(&sups, &state, s.state_root(), bad, &exec);
        assert_eq!(state.pending_count(), 0);
        let _ = handle_spawn(&sups, &state, s.state_root(), spec("ws-3"), &exec);
        assert_eq!(
            state.pending_count(),
            0,
            "happy path also releases the hold"
        );
    }
}
