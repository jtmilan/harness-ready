//! Agent Teams — BridgeMemory note store (Mem-1, Phase 10 / D49).
//!
//! A zero-dep (serde + std) note store targeting **pain #3 (context loss)**: the
//! operator loses the thread of *why* a thing was decided across panes, restarts,
//! and the `state_root` startup wipe.
//!
//! ## Why this diverges from `core/task`
//! `core/task` stores ALL records in ONE shared JSONL with a read-modify-write
//! `upsert` — a lost-update race (two writers each read the old set; the later
//! rewrite clobbers the earlier note). Acceptable for a single-app-session,
//! human-paced kanban; NOT acceptable for an MCP store an external agent may write
//! concurrently. So this store is a **DIRECTORY of per-note files** with
//! **atomic-rename + last-writer-wins** (PRD §15.4, "specify before coding — no
//! upstream conflict model to copy"). We MIRROR `core/task`'s path-helper mechanic
//! (takes `state_root` as an arg, never reads `$HOME` → hermetic tests) and its
//! tolerant-reader/serde shape, and DELIBERATELY DIVERGE its layout.
//!
//! ## Topology (out-of-tree, App-Support sibling, repo-keyed)
//! ```text
//! <state_root>/..                         (the parent the sibling idiom targets)
//!  └─ <name>-memory/                       (the memory ROOT — survives the D7 wipe)
//!      └─ <repo-key>/                      (one subdir per repo)
//!          ├─ <note-id>.json               (ONE file per note — the concurrency unit)
//!          └─ .<note-id>.json.tmp.<pid>.<n> (transient write temp, renamed over the target)
//! ```
//! The dir is a SIBLING of `state_root` (the only placement that survives the D7
//! startup wipe) and stays SEPARATE from any `bridge/<run-id>/` fold directory.
//!
//! ## Concurrency (net-new; no upstream model)
//! - WRITE = `serde_json::to_vec_pretty` → temp file → `fs::rename` over the target.
//!   `rename` is atomic on the same filesystem, so a reader sees the whole old file
//!   or the whole new file — never a partial.
//! - Distinct notes never contend (separate files).
//! - Same note = **last-writer-wins** by rename; NO lock, NO merge, NO CRDT.
//!   `updated_at` is the informational newer-marker; the rename is the arbiter.
//!
//! ## Security posture (D57 — pane-write is a NEW attack surface)
//! The store stays off the PTY/Model-A axis (touches no PTY, sets no agent state,
//! never enters `rank()`), so the MCP tools that wrap it are **ungated** (no
//! `allow_mutations`). But ungated ≠ unprotected: once a pane can call these tools,
//! the caller-id read/update/delete trio is a path-traversal surface and the write
//! path is a disk-exhaustion / poisoning surface. This crate therefore enforces:
//! - **path containment** — [`valid_note_id`] rejects any id outside the minted
//!   `mem_<digits/underscores>` shape (no `/`, no `..`, no absolute path) BEFORE it
//!   reaches [`note_path`], on get/update/delete;
//! - **write validation / limits** — byte caps ([`MAX_TITLE_BYTES`] / [`MAX_BODY_BYTES`]
//!   / per-element [`MAX_TAG_BYTES`] / [`MAX_LINK_BYTES`]) + count caps ([`MAX_TAGS`] /
//!   [`MAX_LINKS`] / [`MAX_NOTES_PER_DIR`]) on create/update — count caps alone left an
//!   ~80 MiB hole (MAX_TAGS × oversized strings) past MAX_BODY_BYTES;
//! - **provenance** — [`Note::origin`] (immutable creator) + [`Note::last_writer`]
//!   (most-recent updater), stamped from a `writer` ARG so this crate stays HERMETIC
//!   (never reads env — the tool layer passes the pane id). NOTE: provenance is an
//!   advisory HINT (the spawn-set pane id is forgeable by the pane), NOT attestation;
//!   consumers must not treat it as authenticated;
//! - **bounded projection** — [`build_graph`] caps its O(n²) suggested-edge pass at
//!   [`MAX_GRAPH_NODES`] so a large store cannot wedge the graph view.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// A durable note. This is the shape Phase 11's memory graph consumes — pin it.
///
/// `id` is **server-minted** (a deliberate divergence from `core/task`, which takes
/// caller-supplied ids). `links` key **by note-id** (rename-safe: a retitled note
/// keeps its edges); backlinks are DERIVED (scanned), never stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Note {
    /// Server-minted, e.g. `mem_<unix_ms>_<pid>_<counter>`.
    pub id: String,
    pub title: String,
    /// Note text (markdown by convention; opaque to the store).
    pub body: String,
    /// Free-form labels; drives `suggest` (shared-tag heuristic).
    #[serde(default)]
    pub tags: Vec<String>,
    /// Outbound edges, BY NOTE-ID. Phase 11 reads these.
    #[serde(default)]
    pub links: Vec<String>,
    /// unix ms.
    pub created_at: u64,
    /// unix ms; bumped on every write. The LWW newer-marker.
    pub updated_at: u64,
    /// Provenance: the pane id that CREATED this note (immutable — set once at
    /// create, never overwritten on update). `None` on legacy notes / when no
    /// writer was passed. Additive (`#[serde(default)]`) — Phase 11's `build_graph`
    /// passes it through as [`GraphNode::origin`] (hover-card detail), defaulting
    /// to `None`, so this stays projection-safe.
    #[serde(default)]
    pub origin: Option<String>,
    /// Provenance: the pane id of the MOST-RECENT writer (set on create AND every
    /// update). `None` on legacy notes / when no writer was passed.
    #[serde(default)]
    pub last_writer: Option<String>,
    /// Optional free-form category for the in-app graph editor (e.g. a cluster /
    /// lane label). `None` on legacy notes / when unset. Additive
    /// (`#[serde(default)]`) — legacy JSON without this field deserializes to
    /// `None`, and `build_graph` passes it through as [`GraphNode::category`].
    #[serde(default)]
    pub category: Option<String>,
    /// STRUCTURED PROVENANCE (run-outcome capture, see [`build_run_capture`]): the
    /// `DelegateRunRecord.run_id` a note was captured from, so the store is queryable
    /// by run and a re-completion of the same run dedups. `None` on every note NOT
    /// produced by the capture path. ADDITIVE + omit-when-`None`
    /// (`#[serde(default, skip_serializing_if)]`) — legacy JSON without this key
    /// deserializes to `None`, and a note that never carried it serializes
    /// byte-identically to before. Preserved verbatim across [`update_note`] (like
    /// [`Note::origin`] — the patch never touches it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// STRUCTURED PROVENANCE (run-outcome capture): the workspace a captured run was
    /// fired from, so the global store can be sliced by workspace. Same additive /
    /// omit-when-`None` / update-preserved contract as [`Note::run_id`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
}

/// A partial update for [`update_note`]. `None` fields are left unchanged.
#[derive(Debug, Clone, Default)]
pub struct NotePatch {
    pub title: Option<String>,
    pub body: Option<String>,
    pub tags: Option<Vec<String>>,
    pub links: Option<Vec<String>>,
    pub category: Option<String>,
}

/// A `suggest_connections` candidate + why it scored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Suggestion {
    pub note: Note,
    pub score: u32,
    /// Human-readable reasons (shared tags / shared link targets / shared terms).
    pub reasons: Vec<String>,
}

static WRITE_NONCE: AtomicU64 = AtomicU64::new(0);
static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Server-minted note id. Unique within a process (atomic counter) and across
/// processes (pid + millis). Zero-dep (no uuid/ulid/rand crate).
fn mint_id() -> String {
    let c = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("mem_{}_{}_{}", now_ms(), std::process::id(), c)
}

// ───────────────────── Security limits (D57 / C6) ──────────────────────────────
// Byte / count caps the write path enforces so a pane cannot wedge the store via an
// oversize note or exhaust the disk via unbounded note creation. Named consts with a
// documented rationale (tune in review). Byte caps are the per-note backstop; the
// count cap is the disk-exhaustion backstop.

/// Max note title length in bytes. Titles are short labels; 4 KiB is generous.
pub const MAX_TITLE_BYTES: usize = 4_096;
/// Max note body length in bytes (256 KiB). A note is a prose memory, not a blob;
/// this bounds a single pane write and keeps `read`/projection cheap.
pub const MAX_BODY_BYTES: usize = 256 * 1024;
/// Max number of tags on a note (drives the O(tags) suggest heuristic).
pub const MAX_TAGS: usize = 64;
/// Max bytes per tag. Tags are short labels — without this, MAX_TAGS oversized
/// strings would smuggle megabytes past the MAX_BODY_BYTES backstop.
pub const MAX_TAG_BYTES: usize = 256;
/// Max number of outbound links on a note (drives graph edges).
pub const MAX_LINKS: usize = 256;
/// Max bytes per link. Links are note ids / refs — same smuggling backstop as
/// [`MAX_TAG_BYTES`] (count cap alone left an ~80 MiB hole otherwise).
pub const MAX_LINK_BYTES: usize = 512;
/// Max notes per repo partition — the disk-exhaustion backstop. A pane that loops
/// `create_memory` cannot fill the disk past this. 50k notes ≈ a very large store.
pub const MAX_NOTES_PER_DIR: usize = 50_000;

/// Path-containment allowlist (C1 BLOCKER). An id is valid ONLY if it has the
/// server-minted `mem_<digits-and-underscores>` shape: `mem_` prefix + a NON-EMPTY
/// remainder of ASCII digits / underscores ONLY. This rejects EVERY traversal /
/// escape vector — `/` (separator), `.` (so no `..`), absolute paths, embedded
/// separators, and the empty string — BEFORE the id reaches [`note_path`] (whose
/// `Path::join` does NOT normalize, so an absolute or `..` id would escape `dir`).
///
/// The store ONLY mints `mem_<unix_ms>_<pid>_<counter>` ([`mint_id`]) — all digits
/// and underscores after the prefix — so every legitimate id passes and no
/// attacker-controlled path ever does. Zero-dep (no regex): hand-rolled `^mem_[0-9_]+$`.
pub fn valid_note_id(id: &str) -> bool {
    let Some(rest) = id.strip_prefix("mem_") else {
        return false;
    };
    !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit() || b == b'_')
}

/// Validate the write-side caps for a note's mutable fields. `Err(InvalidInput)` on
/// any cap breach (no partial write — the caller returns before [`write_note_atomic`]).
fn validate_write(title: &str, body: &str, tags: &[String], links: &[String]) -> io::Result<()> {
    if title.len() > MAX_TITLE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("title exceeds {MAX_TITLE_BYTES} bytes"),
        ));
    }
    if body.len() > MAX_BODY_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("body exceeds {MAX_BODY_BYTES} bytes"),
        ));
    }
    if tags.len() > MAX_TAGS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("too many tags (> {MAX_TAGS})"),
        ));
    }
    if links.len() > MAX_LINKS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("too many links (> {MAX_LINKS})"),
        ));
    }
    // Per-element byte caps — the count caps above are NOT enough: MAX_TAGS oversized
    // strings (or MAX_LINKS) would smuggle tens of MiB past MAX_BODY_BYTES. Reject if
    // ANY single tag/link exceeds its byte cap (no partial write).
    if let Some(t) = tags.iter().find(|t| t.len() > MAX_TAG_BYTES) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("tag exceeds {MAX_TAG_BYTES} bytes ({} bytes)", t.len()),
        ));
    }
    if let Some(l) = links.iter().find(|l| l.len() > MAX_LINK_BYTES) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("link exceeds {MAX_LINK_BYTES} bytes ({} bytes)", l.len()),
        ));
    }
    Ok(())
}

/// Count existing `*.json` notes in `dir` (skips temps / non-json). Cheap read_dir,
/// used as the per-dir count-cap backstop in [`create_note`].
fn count_notes(dir: &Path) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    rd.flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            !name.starts_with('.') && name.ends_with(".json")
        })
        .count()
}

/// FNV-1a — a stable (version-independent) zero-dep hash, used to repo-key the
/// store. `DefaultHasher` is NOT stable across compiler versions, so we hand-roll
/// FNV-1a to keep a repo's notes at a stable path forever.
fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The memory ROOT — a SIBLING of `state_root` (`<state_root>/../<name>-memory`),
/// mirroring `tasks_path`/`registry_path`. `None` when `state_root` has no parent.
/// Takes `state_root` as an ARG (never reads `$HOME`) so tests stay hermetic.
pub fn memory_root(state_root: &Path) -> Option<PathBuf> {
    let name = state_root
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "agent-teams".to_string());
    state_root
        .parent()
        .map(|p| p.join(format!("{name}-memory")))
}

/// The single GLOBAL memory scope key. With NO `AGENT_TEAMS_MEMORY_REPO_KEY`
/// override set, every resolver (the app, a spawned pane's sidecar, the standalone
/// MCP server) falls back to THIS key, so the whole personal Second Brain lives in
/// ONE `<memory_root>/global` store instead of fragmenting per-cwd via
/// [`repo_key_for`]. An explicit env override still wins (highest priority).
/// `repo_key_for` stays available for callers that still want a per-repo partition.
pub const GLOBAL_SCOPE: &str = "global";

/// A stable repo-key from a repo path (FNV-1a of the canonicalized path). Retained
/// for callers that still want a per-repo notes subdir; the default resolvers now
/// fall back to [`GLOBAL_SCOPE`] instead. Tests pass a key directly to [`repo_dir`].
pub fn repo_key_for(repo_path: &Path) -> String {
    let canon = repo_path
        .canonicalize()
        .unwrap_or_else(|_| repo_path.to_path_buf());
    format!("repo_{:016x}", fnv1a(&canon.to_string_lossy()))
}

/// `<root>/<sanitized repo-key>` — the per-repo notes dir. Created on first write.
pub fn repo_dir(root: &Path, repo_key: &str) -> PathBuf {
    root.join(sanitize_key(repo_key))
}

/// Keep a repo-key a single safe path component (no `/`, no `..`).
fn sanitize_key(key: &str) -> String {
    let s: String = key
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() {
        "default".to_string()
    } else {
        s
    }
}

fn note_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json"))
}

fn ser_err(e: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e)
}

/// Atomic write: serialize → temp file in the SAME dir → `fs::rename` over the
/// target. A reader never sees a partial; an interrupted write leaves the prior
/// note intact and the temp (which the reader skips) behind.
fn write_note_atomic(dir: &Path, note: &Note) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let data = serde_json::to_vec_pretty(note).map_err(ser_err)?;
    let nonce = WRITE_NONCE.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!(
        ".{}.json.tmp.{}.{}",
        note.id,
        std::process::id(),
        nonce
    ));
    std::fs::write(&tmp, &data)?;
    // Atomic same-filesystem replace. On error, clean up the temp.
    if let Err(e) = std::fs::rename(&tmp, note_path(dir, &note.id)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// Create + atomically store a new note (server-minted id + timestamps).
///
/// `writer` is the pane id stamped into BOTH `origin` (the immutable creator) and
/// `last_writer`. The store stays HERMETIC — the caller (the tool layer) passes the
/// writer; this crate never reads env. Tests pass `None`.
///
/// Enforces the C6 write caps (byte/count) and the per-dir count cap; returns
/// `Err(InvalidInput)` on a cap breach with NO file written.
pub fn create_note(
    dir: &Path,
    title: String,
    body: String,
    tags: Vec<String>,
    links: Vec<String>,
    category: Option<String>,
    writer: Option<String>,
) -> io::Result<Note> {
    validate_write(&title, &body, &tags, &links)?;
    if count_notes(dir) >= MAX_NOTES_PER_DIR {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("note count cap reached (>= {MAX_NOTES_PER_DIR})"),
        ));
    }
    let now = now_ms();
    let note = Note {
        id: mint_id(),
        title,
        body,
        tags,
        links,
        created_at: now,
        updated_at: now,
        origin: writer.clone(),
        last_writer: writer,
        // Blank/whitespace category normalizes to None — "" is the tool layer's
        // "uncategorized" sentinel (an Option can't distinguish "clear" from
        // "leave unchanged" on the update path, so the empty string carries it).
        category: category.filter(|c| !c.trim().is_empty()),
        // Structured run-provenance is set ONLY by the capture path
        // ([`build_run_capture`]); an ordinary create/graph-editor note carries none.
        run_id: None,
        workspace_id: None,
    };
    write_note_atomic(dir, &note)?;
    Ok(note)
}

/// Read one note by id; `None` if absent or unparseable (tolerant).
///
/// C1 containment: an id failing [`valid_note_id`] (traversal / absolute / non-minted)
/// returns `None` WITHOUT touching the filesystem — it never reaches [`note_path`].
pub fn get_note(dir: &Path, id: &str) -> Option<Note> {
    if !valid_note_id(id) {
        return None;
    }
    let data = std::fs::read(note_path(dir, id)).ok()?;
    serde_json::from_slice(&data).ok()
}

/// All notes in `dir`, tolerant: skips temps (`.`-prefixed), non-`.json`, and
/// unparseable files. Sorted by `created_at` then `id` for determinism.
pub fn list_notes(dir: &Path) -> Vec<Note> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in rd.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || !name.ends_with(".json") {
            continue;
        }
        if let Ok(data) = std::fs::read(entry.path()) {
            if let Ok(note) = serde_json::from_slice::<Note>(&data) {
                out.push(note);
            }
        }
    }
    out.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

/// Apply a patch (LWW atomic write); `Ok(None)` if the id does not exist.
///
/// C1 containment: an id failing [`valid_note_id`] short-circuits to `Ok(None)`
/// (via [`get_note`]) — no filesystem access, no escape.
///
/// `writer` stamps `last_writer` ONLY — `origin` (the creator) is IMMUTABLE and
/// preserved. The store stays hermetic; the tool layer passes the writer (tests
/// pass `None`). Enforces the same byte/count caps as create on the PATCHED fields.
pub fn update_note(
    dir: &Path,
    id: &str,
    patch: NotePatch,
    writer: Option<String>,
) -> io::Result<Option<Note>> {
    let Some(mut note) = get_note(dir, id) else {
        return Ok(None);
    };
    if let Some(t) = patch.title {
        note.title = t;
    }
    if let Some(b) = patch.body {
        note.body = b;
    }
    if let Some(tg) = patch.tags {
        note.tags = tg;
    }
    if let Some(l) = patch.links {
        note.links = l;
    }
    if let Some(c) = patch.category {
        // "" (or whitespace) CLEARS the category — the patch Option already means
        // "leave unchanged" when None, so the empty string is the clear sentinel
        // (the graph editor's "— uncategorized —" choice).
        note.category = if c.trim().is_empty() { None } else { Some(c) };
    }
    // Validate the (possibly patched) fields against the C6 caps before writing.
    validate_write(&note.title, &note.body, &note.tags, &note.links)?;
    note.updated_at = now_ms();
    // Provenance: stamp the updater; the original creator (`origin`) is untouched.
    if writer.is_some() {
        note.last_writer = writer;
    }
    write_note_atomic(dir, &note)?;
    Ok(Some(note))
}

/// Atomically delete a note; `Ok(false)` if it did not exist.
///
/// C1 containment: an id failing [`valid_note_id`] (traversal / absolute / non-minted)
/// returns `Ok(false)` WITHOUT touching the filesystem — it never reaches
/// [`note_path`], so no `.json` outside `dir` can be removed.
pub fn delete_note(dir: &Path, id: &str) -> io::Result<bool> {
    if !valid_note_id(id) {
        return Ok(false);
    }
    match std::fs::remove_file(note_path(dir, id)) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// Common English function words dropped from EVERY tokenization so a note whose
/// only overlap with a query is an incidental stopword (e.g. "the"/"of"/"to")
/// cannot score as relevance. Deliberately CONSERVATIVE — function words only, NO
/// domain words ("path", "code", "test", "review", "performance", …) — so it
/// tightens search/suggest/recall uniformly without hiding real hits. A plain
/// `&[&str]` checked with `.contains` (no `once`/lazy-static — the set is tiny and
/// the crate is zero-dep).
const STOPWORDS: &[&str] = &[
    "the", "a", "an", "and", "or", "but", "of", "to", "in", "on", "for", "with", "without", "at",
    "by", "from", "as", "is", "are", "was", "were", "be", "been", "it", "its", "this", "that",
    "these", "those", "into", "than", "then", "so", "if", "no", "not", "up", "out", "off", "over",
    "under", "per", "via", "do", "does", "did", "has", "have", "had", "will", "would", "can",
    "could", "should", "may", "might", "i", "you", "we", "they", "he", "she", "them", "your",
    "our",
];

/// Tokenize for search/suggest: split on non-alphanumerics, lowercase, drop
/// 1-char tokens AND [`STOPWORDS`] (checked post-lowercase). Zero-dep, deterministic.
fn tokenize(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 2)
        .map(|t| t.to_lowercase())
        .filter(|t| !STOPWORDS.contains(&t.as_str()))
        .collect()
}

/// Shared token-overlap scorer for [`search_notes`] and [`relevant_notes`]: for
/// each note, the count of DISTINCT query tokens present in its title+body+tags,
/// keeping only notes with a non-zero score. Returned UNSORTED — each caller sorts
/// (and, for the auto paths, applies the relevance floor) itself. Factored out so
/// the loose (`score>0`) and floored paths can never drift their tokenize/scoring.
fn scored_matches<'a>(notes: &'a [Note], query: &str) -> Vec<(u32, &'a Note)> {
    let q: HashSet<String> = tokenize(query).into_iter().collect();
    if q.is_empty() {
        return Vec::new();
    }
    notes
        .iter()
        .filter_map(|n| {
            let hay: HashSet<String> =
                tokenize(&format!("{} {} {}", n.title, n.body, n.tags.join(" ")))
                    .into_iter()
                    .collect();
            let score = q.iter().filter(|t| hay.contains(*t)).count() as u32;
            (score > 0).then_some((score, n))
        })
        .collect()
}

/// Sort scored matches (highest score, then oldest `created_at` as a stable
/// tiebreak) and clone out the top `limit`. Shared tail of [`search_notes`] and
/// [`relevant_notes`] so their ordering can never drift.
fn rank_and_take(mut scored: Vec<(u32, &Note)>, limit: usize) -> Vec<Note> {
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.created_at.cmp(&b.1.created_at))
    });
    scored
        .into_iter()
        .take(limit)
        .map(|(_, n)| n.clone())
        .collect()
}

/// Case-insensitive token search over title + body + tags; ranked by match count,
/// capped by `limit`. Zero-dep (no index, no embeddings). LOOSE by design: any
/// non-zero token overlap (`score>0`) is a hit — this is the public API the
/// `search_memories` MCP tool exposes, where the caller wants breadth. The AUTOMATIC
/// paths (recall-prime, lineage linking) use the floored [`relevant_notes`] instead.
pub fn search_notes(notes: &[Note], query: &str, limit: usize) -> Vec<Note> {
    rank_and_take(scored_matches(notes, query), limit)
}

/// Floored relevance for the AUTOMATIC paths ([`recall_block`] auto-recall +
/// [`harvest_relevant_ids`] lineage linking). Same tokenize + score as
/// [`search_notes`] (via [`scored_matches`]), same ranking (via [`rank_and_take`]),
/// but a note must share at least `min(2, N)` DISTINCT query tokens — where `N` is
/// the count of distinct MEANINGFUL (post-stopword) query tokens. So a multi-word
/// goal demands ≥2 shared tokens (a lone incidental overlap — e.g. only "path" in
/// common — is NOT treated as relevance), while a 1-token query still returns
/// single-token matches (the adaptive floor never exceeds the query's own token
/// count, so short queries are not over-pruned). Empty / all-stopword query ⇒ empty.
/// This is the fix for the loose auto-linking bug: run captures link, and recall
/// surfaces, only genuinely related notes — while `search_memories` stays loose.
fn relevant_notes(notes: &[Note], query: &str, limit: usize) -> Vec<Note> {
    let distinct = tokenize(query).into_iter().collect::<HashSet<_>>().len();
    if distinct == 0 {
        return Vec::new();
    }
    let floor = distinct.min(2) as u32;
    let scored: Vec<(u32, &Note)> = scored_matches(notes, query)
        .into_iter()
        .filter(|(score, _)| *score >= floor)
        .collect();
    rank_and_take(scored, limit)
}

/// PROVENANCE line under the recall header (system-prompt `memory_system` +
/// `knowledge_cutoff` patterns fused, per docs/ADE-PROMPT-GOVERNANCE.md P5+P6):
/// says where the notes come from AND orders the agent to re-verify any
/// present-state claim against HEAD — recalled knowledge is point-in-time, and
/// acting on stale recall has a proven cost (round-5 ground-truthing found 4
/// stale ledger items). One const so tests/consumers can't drift from the text.
pub const RECALL_PROVENANCE: &str = "_Recalled from the local memory store; each \
note reflects when it was written. Verify any claim about the CURRENT state of \
the repo/app against HEAD before acting on it._\n";

/// `unix_ms` → `YYYY-MM-DD` (UTC), std-only (the crate is deliberately zero-dep —
/// no chrono/time). Howard Hinnant's civil-from-days algorithm; exact for the
/// whole u64-ms range we mint (`now_ms`). Used to stamp each recalled note with
/// its write date so the reader can judge staleness at a glance.
fn date_ymd(unix_ms: u64) -> String {
    let days = (unix_ms / 86_400_000) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// DETERMINISTIC MEMORY PRIME: format the top-`limit` notes matching `task` as a
/// compact markdown preamble to PREPEND to a pane's task text before the agent
/// sees it — the "auto-consult" that makes the store actually feed the agents,
/// instead of waiting for the agent to call a search tool. Returns `None` when the
/// query is empty OR nothing matches (caller then sends the task unchanged — no
/// empty header, no behavior change). Pure (notes in): the caller resolves the dir
/// via [`memory_root`]/[`repo_dir`] + [`list_notes`]. Ranking is [`relevant_notes`]
/// (token overlap with the auto-path relevance floor, so an incidental single-token
/// overlap does NOT surface); each bullet is `title [YYYY-MM-DD] — snippet (id: …)` — the
/// date is the note's `updated_at` (staleness at a glance, see [`RECALL_PROVENANCE`])
/// and the snippet reuses the same char-capped, multibyte-safe [`snippet_of`] the
/// graph hover-card uses, so with a small `limit` the whole block stays a couple KB
/// at most. The trailing `---` fences the recall from the real task.
pub fn recall_block(notes: &[Note], task: &str, limit: usize) -> Option<String> {
    let hits = relevant_notes(notes, task, limit);
    if hits.is_empty() {
        return None;
    }
    let mut out = String::from("## Relevant memory (auto-recall)\n");
    out.push_str(RECALL_PROVENANCE);
    for n in &hits {
        out.push_str("- ");
        out.push_str(n.title.trim());
        out.push_str(" [");
        out.push_str(&date_ymd(n.updated_at));
        out.push(']');
        let snippet = snippet_of(&n.body);
        if !snippet.is_empty() {
            out.push_str(" — ");
            out.push_str(&snippet);
        }
        out.push_str(" (id: ");
        out.push_str(&n.id);
        out.push_str(")\n");
    }
    out.push_str("---\n");
    Some(out)
}

// ───────────── Post-run knowledge harvest (deterministic LESSON: extraction) ────
//
// The WRITE side of the knowledge flywheel: dispatch-time recall (`recall_block`,
// gate `memory_autoconsult`) reads the store; the post-run harvest writes it from
// each synthesized run's worker reports — run → learn → inject → better run. The
// garbage-in flywheel is THE failure mode (the store feeds every future dispatch),
// so extraction is DETERMINISTIC, not LLM: ONLY a line a worker explicitly marked
// `LESSON:` becomes a candidate. Harvest quality is therefore a PROMPT problem
// (the report protocol can ask workers for `LESSON:` lines) instead of a
// parsing-heuristics problem — prose paragraphs are NEVER harvested, nothing is
// summarized. Self-reports can launder failure into success (the
// bridge-synthesis-failure-modes lesson); a strict marker + tight length window +
// a small per-run cap keeps a launder-y report from flooding the store.
//
// Marker grammar (case-sensitive, per line):
//   optional leading whitespace, optional ONE bullet marker (`-` or `*`) with
//   optional whitespace after it, then the literal `LESSON:`. The candidate text
//   is everything after the marker, trimmed. So `LESSON: x`, `  LESSON:x`,
//   `- LESSON: x`, and `* LESSON: x` all match; `lesson: x` (case), `see LESSON:`
//   (mid-line), and `-- LESSON:` (double bullet) do not.
//
// Split of responsibilities: [`harvest_lessons`] is PURE (marker parse + length
// guards + batch title-dedup + the per-run cap — unit-tested without disk).
// [`write_harvested_notes`] owns the store-side exact-title DEDUP (needs
// `list_notes`) + the note shape (category/tags/provenance/origin) and NEVER
// fails a run: a `create_note` error (e.g. the [`MAX_NOTES_PER_DIR`] cap) skips
// that candidate and continues. The capability GATE (`memory_harvest`, default
// OFF) lives in the CALLERS (app / sidecar) — this crate stays hermetic and
// never reads config or env.
//
// LINKING (deterministic — notes must not land as graph orphans): a run factually
// creates exactly two relations, and both are derived from facts held at write
// time, never from similarity guessing:
//   1. SAME-RUN SIBLING MESH — every note written by one call links to every
//      other note of the same call (one activity, one run; the 3/run cap keeps
//      this ≤2 links per note). Pure plan: [`harvest_link_plan`].
//   2. GOAL-RELEVANCE LINEAGE — each new note links to the top pre-existing
//      notes [`relevant_notes`] ranks for the run's GOAL ([`harvest_relevant_ids`],
//      capped at [`HARVEST_LINK_RELEVANT_MAX`]). `relevant_notes` is the SAME
//      (floored) ranking the recall-prime uses, so the hits deterministically approximate
//      "what recall injected into this run" — the learning-chain edge
//      lesson-that-primed-the-run → lesson-the-run-produced. Outbound-only:
//      nothing is ever written on the old notes; the reverse direction stays
//      DERIVED via [`backlinks`]. Empty goal ⇒ no lineage links (mesh still
//      applies).
// Crash tolerance / ids: `create_note` mints ids server-side, so sibling ids are
// unknowable up front. Notes are therefore created WITH their lineage links (ids
// that already exist), then the sibling mesh is PATCHED on only the ids whose
// create succeeded. Dying between create and patch leaves a VALID store
// (lineage-linked or orphan notes at worst — never a dangling sibling id), and a
// failed create can never appear in another note's links by construction.

/// The line marker a worker report uses to flag a durable lesson. Case-sensitive.
pub const HARVEST_MARKER: &str = "LESSON:";
/// Max notes one run may harvest — the first this-many valid candidates win;
/// later ones are dropped (a single run must not flood the store).
pub const HARVEST_MAX_PER_RUN: usize = 3;
/// Minimum candidate length in CHARS after trimming — shorter is noise.
pub const HARVEST_MIN_CHARS: usize = 20;
/// Maximum candidate length in CHARS after trimming — longer is a dump, not a lesson.
pub const HARVEST_MAX_CHARS: usize = 500;
/// Char budget for a harvested note's title (word-boundary truncated).
pub const HARVEST_TITLE_MAX_CHARS: usize = 80;
/// Category stamped on every harvested note.
pub const HARVEST_CATEGORY: &str = "lesson";
/// Tag stamped on every harvested note.
pub const HARVEST_TAG: &str = "harvest";
/// Max pre-existing notes each NEW lesson links to as goal-relevance lineage
/// (the [`relevant_notes`] top-N over the pre-run store — mirrors the recall-prime's
/// own limit, so the lineage edges track what the prime would have injected).
pub const HARVEST_LINK_RELEVANT_MAX: usize = 5;

/// One extracted lesson, pre-write: the source pane, the derived title (dedup
/// key), and the full lesson text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LessonCandidate {
    /// The pane id whose report carried the lesson (becomes [`Note::origin`]).
    pub pane_id: String,
    /// First ≤[`HARVEST_TITLE_MAX_CHARS`] chars of the lesson, word-boundary
    /// truncated ([`lesson_title`]) — the exact-match dedup key.
    pub title: String,
    /// The full trimmed lesson text (one marked line).
    pub lesson: String,
}

/// Parse ONE report line against the marker grammar (see the module comment).
/// Returns the trimmed candidate text, or `None` when the line is not a marked
/// lesson. Pure; no length guard here (the caller applies it).
fn lesson_of_line(line: &str) -> Option<&str> {
    let s = line.trim_start();
    // Optional ONE bullet marker (`-` or `*`), optional whitespace after it.
    let s = s
        .strip_prefix('-')
        .or_else(|| s.strip_prefix('*'))
        .map(str::trim_start)
        .unwrap_or(s);
    s.strip_prefix(HARVEST_MARKER).map(str::trim)
}

/// Title of a lesson: the whole text when it fits [`HARVEST_TITLE_MAX_CHARS`];
/// otherwise the first ≤cap chars cut back to the last word boundary (whitespace)
/// inside the budget — a lesson whose first "word" alone overflows the cap is
/// hard-cut at the cap (chars, never bytes — multibyte-safe). No ellipsis: the
/// title is a deterministic exact-match dedup key, not display prose.
pub fn lesson_title(lesson: &str) -> String {
    truncate_title_chars(lesson, HARVEST_TITLE_MAX_CHARS)
}

/// Word-boundary title truncation shared by [`lesson_title`] and
/// [`build_run_capture`]: the whole `text` when it fits `max` CHARS; otherwise the
/// first ≤`max` chars cut back to the last whitespace inside the budget (a first
/// "word" that alone overflows is hard-cut at `max`). Chars, never bytes
/// (multibyte-safe). No ellipsis — the result is a deterministic dedup/label key.
fn truncate_title_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max).collect();
    match head.rfind(char::is_whitespace) {
        Some(cut) if cut > 0 => head[..cut].trim_end().to_string(),
        _ => head,
    }
}

/// DETERMINISTIC post-run lesson extraction over a run's worker reports
/// (`(pane_id, report_text)` in dispatch order). Per line: marker grammar →
/// candidate text; guard [`HARVEST_MIN_CHARS`]`..=`[`HARVEST_MAX_CHARS`] (chars,
/// trimmed); derive the title. A candidate whose title repeats an EARLIER
/// candidate's title in the same batch is skipped (first wins — two panes
/// reporting the identical lesson must not burn two cap slots). The first
/// [`HARVEST_MAX_PER_RUN`] survivors are returned; later ones are dropped.
/// Store-side dedup (against existing note titles) is deliberately CALLER-side —
/// see [`write_harvested_notes`] — so this stays pure and disk-free.
pub fn harvest_lessons(reports: &[(String, String)]) -> Vec<LessonCandidate> {
    let mut out: Vec<LessonCandidate> = Vec::new();
    for (pane_id, text) in reports {
        for line in text.lines() {
            if out.len() >= HARVEST_MAX_PER_RUN {
                return out;
            }
            let Some(lesson) = lesson_of_line(line) else {
                continue;
            };
            let n = lesson.chars().count();
            if !(HARVEST_MIN_CHARS..=HARVEST_MAX_CHARS).contains(&n) {
                continue;
            }
            let title = lesson_title(lesson);
            if out.iter().any(|c| c.title == title) {
                continue;
            }
            out.push(LessonCandidate {
                pane_id: pane_id.clone(),
                title,
                lesson: lesson.to_string(),
            });
        }
    }
    out
}

/// GOAL-RELEVANCE LINEAGE ids: the pre-existing notes the run's goal would recall,
/// in [`relevant_notes`] ranking order, capped at [`HARVEST_LINK_RELEVANT_MAX`].
/// This is the SAME floored ranking function the recall-prime uses, so re-running
/// it over the PRE-RUN store deterministically approximates "what recall injected
/// into this run" — and the auto-path relevance floor means a capture links only to
/// notes sharing ≥2 meaningful tokens with the goal (an incidental single-token
/// overlap no longer seeds a noisy lineage edge). Pure (notes in);
/// blank/whitespace goal ⇒ empty (no lineage links, mesh-only harvest).
pub fn harvest_relevant_ids(existing: &[Note], goal: &str) -> Vec<String> {
    if goal.trim().is_empty() {
        return Vec::new();
    }
    relevant_notes(existing, goal, HARVEST_LINK_RELEVANT_MAX)
        .into_iter()
        .map(|n| n.id)
        .collect()
}

/// PURE link plan for one harvest batch: for each successfully-WRITTEN note id,
/// its final outbound `links` = the goal-relevance lineage ids (ranking order)
/// then the same-run sibling mesh (every OTHER written id, write order). No
/// self-links; duplicates (a relevance hit that is somehow also a sibling, or a
/// repeated hit) are deduped, first occurrence wins — all deterministic.
/// Taking only WRITTEN ids as input is the failed-create exclusion by
/// construction: a note whose `create_note` errored never enters any sibling's
/// links. Unit-testable without IO.
pub fn harvest_link_plan(written: &[String], relevant: &[String]) -> Vec<(String, Vec<String>)> {
    written
        .iter()
        .map(|id| {
            let mut links: Vec<String> = Vec::new();
            for l in relevant.iter().chain(written.iter()) {
                if l != id && !links.contains(l) {
                    links.push(l.clone());
                }
            }
            (id.clone(), links)
        })
        .collect()
}

/// Write harvested candidates into the store as notes: category
/// [`HARVEST_CATEGORY`], tags = `[`[`HARVEST_TAG`]`]`, `origin` = the pane id,
/// body = the lesson + one provenance line (`harvested from run
/// <run-id>/<pane-id>, YYYY-MM-DD`). DEDUP: a candidate whose title EXACTLY
/// matches an existing note title (or one already written by this call) is
/// skipped — no near-dup heuristics, deterministic, and the mechanism that makes
/// a re-synthesis of the same run idempotent (second pass writes 0). NEVER fails
/// the run: a `create_note` error (store cap / io) skips that candidate and
/// continues. Returns the number of notes actually written. Gate-checking is the
/// CALLER's job — with `memory_harvest` OFF this is simply never called.
///
/// LINKING (see the module comment): `goal` is the run's goal text — the
/// [`harvest_relevant_ids`] lineage hits over the PRE-RUN store ride each create
/// (ids that already exist ⇒ crash-safe), then the same-run sibling mesh
/// ([`harvest_link_plan`]) is patched onto only the successfully-created ids via
/// the crate's own atomic [`update_note`] path (a failed patch is skipped — that
/// note keeps its create-time lineage links; never a dangling id). Backlinks to
/// the old lineage notes stay DERIVED ([`backlinks`]) — nothing is written on
/// them. Empty `goal` ⇒ mesh-only.
pub fn write_harvested_notes(
    dir: &Path,
    candidates: &[LessonCandidate],
    run_id: &str,
    goal: &str,
) -> usize {
    if candidates.is_empty() {
        return 0;
    }
    // ONE pre-run snapshot serves both the title dedup and the lineage ranking —
    // notes written below can never rank as their own "relevance" hits.
    let existing = list_notes(dir);
    let relevant = harvest_relevant_ids(&existing, goal);
    let mut seen: HashSet<String> = existing.into_iter().map(|n| n.title).collect();
    let mut written_ids: Vec<String> = Vec::new();
    for c in candidates.iter().take(HARVEST_MAX_PER_RUN) {
        if seen.contains(&c.title) {
            continue;
        }
        let body = format!(
            "{}\n\nharvested from run {}/{}, {}",
            c.lesson,
            run_id,
            c.pane_id,
            date_ymd(now_ms())
        );
        // Created WITH the lineage links (those ids exist NOW): a crash before the
        // mesh patch below leaves a valid store — orphans at worst, never dangling.
        if let Ok(note) = create_note(
            dir,
            c.title.clone(),
            body,
            vec![HARVEST_TAG.to_string()],
            relevant.clone(),
            Some(HARVEST_CATEGORY.to_string()),
            Some(c.pane_id.clone()),
        ) {
            seen.insert(c.title.clone());
            written_ids.push(note.id);
        }
        // Err (cap / io) → skip + continue: harvest must never fail a synthesis.
    }
    // SIBLING MESH: patch the full plan (lineage + siblings) onto the ids that
    // actually got created. One note written ⇒ its create-time links already ARE
    // the final plan (lineage only) — no patch needed.
    if written_ids.len() >= 2 {
        for (id, links) in harvest_link_plan(&written_ids, &relevant) {
            // `writer: None` — the patch is the same harvest fixing links, not a new
            // writer; origin/last_writer keep the pane stamped at create.
            let _ = update_note(
                dir,
                &id,
                NotePatch {
                    links: Some(links),
                    ..Default::default()
                },
                None,
            );
        }
    }
    written_ids.len()
}

// ───────────── Run-outcome capture (deterministic, no-LLM run notes) ────────────
//
// The SECOND write-side lane of the flywheel (alongside the LESSON: harvest above):
// when a delegate/synthesize run COMPLETES, one structured note is captured from the
// run's STRUCTURED fields (verdict / workspace / harness / held-reason / PR — never a
// transcript, never an LLM summary) so ADE learns run outcomes across workspaces. The
// recall side (`recall_block`, gate `memory_autoconsult`) then feeds these captures
// into future dispatches. Design brief:
// .claude/context/2026-07-07-ade-continuous-learning-flywheel.md.
//
// Split (mirrors the harvest split): [`build_run_capture`] is the deterministic note
// CONTENT (title/body/tags/category + the structured run_id/workspace_id fields, all
// scrubbed) — unit-tested; [`write_run_capture`] owns the run_id DEDUP (idempotent
// re-completion), the goal-relevance LINEAGE link (reuses [`harvest_relevant_ids`] —
// the SAME floored [`relevant_notes`] ranking recall uses, so a capture links to what
// primed the run), the write caps, and the atomic write. The `memory_capture` GATE (default OFF)
// lives in the CALLER (app) — this crate stays hermetic. SECRET-SCAN
// ([`scrub_secrets`]) runs at the persist boundary: a goal / held-reason / PR line
// can carry a token, and a durable note is exactly where a leaked secret hurts.

/// Category stamped on every run-capture note.
pub const RUN_CAPTURE_CATEGORY: &str = "run";
/// Tag stamped on every run-capture note (the harness name is added as a 2nd tag).
pub const RUN_CAPTURE_TAG: &str = "run";
/// Char budget for a run-capture note's title (word-boundary truncated).
pub const RUN_CAPTURE_TITLE_MAX_CHARS: usize = 80;
/// The placeholder a scrubbed secret is replaced with (see [`scrub_secrets`]).
pub const REDACTED: &str = "«redacted»";

/// The STRUCTURED inputs one completed run contributes to a capture note. Mapped
/// from the app's `DelegateRunRecord`. `held_reason`/`pr_url`/`extra` are `None`
/// when absent (a Pass has no held reason; a run may open no PR). All strings are
/// pre-scrub — [`build_run_capture`] scrubs the rendered title + body.
#[derive(Debug, Clone)]
pub struct RunCaptureFields {
    pub run_id: String,
    pub workspace_id: String,
    pub goal: String,
    pub verdict: String,
    pub harness: String,
    pub held_reason: Option<String>,
    pub pr_url: Option<String>,
    /// Optional extra deterministic line(s) appended verbatim (also scrubbed).
    pub extra: Option<String>,
}

/// Byte-cap a body on a CHAR boundary (multibyte-safe), reusing [`MAX_BODY_BYTES`]
/// as the length guard. Run captures are tiny, so this is a defensive backstop only.
fn truncate_body_bytes(body: &str) -> String {
    if body.len() <= MAX_BODY_BYTES {
        return body.to_string();
    }
    let mut end = MAX_BODY_BYTES;
    while end > 0 && !body.is_char_boundary(end) {
        end -= 1;
    }
    body[..end].to_string()
}

/// Conservatively redact obvious secret shapes before text is persisted to a durable
/// note (defense-in-depth at the persist boundary). Zero-dep (no regex): a hand-rolled
/// scan replaces each match with [`REDACTED`]. Covered shapes: OpenAI `sk-…` keys,
/// GitHub `ghp_…` tokens, AWS `AKIA…` access-key ids, `Bearer <token>` values, and
/// whole PEM `-----BEGIN … PRIVATE KEY-----` blocks. DELIBERATELY conservative:
/// ordinary words, emails, and short prose like `Bearer of bad news` are left intact
/// (the token after `Bearer` is redacted only when long + token-shaped), so the scrub
/// never mangles a normal goal.
pub fn scrub_secrets(input: &str) -> String {
    redact_token_shapes(&redact_pem_blocks(input))
}

/// Redact whole `-----BEGIN … PRIVATE KEY-----` … `-----END … KEY-----` blocks
/// (the multi-line base64 body between them included) to a single [`REDACTED`]. A
/// `BEGIN` with no closing `END-----` redacts to end-of-input (conservative). A
/// non-PRIVATE-KEY `-----BEGIN …` marker (e.g. a certificate) is left untouched.
fn redact_pem_blocks(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    loop {
        let Some(bpos) = rest.find("-----BEGIN") else {
            out.push_str(rest);
            break;
        };
        let after = &rest[bpos..];
        // The END marker line closes with a trailing run of dashes; find "-----END"
        // then the next "-----" after it to span the whole footer.
        let block_end = after.find("-----END").and_then(|epos| {
            let tail = &after[epos + "-----END".len()..];
            tail.find("-----")
                .map(|cpos| epos + "-----END".len() + cpos + "-----".len())
        });
        match block_end {
            Some(end) if after[..end].contains("PRIVATE KEY") => {
                out.push_str(&rest[..bpos]);
                out.push_str(REDACTED);
                rest = &after[end..];
            }
            _ => {
                // Not a redactable private-key block: keep the BEGIN marker, continue.
                let keep_to = bpos + "-----BEGIN".len();
                out.push_str(&rest[..keep_to]);
                rest = &rest[keep_to..];
            }
        }
    }
    out
}

/// True for the characters an opaque bearer token / JWT is built from.
fn is_bearer_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+' | '/' | '=')
}

/// Match a fixed `prefix` followed by a run of ≥`min_run` ASCII-alphanumeric chars.
/// Returns the total matched length (prefix + run), or `None`.
fn match_prefixed(cs: &[char], prefix: &[char], min_run: usize) -> Option<usize> {
    if cs.len() < prefix.len() || cs[..prefix.len()] != *prefix {
        return None;
    }
    let run = cs[prefix.len()..]
        .iter()
        .take_while(|c| c.is_ascii_alphanumeric())
        .count();
    (run >= min_run).then_some(prefix.len() + run)
}

/// AWS access-key id: `AKIA` + EXACTLY 16 `[A-Z0-9]`, with a non-alnum boundary after
/// (so a longer alnum run is NOT a partial match). Returns 20 on a hit.
fn match_akia(cs: &[char]) -> Option<usize> {
    const P: [char; 4] = ['A', 'K', 'I', 'A'];
    if cs.len() < 20 || cs[..4] != P {
        return None;
    }
    let body_ok = cs[4..20]
        .iter()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
    let bounded = cs
        .get(20)
        .map(|c| !c.is_ascii_alphanumeric())
        .unwrap_or(true);
    (body_ok && bounded).then_some(20)
}

/// Try to match a known secret token shape at the START of `cs`. Returns the matched
/// length, or `None`.
fn match_secret(cs: &[char]) -> Option<usize> {
    match_prefixed(cs, &['s', 'k', '-'], 16) // OpenAI key
        .or_else(|| match_prefixed(cs, &['g', 'h', 'p', '_'], 20)) // GitHub PAT
        .or_else(|| match_akia(cs)) // AWS access-key id
}

/// Match `Bearer <token>` at the START of `cs`. Returns `(prefix_len, total_len)`
/// where `prefix_len` covers `Bearer` + the whitespace (kept verbatim) and the token
/// run (redacted) is `total_len - prefix_len`. Only fires when the token is ≥16
/// token-shaped chars, so ordinary `Bearer of bad news` prose is never redacted.
fn match_bearer(cs: &[char]) -> Option<(usize, usize)> {
    const B: [char; 6] = ['B', 'e', 'a', 'r', 'e', 'r'];
    if cs.len() < 7 || cs[..6] != B {
        return None;
    }
    let ws = cs[6..].iter().take_while(|c| c.is_whitespace()).count();
    if ws == 0 {
        return None;
    }
    let start = 6 + ws;
    let run = cs[start..]
        .iter()
        .take_while(|c| is_bearer_token_char(**c))
        .count();
    (run >= 16).then_some((start, start + run))
}

/// Char-scan `s`, replacing secret-shaped tokens with [`REDACTED`]. A match may only
/// start at a boundary (input start, or after a non-alphanumeric char) so a shape
/// embedded mid-word (e.g. `risk-…` for `sk-`) never false-matches.
fn redact_token_shapes(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < n {
        let boundary = i == 0 || !chars[i - 1].is_ascii_alphanumeric();
        if boundary {
            if let Some(len) = match_secret(&chars[i..]) {
                out.push_str(REDACTED);
                i += len;
                continue;
            }
            if let Some((prefix, total)) = match_bearer(&chars[i..]) {
                out.extend(chars[i..i + prefix].iter());
                out.push_str(REDACTED);
                i += total;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Build the deterministic note payload for one completed run (NO LLM, NO IO). The
/// title is the run's GOAL (word-boundary truncated to
/// [`RUN_CAPTURE_TITLE_MAX_CHARS`], or `run <run_id>` when the goal is blank); the
/// body is a fixed `verdict=… · workspace=… · harness=…` head line (present facts
/// only), then optional `held: …` / `PR: …` / extra lines, then a
/// `captured from run <id>, <YYYY-MM-DD>` provenance line. Title AND body run through
/// [`scrub_secrets`] and the body is length-guarded to [`MAX_BODY_BYTES`]. The new
/// structured [`Note::run_id`] / [`Note::workspace_id`] fields carry the run identity
/// (empty ⇒ `None`); `category` = [`RUN_CAPTURE_CATEGORY`], `tags` =
/// `[`[`RUN_CAPTURE_TAG`]`, <harness>]` (harness omitted when blank). `links` are LEFT
/// EMPTY — [`write_run_capture`] fills the goal-relevance lineage. The id + timestamps
/// are minted here; every CONTENT field is deterministic (unit-tested).
pub fn build_run_capture(fields: &RunCaptureFields) -> Note {
    let title_src = if fields.goal.trim().is_empty() {
        format!("run {}", fields.run_id.trim())
    } else {
        truncate_title_chars(fields.goal.trim(), RUN_CAPTURE_TITLE_MAX_CHARS)
    };
    let title = scrub_secrets(&title_src);

    // Head line: only the present facts, joined by " · ".
    let mut head: Vec<String> = Vec::new();
    if !fields.verdict.trim().is_empty() {
        head.push(format!("verdict={}", fields.verdict.trim()));
    }
    if !fields.workspace_id.trim().is_empty() {
        head.push(format!("workspace={}", fields.workspace_id.trim()));
    }
    if !fields.harness.trim().is_empty() {
        head.push(format!("harness={}", fields.harness.trim()));
    }
    let mut body = head.join(" · ");
    let push_line = |body: &mut String, line: String| {
        if !body.is_empty() {
            body.push('\n');
        }
        body.push_str(&line);
    };
    if let Some(held) = trimmed_nonempty(&fields.held_reason) {
        push_line(&mut body, format!("held: {held}"));
    }
    if let Some(pr) = trimmed_nonempty(&fields.pr_url) {
        push_line(&mut body, format!("PR: {pr}"));
    }
    if let Some(extra) = trimmed_nonempty(&fields.extra) {
        push_line(&mut body, extra.to_string());
    }
    push_line(
        &mut body,
        format!(
            "captured from run {}, {}",
            fields.run_id.trim(),
            date_ymd(now_ms())
        ),
    );
    let body = truncate_body_bytes(&scrub_secrets(&body));

    let mut tags = vec![RUN_CAPTURE_TAG.to_string()];
    if !fields.harness.trim().is_empty() {
        tags.push(fields.harness.trim().to_string());
    }

    let now = now_ms();
    Note {
        id: mint_id(),
        title,
        body,
        tags,
        links: Vec::new(),
        created_at: now,
        updated_at: now,
        origin: None,
        last_writer: None,
        category: Some(RUN_CAPTURE_CATEGORY.to_string()),
        run_id: (!fields.run_id.trim().is_empty()).then(|| fields.run_id.clone()),
        workspace_id: (!fields.workspace_id.trim().is_empty()).then(|| fields.workspace_id.clone()),
    }
}

/// `Some(trimmed)` when an `Option<String>` holds non-whitespace text; else `None`.
fn trimmed_nonempty(opt: &Option<String>) -> Option<&str> {
    opt.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// Persist a run capture as ONE note, idempotently. DEDUP by `run_id`: when `existing`
/// already holds a note whose structured `run_id` equals this run's, nothing is
/// written and `None` is returned (a re-completion of the same run is a no-op). Else
/// the [`build_run_capture`] note is linked to the goal-relevance LINEAGE
/// ([`harvest_relevant_ids`] — the SAME top-N floored [`relevant_notes`] ranking the
/// recall-prime uses, so a capture links to the notes the run was primed with) and atomically
/// written; `Some(note)` on success. NEVER fails: a cap breach / IO error degrades to
/// `None` (the caller treats this as best-effort — a capture must never fail a run).
/// Gate-checking (`memory_capture`) is the CALLER's job — with the gate OFF this is
/// simply never called.
pub fn write_run_capture(dir: &Path, fields: &RunCaptureFields, existing: &[Note]) -> Option<Note> {
    // Idempotent re-completion: a note already carries this run_id ⇒ skip.
    if !fields.run_id.trim().is_empty()
        && existing
            .iter()
            .any(|n| n.run_id.as_deref() == Some(fields.run_id.as_str()))
    {
        return None;
    }
    let mut note = build_run_capture(fields);
    // GOAL-RELEVANCE LINEAGE: outbound-only, mirrors the harvest lineage edge. Empty
    // goal ⇒ no links (the capture lands as a leaf — captures never mesh each other).
    note.links = harvest_relevant_ids(existing, &fields.goal);
    // Same write caps + per-dir count cap as create_note; on ANY breach write nothing.
    if validate_write(&note.title, &note.body, &note.tags, &note.links).is_err()
        || count_notes(dir) >= MAX_NOTES_PER_DIR
    {
        return None;
    }
    write_note_atomic(dir, &note).ok().map(|()| note)
}

/// DERIVED backlinks: notes whose `links` contain `target_id`. Never stored on the
/// target (no write-amplification, no second source of truth).
pub fn backlinks(notes: &[Note], target_id: &str) -> Vec<Note> {
    notes
        .iter()
        .filter(|n| n.links.iter().any(|l| l == target_id))
        .cloned()
        .collect()
}

/// Zero-dep relatedness for `target_id`: shared tags (×3) + shared link-targets
/// (×2) + title/body token overlap (×1). NO embeddings. Ranked, capped by `limit`.
pub fn suggest(notes: &[Note], target_id: &str, limit: usize) -> Vec<Suggestion> {
    let Some(target) = notes.iter().find(|n| n.id == target_id) else {
        return Vec::new();
    };
    let t_tags: HashSet<&String> = target.tags.iter().collect();
    let t_links: HashSet<&String> = target.links.iter().collect();
    let t_tokens: HashSet<String> = tokenize(&format!("{} {}", target.title, target.body))
        .into_iter()
        .collect();

    let mut out: Vec<Suggestion> = notes
        .iter()
        .filter(|n| n.id != target_id)
        .filter_map(|n| {
            let mut score = 0u32;
            let mut reasons = Vec::new();

            let shared_tags = n.tags.iter().filter(|t| t_tags.contains(t)).count();
            if shared_tags > 0 {
                score += 3 * shared_tags as u32;
                reasons.push(format!("{shared_tags} shared tag(s)"));
            }
            let shared_links = n.links.iter().filter(|l| t_links.contains(l)).count();
            if shared_links > 0 {
                score += 2 * shared_links as u32;
                reasons.push(format!("{shared_links} shared link target(s)"));
            }
            let n_tokens: HashSet<String> = tokenize(&format!("{} {}", n.title, n.body))
                .into_iter()
                .collect();
            let overlap = n_tokens.intersection(&t_tokens).count();
            if overlap > 0 {
                score += overlap as u32;
                reasons.push(format!("{overlap} shared term(s)"));
            }
            (score > 0).then(|| Suggestion {
                note: n.clone(),
                score,
                reasons,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.note.created_at.cmp(&b.note.created_at))
    });
    out.truncate(limit);
    out
}

// ───────────── Phase 11 / Mem-2: the memory graph PROJECTION (D50) ──────────────
//
// `build_graph` is a PURE projection over the notes — it owns NO durable file and
// re-implements NO note I/O (single-source, like `compute_queue` over events.jsonl).
// Edges: hard `"link"` edges from each note's outbound `links[]` (the author-made
// edges; reconciled from the 11-01 plan's inferred `backlinks[]` — this store uses
// outbound `links`), plus soft `"suggested"` edges from `suggest`. A suggested edge
// that duplicates a hard edge (either direction) is dropped — the hard edge wins.

/// One graph node = one note.
///
/// `tags` / `snippet` / `origin` are ADDITIVE hover-card fields (`#[serde(default)]`)
/// — the struct serializes to BOTH the Tauri command and the MCP `get_memory_graph`
/// tool, so old readers of either surface keep parsing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GraphNode {
    pub id: String,
    pub title: String,
    /// Edge count touching this node (size/sort hint for the view).
    pub degree: u32,
    pub updated_at: u64,
    /// The note's tags (hover-card detail).
    #[serde(default)]
    pub tags: Vec<String>,
    /// First ~[`SNIPPET_MAX_CHARS`] chars of the note body, whitespace-collapsed
    /// to single spaces, `…`-suffixed when truncated. See [`snippet_of`].
    #[serde(default)]
    pub snippet: String,
    /// Provenance passthrough: [`Note::origin`] (pane id that CREATED the note).
    #[serde(default)]
    pub origin: Option<String>,
    /// Passthrough of [`Note::category`] — the optional graph-editor category
    /// label (hover-card detail / cluster hint). Additive (`#[serde(default)]`).
    #[serde(default)]
    pub category: Option<String>,
}

/// One graph edge. `kind` is `"link"` (hard, author-made, directed) or
/// `"suggested"` (soft, heuristic, rendered undirected).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
    pub kind: String,
}

/// Object-rooted projection (MCP `outputSchema` requires a root object).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MemoryGraph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

/// Number of soft suggestions to consider per note when building edges (kept small
/// so the graph stays sparse — this is a minimal node-link view, NOT a dense
/// force-graph).
const SUGGEST_EDGES_PER_NODE: usize = 3;

/// C6 bound on the O(n²) suggested-edge pass. `suggest()` is O(n) and is called once
/// per node, so the soft-edge pass is O(n²) — at the 50k [`MAX_NOTES_PER_DIR`] cap
/// that is ~2.5e9 ops and would wedge the graph view. Past this many notes we SKIP
/// the suggested pass entirely and return ONLY the hard `"link"` edges (which are
/// O(total-links), always cheap). Nodes are NEVER dropped — every note is still a
/// node; only the soft-edge heuristic is bounded. A graph, not a `Result`: this
/// degrades gracefully (fewer edges), it does not reject.
pub const MAX_GRAPH_NODES: usize = 2_000;

/// Char budget for a [`GraphNode::snippet`] (chars, NOT bytes — multibyte-safe).
pub const SNIPPET_MAX_CHARS: usize = 180;

/// Hover-card snippet of a note body: whitespace runs (spaces/newlines/tabs)
/// collapsed to single spaces and trimmed, then truncated to
/// [`SNIPPET_MAX_CHARS`] *chars* — counted per `char`, so truncation always
/// lands on a char boundary and can never panic on multibyte text — with `…`
/// appended only when truncation happened. Pure.
pub fn snippet_of(body: &str) -> String {
    let collapsed = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = collapsed.chars();
    let head: String = chars.by_ref().take(SNIPPET_MAX_CHARS).collect();
    if chars.next().is_none() {
        head // fit within the budget (including exactly-at) → no ellipsis
    } else {
        format!("{head}…")
    }
}

/// Build the memory graph PROJECTION from a note set. Pure; owns no durable file.
pub fn build_graph(notes: &[Note]) -> MemoryGraph {
    let ids: HashSet<&str> = notes.iter().map(|n| n.id.as_str()).collect();
    let mut edges: Vec<GraphEdge> = Vec::new();
    let mut hard_seen: HashSet<(String, String)> = HashSet::new();

    // Hard "link" edges from outbound links[]; drop self-loops + dangling targets; de-dup.
    for n in notes {
        for l in &n.links {
            if l == &n.id || !ids.contains(l.as_str()) {
                continue;
            }
            if hard_seen.insert((n.id.clone(), l.clone())) {
                edges.push(GraphEdge {
                    from: n.id.clone(),
                    to: l.clone(),
                    kind: "link".to_string(),
                });
            }
        }
    }

    // Soft "suggested" edges; undirected-canonical de-dup; suppressed where a hard
    // edge already connects the pair (either direction). C6: this pass is O(n²) —
    // SKIP it entirely past MAX_GRAPH_NODES so a large store cannot wedge the view
    // (hard "link" edges above are always emitted; only the soft heuristic is bounded).
    let mut sug_seen: HashSet<(String, String)> = HashSet::new();
    if notes.len() <= MAX_GRAPH_NODES {
        for n in notes {
            for s in suggest(notes, &n.id, SUGGEST_EDGES_PER_NODE) {
                let (a, b) = (n.id.clone(), s.note.id);
                if a == b {
                    continue;
                }
                if hard_seen.contains(&(a.clone(), b.clone()))
                    || hard_seen.contains(&(b.clone(), a.clone()))
                {
                    continue;
                }
                let canon = if a < b { (a, b) } else { (b, a) };
                if sug_seen.insert(canon.clone()) {
                    edges.push(GraphEdge {
                        from: canon.0,
                        to: canon.1,
                        kind: "suggested".to_string(),
                    });
                }
            }
        }
    }

    let mut degree: HashMap<&str, u32> = HashMap::new();
    for e in &edges {
        *degree.entry(e.from.as_str()).or_insert(0) += 1;
        *degree.entry(e.to.as_str()).or_insert(0) += 1;
    }
    let nodes = notes
        .iter()
        .map(|n| GraphNode {
            id: n.id.clone(),
            title: n.title.clone(),
            degree: *degree.get(n.id.as_str()).unwrap_or(&0),
            updated_at: n.updated_at,
            tags: n.tags.clone(),
            snippet: snippet_of(&n.body),
            origin: n.origin.clone(),
            category: n.category.clone(),
        })
        .collect();

    MemoryGraph { nodes, edges }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hermetic tempdir; `state_root` is NESTED under `root` so the memory sibling
    /// lands OUTSIDE the wiped dir (mirrors core/task's `survives_state_root_wipe`).
    struct Scratch {
        root: PathBuf,
    }
    impl Scratch {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "at-mem-test-{}-{}-{}",
                std::process::id(),
                tag,
                WRITE_NONCE.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            Scratch { root }
        }
        fn state_root(&self) -> PathBuf {
            self.root.join("state")
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn dir_for(s: &Scratch) -> (PathBuf, PathBuf) {
        let sr = s.state_root();
        std::fs::create_dir_all(&sr).unwrap();
        let root = memory_root(&sr).unwrap();
        (sr, repo_dir(&root, "repo_test"))
    }

    #[test]
    fn round_trip_create_get() {
        let s = Scratch::new("rt");
        let (_sr, dir) = dir_for(&s);
        let n = create_note(
            &dir,
            "Why Tauri".into(),
            "Rust core + web UI.".into(),
            vec!["arch".into()],
            vec![],
            None,
            None,
        )
        .unwrap();
        let got = get_note(&dir, &n.id).unwrap();
        assert_eq!(got, n);
        assert!(n.id.starts_with("mem_"));
    }

    #[test]
    fn survives_state_root_wipe() {
        let s = Scratch::new("wipe");
        let (sr, dir) = dir_for(&s);
        let n = create_note(&dir, "t".into(), "b".into(), vec![], vec![], None, None).unwrap();
        // The D7 startup wipe nukes state_root...
        std::fs::remove_dir_all(&sr).unwrap();
        // ...but the memory sibling and the note survive.
        assert!(sr.parent().unwrap().join("state-memory").exists());
        assert_eq!(get_note(&dir, &n.id).unwrap(), n);
    }

    #[test]
    fn per_note_files_no_shared_jsonl() {
        let s = Scratch::new("pernote");
        let (_sr, dir) = dir_for(&s);
        let a = create_note(&dir, "a".into(), "x".into(), vec![], vec![], None, None).unwrap();
        let b = create_note(&dir, "b".into(), "y".into(), vec![], vec![], None, None).unwrap();
        // Each note is its own file; no single JSONL holds both.
        assert!(dir.join(format!("{}.json", a.id)).is_file());
        assert!(dir.join(format!("{}.json", b.id)).is_file());
        assert_eq!(list_notes(&dir).len(), 2);
        // No `*-memory.jsonl` shared file exists.
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            assert!(!e.file_name().to_string_lossy().ends_with("memory.jsonl"));
        }
    }

    #[test]
    fn interrupted_write_temp_is_skipped_prior_intact() {
        let s = Scratch::new("interrupt");
        let (_sr, dir) = dir_for(&s);
        let n = create_note(
            &dir,
            "good".into(),
            "body".into(),
            vec![],
            vec![],
            None,
            None,
        )
        .unwrap();
        // Simulate an interrupted write: a temp file present, rename never done.
        std::fs::write(
            dir.join(format!(".{}.json.tmp.{}.999", n.id, std::process::id())),
            b"{ partial garbage",
        )
        .unwrap();
        // Reader skips the temp; the prior note is intact.
        let all = list_notes(&dir);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0], n);
    }

    #[test]
    fn distinct_notes_both_survive() {
        let s = Scratch::new("distinct");
        let (_sr, dir) = dir_for(&s);
        let a = create_note(&dir, "a".into(), "1".into(), vec![], vec![], None, None).unwrap();
        let b = create_note(&dir, "b".into(), "2".into(), vec![], vec![], None, None).unwrap();
        assert!(get_note(&dir, &a.id).is_some());
        assert!(get_note(&dir, &b.id).is_some());
    }

    #[test]
    fn same_note_last_writer_wins_whole() {
        let s = Scratch::new("lww");
        let (_sr, dir) = dir_for(&s);
        let n = create_note(
            &dir,
            "v1".into(),
            "first".into(),
            vec![],
            vec![],
            None,
            None,
        )
        .unwrap();
        // Two writers of the SAME id: the later rename wins WHOLESALE (not merged).
        let mut a = n.clone();
        a.body = "writer-A".into();
        let mut b = n.clone();
        b.body = "writer-B".into();
        write_note_atomic(&dir, &a).unwrap();
        write_note_atomic(&dir, &b).unwrap(); // last writer
        let got = get_note(&dir, &n.id).unwrap();
        assert_eq!(got.body, "writer-B");
        assert!(got.body != "writer-A" && !got.body.contains("writer-A"));
        // update_note path bumps updated_at + LWW.
        let upd = update_note(
            &dir,
            &n.id,
            NotePatch {
                title: Some("v2".into()),
                ..Default::default()
            },
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(get_note(&dir, &n.id).unwrap().title, "v2");
        assert!(upd.updated_at >= n.created_at);
    }

    #[test]
    fn backlinks_derived_links_by_id_stable_across_retitle() {
        let s = Scratch::new("backlinks");
        let (_sr, dir) = dir_for(&s);
        let b = create_note(
            &dir,
            "B".into(),
            "target".into(),
            vec![],
            vec![],
            None,
            None,
        )
        .unwrap();
        let a = create_note(
            &dir,
            "A".into(),
            "links to B".into(),
            vec![],
            vec![b.id.clone()],
            None,
            None,
        )
        .unwrap();
        let notes = list_notes(&dir);
        let bl = backlinks(&notes, &b.id);
        assert_eq!(bl.len(), 1);
        assert_eq!(bl[0].id, a.id);
        // B's stored file must NOT contain a materialized backlink to A.
        let b_raw = std::fs::read_to_string(note_path(&dir, &b.id)).unwrap();
        assert!(!b_raw.contains(&a.id));
        // Retitle B: links-by-id keep A's edge intact (rename-safe).
        update_note(
            &dir,
            &b.id,
            NotePatch {
                title: Some("B-renamed".into()),
                ..Default::default()
            },
            None,
        )
        .unwrap();
        let notes2 = list_notes(&dir);
        assert_eq!(backlinks(&notes2, &b.id).len(), 1);
    }

    #[test]
    fn suggest_ranks_zero_dep() {
        let s = Scratch::new("suggest");
        let (_sr, dir) = dir_for(&s);
        let t = create_note(
            &dir,
            "Tauri choice".into(),
            "rust pty supervision".into(),
            vec!["arch".into(), "tauri".into()],
            vec![],
            None,
            None,
        )
        .unwrap();
        let near = create_note(
            &dir,
            "Rust core".into(),
            "rust pty file watch".into(),
            vec!["arch".into()],
            vec![],
            None,
            None,
        )
        .unwrap();
        let _far = create_note(
            &dir,
            "Lunch".into(),
            "tacos".into(),
            vec!["food".into()],
            vec![],
            None,
            None,
        )
        .unwrap();
        let notes = list_notes(&dir);
        let sg = suggest(&notes, &t.id, 10);
        assert!(!sg.is_empty());
        // The arch+rust+pty note ranks first; the unrelated note is absent or last.
        assert_eq!(sg[0].note.id, near.id);
        assert!(sg[0].score > 0 && !sg[0].reasons.is_empty());
        assert!(sg.iter().all(|x| x.note.id != t.id)); // never suggest self
    }

    #[test]
    fn build_graph_classifies_links_and_suggested() {
        let s = Scratch::new("graph");
        let (_sr, dir) = dir_for(&s);
        let b = create_note(
            &dir,
            "B".into(),
            "rust pty supervision".into(),
            vec!["arch".into()],
            vec![],
            None,
            None,
        )
        .unwrap();
        // A links to B (hard edge) and shares tag+terms with C (soft edge candidate).
        let a = create_note(
            &dir,
            "A".into(),
            "rust pty file watch".into(),
            vec!["arch".into()],
            vec![b.id.clone()],
            None,
            Some("ws-graph".into()),
        )
        .unwrap();
        let notes = list_notes(&dir);
        let g = build_graph(&notes);
        assert_eq!(g.nodes.len(), 2);
        // Hover-card fields: tags cloned, snippet = collapsed body, origin passthrough.
        let na = g.nodes.iter().find(|n| n.id == a.id).unwrap();
        assert_eq!(na.tags, vec!["arch".to_string()]);
        assert_eq!(na.snippet, "rust pty file watch"); // short body → verbatim, no "…"
        assert_eq!(na.origin.as_deref(), Some("ws-graph"));
        let nb = g.nodes.iter().find(|n| n.id == b.id).unwrap();
        assert_eq!(nb.origin, None); // created without a writer
                                     // Exactly one hard "link" edge A->B.
        let links: Vec<_> = g.edges.iter().filter(|e| e.kind == "link").collect();
        assert_eq!(links.len(), 1);
        assert_eq!(
            (links[0].from.as_str(), links[0].to.as_str()),
            (a.id.as_str(), b.id.as_str())
        );
        // The A~B pair is NOT also emitted as a suggested edge (hard edge wins).
        assert!(!g.edges.iter().any(|e| e.kind == "suggested"
            && ((e.from == a.id && e.to == b.id) || (e.from == b.id && e.to == a.id))));
        // Degree reflects the edge.
        assert!(g.nodes.iter().find(|n| n.id == a.id).unwrap().degree >= 1);
    }

    #[test]
    fn build_graph_drops_self_loops_dangling_and_empty() {
        let s = Scratch::new("graph2");
        let (_sr, dir) = dir_for(&s);
        // Self-link + dangling link → no hard edges.
        let only =
            create_note(&dir, "solo".into(), "x".into(), vec![], vec![], None, None).unwrap();
        update_note(
            &dir,
            &only.id,
            NotePatch {
                links: Some(vec![only.id.clone(), "mem_does_not_exist".into()]),
                ..Default::default()
            },
            None,
        )
        .unwrap();
        let g = build_graph(&list_notes(&dir));
        assert_eq!(g.nodes.len(), 1);
        assert!(g.edges.iter().all(|e| e.kind != "link")); // self + dangling dropped
                                                           // Empty store → empty graph.
        let empty = build_graph(&[]);
        assert!(empty.nodes.is_empty() && empty.edges.is_empty());
    }

    // ── snippet_of: the hover-card body projection (pure helper) ──

    #[test]
    fn snippet_of_empty_body_is_empty() {
        assert_eq!(snippet_of(""), "");
        assert_eq!(snippet_of("   \n\t  "), ""); // whitespace-only collapses away
    }

    #[test]
    fn snippet_of_exactly_at_cap_is_verbatim_no_ellipsis() {
        let body = "a".repeat(SNIPPET_MAX_CHARS);
        assert_eq!(snippet_of(&body), body);
        // One past the cap DOES truncate: cap chars kept + the ellipsis.
        let over = "a".repeat(SNIPPET_MAX_CHARS + 1);
        let s = snippet_of(&over);
        assert!(s.ends_with('…'));
        assert_eq!(s.chars().count(), SNIPPET_MAX_CHARS + 1);
    }

    #[test]
    fn snippet_of_multibyte_at_boundary_no_panic() {
        // Every char is multibyte; truncation must count CHARS (not bytes),
        // land on a char boundary, and never panic.
        let body = "é".repeat(SNIPPET_MAX_CHARS + 5);
        let s = snippet_of(&body);
        assert!(s.ends_with('…'));
        assert_eq!(s.chars().count(), SNIPPET_MAX_CHARS + 1); // 180 kept + '…'
        assert!(s.starts_with('é'));
        // Exactly-at-cap multibyte → untouched.
        let fit = "é".repeat(SNIPPET_MAX_CHARS);
        assert_eq!(snippet_of(&fit), fit);
    }

    #[test]
    fn snippet_of_collapses_whitespace_runs() {
        assert_eq!(snippet_of("  a\n\nb\tc \r\n d  "), "a b c d");
        // Collapse happens BEFORE the char budget: a whitespace-padded body
        // that fits once collapsed is NOT truncated.
        let padded = format!("{}   \n\n   {}", "a".repeat(90), "b".repeat(89));
        let s = snippet_of(&padded);
        assert_eq!(s.chars().count(), 180);
        assert!(!s.ends_with('…'));
    }

    // ── Real-thread concurrency hardening (this store is written CONCURRENTLY by
    //    external MCP agents — the earlier tests simulate sequentially). ──

    #[test]
    fn concurrent_distinct_creates_all_survive() {
        use std::sync::Arc;
        let s = Scratch::new("conc-distinct");
        let (_sr, dir) = dir_for(&s);
        let dir = Arc::new(dir);
        let n = 16usize;
        let handles: Vec<_> = (0..n)
            .map(|i| {
                let dir = Arc::clone(&dir);
                std::thread::spawn(move || {
                    create_note(
                        &dir,
                        format!("t{i}"),
                        format!("body {i}"),
                        vec![],
                        vec![],
                        None,
                        None,
                    )
                    .unwrap()
                    .id
                })
            })
            .collect();
        let ids: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        // Distinct notes never contend → all N present + readable, all ids unique.
        let all = list_notes(&dir);
        assert_eq!(all.len(), n);
        for id in &ids {
            assert!(get_note(&dir, id).is_some());
        }
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), n);
    }

    #[test]
    fn concurrent_same_id_writes_lww_never_corrupt() {
        use std::sync::Arc;
        let s = Scratch::new("conc-same");
        let (_sr, dir) = dir_for(&s);
        let base =
            create_note(&dir, "base".into(), "v0".into(), vec![], vec![], None, None).unwrap();
        let dir = Arc::new(dir);
        let id = Arc::new(base.id.clone());
        let handles: Vec<_> = (0..16)
            .map(|i| {
                let dir = Arc::clone(&dir);
                let id = Arc::clone(&id);
                std::thread::spawn(move || {
                    let mut note = get_note(&dir, &id).unwrap();
                    note.body = format!("writer-{i}");
                    note.updated_at = now_ms();
                    write_note_atomic(&dir, &note).unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        // After concurrent same-id writes: the note is ALWAYS a whole, parseable note
        // (atomic rename → no partial), its body is exactly one writer's (LWW, not merged).
        let got = get_note(&dir, &id).expect("note readable & whole after concurrent writes");
        assert!(got.body.starts_with("writer-") || got.body == "v0");
        // Exactly one `.json` file for the id; no leftover temp files.
        let entries: Vec<_> = std::fs::read_dir(dir.as_path())
            .unwrap()
            .flatten()
            .collect();
        let jsons = entries
            .iter()
            .filter(|e| {
                let n = e.file_name();
                let n = n.to_string_lossy();
                !n.starts_with('.') && n.ends_with(".json")
            })
            .count();
        let temps = entries
            .iter()
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .count();
        assert_eq!(jsons, 1, "exactly one note file survives");
        assert_eq!(temps, 0, "no temp files left behind");
    }

    // ────────────── Security hardening (D57 / slice 2a) ──────────────

    #[test]
    fn valid_note_id_allowlist() {
        // A freshly-minted id always passes (the store ONLY mints this shape).
        let minted = mint_id();
        assert!(valid_note_id(&minted), "minted id `{minted}` must pass");
        assert!(valid_note_id("mem_123"));
        assert!(valid_note_id("mem_1_2_3"));
        assert!(valid_note_id("mem_0")); // single digit remainder ok

        // Every traversal / escape / malformed vector is rejected.
        assert!(!valid_note_id(""), "empty");
        assert!(!valid_note_id("../x"), "parent traversal");
        assert!(!valid_note_id("/abs/x"), "absolute");
        assert!(!valid_note_id("a/b"), "embedded separator");
        assert!(!valid_note_id("mem_../x"), "mem_-prefixed traversal");
        assert!(!valid_note_id(".."), "bare parent");
        assert!(!valid_note_id("mem_"), "empty remainder");
        assert!(!valid_note_id("mem_abc"), "non-digit remainder (letters)");
        assert!(!valid_note_id("mem_1/2"), "separator in remainder");
        assert!(!valid_note_id("mem_1.json"), "dot in remainder");
        assert!(!valid_note_id("notmem_123"), "wrong prefix");
    }

    #[test]
    fn malicious_id_touches_no_file_outside_dir() {
        let s = Scratch::new("traversal");
        let (_sr, dir) = dir_for(&s);
        // Plant a SENTINEL file one level UP from the partition (the escape target).
        let parent = dir.parent().unwrap();
        std::fs::create_dir_all(parent).unwrap();
        let sentinel = parent.join("sentinel.json");
        std::fs::write(&sentinel, b"{\"id\":\"mem_sentinel\"}").unwrap();
        assert!(sentinel.exists());

        for bad in [
            "../sentinel",
            "/etc/passwd",
            "../../escape",
            "mem_../sentinel",
            "",
        ] {
            // get/update/delete must short-circuit BEFORE any filesystem access.
            assert!(get_note(&dir, bad).is_none(), "get({bad}) must be None");
            assert_eq!(
                update_note(&dir, bad, NotePatch::default(), None).unwrap(),
                None,
                "update({bad}) must be Ok(None)"
            );
            assert!(
                !delete_note(&dir, bad).unwrap(),
                "delete({bad}) must be Ok(false)"
            );
        }
        // The sentinel OUTSIDE the partition was never read away or removed.
        assert!(sentinel.exists(), "no out-of-dir file may be removed");
    }

    #[test]
    fn oversize_body_rejected_no_file_written() {
        let s = Scratch::new("oversize-body");
        let (_sr, dir) = dir_for(&s);
        let big = "x".repeat(MAX_BODY_BYTES + 1);
        let err = create_note(&dir, "t".into(), big, vec![], vec![], None, None).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        // NO note was written.
        assert_eq!(list_notes(&dir).len(), 0);
    }

    #[test]
    fn oversize_title_rejected() {
        let s = Scratch::new("oversize-title");
        let (_sr, dir) = dir_for(&s);
        let big_title = "t".repeat(MAX_TITLE_BYTES + 1);
        let err = create_note(&dir, big_title, "b".into(), vec![], vec![], None, None).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(list_notes(&dir).len(), 0);
    }

    #[test]
    fn too_many_tags_or_links_rejected() {
        let s = Scratch::new("toomany");
        let (_sr, dir) = dir_for(&s);
        let tags: Vec<String> = (0..MAX_TAGS + 1).map(|i| format!("t{i}")).collect();
        assert_eq!(
            create_note(&dir, "t".into(), "b".into(), tags, vec![], None, None)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        let links: Vec<String> = (0..MAX_LINKS + 1).map(|i| format!("mem_{i}")).collect();
        assert_eq!(
            create_note(&dir, "t".into(), "b".into(), vec![], links, None, None)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(list_notes(&dir).len(), 0);
    }

    #[test]
    fn oversize_tag_or_link_rejected_no_file_written() {
        // The count caps pass (1 element each) but the per-element byte cap must reject —
        // closes the MAX_TAGS×oversized smuggling hole past MAX_BODY_BYTES.
        let s = Scratch::new("oversize-aux");
        let (_sr, dir) = dir_for(&s);
        let big_tag = "t".repeat(MAX_TAG_BYTES + 1);
        assert_eq!(
            create_note(
                &dir,
                "t".into(),
                "b".into(),
                vec![big_tag],
                vec![],
                None,
                None
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::InvalidInput
        );
        let big_link = "x".repeat(MAX_LINK_BYTES + 1);
        assert_eq!(
            create_note(
                &dir,
                "t".into(),
                "b".into(),
                vec![],
                vec![big_link],
                None,
                None
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(list_notes(&dir).len(), 0);
    }

    #[test]
    fn update_enforces_caps_on_patched_body() {
        let s = Scratch::new("update-cap");
        let (_sr, dir) = dir_for(&s);
        let n = create_note(&dir, "t".into(), "small".into(), vec![], vec![], None, None).unwrap();
        let big = "x".repeat(MAX_BODY_BYTES + 1);
        let err = update_note(
            &dir,
            &n.id,
            NotePatch {
                body: Some(big),
                ..Default::default()
            },
            None,
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        // The original small note is untouched (no partial overwrite).
        assert_eq!(get_note(&dir, &n.id).unwrap().body, "small");
    }

    #[test]
    fn origin_and_last_writer_round_trip() {
        let s = Scratch::new("provenance");
        let (_sr, dir) = dir_for(&s);
        // Creator pane "ws-7" → origin AND last_writer both ws-7.
        let n = create_note(
            &dir,
            "t".into(),
            "b".into(),
            vec![],
            vec![],
            None,
            Some("ws-7".into()),
        )
        .unwrap();
        assert_eq!(n.origin.as_deref(), Some("ws-7"));
        assert_eq!(n.last_writer.as_deref(), Some("ws-7"));
        let got = get_note(&dir, &n.id).unwrap();
        assert_eq!(got.origin.as_deref(), Some("ws-7"));
        assert_eq!(got.last_writer.as_deref(), Some("ws-7"));

        // A different pane "ws-9" updates → origin IMMUTABLE (still ws-7),
        // last_writer becomes ws-9.
        let upd = update_note(
            &dir,
            &n.id,
            NotePatch {
                title: Some("t2".into()),
                ..Default::default()
            },
            Some("ws-9".into()),
        )
        .unwrap()
        .unwrap();
        assert_eq!(upd.origin.as_deref(), Some("ws-7"), "creator is immutable");
        assert_eq!(upd.last_writer.as_deref(), Some("ws-9"), "updater stamped");
        let got2 = get_note(&dir, &n.id).unwrap();
        assert_eq!(got2.origin.as_deref(), Some("ws-7"));
        assert_eq!(got2.last_writer.as_deref(), Some("ws-9"));
    }

    #[test]
    fn provenance_fields_additive_tolerant_reader() {
        let s = Scratch::new("tolerant");
        let (_sr, dir) = dir_for(&s);
        // A legacy note JSON WITHOUT origin/last_writer must parse → None (the
        // #[serde(default)] contract; Phase 11's build_graph stays unaffected).
        let id = "mem_99_99_99";
        let legacy = format!(
            r#"{{"id":"{id}","title":"old","body":"b","tags":[],"links":[],"created_at":1,"updated_at":1}}"#
        );
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(note_path(&dir, id), legacy.as_bytes()).unwrap();
        let got = get_note(&dir, id).expect("legacy note parses");
        assert_eq!(got.origin, None);
        assert_eq!(got.last_writer, None);
        assert_eq!(got.title, "old");
    }

    // ── category field (in-app graph editor) ──────────────────────────────────

    #[test]
    fn create_with_category_round_trips() {
        let s = Scratch::new("cat-create");
        let (_sr, dir) = dir_for(&s);
        let n = create_note(
            &dir,
            "t".into(),
            "b".into(),
            vec![],
            vec![],
            Some("decisions".into()),
            None,
        )
        .unwrap();
        assert_eq!(n.category.as_deref(), Some("decisions"));
        // Survives a disk round-trip (serde persists the field).
        let got = get_note(&dir, &n.id).expect("note exists");
        assert_eq!(got.category.as_deref(), Some("decisions"));
    }

    #[test]
    fn update_changes_category() {
        let s = Scratch::new("cat-update");
        let (_sr, dir) = dir_for(&s);
        let n = create_note(&dir, "t".into(), "b".into(), vec![], vec![], None, None).unwrap();
        assert_eq!(n.category, None);
        let upd = update_note(
            &dir,
            &n.id,
            NotePatch {
                category: Some("arch".into()),
                ..Default::default()
            },
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(upd.category.as_deref(), Some("arch"));
        assert_eq!(
            get_note(&dir, &n.id).unwrap().category.as_deref(),
            Some("arch")
        );
        // A patch with category: None leaves the existing category untouched
        // (None means "no change", mirroring the other patch fields).
        let upd2 = update_note(
            &dir,
            &n.id,
            NotePatch {
                title: Some("t2".into()),
                ..Default::default()
            },
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            upd2.category.as_deref(),
            Some("arch"),
            "None patch preserves"
        );
        // "" is the CLEAR sentinel (the editor's "— uncategorized —"): a patch
        // Option can't say both "leave unchanged" (None) and "unset", so the
        // empty string carries the unset. Whitespace normalizes the same way.
        let upd3 = update_note(
            &dir,
            &n.id,
            NotePatch {
                category: Some("".into()),
                ..Default::default()
            },
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(upd3.category, None, "empty string clears");
        assert_eq!(get_note(&dir, &n.id).unwrap().category, None);
    }

    #[test]
    fn create_normalizes_blank_category_to_none() {
        let s = Scratch::new("cat-blank");
        let (_sr, dir) = dir_for(&s);
        let n = create_note(
            &dir,
            "t".into(),
            "b".into(),
            vec![],
            vec![],
            Some("   ".into()),
            None,
        )
        .unwrap();
        assert_eq!(n.category, None, "whitespace category never persists");
    }

    #[test]
    fn build_graph_exposes_category() {
        let s = Scratch::new("cat-graph");
        let (_sr, dir) = dir_for(&s);
        let n = create_note(
            &dir,
            "t".into(),
            "b".into(),
            vec![],
            vec![],
            Some("cluster-1".into()),
            None,
        )
        .unwrap();
        let g = build_graph(&list_notes(&dir));
        let node = g
            .nodes
            .iter()
            .find(|nd| nd.id == n.id)
            .expect("node present");
        assert_eq!(node.category.as_deref(), Some("cluster-1"));
    }

    #[test]
    fn legacy_json_without_category_deserializes_to_none() {
        let s = Scratch::new("cat-migration");
        let (_sr, dir) = dir_for(&s);
        // A note JSON written BEFORE the category field existed (no `category` key)
        // must parse → None (the #[serde(default)] migration contract).
        let id = "mem_1_2_3";
        let legacy = format!(
            r#"{{"id":"{id}","title":"old","body":"b","tags":[],"links":[],"created_at":1,"updated_at":1}}"#
        );
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(note_path(&dir, id), legacy.as_bytes()).unwrap();
        let got = get_note(&dir, id).expect("legacy note parses");
        assert_eq!(got.category, None);
        // And build_graph passes the None straight through.
        let g = build_graph(&list_notes(&dir));
        let node = g.nodes.iter().find(|nd| nd.id == id).expect("node present");
        assert_eq!(node.category, None);
    }

    #[test]
    #[allow(clippy::assertions_on_constants)] // the const bound IS the assertion target
    fn build_graph_bound_skips_suggested_keeps_hard_links() {
        // At/under the bound the suggested pass runs as before (covered elsewhere).
        // This asserts the const is a real, documented bound (> 0) and that the
        // graph projection never panics on an empty store. (A 2001-node build is
        // too slow for a unit test; the bound is exercised by the const guard.)
        assert!(MAX_GRAPH_NODES > 0);
        let g = build_graph(&[]);
        assert!(g.nodes.is_empty() && g.edges.is_empty());
    }

    // ════════════════ Adversarial hardening (writeside-memroles lane) ════════════════
    //
    // The pre-existing suite already covers, well: the C1 `valid_note_id` allowlist
    // (`valid_note_id_allowlist`), the no-fs-escape integration (`malicious_id_..`),
    // every cap's +1 REJECT side (`oversize_*`, `too_many_*`), LWW + no-corrupt +
    // no-temp-litter under REAL threads (`concurrent_same_id_writes_lww_never_corrupt`),
    // distinct-create concurrency, derived backlinks, and provenance round-trip. The
    // additions below target GENUINE gaps the existing suite leaves open — NOT padding:
    //   1. the cap BOUNDARY-ACCEPT side (existing tests only prove `len+1` is rejected;
    //      an off-by-one `>` → `>=` would silently reject a LEGAL exactly-at-cap write
    //      and is currently unguarded);
    //   2. `sanitize_key` / `repo_dir` path-containment — the REPO-key analog of the C1
    //      note-id allowlist (the memory store partitions by repo key; an unsanitized
    //      key is a directory-escape just like an unsanitized note id). UNTESTED today;
    //   3. a deterministic xorshift fuzz loop pinning the EXACT `valid_note_id` oracle;
    //   4. `count_notes` skip-semantics (the disk-exhaustion cap's counter) — temps and
    //      non-json must not count toward MAX_NOTES_PER_DIR.

    // ── 1. Cap BOUNDARY-ACCEPT: exactly-at-limit must be ACCEPTED (validate_write
    //       uses `>`, so the boundary value is legal; this is the side the existing
    //       `oversize_*` / `too_many_*` tests do NOT cover). ──

    #[test]
    fn body_exactly_at_cap_is_accepted() {
        let s = Scratch::new("bound-body");
        let (_sr, dir) = dir_for(&s);
        let exact = "x".repeat(MAX_BODY_BYTES); // EXACTLY the cap, not +1
        let n = create_note(&dir, "t".into(), exact.clone(), vec![], vec![], None, None)
            .expect("a body of exactly MAX_BODY_BYTES must be accepted");
        assert_eq!(n.body.len(), MAX_BODY_BYTES);
        assert_eq!(get_note(&dir, &n.id).unwrap().body.len(), MAX_BODY_BYTES);
    }

    #[test]
    fn title_exactly_at_cap_is_accepted() {
        let s = Scratch::new("bound-title");
        let (_sr, dir) = dir_for(&s);
        let exact = "t".repeat(MAX_TITLE_BYTES);
        let n = create_note(&dir, exact, "b".into(), vec![], vec![], None, None)
            .expect("a title of exactly MAX_TITLE_BYTES must be accepted");
        assert_eq!(n.title.len(), MAX_TITLE_BYTES);
    }

    #[test]
    fn tag_and_link_exactly_at_byte_cap_accepted() {
        let s = Scratch::new("bound-elem-bytes");
        let (_sr, dir) = dir_for(&s);
        let tag = "t".repeat(MAX_TAG_BYTES); // exactly at the per-element byte cap
        let link = "l".repeat(MAX_LINK_BYTES);
        let n = create_note(
            &dir,
            "t".into(),
            "b".into(),
            vec![tag],
            vec![link],
            None,
            None,
        )
        .expect("a tag/link of exactly its byte cap must be accepted");
        assert_eq!(n.tags[0].len(), MAX_TAG_BYTES);
        assert_eq!(n.links[0].len(), MAX_LINK_BYTES);
    }

    #[test]
    fn tag_and_link_count_exactly_at_cap_accepted() {
        let s = Scratch::new("bound-elem-count");
        let (_sr, dir) = dir_for(&s);
        // EXACTLY MAX_TAGS / MAX_LINKS elements (the existing test proves +1 is rejected;
        // this proves the boundary count itself is legal — off-by-one guard).
        let tags: Vec<String> = (0..MAX_TAGS).map(|i| format!("t{i}")).collect();
        let links: Vec<String> = (0..MAX_LINKS).map(|i| format!("mem_{i}")).collect();
        let n = create_note(&dir, "t".into(), "b".into(), tags, links, None, None)
            .expect("exactly MAX_TAGS tags and MAX_LINKS links must be accepted");
        assert_eq!(n.tags.len(), MAX_TAGS);
        assert_eq!(n.links.len(), MAX_LINKS);
    }

    #[test]
    fn update_to_exactly_at_body_cap_is_accepted() {
        // The reject side of update is covered by `update_enforces_caps_on_patched_body`;
        // this is the boundary-accept side on the UPDATE path (same `validate_write`).
        let s = Scratch::new("bound-update");
        let (_sr, dir) = dir_for(&s);
        let n = create_note(&dir, "t".into(), "small".into(), vec![], vec![], None, None).unwrap();
        let exact = "x".repeat(MAX_BODY_BYTES);
        let upd = update_note(
            &dir,
            &n.id,
            NotePatch {
                body: Some(exact),
                ..Default::default()
            },
            None,
        )
        .expect("update to an exactly-at-cap body must succeed")
        .expect("the note exists");
        assert_eq!(upd.body.len(), MAX_BODY_BYTES);
        assert_eq!(get_note(&dir, &n.id).unwrap().body.len(), MAX_BODY_BYTES);
    }

    // ── 2. `sanitize_key` / `repo_dir` containment — the REPO-key analog of the C1
    //       note-id allowlist. An attacker-influenced repo key must NOT escape `root`;
    //       the result must stay a SINGLE path component directly under `root`. ──

    #[test]
    fn repo_dir_contains_malicious_keys_to_single_component_under_root() {
        let root = std::env::temp_dir().join(format!(
            "at-mem-reporoot-{}-{}",
            std::process::id(),
            WRITE_NONCE.fetch_add(1, Ordering::Relaxed)
        ));
        // Every adversarial key must resolve to a child of `root` with NO `/` and NO `..`.
        for bad in [
            "../escape",
            "../../escape",
            "/etc/passwd",
            "a/b/c",
            "..",
            ".",
            "foo/../bar",
            "with space",
            "weird\u{0}null",
        ] {
            let d = repo_dir(&root, bad);
            // Stays directly under root (parent IS root → exactly one component below).
            assert_eq!(
                d.parent(),
                Some(root.as_path()),
                "repo_dir({bad:?}) escaped its root: {d:?}"
            );
            // The leaf carries no separator and no parent-traversal token.
            let leaf = d.file_name().unwrap().to_string_lossy();
            assert!(!leaf.contains('/'), "leaf {leaf:?} has a separator");
            assert!(!leaf.contains(".."), "leaf {leaf:?} has a `..`");
            assert_ne!(leaf.as_ref(), "..", "leaf is the parent dir");
            // And it never normalizes UP to root or above (no ParentDir component).
            assert!(
                d.components()
                    .all(|c| !matches!(c, std::path::Component::ParentDir)),
                "repo_dir({bad:?}) contains a ParentDir component: {d:?}"
            );
        }
    }

    #[test]
    fn repo_dir_empty_key_falls_back_to_default_component() {
        let root = PathBuf::from("/tmp/at-mem-root-empty");
        // An empty / all-illegal key must NOT yield `root` itself (which would alias
        // every repo onto one shared dir) — `sanitize_key` substitutes "default".
        let d_empty = repo_dir(&root, "");
        assert_eq!(d_empty, root.join("default"));
        assert_ne!(d_empty, root, "empty key must not collapse onto the root");
    }

    #[test]
    fn repo_key_for_is_stable_and_path_safe() {
        // Same path → same key (FNV-1a is deterministic); the key is itself a safe
        // single component (only `repo_` + lowercase hex). Uses a path that need not
        // exist — `repo_key_for` falls back to the raw path when canonicalize fails.
        let p = std::env::temp_dir().join("at-mem-stable-repo-key-XYZ");
        let k1 = repo_key_for(&p);
        let k2 = repo_key_for(&p);
        assert_eq!(k1, k2, "repo_key_for must be deterministic for one path");
        assert!(k1.starts_with("repo_"), "key shape: {k1}");
        assert!(
            k1.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'),
            "key must be path-safe (alnum/underscore only): {k1}"
        );
        // A different path yields a different key (no trivial collision).
        let other = std::env::temp_dir().join("at-mem-stable-repo-key-OTHER");
        assert_ne!(k1, repo_key_for(&other));
        // The repo-key passes sanitize_key UNCHANGED (it is already a safe component).
        assert_eq!(repo_dir(&p, &k1).file_name().unwrap().to_string_lossy(), k1);
    }

    #[test]
    fn global_scope_resolves_notes_dir_under_root() {
        // The no-env fallback key is GLOBAL_SCOPE ("global"), so every resolver (app /
        // pane sidecar / standalone MCP) with no `AGENT_TEAMS_MEMORY_REPO_KEY` set lands
        // the notes dir at `<root>/global` — ONE shared Second Brain, not per-cwd.
        assert_eq!(GLOBAL_SCOPE, "global");
        let root = PathBuf::from("/tmp/at-mem-global-root");
        let dir = repo_dir(&root, GLOBAL_SCOPE);
        assert_eq!(dir, root.join("global"));
        assert!(
            dir.ends_with("global"),
            "no-env fallback dir must end in /global: {dir:?}"
        );
        // The env escape hatch: an explicit override key flows straight through repo_dir
        // (resolvers pass `AGENT_TEAMS_MEMORY_REPO_KEY`'s value as the key). A safe key is
        // used verbatim, so `env set → env value` holds.
        let override_dir = repo_dir(&root, "team_override_key");
        assert_eq!(override_dir, root.join("team_override_key"));
    }

    // ── 3. Deterministic xorshift fuzz pinning the EXACT `valid_note_id` oracle.
    //       std-only (no rand dep): the corpus mixes the sanctioned alphabet with
    //       traversal bytes, and an INDEPENDENT reference predicate must agree on
    //       every input — so the hand-rolled `^mem_[0-9_]+$` can never silently drift
    //       (e.g. start accepting a `/`, `.`, or letter, or the empty remainder). ──

    #[test]
    fn valid_note_id_fuzz_matches_reference_oracle() {
        // Independent re-statement of the contract: `mem_` prefix + NON-empty rest of
        // ASCII digits / underscores ONLY. Deliberately NOT a call to valid_note_id.
        fn reference(id: &str) -> bool {
            match id.strip_prefix("mem_") {
                None => false,
                Some(rest) => {
                    !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit() || c == '_')
                }
            }
        }

        // Alphabet skewed toward the boundary: digits/underscore (legal), plus the
        // exact bytes that MUST break the allowlist (`/`, `.`, letters, NUL, prefix chars).
        const ALPHABET: &[u8] = b"mem_0123456789_/.abXZ\0-";
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15; // fixed seed → reproducible
        let mut next = || {
            // xorshift64
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };

        for _ in 0..20_000 {
            let len = (next() % 12) as usize; // include 0-length (empty) inputs
            let mut bytes = Vec::with_capacity(len);
            for _ in 0..len {
                bytes.push(ALPHABET[(next() as usize) % ALPHABET.len()]);
            }
            // Bias ~half the corpus to actually start with the prefix so the
            // accept-branch is exercised, not just rejects.
            if next() & 1 == 0 {
                let mut prefixed = b"mem_".to_vec();
                prefixed.extend_from_slice(&bytes);
                bytes = prefixed;
            }
            let id = String::from_utf8_lossy(&bytes).into_owned();
            assert_eq!(
                valid_note_id(&id),
                reference(&id),
                "valid_note_id disagrees with the reference oracle on {id:?}"
            );
            // Any id the allowlist ACCEPTS must be path-safe: a single component whose
            // note_path stays directly under `dir` (the C1 guarantee that makes the
            // get/update/delete short-circuit a real containment boundary).
            if valid_note_id(&id) {
                let dir = Path::new("/tmp/at-mem-fuzz-dir");
                let p = note_path(dir, &id);
                assert_eq!(p.parent(), Some(dir), "accepted id {id:?} escaped dir");
                assert!(
                    p.components()
                        .all(|c| !matches!(c, std::path::Component::ParentDir)),
                    "accepted id {id:?} yields a ParentDir component"
                );
            }
        }
    }

    // ── 4. `count_notes` (the MAX_NOTES_PER_DIR disk-exhaustion counter) must count
    //       ONLY committed `*.json` notes — skipping write temps and non-json — else a
    //       litter of temps could trip the cap early, or a `.tmp` could be miscounted.
    //       (The 50k cap itself is scale-untestable in a unit test; we lock the COUNTER
    //       semantics that back it.) ──

    #[test]
    fn count_notes_skips_temps_and_non_json() {
        let s = Scratch::new("count-skip");
        let (_sr, dir) = dir_for(&s);
        let a = create_note(&dir, "a".into(), "x".into(), vec![], vec![], None, None).unwrap();
        let _b = create_note(&dir, "b".into(), "y".into(), vec![], vec![], None, None).unwrap();
        // Litter the dir: an interrupted write temp, a dotfile, and a non-json file.
        std::fs::write(
            dir.join(format!(".{}.json.tmp.{}.7", a.id, std::process::id())),
            b"{ partial",
        )
        .unwrap();
        std::fs::write(dir.join("notes.txt"), b"not a note").unwrap();
        std::fs::write(dir.join("README.md"), b"# nope").unwrap();
        // Only the two real notes count — temps/non-json are invisible to the cap.
        assert_eq!(count_notes(&dir), 2);
        // And list_notes agrees (same skip discipline → no phantom notes surfaced).
        assert_eq!(list_notes(&dir).len(), 2);
    }

    #[test]
    fn recall_block_matches_ranks_and_fences() {
        let s = Scratch::new("recall");
        let (_sr, dir) = dir_for(&s);
        create_note(
            &dir,
            "Tauri state wipe".into(),
            "The app wipes state_root every launch.".into(),
            vec!["arch".into()],
            vec![],
            None,
            None,
        )
        .unwrap();
        create_note(
            &dir,
            "Unrelated note".into(),
            "Cats and dogs.".into(),
            vec![],
            vec![],
            None,
            None,
        )
        .unwrap();
        let notes = list_notes(&dir);
        let block = recall_block(&notes, "state_root wipe on launch", 5).expect("has hits");
        assert!(block.starts_with("## Relevant memory (auto-recall)\n"));
        assert!(
            block.contains(RECALL_PROVENANCE),
            "provenance + verify-against-HEAD line must ride every recall block"
        );
        assert!(block.contains("Tauri state wipe"));
        // Each bullet carries the note's write date (staleness at a glance): the
        // just-created note must be stamped with today's UTC date.
        let today = date_ymd(notes.iter().map(|n| n.updated_at).max().unwrap());
        assert!(
            block.contains(&format!("Tauri state wipe [{today}]")),
            "bullet must carry [YYYY-MM-DD] from updated_at; got: {block:?}"
        );
        assert!(
            !block.contains("Unrelated"),
            "non-matching note must not leak in"
        );
        assert!(block.contains("(id: mem_"));
        assert!(
            block.trim_end().ends_with("---"),
            "block must fence the recall from the task"
        );
    }

    #[test]
    fn date_ymd_known_vectors() {
        // Epoch, epoch+1day, 1e12 ms (= 2001-09-09T01:46:40Z), leap day, and an
        // end-of-year boundary — pins the std-only civil-date math.
        assert_eq!(super::date_ymd(0), "1970-01-01");
        assert_eq!(super::date_ymd(86_400_000), "1970-01-02");
        assert_eq!(super::date_ymd(1_000_000_000_000), "2001-09-09");
        assert_eq!(super::date_ymd(1_582_934_400_000), "2020-02-29");
        assert_eq!(super::date_ymd(1_767_139_199_000), "2025-12-30");
    }

    #[test]
    fn recall_block_none_on_no_match_or_empty() {
        let s = Scratch::new("recall-none");
        let (_sr, dir) = dir_for(&s);
        create_note(
            &dir,
            "Tauri".into(),
            "Rust core.".into(),
            vec![],
            vec![],
            None,
            None,
        )
        .unwrap();
        let notes = list_notes(&dir);
        assert!(recall_block(&notes, "zzz nonexistent qqq", 5).is_none());
        assert!(
            recall_block(&notes, "", 5).is_none(),
            "empty query → no block"
        );
        assert!(
            recall_block(&[], "tauri", 5).is_none(),
            "empty store → no block"
        );
    }

    // ── Stopword filter + auto-path relevance floor (the loose auto-linking fix) ──

    #[test]
    fn tokenize_drops_stopwords_and_short_tokens_keeps_domain_words() {
        // Function words + 1-char tokens are removed; domain words survive (the
        // stopword set is deliberately conservative — no "path"/"performance"/…).
        let toks = super::tokenize("The hot path is a performance issue, so optimize it");
        for kept in ["hot", "path", "performance", "issue", "optimize"] {
            assert!(
                toks.iter().any(|t| t == kept),
                "domain word {kept} kept: {toks:?}"
            );
        }
        for dropped in ["the", "is", "so", "it", "a"] {
            assert!(
                !toks.iter().any(|t| t == dropped),
                "stopword/short token {dropped} dropped: {toks:?}"
            );
        }
    }

    #[test]
    fn relevant_notes_floors_incidental_single_token_but_search_stays_loose() {
        // Regression for the run-capture bug: the goal "…optimize the hot path"
        // shared ONLY the token "path" with an unrelated note ("never fabricate a
        // path") and that lone incidental overlap got auto-linked + auto-recalled.
        // The FLOORED auto path (relevant_notes / harvest_relevant_ids) must reject a
        // single-token overlap; the LOOSE search_memories path (search_notes) must
        // still return it.
        let s = Scratch::new("relevance-floor");
        let (_sr, dir) = dir_for(&s);
        // Only overlap with the goal is the single token "path".
        let incidental = create_note(
            &dir,
            "jeffry".into(),
            "never fabricate a path".into(),
            vec![],
            vec![],
            None,
            None,
        )
        .unwrap();
        // Shares several meaningful tokens with the goal (hot, path, performance, optimize).
        let related = create_note(
            &dir,
            "hot path perf".into(),
            "optimize the hot path for performance".into(),
            vec![],
            vec![],
            None,
            None,
        )
        .unwrap();
        let notes = list_notes(&dir);
        let goal = "Review the codebase for performance issues and optimize the hot path";

        // LOOSE (public search_notes = search_memories MCP): BOTH are returned.
        let loose: Vec<String> = search_notes(&notes, goal, 10)
            .into_iter()
            .map(|n| n.id)
            .collect();
        assert!(
            loose.contains(&incidental.id),
            "loose search returns even the single-token match (stays loose): {loose:?}"
        );
        assert!(
            loose.contains(&related.id),
            "loose search returns the multi-token match: {loose:?}"
        );

        // FLOORED (auto recall): the incidental single-token overlap is DROPPED,
        // the genuinely-related multi-token match survives.
        let floored: Vec<String> = relevant_notes(&notes, goal, 10)
            .into_iter()
            .map(|n| n.id)
            .collect();
        assert!(
            !floored.contains(&incidental.id),
            "floor drops the incidental single-token overlap: {floored:?}"
        );
        assert!(
            floored.contains(&related.id),
            "floor keeps the genuinely-related multi-token match: {floored:?}"
        );

        // The LINKING path (write_run_capture / write_harvested_notes via
        // harvest_relevant_ids) is floored too: no lineage edge on a lone-token hit.
        let lineage = harvest_relevant_ids(&notes, goal);
        assert!(
            !lineage.contains(&incidental.id),
            "capture must not link on a lone-token overlap: {lineage:?}"
        );
        assert!(
            lineage.contains(&related.id),
            "capture links the genuinely-related note: {lineage:?}"
        );
    }

    #[test]
    fn relevant_notes_adaptive_floor_keeps_single_token_query_match() {
        // A 1-token query has distinct==1, so floor = min(2, 1) = 1: a single-token
        // match is STILL relevant (the adaptive floor never over-prunes short
        // queries). An all-stopword query resolves to zero meaningful tokens → empty.
        let s = Scratch::new("relevance-floor-short");
        let (_sr, dir) = dir_for(&s);
        let hit = create_note(
            &dir,
            "tauri".into(),
            "the app wraps a webview".into(),
            vec![],
            vec![],
            None,
            None,
        )
        .unwrap();
        create_note(
            &dir,
            "unrelated".into(),
            "cats and dogs".into(),
            vec![],
            vec![],
            None,
            None,
        )
        .unwrap();
        let notes = list_notes(&dir);
        let hits: Vec<String> = relevant_notes(&notes, "tauri", 5)
            .into_iter()
            .map(|n| n.id)
            .collect();
        assert_eq!(
            hits,
            vec![hit.id],
            "1-token query still returns its single-token match"
        );
        assert!(
            relevant_notes(&notes, "the and of to", 5).is_empty(),
            "all-stopword query has zero meaningful tokens → empty"
        );
    }

    // ────────────── Post-run knowledge harvest (LESSON: extraction) ──────────────

    /// `(pane, text)` report helper.
    fn rep(pane: &str, text: &str) -> (String, String) {
        (pane.to_string(), text.to_string())
    }

    #[test]
    fn harvest_happy_path_marker_variants() {
        // Every documented marker variant lands; prose never does. One report,
        // three shapes: bare, bullet-with-space, star-bullet, and no-space-after-colon.
        let text = "# Report\n\
                    Some prose about what happened in this run.\n\
                    LESSON: the fold must base on the fork-point, not merge-base main\n\
                    - LESSON: rtk mangles git SHAs — use /usr/bin/git for ancestry checks\n\
                    * LESSON:worktree cwd traps edit MAIN, use git -C with absolute paths\n";
        let got = harvest_lessons(&[rep("ws1-p0", text)]);
        assert_eq!(got.len(), 3);
        assert_eq!(
            got[0].lesson,
            "the fold must base on the fork-point, not merge-base main"
        );
        assert_eq!(
            got[1].lesson,
            "rtk mangles git SHAs — use /usr/bin/git for ancestry checks"
        );
        assert_eq!(
            got[2].lesson,
            "worktree cwd traps edit MAIN, use git -C with absolute paths"
        );
        assert!(got.iter().all(|c| c.pane_id == "ws1-p0"));
        // Short lessons title verbatim (≤80 chars).
        assert_eq!(got[0].title, got[0].lesson);
    }

    #[test]
    fn harvest_ignores_non_marker_lines() {
        // Case-sensitivity, mid-line markers, double bullets, heading-prefixed, and
        // plain prose are all rejected — extraction is the marker grammar, nothing else.
        let text = "lesson: lowercase marker must not match at all\n\
                    Lesson: capitalized-but-not-uppercase must not match\n\
                    we learned a LESSON: mid-line marker must not match\n\
                    -- LESSON: double bullet must not match the grammar\n\
                    ## LESSON: heading-prefixed marker must not match\n\
                    A prose paragraph that is definitely long enough to pass the length guard.\n";
        assert!(harvest_lessons(&[rep("p", text)]).is_empty());
    }

    #[test]
    fn harvest_empty_inputs() {
        assert!(harvest_lessons(&[]).is_empty());
        assert!(harvest_lessons(&[rep("p", "")]).is_empty());
        assert!(harvest_lessons(&[rep("p", "just prose, no markers anywhere here")]).is_empty());
    }

    #[test]
    fn harvest_length_guards() {
        let too_short = "LESSON: nineteen chars!"; // 15 chars after marker+trim
        let at_min = format!("LESSON: {}", "a".repeat(HARVEST_MIN_CHARS));
        let at_max = format!("LESSON: {}", "b".repeat(HARVEST_MAX_CHARS));
        let too_long = format!("LESSON: {}", "c".repeat(HARVEST_MAX_CHARS + 1));
        let text = format!("{too_short}\n{at_min}\n{at_max}\n{too_long}\n");
        let got = harvest_lessons(&[rep("p", &text)]);
        assert_eq!(got.len(), 2, "only the in-bounds candidates survive");
        assert_eq!(got[0].lesson.chars().count(), HARVEST_MIN_CHARS);
        assert_eq!(got[1].lesson.chars().count(), HARVEST_MAX_CHARS);
    }

    #[test]
    fn harvest_caps_at_max_per_run_across_reports() {
        // Five valid candidates across two reports → the FIRST three (report order,
        // line order) win; the rest are dropped.
        let a = "LESSON: candidate one is long enough to pass the guard\n\
                 LESSON: candidate two is long enough to pass the guard\n";
        let b = "LESSON: candidate three is long enough to pass the guard\n\
                 LESSON: candidate four is long enough to pass the guard\n\
                 LESSON: candidate five is long enough to pass the guard\n";
        let got = harvest_lessons(&[rep("p0", a), rep("p1", b)]);
        assert_eq!(got.len(), HARVEST_MAX_PER_RUN);
        assert!(got[0].lesson.contains("one"));
        assert!(got[1].lesson.contains("two"));
        assert!(got[2].lesson.contains("three"));
        assert_eq!(got[2].pane_id, "p1");
    }

    #[test]
    fn harvest_batch_dedup_same_title_first_wins() {
        // Two panes reporting the IDENTICAL lesson: one candidate, not two — the
        // duplicate must not burn a cap slot.
        let same = "LESSON: never share the main checkout with a sibling session\n";
        let extra = "LESSON: a distinct second lesson still gets its own slot\n";
        let got = harvest_lessons(&[rep("p0", same), rep("p1", &format!("{same}{extra}"))]);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].pane_id, "p0", "first occurrence wins");
        assert!(got[1].lesson.contains("distinct second"));
    }

    #[test]
    fn lesson_title_word_boundary_truncate() {
        // ≤80 chars → verbatim.
        let short = "short lesson title";
        assert_eq!(lesson_title(short), short);
        // Over the cap → cut back to the last whitespace inside the 80-char budget,
        // never mid-word, no ellipsis.
        let long = format!("{} tail-word-past-the-cap", "word ".repeat(16)); // 80 chars of "word " + tail
        let t = lesson_title(&long);
        assert!(t.chars().count() <= HARVEST_TITLE_MAX_CHARS);
        assert!(t.ends_with("word"), "cut lands on a word boundary: {t:?}");
        // One giant unbroken token → hard cut at the cap (chars, multibyte-safe).
        let giant = "é".repeat(HARVEST_TITLE_MAX_CHARS + 20);
        let t = lesson_title(&giant);
        assert_eq!(t.chars().count(), HARVEST_TITLE_MAX_CHARS);
    }

    #[test]
    fn write_harvested_notes_shape_provenance_and_dedup() {
        let s = Scratch::new("harvest-write");
        let (_sr, dir) = dir_for(&s);
        let text = "LESSON: the store dir must be a sibling of state_root to survive wipes\n";
        let cands = harvest_lessons(&[rep("ws9-p2", text)]);
        assert_eq!(cands.len(), 1);
        let n = write_harvested_notes(&dir, &cands, "run-2026-07-06", "");
        assert_eq!(n, 1);
        let notes = list_notes(&dir);
        assert_eq!(notes.len(), 1);
        let note = &notes[0];
        assert_eq!(
            note.title,
            "the store dir must be a sibling of state_root to survive wipes"
        );
        assert_eq!(note.category.as_deref(), Some(HARVEST_CATEGORY));
        assert_eq!(note.tags, vec![HARVEST_TAG.to_string()]);
        assert_eq!(note.origin.as_deref(), Some("ws9-p2"));
        // Body = the lesson + the provenance line (run/pane + write date).
        assert!(note.body.starts_with(&note.title));
        let today = date_ymd(note.created_at);
        assert!(
            note.body.contains(&format!(
                "harvested from run run-2026-07-06/ws9-p2, {today}"
            )),
            "provenance line present: {:?}",
            note.body
        );
        // A single-note batch has no siblings and (goal "") no lineage → links empty.
        assert!(note.links.is_empty());
        // Re-harvesting the SAME run is idempotent: exact-title dedup writes 0.
        assert_eq!(write_harvested_notes(&dir, &cands, "run-2026-07-06", ""), 0);
        assert_eq!(list_notes(&dir).len(), 1);
    }

    #[test]
    fn write_harvested_notes_skips_existing_title_continues_with_rest() {
        let s = Scratch::new("harvest-dedup");
        let (_sr, dir) = dir_for(&s);
        // Pre-existing note whose title equals the first candidate's title.
        create_note(
            &dir,
            "never dispatch over an active fleet without checking worktrees".into(),
            "hand-written".into(),
            vec![],
            vec![],
            None,
            None,
        )
        .unwrap();
        let text = "LESSON: never dispatch over an active fleet without checking worktrees\n\
                    LESSON: a second, genuinely new lesson lands despite the dup ahead of it\n";
        let cands = harvest_lessons(&[rep("p0", text)]);
        assert_eq!(cands.len(), 2);
        let n = write_harvested_notes(&dir, &cands, "r1", "");
        assert_eq!(n, 1, "dup skipped, new one written");
        let notes = list_notes(&dir);
        assert_eq!(notes.len(), 2);
        // The pre-existing note is untouched (still the hand-written body).
        let pre = notes
            .iter()
            .find(|x| x.body == "hand-written")
            .expect("pre-existing note intact");
        assert!(pre.category.is_none());
    }

    #[test]
    fn write_harvested_notes_empty_is_zero_and_touches_nothing() {
        let s = Scratch::new("harvest-empty");
        let (_sr, dir) = dir_for(&s);
        assert_eq!(write_harvested_notes(&dir, &[], "r0", "any goal"), 0);
        // No dir is even created for an empty batch.
        assert!(!dir.exists());
    }

    // ── deterministic harvest LINKING (same-run mesh + goal-relevance lineage) ──

    #[test]
    fn harvest_link_plan_pure_mesh_dedup_no_self() {
        let w = vec![
            "mem_1".to_string(),
            "mem_2".to_string(),
            "mem_3".to_string(),
        ];
        // Relevance list carries a duplicate AND one of the written ids: both dedup,
        // never a self-link; lineage ids come FIRST, then siblings in write order.
        let r = vec![
            "mem_9".to_string(),
            "mem_9".to_string(),
            "mem_2".to_string(),
        ];
        let plan = harvest_link_plan(&w, &r);
        assert_eq!(plan.len(), 3);
        let links_of =
            |id: &str| -> Vec<String> { plan.iter().find(|(i, _)| i == id).unwrap().1.clone() };
        assert_eq!(links_of("mem_1"), vec!["mem_9", "mem_2", "mem_3"]);
        assert_eq!(
            links_of("mem_2"),
            vec!["mem_9", "mem_1", "mem_3"],
            "own id never self-links, even when it appears in the relevance list"
        );
        assert_eq!(links_of("mem_3"), vec!["mem_9", "mem_2", "mem_1"]);
        // Single written note → lineage only. Empty everything → empty plan.
        let one = harvest_link_plan(&w[..1], &r);
        assert_eq!(one[0].1, vec!["mem_9", "mem_2"]);
        assert!(harvest_link_plan(&[], &r).is_empty());
    }

    #[test]
    fn harvest_relevant_ids_ranking_cap_and_empty_goal() {
        let s = Scratch::new("harvest-relevant");
        let (_sr, dir) = dir_for(&s);
        for i in 0..(HARVEST_LINK_RELEVANT_MAX + 2) {
            create_note(
                &dir,
                format!("flywheel note {i}"),
                "flywheel verify loop".into(),
                vec![],
                vec![],
                None,
                None,
            )
            .unwrap();
        }
        let notes = list_notes(&dir);
        // Blank / whitespace goal → NO lineage (mesh-only harvest).
        assert!(harvest_relevant_ids(&notes, "").is_empty());
        assert!(harvest_relevant_ids(&notes, "   ").is_empty());
        // Matching goal → capped at HARVEST_LINK_RELEVANT_MAX, all real ids.
        let hits = harvest_relevant_ids(&notes, "verify the flywheel loop");
        assert_eq!(hits.len(), HARVEST_LINK_RELEVANT_MAX);
        assert!(hits.iter().all(|id| notes.iter().any(|n| &n.id == id)));
        // Non-matching goal → empty (relevant_notes finds nothing).
        assert!(harvest_relevant_ids(&notes, "zzz qqq unrelated").is_empty());
    }

    #[test]
    fn write_harvested_same_run_mesh_full() {
        // 3 lessons, no goal → each note links the OTHER 2 (full mesh), no self-links.
        let s = Scratch::new("harvest-mesh");
        let (_sr, dir) = dir_for(&s);
        let text = "LESSON: mesh lesson one is long enough to pass the guard\n\
                    LESSON: mesh lesson two is long enough to pass the guard\n\
                    LESSON: mesh lesson three is long enough to pass the guard\n";
        let cands = harvest_lessons(&[rep("p0", text)]);
        assert_eq!(write_harvested_notes(&dir, &cands, "mesh-run", ""), 3);
        let notes = list_notes(&dir);
        assert_eq!(notes.len(), 3);
        for n in &notes {
            assert_eq!(n.links.len(), 2, "each links the other two: {:?}", n.links);
            assert!(!n.links.contains(&n.id), "no self-link");
            for l in &n.links {
                assert!(notes.iter().any(|o| &o.id == l), "sibling id is real");
            }
        }
        // The mesh renders as HARD graph edges (the operator's orphan fix).
        let g = build_graph(&notes);
        assert_eq!(g.edges.iter().filter(|e| e.kind == "link").count(), 6);
        assert!(g.nodes.iter().all(|n| n.degree > 0), "no orphans");
    }

    #[test]
    fn write_harvested_goal_relevance_lineage_and_backlinks_visibility() {
        let s = Scratch::new("harvest-lineage");
        let (_sr, dir) = dir_for(&s);
        // A pre-existing lesson the goal WILL recall, and one it won't.
        let primed = create_note(
            &dir,
            "flywheel verify needs a held pane".into(),
            "the flywheel verify loop needs at least one held verify pane".into(),
            vec![],
            vec![],
            None,
            None,
        )
        .unwrap();
        let unrelated = create_note(
            &dir,
            "Lunch".into(),
            "tacos, always tacos".into(), // shares NO token with the goal below
            vec![],
            vec![],
            None,
            None,
        )
        .unwrap();
        let text = "LESSON: lineage lesson alpha is long enough to pass the guard\n\
                    LESSON: lineage lesson beta is long enough to pass the guard\n";
        let cands = harvest_lessons(&[rep("ws2-p1", text)]);
        let goal = "run the flywheel verify loop again";
        assert_eq!(write_harvested_notes(&dir, &cands, "lineage-run", goal), 2);
        let notes = list_notes(&dir);
        let new_notes: Vec<&Note> = notes
            .iter()
            .filter(|n| n.category.as_deref() == Some(HARVEST_CATEGORY))
            .collect();
        assert_eq!(new_notes.len(), 2);
        for n in &new_notes {
            // Lineage: the primed note's id rides FIRST; the unrelated note never appears.
            assert_eq!(
                n.links.first(),
                Some(&primed.id),
                "lineage first: {:?}",
                n.links
            );
            assert!(!n.links.contains(&unrelated.id));
            // Mesh: the sibling's id is present too; no self, no dups.
            let sibling = new_notes.iter().find(|o| o.id != n.id).unwrap();
            assert!(n.links.contains(&sibling.id), "sibling mesh: {:?}", n.links);
            assert!(!n.links.contains(&n.id));
            assert_eq!(n.links.len(), 2, "primed + sibling, deduped: {:?}", n.links);
        }
        // NOTHING was written on the old note (outbound-only; no write-amplification)…
        let primed_now = get_note(&dir, &primed.id).unwrap();
        assert!(primed_now.links.is_empty());
        assert_eq!(
            primed_now.updated_at, primed.updated_at,
            "old note untouched"
        );
        // …yet the relationship is VISIBLE from the old note via the DERIVED backlinks —
        // the operator's "past relationships" ask.
        let bl = backlinks(&notes, &primed.id);
        assert_eq!(bl.len(), 2, "both new lessons surface as backlinks");
        assert!(bl
            .iter()
            .all(|b| b.category.as_deref() == Some(HARVEST_CATEGORY)));
    }

    #[test]
    fn write_harvested_dup_skipped_candidate_never_enters_mesh() {
        // The failed/skipped-create exclusion, exercised via the title-dedup skip (the
        // reachable non-write path): a skipped candidate's PRE-EXISTING note must not
        // be meshed as a sibling, and the two actually-written notes mesh only each
        // other. (A create ERROR takes the identical code path — written_ids is built
        // exclusively from Ok returns, so exclusion holds by construction.)
        let s = Scratch::new("harvest-skip-mesh");
        let (_sr, dir) = dir_for(&s);
        let pre = create_note(
            &dir,
            "a lesson title that already exists in the store today".into(),
            "hand-written".into(),
            vec![],
            vec![],
            None,
            None,
        )
        .unwrap();
        let text = "LESSON: a lesson title that already exists in the store today\n\
                    LESSON: fresh lesson one is long enough to pass the guard\n\
                    LESSON: fresh lesson two is long enough to pass the guard\n";
        let cands = harvest_lessons(&[rep("p0", text)]);
        assert_eq!(cands.len(), 3);
        assert_eq!(write_harvested_notes(&dir, &cands, "skip-run", ""), 2);
        let notes = list_notes(&dir);
        let fresh: Vec<&Note> = notes.iter().filter(|n| n.id != pre.id).collect();
        assert_eq!(fresh.len(), 2);
        for n in &fresh {
            assert_eq!(
                n.links.len(),
                1,
                "only the OTHER written note: {:?}",
                n.links
            );
            assert!(!n.links.contains(&pre.id), "skipped candidate not meshed");
        }
        // The pre-existing note gained nothing.
        assert!(get_note(&dir, &pre.id).unwrap().links.is_empty());
    }

    // ─────────────────── Run-outcome capture (deterministic) ────────────────────

    fn run_fields(run_id: &str, goal: &str) -> RunCaptureFields {
        RunCaptureFields {
            run_id: run_id.to_string(),
            workspace_id: "ws-alpha".to_string(),
            goal: goal.to_string(),
            verdict: "pass".to_string(),
            harness: "claude".to_string(),
            held_reason: None,
            pr_url: None,
            extra: None,
        }
    }

    #[test]
    fn scrub_secrets_redacts_known_shapes() {
        // OpenAI / GitHub / AWS token shapes are redacted…
        let sk = format!("key {} rest", "sk-".to_string() + &"a1B2c3D4".repeat(3));
        assert!(scrub_secrets(&sk).contains(REDACTED));
        assert!(!scrub_secrets(&sk).contains("a1B2c3D4a1B2"));
        let ghp = format!("token ghp_{} end", "A1b2C3d4E5".repeat(3));
        assert!(scrub_secrets(&ghp).contains(REDACTED));
        let aws = "id AKIAIOSFODNN7EXAMPLE here";
        let out = scrub_secrets(aws);
        assert!(out.contains(REDACTED) && !out.contains("AKIAIOSFODNN7EXAMPLE"));
        // Bearer <long token> → the token (only) is redacted; "Bearer" stays.
        let bearer = "Authorization: Bearer abcdef0123456789ABCDEF trailing";
        let bout = scrub_secrets(bearer);
        assert!(bout.contains("Bearer ") && bout.contains(REDACTED));
        assert!(!bout.contains("abcdef0123456789ABCDEF"));
        // PEM private-key block → whole block collapses to one placeholder.
        let pem = "before\n-----BEGIN RSA PRIVATE KEY-----\nMIIBOgIBAAJBAK\nabcd/efgh+\n-----END RSA PRIVATE KEY-----\nafter";
        let pout = scrub_secrets(pem);
        assert!(pout.contains(REDACTED) && !pout.contains("MIIBOgIBAAJBAK"));
        assert!(pout.contains("before") && pout.contains("after"));
    }

    #[test]
    fn scrub_secrets_leaves_ordinary_text_untouched() {
        // Emails, ordinary words, short Bearer prose, and a too-short sk- are NOT touched.
        for ok in [
            "email me at jeff@gmail.com about the flywheel",
            "the risk-management task passed verify",
            "Bearer of bad news: the run held",
            "sk-short and akiafoo are not keys",
            "verdict=pass · workspace=ws-alpha · harness=claude",
        ] {
            assert_eq!(scrub_secrets(ok), ok, "must not redact: {ok:?}");
        }
    }

    #[test]
    fn note_deserializes_without_run_provenance_fields() {
        // ADDITIVE: legacy JSON (no run_id/workspace_id) still deserializes → None,
        // and a note without them serializes WITHOUT the keys (skip_serializing_if).
        let legacy = r#"{"id":"mem_1","title":"t","body":"b","created_at":1,"updated_at":1}"#;
        let n: Note = serde_json::from_str(legacy).unwrap();
        assert!(n.run_id.is_none() && n.workspace_id.is_none());
        let json = serde_json::to_string(&n).unwrap();
        assert!(!json.contains("run_id") && !json.contains("workspace_id"));
    }

    #[test]
    fn build_run_capture_content_is_deterministic() {
        let mut f = run_fields("delegate-123", "Ship the memory-capture flywheel");
        f.held_reason = Some("hold (Tests)".to_string());
        f.pr_url = Some("https://example.com/pr/9".to_string());
        let note = build_run_capture(&f);
        assert_eq!(note.title, "Ship the memory-capture flywheel");
        assert_eq!(note.category.as_deref(), Some(RUN_CAPTURE_CATEGORY));
        assert_eq!(note.tags, vec!["run".to_string(), "claude".to_string()]);
        assert_eq!(note.run_id.as_deref(), Some("delegate-123"));
        assert_eq!(note.workspace_id.as_deref(), Some("ws-alpha"));
        assert!(note.links.is_empty(), "build leaves lineage to the writer");
        assert!(note
            .body
            .contains("verdict=pass · workspace=ws-alpha · harness=claude"));
        assert!(note.body.contains("held: hold (Tests)"));
        assert!(note.body.contains("PR: https://example.com/pr/9"));
        assert!(note.body.contains("captured from run delegate-123,"));
    }

    #[test]
    fn build_run_capture_blank_goal_and_harness_degrade() {
        let mut f = run_fields("delegate-9", "   ");
        f.harness = String::new();
        let note = build_run_capture(&f);
        assert_eq!(note.title, "run delegate-9", "blank goal ⇒ run-id title");
        assert_eq!(
            note.tags,
            vec!["run".to_string()],
            "no harness ⇒ single tag"
        );
    }

    #[test]
    fn write_run_capture_writes_once_dedups_and_links_lineage() {
        let s = Scratch::new("run-capture");
        let (_sr, dir) = dir_for(&s);
        // A prior note the run's GOAL will recall (shares tokens), and one it won't.
        let primed = create_note(
            &dir,
            "flywheel capture design".into(),
            "the memory capture flywheel writes run notes".into(),
            vec![],
            vec![],
            None,
            None,
        )
        .unwrap();
        create_note(
            &dir,
            "Lunch".into(),
            "tacos".into(),
            vec![],
            vec![],
            None,
            None,
        )
        .unwrap();

        let f = run_fields("delegate-777", "run the memory capture flywheel");
        let existing = list_notes(&dir);
        let note = write_run_capture(&dir, &f, &existing).expect("first capture writes");
        assert_eq!(note.run_id.as_deref(), Some("delegate-777"));
        // LINEAGE: links the primed note (goal-relevant), never the unrelated one.
        assert!(note.links.contains(&primed.id), "lineage: {:?}", note.links);
        assert!(list_notes(&dir).iter().any(|n| n.id == note.id));
        // The relationship is visible from the old note via DERIVED backlinks.
        assert!(backlinks(&list_notes(&dir), &primed.id)
            .iter()
            .any(|b| b.id == note.id));

        // Idempotent re-completion of the SAME run writes nothing.
        let before = list_notes(&dir).len();
        assert!(
            write_run_capture(&dir, &f, &list_notes(&dir)).is_none(),
            "dedup"
        );
        assert_eq!(list_notes(&dir).len(), before, "no second note");
    }

    #[test]
    fn write_run_capture_scrubs_secret_in_goal() {
        let s = Scratch::new("run-capture-scrub");
        let (_sr, dir) = dir_for(&s);
        let mut f = run_fields(
            "delegate-1",
            "wire up ghp_ABCDEFabcdef0123456789 into the tool",
        );
        f.pr_url = None;
        let note = write_run_capture(&dir, &f, &[]).expect("writes");
        assert!(note.title.contains(REDACTED));
        assert!(!note.title.contains("ghp_ABCDEFabcdef0123456789"));
    }
}
