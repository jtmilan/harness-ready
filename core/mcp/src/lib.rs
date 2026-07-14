//! Agent Teams — core queue projection (MCP Phase A).
//!
//! A **read-only**, serde-able view of the cross-harness state adapter, sized to
//! be the single home for the wire `QueueRow` (PRD §14 / `.paul/analysis/
//! context-router-mcp.md`). The MCP sidecar (`agent-teams-mcp`) and — in a later
//! phase — the Tauri app's `list_queue` are meant to be thin callers of the
//! functions here.
//!
//! ## What this crate is, and what it deliberately is *not*
//!
//! * **Ranking stays single-source.** [`compute_queue`] calls
//!   [`state_adapter::watch::current_states`], which calls the one canonical
//!   [`state_adapter::rank`]. There is no second comparator here.
//! * **The projection is corrected here.** The shared [`QueueRow`] emits the
//!   spec wire form `turn_end` / `rate_limit` (matching the `adapter` binary,
//!   `core/state-adapter/src/bin/adapter.rs:151-159`) and carries `since`.
//!   The app's own `compute_queue` (`app/src-tauri/src/lib.rs:359`) still emits
//!   the off-spec `turnend` / `ratelimit` and lacks `since`; this scaffold does
//!   **not** edit that file, so two projections coexist until the future rewire
//!   makes `lib.rs` import from here. See the bridge note for this lane.
//! * **`state-adapter` stays zero-dep.** serde lives here, not there.
//!
//! ## App-down vs app-up reads (FR-7)
//!
//! [`compute_queue`] takes an optional `live` set. The MCP read path runs
//! app-independently off `events.jsonl`, so by default (`live = None`) it returns
//! *every* discovered workspace — an honest superset that may include stale
//! workspaces from prior runs. When a future phase teaches the sidecar the live
//! set (app `workspace.json` + IPC), pass `Some(&live)` to reproduce the in-app
//! queue exactly. This mirrors the app's `compute_queue(state_root, live)` shape
//! so the eventual extraction is a drop-in.

use serde::{Deserialize, Serialize};
use state_adapter::watch::{current_states, discover};
use state_adapter::{AgentState, Harness, State, WaitingReason};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// One row of the ranked "who needs me" queue, in wire form.
///
/// `reason` is `None` (JSON `null`) when there is no waiting reason; otherwise
/// one of `approval` / `question` / `turn_end` / `rate_limit` (the corrected,
/// underscore wire form). `since` is unix-millis when the state began and drives
/// the wait-time tie-break.
///
/// Identity fields (gap #4 — "which pane is the coordinator?"): `role`, `tag`,
/// and `workspace` are OPTIONAL + serde-additive so rows for panes spawned
/// before they existed still parse/serialize (absent ⇒ omitted from the wire).
/// `workspace` is derivable from `id` (the `wsNNNNNxK` prefix) but is emitted
/// explicitly so no client has to re-derive it; `role`/`tag` are joined from the
/// live registry ([`enrich_queue`]) — the app records them at spawn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct QueueRow {
    pub id: String,
    pub harness: String,
    pub state: String,
    pub reason: Option<String>,
    pub needs_human: bool,
    pub since: u64,
    /// The typed persona assigned at spawn (wire form: "coordinator"/"builder"/
    /// "scout"/…), from the live registry. `None` = homogeneous pane / registry
    /// absent / spawned before roles were recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// The create-time tag (external `create_workspace {tag}` → stamped on every
    /// pane of that workspace), from the live registry. `None` = untagged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// The workspace prefix of `id` (`wsNNNNNxK` from `wsNNNNNxK-pN`). Derivable,
    /// but included so an external orchestrator never re-derives it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
}

/// The workspace id embedded in a pane id: `${wsId}-p${idx}` → `${wsId}` (a wsId
/// like `ws76101x0` carries no interior `-p`, so a right-split on `-p` is
/// unambiguous). When the id has no `-p<idx>` tail (an unexpected shape) the
/// WHOLE id is returned so it still groups as its own distinct workspace —
/// never silently merged. Mirrors the app's `ws_prefix_of` (`app/src-tauri`).
pub fn ws_prefix(id: &str) -> &str {
    match id.rsplit_once("-p") {
        Some((ws, _)) => ws,
        None => id,
    }
}

/// `Harness` → wire string (`adapter.rs:134-139`).
fn harness_str(h: Harness) -> &'static str {
    match h {
        Harness::Claude => "claude",
        Harness::Cursor => "cursor",
        Harness::Codex => "codex",
        Harness::CommandCode => "commandcode",
        Harness::OpenCode => "opencode",
        Harness::Cline => "cline",
        Harness::Grok => "grok",
    }
}

/// `State` → wire string (`adapter.rs:141-149`).
fn state_str(s: State) -> &'static str {
    match s {
        State::Idle => "idle",
        State::Working => "working",
        State::Waiting => "waiting",
        State::Done => "done",
        State::Error => "error",
    }
}

/// `WaitingReason` → corrected wire string (`adapter.rs:151-159`). `None` here
/// becomes JSON `null`, not the table's `"-"` placeholder.
fn reason_str(r: Option<WaitingReason>) -> Option<&'static str> {
    match r {
        Some(WaitingReason::Approval) => Some("approval"),
        Some(WaitingReason::Question) => Some("question"),
        Some(WaitingReason::TurnEnd) => Some("turn_end"),
        Some(WaitingReason::RateLimit) => Some("rate_limit"),
        None => None,
    }
}

/// Project one normalized `(id, harness, AgentState)` triple into a [`QueueRow`].
/// `workspace` is stamped here (pure id derivation — every row carries it, even
/// for panes spawned before the identity fields existed); `role`/`tag` start
/// `None` and are joined from the live registry by [`enrich_queue`].
fn project(id: String, harness: Harness, st: AgentState) -> QueueRow {
    let workspace = Some(ws_prefix(&id).to_string());
    QueueRow {
        id,
        harness: harness_str(harness).to_string(),
        state: state_str(st.state).to_string(),
        reason: reason_str(st.waiting_reason).map(str::to_string),
        needs_human: st.needs_human,
        since: st.since,
        role: None,
        tag: None,
        workspace,
    }
}

/// The ranked queue across all workspaces under `state_dir`, in wire form.
///
/// Ranking is the canonical single-source [`state_adapter::rank`] (via
/// [`current_states`]); this only projects + optionally filters. When `live` is
/// `Some`, only workspaces whose id is in the set are kept (app-up exact match);
/// when `None`, every discovered workspace is returned (app-down superset, FR-7).
/// Filtering after ranking is order-preserving — `rank` is stable.
pub fn compute_queue(state_dir: &Path, live: Option<&HashSet<String>>) -> Vec<QueueRow> {
    current_states(&discover(state_dir))
        .into_iter()
        .filter(|(id, _, _)| live.is_none_or(|set| set.contains(id)))
        .map(|(id, h, st)| project(id, h, st))
        .collect()
}

/// Discovered workspace ids under `state_dir`, sorted. Read-only directory scan;
/// does not parse events. Empty when `state_dir` is missing/unreadable.
pub fn list_workspaces(state_dir: &Path) -> Vec<String> {
    let mut ids: Vec<String> = discover(state_dir).into_iter().map(|w| w.id).collect();
    ids.sort();
    ids
}

/// The [`QueueRow`] for a single workspace id, or `None` if it has no current
/// state (missing/empty `events.jsonl`) or is unknown.
pub fn get_workspace(state_dir: &Path, id: &str) -> Option<QueueRow> {
    compute_queue(state_dir, None)
        .into_iter()
        .find(|row| row.id == id)
}

/// Join spawn-time identity onto queue rows from the live registry: for every row
/// whose id has a registry entry, copy that entry's `role` + `tag` (recorded by the
/// app's `do_spawn`). Rows with no entry (app-down superset / panes spawned before
/// role/tag were recorded) keep `None` — the serde-additive contract. `workspace`
/// is untouched here (already stamped by `project` from the id itself).
pub fn enrich_queue(rows: &mut [QueueRow], registry: &LiveRegistry) {
    for row in rows.iter_mut() {
        if let Some(w) = registry.workspaces.iter().find(|w| w.id == row.id) {
            row.role = w.role.clone();
            row.tag = w.tag.clone();
        }
    }
}

/// [`compute_queue`] + identity, in one call — THE row source for the MCP sidecar
/// (gap #4). `registry` present ⇒ app-up: filter to its live set AND join each
/// row's `role`/`tag` from it. `registry` absent ⇒ app-down: the discovered
/// superset (FR-7), rows carrying only the id-derived `workspace`.
pub fn compute_queue_identified(
    state_dir: &Path,
    registry: Option<&LiveRegistry>,
) -> Vec<QueueRow> {
    let live = registry.map(LiveRegistry::live_ids);
    let mut rows = compute_queue(state_dir, live.as_ref());
    if let Some(reg) = registry {
        enrich_queue(&mut rows, reg);
    }
    rows
}

// ───────────────────────────── live registry (Phase A→B seam) ──────────────────
//
// THE WORKSPACE.JSON-AT-SPAWN FORMAT. The future Tauri-app writer
// (`app/src-tauri/src/lib.rs`, NOT edited by this lane) emits/maintains a single
// **live registry** as it spawns/retires workspaces; the sidecar reads it to
// learn the `live` set and pass `Some(&live)` to [`compute_queue`].
//
// **Placement — sibling of `state_root` (per analysis §5 / PRD §14).** Durable
// files live next to `state_root`, not inside it, because the app wipes
// `state_root` on startup (`lib.rs:753`). The path is defined in exactly ONE
// place — [`registry_path`] — which the future writer MUST import so writer and
// reader can never drift. Change the location here and both sides move together.
//
// **Presence is the app-up signal.** Registry present & parses ⇒ the app is (or
// was) running and this is its live set — trust it (even an empty list ⇒ "app up,
// nothing live"). Absent/invalid ⇒ app-down ⇒ caller falls back to the discovered
// superset (FR-7). `app_pid` is carried for a *future* process-liveness refinement
// (deferred to Phase B "liveness over the socket"); Phase A does NOT verify it.

/// Filename of the live registry (sibling of `state_root`).
pub const LIVE_REGISTRY_FILE: &str = "agent-teams-live.json";

/// Current `schema` version of [`LiveRegistry`]. Bump on any breaking shape change.
pub const LIVE_REGISTRY_SCHEMA: u32 = 1;

/// One spawned workspace's entry in the [`LiveRegistry`]. Only `id` is required;
/// the rest are forward-compat metadata a minimal writer may omit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LiveWorkspace {
    /// Workspace id — the directory name under `state_root` (matches `QueueRow.id`).
    pub id: String,
    /// Child harness process id (for the deferred Phase-B process-liveness check).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// `"claude"` | `"cursor"` — the harness, if the writer records it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    /// Absolute repo/worktree path backing this workspace, if recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// The typed persona assigned at spawn, wire form ("coordinator"/"builder"/…),
    /// if the writer records it. This is what lets an external orchestrator answer
    /// "which pane is the coordinator" from the queue rows (gap #4 — joined onto
    /// [`QueueRow::role`] by [`enrich_queue`]). `None` for homogeneous panes /
    /// older writers. Additive (serde-lenient), same pattern as `session_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// The create-time tag (external `create_workspace {tag}`), stamped on every
    /// pane of that workspace so an external brain can find "its" panes without
    /// guessing ids. `None` for untagged / older writers. Additive (serde-lenient).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// The stable claude conversation/session id passed at spawn (`--session-id`/`--resume`),
    /// if recorded. This is the transcript FILENAME (`~/.claude/projects/*/<session_id>.jsonl`),
    /// so a reader can locate the transcript by session id regardless of the launch-cwd encoding
    /// of the project dir (a pane launched at cwd `/` slugs its dir to a bare `-`, which the
    /// dir-suffix match misses). `None` for cursor / older writers. Additive (serde-lenient).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Spawn time (unix millis), if recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawned_at: Option<u64>,
}

/// The live registry the app writes at spawn (sibling of `state_root`). Carries
/// the live workspace set the sidecar filters on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LiveRegistry {
    /// Format version. Readers should tolerate `schema > LIVE_REGISTRY_SCHEMA`
    /// by treating unknown fields leniently (serde already ignores extras).
    pub schema: u32,
    /// PID of the Tauri app that owns this registry. Carried for a future
    /// process-liveness check (Phase B); Phase A does NOT verify it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_pid: Option<u32>,
    /// When the registry was last rewritten (unix millis), if recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<u64>,
    /// The frontend's currently-active workspace prefix (`wsNNNNNxK`), if any. Written
    /// by the app on `setActive`; read by external tooling (GlikaAgents) to name "the
    /// workspace we're on". Serde-additive (older readers ignore it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
    /// The currently-live workspaces.
    #[serde(default)]
    pub workspaces: Vec<LiveWorkspace>,
}

impl LiveRegistry {
    /// The set of live workspace ids — what the sidecar filters [`compute_queue`] by.
    pub fn live_ids(&self) -> HashSet<String> {
        self.workspaces.iter().map(|w| w.id.clone()).collect()
    }
}

/// The live-registry path: `<state_root>/../agent-teams-live.json` (sibling of
/// `state_root`). `None` if `state_root` has no parent. **The single source of
/// truth for this location** — the future app-side writer MUST use this function.
pub fn registry_path(state_root: &Path) -> Option<PathBuf> {
    state_root
        .parent()
        .map(|parent| parent.join(LIVE_REGISTRY_FILE))
}

/// Read + parse the live registry, or `None` if it is absent, unreadable, or
/// malformed. `None` is the app-down signal: the caller should fall back to the
/// discovered superset (`compute_queue(state_dir, None)`).
pub fn read_registry(state_root: &Path) -> Option<LiveRegistry> {
    let path = registry_path(state_root)?;
    let body = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&body).ok()
}

// ───────────────────── Phase-B mutation seam: shared SSOT (06-02) ───────────────
//
// The Unix-domain socket path + wire protocol + config live HERE (beside
// `registry_path`) so the **app-side binder** (`app/src-tauri/src/lib.rs`) and the
// **sidecar dialer** (`agent-teams-mcp`) serialize the EXACT SAME definition and
// can never drift. Promoting these out of the sidecar's hand-defined copy is the
// MUST-DO prerequisite the Phase-B skeleton doc-comments flag. These types are NOT
// behind the `phase-b-mutations` feature: both crates always need the shape (only
// the sidecar's *registered tools* stay feature-gated). No real I/O happens here —
// this is pure path policy + serde + pure decision helpers, all unit-testable.

/// Filename of the Unix-domain mutation socket — a SIBLING of `state_root`,
/// alongside `agent-teams-live.json`. `state_root` is wiped on app startup, so
/// durable IPC files live *beside* it.
pub const SOCKET_FILE: &str = "agent-teams-mcp.sock";

/// Filename of the MCP runtime config — a SIBLING of `state_root` (survives the
/// startup wipe). Holds the capability gate. Absent OR malformed ⇒ SAFE defaults.
pub const MCP_CONFIG_FILE: &str = "mcp-config.json";

/// Filename of the loopback-HTTP Bearer token — a SIBLING of `state_root`. MUST
/// be created `0600` by the app (the at-rest secrecy of this token is the
/// same-user boundary for the HTTP transport, replacing the socket's peer euid).
pub const HTTP_TOKEN_FILE: &str = "agent-teams-mcp-http.token";

/// Filename of the loopback-HTTP port-discovery file — a SIBLING of `state_root`.
/// The app binds an ephemeral port (`127.0.0.1:0`) and writes the chosen port
/// here so a future dialer can find it (mirrors the `.sock` path convention).
pub const HTTP_PORT_FILE: &str = "agent-teams-mcp-http.port";

/// The mutation socket path: `<state_root>/../agent-teams-mcp.sock`. Mirrors
/// [`registry_path`] EXACTLY so the `.sock` and `agent-teams-live.json` co-locate.
/// `None` if `state_root` has no parent.
pub fn socket_path(state_root: &Path) -> Option<PathBuf> {
    state_root.parent().map(|parent| parent.join(SOCKET_FILE))
}

/// The MCP config path: `<state_root>/../mcp-config.json`. `None` if `state_root`
/// has no parent.
pub fn mcp_config_path(state_root: &Path) -> Option<PathBuf> {
    state_root
        .parent()
        .map(|parent| parent.join(MCP_CONFIG_FILE))
}

/// The loopback-HTTP Bearer token path: `<state_root>/../agent-teams-mcp-http.token`
/// (sibling of `state_root`, mirroring [`socket_path`]). `None` if `state_root` has
/// no parent. The app MUST create this file `0600`; its at-rest secrecy is the
/// same-user boundary for the HTTP transport.
pub fn http_token_path(state_root: &Path) -> Option<PathBuf> {
    state_root
        .parent()
        .map(|parent| parent.join(HTTP_TOKEN_FILE))
}

/// The loopback-HTTP port-discovery path: `<state_root>/../agent-teams-mcp-http.port`
/// (sibling of `state_root`, mirroring [`socket_path`]). `None` if `state_root` has
/// no parent.
pub fn http_port_path(state_root: &Path) -> Option<PathBuf> {
    state_root
        .parent()
        .map(|parent| parent.join(HTTP_PORT_FILE))
}

/// One newline-delimited JSON request the sidecar writes to the socket. Tagged by
/// `op` so the on-wire JSON is exactly: `{"op":"send_input","id":..,"text":..}` and
/// `{"op":"focus","id":..}`. One request per line; the app-side handler
/// deserializes, routes, and replies a [`SocketResponse`].
///
/// 06-03 adds the **Context Router** ops on top of the 06-02 mutation seam — they
/// route a GOAL / a broadcast / a handoff through the EXISTING in-app
/// `orchestrate()` / `write_to_pane` / fan-in paths (no second synthesizer). The
/// synthesis-wrapping op (`Orchestrate`) needs a LONG read window on both sides
/// (the headless-claude D43 budget is 120s); see [`op_timeout`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SocketRequest {
    /// Route the human's reply to a live workspace PTY. `text` is exactly one
    /// line; the write path appends the single trailing `\r` (CR — the
    /// Enter-submits-TUI invariant: raw-mode agent TUIs bind Enter to CR, and cooked
    /// shells map CR→LF via ICRNL, so one trailing `\r` submits in BOTH modes).
    /// Interior newlines AND every other control byte are REJECTED by
    /// [`normalize_input`] before any PTY write (a second line could inject a second
    /// TUI submission — the Model-A risk; a control byte could drive the TUI).
    SendInput { id: String, text: String },
    /// Raise / jump to a workspace in the running app.
    Focus { id: String },
    /// Context Router (06-03): run the EXISTING in-app synthesizer (`orchestrate`,
    /// D22) over the live pane set for `goal`. `dispatch:false` (the SAFE default at
    /// the tool boundary) PREVIEWS — returns the `{id,task}` mapping in
    /// [`SocketData::Mapping`] WITHOUT writing to any pane (D23). `dispatch:true`
    /// loops the mapping through the gated `write_to_pane` (each one line + trailing
    /// `\n`, dead panes skipped). A `parse_dispatch` Err ⇒ dispatch NOTHING.
    ///
    /// `target_workspace` (B4) SCOPES the fan-out to ONE workspace. Pane ids embed the
    /// workspace (`${wsId}-p${idx}`), so the app keeps only `${target}-p…` panes. When
    /// ABSENT the app proceeds only if EXACTLY ONE workspace is live; with >1 live it
    /// REFUSES (`BAD_REQUEST`) rather than blasting every workspace. Additive/serde-
    /// lenient: an omitted field deserializes to `None` and is not re-serialized.
    Orchestrate {
        goal: String,
        dispatch: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_workspace: Option<String>,
    },
    /// Context Router (06-03): send `text` to EVERY live pane through the same gate
    /// as `SendInput` (is_alive/D30, one line + `\n`, Model-A — never auto-answers
    /// an approval). Returns [`SocketData::Broadcast`] `{sent, skipped}`.
    Broadcast { text: String },
    /// Context Router (06-03): relay ONE single-line handoff message (provenance
    /// from `from` + the operator's `instruction`) to pane `to` through the gate.
    /// ONE relay hop — NO multi-turn agent-to-agent loop, NO second synthesizer.
    Handoff {
        from: String,
        to: String,
        instruction: String,
    },
    /// Context Router (06-05): fan-in. Synthesize the dispatched panes' reports in run
    /// dir `dir` (its `manifest.json` is the authoritative id set; each `<id>.md` is a
    /// pane's self-report) into ONE consolidated `final.md`, and return its path PLUS
    /// the per-pane `verify_dispatched` verdicts (the machine answer to "did every
    /// dispatched harness produce a usable report"). ADVISORY: the MCP path gathers NO
    /// git ground-truth and runs NO authoritative test (no integration tree in hand), so
    /// the synthesizer marks every claim `[CLAIMED]`. WRAPS headless claude → the LONG
    /// read window (see [`op_timeout`]). `goal` frames the consolidation (may be empty).
    Synthesize { dir: String, goal: String },
    /// Autonomous delegation (MVP spine, depth=1, UDS-only). The app's `socket_delegate`
    /// controller, run on a DETACHED thread so the serial accept loop is never wedged:
    /// bridge_new_run → spawn `max_workers` invisible PTY workers FIRST → orchestrate_sync
    /// (one subtask each) → fan-in (bridge_ready / bridge_synthesize) → write_to_pane ONE
    /// normalized summary line of final.md back to `parent_id`. Workers auto-retire.
    /// `parent_id`+`depth` are stamped into manifest.json (forward-compat recursion guard).
    /// depth>1 is REJECTED in the MVP. Double-gated: `allow_mutations` AND `autonomy_ceiling>=1`.
    Delegate {
        parent_id: String,
        goal: String,
        max_workers: u32,
        depth: u32,
    },

    // ───────────────── role-inversion read ops (08 Sub-build 3, §3.1) ─────────────────
    // All EUID-GATED ONLY (no `allow_mutations` check — MF-D): they expose only
    // same-user PTY scrollback the user could already read by attaching the GUI, and
    // they are the re-attach path that MUST work on a default install. See
    // [`op_requires_mutations`].
    /// Query the daemon's live pane set. Returns immediately (`FAST_OP_TIMEOUT`).
    /// EUID-GATED ONLY (no `allow_mutations` check — MF-D). The relaunched GUI calls
    /// this BEFORE any spawn to discover already-live ids (AC-6 anti-double-spawn).
    /// Reply carries [`SocketData::LivePanes`].
    ListLive,
    /// Upgrade THIS connection to a long-lived output subscription for pane `id`. On
    /// success the server sends a snapshot frame then enters delta mode on the same
    /// connection (the streaming machinery is slice 2/3 — this variant is the wire
    /// type only). Multiplexed: multiple `Attach{id}` on one connection stream several
    /// panes, each frame tagged with its `id`. EUID-GATED ONLY (MF-D). Failure (pane
    /// absent / died during setup) → error response ([`response_code::PANE_DIED`]); the
    /// connection stays in awaiting-request state (no half-upgrade).
    Attach { id: String },
    /// Close the subscription for pane `id` on this connection. No-op (OK) if not
    /// subscribed. Other subscriptions persist; closing the connection drops all.
    /// EUID-GATED ONLY (MF-D).
    Detach { id: String },

    // ───────────────── Q4 daemon-spawns-on-behalf (approach B, §2) ─────────────────
    /// Q4 daemon-spawns-on-behalf. The DAEMON builds argv + opens the PTY + spawns the
    /// child, owning the master fd AND the owned `Child` from birth — the most
    /// consequential op in the protocol (it can spawn an arbitrary harness that survives
    /// app quit). MUTATING + daemon-local. Gated by euid + per-request `allow_mutations`
    /// AND a SEPARATE dedicated `daemon_spawn_enabled` flag (both default OFF); the daemon
    /// then RE-VALIDATES every field of [`SpawnSpec`] independently of the app (the gates
    /// answer WHO may spawn, never WHAT is spawned).
    Spawn { spec: SpawnSpec },
    /// Close a daemon-owned pane: remove it from the daemon's live map, kill the child
    /// (kernel-tied, PID-reuse-safe), remove the worktree, rewrite the live registry, and
    /// audit-log. Idempotent-OK for an absent id (like [`SocketRequest::Detach`]).
    Close { id: String },

    // ───────── #262 ext: EXTERNAL visible-grid spawn (app-served, event→frontend) ─────────
    /// Open a NEW VISIBLE workspace in the running app's grid. Served by the APP (not the
    /// daemon): the handler validates + emits a Tauri event the FRONTEND consumes to mint
    /// the wsId, spawn `count` panes, render + persist (the only path that makes panes
    /// appear). Gated by `allow_external_spawn` + pid-pin + WHAT-validation (trusted repo,
    /// harness allowlist [no bash], count cap). Fire-and-ack: the reply is `SpawnRequested`
    /// (the wsId is minted async by the webview, unknown at reply time).
    CreateWorkspace {
        repo: String,
        harness: String,
        count: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        /// #262 ext ATOMIC: a per-pane spec list. When NON-EMPTY it OVERRIDES the scalar
        /// `harness`/`count`/`role`/`model` above, so a single op opens ONE workspace with a
        /// mixed-harness / mixed-role team (e.g. 2 claude builders + 1 codex reviewer). The
        /// app sums every spec's `count` against [`EXTERNAL_SPAWN_MAX_PANES`] and validates
        /// EACH spec's harness + role before emitting (all-or-nothing — never partial-spawn).
        /// Empty (the default) preserves the legacy scalar behaviour byte-for-byte on the wire.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        panes: Vec<PaneSpec>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tag: Option<String>,
    },
    /// Add ONE pane to a TARGET workspace (same app-served event model). `target_workspace`
    /// names the destination by wsId OR by the `tag` stamped at create time; when absent the
    /// app falls back to the frontend's ACTIVE workspace (legacy behaviour). Naming the target
    /// removes the activeWs race that scattered panes across workspaces.
    AddPane {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        harness: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_workspace: Option<String>,
    },

    // ───────────── #21 gap-7: live-scrollback read (app-served READ op) ─────────────
    /// Read a LIVE tail of pane `id`'s in-memory PTY scrollback from the RUNNING app —
    /// the read primitive for STATE-BLIND harnesses (commandcode/codex/opencode/cline)
    /// that persist no on-disk transcript, so every sidecar disk locator misses and
    /// `team_read_output` could only answer an honest `source:"none"`. READ-ONLY: the
    /// app copies the pane's retained output buffer (`PaneBuffer` / the daemon
    /// attach-stream buffer) and NEVER mutates pane state.
    ///
    /// `max_bytes` is clamped server-side ([`read_output_cap`]: absent ⇒
    /// [`READ_OUTPUT_DEFAULT_MAX_BYTES`], hard cap [`READ_OUTPUT_HARD_MAX_BYTES`] —
    /// mirroring the sidecar's `read_output.rs` tail contract); the NEWEST tail comes
    /// back in [`SocketData::Output`]. Admission mirrors the external-orchestrator gate
    /// (a Coordinator pane OR `allow_external_orchestrator` + kernel-pid-pin — the op is
    /// in [`op_external_orchestrator_allowed`] as its one READ member); the id is
    /// validated app-side with [`validate_spawn_id`] (an id, NEVER a caller path).
    /// Serde-additive/backward-compatible: an OLD app deserializes this unknown `op` as
    /// a malformed request (structured `BAD_REQUEST`, never a panic), and an omitted
    /// `max_bytes` deserializes to `None` and is not re-serialized.
    ReadOutput {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_bytes: Option<u64>,
    },
}

/// One pane group in an atomic multi-pane [`SocketRequest::CreateWorkspace`] (#262 ext).
/// `count` replicates this exact `(harness, role, model)` triple N times, so a mixed team is
/// a short list — `[{claude, builder, x2}, {codex, reviewer, x1}]`. Every field is
/// app-side re-validated (harness + role allowlists); `count` contributes to the SUMMED cap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PaneSpec {
    /// Harness wire string. App rejects anything outside [`external_spawn_harness_allowed`].
    pub harness: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Replication count for THIS spec. Default 1. The SUM across specs must be ≤ the cap.
    #[serde(default = "pane_spec_count_default")]
    pub count: u32,
}

fn pane_spec_count_default() -> u32 {
    1
}

/// The wire SSOT for a Q4 [`SocketRequest::Spawn`] — constrained fields ONLY, never
/// argv / an fd / a `Box<dyn Child>`. The daemon maps this INTO a `WorkspaceSpec`
/// (`core/supervisor`) daemon-side after re-validating every field; this DTO gains no
/// new app coupling (the `Harness` enum stays supervisor/app-local — `harness` is the
/// stable wire string, validated via `Harness::from_wire`).
///
/// **`force_fresh` is structurally ABSENT (decision 5 / must-fix C5):** the destructive
/// freshen (`git reset --hard` + `git clean -fd`) is UNREPRESENTABLE over the wire, so an
/// injected `"force_fresh":true` is dropped by serde and never reaches `freshen_worktree`.
/// `is_worker=true` is WIRE-REJECTED by the daemon (decision 3 / C4) — the in-app
/// delegate/flywheel worker path keeps LOCAL spawn for workers. `repo` + `id` cross the
/// wire SEPARATELY (never a pre-resolved worktree path) so the daemon re-derives and
/// re-validates the sanctioned per-id worktree itself (C2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SpawnSpec {
    /// Workspace/pane id. Keys the per-id worktree dir, the write-lock map, and the
    /// registry — daemon validates charset/length / no sep / no `..` ([`validate_spawn_id`]).
    pub id: String,
    /// Harness WIRE string (`"claude"` / `"bash"` / …); daemon maps via `Harness::from_wire`.
    pub harness: String,
    /// Repo root the per-id worktree is derived under (C2). NOT a resolved worktree path.
    pub repo: String,
    /// Stable conversation id (claude `--session-id` / `--resume`); `None` → harness picks.
    #[serde(default)]
    pub session_id: Option<String>,
    /// Reopen an existing conversation rather than start fresh.
    #[serde(default)]
    pub resume: bool,
    /// Typed agent role (wire string; fail-soft parse daemon-side).
    #[serde(default)]
    pub role: Option<String>,
    /// Delegate-worker flag. WIRE-REJECTED if `true` (decision 3 / C4).
    #[serde(default)]
    pub is_worker: bool,
    /// Extra writable dirs (`--add-dir`). Each is REPO-SCOPE validated daemon-side
    /// ([`extra_dir_in_repo_scope`]) — an out-of-repo dir is rejected (decision 2 / C3).
    #[serde(default)]
    pub extra_dirs: Vec<String>,
    /// Optional model override (verbatim to the harness CLI; harness validates it).
    #[serde(default)]
    pub model: Option<String>,
    /// NON-destructive freshen request: re-base a REUSED worktree onto main. If the
    /// reused tree is dirty the daemon returns [`response_code::UNCOMMITTED_WORK`] — it
    /// NEVER runs the destructive freshen (decision 5); the GUI confirms + cleans app-side.
    #[serde(default)]
    pub fresh_from_main: bool,
    /// Require a real isolated worktree (fail loud if `add_worktree` cannot create one).
    #[serde(default)]
    pub require_worktree: bool,
}

/// The per-op read/write timeout. The SINGLE source of truth both the app
/// (`serve_socket_conn`, AFTER it has parsed a valid request) and the sidecar
/// (`dial`, the read-wait that blocks on the app's reply) import, so the two sides
/// can never disagree on how long a given op may take.
///
/// **Why per-op (the 06-02 security-review gap).** Fast ops (`SendInput`/`Focus`/
/// `Broadcast`/`Handoff`) are a single lock+write and must stay snappy (5s) so a
/// wedged peer can't tie up the serial listener. But the synthesis ops WRAP the
/// in-app synthesizer, which runs headless `claude` up to the app's 180s kill-timeout
/// — a 5s window would abort it far too early. `Orchestrate` is one pass → the 1-pass
/// window ([`ORCHESTRATE_TIMEOUT`]); `Synthesize` can chain up to three serial passes
/// (06-18 #1 independent two-pass conflict adversary) → the wider [`SYNTHESIZE_TIMEOUT`].
/// Everything else stays bounded at [`FAST_OP_TIMEOUT`].
pub fn op_timeout(req: &SocketRequest) -> std::time::Duration {
    match req {
        // Orchestrate is a SINGLE headless-claude pass (synthesize per-pane tasks) → the 1-pass
        // window. Synthesize fans the pane reports into final.md AND (06-18 #1) escalates any
        // cross-pane conflict through an INDEPENDENT two-pass adversary — up to THREE serial
        // headless-claude passes (synthesis → adversary → decide), so it gets the wider window.
        SocketRequest::Orchestrate { .. } => ORCHESTRATE_TIMEOUT,
        SocketRequest::Synthesize { .. } => SYNTHESIZE_TIMEOUT,
        // Q4 Spawn = worktree add + fork/exec + claude/cursor trust pre-seed + injection
        // → seconds, far past the 5s FAST_OP_TIMEOUT. Explicit arm (this match HAS a `_`
        // fallthrough, so without it Spawn would silently get the too-short fast window).
        // `Close` (remove + kill + worktree remove) correctly falls to the fast arm.
        SocketRequest::Spawn { .. } => SPAWN_TIMEOUT,
        _ => FAST_OP_TIMEOUT,
    }
}

/// Read/write timeout for the fast (single lock+write) ops. A wedged same-user
/// peer on one of these can stall the serial listener for at most this long.
pub const FAST_OP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Read/write window for `Orchestrate` — ONE headless-claude pass (the app's D43
/// kill-timeout is 180s) plus margin for the surrounding I/O, so the synthesis op is
/// never aborted by the SOCKET timeout before the app's OWN kill-timeout can fire (the
/// app stays the authority on when a single pass is "too long").
pub const ORCHESTRATE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

/// Read/write window for `Synthesize` — the fan-in can chain up to THREE serial
/// headless-claude passes when a run has cross-pane conflicts (06-18 #1: synthesis →
/// independent adversary → final decide), each bounded by the app's per-pass kill-timeout
/// (`SYNTH_DEADLINE_SECS`, raised 180→240 on 06-19 when the Opus adjudicator went to
/// `--effort xhigh`). Sized to cover all three plus I/O margin (3×240 + 60) so the socket
/// read-wait never aborts a conflict-bearing synthesis — which would orphan a completed
/// final.md AND the Opus tokens already spent on the escalation. A no-conflict run still
/// returns far inside this window (it makes only the single synthesis pass).
pub const SYNTHESIZE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(780);

/// Read/write window for a Q4 [`SocketRequest::Spawn`] — wider than [`FAST_OP_TIMEOUT`]
/// because the daemon's `handle_spawn` does a `git worktree add` (sparse checkout on a
/// big repo can take seconds) + fork/exec + claude/cursor trust pre-seed + MCP/persona
/// injection before it can reply. Bounded so a wedged same-user peer mid-spawn can stall
/// only its own connection for at most this long.
pub const SPAWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

// ───────────────────── delegate fairness + depth policy (MVP) ───────────────────
//
// Pure, dep-free decision helpers for the autonomous `Delegate` op. The fairness
// sub-budget + hard cap + depth guard live HERE so the app controller and the
// prototype simulator agree on the exact admission policy.

/// Hard ceiling on workers per delegation. Raised 3→10 (operator request) — note the FAIRNESS
/// budget (`max_concurrent - 1`) still bites first, so the effective count is
/// `min(requested, cap-1, 10)`; reaching 10 needs a concurrency cap ≥ 11 (see `clamp_default_cap`).
pub const DELEGATE_MAX_WORKERS: u32 = 10;

/// FR-7 fairness sub-budget: workers may use at most `max_concurrent - 1` slots so at
/// least one slot is always reserved for a human-driven pane. Never returns 0.
pub fn delegate_worker_budget(max_concurrent: usize) -> usize {
    max_concurrent.saturating_sub(1).max(1)
}

/// Admitted worker count = min(requested, fairness budget, hard cap), floored at 1.
/// Mirrors the prototype `_clampWorkerSpawn` so sim and backend agree.
pub fn delegate_admit(requested: u32, max_concurrent: usize) -> u32 {
    let budget = delegate_worker_budget(max_concurrent) as u32;
    // DELEGATE_MAX_WORKERS >= 1, so clamp's max >= min invariant holds.
    requested.min(budget).clamp(1, DELEGATE_MAX_WORKERS)
}

/// MVP pins depth=1 (no recursion). True only for depth==1.
pub fn delegate_depth_ok(depth: u32) -> bool {
    depth == 1
}

// ───────────────────── write-path payload defenses (08 Sub-build 3 / MF-E) ───────────────────
//
// Lifted from the app's `write_to_pane` site so the GUI accept path and the daemon
// accept loop share ONE implementation and can never drift (design §2 MF-E). These
// are pure and dep-free; they operate on the harness's stable WIRE STRING (the
// `Harness` enum stays app/supervisor-local — callers map their harness → wire via
// `descriptor().wire`).

/// The `\n`-rule normalizer (pure → unit-tested). Mutation text MUST be exactly one
/// line of LITERAL text; this returns `text` with a single trailing `\r` appended
/// (the Enter-submits-TUI invariant: raw-mode agent TUIs — claude/codex/opencode —
/// submit on CR `\r`, not LF `\n`; cooked shells map CR→LF via the tty's ICRNL, so a
/// trailing `\r` submits in BOTH modes while a bare `\n` only submits a cooked shell).
/// A trailing `\n`/`\r\n` the caller already added is collapsed to one. INTERIOR
/// newlines are REJECTED (`Err`) — a multi-line payload could submit a second TUI line
/// (e.g. sneak an extra "yes"), the Model-A auto-confirm risk. Any other CONTROL
/// character is also REJECTED: ESC (`\x1b`), Ctrl-C (`\x03`), backspace, tab, DEL, C1
/// controls, etc. drive the TUI (cursor/history/signals/completion) rather than
/// delivering literal text, which would violate the Model-A "literal text only"
/// invariant for a socket peer (security review A.4, D38). The trailing `\r` we append
/// below is the ONLY control byte allowed to reach the PTY. Empty text is allowed (a
/// bare Enter).
pub fn normalize_input(text: &str) -> Result<String, &'static str> {
    // Strip a single trailing newline the caller may have included, then forbid any
    // remaining interior newline (CR or LF).
    let trimmed = text.strip_suffix('\n').unwrap_or(text);
    let trimmed = trimmed.strip_suffix('\r').unwrap_or(trimmed);
    if trimmed.contains('\n') || trimmed.contains('\r') {
        return Err("interior newline rejected (input must be a single line)");
    }
    // Forbid every other control character — only literal text may reach the PTY.
    if trimmed.chars().any(|c| c.is_control()) {
        return Err("control character rejected (input must be literal single-line text)");
    }
    // Submit with CR (`\r`), not LF (`\n`): raw-mode agent TUIs (claude/codex/opencode/
    // commandcode) bind Enter to CR and ignore a bare LF, so `\n` left the text sitting
    // unsubmitted in the input box. Cooked shells map CR→LF via the tty's ICRNL, so a
    // trailing `\r` submits in BOTH raw and cooked panes. Still exactly one control byte.
    Ok(format!("{trimmed}\r"))
}

/// Paste-burst-coalescing TUIs misread a trailing `\r` GLUED to the text as a
/// newline-insert instead of submit, so the submit `\r` must be a SEPARATE PTY write
/// (see the GUI `write_to_pane` two-phase write and the daemon `handle_split_write`).
/// Every interactive agent TUI now coalesces a glued `\r` (paste-burst for codex; Ink
/// stdin batching for cursor/commandcode/opencode; claude Code v2.1.181 joined). Only
/// `bash` (a raw shell, ICRNL maps CR→LF) submits on the single concat write. Operates
/// on the harness WIRE STRING so core needs no `Harness` enum.
pub fn harness_needs_split_submit(wire: &str) -> bool {
    wire != "bash"
}

/// Per-harness settle (ms) between the body write and the lone submit `\r` — the window
/// the harness's paste-burst / Ink stdin coalescer needs to flush the body BEFORE the
/// `\r` arrives as its OWN keypress. SCALED by payload: a long paste arrives in more TTY
/// chunks, so the coalescer debounces LATER — a fixed short window fires mid-paste and
/// the `\r` is folded into the composer instead of submitting (06-08: a ~600B orchestrate
/// task hung on commandcode + opencode at the old fixed 120ms). Base is per-harness (Ink
/// TUIs batch slower than codex's paste-burst); `+1ms / 3 bytes` over it, capped at
/// 1200ms. Operates on the harness WIRE STRING. An unknown wire defaults to the
/// claude/100ms base (a safe non-zero window — only `bash` is exempt, and
/// `harness_needs_split_submit` already gates whether this is reached at all).
pub fn split_settle_ms(wire: &str, payload_len: usize) -> u64 {
    let base: u64 = match wire {
        "codex" => 80,        // paste-burst detection flushes fast
        "claude" => 100,      // v2.1.181 glued-\r coalescing
        "cursor" => 150,      // Ink stdin batching
        "opencode" => 180,    // joined the coalescing set (06-08)
        "commandcode" => 200, // Ink, slowest batching observed
        "cline" => 200,       // Ink TUI, slowest-batch class (= commandcode)
        "grok" => 200,        // conservative base (= cline); TUI coalescing unmeasured
        "bash" => return 0,   // never splits (harness_needs_split_submit=false)
        _ => 100,             // unknown harness → claude-equivalent safe default
    };
    (base + (payload_len as u64) / 3).min(1200)
}

/// MF-D op classifier (pure → unit-tested). `true` for the mutating ops (which require
/// BOTH the euid gate AND the per-request `allow_mutations` gate), `false` for the
/// euid-gated-only read ops (`ListLive`/`Attach`/`Detach`) that MUST work on a default
/// install so re-attach can happen with `allow_mutations=false` (design §2 MF-D). The
/// `match` is EXHAUSTIVE (no `_` arm) so adding a `SocketRequest` variant forces an
/// explicit read-vs-mutate decision here rather than silently defaulting.
pub fn op_requires_mutations(req: &SocketRequest) -> bool {
    match req {
        SocketRequest::ListLive
        | SocketRequest::Attach { .. }
        | SocketRequest::Detach { .. }
        // gap-7: live-scrollback read — a READ op (the app only copies the pane's
        // retained output buffer; no pane state mutates). NOT euid-only like ListLive
        // though: because it exfiltrates pane CONTENT, the app layers the coordinator/
        // external-orchestrator admission on it in `handle_socket_request`.
        | SocketRequest::ReadOutput { .. } => false,
        SocketRequest::SendInput { .. }
        | SocketRequest::Focus { .. }
        | SocketRequest::Orchestrate { .. }
        | SocketRequest::Broadcast { .. }
        | SocketRequest::Handoff { .. }
        | SocketRequest::Synthesize { .. }
        | SocketRequest::Delegate { .. }
        // Q4: Spawn/Close MUTATE the daemon's live-pane set → euid + fresh per-request
        // `allow_mutations` (a default install refuses them). The daemon ALSO checks the
        // separate `daemon_spawn_enabled` gate inside `handle_spawn` — both default OFF.
        | SocketRequest::Spawn { .. }
        | SocketRequest::Close { .. }
        // #262 ext: external visible-grid spawn — mutates the grid (creates panes).
        | SocketRequest::CreateWorkspace { .. }
        | SocketRequest::AddPane { .. } => true,
    }
}

/// External-orchestrator op allowlist (pure → unit-tested). The NARROW subset of ops a
/// trusted EXTERNAL caller (e.g. GlikaAgents, opt-in + pid-pinned) may drive against
/// the VISIBLE pane grid: prompt one pane, broadcast to all, orchestrate a goal (preview
/// or dispatch), focus a pane, and (gap-7) READ a pane's live scrollback tail
/// (`ReadOutput` — the one non-mutating member: the external brain must be able to read
/// what a state-blind pane produced, and the read is gated exactly like the control ops).
/// DELIBERATELY EXCLUDES `Handoff` / `Synthesize` / `Delegate` / `Spawn` / `Close` —
/// those keep requiring a real Coordinator pane (Delegate also stays behind
/// autonomy_ceiling + the delegate-live build). Scoping by OP, never by granting the
/// Coordinator role, is what keeps the autonomous/lifecycle surface closed to externals.
pub fn op_external_orchestrator_allowed(req: &SocketRequest) -> bool {
    matches!(
        req,
        SocketRequest::SendInput { .. }
            | SocketRequest::Broadcast { .. }
            | SocketRequest::Orchestrate { .. }
            | SocketRequest::Focus { .. }
            | SocketRequest::ReadOutput { .. }
    )
}

/// External-SPAWN op allowlist (pure → unit-tested). The NARROW subset a pid-pinned
/// external caller may use to OPEN visible panes (`CreateWorkspace` / `AddPane`) when
/// `allow_external_spawn` is armed. A SEPARATE axis from `op_external_orchestrator_allowed`
/// (creation vs. control) so spawning — strictly more powerful — is independently opt-in.
/// Excludes everything else, including `Spawn`/`Close`/`Delegate` (daemon/autonomous).
pub fn op_external_spawn_allowed(req: &SocketRequest) -> bool {
    matches!(
        req,
        SocketRequest::CreateWorkspace { .. } | SocketRequest::AddPane { .. }
    )
}

/// External-spawn harness allowlist (pure → unit-tested). An external caller may spawn ONLY
/// agent harnesses whose own permission models gate shell exec — NEVER `bash`/raw shells
/// (a shell PTY + the already-granted SendInput = arbitrary command execution). Case/space
/// tolerant; unknown → false (fail closed).
pub fn external_spawn_harness_allowed(harness: &str) -> bool {
    matches!(
        harness.trim().to_ascii_lowercase().as_str(),
        "claude" | "cursor" | "codex" | "opencode" | "commandcode" | "cline" | "grok"
    )
}

/// External-spawn ROLE allowlist (pure → unit-tested). A role string sets a pane's persona
/// AND (for `coordinator`) the privileged sidecar that passes the #262 coordinator gate — so
/// an UNPARSEABLE role must be REJECTED, never silently dropped to "no role". This is the
/// allowlist of the KNOWN [`core/roles`] variants (kept in sync with `AgentRole`); the
/// operator has opted to permit `coordinator` on this path (their machine; pid-pin + audit +
/// confirm are the backstops), so it is included. Case/space tolerant; unknown → false
/// (fail closed). `None`/omitted role is fine — that means a plain pane with no persona.
pub fn external_spawn_role_allowed(role: &str) -> bool {
    matches!(
        role.trim().to_ascii_lowercase().as_str(),
        "coordinator"
            | "builder"
            | "coder"
            | "scout"
            | "reviewer"
            | "tester"
            | "performance"
            | "perf"
            | "security"
            | "db-migration"
            | "dbmigration"
            | "migration"
    )
}

/// DEFAULT per-request cap on panes an external `CreateWorkspace` may open (refuse above it —
/// never enqueue a flood). Bounded on purpose; the operator confirms each spawn anyway.
/// 8 fits a realistic mixed team (e.g. 2 builders + 2 reviewers + a coordinator + spares).
/// Operator-overridable per install via `McpConfig.external_spawn_max_panes` — resolve the
/// EFFECTIVE cap through [`external_spawn_cap`], never this const directly.
pub const EXTERNAL_SPAWN_MAX_PANES: u32 = 8;

/// The EFFECTIVE external-spawn pane cap for this install: the operator's
/// `external_spawn_max_panes` (mcp-config.json) if set, else [`EXTERNAL_SPAWN_MAX_PANES`];
/// clamped to 1..=16 (16 = the daemon's `MAX_DAEMON_PANES` live ceiling — a bigger
/// per-request cap could never spawn anyway). Pure → unit-tested.
pub fn external_spawn_cap(cfg: &McpConfig) -> u32 {
    cfg.external_spawn_max_panes
        .unwrap_or(EXTERNAL_SPAWN_MAX_PANES)
        .clamp(1, 16)
}

/// The EFFECTIVE pinned external-orchestrator executable set: the singular
/// `external_orchestrator_path` plus every `external_orchestrator_paths` entry — trimmed,
/// empties dropped, deduped (order-preserving). The pid-ancestry gate admits a caller
/// matching ANY of these. Empty ⇒ no external caller is ever admitted (fail closed).
/// Pure → unit-tested.
pub fn external_orchestrator_pins(cfg: &McpConfig) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let singular = cfg.external_orchestrator_path.as_deref().unwrap_or("");
    for p in
        std::iter::once(singular).chain(cfg.external_orchestrator_paths.iter().map(String::as_str))
    {
        let t = p.trim();
        if !t.is_empty() && !out.iter().any(|e| e == t) {
            out.push(t.to_string());
        }
    }
    out
}

/// Default returned-bytes cap for a [`SocketRequest::ReadOutput`] live-scrollback tail.
/// MIRRORS the sidecar's `agent-teams-mcp/src/read_output.rs` disk-read default so the
/// two read surfaces (disk artifact vs live buffer) obey ONE tail contract.
pub const READ_OUTPUT_DEFAULT_MAX_BYTES: usize = 65_536;

/// Hard server-side cap for [`SocketRequest::ReadOutput`] (mirrors `read_output.rs`).
/// A caller asking for more gets exactly this much of the newest tail.
pub const READ_OUTPUT_HARD_MAX_BYTES: usize = 262_144;

/// Clamp a caller-supplied `max_bytes` to the sanctioned tail window (pure →
/// unit-tested): absent ⇒ [`READ_OUTPUT_DEFAULT_MAX_BYTES`]; anything above
/// [`READ_OUTPUT_HARD_MAX_BYTES`] (including a u64 that overflows usize) ⇒ the hard
/// cap. An explicit `0` is honored (a liveness probe that returns an empty tail) —
/// the same arithmetic the sidecar's disk reader applies.
pub fn read_output_cap(max_bytes: Option<u64>) -> usize {
    max_bytes
        .map(|m| usize::try_from(m).unwrap_or(READ_OUTPUT_HARD_MAX_BYTES))
        .unwrap_or(READ_OUTPUT_DEFAULT_MAX_BYTES)
        .min(READ_OUTPUT_HARD_MAX_BYTES)
}

/// MF-D ROUTING classifier (pure → unit-tested). `true` for the synthesis/orchestration
/// ops (`Orchestrate`/`Broadcast`/`Handoff`/`Synthesize`/`Delegate`) that WRAP the in-app
/// synthesizer (design §7) and are therefore NEVER served by the daemon — its accept loop
/// answers [`response_code::SERVED_BY_APP`] so the GUI re-routes them to the app's own
/// `handle_socket_request`.
///
/// This is a ROUTING FACT independent of `allow_mutations`: the daemon owns no synthesis
/// machinery, so the served-by-app answer must be given EVEN on a default install
/// (mutations OFF) and AHEAD of the mutation gate. Otherwise the gate (these ops are also
/// classified mutating by [`op_requires_mutations`]) fires first and the GUI gets
/// `MUTATIONS_DISABLED`, which it cannot distinguish from "wrong endpoint" → it never
/// re-routes (e.g. a harmless `Orchestrate{dispatch:false}` preview). EXHAUSTIVE (no `_`)
/// so a new `SocketRequest` variant forces an explicit served-vs-local decision here.
pub fn op_served_by_app(req: &SocketRequest) -> bool {
    match req {
        SocketRequest::Orchestrate { .. }
        | SocketRequest::Broadcast { .. }
        | SocketRequest::Handoff { .. }
        | SocketRequest::Synthesize { .. }
        | SocketRequest::Delegate { .. }
        // #262 ext: external spawn is APP-served — only the app's webview owns the grid +
        // `createWorkspace`. The daemon answers SERVED_BY_APP so the GUI reroutes to the app.
        | SocketRequest::CreateWorkspace { .. }
        | SocketRequest::AddPane { .. }
        // gap-7: the live-scrollback read is APP-served — the app owns BOTH live buffer
        // seams (the in-process `PaneBuffer` registry AND the attach-stream buffers it
        // keeps for daemon-owned panes), so the daemon reroutes rather than half-serving.
        | SocketRequest::ReadOutput { .. } => true,
        SocketRequest::ListLive
        | SocketRequest::SendInput { .. }
        | SocketRequest::Focus { .. }
        | SocketRequest::Attach { .. }
        | SocketRequest::Detach { .. }
        // Q4: Spawn/Close are DAEMON-LOCAL — the daemon owns `DaemonSups` + links the
        // supervisor crate, so it serves them itself (NOT served by the app synthesizer).
        | SocketRequest::Spawn { .. }
        | SocketRequest::Close { .. } => false,
    }
}

// ───────────────────── Q4 spawn-payload validators (C1/C3/C6) ───────────────────
//
// Pure, dep-free re-validators the DAEMON applies independently of the app to a wire
// `SpawnSpec` (the gates answer WHO, these answer WHAT — app-side validation is no
// defense because a same-user peer crafts the Spawn directly). Lifted into this shared
// crate so the daemon validates without reaching into the app (the `normalize_input`
// pattern applied to `Spawn`).

/// Validate a peer-supplied workspace id BEFORE it keys the per-id worktree dir
/// (`add_worktree` → `git_root/.agent-teams-worktrees/<id>`), the write-lock map, or the
/// registry (C1). `true` iff `id` is non-empty, within the length cap, and uses ONLY
/// `[A-Za-z0-9_-]`. Forbidding `.`/`/`/`\\`/whitespace/control bytes makes a path
/// separator or `..` traversal structurally impossible — over the wire the id is
/// attacker-influenced (app-side it is server-set provenance, `AGENT_TEAMS_PANE_ID`).
pub fn validate_spawn_id(id: &str) -> bool {
    const MAX_ID_LEN: usize = 128;
    !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Repo-scope an `extra_dir` (`--add-dir`, `supervisor::worker_args`) BEFORE it grants
/// the spawned agent write access (C3 / decision 2). PURE lexical containment: `true`
/// iff both paths are ABSOLUTE, the `extra_dir` carries no `.`/`..` component (so a
/// non-existent dir can't smuggle traversal), and `extra_dir` is `repo_root` itself or a
/// whole-component descendant of it (`Path::starts_with`, so `/repo-evil` is NOT under
/// `/repo`). The daemon passes the CANONICALIZED repo root (and canonicalizes the
/// `extra_dir` when it exists) so a symlink can't escape the lexical check. The opt-in
/// allowlist-of-roots (default empty) is a SEPARATE, deferred check layered on top.
pub fn extra_dir_in_repo_scope(repo_root: &Path, extra_dir: &Path) -> bool {
    use std::path::Component;
    if !repo_root.is_absolute() || !extra_dir.is_absolute() {
        return false;
    }
    if extra_dir
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return false;
    }
    extra_dir.starts_with(repo_root)
}

/// Validate a peer-supplied `session_id` BEFORE it flows VERBATIM into the harness argv
/// (claude `session_args` emits `["--resume", id]` / `["--session-id", id]`). `--resume`
/// takes an OPTIONAL value, so a session_id that is itself a flag token
/// (e.g. `--dangerously-skip-permissions`) is NOT consumed as the resume value — a
/// clap-style parser reads it as a STANDALONE FLAG and the spawned agent runs in
/// permission-bypass/YOLO mode (flag injection that nullifies the C3 extra_dirs repo-scope).
/// `true` iff non-empty, within the length cap, ASCII `[A-Za-z0-9_-]` only (covers UUIDs —
/// claude's session id form), and NOT leading with `-` (the load-bearing anti-injection
/// check). The wire-attacker-influenced session_id is the `Spawn` analogue of `id`
/// ([`validate_spawn_id`]) — the daemon re-validates it (C6: gates answer WHO, never WHAT).
pub fn validate_session_id(id: &str) -> bool {
    const MAX_SESSION_ID_LEN: usize = 128;
    !id.is_empty()
        && id.len() <= MAX_SESSION_ID_LEN
        && !id.starts_with('-')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Validate a peer-supplied `model` override BEFORE it flows VERBATIM into the harness argv
/// (`model_args` emits `["--model", m]` / `["-m", m]`). A model that is itself a flag token
/// (leading `-`, e.g. `--dangerously-skip-permissions`) is the same flag-injection class as
/// [`validate_session_id`]; interior whitespace / control bytes have no place in a model id.
/// `true` iff non-empty, within the length cap, NOT leading with `-` (the load-bearing
/// check), and free of whitespace/control bytes. The charset is intentionally LOOSER than
/// `session_id` because model ids legitimately contain `/` and `.`
/// (opencode `provider/model`, dated snapshots like `claude-3-5-sonnet-20241022`). An empty
/// model is handled by the caller as "account default" (never reaches this validator).
pub fn validate_model(m: &str) -> bool {
    const MAX_MODEL_LEN: usize = 128;
    !m.is_empty()
        && m.len() <= MAX_MODEL_LEN
        && !m.starts_with('-')
        && !m
            .bytes()
            .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
}

/// One `{id, task}` entry in an `Orchestrate{dispatch:false}` preview mapping. The
/// core mirror of the app's private `Dispatch` row — defined HERE so the structured
/// [`SocketResponse::data`] payload crosses the wire through the shared SSOT and the
/// sidecar can deserialize it without reaching into the app crate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DispatchEntry {
    /// Target pane id (matches `QueueRow.id`).
    pub id: String,
    /// The synthesized, self-contained task for that pane.
    pub task: String,
}

/// One pane's report verdict (06-05) — the wire form of the app's `verify_dispatched`
/// reconciler. Defined HERE so the app emits and the sidecar deserializes the SAME
/// shape. `status` is one of `"ok"` / `"empty"` / `"incomplete"` / `"missing"` (a pane
/// dispatched but with no report on disk is `missing`; `dead` ⇒ it will never write it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PaneVerdictWire {
    /// Pane id (matches `QueueRow.id` / the manifest entry).
    pub id: String,
    /// `"ok"` | `"empty"` | `"incomplete"` | `"missing"`.
    pub status: String,
    /// Byte size of the pane's `<id>.md` report (0 = never written).
    pub bytes: u64,
    /// The pane's PTY is gone — a `missing`/`empty` report will never improve.
    pub dead: bool,
}

/// One pane's contribution to a `team_synthesize{pane_ids}` consolidation (Phase 21) —
/// the wire form of the sidecar's `team_read_output` resolution for that pane. Sibling
/// of [`PaneVerdictWire`] (which reconciles a MANIFEST-dispatched run); a `pane_ids`
/// consolidation has no manifest, so each row reports the resolved on-disk SOURCE
/// instead of a dispatch verdict. A pane with nothing on disk appears as
/// `source:"none"` — reported, never silently dropped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PaneSourceWire {
    /// Pane id as requested (matches `QueueRow.id` / the live registry).
    pub id: String,
    /// Where the content came from: `orchestrate_report` | `claude_transcript` |
    /// `cursor_transcript` | `none` (nothing on disk for this pane).
    pub source: String,
    /// Bytes of content folded into the consolidated document (0 when `source:"none"`).
    pub bytes: u64,
    /// True when the pane's artifact exceeded the per-pane cap and only its newest
    /// tail was folded in.
    pub truncated: bool,
}

/// Structured payload a [`SocketResponse`] may carry (06-03). The 06-02 responses
/// are flat `{ok,code,detail}` and carry NO `data`; this is the typed channel for
/// the Context Router ops that must return DATA (a preview mapping, a broadcast
/// fan-out result) rather than abusing the human-readable `detail` string.
///
/// A TYPED enum (not `Option<serde_json::Value>`) so [`SocketResponse`] keeps its
/// `Eq`/`PartialEq` derives (`serde_json::Value` is not `Eq`) and both panes agree
/// on the exact shape. Tagged by `kind` for an unambiguous wire form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SocketData {
    /// `Orchestrate{dispatch:false}` preview: the per-pane `{id,task}` mapping the
    /// host inspects BEFORE a separate `dispatch:true` call fans it out (D23).
    Mapping { tasks: Vec<DispatchEntry> },
    /// `Broadcast` (and a dispatched `Orchestrate`) fan-out result: the ids that
    /// received input vs the dead panes that were skipped (never silently dropped).
    Broadcast {
        sent: Vec<String>,
        skipped: Vec<String>,
    },
    /// `Delegate` accepted: the controller is running detached. `run_id` is the bridge
    /// run dir name; `workers` is the admitted worker count after the fairness clamp.
    Delegate { run_id: String, workers: u32 },
    /// `CreateWorkspace`/`AddPane` accepted (#262 ext): the spawn was REQUESTED of the
    /// frontend. The wsId is minted async by the webview, so it is NOT known at reply time;
    /// `tag` echoes the caller's correlation tag for `list_workspaces` disambiguation.
    SpawnRequested {
        op: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tag: Option<String>,
    },
    /// `Orchestrate{dispatch:true}` with fan-in (06-07): tasks were dispatched AND a run dir
    /// was stamped — its `manifest.json` holds the dispatched ids and each pane was told to
    /// write `<run_dir>/<id>.md` (ending `## BOUNDARIES`). Pass `run_dir` straight to
    /// `team_synthesize` to fan the reports into one final.md — the full orchestrate→synthesize
    /// loop with no GUI. `sent`/`skipped` partition the dispatched ids (dead / un-normalizable
    /// panes skipped, never silently dropped).
    Dispatched {
        run_dir: String,
        sent: Vec<String>,
        skipped: Vec<String>,
    },
    /// `Synthesize` fan-in result (06-05): the written consolidated report + the per-pane
    /// verdicts. `report_path` is the absolute `final.md`; `run_id` is the run dir name;
    /// `verdicts` is the `verify_dispatched` engine's answer to "did every dispatched
    /// harness produce a usable report" (so the host sees what was/wasn't consolidated).
    Synthesis {
        report_path: String,
        run_id: String,
        verdicts: Vec<PaneVerdictWire>,
    },
    /// `team_synthesize{pane_ids}` SIDECAR-LOCAL fan-in result (Phase 21): the
    /// consolidated markdown assembled from each requested pane's on-disk output (the
    /// same report/transcript resolution as `team_read_output` — no orchestrate run
    /// dir needed). Unlike [`Self::Synthesis`] (app-side, path-only), `content`
    /// carries the document ITSELF: this mode works with the app closed, so the host
    /// must not need a second read to get the result. `report_path` is where the
    /// sidecar ALSO wrote it (a server-chosen synth dir, never a caller path);
    /// `panes` lists every requested pane's resolved source — `source:"none"` panes
    /// are reported, never dropped. Everything is CLAIMED (assembled from pane
    /// outputs, not verified). ADDITIVE: the app never emits this variant; only the
    /// sidecar constructs it locally.
    PaneSynthesis {
        report_path: String,
        run_id: String,
        content: String,
        panes: Vec<PaneSourceWire>,
    },
    /// `ListLive` reply (08 Sub-build 3, §3.2): the live workspace ids + optional
    /// metadata. Mirrors `LiveRegistry.workspaces`. `workspaces` may be omitted (serde
    /// default) for a minimal reply — when present it carries the per-id [`LiveWorkspace`]
    /// rows the relaunched GUI needs to partition reattach-vs-cold-resume.
    LivePanes {
        ids: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspaces: Option<Vec<LiveWorkspace>>,
    },
    /// [`SocketRequest::ReadOutput`] reply (gap-7): a LIVE tail of the pane's in-memory
    /// PTY scrollback. `content` is lossy-UTF-8 (raw PTY bytes carry ANSI escapes and
    /// may split codepoints) and at most the clamped `max_bytes`; `truncated` is true
    /// when older retained scrollback was cut to fit the cap. UNVERIFIED live data —
    /// the pane may be mid-stream; there is no report contract on this channel.
    Output { content: String, truncated: bool },
}

/// The app's structured reply to a [`SocketRequest`] (one line of JSON). NEVER
/// carries secret material.
///
/// 06-03 adds the optional, typed [`data`](Self::data) payload so the Context
/// Router ops can return DATA (a preview mapping / a broadcast result) without
/// abusing `detail`. **Backward-compatible:** `data` is `#[serde(default,
/// skip_serializing_if = "Option::is_none")]`, so a 06-02 `{ok,code,detail}`
/// response serializes BYTE-FOR-BYTE as before (no `data` key) and an old reply
/// with no `data` field still deserializes — the existing 06-02 tests + handlers
/// are unaffected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SocketResponse {
    /// `true` iff the op was applied to the live app.
    pub ok: bool,
    /// Machine-readable code — one of [`response_code`].
    pub code: String,
    /// Human-readable detail (diagnostic only).
    pub detail: String,
    /// Optional typed payload (06-03 Context Router ops). Absent for every 06-02
    /// response, so the flat `{ok,code,detail}` wire form is preserved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<SocketData>,
}

impl SocketResponse {
    /// Construct an `ok` response with [`response_code::OK`] and no payload.
    pub fn ok(detail: impl Into<String>) -> Self {
        Self {
            ok: true,
            code: response_code::OK.to_string(),
            detail: detail.into(),
            data: None,
        }
    }
    /// Construct a failure response with a [`response_code`] string and no payload.
    pub fn err(code: &str, detail: impl Into<String>) -> Self {
        Self {
            ok: false,
            code: code.to_string(),
            detail: detail.into(),
            data: None,
        }
    }
    /// Attach a typed [`SocketData`] payload to a response (builder; 06-03).
    pub fn with_data(mut self, data: SocketData) -> Self {
        self.data = Some(data);
        self
    }
}

/// The closed set of [`SocketResponse::code`] strings the app emits and the
/// sidecar maps. Defined here so both panes agree on the exact strings.
pub mod response_code {
    /// Op applied successfully.
    pub const OK: &str = "OK";
    /// Target PTY failed the `is_alive()` gate (D30) — write rejected, not
    /// silently dropped.
    pub const DEAD_PANE: &str = "DEAD_PANE";
    /// No such workspace id in the live set.
    pub const UNKNOWN_WORKSPACE: &str = "UNKNOWN_WORKSPACE";
    /// Peer-cred (euid) check failed — the caller is not the same local user.
    pub const FORBIDDEN: &str = "FORBIDDEN";
    /// `allow_mutations` is off — the capability gate denied the op.
    pub const MUTATIONS_DISABLED: &str = "MUTATIONS_DISABLED";
    /// `send_input_enabled` is off — the NARROW agent→agent send-input gate denied the op
    /// (its own axis, decoupled from `allow_mutations`; armed via the Settings UI toggle).
    pub const SEND_INPUT_DISABLED: &str = "SEND_INPUT_DISABLED";
    /// The request line was not a valid [`SocketRequest`].
    pub const BAD_REQUEST: &str = "BAD_REQUEST";
    /// `autonomy_ceiling` is L0 (default) — autonomous delegation refused (suggest-only).
    pub const AUTONOMY_DISABLED: &str = "AUTONOMY_DISABLED";
    /// depth>1 requested — recursion is out of scope for the MVP (FR-12 deferred).
    pub const DEPTH_EXCEEDED: &str = "DEPTH_EXCEEDED";
    /// The Delegate request passed every gate, but the live `socket_delegate`
    /// controller body is NOT compiled into this binary (the `delegate-live` cargo
    /// feature is off, pending the security review). The default build ships the gated
    /// handler + the fast-ack contract but not the spawn loop, so a delegation is
    /// accepted-but-unavailable rather than silently spawning unreviewed workers.
    pub const DELEGATE_UNAVAILABLE: &str = "DELEGATE_UNAVAILABLE";
    /// A delegation is ALREADY running. The MVP admits ONE delegation at a time (the
    /// global in-flight guard) so two detached controllers can never race the shared
    /// AppState mutexes / the concurrency cap (review Q1 re-entrancy). The caller should
    /// wait for the in-flight run's write-back before delegating again.
    pub const DELEGATION_IN_FLIGHT: &str = "DELEGATION_IN_FLIGHT";
    /// `Attach` succeeded; the connection has entered streaming mode (08 Sub-build 3,
    /// §3.3). The snapshot frame follows on the same connection.
    pub const STREAMING: &str = "STREAMING";
    /// `Attach` failed: the pane was live at `ListLive` time but died during setup
    /// (08 Sub-build 3, §3.3) — the subscription is not established.
    pub const PANE_DIED: &str = "PANE_DIED";
    /// The op is a synthesis/orchestration op (`Orchestrate`/`Broadcast`/`Handoff`/
    /// `Synthesize`/`Delegate`) that WRAPS the in-APP synthesizer (design §7). The
    /// DAEMON does not own that machinery, so its accept loop refuses these with this
    /// sentinel; the GUI routes them to the app's own `handle_socket_request` instead.
    pub const SERVED_BY_APP: &str = "SERVED_BY_APP";
    /// `Attach`/`Detach` reached the daemon before the streaming machinery exists
    /// (08 Sub-build 3 slice 2 — subscriptions land in slice 3). The connection is
    /// KEPT ALIVE; the caller may issue other ops. Distinct from [`STREAMING`]
    /// (success) and [`PANE_DIED`] (a live-then-dead pane during setup).
    pub const STREAMING_UNAVAILABLE: &str = "STREAMING_UNAVAILABLE";
    /// The daemon's bounded in-flight connection cap was hit (MF-A). The connection is
    /// refused WITHOUT spawning a handler thread, so a same-user connection flood (or a
    /// buggy client) cannot grow daemon threads without limit. The caller may retry once
    /// existing connections drain.
    pub const BUSY: &str = "BUSY";

    // ───────────────── Q4 daemon-spawns-on-behalf (approach B, §10) ─────────────────
    /// `Spawn` passed the euid + `allow_mutations` gates but the SEPARATE dedicated
    /// `daemon_spawn_enabled` flag is OFF (decision 1 / B2) — the daemon spawns NOTHING
    /// and the GUI falls back to local spawn. The DISABLED/refused variant: `allow_mutations`
    /// alone is never sufficient.
    pub const SPAWN_DISABLED: &str = "SPAWN_DISABLED";
    /// `Spawn`/`Close` reached a daemon built WITHOUT the `daemon-spawn` feature (compiled
    /// out by default). The route arm has no handler; the GUI falls back to local spawn.
    pub const SPAWN_UNAVAILABLE: &str = "SPAWN_UNAVAILABLE";
    /// `Spawn` over an id that is ALREADY live (reject-over-live-id / D5). The daemon does
    /// NOT kill-on-replace over the wire (that would be a same-user-peer DoS on the running
    /// agent); a deliberate reopen is an explicit `Close` then `Spawn`.
    pub const ALREADY_LIVE: &str = "ALREADY_LIVE";
    /// The daemon's INDEPENDENT re-validation of the wire spec failed (C1-C4): a bad id
    /// (sep/`..`/charset/length), an unknown harness, `is_worker=true` (decision 3), an
    /// out-of-repo `extra_dir` (decision 2), an out-of-scope/unmet worktree, etc. The gates
    /// answer WHO; this answers WHAT — app-side validation is no defense (a peer bypasses it).
    pub const SPAWN_REJECTED: &str = "SPAWN_REJECTED";
    /// The daemon's max live-pane cap (`MAX_DAEMON_PANES`) was hit (decision 4 / panic
    /// containment) — refused BEFORE any worktree op so a flood cannot grow daemon panes.
    pub const CAP_EXCEEDED: &str = "CAP_EXCEEDED";
    /// A reused worktree is DIRTY and `fresh_from_main` was requested. The daemon refuses
    /// the destructive freshen over the wire (decision 5 / C5) and returns this with detail
    /// `UNCOMMITTED_WORK:<id>:<summary>`; the GUI confirms + cleans app-side, then re-`Spawn`s.
    pub const UNCOMMITTED_WORK: &str = "UNCOMMITTED_WORK";
    /// `Close` raced a still-in-flight `Spawn`/reap/close on the same id: the pane is not
    /// (yet) removable, so nothing was closed. Distinct from the idempotent ok("closed") on
    /// an absent id — answering ok here would let the caller drop its anchor while the
    /// in-flight spawn lands a LIVE pane moments later. The caller retries once it settles.
    pub const CLOSE_PENDING: &str = "CLOSE_PENDING";
}

/// MCP runtime config (SIBLING of `state_root`). SAFE defaults: every capability
/// OFF. The socket handler (app side) and the sidecar tools both honor
/// `allow_mutations`; the **app side is the load-bearing enforcement** (a rogue
/// client can dial the socket directly).
///
/// **Two independent, default-OFF gates** guard the optional loopback-HTTP
/// transport (this PR, a DRAFT for security review — not shipped):
/// * `http_enabled` decides whether the app BINDS the loopback-HTTP listener at
///   all. Default `false` ⇒ no socket is opened, nothing is reachable over TCP.
/// * `allow_mutations` (unchanged) still gates every MUTATING op inside
///   `handle_socket_request`, regardless of which transport delivered it.
///
/// A default install is `{ allow_mutations: false, http_enabled: false }`: HTTP
/// off AND mutations off ⇒ a fail-closed nothing-reachable baseline.
///
/// **Token-at-rest is the new same-user boundary for the HTTP path.** Unlike the
/// Unix socket (same-user-safe via peer euid + 0600), loopback TCP has no clean
/// peer-cred on macOS, so the Bearer token IS the gate. It lives in a `0600`
/// file, sibling of `state_root` ([`http_token_path`]) — NOT the macOS Keychain
/// (Keychain ACLs key on code identity, which an ad-hoc-signed app churns per
/// build; deferred / out of scope for this draft). The chosen ephemeral port is
/// published to a second sibling file ([`http_port_path`]) for discovery.
// NOTE: no `Eq` — the `extra` passthrough map holds `serde_json::Value` (floats),
// which is only `PartialEq`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpConfig {
    /// Capability gate: are mutating ops (`send_input` / `focus`) permitted at
    /// all? Default `false` — fail SAFE, never fail open.
    #[serde(default)]
    pub allow_mutations: bool,
    /// EXTERNAL-ORCHESTRATOR gate (#262 unblock): when `true`, a trusted external caller
    /// (not a Coordinator pane) may drive the NARROW visible-pane op subset
    /// (`op_external_orchestrator_allowed`) over the UDS socket — ADDITIVE to the existing
    /// coordinator path, never replacing it. Default `false` (fail safe). Meaningful ONLY
    /// when the caller's pid ancestry resolves to `external_orchestrator_path` AND
    /// `allow_mutations`/`send_input_enabled` are armed. Never opens Delegate/Spawn/lifecycle.
    #[serde(default)]
    pub allow_external_orchestrator: bool,
    /// EXTERNAL-SPAWN gate (#262 ext): when `true`, a pid-pinned external caller (same
    /// `external_orchestrator_path`) may request VISIBLE workspace/pane spawns
    /// (`CreateWorkspace`/`AddPane`). Its OWN opt-in axis, ORTHOGONAL to
    /// `allow_external_orchestrator` (control) — spawning is strictly more powerful (it
    /// opens panes/processes), so it gets its own deliberate flag. Default `false`. Even
    /// when on, the harness allowlist (no bash), trusted-repo check, count cap, and the
    /// frontend human-confirm still apply. File-only; not LLM-settable.
    #[serde(default)]
    pub allow_external_spawn: bool,
    /// EXTERNAL-SPAWN confirm bypass (#262 ext): default `false` ⇒ the Agent Teams app pops a
    /// human Allow/Cancel before creating panes (the out-of-band intent gate the pid-pin can't
    /// give). Set `true` to spawn STRAIGHT THROUGH once `allow_external_spawn` is armed — the
    /// other guards (trusted-repo allowlist, no-bash harness allowlist, count cap, pid-pin,
    /// audit, + the brain's own in-chat confirm) still apply. File-only opt-out.
    #[serde(default)]
    pub external_spawn_no_confirm: bool,
    /// EXTERNAL-SPAWN per-request pane cap (#262 ext): how many panes ONE external
    /// `CreateWorkspace` may open (summed across its specs). Absent → the built-in default
    /// [`EXTERNAL_SPAWN_MAX_PANES`] (8). Read through [`external_spawn_cap`], which CLAMPS
    /// to 1..=16 (16 = the daemon's live-pane ceiling) so a typo can neither zero the spawn
    /// surface nor unbound it. File-only; not LLM-settable.
    #[serde(default)]
    pub external_spawn_max_panes: Option<u32>,
    /// Absolute executable path of the authorized external orchestrator app (e.g. the
    /// GlikaAgents binary). The UDS gate admits an external caller ONLY if the connecting
    /// process's parent-pid ancestry includes a process whose executable path equals this
    /// (kernel-reported, unforgeable). `None`/empty ⇒ no external caller is ever admitted.
    #[serde(default)]
    pub external_orchestrator_path: Option<String>,
    /// ADDITIONAL authorized external orchestrator executables — the same trust class as
    /// `external_orchestrator_path` (e.g. pin BOTH the GlikaAgents Dev and main app
    /// binaries). The gate admits a caller whose pid ancestry matches ANY pinned path;
    /// resolve the merged, deduped set via [`external_orchestrator_pins`], never read the
    /// two fields separately. File-only; not LLM-settable.
    #[serde(default)]
    pub external_orchestrator_paths: Vec<String>,
    /// Transport gate: should the app BIND the opt-in loopback-HTTP listener
    /// (127.0.0.1 only, ephemeral port, Bearer + Origin/Host gated)? Default
    /// `false` — no listener, nothing reachable over TCP. Independent of
    /// `allow_mutations`: even with HTTP bound, mutating ops still pass the
    /// `allow_mutations` gate inside `handle_socket_request`.
    #[serde(default)]
    pub http_enabled: bool,
    /// Autonomy ceiling: 0 = L0 (delegate refused / suggest-only, DEFAULT), >=1 permits
    /// `team_delegate` (FR-13 ladder; MVP only distinguishes 0 vs >=1). Default 0 ⇒ a
    /// fresh install fails closed on autonomy even if allow_mutations is on.
    #[serde(default)]
    pub autonomy_ceiling: u8,
    /// Flywheel WRITE-mode gate (Phase 2): when `true`, delegate/flywheel workers may FIX +
    /// COMMIT code (local only — `git push` stays denied) and the controller folds their
    /// branches into an integration tree + tests it. Default `false` — a fresh install (and an
    /// L1-armed delegate that only granted report-only) NEVER auto-writes code. This is the
    /// explicit, file-only opt-in for the security-review-owed autonomous merge-back: it is
    /// ADDITIVE to the triple-gate, never a bypass (write-mode still requires allow_mutations +
    /// autonomy_ceiling>=1 + the delegate-live build). Not LLM-settable; operator edits the file.
    #[serde(default)]
    pub flywheel_apply: bool,
    /// Flywheel SHIP gate (Phase 3): when `true` AND a write-mode run reaches a CLEAN Pass verdict,
    /// the controller PUSHES the integration branch + opens a PR (never merges). Default `false`,
    /// SEPARATE from `flywheel_apply` ON PURPOSE: the outward push/PR is the highest-consequence
    /// action in the feature, so it gets its own deliberate opt-in. This enables a STAGED
    /// live-verify — prove the contained `flywheel_apply` half (write→commit→fold→test, inspect the
    /// LOCAL integration branch) across runs, THEN flip `flywheel_ship` to allow the push. Both
    /// flags are ADDITIVE to the triple-gate, never a bypass; file-only, not LLM-settable.
    #[serde(default)]
    pub flywheel_ship: bool,
    /// Flywheel AUTONOMOUS-REMEDIATION gate (§6 v2 outer re-coordinate): when `true` AND a
    /// write+ship run lands a FIXABLE `Hold` (a stale base, or an UNVERIFIED suite from a
    /// recoverable/transient cause), the controller runs ONE bounded remediation wave —
    /// re-base the committed worker branches onto a freshly-fetched `origin/main`, re-fold,
    /// re-run the SAME (trust-pinned) authoritative gate, and re-compute the deterministic
    /// `bridge_verdict` — so a HOLD that was held PURELY on a recoverable input can flip to a
    /// genuine PASS and open a PR WITHOUT a human between the HOLD and the PR. This is the
    /// HIGHEST-consequence opt-in in the feature: it lets the orchestrator re-drive the
    /// pipeline toward green on its own. It NEVER weakens the verdict — PASS is reachable ONLY
    /// as the pure `bridge_verdict` return on genuinely re-executed inputs; a `Reject` (real
    /// test failure) and any folded diff touching a security-sensitive surface are HARD-excluded
    /// and can never flip; the PR remains the sole human review/merge boundary (never auto-merge).
    /// Default `false`. ADDITIVE to the WHOLE stack (delegate-live build ∧ allow_mutations ∧
    /// autonomy_ceiling>=1 ∧ flywheel_apply ∧ flywheel_ship), never a bypass; file-only, not
    /// LLM-settable. Re-read each wave so flipping it off (or dialing autonomy→L0) terminates the
    /// loop at the next boundary → terminal HELD.
    #[serde(default)]
    pub flywheel_remediate: bool,
    /// Flywheel EXPECTED-REPO PIN enforcement: when `true`, EVERY autonomous delegate/flywheel run
    /// MUST declare its intended repo with a leading `@repo:<name>` directive in the goal, and the
    /// run is HARD-REFUSED unless that name matches the resolved workspace repo. Prevents "fired the
    /// goal on the wrong repo" accidents (live-fired: a combine() goal meant for one repo ran on
    /// another). Default `false` — the `@repo:` pin is then OPT-IN (a pin, when present, is still
    /// enforced; absence is allowed). Flip this ON to forbid an unpinned autonomous run entirely.
    /// File-only, not LLM-settable.
    #[serde(default)]
    pub flywheel_require_repo_pin: bool,
    /// PRD-FAST timing knob — NOT a capability gate (grants no new power, only shortens a wait).
    /// When `true`, a delegate/flywheel run uses a much shorter per-worker settle deadline
    /// (`DELEGATE_PRD_DEADLINE_SECS`/`DELEGATE_PRD_HARD_CAP_SECS`, ~90s/180s) instead of the
    /// default ~15/45-min budget, so a PRD-build pass of deliberately small, single-file tasks
    /// returns in 1-2 min rather than waiting on the long backstop. Progress-aware extension still
    /// applies — a worker actively streaming at the deadline is NOT killed mid-fix — so this only
    /// tightens the floor for fast tasks; it never truncates a worker that is still producing.
    /// Carries NO security weight (timing only); it lives in this struct solely because
    /// mcp-config.json is the live, operator-editable surface the running app re-reads. Default
    /// `false`.
    #[serde(default)]
    pub flywheel_prd_fast: bool,
    /// Cross-examination CRITIQUE gate: when `true`, a flywheel fan-in runs a fresh-context
    /// cross-domain critique pass over the folded diff (security/perf/tests/contract) and a
    /// blocking finding the test gate didn't cover DOWNGRADES a `Pass` to `Hold`
    /// (`[REQUIRES-CRITIQUE-FIX]`). STRICTER-ONLY by construction — it can only ADD caution, never
    /// forge a pass (the base verdict is still the pure `bridge_verdict` on real tests). Default
    /// `false`: it costs an extra Opus pass and can hold a ship, so it is an explicit opt-in like
    /// the other flywheel gates. File-only, not LLM-settable.
    #[serde(default)]
    pub flywheel_critique: bool,
    /// LOOP scheduler AUTONOMY gate (§4.6): when `true`, the schedulable saved-LOOP system may
    /// AUTO-FIRE an iteration (the scheduler's ability to re-trigger the unified controller without a
    /// human pressing a button each time). Default `false` — a fresh install NEVER auto-fires a loop
    /// even when the rest of the stack is armed. This gates the SCHEDULER's re-trigger authority ONLY;
    /// every in-controller capability (write/ship/remediate) stays enforced by its own gate as today.
    /// Re-read each iteration so flipping it OFF terminates the loop at the next boundary. The durable
    /// (unattended) scheduler tier additionally requires a non-empty repo-pin. File-only, not
    /// LLM-settable; the scheduler itself (timer/launchd) is P4+ — this is the gate it will consult.
    #[serde(default)]
    pub loop_autonomy: bool,
    /// P5 SMART-PR-REVIEW gate (§3.6): when `true`, the fan-in runs an adversarial PR-level reviewer
    /// over the folded diff (a calibrated, self-tested headless pass) and a `REQUEST_CHANGES` verdict
    /// carrying a `major`/`block` finding DOWNGRADES a `Pass` to `Hold` (`[REQUIRES-REVIEW-FIX]`).
    /// STRICTER-ONLY by construction — same monotonicity as `flywheel_critique` (it can only ADD
    /// caution, never forge a pass; the base verdict is still the pure `bridge_verdict` on real tests).
    /// Default `false`: P5 ships the reviewer ADVISORY-FIRST — with the gate OFF the verdict + findings
    /// are LOGGED + rendered but the verdict is UNCHANGED. File-only, not LLM-settable; re-read each
    /// iteration so flipping it off terminates the downgrade at the next boundary.
    #[serde(default)]
    pub flywheel_review: bool,
    /// P5 CRAP-DELTA gate (§3.10): when `true`, a CRAP regression on a touched method (or a NEW method
    /// with CRAP > 30), measured DELTA vs a captured `origin/main` base (NEVER whole-repo, NEVER live
    /// main), DOWNGRADES a `Pass` to `Hold`. STRICTER-ONLY veto downstream of GATE-1 — it can never
    /// flip a test FAIL to pass and is never itself the pass signal. FAIL-SOFT: when the coverage
    /// artifact / CRAP tool is absent the gate degrades to NO veto (today's behavior). Default `false`
    /// — with the gate OFF the crap_delta is LOGGED + fed to the reviewer as evidence but the verdict
    /// is UNCHANGED. File-only, not LLM-settable; re-read each iteration.
    #[serde(default)]
    pub flywheel_crap: bool,
    /// P5 SERENA pre-flight (§3.9-B): when `true`, a per-worker worktree registers ONE project-pinned
    /// serena LSP-MCP server (stdio, dies with the session — NEVER a shared multiplexing server) so the
    /// fix worker can make symbol-accurate edits + the reviewer can pull reference truth. Default
    /// `false` — serena is opt-in (rust-analyzer is RAM-hungry; it ties to the concurrency cap). With
    /// the gate OFF no serena process is ever started. Fail-soft: a missing `uvx` / index failure is
    /// logged + skipped, never fatal. File-only, not LLM-settable.
    #[serde(default)]
    pub serena: bool,
    /// Q4 DAEMON-SPAWN gate (decision 1 / B2): when `true`, the daemon's `handle_spawn`
    /// is permitted to spawn on behalf of the GUI over the `Spawn` wire op. DEFAULT
    /// `false`. This is a SEPARATE axis from `allow_mutations` (which gates WHO may issue
    /// a mutating op) and from the future GUI routing flag — a peer's direct `Spawn`
    /// cannot piggyback on the general mutation toggle. Read FRESH per request inside
    /// `handle_spawn` (exactly like `allow_mutations`), so flipping it OFF refuses the
    /// next spawn. A `Spawn` needs BOTH `allow_mutations=true` AND `daemon_spawn_enabled=true`
    /// (and the compiled-in `daemon-spawn` feature); without all of them it is refused and
    /// spawns nothing. File-only, not LLM-settable; flip ON only after the Q4 security review.
    #[serde(default)]
    pub daemon_spawn_enabled: bool,
    /// Q4 GUI ROUTING flag (Stage 4 / F1): when `true` the APP DIALS the daemon's
    /// `Spawn`/`Close` ops over the UDS instead of spawning the PTY in-process. SEPARATE
    /// axis from `daemon_spawn_enabled` (the DAEMON's own accept-side gate) and from
    /// `allow_mutations` — this flag only steers WHICH side of the socket owns the new pane.
    /// DEFAULT `false` ⇒ `do_spawn` is byte-identical to the app-resident path (the routing
    /// branch is never taken). Read FRESH per spawn (mirrors `allow_mutations`/
    /// `daemon_spawn_enabled`) so flipping it never needs a restart. File-only, not LLM-settable.
    #[serde(default)]
    pub daemon_spawn: bool,
    /// AGENT→AGENT SEND-INPUT gate: when `true`, a COORDINATOR pane's `team_send_input` mutation
    /// (route one line of text into another live pane's PTY) is permitted. SEPARATE, NARROWER axis
    /// than `allow_mutations` ON PURPOSE — arming agent→agent prompting must NOT blanket-enable the
    /// rest of the mutation surface (orchestrate/broadcast/delegate/spawn stay behind
    /// `allow_mutations`). `SendInput` is permitted only when `send_input_enabled=true` AND the
    /// calling pane is `AgentRole::Coordinator` (peer-pid hard gate) AND the target pane is alive.
    /// DEFAULT `false` — a fresh install never lets one agent write into another. Unlike the broad
    /// gate this one IS operator-flippable from the Settings UI (a confirm-gated danger toggle), and
    /// is read FRESH per request in `handle_socket_request` (app AND daemon), so flipping it off
    /// refuses the next call with no respawn. Tool PRESENCE is a separate spawn-time axis (only a
    /// Coordinator pane built with the phase-b sidecar even HAS the tool).
    #[serde(default)]
    pub send_input_enabled: bool,
    /// MEMORY AUTO-CONSULT gate (deterministic prime): when `true`, the app/supervisor
    /// runs `core/memory::search_notes` on a pane's TASK text at dispatch/orchestrate/
    /// worker-spawn and PREPENDS the top matching notes (`recall_block`) to the prompt
    /// before the agent sees it — the store actually feeds the agents instead of
    /// waiting for a tool call. Its OWN axis, ORTHOGONAL to `allow_mutations` (this
    /// only injects READ-ONLY recalled text into an outbound prompt; it never mutates
    /// state or another pane). DEFAULT `false` ⇒ dispatch prompts are byte-identical to
    /// today (no prime). Read FRESH per dispatch so flipping it never needs a restart.
    /// File-only for now (Dev enables it in mcp-config.json); not LLM-settable.
    #[serde(default)]
    pub memory_autoconsult: bool,
    /// POST-RUN KNOWLEDGE HARVEST gate (deterministic lesson extraction): when `true`,
    /// a completed fan-in synthesis (Bridge `bridge_synthesize`, the MCP dir-mode
    /// `socket_synthesize`, the delegate-live verdict path, the sidecar-local
    /// `synthesize_panes_local`) extracts explicitly-marked `LESSON:` lines from the
    /// run's worker reports and writes them into the memory store as notes
    /// (`core/memory::harvest_lessons` + `write_harvested_notes`: max 3/run,
    /// length-guarded, exact-title dedup) — the WRITE side of the flywheel whose READ
    /// side is `memory_autoconsult`. Its OWN axis, ORTHOGONAL to `allow_mutations`
    /// AND to `memory_autoconsult` ON PURPOSE: it WRITES notes (not read-only like
    /// the prime) but never mutates panes/PTY/agent state, and arming recall must not
    /// silently arm store-writes (garbage-in feeds every future dispatch). DEFAULT
    /// `false` ⇒ zero writes and byte-identical synthesis output/acks. Read FRESH per
    /// run so flipping it never needs a restart; harvest never fails or blocks a
    /// synthesis. File-only for now; not LLM-settable.
    #[serde(default)]
    pub memory_harvest: bool,
    /// RUN-OUTCOME CAPTURE gate (deterministic, no-LLM): when `true`, a completed
    /// delegate/synthesize run writes ONE structured note into the GLOBAL memory store
    /// (`core/memory::write_run_capture`: verdict/workspace/harness/held-reason/PR from
    /// the run's `DelegateRunRecord` — never a transcript, never summarized; secret-
    /// scanned; deduped by run_id; linked to goal-relevant prior notes) so ADE learns
    /// run outcomes across workspaces. Its OWN axis, ORTHOGONAL to `allow_mutations`,
    /// `memory_autoconsult` (recall), AND `memory_harvest` (LESSON: lines): this is the
    /// broader WRITE-side capture, and arming recall/harvest must not silently arm it.
    /// DEFAULT `false` ⇒ zero writes, every run's output/ack byte-identical. Read FRESH
    /// per run so flipping it never needs a restart; capture never fails or blocks a
    /// run. File-only for now; not LLM-settable.
    #[serde(default)]
    pub memory_capture: bool,
    /// UNKNOWN-KEY PRESERVATION (Settings RMW safety): every key this build does not
    /// model — a NEWER build's gate, or an unrelated tool's sibling setting kept in the
    /// same file (e.g. `insforge_dashboard`, read as raw JSON elsewhere) — is captured
    /// here on deserialize and re-emitted on serialize. Without this, a typed
    /// read→modify→write of the config (the Settings toggles) silently DELETES other
    /// writers' keys. `#[serde(flatten)]` keeps it invisible on the wire; compatible
    /// with the per-field `#[serde(default)]`s (flatten only collects keys no typed
    /// field consumed).
    #[serde(flatten, default)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

// Written explicitly (not `#[derive(Default)]`) so the SAFE default — every gate
// OFF — is auditable at a glance for the security review, and stays at its locked
// value even if a future field's type default differs. Equivalent to the derive
// today (two `false` fields + a `0` `autonomy_ceiling` are still derivable, so
// clippy fires without the allow); intentional. `autonomy_ceiling: 0` ⇒ a fresh
// install fails closed on autonomous delegation even when `allow_mutations` is on.
#[allow(clippy::derivable_impls)]
impl Default for McpConfig {
    fn default() -> Self {
        Self {
            allow_mutations: false,
            allow_external_orchestrator: false,
            allow_external_spawn: false,
            external_spawn_no_confirm: false,
            external_spawn_max_panes: None,
            external_orchestrator_path: None,
            external_orchestrator_paths: Vec::new(),
            http_enabled: false,
            autonomy_ceiling: 0,
            flywheel_apply: false,
            flywheel_ship: false,
            flywheel_remediate: false,
            flywheel_require_repo_pin: false,
            flywheel_prd_fast: false,
            flywheel_critique: false,
            loop_autonomy: false,
            flywheel_review: false,
            flywheel_crap: false,
            serena: false,
            daemon_spawn_enabled: false,
            daemon_spawn: false,
            send_input_enabled: false,
            memory_autoconsult: false,
            memory_harvest: false,
            memory_capture: false,
            extra: serde_json::Map::new(),
        }
    }
}

/// Read + parse the MCP config, returning the SAFE default ([`McpConfig::default`],
/// mutations OFF) when the file is absent, unreadable, or malformed. NEVER fails
/// open: a parse error yields the locked-down config, not an enabled one.
pub fn read_mcp_config(state_root: &Path) -> McpConfig {
    let Some(path) = mcp_config_path(state_root) else {
        return McpConfig::default();
    };
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|body| serde_json::from_str(&body).ok())
        .unwrap_or_default()
}

/// Read + parse the MCP config for a **read-modify-write** writer, distinguishing
/// "absent" from "present-but-corrupt".
///
/// - file ABSENT (or `state_root` has no config path) ⇒ `Ok(McpConfig::default())`
///   (a first write legitimately creates the file);
/// - file PRESENT and valid ⇒ `Ok(parsed)` (including the `extra` unknown-key
///   carry-through, so the RMW re-emits sibling writers' keys);
/// - file PRESENT but unreadable or unparseable ⇒ `Err(msg)`.
///
/// # Why a checked variant exists alongside [`read_mcp_config`]
/// The infallible [`read_mcp_config`] fails CLOSED (malformed ⇒ locked-down default)
/// — correct for the hot READ/authorization path, where a corrupt file must never
/// fail *open*. But an RMW WRITER (a Settings toggle) that starts from that default
/// would then serialize it back and **overwrite the corrupt file**, ERASING whatever
/// gates + unknown sibling keys it actually held. A writer must instead REFUSE on a
/// malformed file rather than clobber it. This variant surfaces that error so the
/// caller can abort the write and preserve the on-disk bytes.
pub fn read_mcp_config_checked(state_root: &Path) -> Result<McpConfig, String> {
    let Some(path) = mcp_config_path(state_root) else {
        return Ok(McpConfig::default());
    };
    match std::fs::read_to_string(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(McpConfig::default()),
        Err(e) => Err(format!("cannot read MCP config {}: {e}", path.display())),
        Ok(body) => serde_json::from_str::<McpConfig>(&body).map_err(|e| {
            format!(
                "MCP config {} is malformed ({e}); refusing to overwrite it and wipe existing \
                 gates/keys",
                path.display()
            )
        }),
    }
}

// ─────────────── loopback-HTTP transport guards (DRAFT — security review) ───────
//
// Pure, dep-free validators for the OPT-IN loopback-HTTP transport. On the Unix
// socket the same-user boundary is peer euid + 0600; on TCP 127.0.0.1 that euid
// gate is GONE (loopback is not user-scoped, and macOS gives no clean TCP
// peer-cred). So here the **Bearer token + Host/Origin checks ARE the gate** — the
// load-bearing replacement for euid, not belt-and-suspenders. Every helper returns
// `bool` and FAILS CLOSED (missing/malformed/forged ⇒ `false`/reject). No I/O, no
// logging, no token ever crosses a log boundary.

/// `Host`-header allow-check (anti-DNS-rebinding). `true` ONLY when `host_header`
/// is EXACTLY `"127.0.0.1:<bound_port>"` — the app binds 127.0.0.1 only, so the
/// host must match the literal loopback IP and the live port. Missing header, a
/// hostname (`localhost`, any DNS name), a wrong port, or extra cruft ⇒ `false`.
///
/// The asymmetry with [`http_origin_allowed`] (which also allows `localhost`) is
/// INTENTIONAL: `Origin` is set by browsers and a loopback hostname there is benign,
/// but the `Host` we route on must pin to the exact address+port we bound, so a
/// rebound DNS name resolving to 127.0.0.1 can't smuggle a request through.
pub fn http_host_allowed(host_header: Option<&str>, bound_port: u16) -> bool {
    match host_header {
        Some(host) => host == format!("127.0.0.1:{bound_port}"),
        None => false,
    }
}

/// `Origin`-header allow-check (anti-DNS-rebinding for browser callers). `None`
/// ⇒ `true`: non-browser MCP clients send no `Origin`, and we do not want to lock
/// them out. `Some(o)` ⇒ `true` ONLY when `o` is a loopback origin whose host is
/// EXACTLY `127.0.0.1` or `localhost` (after the scheme + optional `:port` are
/// stripped). Any other origin ⇒ `false`.
///
/// Critically this is NOT a `starts_with("http://127.0.0.1")` prefix test — that
/// would accept `http://127.0.0.1.evil.com`, the exact DNS-rebinding origin this
/// guard exists to block. We strip the scheme, cut at the first `/`, drop the
/// `:port` suffix, then require the remaining host to EQUAL a loopback name.
pub fn http_origin_allowed(origin_header: Option<&str>) -> bool {
    let Some(origin) = origin_header else {
        return true; // non-browser client, no Origin → allowed.
    };
    // Strip the scheme (http/https only — any other scheme is rejected).
    let rest = match origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
    {
        Some(r) => r,
        None => return false,
    };
    // The authority is everything up to the first '/' (path), '?' (query) or
    // '#' (fragment). A bare origin usually has none of these.
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    // Drop a userinfo segment if any (`user@host`) — fail closed: a forged
    // `127.0.0.1@evil.com` must NOT be read as host 127.0.0.1.
    if authority.contains('@') {
        return false;
    }
    // Drop the `:port` suffix to get the bare host. `rsplit_once` so an IPv6
    // form (which we don't accept anyway) doesn't get mis-split — but a plain
    // host:port splits cleanly.
    let host = match authority.rsplit_once(':') {
        Some((h, _port)) => h,
        None => authority,
    };
    host == "127.0.0.1" || host == "localhost"
}

/// Bearer-token check for the HTTP transport — the load-bearing same-user gate.
/// Parses `"Bearer <tok>"` (scheme matched case-insensitively per RFC 7235) and
/// compares `<tok>` to `expected` in CONSTANT TIME. Returns `true` ONLY on an
/// exact match.
///
/// Fails closed on every edge: an EMPTY `expected` ⇒ `false` (we never match the
/// empty token, so a missing/empty token file can't authorize anyone); a missing,
/// schemeless, or wrong-scheme header ⇒ `false`. The token is NOT trimmed —
/// `" tok "` must not match `"tok"`.
///
/// **Constant-time:** a length mismatch short-circuits to `false` (length is not
/// secret), but the byte comparison itself XOR-accumulates over ALL bytes with no
/// early exit, so it leaks no information about WHERE a wrong token diverges (no
/// timing oracle). Hand-rolled — this crate adds no constant-time-compare dep.
pub fn bearer_token_matches(auth_header: Option<&str>, expected: &str) -> bool {
    // Never authorize against an empty expected token.
    if expected.is_empty() {
        return false;
    }
    let Some(header) = auth_header else {
        return false;
    };
    // Split "<scheme> <token>" on the FIRST space. No space ⇒ malformed ⇒ reject.
    let Some((scheme, token)) = header.split_once(' ') else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("bearer") {
        return false;
    }
    constant_time_eq(token.as_bytes(), expected.as_bytes())
}

/// Constant-time byte-slice equality. A length difference returns `false` without
/// comparing content (length is not secret here), but equal-length inputs are
/// compared with a full XOR-accumulate (no early exit) so the time taken does not
/// depend on the position of the first differing byte.
///
/// Exposed `pub` for the Phase-12 (D51) sidecar-dial: the client's MAC compare in
/// [`verify_challenge_mac`] reuses THIS exact pattern rather than a second `==` or a
/// new constant-time-compare dep, so the server's [`bearer_token_matches`] and the
/// client's MAC check can never diverge on constant-time semantics.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

// ───────────── Phase-12 (D51) sidecar-dial HMAC-SHA256 challenge SSOT ─────────────
//
// The PURE cryptographic primitive the verify-before-send HTTP dial needs, and the
// SINGLE source both the app-side `identity_challenge` handler and the sidecar client
// will call (no second copy → the two sides cannot disagree on key/MAC semantics).
//
// SCOPE: this is ONLY the compute+verify+derive primitive. There is NO I/O, NO socket,
// NO HTTP listener, NO request handling here — the listener / endpoint / gate-ordering /
// body-buffering surface (H1/H2) is the GATED half and is deliberately NOT written until
// the 12-01-PLAN re-review. See `.paul/analysis/sidecar-dial-security-review.md`.
//
// ── HMAC-KEY / NONCE REPRESENTATION (the pinned SSOT — read this before changing anything)
// The at-rest token is persisted as a 64-char ASCII-hex string (32 CSPRNG bytes,
// `write_token_file_0600`) and read off disk `.trim()`-ed (`read_http_token`). The HMAC
// here is keyed by the **32 HEX-DECODED bytes** of that token — NOT the 64 ASCII-hex
// chars — and the message is the **32 HEX-DECODED bytes** of the challenge nonce — NOT
// the 64 ASCII-hex chars. This matches the threat model (§3.2: "key = hex-decoded token
// bytes", "msg = hex-decoded nonce") and PLAN <architecture> step 3. BOTH sides MUST use
// THIS function so the representation can never silently diverge — a mismatch fails
// CLOSED (MACs never match ⇒ every dial aborts), so it is a correctness/interop blocker,
// not a security hole, but it WOULD break every dial. The `derive_hmac_key`/
// `compute_challenge_mac` token→key→MAC known-answer test PINS this representation; the
// RFC-4231 vectors PIN the HMAC math itself.

/// The challenge nonce is exactly 32 bytes, carried on the wire as a 64-char
/// lowercase-or-uppercase ASCII-hex string. (The MAC output is likewise 32 bytes /
/// 64 hex chars — HMAC-SHA256 is a 256-bit tag.)
pub const CHALLENGE_NONCE_BYTES: usize = 32;
/// The hex-string length of a [`CHALLENGE_NONCE_BYTES`]-byte value (and of the MAC).
pub const CHALLENGE_HEX_LEN: usize = CHALLENGE_NONCE_BYTES * 2; // 64

type HmacSha256 = hmac::Hmac<sha2::Sha256>;

/// Derive the HMAC key from the on-disk token string — the pinned SSOT representation.
///
/// The token is the 64-char ASCII-hex contents of `agent-teams-mcp-http.token`
/// (already `.trim()`-ed by `read_http_token`). The key is its **32 hex-decoded
/// bytes**. Returns `None` (fail CLOSED) on an empty token, a non-hex token, or any
/// token that does not decode to exactly [`CHALLENGE_NONCE_BYTES`] bytes — never a
/// partial/zero key.
///
/// This is the ONE place the token→key representation is defined; app server and
/// sidecar client MUST both derive their key here.
pub fn derive_hmac_key(token: &str) -> Option<[u8; CHALLENGE_NONCE_BYTES]> {
    // Mirror `bearer_token_matches`' empty-expected rule: never key off an empty token.
    if token.is_empty() {
        return None;
    }
    let bytes = hex::decode(token).ok()?;
    bytes.try_into().ok()
}

/// Validate a challenge nonce's WIRE shape (before it is used as a MAC message):
/// exactly [`CHALLENGE_HEX_LEN`] (64) ASCII-hex chars, no interior whitespace, decodes
/// to exactly [`CHALLENGE_NONCE_BYTES`] (32) bytes. Returns the decoded bytes on
/// success, `None` (reject) otherwise. Fails CLOSED on length, non-hex, or any decode
/// error — so a malformed-nonce challenge can never reach the MAC computation.
pub fn decode_challenge_nonce(nonce_hex: &str) -> Option<[u8; CHALLENGE_NONCE_BYTES]> {
    // Strict length FIRST (length is not secret) — `hex::decode` would otherwise accept
    // any even-length hex string; we require EXACTLY the 64-char nonce shape.
    if nonce_hex.len() != CHALLENGE_HEX_LEN {
        return None;
    }
    let bytes = hex::decode(nonce_hex).ok()?;
    bytes.try_into().ok()
}

/// Compute the challenge MAC: `HMAC-SHA256(key = derive_hmac_key(token), msg = nonce
/// bytes)`, returned as the 64-char lowercase-hex tag the wire carries.
///
/// `nonce_hex` must be a valid 64-hex nonce ([`decode_challenge_nonce`]); an invalid
/// nonce or an underivable key ⇒ `None` (fail CLOSED — no MAC over a bad input). Both
/// the app server (answering a challenge) and the sidecar client (computing the
/// expected MAC) call THIS — there is no second MAC path.
pub fn compute_challenge_mac(token: &str, nonce_hex: &str) -> Option<String> {
    let key = derive_hmac_key(token)?;
    let msg = decode_challenge_nonce(nonce_hex)?;
    // `Hmac::new_from_slice` only errors on a key-length constraint HMAC itself does not
    // impose (it accepts any key length), so this is infallible for our fixed-size key;
    // map to None defensively rather than unwrap.
    let mut mac = <HmacSha256 as hmac::Mac>::new_from_slice(&key).ok()?;
    hmac::Mac::update(&mut mac, &msg);
    let tag = hmac::Mac::finalize(mac).into_bytes();
    Some(hex::encode(tag))
}

/// Constant-time verify of a server-returned MAC against the locally-recomputed
/// expected MAC for `(token, nonce)`. This is the client's decision point in
/// verify-before-send: it returns `true` ONLY when the response MAC is present,
/// EXACTLY [`CHALLENGE_HEX_LEN`] (64) hex chars, and constant-time-equal to the MAC
/// this side computes — the MAC match is the decision, NOT an `ok:true` flag.
///
/// Fails CLOSED on: a `response_mac` that is not exactly 64 hex chars (missing/empty/
/// short/long/non-hex all reject BEFORE the compare — this is the H4 / MAC-field
/// strictness rule baked into the primitive), an underivable key, or an invalid nonce.
/// The byte compare reuses [`constant_time_eq`] (no `==` on MAC bytes), so it shares the
/// server's `bearer_token_matches` constant-time semantics.
pub fn verify_challenge_mac(token: &str, nonce_hex: &str, response_mac: &str) -> bool {
    // Strict MAC-field shape FIRST: exactly 64 hex chars. Reject empty/short/long/non-hex
    // before any compare so a squatter cannot pass with a missing or truncated `mac`.
    if response_mac.len() != CHALLENGE_HEX_LEN {
        return false;
    }
    let Ok(response_bytes) = hex::decode(response_mac) else {
        return false;
    };
    let Some(expected_hex) = compute_challenge_mac(token, nonce_hex) else {
        return false;
    };
    // `compute_challenge_mac` always returns a 64-hex string, so this decode is infallible
    // here; both sides are raw 32-byte tags for the constant-time compare.
    let Ok(expected_bytes) = hex::decode(&expected_hex) else {
        return false;
    };
    constant_time_eq(&expected_bytes, &response_bytes)
}

// ─────────────── Phase-12 (D51) identity-challenge wire types ───────────────
//
// TRANSPORT-LEVEL handshake types for the verify-before-send HTTP dial. These are
// the on-wire JSON for the pre-Bearer `identity_challenge` exchange — NOT a
// `SocketRequest` variant: the challenge terminates INSIDE `serve_http_request`
// (it never reaches `handle_socket_request`) and is meaningless on the Unix socket
// (which has a euid gate). Both the app server (answering) and the sidecar client
// (verifying) import THESE one definitions so the wire form cannot drift.

/// The sidecar's pre-Bearer challenge body: `{"op":"identity_challenge","nonce":"<64-hex>"}`.
/// The app server parses this AFTER the Host+Origin checks and BEFORE the Bearer gate;
/// only an exact `identity_challenge` op carrying a valid 64-hex `nonce` takes the
/// MAC-answer path (everything else falls through to the Bearer gate). `nonce` is a
/// 64-char ASCII-hex string (32 random bytes); a malformed nonce is rejected by
/// [`decode_challenge_nonce`] and never reaches the MAC computation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityChallenge {
    /// The fresh per-dial nonce, 64 ASCII-hex chars ([`CHALLENGE_HEX_LEN`]).
    pub nonce: String,
}

/// The app server's challenge proof: `{"ok":true,"mac":"<64-hex>"}`. The client decides
/// on the **MAC match, NEVER on `ok`** — `mac` is a NON-OPTIONAL `String` (never
/// `Option`) ON PURPOSE: an absent `mac` must reach the strict 64-hex reject in
/// [`verify_challenge_mac`], not default to a bypass. `mac = compute_challenge_mac(token,
/// nonce)` (the pinned core SSOT). `ok` is advisory only — the client still recomputes
/// and constant-time-compares the MAC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityProof {
    /// Advisory success flag — NOT the decision (the client decides on the MAC match).
    pub ok: bool,
    /// The server's HMAC-SHA256 tag, 64 ASCII-hex chars. Non-optional so an absent
    /// `mac` hits the strict reject rather than defaulting to a bypass.
    pub mac: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    /// A unique scratch dir, cleaned at drop. The state dir is *nested* under a
    /// private root (`<root>/state`) so the live registry — a SIBLING of the
    /// state dir (`<root>/agent-teams-live.json`) — is isolated per test rather
    /// than colliding in the shared system temp dir. No `notify`/serde needed for
    /// events — we write the same fixed JSONL shape `state-writer.sh` emits.
    struct Scratch {
        root: PathBuf,
        state: PathBuf,
    }
    impl Scratch {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!("at-mcp-{}-{}", tag, std::process::id()));
            let _ = fs::remove_dir_all(&root);
            let state = root.join("state");
            fs::create_dir_all(&state).unwrap();
            Scratch { root, state }
        }
        /// Write `<state>/<id>/events.jsonl` with a single event line.
        fn workspace(&self, id: &str, line: &str) {
            let wsdir = self.state.join(id);
            fs::create_dir_all(&wsdir).unwrap();
            fs::write(wsdir.join("events.jsonl"), format!("{line}\n")).unwrap();
        }
        /// Write the live registry (sibling of the state dir) with raw JSON.
        fn write_registry(&self, json: &str) {
            fs::write(registry_path(&self.state).unwrap(), json).unwrap();
        }
        /// Remove the live registry (simulate app-down / no liveness info).
        fn remove_registry(&self) {
            let _ = fs::remove_file(registry_path(&self.state).unwrap());
        }
        fn path(&self) -> &Path {
            &self.state
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn projects_corrected_wire_strings_and_since() {
        let s = Scratch::new("wire");
        // claude PermissionRequest → waiting/approval/needs_human, since = ts.
        s.workspace(
            "blocked",
            r#"{"harness":"claude","event":"PermissionRequest","ts":1000}"#,
        );
        let rows = compute_queue(s.path(), None);
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(r.id, "blocked");
        assert_eq!(r.harness, "claude");
        assert_eq!(r.state, "waiting");
        assert_eq!(r.reason.as_deref(), Some("approval"));
        assert!(r.needs_human);
        assert_eq!(r.since, 1000);
    }

    #[test]
    fn turn_end_uses_underscore_form_not_the_lib_bug() {
        let s = Scratch::new("turnend");
        // cursor stop → done/turn_end (the wire form, NOT `turnend`).
        s.workspace(
            "finished",
            r#"{"harness":"cursor","event":"stop","ts":2000}"#,
        );
        let row = get_workspace(s.path(), "finished").expect("row");
        assert_eq!(row.state, "done");
        assert_eq!(row.reason.as_deref(), Some("turn_end"));
        assert!(!row.needs_human);
    }

    #[test]
    fn error_state_projects_the_error_wire_string_with_null_reason() {
        // claude StopFailure → error/None. State::Error is a REACHABLE terminal state
        // (the only one besides waiting/done surfaced to MCP), and "error" was the last
        // reachable-state wire string never executed — same bug class the turn_end /
        // rate_limit tests guard (a typo "err"/"errored" would ship silently). Also pins
        // reason_str(None) → JSON null (not the table's "-" placeholder).
        let s = Scratch::new("errorstate");
        s.workspace(
            "failed",
            r#"{"harness":"claude","event":"StopFailure","ts":3000}"#,
        );
        let row = get_workspace(s.path(), "failed").expect("row");
        assert_eq!(row.state, "error");
        assert!(
            row.reason.is_none(),
            "an error carries no waiting-reason → null"
        );
        assert!(!row.needs_human);
        assert_eq!(row.since, 3000);
    }

    #[test]
    fn rate_limit_projects_underscore_form_and_is_waiting_without_needs_human() {
        // cursor resource_exhausted → waiting/rate_limit (the underscore wire form, NOT
        // the app's off-spec `ratelimit`). The ONLY projected row that is state="waiting"
        // yet needs_human=false — proving a waiting row does not always need a human.
        let s = Scratch::new("ratelimit");
        s.workspace(
            "throttled",
            r#"{"harness":"cursor","event":"resource_exhausted","ts":2500}"#,
        );
        let row = get_workspace(s.path(), "throttled").expect("row");
        assert_eq!(row.state, "waiting");
        assert_eq!(row.reason.as_deref(), Some("rate_limit"));
        assert!(!row.needs_human);
        assert_eq!(row.since, 2500);
    }

    #[test]
    fn ranks_needs_human_first_via_single_source_rank() {
        let s = Scratch::new("rank");
        s.workspace("idle", r#"{"harness":"cursor","event":"stop","ts":500}"#);
        s.workspace(
            "needsme",
            r#"{"harness":"claude","event":"PermissionRequest","ts":900}"#,
        );
        let rows = compute_queue(s.path(), None);
        assert_eq!(rows.len(), 2);
        // needs_human ranks first regardless of insertion / dir order.
        assert_eq!(rows[0].id, "needsme");
        assert!(rows[0].needs_human);
        assert!(!rows[1].needs_human);
    }

    #[test]
    fn live_filter_drops_stale_workspaces() {
        let s = Scratch::new("live");
        s.workspace(
            "alive",
            r#"{"harness":"claude","event":"SessionStart","ts":1}"#,
        );
        s.workspace(
            "stale",
            r#"{"harness":"claude","event":"SessionStart","ts":2}"#,
        );
        let live: HashSet<String> = ["alive".to_string()].into_iter().collect();
        let rows = compute_queue(s.path(), Some(&live));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "alive");
    }

    // ───────────── identity-on-rows (gap #4): role / tag / workspace ─────────────

    #[test]
    fn ws_prefix_extracts_workspace_id_and_passes_through_odd_shapes() {
        assert_eq!(ws_prefix("ws76101x0-p0"), "ws76101x0");
        assert_eq!(ws_prefix("ws76101x0-p12"), "ws76101x0");
        assert_eq!(ws_prefix("noPtail"), "noPtail");
    }

    #[test]
    fn rows_carry_workspace_prefix_and_omit_unset_role_tag_on_the_wire() {
        // AC (serde-additive): a pane spawned BEFORE role/tag existed produces a row
        // whose JSON has `workspace` (derived from the id) but NO `role`/`tag` keys.
        let s = Scratch::new("identity-derive");
        s.workspace(
            "ws50144x0-p4",
            r#"{"harness":"claude","event":"SessionStart","ts":1}"#,
        );
        let rows = compute_queue(s.path(), None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].workspace.as_deref(), Some("ws50144x0"));
        assert!(rows[0].role.is_none());
        assert!(rows[0].tag.is_none());
        let json = serde_json::to_string(&rows[0]).unwrap();
        assert!(json.contains(r#""workspace":"ws50144x0""#));
        assert!(
            !json.contains(r#""role""#),
            "unset role must be OMITTED, not null"
        );
        assert!(
            !json.contains(r#""tag""#),
            "unset tag must be OMITTED, not null"
        );
    }

    #[test]
    fn row_serializes_role_tag_workspace_when_set() {
        let row = QueueRow {
            id: "ws1x0-p0".into(),
            harness: "claude".into(),
            state: "working".into(),
            reason: None,
            needs_human: false,
            since: 7,
            role: Some("coordinator".into()),
            tag: Some("team-a".into()),
            workspace: Some("ws1x0".into()),
        };
        let json = serde_json::to_string(&row).unwrap();
        assert!(json.contains(r#""role":"coordinator""#));
        assert!(json.contains(r#""tag":"team-a""#));
        assert!(json.contains(r#""workspace":"ws1x0""#));
    }

    #[test]
    fn compute_queue_identified_joins_role_tag_from_registry() {
        // p0 has role+tag in the registry (recorded at spawn); p1 is a LEGACY entry
        // (id only — the pre-identity writer shape) → its row keeps None/None. Both
        // rows still carry the id-derived `workspace`.
        let s = Scratch::new("identity-join");
        s.workspace(
            "ws1x0-p0",
            r#"{"harness":"claude","event":"SessionStart","ts":1}"#,
        );
        s.workspace(
            "ws1x0-p1",
            r#"{"harness":"claude","event":"SessionStart","ts":2}"#,
        );
        s.workspace(
            "ws9x9-p0",
            r#"{"harness":"claude","event":"SessionStart","ts":3}"#,
        );
        s.write_registry(
            r#"{"schema":1,"workspaces":[
                {"id":"ws1x0-p0","role":"coordinator","tag":"team-a"},
                {"id":"ws1x0-p1"}
            ]}"#,
        );
        let reg = read_registry(s.path()).expect("registry parses");
        let rows = compute_queue_identified(s.path(), Some(&reg));
        // registry present ⇒ live filter: the un-registered ws9x9-p0 is dropped.
        assert_eq!(rows.len(), 2);
        let p0 = rows.iter().find(|r| r.id == "ws1x0-p0").expect("p0 row");
        assert_eq!(p0.role.as_deref(), Some("coordinator"));
        assert_eq!(p0.tag.as_deref(), Some("team-a"));
        assert_eq!(p0.workspace.as_deref(), Some("ws1x0"));
        let p1 = rows.iter().find(|r| r.id == "ws1x0-p1").expect("p1 row");
        assert!(p1.role.is_none(), "legacy registry entry → role stays None");
        assert!(p1.tag.is_none(), "legacy registry entry → tag stays None");
        assert_eq!(p1.workspace.as_deref(), Some("ws1x0"));
        // registry absent (app-down) ⇒ superset, identity limited to `workspace`.
        let all = compute_queue_identified(s.path(), None);
        assert_eq!(all.len(), 3);
        assert!(all.iter().all(|r| r.role.is_none() && r.tag.is_none()));
    }

    #[test]
    fn live_workspace_role_tag_roundtrip_and_legacy_registry_still_parses() {
        // Roundtrip WITH the new fields.
        let w = LiveWorkspace {
            id: "ws1x0-p0".into(),
            pid: None,
            harness: Some("claude".into()),
            repo: None,
            role: Some("scout".into()),
            tag: Some("audit-run".into()),
            session_id: None,
            spawned_at: None,
        };
        let json = serde_json::to_string(&w).unwrap();
        assert!(json.contains(r#""role":"scout""#));
        assert!(json.contains(r#""tag":"audit-run""#));
        let back: LiveWorkspace = serde_json::from_str(&json).unwrap();
        assert_eq!(back, w);
        // A PRE-identity registry line (no role/tag keys) parses with None defaults —
        // the serde-additive requirement for panes spawned before this change.
        let legacy: LiveWorkspace =
            serde_json::from_str(r#"{"id":"old-pane","pid":7,"harness":"cursor"}"#).unwrap();
        assert_eq!(legacy.id, "old-pane");
        assert!(legacy.role.is_none());
        assert!(legacy.tag.is_none());
        // ...and a None role/tag entry serializes WITHOUT the keys (wire unchanged
        // for older panes).
        let none_json = serde_json::to_string(&legacy).unwrap();
        assert!(!none_json.contains(r#""role""#));
        assert!(!none_json.contains(r#""tag""#));
    }

    #[test]
    fn list_workspaces_is_sorted_and_get_unknown_is_none() {
        let s = Scratch::new("list");
        s.workspace(
            "bbb",
            r#"{"harness":"claude","event":"SessionStart","ts":1}"#,
        );
        s.workspace(
            "aaa",
            r#"{"harness":"claude","event":"SessionStart","ts":1}"#,
        );
        assert_eq!(list_workspaces(s.path()), vec!["aaa", "bbb"]);
        assert!(get_workspace(s.path(), "nope").is_none());
    }

    /// A workspace dir with an EMPTY or MALFORMED `events.jsonl` is still
    /// discovered by `list_workspaces` (discover only checks the file exists), but
    /// `get_workspace`/`compute_queue` drop it (no parseable latest state). This is
    /// the documented divergence (lib.rs `get_workspace` doc) — previously only the
    /// no-dir-at-all path was tested.
    #[test]
    fn empty_or_malformed_events_listed_but_not_projected() {
        let s = Scratch::new("illformed");
        s.workspace(
            "good",
            r#"{"harness":"claude","event":"SessionStart","ts":1}"#,
        );
        s.workspace("empty", ""); // events.jsonl with no non-empty line
        s.workspace("garbage", "{ not json"); // unparseable; no harness field
                                              // discover includes ANY dir with an events.jsonl → all three, sorted.
        assert_eq!(list_workspaces(s.path()), vec!["empty", "garbage", "good"]);
        // but only the parseable one projects a row.
        assert!(get_workspace(s.path(), "good").is_some());
        assert!(get_workspace(s.path(), "empty").is_none());
        assert!(get_workspace(s.path(), "garbage").is_none());
        let rows = compute_queue(s.path(), None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "good");
    }

    #[test]
    fn registry_path_is_sibling_of_state_root() {
        let p = registry_path(Path::new("/var/app/agent-teams")).unwrap();
        assert_eq!(p, PathBuf::from("/var/app/agent-teams-live.json"));
        // Root has no parent → None (never panics).
        assert!(registry_path(Path::new("/")).is_none());
    }

    #[test]
    fn read_registry_absent_is_none_present_parses_live_ids() {
        let s = Scratch::new("registry");
        // Absent → app-down signal.
        assert!(read_registry(s.path()).is_none());
        // A minimal writer may emit just {schema, workspaces:[{id}]}.
        s.write_registry(
            r#"{"schema":1,"app_pid":4242,"workspaces":[{"id":"alive","pid":5001,"harness":"claude"}]}"#,
        );
        let reg = read_registry(s.path()).expect("present registry parses");
        assert_eq!(reg.schema, LIVE_REGISTRY_SCHEMA);
        assert_eq!(reg.app_pid, Some(4242));
        let ids = reg.live_ids();
        assert!(ids.contains("alive"));
        assert_eq!(ids.len(), 1);
        // Malformed → None (lenient app-down fallback, never panics).
        s.write_registry("{ not json");
        assert!(read_registry(s.path()).is_none());
    }

    #[test]
    fn registry_roundtrips_and_tolerates_unknown_fields() {
        // Forward-compat: an extra field a newer writer adds is ignored.
        let s = Scratch::new("registry-fwd");
        s.write_registry(
            r#"{"schema":2,"app_pid":1,"updated_at":99,"future_field":true,
                "workspaces":[{"id":"w","repo":"/r","spawned_at":7,"extra":1}]}"#,
        );
        let reg = read_registry(s.path()).expect("parses despite extras + newer schema");
        assert_eq!(reg.schema, 2);
        assert_eq!(reg.workspaces[0].repo.as_deref(), Some("/r"));
        // Serialize round-trips through the typed form.
        let json = serde_json::to_string(&reg).unwrap();
        let back: LiveRegistry = serde_json::from_str(&json).unwrap();
        assert_eq!(back, reg);
        s.remove_registry();
        assert!(read_registry(s.path()).is_none());
    }

    #[test]
    fn registry_active_field_roundtrips_and_is_omitted_when_none() {
        // The frontend's active-workspace prefix parses + round-trips (the contract
        // GlikaAgents' live block + Team-Grid depend on).
        let with_active: LiveRegistry =
            serde_json::from_str(r#"{"schema":1,"active":"ws50144x0","workspaces":[]}"#).unwrap();
        assert_eq!(with_active.active.as_deref(), Some("ws50144x0"));
        let back: LiveRegistry =
            serde_json::from_str(&serde_json::to_string(&with_active).unwrap()).unwrap();
        assert_eq!(back, with_active);
        // Absent → None; and None is skipped on serialize (no `"active":null` noise).
        let no_active: LiveRegistry =
            serde_json::from_str(r#"{"schema":1,"workspaces":[]}"#).unwrap();
        assert!(no_active.active.is_none());
        assert!(!serde_json::to_string(&no_active)
            .unwrap()
            .contains("active"));
    }

    // ───────────────── Phase-B mutation seam shared types (06-02) ───────────────

    #[test]
    fn socket_path_and_config_path_are_siblings_of_state_root() {
        assert_eq!(
            socket_path(Path::new("/var/app/agent-teams")),
            Some(PathBuf::from("/var/app/agent-teams-mcp.sock"))
        );
        assert_eq!(
            mcp_config_path(Path::new("/var/app/agent-teams")),
            Some(PathBuf::from("/var/app/mcp-config.json"))
        );
        // No parent ⇒ no path (mirrors registry_path's None case; never panics).
        assert_eq!(socket_path(Path::new("/")), None);
        assert_eq!(mcp_config_path(Path::new("/")), None);
    }

    #[test]
    fn socket_wire_protocol_serializes_to_the_exact_contract() {
        // send_input → {"op":"send_input","id":..,"text":..}
        let s = serde_json::to_string(&SocketRequest::SendInput {
            id: "w1".into(),
            text: "approve".into(),
        })
        .unwrap();
        assert_eq!(s, r#"{"op":"send_input","id":"w1","text":"approve"}"#);
        // focus → {"op":"focus","id":..}
        let f = serde_json::to_string(&SocketRequest::Focus { id: "w1".into() }).unwrap();
        assert_eq!(f, r#"{"op":"focus","id":"w1"}"#);
        // round-trips
        assert_eq!(
            serde_json::from_str::<SocketRequest>(&s).unwrap(),
            SocketRequest::SendInput {
                id: "w1".into(),
                text: "approve".into()
            }
        );
        // response constructors carry the canonical codes
        assert_eq!(SocketResponse::ok("done").code, response_code::OK);
        assert!(SocketResponse::ok("done").ok);
        let e = SocketResponse::err(response_code::DEAD_PANE, "x");
        assert_eq!(e.code, "DEAD_PANE");
        assert!(!e.ok);
    }

    // ───────────────── Context Router wire types (06-03) ───────────────────────

    #[test]
    fn context_router_ops_round_trip_through_the_exact_wire_contract() {
        // orchestrate → {"op":"orchestrate","goal":..,"dispatch":..}
        let o = serde_json::to_string(&SocketRequest::Orchestrate {
            goal: "ship the login page".into(),
            dispatch: false,
            target_workspace: None,
        })
        .unwrap();
        // target_workspace: None is skip_serializing_if → the wire stays byte-identical.
        assert_eq!(
            o,
            r#"{"op":"orchestrate","goal":"ship the login page","dispatch":false}"#
        );
        assert_eq!(
            serde_json::from_str::<SocketRequest>(&o).unwrap(),
            SocketRequest::Orchestrate {
                goal: "ship the login page".into(),
                dispatch: false,
                target_workspace: None,
            }
        );
        // broadcast → {"op":"broadcast","text":..}
        let b = serde_json::to_string(&SocketRequest::Broadcast {
            text: "status?".into(),
        })
        .unwrap();
        assert_eq!(b, r#"{"op":"broadcast","text":"status?"}"#);
        assert_eq!(
            serde_json::from_str::<SocketRequest>(&b).unwrap(),
            SocketRequest::Broadcast {
                text: "status?".into()
            }
        );
        // handoff → {"op":"handoff","from":..,"to":..,"instruction":..}
        let h = serde_json::to_string(&SocketRequest::Handoff {
            from: "a1".into(),
            to: "b2".into(),
            instruction: "wire it up".into(),
        })
        .unwrap();
        assert_eq!(
            h,
            r#"{"op":"handoff","from":"a1","to":"b2","instruction":"wire it up"}"#
        );
        assert_eq!(
            serde_json::from_str::<SocketRequest>(&h).unwrap(),
            SocketRequest::Handoff {
                from: "a1".into(),
                to: "b2".into(),
                instruction: "wire it up".into()
            }
        );
    }

    // ───────────── role-inversion read ops + write-path defenses (08 Sub-build 3) ─────────────

    #[test]
    fn role_inversion_read_ops_round_trip_by_tag() {
        // ListLive → {"op":"list_live"} (unit variant, no fields).
        let l = serde_json::to_string(&SocketRequest::ListLive).unwrap();
        assert_eq!(l, r#"{"op":"list_live"}"#);
        assert_eq!(
            serde_json::from_str::<SocketRequest>(&l).unwrap(),
            SocketRequest::ListLive
        );
        // Attach → {"op":"attach","id":..}
        let a = serde_json::to_string(&SocketRequest::Attach { id: "w1".into() }).unwrap();
        assert_eq!(a, r#"{"op":"attach","id":"w1"}"#);
        assert_eq!(
            serde_json::from_str::<SocketRequest>(&a).unwrap(),
            SocketRequest::Attach { id: "w1".into() }
        );
        // Detach → {"op":"detach","id":..}
        let d = serde_json::to_string(&SocketRequest::Detach { id: "w1".into() }).unwrap();
        assert_eq!(d, r#"{"op":"detach","id":"w1"}"#);
        assert_eq!(
            serde_json::from_str::<SocketRequest>(&d).unwrap(),
            SocketRequest::Detach { id: "w1".into() }
        );
    }

    #[test]
    fn live_panes_payload_round_trips_with_and_without_workspaces() {
        // Minimal reply: ids only, `workspaces` omitted (serde default → no key on wire).
        let minimal = SocketResponse::ok("live").with_data(SocketData::LivePanes {
            ids: vec!["a".into(), "b".into()],
            workspaces: None,
        });
        let s = serde_json::to_string(&minimal).unwrap();
        assert!(s.contains(r#""kind":"live_panes""#));
        assert!(
            !s.contains("workspaces"),
            "None workspaces is skipped on the wire"
        );
        assert_eq!(serde_json::from_str::<SocketResponse>(&s).unwrap(), minimal);
        // Full reply: ids + the LiveWorkspace metadata rows.
        let full = SocketResponse::ok("live").with_data(SocketData::LivePanes {
            ids: vec!["a".into()],
            workspaces: Some(vec![LiveWorkspace {
                id: "a".into(),
                pid: Some(42),
                harness: Some("claude".into()),
                repo: None,
                role: None,
                tag: None,
                session_id: None,
                spawned_at: None,
            }]),
        });
        let s = serde_json::to_string(&full).unwrap();
        let back: SocketResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(back, full);
        match back.data.unwrap() {
            SocketData::LivePanes { ids, workspaces } => {
                assert_eq!(ids, vec!["a".to_string()]);
                assert_eq!(workspaces.unwrap()[0].harness.as_deref(), Some("claude"));
            }
            other => panic!("expected LivePanes, got {other:?}"),
        }
    }

    #[test]
    fn new_response_codes_have_their_exact_strings() {
        assert_eq!(response_code::STREAMING, "STREAMING");
        assert_eq!(response_code::PANE_DIED, "PANE_DIED");
    }

    #[test]
    fn op_requires_mutations_false_for_read_ops_true_for_mutations() {
        // Read ops (MF-D): euid-gated only, no allow_mutations gate.
        assert!(!op_requires_mutations(&SocketRequest::ListLive));
        assert!(!op_requires_mutations(&SocketRequest::Attach {
            id: "w".into()
        }));
        assert!(!op_requires_mutations(&SocketRequest::Detach {
            id: "w".into()
        }));
        // gap-7: the live-scrollback read is a READ op (never behind allow_mutations);
        // its content-exfil admission is layered app-side (coordinator OR external gate).
        assert!(!op_requires_mutations(&SocketRequest::ReadOutput {
            id: "w".into(),
            max_bytes: None
        }));
        // Mutating ops: behind allow_mutations.
        assert!(op_requires_mutations(&SocketRequest::SendInput {
            id: "w".into(),
            text: "y".into()
        }));
        assert!(op_requires_mutations(&SocketRequest::Focus {
            id: "w".into()
        }));
        assert!(op_requires_mutations(&SocketRequest::Orchestrate {
            goal: "g".into(),
            dispatch: false,
            target_workspace: None
        }));
        assert!(op_requires_mutations(&SocketRequest::Broadcast {
            text: "t".into()
        }));
        assert!(op_requires_mutations(&SocketRequest::Handoff {
            from: "a".into(),
            to: "b".into(),
            instruction: "i".into()
        }));
        assert!(op_requires_mutations(&SocketRequest::Synthesize {
            dir: "d".into(),
            goal: "g".into()
        }));
        assert!(op_requires_mutations(&SocketRequest::Delegate {
            parent_id: "p".into(),
            goal: "g".into(),
            max_workers: 1,
            depth: 1
        }));
    }

    #[test]
    fn op_external_orchestrator_allowed_is_visible_pane_subset_only() {
        // The trusted-external subset: prompt one pane / broadcast / orchestrate / focus.
        assert!(op_external_orchestrator_allowed(
            &SocketRequest::SendInput {
                id: "w".into(),
                text: "y".into()
            }
        ));
        assert!(op_external_orchestrator_allowed(
            &SocketRequest::Broadcast { text: "t".into() }
        ));
        assert!(op_external_orchestrator_allowed(
            &SocketRequest::Orchestrate {
                goal: "g".into(),
                dispatch: false,
                target_workspace: None
            }
        ));
        assert!(op_external_orchestrator_allowed(
            &SocketRequest::Orchestrate {
                goal: "g".into(),
                dispatch: true,
                target_workspace: None
            }
        ));
        assert!(op_external_orchestrator_allowed(&SocketRequest::Focus {
            id: "w".into()
        }));
        // gap-7: the live-scrollback READ is in the subset — the external brain must be
        // able to read what a state-blind pane produced, gated exactly like the control ops.
        assert!(op_external_orchestrator_allowed(
            &SocketRequest::ReadOutput {
                id: "w".into(),
                max_bytes: None
            }
        ));
        assert!(op_external_orchestrator_allowed(
            &SocketRequest::ReadOutput {
                id: "w".into(),
                max_bytes: Some(1024)
            }
        ));
        // Autonomous / multi-hop / lifecycle are EXCLUDED — they stay coordinator-only.
        assert!(!op_external_orchestrator_allowed(&SocketRequest::Handoff {
            from: "a".into(),
            to: "b".into(),
            instruction: "i".into()
        }));
        assert!(!op_external_orchestrator_allowed(
            &SocketRequest::Synthesize {
                dir: "d".into(),
                goal: "g".into()
            }
        ));
        assert!(!op_external_orchestrator_allowed(
            &SocketRequest::Delegate {
                parent_id: "p".into(),
                goal: "g".into(),
                max_workers: 1,
                depth: 1
            }
        ));
        assert!(!op_external_orchestrator_allowed(&SocketRequest::ListLive));
    }

    #[test]
    fn read_output_wire_shape_is_additive_and_backward_compatible() {
        // Omitted max_bytes ⇒ None and NOT re-serialized (the serde-additive contract
        // every optional field in this enum follows).
        let req: SocketRequest =
            serde_json::from_str(r#"{"op":"read_output","id":"ws9-p4"}"#).unwrap();
        assert_eq!(
            req,
            SocketRequest::ReadOutput {
                id: "ws9-p4".into(),
                max_bytes: None
            }
        );
        assert_eq!(
            serde_json::to_string(&req).unwrap(),
            r#"{"op":"read_output","id":"ws9-p4"}"#
        );
        // Present max_bytes round-trips.
        let req = SocketRequest::ReadOutput {
            id: "ws9-p4".into(),
            max_bytes: Some(4096),
        };
        let s = serde_json::to_string(&req).unwrap();
        assert_eq!(s, r#"{"op":"read_output","id":"ws9-p4","max_bytes":4096}"#);
        assert_eq!(serde_json::from_str::<SocketRequest>(&s).unwrap(), req);
        // The new app still parses every OLD request (spot-check the 06-02 shapes)…
        assert!(serde_json::from_str::<SocketRequest>(r#"{"op":"focus","id":"w"}"#).is_ok());
        assert!(serde_json::from_str::<SocketRequest>(
            r#"{"op":"send_input","id":"w","text":"y"}"#
        )
        .is_ok());
        // …and an UNKNOWN op (what an old app sees from a newer peer) is a clean serde
        // Err — the server maps it to a structured BAD_REQUEST, never a panic.
        assert!(
            serde_json::from_str::<SocketRequest>(r#"{"op":"read_output_v2","id":"w"}"#).is_err()
        );
        // The Output payload round-trips through SocketResponse.data.
        let resp = SocketResponse::ok("live tail").with_data(SocketData::Output {
            content: "hello".into(),
            truncated: true,
        });
        let s = serde_json::to_string(&resp).unwrap();
        let back: SocketResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(back, resp);
    }

    #[test]
    fn read_output_cap_defaults_and_hard_caps() {
        // Absent ⇒ default 64 KiB.
        assert_eq!(read_output_cap(None), READ_OUTPUT_DEFAULT_MAX_BYTES);
        // In-range values pass through (including the exact boundaries).
        assert_eq!(read_output_cap(Some(1)), 1);
        assert_eq!(read_output_cap(Some(0)), 0); // explicit 0 honored (liveness probe)
        assert_eq!(
            read_output_cap(Some(READ_OUTPUT_HARD_MAX_BYTES as u64)),
            READ_OUTPUT_HARD_MAX_BYTES
        );
        // One past the hard cap clamps; so does a u64::MAX ask.
        assert_eq!(
            read_output_cap(Some(READ_OUTPUT_HARD_MAX_BYTES as u64 + 1)),
            READ_OUTPUT_HARD_MAX_BYTES
        );
        assert_eq!(read_output_cap(Some(u64::MAX)), READ_OUTPUT_HARD_MAX_BYTES);
        // The default is itself within the hard cap (the constants can't drift
        // inverted) — a const block so the check runs at COMPILE time (and clippy's
        // assertions_on_constants stays quiet in the runtime test body).
        const {
            assert!(READ_OUTPUT_DEFAULT_MAX_BYTES <= READ_OUTPUT_HARD_MAX_BYTES);
        }
    }

    #[test]
    fn op_external_spawn_subset_and_disjoint_from_control() {
        // Spawn subset = CreateWorkspace + AddPane ONLY.
        assert!(op_external_spawn_allowed(&SocketRequest::CreateWorkspace {
            repo: "/r".into(),
            harness: "claude".into(),
            count: 1,
            role: None,
            model: None,
            panes: vec![],
            tag: None
        }));
        assert!(op_external_spawn_allowed(&SocketRequest::AddPane {
            harness: None,
            role: None,
            model: None,
            target_workspace: None
        }));
        // Control ops are NOT spawn ops (disjoint axes), and lifecycle/autonomous stay out.
        assert!(!op_external_spawn_allowed(&SocketRequest::SendInput {
            id: "w".into(),
            text: "y".into()
        }));
        assert!(!op_external_spawn_allowed(&SocketRequest::Orchestrate {
            goal: "g".into(),
            dispatch: false,
            target_workspace: None
        }));
        assert!(!op_external_spawn_allowed(&SocketRequest::Delegate {
            parent_id: "p".into(),
            goal: "g".into(),
            max_workers: 1,
            depth: 1
        }));
        // ...and the spawn ops are NOT in the control subset.
        assert!(!op_external_orchestrator_allowed(
            &SocketRequest::CreateWorkspace {
                repo: "/r".into(),
                harness: "claude".into(),
                count: 1,
                role: None,
                model: None,
                panes: vec![],
                tag: None
            }
        ));
    }

    #[test]
    fn external_spawn_harness_allowlist_forbids_bash() {
        assert!(external_spawn_harness_allowed("claude"));
        assert!(external_spawn_harness_allowed("Cursor")); // case-insensitive
        assert!(external_spawn_harness_allowed("codex"));
        // Raw shells are RCE primitives → never allowed for an external spawner.
        assert!(!external_spawn_harness_allowed("bash"));
        assert!(!external_spawn_harness_allowed("sh"));
        assert!(!external_spawn_harness_allowed("zsh"));
        assert!(!external_spawn_harness_allowed("")); // unknown → fail closed
    }

    #[test]
    fn external_spawn_role_allowlist_known_roles_only() {
        // All known core/roles variants (+ aliases) are accepted, case/space tolerant.
        for r in [
            "coordinator",
            "builder",
            "coder",
            "scout",
            "reviewer",
            "tester",
            "performance",
            "perf",
            "security",
            "db-migration",
            "dbmigration",
            "migration",
        ] {
            assert!(external_spawn_role_allowed(r), "expected allowed: {r}");
        }
        assert!(external_spawn_role_allowed("  Coordinator  ")); // trim + case
                                                                 // Operator opted to permit coordinator on this path (pid-pin + audit + confirm backstop).
        assert!(external_spawn_role_allowed("coordinator"));
        // Garbage / injection strings fail CLOSED — never silently dropped to "no role".
        assert!(!external_spawn_role_allowed("root"));
        assert!(!external_spawn_role_allowed("admin"));
        assert!(!external_spawn_role_allowed("'; rm -rf /"));
        assert!(!external_spawn_role_allowed(""));
    }

    #[test]
    fn pane_spec_count_defaults_to_one() {
        // A panes-spec without an explicit count deserializes to 1 (the replication default).
        let p: PaneSpec = serde_json::from_str(r#"{"harness":"claude"}"#).unwrap();
        assert_eq!(p.count, 1);
        assert_eq!(p.harness, "claude");
        assert!(p.role.is_none());
    }

    #[test]
    fn create_workspace_panes_is_wire_additive() {
        // A legacy scalar message (no `panes`) still round-trips, and `panes` defaults empty.
        let legacy = r#"{"op":"create_workspace","repo":"/r","harness":"claude","count":2}"#;
        let req: SocketRequest = serde_json::from_str(legacy).unwrap();
        match req {
            SocketRequest::CreateWorkspace {
                ref panes, count, ..
            } => {
                assert!(panes.is_empty());
                assert_eq!(count, 2);
            }
            _ => panic!("expected CreateWorkspace"),
        }
        // A panes message overrides the scalar shape.
        let atomic = r#"{"op":"create_workspace","repo":"/r","harness":"claude","count":1,
            "panes":[{"harness":"claude","role":"builder","count":2},
                     {"harness":"codex","role":"reviewer"}]}"#;
        let req: SocketRequest = serde_json::from_str(atomic).unwrap();
        match req {
            SocketRequest::CreateWorkspace { panes, .. } => {
                assert_eq!(panes.len(), 2);
                assert_eq!(panes[0].count, 2);
                assert_eq!(panes[1].count, 1);
                assert_eq!(panes[1].role.as_deref(), Some("reviewer"));
            }
            _ => panic!("expected CreateWorkspace"),
        }
    }

    #[test]
    fn op_served_by_app_true_only_for_synthesis_ops() {
        // MF-D routing: the synthesis/orchestration ops are SERVED_BY_APP regardless of
        // the mutation gate (a routing fact, not a capability grant).
        assert!(op_served_by_app(&SocketRequest::Orchestrate {
            goal: "g".into(),
            dispatch: false,
            target_workspace: None
        }));
        assert!(op_served_by_app(&SocketRequest::Broadcast {
            text: "t".into()
        }));
        assert!(op_served_by_app(&SocketRequest::Handoff {
            from: "a".into(),
            to: "b".into(),
            instruction: "i".into()
        }));
        assert!(op_served_by_app(&SocketRequest::Synthesize {
            dir: "d".into(),
            goal: "g".into()
        }));
        assert!(op_served_by_app(&SocketRequest::Delegate {
            parent_id: "p".into(),
            goal: "g".into(),
            max_workers: 1,
            depth: 1
        }));
        // Daemon-local / read ops are NOT served by the app.
        assert!(!op_served_by_app(&SocketRequest::ListLive));
        assert!(!op_served_by_app(&SocketRequest::SendInput {
            id: "w".into(),
            text: "y".into()
        }));
        assert!(!op_served_by_app(&SocketRequest::Focus { id: "w".into() }));
        assert!(!op_served_by_app(&SocketRequest::Attach { id: "w".into() }));
        assert!(!op_served_by_app(&SocketRequest::Detach { id: "w".into() }));
    }

    // ───────────────── Q4 daemon-spawns-on-behalf wire (Stage 2) ─────────────────

    fn sample_spawn_spec() -> SpawnSpec {
        SpawnSpec {
            id: "ws-1".into(),
            harness: "claude".into(),
            repo: "/repo".into(),
            session_id: Some("sess".into()),
            resume: false,
            role: Some("builder".into()),
            is_worker: false,
            extra_dirs: vec![],
            model: None,
            fresh_from_main: false,
            require_worktree: true,
        }
    }

    #[test]
    fn spawn_and_close_round_trip_by_tag() {
        // Spawn → {"op":"spawn","spec":{…}} round-trips losslessly.
        let spawn = SocketRequest::Spawn {
            spec: sample_spawn_spec(),
        };
        let s = serde_json::to_string(&spawn).unwrap();
        assert!(
            s.starts_with(r#"{"op":"spawn","spec":{"#),
            "tagged spawn wire: {s}"
        );
        assert_eq!(serde_json::from_str::<SocketRequest>(&s).unwrap(), spawn);
        // Close → {"op":"close","id":..}
        let close = SocketRequest::Close { id: "ws-1".into() };
        let c = serde_json::to_string(&close).unwrap();
        assert_eq!(c, r#"{"op":"close","id":"ws-1"}"#);
        assert_eq!(serde_json::from_str::<SocketRequest>(&c).unwrap(), close);
    }

    #[test]
    fn spawn_spec_has_no_force_fresh_field_and_drops_an_injected_one() {
        // C5 / decision 5: the destructive freshen is UNREPRESENTABLE on the wire. The
        // serialized form must carry no `force_fresh` key…
        let s = serde_json::to_string(&sample_spawn_spec()).unwrap();
        assert!(
            !s.contains("force_fresh"),
            "force_fresh must not exist on the wire: {s}"
        );
        // …and an attacker who INJECTS one is silently ignored (serde drops the unknown
        // field) — it can never reach `freshen_worktree`.
        let injected = r#"{"id":"ws-1","harness":"claude","repo":"/repo","force_fresh":true}"#;
        let spec: SpawnSpec = serde_json::from_str(injected).unwrap();
        assert_eq!(spec.id, "ws-1");
        assert!(
            !spec.fresh_from_main,
            "the only freshen field is non-destructive fresh_from_main"
        );
    }

    #[test]
    fn spawn_spec_optional_fields_default_when_absent() {
        // The minimal wire form (only the three required fields) deserializes with safe
        // defaults — is_worker false (so the wire-reject path is not auto-tripped), no
        // extra_dirs, no resume.
        let minimal = r#"{"id":"ws-9","harness":"bash","repo":"/r"}"#;
        let spec: SpawnSpec = serde_json::from_str(minimal).unwrap();
        assert!(!spec.is_worker && !spec.resume && !spec.fresh_from_main && !spec.require_worktree);
        assert!(spec.session_id.is_none() && spec.role.is_none() && spec.model.is_none());
        assert!(spec.extra_dirs.is_empty());
    }

    #[test]
    fn spawn_and_close_classify_mutating_daemon_local_and_spawn_gets_the_wide_window() {
        let spawn = SocketRequest::Spawn {
            spec: sample_spawn_spec(),
        };
        let close = SocketRequest::Close { id: "ws-1".into() };
        // Mutating (euid + allow_mutations) …
        assert!(op_requires_mutations(&spawn));
        assert!(op_requires_mutations(&close));
        // … and daemon-local (NOT served by the app synthesizer).
        assert!(!op_served_by_app(&spawn));
        assert!(!op_served_by_app(&close));
        // Spawn gets the wide window (worktree add + fork/exec); Close stays fast.
        assert_eq!(op_timeout(&spawn), SPAWN_TIMEOUT);
        assert_eq!(op_timeout(&close), FAST_OP_TIMEOUT);
        assert!(SPAWN_TIMEOUT.as_secs() > FAST_OP_TIMEOUT.as_secs());
    }

    #[test]
    fn new_q4_response_codes_have_their_exact_strings() {
        assert_eq!(response_code::SPAWN_DISABLED, "SPAWN_DISABLED");
        assert_eq!(response_code::SPAWN_UNAVAILABLE, "SPAWN_UNAVAILABLE");
        assert_eq!(response_code::ALREADY_LIVE, "ALREADY_LIVE");
        assert_eq!(response_code::SPAWN_REJECTED, "SPAWN_REJECTED");
        assert_eq!(response_code::CAP_EXCEEDED, "CAP_EXCEEDED");
        assert_eq!(response_code::UNCOMMITTED_WORK, "UNCOMMITTED_WORK");
    }

    #[test]
    fn validate_spawn_id_rejects_traversal_separators_and_overlength() {
        // Accept the real id shape (`ws28901x0-p3`).
        assert!(validate_spawn_id("ws28901x0-p3"));
        assert!(validate_spawn_id("a_b-C9"));
        // Reject empty, separators, traversal, dots, whitespace, control bytes.
        assert!(!validate_spawn_id(""));
        assert!(!validate_spawn_id(".."));
        assert!(!validate_spawn_id("a/../b"));
        assert!(!validate_spawn_id("a/b"));
        assert!(!validate_spawn_id("a\\b"));
        assert!(!validate_spawn_id("a.b"));
        assert!(!validate_spawn_id("a b"));
        assert!(!validate_spawn_id("a\nb"));
        assert!(
            !validate_spawn_id(&"x".repeat(129)),
            "over the 128-char cap"
        );
        assert!(
            validate_spawn_id(&"x".repeat(128)),
            "exactly at the cap is allowed"
        );
    }

    #[test]
    fn extra_dir_in_repo_scope_accepts_in_repo_rejects_out_of_repo_and_traversal() {
        let repo = Path::new("/repo");
        // The repo root itself and any descendant are in scope.
        assert!(extra_dir_in_repo_scope(repo, Path::new("/repo")));
        assert!(extra_dir_in_repo_scope(
            repo,
            Path::new("/repo/bridge/run/id")
        ));
        // A sibling that merely shares a name PREFIX is NOT under the repo (whole-component).
        assert!(!extra_dir_in_repo_scope(repo, Path::new("/repo-evil")));
        // An out-of-repo absolute dir is rejected.
        assert!(!extra_dir_in_repo_scope(repo, Path::new("/etc")));
        // A `..` component is rejected even if it'd resolve back inside the repo (it is
        // the real traversal vector and `Path::components` PRESERVES it).
        assert!(!extra_dir_in_repo_scope(repo, Path::new("/repo/../etc")));
        assert!(!extra_dir_in_repo_scope(repo, Path::new("/etc/../repo")));
        // (`Path::components` normalizes a bare interior `.` away, so `/repo/./x` is
        // lexically `/repo/x` and stays in scope — `.` is not a traversal vector.)
        assert!(extra_dir_in_repo_scope(repo, Path::new("/repo/./x")));
        // Relative paths (either side) are rejected (the daemon passes canonical absolutes).
        assert!(!extra_dir_in_repo_scope(repo, Path::new("relative/dir")));
        assert!(!extra_dir_in_repo_scope(
            Path::new("repo"),
            Path::new("/repo/x")
        ));
    }

    #[test]
    fn validate_session_id_accepts_uuids_and_rejects_flag_injection() {
        // A claude session id is a UUID — accepted.
        assert!(validate_session_id("3f2504e0-4f89-41d3-9a0c-0305e82c3301"));
        assert!(validate_session_id("ws28901x0-p3"));
        assert!(validate_session_id("a_b-C9"));
        // The load-bearing anti-injection check: a leading `-` (a flag token on the
        // optional-value `--resume` path) is REJECTED.
        assert!(!validate_session_id("--dangerously-skip-permissions"));
        assert!(!validate_session_id("-m"));
        // Empty / whitespace / separators / control bytes are rejected (no argv mischief).
        assert!(!validate_session_id(""));
        assert!(!validate_session_id("a b"));
        assert!(!validate_session_id("a/b"));
        assert!(!validate_session_id("a.b"));
        assert!(!validate_session_id("a\nb"));
        assert!(
            !validate_session_id(&"x".repeat(129)),
            "over the 128-char cap"
        );
        assert!(
            validate_session_id(&"x".repeat(128)),
            "exactly at the cap is allowed"
        );
    }

    #[test]
    fn validate_model_accepts_real_ids_and_rejects_flag_injection() {
        // Real model ids (incl. opencode provider/model + dated snapshots) are accepted.
        assert!(validate_model("claude-haiku-4-5"));
        assert!(validate_model("claude-3-5-sonnet-20241022"));
        assert!(validate_model("anthropic/claude-opus-4"));
        // The load-bearing anti-injection check: a leading `-` is REJECTED.
        assert!(!validate_model("--dangerously-skip-permissions"));
        assert!(!validate_model("-m"));
        // Empty / whitespace / control bytes are rejected.
        assert!(!validate_model(""));
        assert!(!validate_model("foo bar"));
        assert!(!validate_model("foo\tbar"));
        assert!(!validate_model("foo\nbar"));
        assert!(!validate_model(&"x".repeat(129)), "over the 128-char cap");
    }

    #[test]
    fn daemon_spawn_enabled_defaults_off_and_reads_fresh() {
        // The explicit Default is OFF (the fail-closed Q4 baseline).
        assert!(!McpConfig::default().daemon_spawn_enabled);
        let s = Scratch::new("daemon-spawn-gate");
        let p = mcp_config_path(s.path()).unwrap();
        // Absent ⇒ OFF; missing field ⇒ serde default OFF; explicit false ⇒ OFF.
        assert!(!read_mcp_config(s.path()).daemon_spawn_enabled);
        fs::write(&p, r#"{"allow_mutations":true}"#).unwrap();
        assert!(
            !read_mcp_config(s.path()).daemon_spawn_enabled,
            "absent field defaults OFF"
        );
        fs::write(&p, r#"{"daemon_spawn_enabled":false}"#).unwrap();
        assert!(!read_mcp_config(s.path()).daemon_spawn_enabled);
        // Only an explicit true enables — and it is INDEPENDENT of allow_mutations.
        fs::write(
            &p,
            r#"{"allow_mutations":false,"daemon_spawn_enabled":true}"#,
        )
        .unwrap();
        let cfg = read_mcp_config(s.path());
        assert!(
            cfg.daemon_spawn_enabled && !cfg.allow_mutations,
            "separate axis from allow_mutations"
        );
        // Malformed ⇒ STILL the locked-down default (never fail open).
        fs::write(&p, "{ not json").unwrap();
        assert!(!read_mcp_config(s.path()).daemon_spawn_enabled);
    }

    #[test]
    fn read_mcp_config_checked_distinguishes_absent_valid_and_malformed() {
        let s = Scratch::new("checked-read");
        let p = mcp_config_path(s.path()).unwrap();

        // ABSENT ⇒ Ok(default) (a first RMW write legitimately creates the file).
        assert!(!p.exists());
        let absent = read_mcp_config_checked(s.path()).expect("absent must be Ok(default)");
        assert_eq!(absent, McpConfig::default());

        // VALID ⇒ Ok(parsed), including unknown sibling keys carried in `extra`.
        fs::write(
            &p,
            r#"{"allow_mutations":true,"insforge_dashboard":{"port":9000}}"#,
        )
        .unwrap();
        let valid = read_mcp_config_checked(s.path()).expect("valid must parse");
        assert!(valid.allow_mutations, "typed gate parsed");
        assert!(
            valid.extra.contains_key("insforge_dashboard"),
            "an unknown sibling key is carried through so an RMW re-emits it"
        );

        // MALFORMED ⇒ Err (so an RMW writer REFUSES to overwrite and wipe gates/keys).
        fs::write(&p, "{ not json").unwrap();
        let err = read_mcp_config_checked(s.path())
            .expect_err("malformed must be Err, not a silent default");
        assert!(
            err.contains("malformed"),
            "error explains the refusal: {err}"
        );
        // The infallible reader still fails CLOSED on the same bytes (hot path unchanged).
        assert!(!read_mcp_config(s.path()).allow_mutations);
    }

    #[test]
    fn daemon_spawn_routing_flag_defaults_off_and_reads_fresh() {
        // Stage-4 GUI ROUTING flag — SEPARATE axis from the daemon's `daemon_spawn_enabled`
        // accept-side gate. Default OFF ⇒ `do_spawn` stays byte-identical to the local path.
        assert!(!McpConfig::default().daemon_spawn);
        let s = Scratch::new("daemon-spawn-routing");
        let p = mcp_config_path(s.path()).unwrap();
        // Absent ⇒ OFF; missing field ⇒ serde default OFF.
        assert!(!read_mcp_config(s.path()).daemon_spawn);
        fs::write(&p, r#"{"allow_mutations":true}"#).unwrap();
        assert!(
            !read_mcp_config(s.path()).daemon_spawn,
            "absent routing field defaults OFF"
        );
        // The two axes are INDEPENDENT: the daemon gate ON does not imply the GUI routes.
        fs::write(&p, r#"{"daemon_spawn_enabled":true}"#).unwrap();
        let cfg = read_mcp_config(s.path());
        assert!(
            cfg.daemon_spawn_enabled && !cfg.daemon_spawn,
            "routing flag is its own axis"
        );
        // Only an explicit true steers the GUI to the daemon dial.
        fs::write(&p, r#"{"daemon_spawn":true}"#).unwrap();
        assert!(read_mcp_config(s.path()).daemon_spawn);
        // Malformed ⇒ STILL the locked-down default (never fail open).
        fs::write(&p, "{ not json").unwrap();
        assert!(!read_mcp_config(s.path()).daemon_spawn);
    }

    #[test]
    fn send_input_enabled_defaults_off_is_its_own_axis_and_reads_fresh() {
        // The agent→agent send-input gate — a NARROW axis, decoupled from allow_mutations.
        assert!(!McpConfig::default().send_input_enabled, "default OFF");
        let s = Scratch::new("send-input-gate");
        let p = mcp_config_path(s.path()).unwrap();
        // Absent ⇒ OFF; missing field ⇒ serde default OFF.
        assert!(!read_mcp_config(s.path()).send_input_enabled);
        // INDEPENDENT of allow_mutations: the broad gate ON does NOT arm send-input.
        fs::write(&p, r#"{"allow_mutations":true}"#).unwrap();
        let cfg = read_mcp_config(s.path());
        assert!(
            cfg.allow_mutations && !cfg.send_input_enabled,
            "send-input is its own axis"
        );
        // And vice-versa: arming send-input does NOT enable the broad mutation surface.
        fs::write(&p, r#"{"send_input_enabled":true}"#).unwrap();
        let cfg = read_mcp_config(s.path());
        assert!(
            cfg.send_input_enabled && !cfg.allow_mutations,
            "narrow arm doesn't widen the surface"
        );
        // Malformed ⇒ STILL locked down (never fail open).
        fs::write(&p, "{ not json").unwrap();
        assert!(!read_mcp_config(s.path()).send_input_enabled);
    }

    #[test]
    fn memory_harvest_defaults_off_is_its_own_axis_and_reads_fresh() {
        // The post-run knowledge-harvest gate — its OWN axis, orthogonal to BOTH
        // allow_mutations (it writes notes, never panes) and memory_autoconsult
        // (arming recall must not silently arm store-writes).
        assert!(!McpConfig::default().memory_harvest, "default OFF");
        let s = Scratch::new("memory-harvest-gate");
        let p = mcp_config_path(s.path()).unwrap();
        // Absent file ⇒ OFF; missing field ⇒ serde default OFF.
        assert!(!read_mcp_config(s.path()).memory_harvest);
        fs::write(&p, r#"{"allow_mutations":true}"#).unwrap();
        assert!(
            !read_mcp_config(s.path()).memory_harvest,
            "the broad mutation gate does NOT arm harvest"
        );
        // Recall armed ≠ harvest armed (read side and write side are separate opt-ins).
        fs::write(&p, r#"{"memory_autoconsult":true}"#).unwrap();
        let cfg = read_mcp_config(s.path());
        assert!(
            cfg.memory_autoconsult && !cfg.memory_harvest,
            "autoconsult is a separate axis"
        );
        // And vice-versa: harvest ON arms neither recall nor mutations.
        fs::write(&p, r#"{"memory_harvest":true}"#).unwrap();
        let cfg = read_mcp_config(s.path());
        assert!(cfg.memory_harvest && !cfg.memory_autoconsult && !cfg.allow_mutations);
        // Malformed ⇒ STILL locked down (never fail open).
        fs::write(&p, "{ not json").unwrap();
        assert!(!read_mcp_config(s.path()).memory_harvest);
    }

    #[test]
    fn memory_capture_defaults_off_is_its_own_axis_and_reads_fresh() {
        // The run-outcome capture (WRITE-side) gate — its OWN axis, orthogonal to
        // allow_mutations, memory_autoconsult (recall) AND memory_harvest (LESSON:).
        assert!(!McpConfig::default().memory_capture, "default OFF");
        let s = Scratch::new("memory-capture-gate");
        let p = mcp_config_path(s.path()).unwrap();
        // Absent file ⇒ OFF; missing field ⇒ serde default OFF.
        assert!(!read_mcp_config(s.path()).memory_capture);
        // Neither the broad mutation gate nor the sibling memory gates arm capture.
        fs::write(&p, r#"{"allow_mutations":true}"#).unwrap();
        assert!(!read_mcp_config(s.path()).memory_capture);
        fs::write(&p, r#"{"memory_harvest":true,"memory_autoconsult":true}"#).unwrap();
        let cfg = read_mcp_config(s.path());
        assert!(
            cfg.memory_harvest && cfg.memory_autoconsult && !cfg.memory_capture,
            "capture is a separate axis"
        );
        // Reads the flipped value fresh; and arms neither sibling nor mutations.
        fs::write(&p, r#"{"memory_capture":true}"#).unwrap();
        let cfg = read_mcp_config(s.path());
        assert!(
            cfg.memory_capture
                && !cfg.memory_harvest
                && !cfg.memory_autoconsult
                && !cfg.allow_mutations
        );
        // Malformed ⇒ STILL locked down (never fail open).
        fs::write(&p, "{ not json").unwrap();
        assert!(!read_mcp_config(s.path()).memory_capture);
    }

    #[test]
    fn normalize_input_appends_cr_and_rejects_interior_and_control_bytes() {
        // bare text gains exactly one \r (CR — Enter for raw-mode TUIs)
        assert_eq!(normalize_input("approve").unwrap(), "approve\r");
        // caller-supplied trailing \n / \r\n is collapsed to the single \r submit
        assert_eq!(normalize_input("approve\n").unwrap(), "approve\r");
        assert_eq!(normalize_input("approve\r\n").unwrap(), "approve\r");
        // empty ⇒ a bare Enter
        assert_eq!(normalize_input("").unwrap(), "\r");
        // interior newline (CR or LF) → rejected (no second TUI line)
        assert!(normalize_input("yes\nyes").is_err());
        assert!(normalize_input("a\r\nb").is_err());
        // every other control byte → rejected (can't drive the TUI)
        assert!(normalize_input("\x1b[A").is_err()); // ESC — history nav
        assert!(normalize_input("\x03").is_err()); // Ctrl-C — SIGINT
        assert!(normalize_input("oops\x08").is_err()); // backspace
        assert!(normalize_input("a\tb").is_err()); // tab — completion
        assert!(normalize_input("x\x7f").is_err()); // DEL
        assert!(normalize_input("\u{0085}").is_err()); // C1 (NEL)
    }

    #[test]
    fn harness_needs_split_submit_true_for_all_but_bash() {
        for wire in [
            "claude",
            "cursor",
            "codex",
            "commandcode",
            "opencode",
            "cline",
        ] {
            assert!(harness_needs_split_submit(wire), "{wire} must split-submit");
        }
        assert!(
            !harness_needs_split_submit("bash"),
            "bash submits on a single write"
        );
    }

    #[test]
    fn split_settle_ms_scales_with_payload_caps_and_bash_is_zero() {
        // Ink (commandcode) batches slower than codex paste-burst.
        assert!(split_settle_ms("commandcode", 0) >= split_settle_ms("codex", 0));
        // a longer paste settles longer.
        assert!(split_settle_ms("commandcode", 3000) > split_settle_ms("commandcode", 0));
        // capped at 1200ms.
        assert_eq!(split_settle_ms("commandcode", 1_000_000), 1200);
        // bash never splits → zero settle.
        assert_eq!(split_settle_ms("bash", 5000), 0);
        // the exact per-harness bases are pinned (mirror the lifted app values).
        assert_eq!(split_settle_ms("codex", 0), 80);
        assert_eq!(split_settle_ms("claude", 0), 100);
        assert_eq!(split_settle_ms("cursor", 0), 150);
        assert_eq!(split_settle_ms("opencode", 0), 180);
        assert_eq!(split_settle_ms("commandcode", 0), 200);
        assert_eq!(split_settle_ms("cline", 0), 200);
    }

    #[test]
    fn structured_response_payload_round_trips_mapping_and_broadcast() {
        // Mapping (the dispatch:false preview) carries the {id,task} rows.
        let mapping = SocketResponse::ok("preview").with_data(SocketData::Mapping {
            tasks: vec![
                DispatchEntry {
                    id: "a".into(),
                    task: "do A".into(),
                },
                DispatchEntry {
                    id: "b".into(),
                    task: "do B".into(),
                },
            ],
        });
        let s = serde_json::to_string(&mapping).unwrap();
        let back: SocketResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(back, mapping);
        match back.data.unwrap() {
            SocketData::Mapping { tasks } => {
                assert_eq!(tasks.len(), 2);
                assert_eq!(tasks[0].id, "a");
                assert_eq!(tasks[1].task, "do B");
            }
            other => panic!("expected Mapping, got {other:?}"),
        }
        // Broadcast result carries the sent/skipped id partition.
        let bc = SocketResponse::ok("broadcast").with_data(SocketData::Broadcast {
            sent: vec!["a".into()],
            skipped: vec!["dead".into()],
        });
        let s = serde_json::to_string(&bc).unwrap();
        // `kind`-tagged enum is unambiguous on the wire.
        assert!(s.contains(r#""kind":"broadcast""#));
        let back: SocketResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(back, bc);
    }

    #[test]
    fn pane_synthesis_socket_data_round_trips_with_kind_tag() {
        // Phase 21: the sidecar-local pane_ids fan-in payload — content-carrying
        // (unlike Synthesis, which is path-only) + per-pane resolved sources.
        let resp = SocketResponse::ok("consolidated").with_data(SocketData::PaneSynthesis {
            report_path: "/tmp/agent-teams-synth/synth-1/final.md".into(),
            run_id: "synth-1".into(),
            content: "# Consolidated pane outputs (CLAIMED)\n…".into(),
            panes: vec![
                PaneSourceWire {
                    id: "ws9-p0".into(),
                    source: "orchestrate_report".into(),
                    bytes: 42,
                    truncated: false,
                },
                PaneSourceWire {
                    id: "ws9-p1".into(),
                    source: "none".into(),
                    bytes: 0,
                    truncated: false,
                },
            ],
        });
        let s = serde_json::to_string(&resp).unwrap();
        // `kind`-tagged enum is unambiguous on the wire.
        assert!(s.contains(r#""kind":"pane_synthesis""#));
        let back: SocketResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(back, resp);
        match back.data.unwrap() {
            SocketData::PaneSynthesis { content, panes, .. } => {
                assert!(content.contains("CLAIMED"));
                assert_eq!(panes.len(), 2);
                assert_eq!(panes[1].source, "none", "a no-output pane stays visible");
            }
            other => panic!("expected PaneSynthesis, got {other:?}"),
        }
    }

    #[test]
    fn response_data_is_backward_compatible_with_the_06_02_flat_shape() {
        // GAP 1: a payload-less response serializes EXACTLY as the 06-02 flat
        // {ok,code,detail} — skip_serializing_if drops `data`, so existing handlers
        // + tests are byte-for-byte unaffected.
        let flat = SocketResponse::err(response_code::DEAD_PANE, "dead");
        let s = serde_json::to_string(&flat).unwrap();
        assert_eq!(s, r#"{"ok":false,"code":"DEAD_PANE","detail":"dead"}"#);
        // An old reply that has no `data` field still deserializes (serde default).
        let old: SocketResponse =
            serde_json::from_str(r#"{"ok":true,"code":"OK","detail":"sent"}"#).unwrap();
        assert_eq!(old, SocketResponse::ok("sent"));
        assert!(old.data.is_none());
    }

    #[test]
    fn op_timeout_is_long_only_for_orchestrate_fast_for_everything_else() {
        // GAP 2: only the synthesis-wrapping ops get the >120s window; the rest stay
        // bounded so a wedged peer on a fast op can't tie up the listener unbounded.
        let long = op_timeout(&SocketRequest::Orchestrate {
            goal: "g".into(),
            dispatch: true,
            target_workspace: None,
        });
        assert_eq!(long, ORCHESTRATE_TIMEOUT);
        assert!(
            long.as_secs() > 120,
            "orchestrate must outlast the 180s kill-timeout"
        );
        // 06-05 / 06-18 #1: Synthesize wraps headless claude AND can chain up to three serial
        // passes (synthesis → independent adversary → decide) when a run has conflicts, so it
        // gets the WIDER window — else the socket read-wait aborts a conflict-bearing synthesis
        // and orphans both the finished final.md and the Opus tokens spent escalating.
        let synth = op_timeout(&SocketRequest::Synthesize {
            dir: "d".into(),
            goal: "g".into(),
        });
        assert_eq!(
            synth, SYNTHESIZE_TIMEOUT,
            "synthesize fan-in needs the 3-pass window"
        );
        assert!(
            synth.as_secs() >= 3 * 180,
            "covers up to three serial 180s synthesis passes"
        );
        for fast in [
            SocketRequest::SendInput {
                id: "w".into(),
                text: "y".into(),
            },
            SocketRequest::Focus { id: "w".into() },
            SocketRequest::Broadcast { text: "hi".into() },
            SocketRequest::Handoff {
                from: "a".into(),
                to: "b".into(),
                instruction: "go".into(),
            },
        ] {
            assert_eq!(op_timeout(&fast), FAST_OP_TIMEOUT);
        }
        assert_eq!(FAST_OP_TIMEOUT.as_secs(), 5);
    }

    #[test]
    fn external_spawn_cap_defaults_and_clamps() {
        // Absent ⇒ the built-in default.
        let mut cfg = McpConfig::default();
        assert_eq!(external_spawn_cap(&cfg), EXTERNAL_SPAWN_MAX_PANES);
        // Operator raise within range honored.
        cfg.external_spawn_max_panes = Some(12);
        assert_eq!(external_spawn_cap(&cfg), 12);
        // Clamped: 0 can't zero the surface; huge can't unbound it past the daemon ceiling.
        cfg.external_spawn_max_panes = Some(0);
        assert_eq!(external_spawn_cap(&cfg), 1);
        cfg.external_spawn_max_panes = Some(999);
        assert_eq!(external_spawn_cap(&cfg), 16);
        // File round-trip: the knob is plain JSON in mcp-config.json.
        let s = Scratch::new("mcp-config-spawncap");
        let p = mcp_config_path(s.path()).unwrap();
        fs::write(&p, r#"{"external_spawn_max_panes":6}"#).unwrap();
        assert_eq!(external_spawn_cap(&read_mcp_config(s.path())), 6);
    }

    #[test]
    fn external_orchestrator_pins_merges_dedupes_and_fails_closed() {
        // Default: no pins → no external caller ever admitted.
        let mut cfg = McpConfig::default();
        assert!(external_orchestrator_pins(&cfg).is_empty());
        // Singular only (today's installs) — unchanged behavior.
        cfg.external_orchestrator_path = Some("/Applications/Dev.app/Contents/MacOS/x".into());
        assert_eq!(
            external_orchestrator_pins(&cfg),
            vec!["/Applications/Dev.app/Contents/MacOS/x"]
        );
        // Plural adds MORE authorized binaries (e.g. the main app too)…
        cfg.external_orchestrator_paths = vec![
            "/Applications/Main.app/Contents/MacOS/x".into(),
            "  ".into(),
            "/Applications/Dev.app/Contents/MacOS/x".into(),
        ];
        // …trimmed, empties dropped, duplicates of the singular deduped, order-preserving.
        assert_eq!(
            external_orchestrator_pins(&cfg),
            vec![
                "/Applications/Dev.app/Contents/MacOS/x",
                "/Applications/Main.app/Contents/MacOS/x"
            ]
        );
        // Plural works without the singular too.
        cfg.external_orchestrator_path = None;
        assert_eq!(
            external_orchestrator_pins(&cfg),
            vec![
                "/Applications/Main.app/Contents/MacOS/x",
                "/Applications/Dev.app/Contents/MacOS/x"
            ]
        );
        // File round-trip: both fields are plain JSON in mcp-config.json.
        let s = Scratch::new("mcp-config-pins");
        let p = mcp_config_path(s.path()).unwrap();
        fs::write(
            &p,
            r#"{"external_orchestrator_path":"/a","external_orchestrator_paths":["/b","/a"]}"#,
        )
        .unwrap();
        assert_eq!(
            external_orchestrator_pins(&read_mcp_config(s.path())),
            vec!["/a", "/b"]
        );
    }

    #[test]
    fn mcp_config_defaults_to_mutations_off_and_fails_safe() {
        let s = Scratch::new("mcp-config");
        // Absent ⇒ SAFE default (mutations OFF), never fails open.
        assert!(!read_mcp_config(s.path()).allow_mutations);
        // Malformed ⇒ STILL the locked-down default, not enabled.
        let p = mcp_config_path(s.path()).unwrap();
        fs::write(&p, "{ not json").unwrap();
        assert!(!read_mcp_config(s.path()).allow_mutations);
        // Explicit false ⇒ off.
        fs::write(&p, r#"{"allow_mutations":false}"#).unwrap();
        assert!(!read_mcp_config(s.path()).allow_mutations);
        // Only an explicit true enables — and round-trips.
        fs::write(&p, r#"{"allow_mutations":true}"#).unwrap();
        assert!(read_mcp_config(s.path()).allow_mutations);
        // Missing field ⇒ serde default false (fail safe).
        fs::write(&p, "{}").unwrap();
        assert!(!read_mcp_config(s.path()).allow_mutations);
    }

    // ─────────────── loopback-HTTP transport guards (DRAFT, 06-02) ──────────────

    #[test]
    fn mcp_config_http_enabled_defaults_off_and_is_independent_of_mutations() {
        // The explicit Default has BOTH gates off — the fail-closed baseline.
        let d = McpConfig::default();
        assert!(!d.allow_mutations);
        assert!(!d.http_enabled);

        let s = Scratch::new("mcp-config-http");
        let p = mcp_config_path(s.path()).unwrap();
        // Absent / malformed ⇒ SAFE default, http_enabled OFF (never fails open).
        assert!(!read_mcp_config(s.path()).http_enabled);
        fs::write(&p, "{ not json").unwrap();
        assert!(!read_mcp_config(s.path()).http_enabled);
        // Missing field ⇒ serde default false.
        fs::write(&p, "{}").unwrap();
        assert!(!read_mcp_config(s.path()).http_enabled);
        // The two gates are independent: mutations on, HTTP still off.
        fs::write(&p, r#"{"allow_mutations":true}"#).unwrap();
        let c = read_mcp_config(s.path());
        assert!(c.allow_mutations);
        assert!(!c.http_enabled);
        // Only an explicit true binds the listener — and both gates round-trip.
        fs::write(&p, r#"{"allow_mutations":false,"http_enabled":true}"#).unwrap();
        let c = read_mcp_config(s.path());
        assert!(!c.allow_mutations);
        assert!(c.http_enabled);
    }

    #[test]
    fn mcp_config_loop_autonomy_defaults_off_and_round_trips() {
        // §4.6: the loop scheduler arm — file-only, default OFF, fail-closed like every other gate.
        let d = McpConfig::default();
        assert!(!d.loop_autonomy, "fresh install never auto-fires a loop");

        let s = Scratch::new("mcp-config-loop-autonomy");
        let p = mcp_config_path(s.path()).unwrap();
        // Absent ⇒ SAFE default (never fails open).
        assert!(!read_mcp_config(s.path()).loop_autonomy);
        // Malformed ⇒ SAFE default.
        fs::write(&p, "{ not json").unwrap();
        assert!(!read_mcp_config(s.path()).loop_autonomy);
        // Missing field on a real config ⇒ serde default false (byte-compatible with old files).
        fs::write(&p, r#"{"allow_mutations":true}"#).unwrap();
        assert!(!read_mcp_config(s.path()).loop_autonomy);
        // An explicit true round-trips, independent of the other gates.
        fs::write(&p, r#"{"loop_autonomy":true}"#).unwrap();
        let c = read_mcp_config(s.path());
        assert!(c.loop_autonomy);
        assert!(
            !c.allow_mutations,
            "loop_autonomy is independent of allow_mutations"
        );
        // Serialize → re-read preserves it (the read-modify-write discipline relies on this).
        let body = serde_json::to_string_pretty(&c).unwrap();
        let c2: McpConfig = serde_json::from_str(&body).unwrap();
        assert_eq!(c, c2);
        assert!(c2.loop_autonomy);
    }

    #[test]
    fn mcp_config_flywheel_review_defaults_off_and_round_trips() {
        // §3.6 (P5): the smart-PR-review arm — file-only, default OFF, fail-closed like every gate.
        let d = McpConfig::default();
        assert!(
            !d.flywheel_review,
            "fresh install never runs the review-block downgrade"
        );

        let s = Scratch::new("mcp-config-flywheel-review");
        let p = mcp_config_path(s.path()).unwrap();
        // Absent ⇒ SAFE default (never fails open).
        assert!(!read_mcp_config(s.path()).flywheel_review);
        // Malformed ⇒ SAFE default.
        fs::write(&p, "{ not json").unwrap();
        assert!(!read_mcp_config(s.path()).flywheel_review);
        // Missing field on a real config ⇒ serde default false (byte-compatible with old files).
        fs::write(&p, r#"{"allow_mutations":true}"#).unwrap();
        assert!(!read_mcp_config(s.path()).flywheel_review);
        // An explicit true round-trips, independent of the other gates.
        fs::write(&p, r#"{"flywheel_review":true}"#).unwrap();
        let c = read_mcp_config(s.path());
        assert!(c.flywheel_review);
        assert!(
            !c.allow_mutations,
            "flywheel_review is independent of allow_mutations"
        );
        assert!(
            !c.flywheel_crap,
            "flywheel_review is independent of flywheel_crap"
        );
        // Serialize → re-read preserves it (the read-modify-write discipline relies on this).
        let body = serde_json::to_string_pretty(&c).unwrap();
        let c2: McpConfig = serde_json::from_str(&body).unwrap();
        assert_eq!(c, c2);
        assert!(c2.flywheel_review);
    }

    #[test]
    fn mcp_config_flywheel_crap_defaults_off_and_round_trips() {
        // §3.10 (P5): the CRAP-delta veto arm — file-only, default OFF, fail-closed like every gate.
        let d = McpConfig::default();
        assert!(
            !d.flywheel_crap,
            "fresh install never runs the CRAP-delta downgrade"
        );

        let s = Scratch::new("mcp-config-flywheel-crap");
        let p = mcp_config_path(s.path()).unwrap();
        // Absent ⇒ SAFE default.
        assert!(!read_mcp_config(s.path()).flywheel_crap);
        // Malformed ⇒ SAFE default.
        fs::write(&p, "{ not json").unwrap();
        assert!(!read_mcp_config(s.path()).flywheel_crap);
        // Missing field on a real config ⇒ serde default false.
        fs::write(&p, r#"{"allow_mutations":true}"#).unwrap();
        assert!(!read_mcp_config(s.path()).flywheel_crap);
        // An explicit true round-trips, independent of the other gates.
        fs::write(&p, r#"{"flywheel_crap":true}"#).unwrap();
        let c = read_mcp_config(s.path());
        assert!(c.flywheel_crap);
        assert!(
            !c.flywheel_review,
            "flywheel_crap is independent of flywheel_review"
        );
        // Serialize → re-read preserves it.
        let body = serde_json::to_string_pretty(&c).unwrap();
        let c2: McpConfig = serde_json::from_str(&body).unwrap();
        assert_eq!(c, c2);
        assert!(c2.flywheel_crap);
    }

    #[test]
    fn mcp_config_serena_defaults_off_and_round_trips() {
        // §3.9-B (P5): the serena per-worktree LSP-MCP opt-in — file-only, default OFF.
        let d = McpConfig::default();
        assert!(!d.serena, "fresh install never starts a serena server");

        let s = Scratch::new("mcp-config-serena");
        let p = mcp_config_path(s.path()).unwrap();
        assert!(!read_mcp_config(s.path()).serena); // absent ⇒ off
        fs::write(&p, "{ not json").unwrap();
        assert!(!read_mcp_config(s.path()).serena); // malformed ⇒ off
        fs::write(&p, r#"{"allow_mutations":true}"#).unwrap();
        assert!(!read_mcp_config(s.path()).serena); // missing field ⇒ off
        fs::write(&p, r#"{"serena":true}"#).unwrap();
        let c = read_mcp_config(s.path());
        assert!(c.serena);
        let body = serde_json::to_string_pretty(&c).unwrap();
        let c2: McpConfig = serde_json::from_str(&body).unwrap();
        assert_eq!(c, c2);
        assert!(c2.serena);
    }

    #[test]
    fn http_token_and_port_paths_are_siblings_of_state_root() {
        assert_eq!(
            http_token_path(Path::new("/var/app/agent-teams")),
            Some(PathBuf::from("/var/app/agent-teams-mcp-http.token"))
        );
        assert_eq!(
            http_port_path(Path::new("/var/app/agent-teams")),
            Some(PathBuf::from("/var/app/agent-teams-mcp-http.port"))
        );
        // No parent ⇒ no path (mirrors socket_path's None case; never panics).
        assert_eq!(http_token_path(Path::new("/")), None);
        assert_eq!(http_port_path(Path::new("/")), None);
    }

    #[test]
    fn http_host_allowed_requires_exact_loopback_ip_and_port() {
        // Exact 127.0.0.1:<bound_port> ⇒ allowed.
        assert!(http_host_allowed(Some("127.0.0.1:54321"), 54321));
        // Missing Host ⇒ reject (fail closed).
        assert!(!http_host_allowed(None, 54321));
        // Wrong port ⇒ reject (the bound port must match exactly).
        assert!(!http_host_allowed(Some("127.0.0.1:54322"), 54321));
        // A hostname (even one that resolves to loopback) is NOT accepted — we
        // bind 127.0.0.1 only, so the routed Host must be the literal IP.
        assert!(!http_host_allowed(Some("localhost:54321"), 54321));
        // A forged rebinding host is rejected.
        assert!(!http_host_allowed(Some("127.0.0.1.evil.com:54321"), 54321));
        assert!(!http_host_allowed(Some("evil.com:54321"), 54321));
        // Missing port / bare IP ⇒ reject (no implicit port).
        assert!(!http_host_allowed(Some("127.0.0.1"), 54321));
        // Trailing cruft ⇒ reject (exact match only).
        assert!(!http_host_allowed(Some("127.0.0.1:54321 "), 54321));
        assert!(!http_host_allowed(Some("127.0.0.1:54321/x"), 54321));
    }

    #[test]
    fn http_origin_allowed_blocks_dns_rebinding_origins() {
        // No Origin (non-browser MCP client) ⇒ allowed.
        assert!(http_origin_allowed(None));
        // Loopback origins ⇒ allowed (with and without a port; http and https).
        assert!(http_origin_allowed(Some("http://127.0.0.1")));
        assert!(http_origin_allowed(Some("http://127.0.0.1:54321")));
        assert!(http_origin_allowed(Some("http://localhost")));
        assert!(http_origin_allowed(Some("http://localhost:54321")));
        assert!(http_origin_allowed(Some("https://127.0.0.1:54321")));
        // THE property test: a rebinding origin that merely STARTS WITH the
        // loopback string must be rejected (a prefix check would wrongly pass).
        assert!(!http_origin_allowed(Some("http://127.0.0.1.evil.com")));
        assert!(!http_origin_allowed(Some(
            "http://127.0.0.1.evil.com:54321"
        )));
        assert!(!http_origin_allowed(Some("http://localhost.evil.com")));
        // A userinfo trick must not be read as the loopback host.
        assert!(!http_origin_allowed(Some("http://127.0.0.1@evil.com")));
        // A plain external origin ⇒ rejected.
        assert!(!http_origin_allowed(Some("http://evil.com")));
        // A non-http(s) scheme ⇒ rejected.
        assert!(!http_origin_allowed(Some("file:///etc/passwd")));
        assert!(!http_origin_allowed(Some("ftp://127.0.0.1")));
        // Empty / schemeless ⇒ rejected.
        assert!(!http_origin_allowed(Some("")));
        assert!(!http_origin_allowed(Some("127.0.0.1")));
    }

    #[test]
    fn bearer_token_matches_only_on_exact_constant_time_match() {
        // Exact match (case-insensitive scheme).
        assert!(bearer_token_matches(Some("Bearer s3cr3t"), "s3cr3t"));
        assert!(bearer_token_matches(Some("bearer s3cr3t"), "s3cr3t"));
        assert!(bearer_token_matches(Some("BEARER s3cr3t"), "s3cr3t"));
        // Wrong token ⇒ reject (same length and different length).
        assert!(!bearer_token_matches(Some("Bearer s3cr3x"), "s3cr3t"));
        assert!(!bearer_token_matches(Some("Bearer short"), "s3cr3t"));
        assert!(!bearer_token_matches(Some("Bearer s3cr3t-extra"), "s3cr3t"));
        // Empty EXPECTED ⇒ never match (a missing/empty token file authorizes no one).
        assert!(!bearer_token_matches(Some("Bearer "), ""));
        assert!(!bearer_token_matches(Some("Bearer anything"), ""));
        // Missing header ⇒ reject.
        assert!(!bearer_token_matches(None, "s3cr3t"));
        // Wrong scheme ⇒ reject.
        assert!(!bearer_token_matches(Some("Basic s3cr3t"), "s3cr3t"));
        assert!(!bearer_token_matches(Some("Token s3cr3t"), "s3cr3t"));
        // Malformed (no space / no scheme) ⇒ reject.
        assert!(!bearer_token_matches(Some("s3cr3t"), "s3cr3t"));
        assert!(!bearer_token_matches(Some("Bearer"), "s3cr3t"));
        assert!(!bearer_token_matches(Some(""), "s3cr3t"));
        // Token is NOT trimmed — surrounding whitespace must not match.
        assert!(!bearer_token_matches(Some("Bearer  s3cr3t"), "s3cr3t"));
        assert!(!bearer_token_matches(Some("Bearer s3cr3t "), "s3cr3t"));
    }

    #[test]
    fn constant_time_eq_matches_only_equal_bytes() {
        assert!(constant_time_eq(b"", b""));
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(!constant_time_eq(b"ab", b"abc"));
    }

    // ───────────────────────── Lane 6: adversarial AUTH fuzz ─────────────────────
    //
    // These tests treat the three Phase-B loopback-HTTP validators as the
    // load-bearing same-user gate (euid is gone on TCP) and throw bypass-shaped
    // corpora at them. Each assertion LOCKS the validator's CURRENT verdict so a
    // future weakening (a prefix check, a trim, a case-fold of the token, an
    // implicit port) flips a green test red. The suite is deliberately a *finder*:
    // it asserts what the code ACTUALLY does, and any corpus row that turned out
    // to authorize a forged request would have been escalated as a security
    // finding rather than written green. None did — see the two documented
    // benign-TRUE rows below, which are NOT bypasses.

    /// Deterministic xorshift64 PRNG — fixed seed, no `rand` dep, reproducible so a
    /// failure is debuggable. Used only to drive one-byte token mutations.
    fn xorshift64(state: &mut u64) -> u64 {
        let mut x = *state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *state = x;
        x
    }

    const BEARER_EXPECTED: &str = "s3cr3t-deadbeef";

    #[test]
    fn bearer_corpus_only_exact_token_authorizes() {
        // (header, secure_verdict) against expected = "s3cr3t-deadbeef".
        // Every row is a near-miss or malformed credential that must FAIL CLOSED,
        // except the single exact match. The lane corpus listed
        // "bearer s3cr3t-deadbeef" as FALSE; the code returns TRUE and that is
        // INTENDED, not a bypass: the scheme is matched case-insensitively per
        // RFC 7235 (doc at lib.rs:568, locked by the existing test at lib.rs:1032)
        // and the CORRECT secret token was presented — only the scheme case varied.
        let corpus: &[(Option<&str>, bool)] = &[
            (None, false),                            // missing header
            (Some(""), false),                        // empty header (no space → malformed)
            (Some("secret"), false),                  // bare word, no scheme
            (Some("Bearer "), false),                 // scheme only, empty token
            (Some("Bearer wrong"), false),            // wrong token
            (Some("bearer s3cr3t-deadbeef"), true), // lowercase scheme + correct token → intended TRUE
            (Some("Bearer  s3cr3t-deadbeef"), false), // double space → token has leading space
            (Some("Bearer s3cr3t-deadbeef "), false), // trailing space → not trimmed
            (Some("Bearer s3cr3t-deadbee"), false), // truncated by one byte
            (Some("Bearer s3cr3t-deadbeefX"), false), // padded by one byte
            (Some("Bearer S3CR3T-DEADBEEF"), false), // token compared case-SENSITIVELY
            (Some("Bearer x3cr3t-deadbeef"), false), // first byte differs
            (Some("Token s3cr3t-deadbeef"), false), // wrong scheme
            (Some("s3cr3t-deadbeef"), false),       // no prefix at all
            (Some("Bearer s3cr3t-deadbeef"), true), // THE only authorizing credential
        ];
        for (header, want) in corpus {
            let got = bearer_token_matches(*header, BEARER_EXPECTED);
            assert_eq!(
                got, *want,
                "bearer_token_matches({header:?}, {BEARER_EXPECTED:?}) = {got}, want {want}"
            );
        }
        // An EMPTY expected token must authorize NOBODY, even a well-formed header.
        assert!(!bearer_token_matches(Some("Bearer "), ""));
        assert!(!bearer_token_matches(None, ""));
        assert!(!bearer_token_matches(Some("Bearer anything"), ""));
    }

    #[test]
    fn bearer_one_byte_mutation_never_authorizes() {
        // Fuzz: flip exactly one byte of the correct token with a guaranteed-nonzero
        // mask (so the byte ALWAYS changes), present it under a valid "Bearer "
        // header, and assert it never authorizes. A single changed byte makes the
        // token differ in content at equal length → constant_time_eq → false. If
        // any mutation authorized, that would be a near-miss-token bypass to report.
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let expected = BEARER_EXPECTED.as_bytes();
        for _ in 0..10_000 {
            let r = xorshift64(&mut state);
            let idx = (r as usize) % expected.len();
            let mask = ((r >> 40) as u8) % 255 + 1; // 1..=255, never 0 → byte truly changes
            let mut mutated = expected.to_vec();
            mutated[idx] ^= mask;
            // Sanity: the mutation actually diverged from the real token.
            assert_ne!(
                mutated, expected,
                "mutation must differ from the real token"
            );
            let header = format!("Bearer {}", String::from_utf8_lossy(&mutated));
            assert!(
                !bearer_token_matches(Some(&header), BEARER_EXPECTED),
                "one-byte-mutated token must NOT authorize: idx={idx} mask={mask} header={header:?}"
            );
        }
    }

    #[test]
    fn origin_rebinding_corpus_all_rejected() {
        // Anti-DNS-rebinding: every Origin whose host is not EXACTLY a loopback name
        // must be rejected. These are the classic prefix/userinfo/sibling-domain
        // rebinding shapes a `starts_with("http://127.0.0.1")` check would wrongly
        // pass — the validator strips scheme, cuts the authority, drops userinfo and
        // port, then requires host EQUALITY.
        let reject: &[&str] = &[
            "http://127.0.0.1.evil.com", // sibling domain (the canonical rebinding)
            "http://127.0.0.1.attacker.com:1234", // sibling domain + port
            "http://127.0.0.1@evil.com", // userinfo trick → host is evil.com
            "http://evil.com",           // plain external
            "http://localhost.evil.com", // localhost as a label, not the host
            "http://0x7f000001",         // hex-encoded loopback IP (not the literal)
            "http://127.0.0.1 .evil.com", // embedded space → host != loopback
            "http://[::1].evil",         // IPv6-ish junk → rsplit(':') host = "[:"
            "null",                      // schemeless sentinel → reject
        ];
        for o in reject {
            assert!(
                !http_origin_allowed(Some(o)),
                "rebinding origin must be rejected: {o:?}"
            );
        }

        // BENIGN-TRUE (documented, NOT a bypass): the lane corpus listed
        // "http://127.0.0.1:abc" under "all FALSE", but the code returns TRUE and
        // correctly so — the Origin HOST is literally 127.0.0.1; a browser sets
        // Origin to the real page origin, and a page on evil.com cannot forge
        // `Origin: http://127.0.0.1:*`. Rebinding requires a NON-loopback hostname
        // in the Origin; this is not one. The garbage port is irrelevant to a host
        // check. Locked here so the leniency is intentional and visible.
        assert!(
            http_origin_allowed(Some("http://127.0.0.1:abc")),
            "loopback host with a malformed port is still a loopback host"
        );

        // LOOPBACK-ACCEPT: real loopback origins (and the no-Origin non-browser
        // client) must pass. "https://127.0.0.1" is verified TRUE (matches the
        // existing assert at lib.rs:1010).
        let accept: &[Option<&str>] = &[
            None, // non-browser MCP client, no Origin
            Some("http://127.0.0.1"),
            Some("http://127.0.0.1:8080"),
            Some("http://localhost"),
            Some("http://localhost:5173"),
            Some("https://127.0.0.1"),
        ];
        for o in accept {
            assert!(
                http_origin_allowed(*o),
                "loopback origin must be accepted: {o:?}"
            );
        }
    }

    #[test]
    fn host_corpus_only_exact_loopback_ip_and_port() {
        const BOUND_PORT: u16 = 54321;
        // (host_header, secure_verdict) — the Host we route on must pin to the EXACT
        // literal address+port we bound (127.0.0.1:54321). No hostname, no implicit
        // port, no leading zeros, no trailing cruft. Exactly one row is TRUE.
        let corpus: &[(Option<&str>, bool)] = &[
            (Some("127.0.0.1:54321"), true),       // the only allowed Host
            (None, false),                         // missing → fail closed
            (Some("127.0.0.1"), false),            // bare IP, no port
            (Some("127.0.0.1:54322"), false),      // wrong port
            (Some("localhost:54321"), false),      // hostname not the literal IP
            (Some("127.0.0.1:54321 "), false),     // trailing space
            (Some("127.0.0.1:54321.evil"), false), // trailing cruft
            (Some("[::1]:54321"), false),          // IPv6 loopback ≠ literal 127.0.0.1
            (Some("0.0.0.0:54321"), false),        // wildcard bind address
            (Some("127.0.0.1:054321"), false),     // leading-zero port is not "54321"
        ];
        for (host, want) in corpus {
            let got = http_host_allowed(*host, BOUND_PORT);
            assert_eq!(
                got, *want,
                "http_host_allowed({host:?}, {BOUND_PORT}) = {got}, want {want}"
            );
        }
    }

    // ───────── Phase-12 (D51) HMAC-SHA256 challenge SSOT primitive ─────────

    /// Raw HMAC-SHA256(key, msg) over BYTE inputs, returned lowercase-hex. A tiny test
    /// helper that exercises the SAME `HmacSha256` type the primitive uses, so the
    /// PUBLISHED RFC 4231 vectors below pin the HMAC *math* directly (independent of our
    /// token/nonce hex-decoding — those are pinned separately by the derivation KAT).
    fn raw_hmac_sha256_hex(key: &[u8], msg: &[u8]) -> String {
        let mut mac = <HmacSha256 as hmac::Mac>::new_from_slice(key).unwrap();
        hmac::Mac::update(&mut mac, msg);
        hex::encode(hmac::Mac::finalize(mac).into_bytes())
    }

    /// KNOWN-ANSWER: published RFC 4231 HMAC-SHA256 vectors (NOT self-generated). These
    /// prove the HMAC primitive computes the standard algorithm — a self-made vector would
    /// only prove self-consistency and could bake in a wrong representation on both sides.
    #[test]
    fn hmac_sha256_matches_rfc4231_published_vectors() {
        // RFC 4231 Test Case 2: key = "Jefe", data = "what do ya want for nothing?".
        assert_eq!(
            raw_hmac_sha256_hex(b"Jefe", b"what do ya want for nothing?"),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843",
            "RFC 4231 Test Case 2 HMAC-SHA256 mismatch"
        );
        // RFC 4231 Test Case 1: key = 20 bytes of 0x0b, data = "Hi There".
        assert_eq!(
            raw_hmac_sha256_hex(&[0x0b_u8; 20], b"Hi There"),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7",
            "RFC 4231 Test Case 1 HMAC-SHA256 mismatch"
        );
    }

    /// KNOWN-ANSWER: pins the token→key→MAC REPRESENTATION (key = 32 hex-DECODED token
    /// bytes; msg = 32 hex-DECODED nonce bytes). The expected MAC was computed
    /// independently (Python `hmac`) over the DECODED bytes; the two `assert_ne!`s prove
    /// this test would FAIL if either side wrongly keyed/messaged off the 64 ASCII-hex
    /// chars — i.e. it actually catches the silent-mismatch the review flagged.
    #[test]
    fn challenge_mac_pins_hex_decoded_byte_representation() {
        let token_hex = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
        let nonce_hex = "fffefdfcfbfaf9f8f7f6f5f4f3f2f1f0efeeedecebeae9e8e7e6e5e4e3e2e1e0";
        let expected = "e859e9ccf8c926d30dbdd7e9f1a1256dd2ba046b67bef092a70347167be574fd";

        // The SSOT helper must produce exactly the decoded-bytes MAC.
        assert_eq!(
            compute_challenge_mac(token_hex, nonce_hex).as_deref(),
            Some(expected),
            "compute_challenge_mac must key/message off the 32 hex-DECODED bytes"
        );

        // The key the helper derives is exactly the 32 decoded bytes.
        let key = derive_hmac_key(token_hex).expect("64-hex token derives a key");
        assert_eq!(key, hex::decode(token_hex).unwrap().as_slice());
        let nonce_bytes = decode_challenge_nonce(nonce_hex).unwrap();

        // Cross-check the helper against a from-bytes raw HMAC.
        assert_eq!(
            compute_challenge_mac(token_hex, nonce_hex).unwrap(),
            raw_hmac_sha256_hex(&key, &nonce_bytes)
        );

        // Representation-trap guards: the WRONG representations produce a DIFFERENT MAC,
        // so this KAT genuinely pins "decoded bytes" rather than passing either way.
        assert_ne!(
            raw_hmac_sha256_hex(token_hex.as_bytes(), &nonce_bytes),
            expected,
            "keying off the 64 ASCII-hex chars must NOT match the pinned MAC"
        );
        assert_ne!(
            raw_hmac_sha256_hex(&key, nonce_hex.as_bytes()),
            expected,
            "messaging the 64 ASCII-hex nonce chars must NOT match the pinned MAC"
        );
    }

    // ───────────────── delegate MVP wire types + policy (Lane A) ────────────────

    #[test]
    fn delegate_op_round_trips_through_the_exact_wire_contract() {
        // delegate → {"op":"delegate","parent_id":..,"goal":..,"max_workers":..,"depth":..}
        let d = serde_json::to_string(&SocketRequest::Delegate {
            parent_id: "p".into(),
            goal: "g".into(),
            max_workers: 3,
            depth: 1,
        })
        .unwrap();
        assert_eq!(
            d,
            r#"{"op":"delegate","parent_id":"p","goal":"g","max_workers":3,"depth":1}"#
        );
        assert_eq!(
            serde_json::from_str::<SocketRequest>(&d).unwrap(),
            SocketRequest::Delegate {
                parent_id: "p".into(),
                goal: "g".into(),
                max_workers: 3,
                depth: 1
            }
        );
    }

    #[test]
    fn derive_hmac_key_fails_closed_on_bad_tokens() {
        // Valid: exactly 64 hex chars → Some(32 bytes).
        let ok = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        assert!(derive_hmac_key(ok).is_some());
        // Empty token ⇒ None (mirrors bearer_token_matches' empty-expected rule).
        assert!(derive_hmac_key("").is_none());
        // Non-hex chars ⇒ None.
        assert!(derive_hmac_key(&"zz".repeat(32)).is_none());
        // Too short / too long (wrong decoded length) ⇒ None.
        assert!(derive_hmac_key("00112233").is_none());
        assert!(derive_hmac_key(&"00".repeat(33)).is_none());
        // Odd-length hex ⇒ None (cannot decode).
        assert!(derive_hmac_key(&"0".repeat(63)).is_none());
    }

    #[test]
    fn decode_challenge_nonce_requires_exact_64_hex() {
        let ok = "fffefdfcfbfaf9f8f7f6f5f4f3f2f1f0efeeedecebeae9e8e7e6e5e4e3e2e1e0";
        assert!(decode_challenge_nonce(ok).is_some());
        // Wrong length (63 / 66) ⇒ reject even though 66 would be even+decodable.
        assert!(decode_challenge_nonce(&"a".repeat(63)).is_none());
        assert!(decode_challenge_nonce(&"a".repeat(66)).is_none());
        // Right length but non-hex ⇒ reject.
        assert!(decode_challenge_nonce(&"zz".repeat(32)).is_none());
        // Empty ⇒ reject.
        assert!(decode_challenge_nonce("").is_none());
        // compute_challenge_mac inherits the rejection (no MAC over a bad nonce).
        let token = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        assert!(compute_challenge_mac(token, &"a".repeat(63)).is_none());
    }

    #[test]
    fn verify_challenge_mac_is_strict_and_constant_time() {
        let token = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
        let nonce = "fffefdfcfbfaf9f8f7f6f5f4f3f2f1f0efeeedecebeae9e8e7e6e5e4e3e2e1e0";
        let good = compute_challenge_mac(token, nonce).unwrap();

        // The correct MAC verifies.
        assert!(verify_challenge_mac(token, nonce, &good));

        // H4 / MAC-field strictness: missing/empty/short/long/non-hex all REJECT — the
        // MAC match is the decision, never an `ok:true` flag (which this primitive never
        // even sees).
        assert!(
            !verify_challenge_mac(token, nonce, ""),
            "empty mac must reject"
        );
        assert!(
            !verify_challenge_mac(token, nonce, &good[..62]),
            "short (62-hex) mac must reject"
        );
        assert!(
            !verify_challenge_mac(token, nonce, &format!("{good}ab")),
            "over-long (66-hex) mac must reject"
        );
        assert!(
            !verify_challenge_mac(token, nonce, &"zz".repeat(32)),
            "non-hex 64-char mac must reject"
        );
        // A wrong-but-valid-shape MAC (last hex nibble flipped) rejects.
        let mut wrong = good.clone();
        let last = wrong.pop().unwrap();
        wrong.push(if last == '0' { '1' } else { '0' });
        assert!(
            !verify_challenge_mac(token, nonce, &wrong),
            "altered mac must reject"
        );

        // A bad token (can't derive a key) ⇒ verify refuses even with a 64-hex candidate.
        assert!(!verify_challenge_mac("", nonce, &good));
    }

    #[test]
    fn delegate_socket_data_round_trips_with_kind_tag() {
        let ack = SocketResponse::ok("delegation accepted").with_data(SocketData::Delegate {
            run_id: "run-1".into(),
            workers: 3,
        });
        let s = serde_json::to_string(&ack).unwrap();
        // `kind`-tagged enum is unambiguous on the wire.
        assert!(s.contains(r#""kind":"delegate""#));
        let back: SocketResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(back, ack);
        match back.data.unwrap() {
            SocketData::Delegate { run_id, workers } => {
                assert_eq!(run_id, "run-1");
                assert_eq!(workers, 3);
            }
            other => panic!("expected Delegate, got {other:?}"),
        }
    }

    #[test]
    fn op_timeout_for_delegate_is_fast_no_listener_wedge() {
        // Delegate acks fast (the real work runs detached) — it MUST fall in the
        // FAST window, never the long Orchestrate one, so it can't wedge the
        // serial listener.
        let t = op_timeout(&SocketRequest::Delegate {
            parent_id: "p".into(),
            goal: "g".into(),
            max_workers: 3,
            depth: 1,
        });
        assert_eq!(t, FAST_OP_TIMEOUT);
    }

    #[test]
    fn delegate_worker_budget_reserves_one_and_never_zero() {
        // cap-1, but never 0 — at least one slot stays for a human-driven pane.
        assert_eq!(delegate_worker_budget(3), 2);
        assert_eq!(delegate_worker_budget(1), 1);
        assert_eq!(delegate_worker_budget(0), 1);
    }

    #[test]
    fn delegate_admit_clamps_to_budget_hard_cap_and_floor() {
        // Clamped to the fairness budget (cap-1).
        assert_eq!(delegate_admit(99, 3), 2);
        assert_eq!(delegate_admit(1, 3), 1);
        // Requested count passes through when under both bounds.
        assert_eq!(delegate_admit(3, 5), 3);
        // Hard cap DELEGATE_MAX_WORKERS (=10) bites before a large concurrency budget.
        assert_eq!(delegate_admit(99, 20), 10);
        // Floored at 1 even when 0 requested.
        assert_eq!(delegate_admit(0, 3), 1);
    }

    #[test]
    fn delegate_depth_ok_only_for_depth_one() {
        assert!(delegate_depth_ok(1));
        assert!(!delegate_depth_ok(2));
    }

    #[test]
    fn mcp_config_autonomy_ceiling_defaults_to_zero_and_fails_safe() {
        // The explicit Default fails closed on autonomy.
        assert_eq!(McpConfig::default().autonomy_ceiling, 0);

        let s = Scratch::new("mcp-config-autonomy");
        let p = mcp_config_path(s.path()).unwrap();
        // Absent ⇒ SAFE default (autonomy OFF), never fails open.
        assert_eq!(read_mcp_config(s.path()).autonomy_ceiling, 0);
        // Malformed ⇒ STILL the locked-down default, not enabled.
        fs::write(&p, "{ not json").unwrap();
        assert_eq!(read_mcp_config(s.path()).autonomy_ceiling, 0);
        // Missing field ⇒ serde default 0 (fail safe).
        fs::write(&p, "{}").unwrap();
        assert_eq!(read_mcp_config(s.path()).autonomy_ceiling, 0);
        // Explicit value parses through.
        fs::write(&p, r#"{"autonomy_ceiling":1}"#).unwrap();
        assert_eq!(read_mcp_config(s.path()).autonomy_ceiling, 1);
    }

    #[test]
    fn mcp_config_flywheel_remediate_defaults_off_and_fails_safe() {
        // The explicit Default fails closed on autonomous remediation (the highest-consequence opt-in).
        assert!(!McpConfig::default().flywheel_remediate);
        // It is INDEPENDENT of the other gates — a Default has every flywheel gate off.
        let d = McpConfig::default();
        assert!(!d.flywheel_apply && !d.flywheel_ship && !d.flywheel_remediate);

        let s = Scratch::new("mcp-config-remediate");
        let p = mcp_config_path(s.path()).unwrap();
        // Absent ⇒ SAFE default (remediation OFF).
        assert!(!read_mcp_config(s.path()).flywheel_remediate);
        // Malformed ⇒ STILL locked down.
        fs::write(&p, "{ not json").unwrap();
        assert!(!read_mcp_config(s.path()).flywheel_remediate);
        // Missing field ⇒ serde default false (fail safe), even with the rest of the stack armed.
        fs::write(&p, r#"{"allow_mutations":true,"autonomy_ceiling":1,"flywheel_apply":true,"flywheel_ship":true}"#).unwrap();
        assert!(!read_mcp_config(s.path()).flywheel_remediate);
        // Only an explicit true enables — and round-trips.
        fs::write(&p, r#"{"flywheel_remediate":true}"#).unwrap();
        assert!(read_mcp_config(s.path()).flywheel_remediate);
    }

    #[test]
    fn mcp_config_require_repo_pin_defaults_off_and_round_trips() {
        assert!(!McpConfig::default().flywheel_require_repo_pin);
        let s = Scratch::new("mcp-config-repopin");
        let p = mcp_config_path(s.path()).unwrap();
        // absent ⇒ off; armed-stack-without-the-field ⇒ still off (fail safe).
        assert!(!read_mcp_config(s.path()).flywheel_require_repo_pin);
        fs::write(
            &p,
            r#"{"flywheel_apply":true,"flywheel_ship":true,"flywheel_remediate":true}"#,
        )
        .unwrap();
        assert!(!read_mcp_config(s.path()).flywheel_require_repo_pin);
        // explicit true enables.
        fs::write(&p, r#"{"flywheel_require_repo_pin":true}"#).unwrap();
        assert!(read_mcp_config(s.path()).flywheel_require_repo_pin);
    }

    #[test]
    fn mcp_config_prd_fast_defaults_off_and_round_trips() {
        assert!(!McpConfig::default().flywheel_prd_fast);
        let s = Scratch::new("mcp-config-prdfast");
        let p = mcp_config_path(s.path()).unwrap();
        // absent ⇒ off; an armed gate-stack without the field ⇒ still off (the timing knob is opt-in).
        assert!(!read_mcp_config(s.path()).flywheel_prd_fast);
        fs::write(&p, r#"{"flywheel_apply":true,"flywheel_ship":true}"#).unwrap();
        assert!(!read_mcp_config(s.path()).flywheel_prd_fast);
        // explicit true enables — and does not disturb the security gates.
        fs::write(&p, r#"{"flywheel_prd_fast":true}"#).unwrap();
        assert!(read_mcp_config(s.path()).flywheel_prd_fast);
        assert!(!read_mcp_config(s.path()).allow_mutations);
    }

    #[test]
    fn mcp_config_critique_defaults_off_and_round_trips() {
        assert!(!McpConfig::default().flywheel_critique);
        let s = Scratch::new("mcp-config-critique");
        let p = mcp_config_path(s.path()).unwrap();
        // absent ⇒ off; an armed gate-stack without the field ⇒ still off (the critique is opt-in).
        assert!(!read_mcp_config(s.path()).flywheel_critique);
        fs::write(&p, r#"{"flywheel_apply":true,"flywheel_ship":true}"#).unwrap();
        assert!(!read_mcp_config(s.path()).flywheel_critique);
        // explicit true enables — and does not disturb the security gates.
        fs::write(&p, r#"{"flywheel_critique":true}"#).unwrap();
        assert!(read_mcp_config(s.path()).flywheel_critique);
        assert!(!read_mcp_config(s.path()).allow_mutations);
    }

    #[test]
    fn mcp_config_unknown_keys_survive_typed_read_write_round_trip() {
        // Settings-RMW safety: a key this build doesn't model (another writer's sibling
        // setting, or a newer build's gate) must survive read → toggle → serialize —
        // previously the typed round-trip silently DELETED it.
        let s = Scratch::new("mcp-config-extra");
        let p = mcp_config_path(s.path()).unwrap();
        fs::write(
            &p,
            r#"{"allow_mutations":true,"insforge_dashboard":{"url":"http://x","pi":3.5},"future_gate":true}"#,
        )
        .unwrap();
        let mut cfg = read_mcp_config(s.path());
        assert!(cfg.allow_mutations, "typed field still parsed");
        assert_eq!(cfg.extra.len(), 2, "unknown keys captured: {:?}", cfg.extra);
        // the RMW: flip a typed field, write back the TYPED struct.
        cfg.allow_mutations = false;
        fs::write(&p, serde_json::to_string(&cfg).unwrap()).unwrap();
        // re-read RAW: the unknown keys are still there, byte-value intact.
        let raw: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(raw["insforge_dashboard"]["url"], "http://x");
        assert_eq!(raw["insforge_dashboard"]["pi"], 3.5);
        assert_eq!(raw["future_gate"], true);
        assert_eq!(
            raw["allow_mutations"], false,
            "the toggled field wrote through"
        );
        // and the typed reader still round-trips cleanly.
        let again = read_mcp_config(s.path());
        assert!(!again.allow_mutations);
        assert_eq!(again.extra.len(), 2);
        // a config with NO unknown keys serializes without any extra noise.
        let d = McpConfig::default();
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert!(
            v.get("extra").is_none(),
            "flatten: no literal `extra` key on the wire"
        );
    }
}
