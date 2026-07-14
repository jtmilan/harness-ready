//! Phase 14 / item 4b + the agent-write ENABLEMENT slice: the **task_\*** MCP
//! tools, wired into the sidecar **inert-first** (a SEPARATE `task_tool_router`
//! merged in `new()` only under `#[cfg(feature = "task-tools")]`, mirroring the
//! verified memory-notes / Phase-B router pattern) and **ungated** (pure file I/O
//! over `core/task`, the `runs.jsonl` / memory-notes posture — these NEVER call
//! `read_mcp_config().allow_mutations`; the task store is OFF the PTY / Model-A
//! mutation axis, so the right controls are provenance / append-integrity /
//! validate-then-append / scope, not the `allow_mutations` gate — D57).
//!
//! ## What a pane agent can do here
//! - `task_list` / `task_get` (READ) — the board, each row folded with its
//!   `current_lifecycle` from the append-only transition log. The view UNIONS the
//!   operator-owned mutable store with the agent-created (log-only) tasks.
//! - `task_create {title}` (WRITE) — server-mints an id and APPENDS a genesis
//!   `Created` transition carrying the title to the **append-only log** (the
//!   mutable store is NOT touched).
//! - `task_transition {id, to}` (WRITE) — advances ITS task along the legal
//!   lifecycle graph; an illegal edge is REJECTED over the wire (nothing appended).
//!
//! ## Security (threat-model `.paul/analysis/bridgeswarm-agent-write-threat-model.md`)
//! Ungated ≠ unprotected. This layer is the tool boundary for the pane-write task
//! surface, so it bakes in:
//! - **(C2) server-minted ids** — `task_create` mints `task_<ms>_<pid>_<ctr>`
//!   (mirror `core/memory::mint_id`); the agent NEVER supplies the id. The id is a
//!   logical key, never a filesystem path (the log file path derives solely from
//!   `state_root`), so a `../`-laden id supplied to `task_transition` selects no
//!   file and escapes nothing — inert by construction.
//! - **(C2) immutable, server-set provenance** — every transition stamps an
//!   [`Actor`] read from `$AGENT_TEAMS_PANE_ID` (supervisor-set at spawn, NOT an
//!   agent tool arg) as `Actor::Pane { workspace_id }`; `core/task` stays HERMETIC
//!   (it takes the actor as an arg). Unset env ⇒ a stable `unknown` sentinel so the
//!   line is still attributable. NOTE: the spawn-set pane id is a same-user
//!   forgeable HINT, not attestation — a consumer must not treat it as authenticated.
//! - **(C4 — CLOSED) creation is append-only.** `task_create` routes the title onto
//!   the genesis log line via `core/task::create_task`; it NEVER calls the racy
//!   full-file `upsert_task`. Title + lifecycle of an agent-created task both live
//!   in the concurrency-safe append-only log (single-line `O_APPEND`, no
//!   read-modify-write), so N concurrent creates never lose one. (The mutable
//!   `tasks.jsonl` stays the operator/Tauri kanban writer; agents never write it.)
//! - **(C5) writes go through `core/task::transition` / `create_task`** —
//!   validate-then-append: `valid_transition(from, to)` is checked FIRST; an illegal
//!   edge returns a structured error and appends NOTHING. The append-only log IS the
//!   audit trail.
//! - **(C6) quotas** — `create_task` enforces a genesis-count cap ([`MAX_TASKS`]);
//!   `task_transition` enforces a per-task transition cap ([`MAX_TRANSITIONS_PER_TASK`],
//!   bounding the `Doing↔Review` cycle so a looping/injected agent can't fill the
//!   disk or quadratically degrade reads); the title byte cap is enforced in
//!   `core/task::create_task`.
//! - **(4c) ownership scope** — when `$AGENT_TEAMS_TASK_SCOPE` is set (supervisor
//!   pins it to the pane's own id at spawn), `task_transition` permits advancing
//!   ONLY a task this pane authored (the genesis `Actor::Pane{workspace_id}` must
//!   equal the scope). Unset ⇒ unrestricted (operator / dev / back-compat). This
//!   REDUCES casual cross-pane T4 (it is a same-user-forgeable guardrail, like
//!   provenance — not an authenticated boundary). Concretely forgeable: a CURSOR
//!   pane's injected MCP config (`.cursor/mcp.json`) lives IN its agent-writable
//!   worktree, so a determined pane could rewrite its own scope/repo-key and
//!   re-spawn the sidecar. The guardrail bounds accidents and honest-but-injected
//!   agents, not a same-user attacker who controls the shell.
//! - **(AC7)** there is NO `status` / `state` field on `Task`, and the Args reject
//!   any unknown field (`deny_unknown_fields`) — a client cannot smuggle a `status`
//!   key (`rank()` stays the sole machine agent-state authority).
//!
//! ## Seams (stated, not hidden)
//! - **Tasks are GLOBAL** (one `tasks-log.jsonl` sibling of `state_root`), whereas
//!   memory is per-workspace (repo-keyed). So task *reads* are cross-workspace; the
//!   4c scope gates *writes* only. A future slice may partition the log per scope.
//! - **The operator kanban (09 Tier A) reads only the mutable store**, so it does
//!   NOT yet show agent-created (log-only) tasks. Surfacing them on the board (the
//!   board folding the log) is a follow-on Tauri-side projection, out of this slice.
//!
//! The whole module is compiled only under the `task-tools` feature.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_router, ErrorData};
use serde::{Deserialize, Serialize};

use agent_teams_task::{
    create_task, current_lifecycle, log_task_ids, read_tasks, read_transitions, tasks_log_path,
    tasks_path, transition, Actor, Column, Lifecycle, Task, Transition, MAX_TASKS,
    MAX_TRANSITIONS_PER_TASK,
};

use crate::TeamServer;

/// A monotonic per-process counter so two id mints within the same millisecond
/// never collide (mirrors `core/memory`'s `ID_COUNTER`).
static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// unix-ms wall clock (the ONLY clock read in this module; `core/task` is
/// clock-free and takes `at` as an arg, preserving its testability contract).
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Server-mint a task id: `task_<unix_ms>_<pid>_<counter>` (mirror
/// `core/memory::mint_id`). NEVER caller-supplied — the agent cannot smuggle a
/// path-like id, and the id is a logical key (no file path is ever derived from it).
fn mint_id() -> String {
    let c = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("task_{}_{}_{}", now_ms(), std::process::id(), c)
}

/// The provenance actor for an agent transition: `Actor::Pane { workspace_id }`
/// read from `$AGENT_TEAMS_PANE_ID` (supervisor-set at spawn, NOT an agent tool
/// arg — C2). Unset / empty ⇒ a stable `"unknown"` sentinel so the log line stays
/// attributable. This is the ONLY place `$AGENT_TEAMS_PANE_ID` is read; `core/task`
/// stays hermetic.
///
/// NOTE: the spawn-set pane id is a same-user forgeable HINT, not attestation —
/// a consumer must not treat it as authenticated.
fn pane_actor() -> Actor {
    actor_from(
        std::env::var("AGENT_TEAMS_PANE_ID")
            .ok()
            .filter(|s| !s.is_empty()),
    )
}

/// The PURE core of [`pane_actor`] (no env read) — maps an optional pane id to the
/// provenance [`Actor`]. Split out so it is unit-testable without touching the
/// process-global env var.
fn actor_from(pane_id: Option<String>) -> Actor {
    Actor::Pane {
        workspace_id: pane_id.unwrap_or_else(|| "unknown".to_string()),
    }
}

/// The ownership scope for task transitions (4c): `$AGENT_TEAMS_TASK_SCOPE`,
/// supervisor-set at spawn to the pane's OWN id. `None` (unset / empty) ⇒ NO
/// restriction (operator / dev / back-compat). When set, a pane may transition only
/// a task it authored — see [`scope_allows`]. This is the ONLY place
/// `$AGENT_TEAMS_TASK_SCOPE` is read.
fn task_scope() -> Option<String> {
    std::env::var("AGENT_TEAMS_TASK_SCOPE")
        .ok()
        .filter(|s| !s.is_empty())
}

/// The genesis AUTHOR of `task_id`: the `workspace_id` of its genesis line's
/// `Actor::Pane`, or `None` if the genesis is operator-authored / absent. PURE.
fn genesis_owner(log: &[Transition], task_id: &str) -> Option<String> {
    log.iter()
        .find(|t| t.task_id == task_id && t.from.is_none())
        .and_then(|t| match &t.by {
            Actor::Pane { workspace_id } => Some(workspace_id.clone()),
            Actor::Operator | Actor::Unknown => None,
        })
}

/// 4c authorization (PURE, testable): may a pane with `scope` transition `task_id`?
/// `scope == None` ⇒ unrestricted. Else the genesis owner must EQUAL the scope —
/// so a pane advances only a task it authored (cross-pane / operator-task / phantom
/// all rejected). A same-user-forgeable guardrail, not an authenticated boundary.
fn scope_allows(scope: Option<&str>, log: &[Transition], task_id: &str) -> bool {
    match scope {
        None => true,
        Some(s) => genesis_owner(log, task_id).as_deref() == Some(s),
    }
}

/// C6 (PURE): is the store at the genesis-count cap [`MAX_TASKS`]? `task_create`
/// rejects when true (before minting), bounding total store growth.
fn at_task_cap(log: &[Transition]) -> bool {
    log_task_ids(log).len() >= MAX_TASKS
}

/// C6 (PURE): has `task_id` hit the per-task transition cap
/// [`MAX_TRANSITIONS_PER_TASK`]? `task_transition` rejects when true, bounding the
/// `Doing↔Review` cycle so one task can't append unbounded lines.
fn at_transition_cap(log: &[Transition], task_id: &str) -> bool {
    log.iter().filter(|t| t.task_id == task_id).count() >= MAX_TRANSITIONS_PER_TASK
}

// ── object-rooted result wrappers (MCP outputSchema requires a root object) ──

/// A task as seen over the wire: the mutable record's fields + the lifecycle folded
/// from the append-only log.
///
/// This is a LOCAL view type — `core/task::Task` is serde-only (no `schemars` dep,
/// by its load-bearing dependency discipline), so we cannot embed it in a
/// `JsonSchema`-deriving wrapper. We project the fields and render the enums to their
/// stable lowercase wire strings (`column` / `lifecycle`).
#[derive(Serialize, schemars::JsonSchema)]
struct TaskView {
    /// Server-minted task id (logical key).
    id: String,
    title: String,
    /// Operator-owned kanban column wire form (`backlog`/`doing`/`review`/`done`).
    column: String,
    order: i64,
    created_at: u64,
    updated_at: u64,
    /// The agent-writable lifecycle stage folded from the transition log
    /// (`created`/`assigned`/`doing`/`review`/`done`). Orthogonal to `column` (AC7).
    lifecycle: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    workspace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pane_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_id: Option<String>,
}

impl TaskView {
    /// Build a view from a MUTABLE-STORE task (operator-owned) with its lifecycle
    /// ALREADY resolved — so a batch reader folds the log ONCE, not per task.
    fn from_task(t: &Task, lifecycle: Lifecycle) -> Self {
        TaskView {
            id: t.id.clone(),
            title: t.title.clone(),
            column: column_wire(t.column),
            order: t.order,
            created_at: t.created_at,
            updated_at: t.updated_at,
            lifecycle: lifecycle.to_string(),
            workspace_id: t.workspace_id.clone(),
            pane_id: t.pane_id.clone(),
            run_id: t.run_id.clone(),
        }
    }

    /// Build a view for a LOG-ONLY (agent-created) task from its ALREADY-folded parts
    /// (title + genesis `at` + lifecycle). Column defaults to `backlog`; these never
    /// touch the mutable store (C4), so they have no operator column/order/links.
    fn from_log_parts(id: &str, title: Option<&str>, at: u64, lifecycle: Lifecycle) -> Self {
        TaskView {
            id: id.to_string(),
            title: title.unwrap_or_default().to_string(),
            column: "backlog".to_string(),
            order: 0,
            created_at: at,
            updated_at: at,
            lifecycle: lifecycle.to_string(),
            workspace_id: None,
            pane_id: None,
            run_id: None,
        }
    }
}

/// PURE union read (testable without a runtime — the proof Option A is wired): the
/// operator-store tasks (folded with lifecycle) ∪ the agent-created (log-only)
/// tasks. The store wins on id overlap (an operator task that also has a genesis
/// line is rendered once, from the store).
///
/// Folds the log ONCE into per-task lifecycle + genesis (O(L)), so listing T tasks is
/// O(L + T), NOT O(T·L) — bounds the read amplifier (C6/T5; the task analog of
/// memory's capped `build_graph`). The caps already bound L, but a single pass keeps
/// `task_list` cheap even at the cap.
fn list_views(store_tasks: &[Task], log: &[Transition]) -> Vec<TaskView> {
    use std::collections::{BTreeMap, BTreeSet};
    // Single pass: the last `to` per task is its current lifecycle (append order is
    // causal); the FIRST genesis line per task sets (title, at).
    let mut lifecycle: BTreeMap<&str, Lifecycle> = BTreeMap::new();
    let mut genesis: BTreeMap<&str, (Option<&str>, u64)> = BTreeMap::new();
    for t in log {
        lifecycle.insert(t.task_id.as_str(), t.to);
        if t.from.is_none() {
            genesis
                .entry(t.task_id.as_str())
                .or_insert((t.title.as_deref(), t.at));
        }
    }
    let lc = |id: &str| lifecycle.get(id).copied().unwrap_or(Lifecycle::Created);

    let store_ids: BTreeSet<&str> = store_tasks.iter().map(|t| t.id.as_str()).collect();
    let mut out: Vec<TaskView> = store_tasks
        .iter()
        .map(|t| TaskView::from_task(t, lc(&t.id)))
        .collect();
    // log-only (agent-created) tasks in genesis (append) order, skipping store ids.
    for id in log_task_ids(log) {
        if !store_ids.contains(id.as_str()) {
            let (title, at) = genesis.get(id.as_str()).copied().unwrap_or((None, 0));
            out.push(TaskView::from_log_parts(&id, title, at, lc(&id)));
        }
    }
    out
}

/// PURE single-task read: the store record if present, else a log-only
/// (agent-created) task if it has a genesis line, else `None`. One task → the per-call
/// `current_lifecycle` fold (O(L)) is fine (no per-task amplifier here).
fn get_view(id: &str, store_tasks: &[Task], log: &[Transition]) -> Option<TaskView> {
    if let Some(t) = store_tasks.iter().find(|t| t.id == id) {
        return Some(TaskView::from_task(t, current_lifecycle(log, id)));
    }
    if let Some(g) = log.iter().find(|t| t.task_id == id && t.from.is_none()) {
        return Some(TaskView::from_log_parts(
            id,
            g.title.as_deref(),
            g.at,
            current_lifecycle(log, id),
        ));
    }
    None
}

/// The stable lowercase wire token for a `Column` (mirrors the crate's serde form;
/// kept local so we don't add a `Display` to the wave-0 crate from this slice).
fn column_wire(c: Column) -> String {
    match c {
        Column::Backlog => "backlog",
        Column::Doing => "doing",
        Column::Review => "review",
        Column::Done => "done",
        // A future/unknown column token from a newer writer (serde(other)) — surface it
        // honestly; the crate serializes it back as "unknown" too.
        Column::Unknown => "unknown",
    }
    .to_string()
}

#[derive(Serialize, schemars::JsonSchema)]
struct TasksResult {
    tasks: Vec<TaskView>,
}
#[derive(Serialize, schemars::JsonSchema)]
struct TaskResult {
    /// The task, or null if the id is unknown.
    task: Option<TaskView>,
}
#[derive(Serialize, schemars::JsonSchema)]
struct CreateTaskResult {
    /// The server-minted id of the new task.
    id: String,
}
#[derive(Serialize, schemars::JsonSchema)]
struct TransitionResult {
    ok: bool,
    /// The task's lifecycle wire form AFTER the transition.
    lifecycle: String,
}

// ── args ──

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListTasksArgs {}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct GetTaskArgs {
    id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct CreateTaskArgs {
    title: String,
}

/// `task_transition` args. `deny_unknown_fields` is load-bearing for AC7: a client
/// CANNOT smuggle a `status` / `state` key (there is no such field; an extra field
/// is a hard deserialize error, not silently ignored). The agent moves the
/// orthogonal `lifecycle`, never a `rank()`-derived machine status.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct TransitionTaskArgs {
    id: String,
    /// The target lifecycle stage: `assigned` / `doing` / `review` / `done`
    /// (`created` is the genesis only). An illegal edge from the current stage is
    /// rejected with a structured error and nothing is appended.
    to: String,
}

/// RAII cross-process EXCLUSIVE lock on the transition log (flock(2), advisory).
/// `task_transition` is a read-fold-validate-append: two sidecar PROCESSES folding
/// concurrently can both observe the same `from` and both append (double-transition —
/// e.g. two `doing→review` lines). The per-line `O_APPEND` only makes each APPEND
/// atomic, not the fold+validate+append SEQUENCE. Held for the whole critical
/// section; the lock releases on drop (close releases a flock).
struct LogLock(#[allow(dead_code)] std::fs::File);

impl LogLock {
    fn exclusive(path: &std::path::Path) -> std::io::Result<Self> {
        use std::os::unix::io::AsRawFd;
        let f = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(path)?;
        let rc = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) };
        if rc != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(LogLock(f))
    }
}

impl TeamServer {
    /// The mutable store path + the append-only transition log path (both siblings
    /// of `state_root`, via the wave-0 `tasks_path` / `tasks_log_path` derivations).
    /// Errors if `state_root` has no parent (cannot site the siblings).
    fn task_paths(&self) -> Result<(PathBuf, PathBuf), ErrorData> {
        let store = tasks_path(&self.state_dir);
        let log = tasks_log_path(&self.state_dir);
        match (store, log) {
            (Some(store), Some(log)) => Ok((store, log)),
            _ => Err(ErrorData::internal_error(
                "no parent for the state dir — cannot resolve the task store / log paths"
                    .to_string(),
                None,
            )),
        }
    }
}

#[tool_router(router = task_tool_router, vis = "pub(crate)")]
impl TeamServer {
    #[tool(
        name = "task_list",
        description = "List all tasks on the board, each with its current lifecycle \
            stage folded from the append-only transition log. UNIONS the operator \
            kanban store with agent-created (log-only) tasks. Read-only; works \
            whether or not the app is running. Fields per task: \
            {id, title, column, order, created_at, updated_at, lifecycle, \
            workspace_id?, pane_id?, run_id?}."
    )]
    async fn task_list(
        &self,
        Parameters(_a): Parameters<ListTasksArgs>,
    ) -> Result<Json<TasksResult>, ErrorData> {
        let (store, log_path) = self.task_paths()?;
        let log = read_transitions(&log_path);
        let tasks = list_views(&read_tasks(&store), &log);
        Ok(Json(TasksResult { tasks }))
    }

    #[tool(
        name = "task_get",
        description = "Read one task by id (with its current lifecycle folded from the \
            transition log), or null if the id is unknown. Finds operator-store and \
            agent-created (log-only) tasks alike. Read-only."
    )]
    async fn task_get(
        &self,
        Parameters(a): Parameters<GetTaskArgs>,
    ) -> Result<Json<TaskResult>, ErrorData> {
        let (store, log_path) = self.task_paths()?;
        let log = read_transitions(&log_path);
        let task = get_view(&a.id, &read_tasks(&store), &log);
        Ok(Json(TaskResult { task }))
    }

    #[tool(
        name = "task_create",
        description = "Create a task. The server MINTS the id (never caller-supplied) \
            and APPENDS the genesis `created` transition — carrying the title — to the \
            append-only log (Actor::Pane from $AGENT_TEAMS_PANE_ID). The mutable kanban \
            store is NOT touched (append-only ⇒ no lost-update race). Ungated local file \
            I/O — no PTY, no agent state, no allow_mutations gate. Title and total task \
            count are capped. Returns {id}."
    )]
    async fn task_create(
        &self,
        Parameters(a): Parameters<CreateTaskArgs>,
    ) -> Result<Json<CreateTaskResult>, ErrorData> {
        let (_store, log_path) = self.task_paths()?;

        // (C6) genesis-count cap — reject BEFORE minting so a flood appends nothing.
        let log = read_transitions(&log_path);
        if at_task_cap(&log) {
            return Err(ErrorData::invalid_params(
                format!("task_create: task count cap reached (>= {MAX_TASKS})"),
                None,
            ));
        }

        let id = mint_id();
        // (C4) title + genesis BOTH go to the append-only log via create_task; the
        // mutable store is NEVER written. The title byte cap (C6) is enforced inside
        // create_task → an oversize title appends nothing (mapped to invalid_params).
        create_task(&log_path, &id, &a.title, pane_actor(), now_ms()).map_err(|e| {
            if e.kind() == std::io::ErrorKind::InvalidData {
                ErrorData::invalid_params(format!("task_create: {e}"), None)
            } else {
                ErrorData::internal_error(format!("task_create: {e}"), None)
            }
        })?;

        Ok(Json(CreateTaskResult { id }))
    }

    #[tool(
        name = "task_transition",
        description = "Advance a task's lifecycle along the legal graph \
            (created→{assigned,doing}; assigned→doing; doing→{review,done}; \
            review→{doing,done}; done is terminal). Resolves the current stage from \
            the log, validates the edge, then APPENDS one transition line \
            (Actor::Pane from $AGENT_TEAMS_PANE_ID). An ILLEGAL edge is REJECTED \
            (structured error, nothing appended). When $AGENT_TEAMS_TASK_SCOPE is set, \
            a pane may advance only a task it authored. There is NO status/state field \
            — an unknown arg key is rejected. Returns {ok, lifecycle}."
    )]
    async fn task_transition(
        &self,
        Parameters(a): Parameters<TransitionTaskArgs>,
    ) -> Result<Json<TransitionResult>, ErrorData> {
        let to: Lifecycle =
            a.to.parse()
                .map_err(|e: String| ErrorData::invalid_params(e, None))?;
        let (_store, log_path) = self.task_paths()?;

        // CROSS-PROCESS critical section: flock the log for the whole
        // read-fold-validate-append below, so two sidecars can't both fold the same
        // `from` and double-append the same edge. Released on drop (every exit path).
        let _log_lock = LogLock::exclusive(&log_path).map_err(|e| {
            ErrorData::internal_error(format!("task_transition: cannot lock the log: {e}"), None)
        })?;

        let log = read_transitions(&log_path);

        // Existence guard (review T4 / phantom): an unknown id folds to `Created`, so
        // WITHOUT this a pane could transition a never-created id (phantom log line).
        // Require a real genesis for this id before appending.
        if !log.iter().any(|t| t.task_id == a.id) {
            return Err(ErrorData::invalid_params(
                format!("task_transition: unknown task id {:?} (no genesis)", a.id),
                None,
            ));
        }

        // (4c) ownership scope — when pinned, a pane may advance ONLY a task it
        // authored (genesis owner == scope). Rejects cross-pane, operator-task, and
        // any non-owned id. A same-user-forgeable guardrail (like provenance).
        if !scope_allows(task_scope().as_deref(), &log, &a.id) {
            return Err(ErrorData::invalid_params(
                format!(
                    "task_transition: task {:?} is outside this pane's scope \
                     (owner {:?}, scope {:?})",
                    a.id,
                    genesis_owner(&log, &a.id),
                    task_scope()
                ),
                None,
            ));
        }

        // (C6) per-task transition cap — bounds the Doing↔Review cycle so one task
        // can't append unbounded lines (disk + per-read fold amplifier).
        if at_transition_cap(&log, &a.id) {
            return Err(ErrorData::invalid_params(
                format!(
                    "task_transition: task {:?} hit the per-task transition cap (>= {})",
                    a.id, MAX_TRANSITIONS_PER_TASK
                ),
                None,
            ));
        }

        // Resolve `from` = current lifecycle (fold the log); `transition()` does the
        // validate-then-append (C5).
        let from = current_lifecycle(&log, &a.id);

        transition(&log_path, &a.id, Some(from), to, pane_actor(), now_ms()).map_err(|e| {
            // An illegal edge is an InvalidData error from `core/task` — surface it
            // as a structured `invalid_params` rejection over the wire (nothing was
            // appended; the log is unchanged).
            ErrorData::invalid_params(format!("task_transition: {e}"), None)
        })?;

        Ok(Json(TransitionResult {
            ok: true,
            lifecycle: to.to_string(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Server-minted ids carry the `task_` prefix and are UNIQUE per call (the
    /// counter increments) — an agent never supplies the id (C2).
    #[test]
    fn mint_id_is_prefixed_and_unique() {
        let a = mint_id();
        let b = mint_id();
        assert!(a.starts_with("task_"), "id must be `task_<...>`: {a}");
        assert!(b.starts_with("task_"), "id must be `task_<...>`: {b}");
        assert_ne!(
            a, b,
            "two mints in the same ms must differ (counter increments)"
        );
        // No path separators / traversal in a minted id (it is a logical key).
        assert!(
            !a.contains('/') && !a.contains(".."),
            "minted id is path-free: {a}"
        );
    }

    /// `actor_from` (the pure core of `pane_actor`) maps the spawn-set pane id to
    /// `Actor::Pane{workspace_id}` (C2 provenance), and falls back to a stable
    /// `unknown` sentinel when unset. Pure: no env read, so it can't race other tests.
    #[test]
    fn actor_from_stamps_pane_provenance_with_unknown_fallback() {
        assert_eq!(
            actor_from(Some("ws-7".to_string())),
            Actor::Pane {
                workspace_id: "ws-7".to_string()
            },
            "a set pane id becomes Actor::Pane{{workspace_id}}"
        );
        assert_eq!(
            actor_from(None),
            Actor::Pane {
                workspace_id: "unknown".to_string()
            },
            "an unset pane id falls back to the `unknown` sentinel"
        );
    }

    /// THE C4 PROOF (advisor's load-bearing test): `create_task` (the underlying of
    /// `task_create`) routes a title to the log, and the SAME union reads
    /// `task_list`/`task_get` use (`list_views`/`get_view`) SEE it — without ever
    /// writing the mutable store. Remove the union and `get_view` returns `None`
    /// (silent disappearance); this catches that.
    #[test]
    fn create_then_get_and_list_round_trip_via_log_only() {
        let dir = std::env::temp_dir().join(format!(
            "at-mcp-task-rt-{}-{}",
            std::process::id(),
            mint_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let log_path = dir.join("tasks-log.jsonl");

        // create (the task_create core): mint + genesis-with-title on the log only.
        let id = mint_id();
        create_task(
            &log_path,
            &id,
            "build the thing",
            actor_from(Some("ws9-p1".into())),
            100,
        )
        .unwrap();

        // The operator store is EMPTY (agents never write it) — prove the union, not
        // the store, surfaces the task.
        let store: Vec<Task> = Vec::new();
        let log = read_transitions(&log_path);

        // get_view (the task_get core) FINDS the log-only task.
        let got = get_view(&id, &store, &log).expect("agent-created task must be gettable");
        assert_eq!(got.id, id);
        assert_eq!(
            got.title, "build the thing",
            "title round-trips from the genesis line"
        );
        assert_eq!(got.lifecycle, "created");
        assert_eq!(got.column, "backlog", "a log-only task defaults to backlog");

        // list_views (the task_list core) INCLUDES it.
        let listed = list_views(&store, &log);
        assert_eq!(listed.len(), 1, "the agent-created task is listed");
        assert_eq!(listed[0].id, id);

        // An unknown id is still None.
        assert!(get_view("task_nope", &store, &log).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 4c — `scope_allows`: a pane may advance ONLY a task it authored. A
    /// different-pane / operator / phantom genesis is rejected; an unset scope is
    /// unrestricted (back-compat).
    #[test]
    fn scope_allows_only_owned_task() {
        let dir = std::env::temp_dir().join(format!(
            "at-mcp-task-scope-{}-{}",
            std::process::id(),
            mint_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let log_path = dir.join("tasks-log.jsonl");

        // p1 authors task A; p2 authors task B; the operator authors task C.
        create_task(&log_path, "A", "a", actor_from(Some("ws-p1".into())), 1).unwrap();
        create_task(&log_path, "B", "b", actor_from(Some("ws-p2".into())), 2).unwrap();
        create_task(&log_path, "C", "c", Actor::Operator, 3).unwrap();
        let log = read_transitions(&log_path);

        // p1's scope: only A is allowed.
        assert!(
            scope_allows(Some("ws-p1"), &log, "A"),
            "owner may advance its own task"
        );
        assert!(
            !scope_allows(Some("ws-p1"), &log, "B"),
            "cross-pane task rejected"
        );
        assert!(
            !scope_allows(Some("ws-p1"), &log, "C"),
            "operator task rejected under scope"
        );
        assert!(
            !scope_allows(Some("ws-p1"), &log, "nope"),
            "phantom id rejected"
        );
        // Unset scope ⇒ unrestricted.
        assert!(scope_allows(None, &log, "A"));
        assert!(scope_allows(None, &log, "B"));
        assert!(scope_allows(None, &log, "C"));

        // genesis_owner attribution.
        assert_eq!(genesis_owner(&log, "A").as_deref(), Some("ws-p1"));
        assert_eq!(
            genesis_owner(&log, "C"),
            None,
            "operator genesis has no pane owner"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// C6 — the per-task transition cap predicate fires at `MAX_TRANSITIONS_PER_TASK`
    /// (the unbounded `Doing↔Review` cycle guard) and is per-task (a different id is
    /// unaffected). `at_task_cap` is the symmetric genesis-count guard.
    #[test]
    fn transition_cap_predicate_fires_at_threshold() {
        let dir = std::env::temp_dir().join(format!(
            "at-mcp-task-cap-{}-{}",
            std::process::id(),
            mint_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let log_path = dir.join("tasks-log.jsonl");

        // Build a log of exactly MAX_TRANSITIONS_PER_TASK lines for "hot" (raw appends —
        // we are testing the COUNT predicate, not the legality graph), plus one line
        // for "cold" to prove the cap is per-task.
        for i in 0..MAX_TRANSITIONS_PER_TASK {
            agent_teams_task::append_transition(
                &log_path,
                &Transition {
                    task_id: "hot".into(),
                    from: if i == 0 { None } else { Some(Lifecycle::Doing) },
                    to: Lifecycle::Review,
                    by: Actor::Operator,
                    at: i as u64,
                    title: None,
                },
            )
            .unwrap();
        }
        create_task(&log_path, "cold", "c", Actor::Operator, 1).unwrap();
        let log = read_transitions(&log_path);

        assert!(
            at_transition_cap(&log, "hot"),
            "hot is AT the per-task cap → reject"
        );
        assert!(
            !at_transition_cap(&log, "cold"),
            "cold (1 line) is under the cap"
        );
        assert!(
            !at_transition_cap(&log, "absent"),
            "an absent task is under the cap"
        );
        // The genesis-count cap is far from MAX_TASKS with two tasks.
        assert!(!at_task_cap(&log), "two tasks is well under MAX_TASKS");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The illegal-edge rejection the tools rely on, asserted at this crate's layer
    /// (core/task owns the canonical graph test; this pins the contract the tool
    /// boundary depends on — illegal edge ⇒ error, log unchanged; C4/C5).
    #[test]
    fn illegal_edge_is_rejected_and_appends_nothing() {
        let dir = std::env::temp_dir().join(format!(
            "at-mcp-task-test-{}-{}",
            std::process::id(),
            mint_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let log_path = dir.join("tasks-log.jsonl");

        // Genesis Created (legal), via the same path task_create uses.
        create_task(&log_path, "t1", "title", actor_from(Some("w".into())), 1)
            .expect("genesis Created is legal");
        assert_eq!(read_transitions(&log_path).len(), 1);

        // task_transition resolves `from` by folding the log, then calls transition().
        let from = current_lifecycle(&read_transitions(&log_path), "t1");
        assert_eq!(from, Lifecycle::Created);

        // Created→Done is ILLEGAL — must error and append nothing.
        let err = transition(
            &log_path,
            "t1",
            Some(from),
            Lifecycle::Done,
            actor_from(Some("w".into())),
            2,
        )
        .expect_err("Created→Done must be rejected");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(
            read_transitions(&log_path).len(),
            1,
            "an illegal edge must append NOTHING (log unchanged)"
        );

        // A legal edge (Created→Doing) appends exactly one line.
        let from = current_lifecycle(&read_transitions(&log_path), "t1");
        transition(
            &log_path,
            "t1",
            Some(from),
            Lifecycle::Doing,
            actor_from(None),
            3,
        )
        .expect("Created→Doing is legal");
        let log = read_transitions(&log_path);
        assert_eq!(log.len(), 2);
        assert_eq!(current_lifecycle(&log, "t1"), Lifecycle::Doing);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ─── Adversarial / invariant hardening (union, AC7/C2 boundary, inertness) ─

    /// Build a sample MUTABLE-STORE task with a given id (the operator-owned record
    /// the union must prefer over a colliding log genesis).
    fn store_task(id: &str, title: &str, col: Column, order: i64) -> Task {
        Task {
            id: id.to_string(),
            title: title.to_string(),
            column: col,
            order,
            created_at: 42,
            updated_at: 42,
            workspace_id: Some("ws-op".into()),
            pane_id: None,
            run_id: None,
        }
    }

    /// THE UNION CONTRACT — "the store wins on id overlap" (the doc claim on both
    /// `list_views` and `get_view`) is UNTESTED today. Seed the SAME id in the
    /// operator store AND as a log genesis (a different title/owner), then assert:
    ///   · `list_views` renders that id EXACTLY ONCE, from the STORE (store title +
    ///     store column), never doubled and never shadowed by the log genesis;
    ///   · `get_view` for that id returns the STORE projection;
    ///   · a log-ONLY id still surfaces (the union still includes agent-created tasks);
    ///   · the lifecycle is still folded from the log for the overlapping id.
    /// If the id double-renders or the log shadows the store → SECURITY/CORRECTNESS
    /// finding (an agent genesis could mask/duplicate an operator card).
    #[test]
    fn union_store_wins_on_id_overlap() {
        // Operator store: one real card "shared" (Doing) + one store-only "op_only".
        let store = vec![
            store_task("shared", "operator title", Column::Doing, 7),
            store_task("op_only", "op only", Column::Review, 1),
        ];

        // Log: a genesis for the SAME id "shared" (agent-authored, different title),
        // a later legal advance for "shared", and a genesis for a log-ONLY task.
        let log = vec![
            // Colliding genesis — a DIFFERENT title + a pane owner.
            Transition {
                task_id: "shared".into(),
                from: None,
                to: Lifecycle::Created,
                by: Actor::Pane {
                    workspace_id: "ws-attacker".into(),
                },
                at: 1,
                title: Some("AGENT-INJECTED title".into()),
            },
            // Advance the shared task so the folded lifecycle is `doing` (proves the
            // log still drives lifecycle even though the store wins on the row).
            Transition {
                task_id: "shared".into(),
                from: Some(Lifecycle::Created),
                to: Lifecycle::Doing,
                by: Actor::Pane {
                    workspace_id: "ws-attacker".into(),
                },
                at: 2,
                title: None,
            },
            // A genuinely log-only (agent-created) task.
            Transition {
                task_id: "log_only".into(),
                from: None,
                to: Lifecycle::Created,
                by: Actor::Pane {
                    workspace_id: "ws9".into(),
                },
                at: 3,
                title: Some("agent task".into()),
            },
        ];

        let views = list_views(&store, &log);

        // (a) "shared" appears EXACTLY ONCE.
        let shared: Vec<&TaskView> = views.iter().filter(|v| v.id == "shared").collect();
        assert_eq!(
            shared.len(),
            1,
            "an id present in BOTH store and log genesis must render exactly once (got {})",
            shared.len()
        );
        // (b) …and it is the STORE projection (store title + store column), NOT the
        //     agent-injected genesis title — the log genesis must not shadow the store.
        assert_eq!(
            shared[0].title, "operator title",
            "store must WIN on overlap: the agent-injected genesis title must NOT shadow it"
        );
        assert_eq!(
            shared[0].column, "doing",
            "store column wins (genesis defaults to backlog)"
        );
        // (c) …but lifecycle is still FOLDED from the log (the agent's advance).
        assert_eq!(
            shared[0].lifecycle, "doing",
            "lifecycle is folded from the log even for a store-won row"
        );

        // (d) The log-ONLY task still surfaces (the union includes agent-created tasks).
        assert!(
            views
                .iter()
                .any(|v| v.id == "log_only" && v.title == "agent task"),
            "a log-only (agent-created) task must still be listed via the union"
        );
        // (e) Exactly the three distinct ids, no duplicates anywhere.
        let mut ids: Vec<&str> = views.iter().map(|v| v.id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(
            ids,
            vec!["log_only", "op_only", "shared"],
            "no dup / no drop in the union"
        );

        // get_view must MATCH list_views on the overlap (single source of truth).
        let g = get_view("shared", &store, &log).expect("overlapping id must resolve");
        assert_eq!(
            g.title, "operator title",
            "get_view: store wins on overlap too"
        );
        assert_eq!(g.column, "doing");
        assert_eq!(
            g.lifecycle, "doing",
            "get_view folds lifecycle from the log"
        );
    }

    /// AC7 + C2 — the `deny_unknown_fields` BOUNDARY, asserted by deserializing the
    /// Args types DIRECTLY (race-free: no env, no async runtime, no global state).
    /// This is the wire contract a malicious client hits:
    ///   · `CreateTaskArgs` REJECTS a smuggled `id` (C2 — the server mints the id; a
    ///     client cannot supply a path-like or colliding id);
    ///   · `TransitionTaskArgs` REJECTS a smuggled `status`/`state` key (AC7 —
    ///     `rank()` stays the sole machine-state authority; no status smuggling);
    ///   · `GetTaskArgs` / `CreateTaskArgs` reject ANY extra field;
    ///   · the well-formed shapes still parse (the gate is not over-broad).
    /// A failure here (an extra field silently ignored) is an AC7/C2 finding.
    #[test]
    fn args_deny_unknown_fields_blocks_smuggling() {
        // C2: task_create mints the id — a client-supplied `id` must be REJECTED,
        // not silently dropped (a dropped id would still pass but hide intent; a hard
        // error is the contract `deny_unknown_fields` guarantees).
        assert!(
            serde_json::from_str::<CreateTaskArgs>(r#"{"title":"x","id":"forged-../../etc"}"#)
                .is_err(),
            "CreateTaskArgs must REJECT a smuggled `id` (server mints it — C2)"
        );
        // AC7: no status/state field may be smuggled onto a transition.
        assert!(
            serde_json::from_str::<TransitionTaskArgs>(
                r#"{"id":"t","to":"doing","status":"working"}"#
            )
            .is_err(),
            "TransitionTaskArgs must REJECT a smuggled `status` (AC7 — rank() owns machine state)"
        );
        assert!(
            serde_json::from_str::<TransitionTaskArgs>(r#"{"id":"t","to":"doing","state":"idle"}"#)
                .is_err(),
            "TransitionTaskArgs must REJECT a smuggled `state` (AC7)"
        );
        // Any arbitrary extra field is rejected on every Args type.
        assert!(
            serde_json::from_str::<GetTaskArgs>(r#"{"id":"t","extra":1}"#).is_err(),
            "GetTaskArgs must reject an unknown field"
        );
        assert!(
            serde_json::from_str::<CreateTaskArgs>(r#"{"title":"x","x":true}"#).is_err(),
            "CreateTaskArgs must reject an unknown field"
        );
        assert!(
            serde_json::from_str::<TransitionTaskArgs>(r#"{"id":"t","to":"done","junk":[]}"#)
                .is_err(),
            "TransitionTaskArgs must reject an unknown field"
        );

        // The gate is not over-broad — well-formed args still deserialize.
        let c: CreateTaskArgs =
            serde_json::from_str(r#"{"title":"hello"}"#).expect("a valid create arg must parse");
        assert_eq!(c.title, "hello");
        let t: TransitionTaskArgs =
            serde_json::from_str(r#"{"id":"task_1","to":"review"}"#).expect("valid transition arg");
        assert_eq!(t.id, "task_1");
        assert_eq!(t.to, "review");
        let g: GetTaskArgs = serde_json::from_str(r#"{"id":"task_1"}"#).expect("valid get arg");
        assert_eq!(g.id, "task_1");
    }

    /// C2 path-key INERTNESS — a `../`-laden id is a LOGICAL key, never a filesystem
    /// path. `get_view` with a traversal-shaped id over an empty store/log selects
    /// NOTHING (returns `None`); it derives no path and escapes nothing. (The log
    /// file path comes solely from `state_root`; the id never indexes the FS.)
    #[test]
    fn path_like_id_is_inert_in_reads() {
        let store: Vec<Task> = Vec::new();
        let log: Vec<Transition> = Vec::new();
        for evil in [
            "../../etc/passwd",
            "..\\..\\windows\\system32",
            "/etc/shadow",
            "task_1/../../secret",
            "..",
        ] {
            assert!(
                get_view(evil, &store, &log).is_none(),
                "a path-shaped id {evil:?} must select no task (logical key, not a path)"
            );
        }
        // And in a non-empty log it still matches only an EXACT genesis id, never a
        // path-normalized neighbor.
        let log = vec![Transition {
            task_id: "real".into(),
            from: None,
            to: Lifecycle::Created,
            by: Actor::Pane {
                workspace_id: "w".into(),
            },
            at: 1,
            title: Some("t".into()),
        }];
        assert!(
            get_view("real/../real", &store, &log).is_none(),
            "no path-normalization on lookup"
        );
        assert!(
            get_view("real", &store, &log).is_some(),
            "the exact logical id still resolves"
        );
    }

    /// A LOG-ONLY (agent-created) task's lifecycle FOLDS through the read path: the
    /// pre-existing round-trip (`create_then_get_and_list_round_trip_via_log_only`)
    /// only checks the genesis `created` state. Here we advance Created→Doing→Review
    /// and assert BOTH `get_view` and `list_views` report the FOLDED `review` — so the
    /// read path tracks the lifecycle of agent tasks, not just their creation.
    #[test]
    fn log_only_task_lifecycle_folds_through_read_path() {
        let store: Vec<Task> = Vec::new();
        let log = vec![
            Transition {
                task_id: "agt".into(),
                from: None,
                to: Lifecycle::Created,
                by: Actor::Pane {
                    workspace_id: "wsX".into(),
                },
                at: 1,
                title: Some("agent card".into()),
            },
            Transition {
                task_id: "agt".into(),
                from: Some(Lifecycle::Created),
                to: Lifecycle::Doing,
                by: Actor::Pane {
                    workspace_id: "wsX".into(),
                },
                at: 2,
                title: None,
            },
            Transition {
                task_id: "agt".into(),
                from: Some(Lifecycle::Doing),
                to: Lifecycle::Review,
                by: Actor::Pane {
                    workspace_id: "wsX".into(),
                },
                at: 3,
                title: None,
            },
        ];

        let g = get_view("agt", &store, &log).expect("log-only task must be gettable");
        assert_eq!(
            g.lifecycle, "review",
            "get_view folds the agent task's lifecycle to review"
        );
        assert_eq!(g.title, "agent card", "title still from the genesis line");
        assert_eq!(
            g.column, "backlog",
            "a log-only task has no operator column"
        );

        let listed = list_views(&store, &log);
        assert_eq!(listed.len(), 1);
        assert_eq!(
            listed[0].lifecycle, "review",
            "list_views folds the same lifecycle"
        );
    }
}
