//! Persisted **Agent** persona record store — Phase 14-T4.
//!
//! # Why this crate exists
//!
//! An "Agent" here is a named persona definition a human configures and reuses
//! across workspaces: a name, a role hint, a system-prompt fragment, and optional
//! metadata tags. This is distinct from a *pane* (an ephemeral live PTY process)
//! or a *task* (a work-item with a lifecycle). An agent record is:
//!
//! - **Human-owned** — created/edited by the operator, not auto-minted by the machine.
//! - **Persistent across restarts** — stored in a sibling-of-`state_root` directory
//!   so it survives the D7 startup wipe of `state_root` (same durability contract as
//!   `runs.jsonl`, `agent-teams-live.json`, and the task store in `core/task`).
//! - **Read by the Tauri app** to populate a future "start with persona" flow —
//!   currently projected via `create_agent` / `list_agents` Tauri commands (14-T4).
//!   **IMPORTANT**: creating an agent record does NOT spawn a Supervisor or PTY; it
//!   is a pure data write with no process lifecycle side-effect.
//!
//! # Durability contract (load-bearing)
//!
//! Agent records live in a **directory** that is a **SIBLING of `state_root`**
//! (`<state_root>/../<name>-agents/`), where `<name>` is `state_root`'s own dir name.
//! Each record is a separate JSON file (`<id>.json`) written with an **atomic rename**
//! (write to `<id>.json.tmp` → `fs::rename` → `<id>.json`), so a crashed mid-write
//! never leaves a partial record visible to the reader. See [`agents_dir`].
//!
//! # Per-record file — why not a JSONL like `core/task`?
//!
//! Tasks use a JSONL store because they are mutable in bulk (reorder, column change
//! rewrites the whole list). Agent records are individually mutable (one user edits
//! one persona) and typically few. A per-file model means:
//! - `create_agent` / `update_agent` touch ONLY the affected record's file → no
//!   read-modify-write-all race on the shared JSONL (the hazard `core/task` documents
//!   in its `upsert_full_rewrite_can_lose_an_update` test).
//! - `read_agent` / `list_agents` are independent readers — no shared mutable cursor.
//! - The atomic rename is `O_CREAT + write + rename` — a single-record update is
//!   consistent on APFS (metadata flush is atomic at the rename boundary).
//!
//! # Seams — what an Agent record is NOT
//!
//! - **Agent ≠ pane.** Panes are live PTY processes tracked in `state_root`.  A
//!   pane MAY carry an `agent_id` referencing an agent record (correlation, not
//!   ownership), but creating an agent record NEVER spawns a PTY or Supervisor.
//! - **Agent ≠ role.** `core/roles` holds the STATIC typed persona enum
//!   (Coordinator/Builder/Scout/Reviewer) injected at pane spawn. Agent records are
//!   USER-EDITABLE named configurations that may reference a role by string — they do
//!   not replace the `core/roles` SSOT.
//! - **Agent ≠ Task.** Tasks are work-items with a lifecycle state machine. An agent
//!   is a reusable persona template — no lifecycle, no transition log, no append-only
//!   channel. The two types are orthogonal (a Task may reference an `agent_id` to
//!   record "this pane ran as this persona", but that link is correlation only).

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Per-process monotonic counter making each atomic-write temp filename UNIQUE, so two
/// writes to the SAME id (same pid, concurrent threads) never collide on one fixed
/// `<id>.json.tmp` and tear each other's file. Mirrors `core/memory`'s `WRITE_NONCE`.
static WRITE_NONCE: AtomicU64 = AtomicU64::new(0);

/// A persisted agent persona record.
///
/// `id` is a stable, caller-supplied identifier (e.g. a UUID or slug, same posture
/// as `RunRecord.id` / `task_create`'s `task_id` — the server mints it). `created_at`
/// / `updated_at` are unix-ms. All optional fields use `#[serde(default)]` so an
/// older/partial record on disk still deserializes and an unknown future field is
/// silently ignored (forward-compat add pattern).
///
/// # Identity seam
///
/// An agent record is the HUMAN-EDITABLE counterpart to a pane's ephemeral `role`
/// injection: a human names and describes a persona once, then references it at
/// spawn time (or in a task). The `role` field here is an ADVISORY string — the
/// Tauri `spawn_workspace` path reads `core/roles::AgentRole` from the pane's
/// `WorkspaceSpec`; that remains the typed injection SSOT. This crate does NOT
/// reference `core/roles` (no dep cycle, no coupling).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agent {
    /// Stable id — caller-supplied (app-minted), never agent-chosen. Must be
    /// non-empty and contain only URL-safe chars (enforced by [`validate_id`]).
    pub id: String,
    /// Human-readable display name (e.g. "Frontend Specialist").
    pub name: String,
    /// Optional free-form system-prompt fragment injected at pane spawn. An empty
    /// string or `None` means "no extra persona prompt" — both round-trip faithfully.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persona_prompt: Option<String>,
    /// Advisory role hint (wire string, e.g. "builder", "reviewer"). Not a typed
    /// `AgentRole` here — see the seams doc-comment. `None` = role-agnostic persona.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// User-supplied tags for filtering/search (e.g. `["rust", "frontend"]`). Empty
    /// by default; the serializer skips an empty vec.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// unix-ms, caller-supplied (matches `Task.created_at` convention).
    pub created_at: u64,
    /// unix-ms, caller-supplied. Equal to `created_at` on the genesis write; bumped
    /// on every update by the caller (this crate has no clock).
    pub updated_at: u64,
}

// ─── Validation ───────────────────────────────────────────────────────────────

/// Max bytes for an agent name (display label). Prevents a pathologically long
/// name from polluting the registry listing or a future API response.
pub const MAX_AGENT_NAME_BYTES: usize = 512;

/// Max bytes for an agent persona prompt. Generous (16 KiB) — enough for a
/// detailed system prompt fragment; longer inputs indicate a bug on the caller.
pub const MAX_PERSONA_PROMPT_BYTES: usize = 16_384;

/// Max agent records in the sibling directory. Bounds filesystem inode growth
/// from a create-loop (mirrors `core/task::MAX_TASKS`).
pub const MAX_AGENTS: usize = 1_000;

/// Allowed id characters: ASCII alphanumeric, `-`, `_`.
///
/// This is deliberately restrictive — the id is used as a filename component
/// (`<id>.json`) and may appear in URLs / MCP tool calls. We do NOT allow `.`,
/// `/`, or any other character that could escape the directory or create a
/// hidden file. An empty id is rejected.
pub fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("agent id must be non-empty".to_string());
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!(
            "agent id {id:?} contains invalid characters (only ASCII alphanumeric, '-', '_' allowed)"
        ));
    }
    Ok(())
}

/// Validate an agent name against [`MAX_AGENT_NAME_BYTES`]. PURE; counts BYTES
/// (UTF-8 length), not chars — mirrors `core/task::validate_title`.
pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("agent name must be non-empty".to_string());
    }
    if name.len() > MAX_AGENT_NAME_BYTES {
        return Err(format!(
            "agent name exceeds {MAX_AGENT_NAME_BYTES} bytes ({} bytes)",
            name.len()
        ));
    }
    Ok(())
}

/// Validate an optional persona prompt against [`MAX_PERSONA_PROMPT_BYTES`].
/// `None` is always valid (the field is optional).
pub fn validate_persona_prompt(p: Option<&str>) -> Result<(), String> {
    if let Some(s) = p {
        if s.len() > MAX_PERSONA_PROMPT_BYTES {
            return Err(format!(
                "persona_prompt exceeds {MAX_PERSONA_PROMPT_BYTES} bytes ({} bytes)",
                s.len()
            ));
        }
    }
    Ok(())
}

// ─── Path helpers ─────────────────────────────────────────────────────────────

/// The agent store directory: `<state_root>/../<name>-agents/` (a **SIBLING** of
/// `state_root`, where `<name>` is `state_root`'s own dir name). `None` when
/// `state_root` has no parent.
///
/// Mirrors `core/task::tasks_path`'s `parent().map(join)` mechanic so a test can
/// inject a tempdir and the real app uses its `state_root` argument — never
/// `$HOME` (clock-free, hermetic).
pub fn agents_dir(state_root: &Path) -> Option<PathBuf> {
    let name = state_root
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "agent-teams".to_string());
    state_root
        .parent()
        .map(|p| p.join(format!("{name}-agents")))
}

/// The on-disk path for a single agent record: `<agents_dir>/<id>.json`.
/// Returns `None` when `state_root` has no parent (mirrors [`agents_dir`]).
///
/// # Safety
///
/// The caller MUST have validated `id` via [`validate_id`] before calling this.
/// The function does NOT re-validate — validation is the caller's responsibility
/// (same posture as `core/task::tasks_path`).
pub fn agent_path(state_root: &Path, id: &str) -> Option<PathBuf> {
    agents_dir(state_root).map(|d| d.join(format!("{id}.json")))
}

// ─── I/O primitives ───────────────────────────────────────────────────────────

/// Ensure the agent store directory exists. Creates it (and any parents) if
/// absent. Called by write operations before the first write. Read operations
/// handle a missing directory by returning `None` / empty `Vec` (tolerant read).
fn ensure_agents_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}

/// Write one agent record to `path` with an **atomic rename**: serialize to JSON,
/// write to a UNIQUE temp in the same dir, then `fs::rename` to `<path>`. A crashed
/// mid-write never leaves a partial file visible to concurrent readers.
///
/// The temp name is `.{id}.json.tmp.{pid}.{nonce}` (mirrors `core/memory`'s
/// `write_note_atomic`): a fixed `<id>.json.tmp` would be TORN if two writers hit the
/// SAME id concurrently (both open+truncate the one temp, interleave writes, then race
/// to rename a corrupt file over the target). The per-write nonce makes each temp
/// distinct, so writers never share a temp and each rename is atomic. The leading `.`
/// + the non-`.json` extension keep it invisible to `list_agents`' `.json` filter.
///
/// Does NOT validate `agent` fields — callers must call `validate_*` first.
fn write_agent_atomic(path: &Path, agent: &Agent) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(agent)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let nonce = WRITE_NONCE.fetch_add(1, Ordering::Relaxed);
    let tmp = path.with_file_name(format!(
        ".{}.json.tmp.{}.{}",
        agent.id,
        std::process::id(),
        nonce
    ));
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&tmp)?;
    f.write_all(json.as_bytes())?;
    f.flush()?;
    drop(f); // ensure the file handle is closed before rename (Windows compat)
             // Atomic same-filesystem replace. On error, clean up the unique temp we created.
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// Create a new agent record. Validates `id`, `name`, and `persona_prompt` before
/// writing. Returns `Err(AlreadyExists)` if a record with this `id` already exists
/// — creation is an idempotency-safe INSERT, not an upsert.
///
/// The write is a directory-create + atomic rename so it is consistent on APFS.
pub fn create_agent(state_root: &Path, agent: &Agent) -> std::io::Result<()> {
    validate_id(&agent.id).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    validate_name(&agent.name)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    validate_persona_prompt(agent.persona_prompt.as_deref())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    // Enforce the documented cap (bounds inode growth from a create-loop; mirrors
    // `core/task::MAX_TASKS`). `list_agents` tolerates a missing dir (returns empty), so
    // this is a no-op on the first create. Checked BEFORE any directory/file write so a
    // rejected create leaves the store untouched.
    if list_agents(state_root).len() >= MAX_AGENTS {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("agent count cap reached (>= {MAX_AGENTS})"),
        ));
    }

    let dir = agents_dir(state_root).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "state_root has no parent")
    })?;
    ensure_agents_dir(&dir)?;

    let path = dir.join(format!("{}.json", agent.id));
    if path.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("agent {:?} already exists", agent.id),
        ));
    }
    write_agent_atomic(&path, agent)
}

/// Update an existing agent record (full replacement). Validates `id`, `name`, and
/// `persona_prompt` before writing. Returns `Err(NotFound)` if the record does not
/// exist — use [`create_agent`] for new records.
pub fn update_agent(state_root: &Path, agent: &Agent) -> std::io::Result<()> {
    validate_id(&agent.id).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    validate_name(&agent.name)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    validate_persona_prompt(agent.persona_prompt.as_deref())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let dir = agents_dir(state_root).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "state_root has no parent")
    })?;
    let path = dir.join(format!("{}.json", agent.id));
    if !path.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("agent {:?} not found", agent.id),
        ));
    }
    write_agent_atomic(&path, agent)
}

/// Read one agent record by id. Returns `None` if the file is absent or
/// unreadable (tolerant read — mirrors `read_tasks`' tolerant contract).
pub fn read_agent(state_root: &Path, id: &str) -> Option<Agent> {
    let path = agent_path(state_root, id)?;
    let content = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str::<Agent>(&content).ok()
}

/// Read all agent records from the sibling directory, sorted by `created_at`
/// ascending (stable order for the `list_agents` Tauri command). Tolerant: skips
/// files that fail to parse (a partial `.tmp` left by a crash, or an unknown
/// future format). Returns an empty `Vec` if the directory is absent.
pub fn list_agents(state_root: &Path) -> Vec<Agent> {
    let Some(dir) = agents_dir(state_root) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut agents: Vec<Agent> = entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext == "json")
                .unwrap_or(false)
        })
        .filter_map(|e| {
            let content = std::fs::read_to_string(e.path()).ok()?;
            serde_json::from_str::<Agent>(&content).ok()
        })
        .collect();
    agents.sort_by_key(|a| (a.created_at, a.id.clone()));
    agents
}

/// Delete an agent record by id. Returns `Err(NotFound)` if absent.
/// Does NOT cascade — a pane that referenced this `agent_id` continues running.
pub fn delete_agent(state_root: &Path, id: &str) -> std::io::Result<()> {
    validate_id(id).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let path = agent_path(state_root, id).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "state_root has no parent")
    })?;
    if !path.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("agent {id:?} not found"),
        ));
    }
    std::fs::remove_file(&path)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A unique scratch dir tree, cleaned on Drop. `state` is NESTED under `root`
    /// (`<root>/state`) so the agent store directory (`<root>/state-agents/`) lands
    /// at the `root` level, OUTSIDE `state`. `remove_dir_all(&state)` (the D7 wipe)
    /// does NOT take the sibling dir with it. Mirrors `core/task`'s `Scratch`.
    struct Scratch {
        root: PathBuf,
        state: PathBuf,
    }
    impl Scratch {
        fn new(tag: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("at-agent-{}-{}", tag, std::process::id()));
            let _ = fs::remove_dir_all(&root);
            let state = root.join("state");
            fs::create_dir_all(&state).unwrap();
            Scratch { root, state }
        }
        /// The `state_root` handed to the store functions.
        fn state_root(&self) -> &Path {
            &self.state
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn sample(id: &str) -> Agent {
        Agent {
            id: id.to_string(),
            name: format!("Agent {id}"),
            persona_prompt: Some(format!("You are the {id} specialist.")),
            role: Some("builder".to_string()),
            tags: vec!["rust".to_string(), "backend".to_string()],
            created_at: 1_000,
            updated_at: 1_000,
        }
    }

    // ─── Path helpers ──────────────────────────────────────────────────────────

    /// 1. `agents_dir` is a sibling of `state_root` — not inside it — and `None`
    ///    when `state_root` has no parent (mirrors `core/task::tasks_path_is_sibling`).
    #[test]
    fn agents_dir_is_sibling_of_state_root() {
        assert_eq!(
            agents_dir(Path::new("/var/app/agent-teams")),
            Some(PathBuf::from("/var/app/agent-teams-agents")),
        );
        assert_eq!(agents_dir(Path::new("/")), None);
    }

    /// 2. `agent_path` composes `agents_dir` + `<id>.json` and shares the `None`
    ///    behaviour when `state_root` has no parent.
    #[test]
    fn agent_path_composes_dir_and_id() {
        assert_eq!(
            agent_path(Path::new("/var/app/agent-teams"), "my-agent"),
            Some(PathBuf::from("/var/app/agent-teams-agents/my-agent.json")),
        );
        assert_eq!(agent_path(Path::new("/"), "x"), None);
    }

    // ─── Round-trip ────────────────────────────────────────────────────────────

    /// 3. THE CRITICAL CREATE/READ ROUND-TRIP — create then read back and assert
    ///    the record is byte-for-byte identical. Covers the atomic-rename write path
    ///    and the JSON round-trip for all fields (including optional ones).
    #[test]
    fn create_then_read_round_trips() {
        let s = Scratch::new("roundtrip");
        let agent = Agent {
            tags: vec!["alpha".to_string(), "beta".to_string()],
            ..sample("agent-42")
        };
        create_agent(s.state_root(), &agent).unwrap();
        let back = read_agent(s.state_root(), "agent-42").expect("must read back what we wrote");
        assert_eq!(back, agent, "round-trip must be byte-for-byte identical");
    }

    /// 4. `list_agents` returns all written records sorted by `created_at` ascending.
    #[test]
    fn list_agents_returns_all_sorted_by_created_at() {
        let s = Scratch::new("list");
        // Write in reverse order of created_at so sorting is visibly needed.
        create_agent(
            s.state_root(),
            &Agent {
                created_at: 300,
                updated_at: 300,
                ..sample("c")
            },
        )
        .unwrap();
        create_agent(
            s.state_root(),
            &Agent {
                created_at: 100,
                updated_at: 100,
                ..sample("a")
            },
        )
        .unwrap();
        create_agent(
            s.state_root(),
            &Agent {
                created_at: 200,
                updated_at: 200,
                ..sample("b")
            },
        )
        .unwrap();

        let list = list_agents(s.state_root());
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].id, "a");
        assert_eq!(list[1].id, "b");
        assert_eq!(list[2].id, "c");
    }

    /// 5. `list_agents` returns an empty Vec when the agents directory is absent
    ///    (tolerant read — no directory → no agents, never an error).
    #[test]
    fn list_agents_tolerates_missing_dir() {
        let s = Scratch::new("emptydir");
        // Never write anything — the sibling dir will not exist.
        assert!(
            list_agents(s.state_root()).is_empty(),
            "absent agents dir must yield an empty list, never an error"
        );
    }

    // ─── State-root wipe survival ──────────────────────────────────────────────

    /// 6. THE CRITICAL WIPE TEST — the agent store is a SIBLING of `state_root`,
    ///    so wiping `state_root` (the D7 startup wipe) must leave all records intact.
    ///    This is the primary correctness property of the store design.
    #[test]
    fn survives_state_root_wipe() {
        let s = Scratch::new("wipe");
        let state_root = s.state_root().to_path_buf();

        create_agent(&state_root, &sample("survivor")).unwrap();
        assert!(
            read_agent(&state_root, "survivor").is_some(),
            "sanity: record must exist before wipe"
        );

        // Simulate the D7 startup wipe.
        fs::remove_dir_all(&state_root).unwrap();
        assert!(!state_root.exists(), "state_root wipe did not happen");

        // The sibling directory and its records must be untouched.
        let dir = agents_dir(&state_root).unwrap();
        assert!(dir.exists(), "agents dir must survive the state_root wipe");
        assert!(
            read_agent(&state_root, "survivor").is_some(),
            "agent record must survive the state_root wipe"
        );
        assert_eq!(list_agents(&state_root).len(), 1);
    }

    // ─── Atomic rename ─────────────────────────────────────────────────────────

    /// 7. A `.tmp` file left by a crashed mid-write does NOT appear in `list_agents`
    ///    (the `.json` extension filter excludes `.json.tmp` files).
    #[test]
    fn list_agents_skips_tmp_files() {
        let s = Scratch::new("tmpleak");
        create_agent(s.state_root(), &sample("real")).unwrap();

        // Simulate a crashed write: a leftover `.tmp` file.
        let dir = agents_dir(s.state_root()).unwrap();
        let tmp = dir.join("crashed.json.tmp");
        fs::write(&tmp, b"{}").unwrap();

        let list = list_agents(s.state_root());
        assert_eq!(
            list.len(),
            1,
            "only the fully-written record (extension=.json)"
        );
        assert_eq!(list[0].id, "real");
    }

    // ─── create / update / delete semantics ────────────────────────────────────

    /// 8. `create_agent` returns `AlreadyExists` on a duplicate id — it is a
    ///    clean INSERT, not an upsert.
    #[test]
    fn create_rejects_duplicate_id() {
        let s = Scratch::new("dup");
        create_agent(s.state_root(), &sample("dup-agent")).unwrap();
        let err = create_agent(s.state_root(), &sample("dup-agent"))
            .expect_err("second create with same id must fail");
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
    }

    /// 9. `update_agent` replaces the record in place. Read back after update
    ///    returns the NEW values, and the list has exactly one record.
    #[test]
    fn update_replaces_record() {
        let s = Scratch::new("update");
        create_agent(s.state_root(), &sample("upd")).unwrap();
        let updated = Agent {
            name: "Updated Name".to_string(),
            role: Some("reviewer".to_string()),
            updated_at: 9_999,
            ..sample("upd")
        };
        update_agent(s.state_root(), &updated).unwrap();

        let back = read_agent(s.state_root(), "upd").unwrap();
        assert_eq!(back.name, "Updated Name");
        assert_eq!(back.role.as_deref(), Some("reviewer"));
        assert_eq!(back.updated_at, 9_999);
        assert_eq!(
            list_agents(s.state_root()).len(),
            1,
            "update must not create a second record"
        );
    }

    /// 10. `update_agent` returns `NotFound` when the record does not exist.
    #[test]
    fn update_returns_not_found_for_absent_record() {
        let s = Scratch::new("updatenotfound");
        let err = update_agent(s.state_root(), &sample("ghost"))
            .expect_err("update on absent record must fail");
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    /// 11. `delete_agent` removes the record; a subsequent `read_agent` returns
    ///     `None` and `list_agents` returns an empty list.
    #[test]
    fn delete_removes_record() {
        let s = Scratch::new("delete");
        create_agent(s.state_root(), &sample("del-me")).unwrap();
        delete_agent(s.state_root(), "del-me").unwrap();
        assert!(
            read_agent(s.state_root(), "del-me").is_none(),
            "deleted record must not be readable"
        );
        assert!(
            list_agents(s.state_root()).is_empty(),
            "deleted record must not appear in list"
        );
    }

    /// 12. `delete_agent` returns `NotFound` when the record does not exist.
    #[test]
    fn delete_returns_not_found_for_absent_record() {
        let s = Scratch::new("delnotfound");
        let err =
            delete_agent(s.state_root(), "ghost").expect_err("delete of absent record must fail");
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    // ─── Validation ────────────────────────────────────────────────────────────

    /// 13. `validate_id` rejects an empty string, ids with forbidden characters
    ///     (`.`, `/`, space, `@`), and accepts valid ids (alphanumeric + `-` + `_`).
    #[test]
    fn validate_id_rejects_invalid_and_accepts_valid() {
        assert!(validate_id("").is_err(), "empty id must be rejected");
        for bad in &["has.dot", "has/slash", "has space", "has@at", "has!bang"] {
            assert!(validate_id(bad).is_err(), "{bad:?} must be rejected");
        }
        for good in &["abc", "my-agent", "my_agent", "Agent42", "a-b_c-1"] {
            assert!(validate_id(good).is_ok(), "{good:?} must be accepted");
        }
    }

    /// 14. `create_agent` propagates id-validation failure before touching the disk
    ///     — the agents directory must NOT be created by a rejected call.
    #[test]
    fn create_agent_validates_id_before_writing() {
        let s = Scratch::new("badid");
        let bad_agent = Agent {
            id: "bad id/hack".to_string(),
            ..sample("placeholder")
        };
        let err = create_agent(s.state_root(), &bad_agent)
            .expect_err("invalid id must be rejected before any I/O");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        // The agents directory must NOT exist (no partial write).
        let dir = agents_dir(s.state_root()).unwrap();
        assert!(
            !dir.exists(),
            "agents dir must NOT be created when id validation fails"
        );
    }

    /// 15. `validate_name` rejects empty and oversize-byte names; boundary values
    ///     (exactly `MAX_AGENT_NAME_BYTES`) are accepted.
    #[test]
    fn validate_name_rejects_empty_and_oversize() {
        assert!(validate_name("").is_err(), "empty name must be rejected");
        let ok = "x".repeat(MAX_AGENT_NAME_BYTES);
        assert!(
            validate_name(&ok).is_ok(),
            "exactly MAX bytes must be accepted"
        );
        let over = "x".repeat(MAX_AGENT_NAME_BYTES + 1);
        assert!(
            validate_name(&over).is_err(),
            "MAX+1 bytes must be rejected"
        );
    }

    /// 16. `validate_name` counts BYTES not chars (mirrors `core/task::validate_title`
    ///     — a char-count refactor would silently 4× the real cap on multibyte input).
    #[test]
    fn validate_name_counts_bytes_not_chars() {
        // '𝕏' (U+1D54F) is 4 UTF-8 bytes. Few chars, many bytes.
        let four_byte = '𝕏';
        assert_eq!(four_byte.len_utf8(), 4);
        let small_char_big_byte: String =
            std::iter::repeat_n(four_byte, MAX_AGENT_NAME_BYTES / 4 + 1).collect();
        assert!(
            small_char_big_byte.chars().count() < small_char_big_byte.len(),
            "multibyte: char-count must be below byte-count"
        );
        assert!(
            validate_name(&small_char_big_byte).is_err(),
            "a small-char-count but oversize-BYTE name must be rejected"
        );
    }

    /// 17. `validate_persona_prompt` accepts `None` and byte-under-cap strings,
    ///     and rejects strings that exceed `MAX_PERSONA_PROMPT_BYTES`.
    #[test]
    fn validate_persona_prompt_caps() {
        assert!(validate_persona_prompt(None).is_ok());
        assert!(validate_persona_prompt(Some("")).is_ok());
        let ok = "p".repeat(MAX_PERSONA_PROMPT_BYTES);
        assert!(validate_persona_prompt(Some(&ok)).is_ok());
        let over = "p".repeat(MAX_PERSONA_PROMPT_BYTES + 1);
        assert!(validate_persona_prompt(Some(&over)).is_err());
    }

    // ─── Optional-field serde ─────────────────────────────────────────────────

    /// 18. An `Agent` with all optional fields absent (minimal record) round-trips
    ///     without phantom fields in the JSON — `skip_serializing_if` works.
    #[test]
    fn minimal_agent_serde_round_trip() {
        let minimal = Agent {
            id: "min".to_string(),
            name: "Minimal".to_string(),
            persona_prompt: None,
            role: None,
            tags: vec![],
            created_at: 42,
            updated_at: 42,
        };
        let json = serde_json::to_string(&minimal).unwrap();
        // Optional fields skipped — no "persona_prompt", "role", or "tags" key.
        assert!(
            !json.contains("persona_prompt"),
            "absent optional must not appear in JSON: {json}"
        );
        assert!(
            !json.contains("\"role\""),
            "absent optional must not appear in JSON: {json}"
        );
        assert!(
            !json.contains("tags"),
            "empty tags must not appear in JSON: {json}"
        );

        let back: Agent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, minimal, "minimal record must round-trip");
    }

    /// 19. BACK-COMPAT — a record serialized WITHOUT the optional fields (older
    ///     format) still deserializes with them as `None`/empty via `#[serde(default)]`.
    #[test]
    fn old_format_record_deserializes_with_defaults() {
        let old = r#"{"id":"old","name":"Old Agent","created_at":1,"updated_at":1}"#;
        let a: Agent = serde_json::from_str(old).expect("old format must still parse");
        assert_eq!(a.id, "old");
        assert!(a.persona_prompt.is_none());
        assert!(a.role.is_none());
        assert!(a.tags.is_empty());
    }

    // ─── Multi-record isolation ───────────────────────────────────────────────

    /// 20. Writing N records produces exactly N distinct files; each is independently
    ///     readable by id; a delete of one leaves N-1 and does not corrupt the others.
    #[test]
    fn multi_record_independence() {
        let s = Scratch::new("multi");
        for i in 0..5u32 {
            create_agent(
                s.state_root(),
                &Agent {
                    created_at: i as u64,
                    updated_at: i as u64,
                    ..sample(&format!("agent-{i}"))
                },
            )
            .unwrap();
        }
        assert_eq!(list_agents(s.state_root()).len(), 5);

        // Delete the middle record.
        delete_agent(s.state_root(), "agent-2").unwrap();
        assert_eq!(list_agents(s.state_root()).len(), 4);
        assert!(read_agent(s.state_root(), "agent-2").is_none());

        // Remaining records are unaffected.
        for i in [0u32, 1, 3, 4] {
            let a = read_agent(s.state_root(), &format!("agent-{i}"))
                .expect("surviving record must still be readable");
            assert_eq!(a.id, format!("agent-{i}"));
        }
    }

    // ─── Cap enforcement ──────────────────────────────────────────────────────

    /// 21. `create_agent` enforces `MAX_AGENTS`: at the cap a further create fails
    ///     with `InvalidInput` and writes NO new record (count stays at the cap).
    #[test]
    fn create_agent_enforces_max_agents_cap() {
        let s = Scratch::new("cap");
        for i in 0..MAX_AGENTS {
            create_agent(
                s.state_root(),
                &Agent {
                    created_at: i as u64,
                    updated_at: i as u64,
                    ..sample(&format!("agent-{i}"))
                },
            )
            .unwrap();
        }
        assert_eq!(list_agents(s.state_root()).len(), MAX_AGENTS);

        let err = create_agent(s.state_root(), &sample("one-too-many"))
            .expect_err("create at the cap must fail");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert_eq!(
            list_agents(s.state_root()).len(),
            MAX_AGENTS,
            "a rejected create must not add a record"
        );
        assert!(
            read_agent(s.state_root(), "one-too-many").is_none(),
            "the rejected record must not be on disk"
        );
    }

    // ─── Atomic unique-temp write ─────────────────────────────────────────────

    /// 22. The atomic write uses a UNIQUE temp consumed by the rename: a create (and a
    ///     subsequent same-id update) round-trips and leaves NO `.tmp` file behind.
    #[test]
    fn atomic_write_uses_unique_temp_and_leaves_none_behind() {
        let s = Scratch::new("atomictmp");
        create_agent(s.state_root(), &sample("atomic")).unwrap();
        assert_eq!(
            read_agent(s.state_root(), "atomic")
                .expect("must read back")
                .id,
            "atomic"
        );

        // A second write to the SAME id (update) must also round-trip via a fresh temp.
        update_agent(
            s.state_root(),
            &Agent {
                updated_at: 2_000,
                ..sample("atomic")
            },
        )
        .unwrap();
        assert_eq!(
            read_agent(s.state_root(), "atomic").unwrap().updated_at,
            2_000
        );

        let dir = agents_dir(s.state_root()).unwrap();
        let temps: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(
            temps.is_empty(),
            "atomic rename must leave no temp file behind"
        );
    }
}
