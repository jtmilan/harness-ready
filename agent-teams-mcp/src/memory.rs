//! Phase 10 / Mem-1 (10-03): the BridgeMemory note tools, wired into the sidecar
//! **inert-first** (a SEPARATE `memory_tool_router` merged in `new()` only under
//! `#[cfg(feature = "memory-notes")]`, mirroring the verified Phase-B pattern) and
//! **ungated** (pure file I/O over `core/memory`, the `runs.jsonl` posture — these
//! NEVER call `read_mcp_config().allow_mutations`; memory is OFF the PTY/Model-A
//! mutation axis, so the right controls are containment/limits/provenance, not the
//! `allow_mutations` gate — D57).
//!
//! ## Security (D57 — these tools become pane-reachable when the feature ships, 2b)
//! Ungated ≠ unprotected. This layer is the tool boundary for a NEW pane-write
//! attack surface, so it:
//! - **path-contains** caller ids — `get/update/delete_memory` reject any id failing
//!   `agent_teams_memory::valid_note_id` with a structured `invalid_params` error
//!   (defense-in-depth: `core/memory` ALSO guards, but the tool returns a clean
//!   rejection instead of a silent null that would mask the attempt);
//! - **stamps provenance** — `create/update_memory` read `$AGENT_TEAMS_PANE_ID` and
//!   pass it as the `writer` ARG (the tool layer reads env; `core/memory` stays
//!   HERMETIC — it never reads env, preserving 10-01's testability contract);
//! - inherits the byte/count write caps `core/memory` enforces (oversize → error).
//!
//! The whole module is compiled only under the `memory-notes` feature.

use std::path::{Path, PathBuf};

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_router, ErrorData};
use serde::{Deserialize, Serialize};

use agent_teams_memory::{
    backlinks, build_graph, create_note, delete_note, get_note, harvest_lessons, list_notes,
    memory_root, repo_dir, search_notes, suggest, update_note, valid_note_id,
    write_harvested_notes, MemoryGraph, Note, NotePatch, Suggestion, GLOBAL_SCOPE,
};

use crate::TeamServer;

// ── object-rooted result wrappers (MCP outputSchema requires a root object) ──
#[derive(Serialize, schemars::JsonSchema)]
struct NoteResult {
    /// The note, or null if absent/unknown.
    note: Option<Note>,
}
#[derive(Serialize, schemars::JsonSchema)]
struct MemoriesResult {
    memories: Vec<Note>,
}
#[derive(Serialize, schemars::JsonSchema)]
struct BacklinksResult {
    backlinks: Vec<Note>,
}
#[derive(Serialize, schemars::JsonSchema)]
struct SuggestionsResult {
    suggestions: Vec<Suggestion>,
}
#[derive(Serialize, schemars::JsonSchema)]
struct DeleteResult {
    deleted: bool,
}

// ── args ──
#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct CreateMemoryArgs {
    title: String,
    body: String,
    #[serde(default)]
    tags: Vec<String>,
    /// Outbound links, BY NOTE-ID.
    #[serde(default)]
    links: Vec<String>,
    /// Optional free-form category / cluster label.
    #[serde(default)]
    category: Option<String>,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SearchMemoriesArgs {
    query: String,
    limit: Option<u32>,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct FindBacklinksArgs {
    /// A note id; returns notes that link TO it.
    target: String,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SuggestConnectionsArgs {
    target: String,
    limit: Option<u32>,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct GetMemoryArgs {
    id: String,
}
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
struct ListMemoriesArgs {}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct UpdateMemoryArgs {
    id: String,
    title: Option<String>,
    body: Option<String>,
    tags: Option<Vec<String>>,
    links: Option<Vec<String>>,
    /// Optional free-form category / cluster label. `null`/omitted = unchanged.
    category: Option<String>,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct DeleteMemoryArgs {
    id: String,
}

fn default_limit(limit: Option<u32>) -> usize {
    limit.unwrap_or(20).min(500) as usize
}

/// The CALLER pane id for provenance, from `$AGENT_TEAMS_PANE_ID` (supervisor-set at
/// spawn, NOT agent-controlled). `None` when unset — `core/memory` then stores no
/// origin/last_writer. This is the ONLY place env is read; `core/memory` stays
/// hermetic (it takes the writer as an ARG).
fn writer_id() -> Option<String> {
    std::env::var("AGENT_TEAMS_PANE_ID")
        .ok()
        .filter(|s| !s.is_empty())
}

/// Map an invalid (traversal / non-minted) caller id to a structured rejection so a
/// pane gets a clear error instead of a silent null that would mask the attempt.
/// `core/memory` ALSO guards (the actual containment fix); this is the clean tool-layer
/// message (defense-in-depth).
fn reject_bad_id() -> ErrorData {
    ErrorData::invalid_params("invalid note id".to_string(), None)
}

/// Resolve the notes dir for a state dir (free-fn form shared by the tool methods
/// below and the phase-b post-run harvest hook, which holds no server):
/// `<state_root>/../<name>-memory/<scope-key>`; scope-key =
/// `$AGENT_TEAMS_MEMORY_REPO_KEY` (explicit override, highest priority) else
/// [`GLOBAL_SCOPE`] (`global`) — so with no override every workspace shares ONE
/// store instead of fragmenting per launch cwd. `None` when the state dir has no
/// parent.
pub(crate) fn notes_dir(state_dir: &Path) -> Option<PathBuf> {
    let root = memory_root(state_dir)?;
    let key =
        std::env::var("AGENT_TEAMS_MEMORY_REPO_KEY").unwrap_or_else(|_| GLOBAL_SCOPE.to_string());
    Some(repo_dir(&root, &key))
}

/// Post-run knowledge harvest for the sidecar-local fan-in (`team_synthesize
/// {pane_ids}`): gate `memory_harvest` (mcp-config.json, default OFF, read FRESH per
/// run) ⇒ OFF returns 0 and writes NOTHING. ON ⇒ deterministic `LESSON:` extraction
/// (`core/memory::harvest_lessons` — max 3/run, length-guarded) + store writes with
/// exact-title dedup (`write_harvested_notes`). New notes land LINKED (same-run
/// sibling mesh + goal-relevance lineage over the pre-existing store — `goal` empty
/// ⇒ mesh-only). Returns notes written. NEVER fails the synthesis: any
/// resolution/write error degrades to fewer/zero notes.
pub(crate) fn harvest_reports(
    state_dir: &Path,
    run_id: &str,
    reports: &[(String, String)],
    goal: &str,
) -> usize {
    if !agent_teams_core::read_mcp_config(state_dir).memory_harvest {
        return 0;
    }
    let Some(dir) = notes_dir(state_dir) else {
        return 0;
    };
    let candidates = harvest_lessons(reports);
    if candidates.is_empty() {
        return 0;
    }
    write_harvested_notes(&dir, &candidates, run_id, goal)
}

impl TeamServer {
    /// Resolve the per-repo notes dir: `<state_root>/../<name>-memory/<repo-key>`.
    /// See [`notes_dir`] (the shared free-fn form).
    fn memory_dir(&self) -> Result<PathBuf, ErrorData> {
        notes_dir(&self.state_dir).ok_or_else(|| {
            ErrorData::internal_error(
                "no parent for the state dir — cannot resolve the memory root".to_string(),
                None,
            )
        })
    }
}

#[tool_router(router = memory_tool_router, vis = "pub(crate)")]
impl TeamServer {
    #[tool(
        name = "create_memory",
        description = "Create a durable note (pain #3: context loss). Ungated local \
            file I/O — never touches a PTY/agent state. Server-mints the id. links are \
            BY NOTE-ID. Returns the stored note."
    )]
    async fn create_memory(
        &self,
        Parameters(a): Parameters<CreateMemoryArgs>,
    ) -> Result<Json<NoteResult>, ErrorData> {
        let dir = self.memory_dir()?;
        let note = create_note(
            &dir,
            a.title,
            a.body,
            a.tags,
            a.links,
            a.category,
            writer_id(),
        )
        .map_err(|e| ErrorData::invalid_params(format!("create_memory: {e}"), None))?;
        Ok(Json(NoteResult { note: Some(note) }))
    }

    #[tool(
        name = "search_memories",
        description = "Case-insensitive token search over note title+body+tags, ranked \
            by match count and capped by limit (default 20). Read-only."
    )]
    async fn search_memories(
        &self,
        Parameters(a): Parameters<SearchMemoriesArgs>,
    ) -> Result<Json<MemoriesResult>, ErrorData> {
        let dir = self.memory_dir()?;
        let all = list_notes(&dir);
        Ok(Json(MemoriesResult {
            memories: search_notes(&all, &a.query, default_limit(a.limit)),
        }))
    }

    #[tool(
        name = "find_backlinks",
        description = "Notes that link TO the given note id (DERIVED, never stored). \
            Read-only."
    )]
    async fn find_backlinks(
        &self,
        Parameters(a): Parameters<FindBacklinksArgs>,
    ) -> Result<Json<BacklinksResult>, ErrorData> {
        let dir = self.memory_dir()?;
        let all = list_notes(&dir);
        Ok(Json(BacklinksResult {
            backlinks: backlinks(&all, &a.target),
        }))
    }

    #[tool(
        name = "suggest_connections",
        description = "Zero-dep relatedness for a note id: shared tags + shared link \
            targets + title/body term overlap, ranked. No embeddings. Read-only."
    )]
    async fn suggest_connections(
        &self,
        Parameters(a): Parameters<SuggestConnectionsArgs>,
    ) -> Result<Json<SuggestionsResult>, ErrorData> {
        let dir = self.memory_dir()?;
        let all = list_notes(&dir);
        Ok(Json(SuggestionsResult {
            suggestions: suggest(&all, &a.target, default_limit(a.limit)),
        }))
    }

    #[tool(
        name = "get_memory",
        description = "Read one note by id, or null. Read-only."
    )]
    async fn get_memory(
        &self,
        Parameters(a): Parameters<GetMemoryArgs>,
    ) -> Result<Json<NoteResult>, ErrorData> {
        if !valid_note_id(&a.id) {
            return Err(reject_bad_id());
        }
        let dir = self.memory_dir()?;
        Ok(Json(NoteResult {
            note: get_note(&dir, &a.id),
        }))
    }

    #[tool(
        name = "list_memories",
        description = "All notes in this repo's store, sorted by created_at. Read-only."
    )]
    async fn list_memories(
        &self,
        Parameters(_a): Parameters<ListMemoriesArgs>,
    ) -> Result<Json<MemoriesResult>, ErrorData> {
        let dir = self.memory_dir()?;
        Ok(Json(MemoriesResult {
            memories: list_notes(&dir),
        }))
    }

    #[tool(
        name = "update_memory",
        description = "Patch a note (last-writer-wins atomic write); null fields are \
            unchanged. Returns the updated note, or null if the id is unknown."
    )]
    async fn update_memory(
        &self,
        Parameters(a): Parameters<UpdateMemoryArgs>,
    ) -> Result<Json<NoteResult>, ErrorData> {
        if !valid_note_id(&a.id) {
            return Err(reject_bad_id());
        }
        let dir = self.memory_dir()?;
        let patch = NotePatch {
            title: a.title,
            body: a.body,
            tags: a.tags,
            links: a.links,
            category: a.category,
        };
        let note = update_note(&dir, &a.id, patch, writer_id())
            .map_err(|e| ErrorData::invalid_params(format!("update_memory: {e}"), None))?;
        Ok(Json(NoteResult { note }))
    }

    #[tool(
        name = "delete_memory",
        description = "Atomically delete a note by id. Returns {deleted:true} if it \
            existed, false otherwise."
    )]
    async fn delete_memory(
        &self,
        Parameters(a): Parameters<DeleteMemoryArgs>,
    ) -> Result<Json<DeleteResult>, ErrorData> {
        if !valid_note_id(&a.id) {
            return Err(reject_bad_id());
        }
        let dir = self.memory_dir()?;
        let deleted = delete_note(&dir, &a.id)
            .map_err(|e| ErrorData::internal_error(format!("delete_memory: {e}"), None))?;
        Ok(Json(DeleteResult { deleted }))
    }

    #[tool(
        name = "get_memory_graph",
        description = "Phase 11 / Mem-2: the memory graph as {nodes, edges} — a pure \
            read-only PROJECTION over the notes (owns no durable file). Edges are \
            'link' (hard, author-made from a note's links) or 'suggested' (soft, \
            heuristic). Read-only; works whether or not the app is running."
    )]
    async fn get_memory_graph(
        &self,
        Parameters(_a): Parameters<ListMemoriesArgs>,
    ) -> Result<Json<MemoryGraph>, ErrorData> {
        let dir = self.memory_dir()?;
        Ok(Json(build_graph(&list_notes(&dir))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ════════════════ Sidecar tool-boundary hardening (writeside-memroles lane) ═══════
    //
    // This module had NO tests. `core/memory` is heavily covered (allowlist, caps, LWW,
    // concurrency, provenance) and is the SSOT for the security logic — we do NOT
    // re-test it here. What lives ONLY in this layer, and was unguarded, is:
    //   • `default_limit` — the read-fan-out DoS clamp (an agent-supplied `limit` is
    //     bounded to 500; an unbounded `take(limit)` would let one call drag the whole
    //     store) and its default-20;
    //   • `reject_bad_id` — the structured `invalid_params` rejection (the doc-comment's
    //     "clean error instead of a silent null that masks the attempt" promise);
    //   • the WIRING: that `get/update/delete_memory` actually gate on `valid_note_id`
    //     and surface that rejection as an Err BEFORE any path resolution — i.e. the
    //     tool layer's defense-in-depth is really wired, not just present in core.
    // Mirrors task.rs's discipline (test the in-module logic; don't fight async/env):
    // the bad-id rejection precedes `memory_dir()`, so those cases need NO env. The
    // happy-path tool tests touch NO env either — each gets a UNIQUE `state_dir`, so
    // `memory_root(state_dir)` → `<unique-root>/state-memory/<key>` is test-private
    // regardless of the cwd-derived repo key (two tests can't collide). We deliberately
    // do NOT call `std::env::set_var`: concurrent `setenv`/`getenv` across libtest's
    // parallel test threads is a process-environment data race (UB on the `environ`
    // block, not merely on the value), and pinning a constant does not make it safe.

    /// Extract the JSON-RPC error code from a tool result WITHOUT requiring the Ok type
    /// to be `Debug` (`rmcp::Json<T>` is not `Debug`, so `.unwrap_err()` won't compile).
    /// `Some(code)` on Err, `None` on Ok.
    fn err_code<T>(r: Result<T, ErrorData>) -> Option<i32> {
        r.err().map(|e| e.code.0)
    }

    /// A TeamServer over a fresh, test-private state_dir (nested so its parent — where
    /// the `<name>-memory` sibling lands — is also test-private).
    fn server_with_unique_state() -> (TeamServer, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "at-mcp-mem-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let state_dir = root.join("state");
        std::fs::create_dir_all(&state_dir).unwrap();
        (TeamServer::new(state_dir.clone()), root)
    }

    #[test]
    fn default_limit_clamps_and_defaults() {
        assert_eq!(default_limit(None), 20, "unset → default 20");
        assert_eq!(default_limit(Some(0)), 0, "explicit 0 is honored");
        assert_eq!(default_limit(Some(50)), 50, "in-range passes through");
        assert_eq!(default_limit(Some(500)), 500, "exactly the cap passes");
        assert_eq!(
            default_limit(Some(501)),
            500,
            "just past the cap is clamped"
        );
        assert_eq!(
            default_limit(Some(u32::MAX)),
            500,
            "an adversarial huge limit is clamped to 500 (read-fan-out DoS bound)"
        );
    }

    #[test]
    fn reject_bad_id_is_structured_invalid_params() {
        let e = reject_bad_id();
        // JSON-RPC invalid_params (-32602) — a clean, structured rejection (NOT a silent
        // null, NOT an internal_error that would leak as a 500-class fault).
        assert_eq!(e.code.0, -32602, "must be JSON-RPC invalid_params");
        assert!(!e.message.is_empty(), "rejection carries a message");
        assert!(
            e.message.to_lowercase().contains("id"),
            "message names the offending field: {:?}",
            e.message
        );
    }

    #[test]
    fn tool_layer_gates_on_the_same_allowlist_as_core() {
        // Defense-in-depth: the tool layer's id guard is the EXACT `valid_note_id`
        // predicate core enforces (re-exported + used in get/update/delete_memory).
        // A minted-shape id passes; traversal / absolute / non-minted are rejected —
        // so the tool can never resolve a path core would reject.
        assert!(valid_note_id("mem_123"));
        for bad in [
            "../sentinel",
            "/etc/passwd",
            "mem_../x",
            "mem_a",
            "",
            "mem_",
        ] {
            assert!(!valid_note_id(bad), "tool guard must reject {bad:?}");
        }
    }

    #[tokio::test]
    async fn get_update_delete_reject_bad_id_before_path_resolution() {
        // THE tool-boundary wiring proof: a malicious id is rejected with a structured
        // Err by every id-taking tool, and that rejection happens BEFORE `memory_dir()`
        // (so it is independent of env / state_dir). A regression that dropped the guard
        // would return Ok(null)/Ok(false) (a silent miss) — these assert the Err arm.
        let (srv, root) = server_with_unique_state();
        for bad in ["../sentinel", "/etc/passwd", "mem_../escape", "mem_abc", ""] {
            let g = srv
                .get_memory(Parameters(GetMemoryArgs { id: bad.into() }))
                .await;
            assert_eq!(err_code(g), Some(-32602), "get_memory({bad:?}) must Err");

            let u = srv
                .update_memory(Parameters(UpdateMemoryArgs {
                    id: bad.into(),
                    title: Some("x".into()),
                    body: None,
                    tags: None,
                    links: None,
                    category: None,
                }))
                .await;
            assert_eq!(err_code(u), Some(-32602), "update_memory({bad:?}) must Err");

            let d = srv
                .delete_memory(Parameters(DeleteMemoryArgs { id: bad.into() }))
                .await;
            assert_eq!(err_code(d), Some(-32602), "delete_memory({bad:?}) must Err");
        }
        // The rejections never created the memory partition (path was never resolved).
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn create_get_list_delete_round_trip_through_the_tools() {
        // Lock the async wiring end-to-end: the tools delegate to core/memory correctly
        // (server-minted id flows back, get/list see the create, delete removes it).
        // A unique state_dir makes the resolved partition test-private (no env touched).
        let (srv, root) = server_with_unique_state();

        let created = srv
            .create_memory(Parameters(CreateMemoryArgs {
                title: "Why per-note files".into(),
                body: "atomic rename + LWW".into(),
                tags: vec!["arch".into()],
                links: vec![],
                category: None,
            }))
            .await
            .expect("create_memory ok")
            .0
            .note
            .expect("a note is returned");
        assert!(
            valid_note_id(&created.id),
            "server-minted id is allowlist-valid"
        );

        // get_memory returns the same note via its (valid) id.
        let got = srv
            .get_memory(Parameters(GetMemoryArgs {
                id: created.id.clone(),
            }))
            .await
            .expect("get_memory ok")
            .0
            .note
            .expect("the created note is found");
        assert_eq!(got.id, created.id);
        assert_eq!(got.title, "Why per-note files");

        // list_memories includes it.
        let listed = srv
            .list_memories(Parameters(ListMemoriesArgs::default()))
            .await
            .expect("list_memories ok")
            .0
            .memories;
        assert!(
            listed.iter().any(|n| n.id == created.id),
            "create is listed"
        );

        // delete_memory removes it; a second delete reports {deleted:false}.
        let del = srv
            .delete_memory(Parameters(DeleteMemoryArgs {
                id: created.id.clone(),
            }))
            .await
            .expect("delete_memory ok")
            .0
            .deleted;
        assert!(del, "first delete reports deleted:true");
        let del2 = srv
            .delete_memory(Parameters(DeleteMemoryArgs {
                id: created.id.clone(),
            }))
            .await
            .expect("delete_memory ok")
            .0
            .deleted;
        assert!(!del2, "second delete reports deleted:false (idempotent)");
        assert!(
            srv.get_memory(Parameters(GetMemoryArgs { id: created.id }))
                .await
                .unwrap()
                .0
                .note
                .is_none(),
            "the note is gone after delete"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn create_memory_oversize_body_is_invalid_params_no_partial() {
        // The tool maps a core cap breach to invalid_params (not a 500) and writes
        // nothing. (Core proves the cap; this proves the tool's error MAPPING + that the
        // store stays empty through the tool path.) Unique state_dir; no env touched.
        let (srv, root) = server_with_unique_state();
        let oversize = "x".repeat(agent_teams_memory::MAX_BODY_BYTES + 1);
        let res = srv
            .create_memory(Parameters(CreateMemoryArgs {
                title: "t".into(),
                body: oversize,
                tags: vec![],
                links: vec![],
                category: None,
            }))
            .await;
        assert_eq!(
            err_code(res),
            Some(-32602),
            "an oversize body must be rejected by the tool as invalid_params"
        );

        let listed = srv
            .list_memories(Parameters(ListMemoriesArgs::default()))
            .await
            .unwrap()
            .0
            .memories;
        assert!(listed.is_empty(), "no partial/oversize note was written");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn notes_dir_falls_back_to_global_scope() {
        // With no `AGENT_TEAMS_MEMORY_REPO_KEY`, the sidecar resolves its notes dir to
        // `<memory_root>/global` (GLOBAL_SCOPE) so a pane shares the app's ONE store
        // rather than fragmenting per launch cwd. Unique state_dir keeps memory_root
        // test-private; we READ (never SET) ambient env, per this module's discipline —
        // so the assertion tracks the resolver's own decision and stays correct even if
        // a dev/CI shell has exported an override.
        let (_srv, root) = server_with_unique_state();
        let state_dir = root.join("state");
        let dir = notes_dir(&state_dir).expect("state_dir has a parent → Some(dir)");

        // env set → env value; else → GLOBAL_SCOPE ("global").
        let expected_key = std::env::var("AGENT_TEAMS_MEMORY_REPO_KEY")
            .unwrap_or_else(|_| GLOBAL_SCOPE.to_string());
        assert_eq!(
            dir,
            memory_root(&state_dir).unwrap().join(&expected_key),
            "notes_dir = <memory_root>/<override-or-global>"
        );
        if std::env::var("AGENT_TEAMS_MEMORY_REPO_KEY").is_err() {
            assert!(
                dir.ends_with("global"),
                "no-env fallback must resolve to /global: {dir:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
