//! Persisted **Task** model — the first durable, human-owned product entity in
//! Agent Teams (option-b plan-item #1; `.paul/phases/09-surfaces/09-01-PLAN.md`
//! Task 1 / AC-1 / AC-4 prerequisite half).
//!
//! # Why this crate exists
//! Everything durable in the product today is either (a) ephemeral frontend
//! `localStorage` (`at_workspaces`), wiped freely by the user, or (b)
//! machine-owned adapter state under `state_root`, **deliberately wiped on every
//! startup** (D7). There is no persisted product entity with a *human-owned*
//! lifecycle. A real kanban board — drag a card from "Backlog" to "Doing" and
//! have it stick across restarts — needs exactly that. That is the [`Task`].
//!
//! # Durability contract (load-bearing)
//! Tasks live in a JSONL file that is a **SIBLING of `state_root`**
//! (`<state_root>/../<name>-tasks.jsonl`), exactly like `runs.jsonl` /
//! `agent-teams-live.json`. This is the ONLY way the data survives the D7
//! startup wipe of `state_root`. See [`tasks_path`].
//!
//! # Store semantics — mutable, NOT append-only
//! Unlike `runs.jsonl` (an append-only event log), the task store is **mutable**:
//! a human changes a Task's `column`/`order` in place. The writer is therefore a
//! full-file rewrite ([`write_tasks`]), never an append — appending would
//! duplicate ids. [`upsert_task`] is the mutation primitive the future Tauri CRUD
//! commands (`create` / `update-column` / `reorder` / `delete`) call.
//!
//! # Seams — what a Task is NOT
//! - **Task ≠ pane.** Panes are ephemeral frontend `localStorage`
//!   (`at_workspaces`). A pane is a live process a Task *may* point at via the
//!   OPTIONAL [`Task::workspace_id`] / [`Task::pane_id`] — correlation, not
//!   ownership. Closing a pane must NOT delete its Task; the Task outlives the
//!   session (the whole point of persistence). This crate has no
//!   supervisor/adapter dependency and knows nothing of pane lifecycle.
//! - **Task ≠ RunRecord.** `runs.jsonl` is an append-only event log of
//!   `RunRecord{id,harness,enqueued_at,…}`. A Task's OPTIONAL [`Task::run_id`]
//!   JOINS to a `RunRecord.id` for "what happened on this task" — but a Task is
//!   NOT folded into `runs.jsonl`. Separate file, separate semantics (mutable
//!   full-rewrite vs append-only).

use serde::{Deserialize, Serialize};
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// The human-owned workflow stage of a [`Task`] — the kanban column.
///
/// Distinct from the *state-machine* columns of the alt-view board (those derive
/// from the adapter `State` and are machine-owned / auto-placed; these are set by
/// a human dragging a card and **persist across restarts**).
///
/// The wire form is lowercase (`backlog` / `doing` / `review` / `done`) and is a
/// stable on-disk contract — see the `column_wire_form_is_stable` test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Column {
    Backlog,
    Doing,
    Review,
    Done,
    /// A FUTURE / unknown column value from a newer writer. `#[serde(other)]` makes
    /// an unrecognized wire token deserialize HERE instead of failing the whole
    /// `Task` line — critical because the mutable store is a full-file rewrite, so a
    /// line that fails to parse would be silently ERASED on the next `write_tasks`.
    /// A Task with an unknown column is preserved (folded to the backlog/first column
    /// for display — see the app's `column_display_ord`). Serializes back as
    /// `"unknown"` (the original future token is not round-tripped, but the Task
    /// survives).
    #[serde(other)]
    Unknown,
}

/// A persisted Task — one JSONL line in the sibling-of-`state_root` task store.
///
/// `order` is **signed** (`i64`) so an in-column reorder can nudge a card up or
/// down without risking unsigned underflow. This crate does **not** mint `id`s —
/// callers supply a stable, app-owned id (mirroring how `RunRecord.id` is
/// caller-supplied). `created_at` / `updated_at` are unix ms.
///
/// The three link fields are OPTIONAL correlation, not ownership (see the
/// crate-level seams): `workspace_id` / `pane_id` point at a live pane the Task is
/// bound to (if any), and `run_id` JOINS to `runs.jsonl` `RunRecord.id`. A Task
/// can be pure backlog with all three `None` and no agent yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub column: Column,
    pub order: i64,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

/// The task-store path: `<state_root>/../<name>-tasks.jsonl` (a **SIBLING** of
/// `state_root`, where `<name>` is `state_root`'s own dir name). `None` when
/// `state_root` has no parent.
///
/// This synthesizes two existing conventions: it mirrors
/// `agent_teams_core::registry_path`'s parametrized `parent().map(join)` mechanic
/// (so a test can inject a tempdir) AND `default_runs_path`'s filename derivation
/// (`<name>-…jsonl`) — but, unlike `default_runs_path`, it takes `state_root` as
/// an argument rather than reading `$HOME`, so it never touches the real home dir
/// and the restart test stays hermetic.
pub fn tasks_path(state_root: &Path) -> Option<PathBuf> {
    let name = state_root
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "agent-teams".to_string());
    state_root
        .parent()
        .map(|p| p.join(format!("{name}-tasks.jsonl")))
}

/// Read all tasks from `path` (tolerant: skips blank / unparseable lines).
/// Returns an empty `Vec` if the file is absent or unreadable. Mirrors the
/// `read_runs` reader contract.
pub fn read_tasks(path: &Path) -> Vec<Task> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<Task>(l).ok())
        .collect()
}

/// Write the **whole** task set to `path`, one JSON object per line, TRUNCATING
/// any existing file.
///
/// This is a full-file rewrite — **never** an append — because tasks are mutable
/// (a reorder / column change rewrites the record in place; appending would
/// duplicate ids). A pathological serialize failure falls back to a blank line,
/// which the tolerant [`read_tasks`] skips, rather than poisoning the file.
pub fn write_tasks(path: &Path, tasks: &[Task]) -> std::io::Result<()> {
    // SHRINK GUARD (data-loss prevention): a full-file rewrite of a store that
    // currently holds lines we could NOT parse would silently ERASE those lines
    // (`read_tasks` drops unparseable lines; `write_tasks` then persists only the
    // survivors). If the file on disk has any non-empty line that does not parse as
    // a `Task`, REFUSE to rewrite and leave the file untouched, so a partial-parse
    // never destroys data. (An unknown *column* value is NOT a parse failure — it
    // folds to `Column::Unknown` — so this only fires on genuinely corrupt lines.)
    if let Ok(existing) = std::fs::read_to_string(path) {
        let mut non_empty = 0usize;
        let mut parsed = 0usize;
        for line in existing.lines().filter(|l| !l.trim().is_empty()) {
            non_empty += 1;
            if serde_json::from_str::<Task>(line).is_ok() {
                parsed += 1;
            }
        }
        if parsed < non_empty {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "refusing to rewrite task store: {} of {non_empty} on-disk line(s) unparseable \
                     (a full-file rewrite would erase them)",
                    non_empty - parsed
                ),
            ));
        }
    }
    let mut body = String::with_capacity(tasks.len() * 128);
    for t in tasks {
        body.push_str(&serde_json::to_string(t).unwrap_or_default());
        body.push('\n');
    }
    // ATOMIC REPLACE (tmp + rename): a bare in-place `fs::write` truncates first, so a
    // concurrent reader (another sidecar / the app's board poll) could observe an empty
    // or half-written store. `rename(2)` is atomic on the same volume.
    let mut tmp_os = path.as_os_str().to_owned();
    tmp_os.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp_os);
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path)
}

/// Insert-or-update one task: read the current set, replace the entry with the
/// same `id` (or push it if new), then rewrite the whole file.
///
/// This is the mutation primitive the future Tauri `update-column` / `reorder` /
/// `create` commands call. Because it rewrites (never appends), an upsert of an
/// existing id leaves **exactly one** record for that id.
pub fn upsert_task(path: &Path, task: &Task) -> std::io::Result<()> {
    let mut tasks = read_tasks(path);
    match tasks.iter_mut().find(|t| t.id == task.id) {
        Some(existing) => *existing = task.clone(),
        None => tasks.push(task.clone()),
    }
    write_tasks(path, &tasks)
}

// ─────────────────────────────────────────────────────────────────────────────
// Lifecycle state machine + append-only transition log (item 4a)
//
// `Column` (above) is the operator-owned *workflow column* — a human drags a card
// and it sticks. The LIFECYCLE below is the AGENT-writable channel: a pane agent
// progresses its task `created → assigned → doing → review → done` along a legal
// directed graph, and every transition is APPENDED (who + when) to a separate
// `<name>-tasks-log.jsonl`. The current lifecycle of a task is `fold(log)`.
//
// # Why a separate APPEND-ONLY file (the load-bearing correctness property)
// The mutable task store ([`write_tasks`] / [`upsert_task`]) is a single JSONL
// with a FULL-FILE rewrite: read-all → modify-one → write-all. With N pane
// sidecars writing it app-independently, two concurrent full-rewrites clobber each
// other's records — even on different tasks. That race is invisible to the
// single-threaded store tests. The transition log avoids it STRUCTURALLY: each
// transition is a single `\n`-terminated line appended with `O_APPEND`
// ([`append_transition`]) — no read-modify-write, so N concurrent appenders never
// clobber. The mutable file stays the app's LOW-CONCURRENCY writer (operator-owned
// `title` / `order` / `column`); the agent-writable channel is the log.
//
// # AC7 — orthogonal to machine agent-state
// This lifecycle is task-WORKFLOW state. It is NOT the machine agent-state
// (`Working` / `Idle` / `needs_human`) owned solely by `rank()` in
// `core/state-adapter`. No lifecycle value is a `rank()`-derived value, and no
// `status` / `state` field is added to [`Task`].
// ─────────────────────────────────────────────────────────────────────────────

/// The agent-writable **lifecycle stage** of a task — a real state machine moved
/// along a legal directed graph (see [`valid_transition`]), distinct from the
/// operator-owned [`Column`].
///
/// The wire form is lowercase (`created` / `assigned` / `doing` / `review` /
/// `done`) and is a stable on-disk contract — see `lifecycle_wire_form_is_stable`.
/// `Display` / `FromStr` share that exact wire form (the `transition` error message
/// and any string parsing round-trips through it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lifecycle {
    Created,
    Assigned,
    Doing,
    Review,
    Done,
    /// A FUTURE / unknown lifecycle stage from a newer writer. `#[serde(other)]`
    /// makes an unrecognized wire token deserialize HERE instead of failing the
    /// whole `Transition` line. On the APPEND-ONLY log a line that fails to parse is
    /// dropped by `read_transitions`, so a future `to` value would make `fold` /
    /// `current_lifecycle` silently REVERT to the prior stage; folding to `Unknown`
    /// instead preserves the fact that the task advanced past the known graph. Never
    /// a legal `valid_transition` source/target (defensive parse-only sentinel).
    #[serde(other)]
    Unknown,
}

impl Lifecycle {
    /// The stable lowercase wire token (shared by serde, [`Display`], [`FromStr`]).
    pub fn as_str(&self) -> &'static str {
        match self {
            Lifecycle::Created => "created",
            Lifecycle::Assigned => "assigned",
            Lifecycle::Doing => "doing",
            Lifecycle::Review => "review",
            Lifecycle::Done => "done",
            Lifecycle::Unknown => "unknown",
        }
    }
}

impl fmt::Display for Lifecycle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Lifecycle {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "created" => Ok(Lifecycle::Created),
            "assigned" => Ok(Lifecycle::Assigned),
            "doing" => Ok(Lifecycle::Doing),
            "review" => Ok(Lifecycle::Review),
            "done" => Ok(Lifecycle::Done),
            other => Err(format!("unknown lifecycle stage: {other:?}")),
        }
    }
}

/// Is the directed edge `from → to` a legal lifecycle transition?
///
/// PURE + TOTAL (no I/O, no clock, defined for every pair). The directed graph:
///
/// ```text
/// Created  → Assigned | Doing      // assign to a pane, or pick up directly
/// Assigned → Doing                 // pane starts work
/// Doing    → Review | Done         // finish to review, or straight to done
/// Review   → Doing  | Done         // kicked back, or approved
/// Done     → (terminal)            // re-open is a NEW task, never a back-edge
/// ```
///
/// Self-edges are NOT legal (a no-op transition records nothing). This is the
/// agent-facing safety bound: a pane can only advance its task along the legal
/// graph, never teleport `Created → Done`.
pub fn valid_transition(from: Lifecycle, to: Lifecycle) -> bool {
    use Lifecycle::*;
    matches!(
        (from, to),
        (Created, Assigned)
            | (Created, Doing)
            | (Assigned, Doing)
            | (Doing, Review)
            | (Doing, Done)
            | (Review, Doing)
            | (Review, Done)
    )
}

/// WHO performed a transition — the attributable half of the audit log.
///
/// `Pane(workspace_id)` is an agent transition (a pane advancing its own task);
/// `Operator` is a human / Tauri-side transition (e.g. the `assigned` binding at
/// spawn). The wire form is internally-tagged so the log line is self-describing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Actor {
    /// An agent transition, tagged with the pane's `workspace_id`.
    Pane { workspace_id: String },
    /// A human / Tauri-side transition.
    Operator,
    /// A FUTURE / unknown actor `kind` from a newer writer. `#[serde(other)]` (on an
    /// internally-tagged enum) makes an unrecognized `kind` deserialize HERE instead
    /// of failing the whole `Transition` line — so an unknown actor on a GENESIS line
    /// doesn't make the line unparseable and thus dropped (which would lose the task
    /// from `log_task_ids` / `current_lifecycle`). Carries no fields.
    #[serde(other)]
    Unknown,
}

/// One appended line in `<name>-tasks-log.jsonl` — an attributable, timestamped
/// lifecycle transition for a single task.
///
/// `from = None` is the GENESIS line (the initial `Created`, which has no
/// predecessor). `at` is unix-ms passed IN by the caller (like `RawEvent.at`) so
/// this crate stays clock-free and deterministically testable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transition {
    pub task_id: String,
    /// `None` for the genesis `Created`; `Some(prev)` otherwise.
    #[serde(default)]
    pub from: Option<Lifecycle>,
    pub to: Lifecycle,
    pub by: Actor,
    /// unix-ms, caller-supplied (the core has no clock).
    pub at: u64,
    /// The agent-supplied task title, set ONLY on the GENESIS line (`from ==
    /// None`) by [`create_task`] — the append-only, lost-update-free home for a
    /// task's title (C4). `None` on every non-genesis transition AND on
    /// operator / pre-enablement genesis lines. `#[serde(default)]` ⇒ old log
    /// lines without the field round-trip to `None` (a back-compat add).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// The transition-log path: `<state_root>/../<name>-tasks-log.jsonl` — a NEW
/// **SIBLING** of `state_root`, separate from [`tasks_path`]'s `-tasks.jsonl`.
/// Mirrors [`tasks_path`]'s derivation exactly (same `<name>`, same `parent`),
/// so the log survives the D7 startup wipe of `state_root` just like the store.
/// `None` when `state_root` has no parent.
pub fn tasks_log_path(state_root: &Path) -> Option<PathBuf> {
    let name = state_root
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "agent-teams".to_string());
    state_root
        .parent()
        .map(|p| p.join(format!("{name}-tasks-log.jsonl")))
}

/// APPEND one `\n`-terminated transition line to `path`.
///
/// **The concurrency-safe write path.** Opens its OWN `O_APPEND` handle per call
/// and `write_all`s the line in a SINGLE syscall (`format!("{json}\n")` built up
/// front — never json-then-newline, which would be an interleave point). On a
/// local FS (APFS), `O_APPEND` + a single short-line write is atomic, so N
/// concurrent appenders never clobber. NEVER read-modify-write — that is the
/// multi-writer hazard this whole design exists to avoid.
///
/// A serialize failure writes NOTHING (returns the error) rather than a partial /
/// blank line — the line is whole or absent.
pub fn append_transition(path: &Path, t: &Transition) -> std::io::Result<()> {
    let json = serde_json::to_string(t)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let mut line = json;
    line.push('\n');
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    f.write_all(line.as_bytes())
}

/// Read all transitions from `path` (tolerant: skips blank / unparseable lines,
/// mirroring [`read_tasks`]). Returns an empty `Vec` if the file is absent.
/// Order is **file order** (= append order); callers MUST NOT re-sort by `at`
/// (caller-supplied, may be constant) — append order IS the causal order.
pub fn read_transitions(path: &Path) -> Vec<Transition> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<Transition>(l).ok())
        .collect()
}

/// The current lifecycle of `task_id` = `fold(log)`: the `to` of that task's
/// LAST (file-order) transition, or [`Lifecycle::Created`] if the task has no
/// transition in the log. Filters by `task_id`; does NOT sort (append order is
/// causal order — see [`read_transitions`]).
pub fn current_lifecycle(log: &[Transition], task_id: &str) -> Lifecycle {
    log.iter()
        .filter(|t| t.task_id == task_id)
        .map(|t| t.to)
        .next_back()
        .unwrap_or(Lifecycle::Created)
}

/// Validate then append one transition for `task_id`.
///
/// Contract for `from`:
/// - `None` = GENESIS — accepted only for the initial `Created` line (`to ==
///   Created`); any other `to` with `from = None` is rejected as illegal.
/// - `Some(f)` → accepted iff [`valid_transition`]`(f, to)`.
///
/// On an illegal edge, returns `Err(InvalidData)` and appends NOTHING (the log
/// line count is unchanged). On a legal edge, [`append_transition`]s one line.
pub fn transition(
    log_path: &Path,
    task_id: &str,
    from: Option<Lifecycle>,
    to: Lifecycle,
    by: Actor,
    at: u64,
) -> std::io::Result<()> {
    let legal = match from {
        None => to == Lifecycle::Created,
        Some(f) => valid_transition(f, to),
    };
    if !legal {
        let from_desc = match from {
            None => "<genesis>".to_string(),
            Some(f) => f.to_string(),
        };
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("illegal transition {from_desc}→{to}"),
        ));
    }
    append_transition(
        log_path,
        &Transition {
            task_id: task_id.to_string(),
            from,
            to,
            by,
            at,
            // A non-genesis edge carries no title (the title lives on the genesis
            // line written by `create_task`). `transition` is never the create path.
            title: None,
        },
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// Agent-create path + caps (item 4b enablement — C4 / C6)
//
// C4 — task CREATION is routed onto the append-only log, NOT the mutable store.
// [`create_task`] appends a GENESIS `Created` line that ALSO carries the agent-
// supplied `title`, so an agent NEVER writes the lost-update-prone `tasks.jsonl`
// (that file stays the operator-owned kanban writer — `upsert_task`). The title
// AND the lifecycle of an agent-created task both live in the concurrency-safe
// log; reads fold both. This is what closes the `task_create` lost-update race.
//
// C6 — a title byte cap ([`validate_title`]) here, plus (enforced at the MCP tool
// boundary, where the log is already read) a genesis-count cap ([`MAX_TASKS`]) and
// a per-task transition-count cap ([`MAX_TRANSITIONS_PER_TASK`], which bounds the
// `Doing↔Review` cycle). Without the per-task cap a single looping/injected agent
// could append unbounded lines → fill the disk + quadratically degrade every read
// (the task analog of the memory read-amplifier). The caps live as consts HERE
// (SSOT) but the count checks run at the boundary so this crate keeps its clean
// append-only-no-read concurrency contract.
// ─────────────────────────────────────────────────────────────────────────────

/// Max bytes for an agent-supplied task title (mirrors `core/memory`'s
/// `MAX_TITLE_BYTES`). Enforced by [`validate_title`] / [`create_task`].
pub const MAX_TASK_TITLE_BYTES: usize = 4_096;

/// Max distinct tasks (genesis lines) in one log — the count cap the MCP
/// `task_create` enforces (it already reads the log to dedup/fold). Bounds total
/// store growth from a create-loop.
pub const MAX_TASKS: usize = 10_000;

/// Max transition lines for a SINGLE task. The legal graph has a `Doing↔Review`
/// cycle, so without this one task could append unbounded lines (disk + a per-read
/// fold amplifier). Enforced by the MCP `task_transition`. Generous: a real task
/// transitions a handful of times; 64 still allows ~30 review round-trips.
pub const MAX_TRANSITIONS_PER_TASK: usize = 64;

/// Validate an agent-supplied task title against [`MAX_TASK_TITLE_BYTES`]. PURE;
/// returns a human error string on violation (the MCP layer renders it as a
/// structured rejection). An EMPTY title is allowed (a placeholder card).
pub fn validate_title(title: &str) -> Result<(), String> {
    if title.len() > MAX_TASK_TITLE_BYTES {
        return Err(format!(
            "title exceeds {MAX_TASK_TITLE_BYTES} bytes ({} bytes)",
            title.len()
        ));
    }
    Ok(())
}

/// Append a GENESIS `Created` transition carrying the agent-supplied `title` —
/// the append-only, lost-update-free home for a task's title (C4). Does NOT touch
/// the mutable `tasks.jsonl` store. Validates the title FIRST: an oversize title
/// returns `Err(InvalidData)` and appends NOTHING (mirrors [`transition`]'s
/// validate-then-append). The genesis edge is trivially legal (`None → Created`).
pub fn create_task(
    log_path: &Path,
    task_id: &str,
    title: &str,
    by: Actor,
    at: u64,
) -> std::io::Result<()> {
    validate_title(title).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    append_transition(
        log_path,
        &Transition {
            task_id: task_id.to_string(),
            from: None,
            to: Lifecycle::Created,
            by,
            at,
            title: Some(title.to_string()),
        },
    )
}

/// The agent-supplied title of `task_id`, read from its GENESIS line (`from ==
/// None`), or `None` if the task has no genesis-with-title in the log (an
/// operator-store-only task, or a pre-enablement genesis line). Does not sort
/// (genesis is the first line for a task; append order is causal order).
pub fn genesis_title(log: &[Transition], task_id: &str) -> Option<String> {
    log.iter()
        .find(|t| t.task_id == task_id && t.from.is_none())
        .and_then(|t| t.title.clone())
}

/// Distinct task ids that have a genesis (`from == None`) line in the log, in
/// first-seen (append/causal) order — the log-resident (agent-created) tasks,
/// distinct from the operator-owned mutable store. A reader unions these with the
/// store so an agent-created task (which never touches `tasks.jsonl`) is visible.
pub fn log_task_ids(log: &[Transition]) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for t in log.iter().filter(|t| t.from.is_none()) {
        if seen.insert(t.task_id.clone()) {
            out.push(t.task_id.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A unique scratch dir tree, cleaned on Drop. `state` is NESTED under `root`
    /// (`<root>/state`) precisely so the task store — a SIBLING of `state`
    /// (`<root>/state-tasks.jsonl`) — lands at `root` level, OUTSIDE `state`.
    /// That lets `survives_state_root_wipe` `remove_dir_all(&state)` (the D7 wipe)
    /// without taking the sibling file with it. Mirrors core/mcp's `Scratch`
    /// pattern — no `tempfile` crate dep.
    struct Scratch {
        root: PathBuf,
        state: PathBuf,
    }
    impl Scratch {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!("at-task-{}-{}", tag, std::process::id()));
            let _ = fs::remove_dir_all(&root);
            let state = root.join("state");
            fs::create_dir_all(&state).unwrap();
            Scratch { root, state }
        }
        /// The `state_root` handed to [`tasks_path`] (its sibling is the store).
        fn state_root(&self) -> &Path {
            &self.state
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn sample(id: &str, column: Column, order: i64) -> Task {
        Task {
            id: id.to_string(),
            title: format!("task {id}"),
            column,
            order,
            created_at: 1_000,
            updated_at: 1_000,
            workspace_id: None,
            pane_id: None,
            run_id: None,
        }
    }

    /// 1. Round-trip a mixed Vec (mixed columns/orders, one all-links-Some, one
    ///    all-None) through write → read and assert it is unchanged.
    #[test]
    fn round_trip_write_read() {
        let s = Scratch::new("roundtrip");
        let path = tasks_path(s.state_root()).unwrap();
        let tasks = vec![
            Task {
                workspace_id: Some("ws1".into()),
                pane_id: Some("ws1-p0".into()),
                run_id: Some("run-abc".into()),
                ..sample("a", Column::Doing, 10)
            },
            sample("b", Column::Backlog, -5), // pure backlog, all links None
            sample("c", Column::Review, 0),
            sample("d", Column::Done, 99),
        ];
        write_tasks(&path, &tasks).unwrap();
        assert_eq!(read_tasks(&path), tasks);
    }

    /// 2. THE CRITICAL ONE — the store is a sibling of `state_root`, so wiping
    ///    `state_root` (the D7 startup wipe) must leave it intact and unchanged.
    #[test]
    fn survives_state_root_wipe() {
        let s = Scratch::new("wipe");
        let state_root = s.state_root().to_path_buf();
        let path = tasks_path(&state_root).unwrap();
        let tasks = vec![sample("keep", Column::Doing, 1)];
        write_tasks(&path, &tasks).unwrap();

        fs::remove_dir_all(&state_root).unwrap(); // simulate the D7 startup wipe
        assert!(!state_root.exists(), "state_root wipe did not happen");
        assert!(path.exists(), "task store must survive the state_root wipe");
        assert_eq!(
            read_tasks(&path),
            tasks,
            "task store must be unchanged after the wipe"
        );
    }

    /// 3. An upsert of an existing id REWRITES in place — it does not append a
    ///    duplicate. Write Backlog, upsert same id to Doing, assert exactly one
    ///    record and column == Doing (proves full-rewrite, not append).
    #[test]
    fn mutate_column_rewrites_not_appends() {
        let s = Scratch::new("mutate");
        let path = tasks_path(s.state_root()).unwrap();
        upsert_task(&path, &sample("x", Column::Backlog, 0)).unwrap();
        upsert_task(&path, &sample("x", Column::Doing, 0)).unwrap();

        let back = read_tasks(&path);
        assert_eq!(
            back.len(),
            1,
            "upsert must rewrite, not append a duplicate id"
        );
        assert_eq!(
            back[0].column,
            Column::Doing,
            "column must reflect the mutation"
        );
    }

    /// 4. The path is a sibling of `state_root`; no parent → `None`
    ///    (mirrors `registry_path`).
    #[test]
    fn tasks_path_is_sibling_of_state_root() {
        assert_eq!(
            tasks_path(Path::new("/var/app/agent-teams")),
            Some(PathBuf::from("/var/app/agent-teams-tasks.jsonl"))
        );
        assert_eq!(tasks_path(Path::new("/")), None);
    }

    /// 5. The `Column` wire form is the stable lowercase contract.
    #[test]
    fn column_wire_form_is_stable() {
        for (col, wire) in [
            (Column::Backlog, "backlog"),
            (Column::Doing, "doing"),
            (Column::Review, "review"),
            (Column::Done, "done"),
        ] {
            assert_eq!(serde_json::to_string(&col).unwrap(), format!("\"{wire}\""));
            let back: Column = serde_json::from_str(&format!("\"{wire}\"")).unwrap();
            assert_eq!(back, col);
        }
    }

    // ─── Lifecycle state machine + append-only transition log (item 4a) ──────

    /// 6. `valid_transition` is the legal directed graph — asserted over the FULL
    ///    pair table (pure + total). Legal edges TRUE, every other pair FALSE
    ///    (incl. all self-edges and every `Done →` back-edge). Spot-checks the
    ///    PLAN's named cases: `Created→Done` FALSE, `Doing→Review` TRUE.
    #[test]
    fn valid_transition_graph() {
        use Lifecycle::*;
        let all = [Created, Assigned, Doing, Review, Done];
        let legal: &[(Lifecycle, Lifecycle)] = &[
            (Created, Assigned),
            (Created, Doing),
            (Assigned, Doing),
            (Doing, Review),
            (Doing, Done),
            (Review, Doing),
            (Review, Done),
        ];
        for &from in &all {
            for &to in &all {
                let want = legal.contains(&(from, to));
                assert_eq!(
                    valid_transition(from, to),
                    want,
                    "valid_transition({from}, {to}) should be {want}"
                );
            }
        }
        // Named PLAN cases (explicit, in case the table above is ever edited):
        assert!(
            !valid_transition(Created, Done),
            "Created→Done must be illegal"
        );
        assert!(
            valid_transition(Doing, Review),
            "Doing→Review must be legal"
        );
        for &to in &all {
            assert!(
                !valid_transition(Done, to),
                "Done is terminal: Done→{to} illegal"
            );
        }
    }

    /// 7. The `Lifecycle` wire form is the stable lowercase contract — across
    ///    serde, `Display`, AND `FromStr` (all five variants round-trip).
    #[test]
    fn lifecycle_wire_form_is_stable() {
        for (lc, wire) in [
            (Lifecycle::Created, "created"),
            (Lifecycle::Assigned, "assigned"),
            (Lifecycle::Doing, "doing"),
            (Lifecycle::Review, "review"),
            (Lifecycle::Done, "done"),
        ] {
            // serde
            assert_eq!(serde_json::to_string(&lc).unwrap(), format!("\"{wire}\""));
            let back: Lifecycle = serde_json::from_str(&format!("\"{wire}\"")).unwrap();
            assert_eq!(back, lc);
            // Display
            assert_eq!(lc.to_string(), wire);
            // FromStr
            assert_eq!(wire.parse::<Lifecycle>().unwrap(), lc);
        }
        assert!("bogus".parse::<Lifecycle>().is_err());
    }

    fn trans(task_id: &str, from: Option<Lifecycle>, to: Lifecycle, at: u64) -> Transition {
        Transition {
            task_id: task_id.to_string(),
            from,
            to,
            by: Actor::Pane {
                workspace_id: "ws1".into(),
            },
            at,
            title: None,
        }
    }

    /// 8. APPEND → FOLD reconstructs the lifecycle: append Created, Assigned,
    ///    Doing for task "x" → `current_lifecycle == Doing`. An unknown task
    ///    folds to `Created`. The log carries who + when on every line.
    #[test]
    fn append_then_fold_reconstructs_lifecycle() {
        let s = Scratch::new("fold");
        let path = tasks_log_path(s.state_root()).unwrap();
        append_transition(&path, &trans("x", None, Lifecycle::Created, 1)).unwrap();
        append_transition(
            &path,
            &trans("x", Some(Lifecycle::Created), Lifecycle::Assigned, 2),
        )
        .unwrap();
        append_transition(
            &path,
            &trans("x", Some(Lifecycle::Assigned), Lifecycle::Doing, 3),
        )
        .unwrap();
        // Interleave another task to prove per-task filtering.
        append_transition(&path, &trans("y", None, Lifecycle::Created, 4)).unwrap();

        let log = read_transitions(&path);
        assert_eq!(log.len(), 4, "every appended line is present");
        assert_eq!(current_lifecycle(&log, "x"), Lifecycle::Doing);
        assert_eq!(current_lifecycle(&log, "y"), Lifecycle::Created);
        assert_eq!(
            current_lifecycle(&log, "absent"),
            Lifecycle::Created,
            "an unknown task folds to Created"
        );
        // who + when survived the round-trip.
        for t in &log {
            assert!(t.at > 0, "every line carries `at` (when)");
            match &t.by {
                Actor::Pane { workspace_id } => assert_eq!(workspace_id, "ws1"),
                Actor::Operator | Actor::Unknown => panic!("expected Pane actor"),
            }
        }
    }

    /// 9. `transition()` REJECTS an illegal edge WITHOUT appending — the log line
    ///    count is unchanged. Then a legal edge appends exactly one line.
    #[test]
    fn transition_rejects_illegal_edge_without_appending() {
        let s = Scratch::new("illegal");
        let path = tasks_log_path(s.state_root()).unwrap();
        let by = || Actor::Operator;

        // Genesis Created — legal.
        transition(&path, "x", None, Lifecycle::Created, by(), 1).unwrap();
        assert_eq!(read_transitions(&path).len(), 1);

        // Created→Done — ILLEGAL; must append nothing.
        let err = transition(
            &path,
            "x",
            Some(Lifecycle::Created),
            Lifecycle::Done,
            by(),
            2,
        )
        .expect_err("Created→Done must be rejected");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(
            err.to_string().contains("created→done"),
            "error names the edge: {err}"
        );
        assert_eq!(
            read_transitions(&path).len(),
            1,
            "an illegal transition must append NOTHING"
        );

        // A genesis line with a non-Created `to` is also illegal.
        transition(&path, "z", None, Lifecycle::Doing, by(), 3)
            .expect_err("genesis must be Created");
        assert_eq!(read_transitions(&path).len(), 1, "still nothing appended");

        // Legal edge appends exactly one line.
        transition(
            &path,
            "x",
            Some(Lifecycle::Created),
            Lifecycle::Doing,
            by(),
            4,
        )
        .unwrap();
        let log = read_transitions(&path);
        assert_eq!(log.len(), 2);
        assert_eq!(current_lifecycle(&log, "x"), Lifecycle::Doing);
    }

    /// 10. THE SPINE — multi-threaded N-appender concurrency. N threads each
    ///     append K distinct lines to the SAME log concurrently (barrier-synced).
    ///     After join: EXACTLY N*K lines, every one parses, AND the recovered
    ///     multiset == the expected set. This is the property single-threaded
    ///     `core/task` tests cannot show; swap `append_transition` for a
    ///     read-modify-write upsert and this test MUST break.
    #[test]
    fn concurrent_appends_never_clobber() {
        use std::collections::BTreeSet;
        use std::sync::{Arc, Barrier};

        let s = Scratch::new("concurrent");
        let path = tasks_log_path(s.state_root()).unwrap();

        const N: usize = 16;
        const K: usize = 50;

        let barrier = Arc::new(Barrier::new(N));
        let path = Arc::new(path);
        let mut handles = Vec::with_capacity(N);
        for thread_id in 0..N {
            let barrier = Arc::clone(&barrier);
            let path = Arc::clone(&path);
            handles.push(std::thread::spawn(move || {
                // Race them all into the append window simultaneously.
                barrier.wait();
                for k in 0..K {
                    let task_id = format!("t{thread_id}-{k}");
                    append_transition(
                        &path,
                        &Transition {
                            task_id,
                            from: None,
                            to: Lifecycle::Created,
                            by: Actor::Pane {
                                workspace_id: format!("ws{thread_id}"),
                            },
                            at: (thread_id * K + k) as u64,
                            title: None,
                        },
                    )
                    .unwrap();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        let log = read_transitions(&path);
        // (a) EXACTLY N*K lines — no clobber, no loss.
        assert_eq!(
            log.len(),
            N * K,
            "expected {} appended lines, got {} (clobber/loss)",
            N * K,
            log.len()
        );
        // (b) Every line parsed (read_transitions skips unparseable) AND the
        //     recovered set of task_ids == the expected set (catches torn lines
        //     and duplication that a bare count would miss).
        let got: BTreeSet<String> = log.iter().map(|t| t.task_id.clone()).collect();
        let want: BTreeSet<String> = (0..N)
            .flat_map(|thread_id| (0..K).map(move |k| format!("t{thread_id}-{k}")))
            .collect();
        assert_eq!(
            got.len(),
            N * K,
            "every line is distinct + whole (no torn/dup)"
        );
        assert_eq!(
            got, want,
            "recovered task_id multiset must equal the expected set"
        );
    }

    /// 11. The CONTROL test (documents WHY the log exists): two concurrent
    ///     `upsert_task` full-rewrites on the SAME base CAN lose a record. Modeled
    ///     deterministically as the lost-update interleave (read A == read B,
    ///     both write back their own +1) so it is not flaky.
    #[test]
    fn upsert_full_rewrite_can_lose_an_update() {
        let s = Scratch::new("lostupdate");
        let path = tasks_path(s.state_root()).unwrap();
        // Seed one record so both "readers" see the same base.
        write_tasks(&path, &[sample("base", Column::Backlog, 0)]).unwrap();

        // Two writers each read the SAME base snapshot...
        let base_a = read_tasks(&path);
        let base_b = read_tasks(&path);
        assert_eq!(base_a, base_b);

        // ...add their own distinct task, then write the WHOLE file back. The
        // second write wins and clobbers the first writer's addition — the
        // single-file full-rewrite lost-update the append log structurally avoids.
        let mut a = base_a;
        a.push(sample("addedByA", Column::Doing, 1));
        write_tasks(&path, &a).unwrap();

        let mut b = base_b;
        b.push(sample("addedByB", Column::Doing, 2));
        write_tasks(&path, &b).unwrap();

        let final_tasks = read_tasks(&path);
        let ids: Vec<&str> = final_tasks.iter().map(|t| t.id.as_str()).collect();
        assert!(
            ids.contains(&"addedByB"),
            "the last writer's record survives"
        );
        assert!(
            !ids.contains(&"addedByA"),
            "the first writer's record was LOST — this is the multi-writer hazard the append log fixes"
        );
        assert_eq!(
            final_tasks.len(),
            2,
            "base + last writer only; one update lost"
        );
    }

    /// 12. `tasks_log_path` is a SIBLING of `state_root` and SURVIVES the D7
    ///     startup wipe (mirrors `survives_state_root_wipe` for the log file).
    #[test]
    fn log_survives_state_root_wipe() {
        let s = Scratch::new("logwipe");
        let state_root = s.state_root().to_path_buf();
        let path = tasks_log_path(&state_root).unwrap();

        transition(&path, "x", None, Lifecycle::Created, Actor::Operator, 1).unwrap();
        transition(
            &path,
            "x",
            Some(Lifecycle::Created),
            Lifecycle::Doing,
            Actor::Operator,
            2,
        )
        .unwrap();

        fs::remove_dir_all(&state_root).unwrap(); // the D7 startup wipe
        assert!(!state_root.exists(), "state_root wipe did not happen");
        assert!(
            path.exists(),
            "transition log must survive the state_root wipe"
        );

        let log = read_transitions(&path);
        assert_eq!(log.len(), 2, "full transition history must persist");
        assert_eq!(current_lifecycle(&log, "x"), Lifecycle::Doing);
    }

    /// 13. `tasks_log_path` derivation: sibling of `state_root`, distinct from
    ///     `tasks_path`; no parent → `None`.
    #[test]
    fn tasks_log_path_is_sibling_of_state_root() {
        assert_eq!(
            tasks_log_path(Path::new("/var/app/agent-teams")),
            Some(PathBuf::from("/var/app/agent-teams-tasks-log.jsonl"))
        );
        // Distinct from the mutable store sibling.
        assert_ne!(
            tasks_log_path(Path::new("/var/app/agent-teams")),
            tasks_path(Path::new("/var/app/agent-teams"))
        );
        assert_eq!(tasks_log_path(Path::new("/")), None);
    }

    /// 14. The `Actor` wire form is self-describing (internally tagged) and
    ///     round-trips — the attributable "who" half of the audit.
    #[test]
    fn actor_wire_form_round_trips() {
        let pane = Actor::Pane {
            workspace_id: "ws7".into(),
        };
        let op = Actor::Operator;
        for a in [pane, op] {
            let json = serde_json::to_string(&a).unwrap();
            assert!(
                json.contains("\"kind\""),
                "actor is internally tagged: {json}"
            );
            let back: Actor = serde_json::from_str(&json).unwrap();
            assert_eq!(back, a);
        }
    }

    // ─── Agent-create path + caps (item 4b enablement — C4 / C6) ─────────────

    /// 15. C4 — `create_task` appends a GENESIS line that carries the title to the
    ///     APPEND-ONLY log (never the mutable store). `genesis_title` reads it back,
    ///     `current_lifecycle` folds to `Created`, and NO `tasks.jsonl` is written
    ///     (the lost-update-prone file is untouched — the race is gone).
    #[test]
    fn create_task_routes_title_to_log_not_store() {
        let s = Scratch::new("createtask");
        let log_path = tasks_log_path(s.state_root()).unwrap();
        let store_path = tasks_path(s.state_root()).unwrap();

        create_task(
            &log_path,
            "task_1",
            "wire the widget",
            Actor::Pane {
                workspace_id: "ws1-p0".into(),
            },
            1_000,
        )
        .unwrap();

        let log = read_transitions(&log_path);
        assert_eq!(log.len(), 1, "exactly one genesis line appended");
        assert_eq!(
            genesis_title(&log, "task_1").as_deref(),
            Some("wire the widget")
        );
        assert_eq!(current_lifecycle(&log, "task_1"), Lifecycle::Created);
        // The mutable store was NEVER touched — C4: no full-file rewrite on create.
        assert!(
            !store_path.exists(),
            "create_task must NOT write the mutable tasks.jsonl store"
        );
        // A non-genesis transition carries no title.
        transition(
            &log_path,
            "task_1",
            Some(Lifecycle::Created),
            Lifecycle::Doing,
            Actor::Operator,
            2,
        )
        .unwrap();
        let log = read_transitions(&log_path);
        assert_eq!(
            genesis_title(&log, "task_1").as_deref(),
            Some("wire the widget"),
            "title stays on the genesis line; later transitions don't carry it"
        );
        assert!(
            log[1].title.is_none(),
            "non-genesis transition has title=None"
        );
    }

    /// 16. C6 — `create_task` REJECTS an oversize title and appends NOTHING.
    #[test]
    fn create_task_rejects_oversize_title() {
        let s = Scratch::new("oversizetitle");
        let log_path = tasks_log_path(s.state_root()).unwrap();
        let big = "x".repeat(MAX_TASK_TITLE_BYTES + 1);

        let err = create_task(&log_path, "task_big", &big, Actor::Operator, 1)
            .expect_err("oversize title must be rejected");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(
            read_transitions(&log_path).len(),
            0,
            "a rejected oversize-title create must append NOTHING"
        );
        // The boundary value (exactly MAX) is accepted.
        let ok = "y".repeat(MAX_TASK_TITLE_BYTES);
        create_task(&log_path, "task_ok", &ok, Actor::Operator, 2).unwrap();
        assert_eq!(read_transitions(&log_path).len(), 1);
        assert!(validate_title("").is_ok(), "an empty title is allowed");
    }

    /// 17. `log_task_ids` returns the DISTINCT genesis-line task ids in first-seen
    ///     order — the agent-created tasks a reader unions with the store.
    #[test]
    fn log_task_ids_lists_genesis_tasks_in_order() {
        let s = Scratch::new("logids");
        let log_path = tasks_log_path(s.state_root()).unwrap();
        create_task(&log_path, "t_a", "a", Actor::Operator, 1).unwrap();
        create_task(&log_path, "t_b", "b", Actor::Operator, 2).unwrap();
        // A non-genesis transition for t_a must NOT add a duplicate id.
        transition(
            &log_path,
            "t_a",
            Some(Lifecycle::Created),
            Lifecycle::Doing,
            Actor::Operator,
            3,
        )
        .unwrap();
        create_task(&log_path, "t_c", "c", Actor::Operator, 4).unwrap();

        let log = read_transitions(&log_path);
        assert_eq!(
            log_task_ids(&log),
            vec!["t_a".to_string(), "t_b".to_string(), "t_c".to_string()],
            "distinct genesis ids in append order; a later transition adds no dup"
        );
    }

    /// 18. BACK-COMPAT — a genesis line serialized in the OLD format (no `title`
    ///     field) deserializes with `title == None` (the `#[serde(default)]`),
    ///     so the field add never breaks an existing on-disk log.
    #[test]
    fn old_format_genesis_line_deserializes_to_none_title() {
        let old =
            r#"{"task_id":"t_old","from":null,"to":"created","by":{"kind":"operator"},"at":7}"#;
        let t: Transition = serde_json::from_str(old).expect("old line must still parse");
        assert_eq!(t.task_id, "t_old");
        assert!(
            t.title.is_none(),
            "a pre-enablement genesis line folds title→None"
        );
        assert!(t.from.is_none());
        assert_eq!(t.to, Lifecycle::Created);
    }

    // ─── Adversarial / invariant hardening (C6 caps, lifecycle SSOT) ──────────

    /// 19. C6 — `validate_title` bounds **BYTES, not chars** (`String::len` is the
    ///     UTF-8 byte length). The pre-existing oversize test (16) is pure ASCII, so
    ///     bytes == chars there and a `chars().count()` refactor would pass it while
    ///     silently 4×-ing the real cap. This pins the byte semantics with multibyte
    ///     corpora at and across the boundary:
    ///     - a SMALL char-count title whose BYTE length is MAX+1 must be REJECTED;
    ///     - exactly MAX bytes (built from 4-byte chars, char-count = MAX/4) accepts;
    ///     - MAX+1 bytes via a 1-byte tail over a 4-byte body rejects (off-by-one).
    #[test]
    fn validate_title_counts_bytes_not_chars() {
        // '𝕏' (U+1D54F MATHEMATICAL DOUBLE-STRUCK CAPITAL X) is 4 UTF-8 bytes.
        let four_byte = '𝕏';
        assert_eq!(
            four_byte.len_utf8(),
            4,
            "corpus assumption: char is 4 bytes"
        );

        // Few CHARS, but byte length = MAX+1 → REJECTED (a char-count cap would pass).
        let over_chars = (MAX_TASK_TITLE_BYTES / 4) + 1; // chars
        let small_char_big_byte: String = std::iter::repeat_n(four_byte, over_chars).collect();
        assert!(
            small_char_big_byte.chars().count() < small_char_big_byte.len(),
            "multibyte: char-count must be far below byte-count"
        );
        assert!(
            small_char_big_byte.len() > MAX_TASK_TITLE_BYTES,
            "corpus must exceed the byte cap"
        );
        assert!(
            validate_title(&small_char_big_byte).is_err(),
            "a small-char-count but oversize-BYTE title MUST be rejected (bytes, not chars)"
        );

        // Exactly MAX bytes, built from 4-byte chars (char-count = MAX/4) → ACCEPTED.
        assert_eq!(
            MAX_TASK_TITLE_BYTES % 4,
            0,
            "corpus assumption: MAX divisible by 4"
        );
        let exactly_max: String =
            std::iter::repeat_n(four_byte, MAX_TASK_TITLE_BYTES / 4).collect();
        assert_eq!(
            exactly_max.len(),
            MAX_TASK_TITLE_BYTES,
            "exactly the byte cap"
        );
        assert!(
            exactly_max.chars().count() < MAX_TASK_TITLE_BYTES,
            "but far fewer chars"
        );
        assert!(
            validate_title(&exactly_max).is_ok(),
            "exactly MAX bytes (multibyte) is the accepted boundary"
        );

        // MAX-3 four-byte body + a 4-byte char = MAX+1 bytes → REJECTED (off-by-one,
        // and a char straddling the boundary must be counted whole, not truncated).
        let body: String = std::iter::repeat_n(four_byte, (MAX_TASK_TITLE_BYTES / 4) - 1).collect(); // MAX-4 bytes
        let mut straddle = body;
        straddle.push('a'); // +1 = MAX-3 bytes
        straddle.push(four_byte); // +4 = MAX+1 bytes
        assert_eq!(
            straddle.len(),
            MAX_TASK_TITLE_BYTES + 1,
            "one byte over the cap"
        );
        assert!(
            validate_title(&straddle).is_err(),
            "MAX+1 bytes must be rejected even when the overflowing char is multibyte"
        );

        // create_task must enforce the SAME byte cap (not just validate_title in
        // isolation): the small-char/big-byte title appends NOTHING.
        let s = Scratch::new("validatebytes");
        let log_path = tasks_log_path(s.state_root()).unwrap();
        let err = create_task(&log_path, "tb", &small_char_big_byte, Actor::Operator, 1)
            .expect_err("create_task must reject an oversize-BYTE title");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(
            read_transitions(&log_path).len(),
            0,
            "a byte-oversize title create must append NOTHING"
        );
    }

    /// 20. THE MARQUEE ADVERSARIAL TEST — a stateful xorshift fuzz over `transition()`
    ///     proving the **disk == in-memory model** invariant under sustained random
    ///     input (the axis the full truth-table (6) and the hand-rolled illegal-edge
    ///     reject (9) do NOT cover — they are single-step / static).
    ///
    ///     For K rounds we draw a random target `to`; we compute the *expected* result
    ///     from a PURE shadow model (`valid_transition(current, to)`), drive the real
    ///     `transition()`, and assert lock-step:
    ///     - a LEGAL edge ⇒ `Ok`, the on-disk line count grows by exactly 1, and the
    ///       re-folded `current_lifecycle` advances to `to`;
    ///     - an ILLEGAL edge ⇒ `Err(InvalidData)`, the on-disk line count is
    ///       UNCHANGED, and the folded lifecycle is UNCHANGED.
    ///
    ///     At the end: `current_lifecycle(read_back) == model` and total lines ==
    ///     accepted count. Deterministic (fixed seed), std-only (no `rand`/`proptest`).
    ///     Swap the validate-then-append for an append-always and this MUST break
    ///     (illegal edges would grow the file and desync disk from the model).
    #[test]
    fn transition_fuzz_disk_matches_model() {
        use Lifecycle::*;

        // std-only deterministic PRNG (xorshift64*). No new deps.
        struct XorShift(u64);
        impl XorShift {
            fn next_u64(&mut self) -> u64 {
                let mut x = self.0;
                x ^= x >> 12;
                x ^= x << 25;
                x ^= x >> 27;
                self.0 = x;
                x.wrapping_mul(0x2545_F491_4F6C_DD1D)
            }
        }

        let all = [Created, Assigned, Doing, Review, Done];
        let s = Scratch::new("fuzz");
        let path = tasks_log_path(s.state_root()).unwrap();

        // Genesis (legal) so the task exists; the model starts at Created.
        transition(&path, "f", None, Created, Actor::Operator, 0).unwrap();
        let mut model = Created;
        let mut accepted: usize = 1; // the genesis line counts
        let mut legal_seen = 0usize;
        let mut illegal_seen = 0usize;

        let mut rng = XorShift(0x9E37_79B9_7F4A_7C15); // fixed seed → deterministic
        const K: usize = 4_000;
        for round in 0..K {
            let to = all[(rng.next_u64() as usize) % all.len()];
            let expected_legal = valid_transition(model, to);

            let res = transition(
                &path,
                "f",
                Some(model),
                to,
                Actor::Operator,
                (round + 1) as u64,
            );
            let lines_before_read = read_transitions(&path);
            let line_count = lines_before_read.len();

            if expected_legal {
                legal_seen += 1;
                res.unwrap_or_else(|e| {
                    panic!("round {round}: {model}→{to} legal but errored: {e}")
                });
                accepted += 1;
                model = to; // model advances only on an accepted edge
                assert_eq!(
                    line_count, accepted,
                    "round {round}: legal {model}→{to} must append exactly one line"
                );
            } else {
                illegal_seen += 1;
                let err = res.expect_err(&format!("round {round}: {model}→{to} is illegal"));
                assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
                assert_eq!(
                    line_count, accepted,
                    "round {round}: ILLEGAL {model}→{to} must append NOTHING (disk unchanged)"
                );
                // model is UNCHANGED on a rejected edge.
            }

            // The load-bearing invariant: the folded on-disk lifecycle == the model,
            // re-derived every round (no drift accumulates).
            assert_eq!(
                current_lifecycle(&lines_before_read, "f"),
                model,
                "round {round}: on-disk fold must equal the in-memory model"
            );
        }

        // The fuzz must have actually exercised BOTH branches (else it proves nothing).
        assert!(
            legal_seen > 0,
            "fuzz never took a legal edge — corpus too narrow"
        );
        assert!(
            illegal_seen > 0,
            "fuzz never took an illegal edge — reject path untested"
        );

        // Final ground truth from a cold re-read.
        let final_log = read_transitions(&path);
        assert_eq!(
            final_log.len(),
            accepted,
            "total on-disk lines == accepted edges (every illegal edge appended nothing)"
        );
        assert_eq!(
            current_lifecycle(&final_log, "f"),
            model,
            "cold re-read lifecycle == the model after {K} fuzzed rounds"
        );
    }

    /// 21. C6 — `MAX_TASKS` is the genesis-count bound. Build a log of exactly
    ///     `MAX_TASKS` distinct genesis lines IN MEMORY (no disk — this asserts the
    ///     consts/predicate the MCP `at_task_cap` reuses) and prove `log_task_ids`
    ///     counts genesis lines (the count the boundary caps): MAX-1 genesis ids is
    ///     UNDER the cap, MAX is AT it. The pre-existing mcp test only covers the
    ///     far-from-cap negative; this pins the threshold itself.
    #[test]
    fn log_task_ids_counts_genesis_up_to_max_tasks() {
        // MAX-1 distinct genesis lines → under the cap.
        let mut log: Vec<Transition> = (0..MAX_TASKS - 1)
            .map(|i| Transition {
                task_id: format!("t{i}"),
                from: None,
                to: Lifecycle::Created,
                by: Actor::Operator,
                at: i as u64,
                title: None,
            })
            .collect();
        // Non-genesis lines must NOT inflate the genesis count (the cap counts tasks,
        // not transitions) — interleave a few advances for an existing task.
        log.push(Transition {
            task_id: "t0".into(),
            from: Some(Lifecycle::Created),
            to: Lifecycle::Doing,
            by: Actor::Operator,
            at: 1,
            title: None,
        });
        assert_eq!(
            log_task_ids(&log).len(),
            MAX_TASKS - 1,
            "genesis count is distinct genesis lines, ignoring non-genesis transitions"
        );

        // Add the MAX-th genesis → exactly at the cap.
        log.push(Transition {
            task_id: format!("t{}", MAX_TASKS - 1),
            from: None,
            to: Lifecycle::Created,
            by: Actor::Operator,
            at: 0,
            title: None,
        });
        assert_eq!(
            log_task_ids(&log).len(),
            MAX_TASKS,
            "genesis count reaches MAX_TASKS at the cap boundary"
        );
    }

    // ─── R3 data-integrity: serde fallbacks + shrink guard (no silent erase) ──

    /// 22. `Column` gains `#[serde(other)] Unknown`: an unknown/future column token
    ///     deserializes to `Unknown` (so the whole `Task` line still parses) rather
    ///     than erroring and being dropped on the next full-file rewrite.
    #[test]
    fn unknown_column_folds_to_unknown_not_error() {
        let line = r#"{"id":"t1","title":"future","column":"archived","order":0,"created_at":1,"updated_at":1}"#;
        let task: Task = serde_json::from_str(line).expect("unknown column must still parse");
        assert_eq!(task.column, Column::Unknown);
        assert_eq!(task.id, "t1");
        // The known values still map to themselves (fallback did not shadow them).
        let known: Task = serde_json::from_str(
            r#"{"id":"t2","title":"x","column":"doing","order":0,"created_at":1,"updated_at":1}"#,
        )
        .unwrap();
        assert_eq!(known.column, Column::Doing);
    }

    /// 23. `Lifecycle` gains `#[serde(other)] Unknown`: a future `to` value on the
    ///     append-only log parses to `Unknown` (so `fold` reflects "advanced past
    ///     the known graph") instead of the line being dropped and the fold silently
    ///     reverting to the prior stage.
    #[test]
    fn unknown_lifecycle_folds_to_unknown_not_dropped() {
        let s = Scratch::new("unknownlc");
        let path = tasks_log_path(s.state_root()).unwrap();
        // Genesis + a future-stage line written by a newer writer.
        append_transition(&path, &trans("x", None, Lifecycle::Created, 1)).unwrap();
        let future =
            r#"{"task_id":"x","from":"created","to":"archived","by":{"kind":"operator"},"at":2}"#;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(format!("{future}\n").as_bytes())
            .unwrap();

        let log = read_transitions(&path);
        assert_eq!(
            log.len(),
            2,
            "the future-stage line still parses (not dropped)"
        );
        assert_eq!(log[1].to, Lifecycle::Unknown);
        assert_eq!(
            current_lifecycle(&log, "x"),
            Lifecycle::Unknown,
            "fold reflects the unknown advance, not a silent revert to Created"
        );
    }

    /// 24. `Actor` gains an `#[serde(other)] Unknown` arm: a future `kind` on a
    ///     transition line parses to `Actor::Unknown` (line kept) rather than
    ///     failing to parse (line dropped → task lost from the log).
    #[test]
    fn unknown_actor_kind_folds_to_unknown() {
        let line = r#"{"task_id":"t","from":null,"to":"created","by":{"kind":"scheduler"},"at":9}"#;
        let t: Transition =
            serde_json::from_str(line).expect("unknown actor kind must still parse");
        assert_eq!(t.by, Actor::Unknown);
        assert!(t.from.is_none());
        assert_eq!(t.to, Lifecycle::Created);
        // Known kinds still resolve to their concrete variants.
        let op: Transition = serde_json::from_str(
            r#"{"task_id":"t","from":null,"to":"created","by":{"kind":"operator"},"at":1}"#,
        )
        .unwrap();
        assert_eq!(op.by, Actor::Operator);
    }

    /// 25. THE ANTI-ERASE GUARD — a store file with one GOOD + one genuinely
    ///     unparseable (malformed JSON) line: an `upsert_task` must NOT rewrite the
    ///     file and silently erase the unparseable line. `write_tasks`/`upsert_task`
    ///     return `Err` and the on-disk bytes are preserved verbatim.
    #[test]
    fn upsert_does_not_erase_unparseable_line() {
        let s = Scratch::new("noerase");
        let path = tasks_path(s.state_root()).unwrap();
        let good = r#"{"id":"good","title":"keep","column":"doing","order":0,"created_at":1,"updated_at":1}"#;
        let corrupt = r#"{"id":"corrupt", THIS IS NOT JSON"#;
        let original = format!("{good}\n{corrupt}\n");
        fs::write(&path, &original).unwrap();

        // upsert would read 1 good task, drop the corrupt line, and rewrite → erase.
        // The guard must refuse.
        let err = upsert_task(&path, &sample("new", Column::Backlog, 5))
            .expect_err("upsert must refuse to rewrite a store with unparseable lines");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);

        // The file is byte-for-byte preserved — nothing erased, nothing added.
        let after = fs::read_to_string(&path).unwrap();
        assert_eq!(
            after, original,
            "the unparseable line (and the good one) must survive untouched"
        );

        // A clean store (all lines parse) still rewrites normally.
        fs::write(&path, format!("{good}\n")).unwrap();
        upsert_task(&path, &sample("new", Column::Backlog, 5)).unwrap();
        let back = read_tasks(&path);
        assert_eq!(back.len(), 2, "a clean store rewrites as before");
    }
}
