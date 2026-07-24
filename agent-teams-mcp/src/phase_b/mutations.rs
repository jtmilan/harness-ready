//! Mutation tool surface SKELETON — `team_send_input` / `team_focus_workspace`.
//!
//! Defined as plain functions, NOT `#[tool]` methods on `TeamServer`, so the shape
//! is reviewable while the tools stay **unregistered** in the `#[tool_router]`
//! (registering them is the live wiring this task forbids). The future
//! `#[tool_router]` adopts these once the socket ([`super::socket`]) + auth
//! ([`super::auth`]) land.
//!
//! **Thin client, not business logic.** The app owns `send_input` / focus; each
//! tool here just builds a [`super::socket::SocketRequest`], dials, and maps the
//! reply. The `\n`-rule is enforced at the APP boundary (lib.rs:626) — the sidecar
//! passes `text` through verbatim (no divergent normalizer here).
//!
//! **Model A invariant.** A mutation routes the *human's* input and notifies; it
//! NEVER auto-approves an agent's pending prompt and NEVER auto-empties the ranked
//! queue (D9–D10). These fns take only `(id, text)` / `(id)` and pass the operator's
//! text through verbatim — they NEVER read queue / approval state and have NO branch
//! keyed on "y"/"yes". That ABSENCE is the invariant; the app boundary normalizes
//! and gates.

#![allow(dead_code)] // Wired into the (feature-gated) tool router by main.rs.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::socket::{self, response_code, SocketRequest};
use super::PhaseBError;
use crate::read_output::PaneOutputResult;
use agent_teams_core::{validate_spawn_id, PaneSourceWire, SocketData};

/// Args for the future `team_send_input` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SendInputArgs {
    /// Target workspace id (matches `QueueRow.id` / the live registry's `id`).
    pub id: String,
    /// Exactly ONE line of input. The **app** appends the single trailing `\n` at
    /// the `send_input` boundary (the `\n`-submits-TUI invariant); `text` MUST NOT
    /// contain interior newlines and MUST NOT pre-include the trailing one.
    /// Enforcement is APP-side — the sidecar transmits `text` verbatim.
    pub text: String,
}

/// Args for the future `team_focus_workspace` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FocusWorkspaceArgs {
    /// Workspace id to raise / focus in the running app.
    pub id: String,
}

/// Acknowledgement a mutation tool returns once implemented. Object-rooted (MCP
/// `outputSchema` discipline, Phase-A gotcha) via the `schemars` derive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct MutationAck {
    /// `true` iff the app applied the mutation.
    pub ok: bool,
    /// Human-readable detail (e.g. the app's [`super::socket::SocketResponse`] code
    /// echoed back). Never secret-bearing.
    pub detail: String,
}

/// Sidecar-side mutation outcome error. Distinguished from a transport/auth
/// failure so the MCP client gets an actionable, structured result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum MutationError {
    /// The app socket is absent / unreachable — the live app is required to mutate
    /// (AC-3). Reads still work app-independently off `events.jsonl`. Surfaces the
    /// [`APP_NOT_RUNNING`] code.
    AppNotRunning,
    /// The app replied with a non-`OK` [`super::socket::SocketResponse`] (e.g.
    /// `DEAD_PANE` from the D30 gate, `UNKNOWN_WORKSPACE`, `FORBIDDEN`,
    /// `MUTATIONS_DISABLED`) — carries the code AND the app's `detail` string. The
    /// detail is load-bearing: one code (FORBIDDEN) covers several distinct refusals
    /// (euid, coordinator-only, trusted-repo, read admission) — discarding it made the
    /// sidecar INVENT a cause and mislead the calling brain.
    Rejected { code: String, detail: String },
    /// Still unimplemented (scaffold) — mirrors [`PhaseBError::Incomplete`].
    Incomplete(&'static str),
}

/// The machine code the sidecar surfaces when the app socket is absent (AC-3).
/// This is a *sidecar* error, distinct from any
/// [`super::socket::response_code`] (those come from a *present* app).
pub const APP_NOT_RUNNING: &str = "APP_NOT_RUNNING";

/// Map the app's [`super::socket::SocketResponse`] (or a transport failure) into a
/// sidecar [`MutationError`] / [`MutationAck`]. Pure → unit-testable without I/O.
///
/// - transport `Incomplete` (socket absent / refused / wedged) ⇒ [`MutationError::AppNotRunning`].
/// - reply `ok:true` ⇒ [`MutationAck`].
/// - reply `ok:false` (e.g. `DEAD_PANE` from the D30 gate, `UNKNOWN_WORKSPACE`,
///   `MUTATIONS_DISABLED`, `FORBIDDEN`, `BAD_REQUEST`) ⇒ [`MutationError::Rejected`] —
///   never a silent vanish (AC-2).
pub fn map_reply(
    reply: Result<socket::SocketResponse, PhaseBError>,
) -> Result<MutationAck, MutationError> {
    match reply {
        // Any dial failure means we could not reach the live app.
        Err(_) => Err(MutationError::AppNotRunning),
        Ok(resp) if resp.ok => Ok(MutationAck {
            ok: true,
            detail: resp.detail,
        }),
        Ok(resp) => Err(MutationError::Rejected {
            code: resp.code,
            detail: resp.detail,
        }),
    }
}

/// `team_send_input` (Phase B). Builds [`SocketRequest::SendInput`]`{id,text}`,
/// dials the app socket, and maps the reply. The sidecar transmits `text`
/// **verbatim** — the `\n`-rule (one line + single trailing newline) and interior-
/// newline rejection are enforced at the APP boundary, not divergently here. Socket
/// absent ⇒ [`MutationError::AppNotRunning`] ([`APP_NOT_RUNNING`]); a non-`OK` reply
/// (e.g. the `is_alive()` / D30 dead-pane gate, or the `allow_mutations` gate) ⇒
/// [`MutationError::Rejected`] — never a silent vanish (AC-2). NEVER auto-approves
/// (Model A): there is no queue read and no branch on the text content.
pub fn team_send_input(
    socket_path: &Path,
    args: SendInputArgs,
) -> Result<MutationAck, MutationError> {
    let req = SocketRequest::SendInput {
        id: args.id,
        text: args.text,
    };
    map_reply(socket::dial(socket_path, &req))
}

/// `team_focus_workspace` (Phase B). Dials [`SocketRequest::Focus`]`{id}`; socket
/// absent ⇒ [`MutationError::AppNotRunning`]; an unknown id ⇒
/// [`MutationError::Rejected`]`(UNKNOWN_WORKSPACE)`.
pub fn team_focus_workspace(
    socket_path: &Path,
    args: FocusWorkspaceArgs,
) -> Result<MutationAck, MutationError> {
    let req = SocketRequest::Focus { id: args.id };
    map_reply(socket::dial(socket_path, &req))
}

/// A short, MCP-client-facing message for a [`MutationError`] (no secrets).
pub fn mutation_error_message(e: &MutationError) -> String {
    match e {
        MutationError::AppNotRunning => format!(
            "{APP_NOT_RUNNING}: the Agent Teams app is not running (mutations require the live app; reads still work)"
        ),
        // The app's own `detail` is authoritative — relay it VERBATIM (one code covers
        // several distinct refusals; inventing a cause here misled the calling brain,
        // e.g. a trusted-repo refusal surfaced as a bogus "same-user check" failure).
        MutationError::Rejected { code, detail } if !detail.trim().is_empty() => {
            format!("{code}: {detail}")
        }
        // Empty-detail fallbacks (older apps / codes whose reply carries no detail):
        MutationError::Rejected { code, .. } if code == response_code::DEAD_PANE => {
            "DEAD_PANE: the target workspace PTY is no longer alive (write rejected, not dropped)".to_string()
        }
        MutationError::Rejected { code, .. } if code == response_code::UNKNOWN_WORKSPACE => {
            "UNKNOWN_WORKSPACE: no such workspace id in the live set".to_string()
        }
        MutationError::Rejected { code, .. } if code == response_code::MUTATIONS_DISABLED => {
            "MUTATIONS_DISABLED: mutations are off in mcp-config.json (allow_mutations=false)".to_string()
        }
        MutationError::Rejected { code, .. } if code == response_code::FORBIDDEN => {
            "FORBIDDEN: the app refused this caller/op (no detail supplied — could be the \
             same-user gate, coordinator-only, the external pid-pin, or the trusted-repos \
             allowlist)".to_string()
        }
        MutationError::Rejected { code, .. } if code == response_code::DELEGATION_IN_FLIGHT => {
            "DELEGATION_IN_FLIGHT: a delegation is already running (one at a time) — wait for its write-back".to_string()
        }
        // WORKSPACE-ISOLATION (Phase 1): empty-detail fallback for the cross-workspace
        // denial. The app-side AuthErr Display ALWAYS carries a non-empty detail (it
        // includes the caller/target ws ids + op name), so this branch fires only on a
        // future regression or a stripped reply. The "do not retry" hint is preserved
        // so the coordinator agent reads it and does not spin retry loops against the
        // boundary.
        MutationError::Rejected { code, .. } if code == response_code::CROSS_WORKSPACE => {
            "CROSS_WORKSPACE: cross-workspace op denied by workspace isolation (the caller \
             and target workspaces are not mutually sharing, or the caller has no workspace \
             identity) — do not retry without operator intervention".to_string()
        }
        MutationError::Rejected { code, .. } => format!("rejected by the app: {code}"),
        MutationError::Incomplete(s) => (*s).to_string(),
    }
}

// ──────────────── 06-03 Context Router tool plumbing (gated, thin) ──────────────
//
// Three NET-NEW tools on top of the 06-02 mutation seam — `team_orchestrate` /
// `team_broadcast` / `team_handoff`. Like the 06-02 tools they are THIN dialers:
// each builds a [`SocketRequest`], dials the app socket, and maps the reply. The
// app owns the synthesis (`orchestrate`) + the gated writes — the sidecar adds NO
// business logic, NO second synthesizer. The ONLY difference from the 06-02 tools
// is that two of these ops return STRUCTURED DATA (a preview mapping / a fan-out
// result), so they map to [`RouterAck`] (carries an optional [`SocketData`]) rather
// than the bare [`MutationAck`].

/// Args for `team_orchestrate`. `dispatch` defaults FALSE (preview-first, D23) — an
/// MCP client that omits it gets the SAFE preview, never a blind fan-out.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct OrchestrateArgs {
    /// The team goal to synthesize into per-pane tasks (via the in-app synthesizer).
    pub goal: String,
    /// `false` (default) ⇒ PREVIEW: return the `{id,task}` mapping, dispatch nothing.
    /// `true` ⇒ DISPATCH: send each task to its pane through the gate. Preview-first.
    #[serde(default)]
    pub dispatch: bool,
    /// The workspace to fan out to — a workspace id like `ws76101x0` (or the create-time
    /// tag). SCOPES the fan-out to THAT workspace's panes only (pane ids embed the
    /// workspace as `${wsId}-p${idx}`). REQUIRED when more than one workspace is live —
    /// omitting it then is a `BAD_REQUEST` (the app refuses to blast every workspace);
    /// when exactly one workspace is live it may be omitted (unambiguous).
    #[serde(default)]
    pub target_workspace: Option<String>,
}

/// One pane group in an atomic `team_create_workspace` (#262 ext). `count` replicates this
/// exact harness/role/model triple, so a mixed team is a short list:
/// `[{harness:"claude",role:"builder",count:2},{harness:"codex",role:"reviewer"}]`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PaneSpecArg {
    /// Harness wire string ("claude"/"cursor"/"codex"/"commandcode"/"opencode"/"pi"/"grok"). NO bash/shell.
    pub harness: String,
    /// Optional role/persona for this pane group (e.g. "coordinator"/"reviewer"/"builder"/"tester").
    #[serde(default)]
    pub role: Option<String>,
    /// Optional per-pane model override (verbatim CLI model flag).
    #[serde(default)]
    pub model: Option<String>,
    /// How many panes of THIS spec to open. Default 1. The SUM across specs must be ≤ 4.
    #[serde(default)]
    pub count: Option<u32>,
}

/// Args for `team_create_workspace` (#262 ext: external visible-grid spawn).
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateWorkspaceArgs {
    /// Absolute repo path the workspace's worktrees derive under. MUST be in the
    /// trusted-repos allowlist (the app rejects otherwise).
    pub repo: String,
    /// PREFERRED for mixed teams: a per-pane spec list. When provided it OVERRIDES the scalar
    /// harness/count/role/model below, opening ONE workspace with all the panes. Total panes
    /// (sum of each spec's count) must be within the app's cap (default 8; operator-
    /// configurable via external_spawn_max_panes in mcp-config.json, max 16). Example:
    /// `[{"harness":"claude","role":"builder","count":2},{"harness":"codex","role":"reviewer"}]`.
    #[serde(default)]
    pub panes: Vec<PaneSpecArg>,
    /// Harness wire string ("claude"/"cursor"/"codex"/…). Default "claude". NO bash/shell.
    /// Ignored when `panes` is provided.
    #[serde(default)]
    pub harness: Option<String>,
    /// Panes to open (homogeneous). Default 1; the app caps the total (default 8, operator-configurable
    /// via external_spawn_max_panes in mcp-config.json, max 16). Ignored when `panes` is provided.
    #[serde(default)]
    pub count: Option<u32>,
    /// Optional typed role for every pane (homogeneous). Ignored when `panes` is provided.
    #[serde(default)]
    pub role: Option<String>,
    /// Optional per-pane model override (verbatim CLI model flag). Ignored when `panes` is provided.
    #[serde(default)]
    pub model: Option<String>,
    /// Optional correlation tag stamped into the workspace name so you can find YOUR
    /// new workspace on a subsequent list_workspaces (the spawn reply is async).
    #[serde(default)]
    pub tag: Option<String>,
}

/// Args for `team_add_pane` (add one pane to a TARGET workspace).
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AddPaneArgs {
    /// Harness for the new pane; default = the target workspace's harness. NO bash/shell.
    #[serde(default)]
    pub harness: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// Which workspace to add the pane to: a workspace id (wsNNNNN…) OR the `tag` you passed
    /// to team_create_workspace. When omitted, adds to whatever workspace is currently ACTIVE
    /// in the app (less reliable — prefer naming the target).
    #[serde(default)]
    pub target_workspace: Option<String>,
}

/// Args for `team_broadcast`. The operator's text, delivered VERBATIM to every live
/// pane (Model A — never an auto-approve).
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BroadcastArgs {
    /// Exactly ONE line; the app appends the trailing newline + gates each pane.
    pub text: String,
}

/// Args for `team_handoff`. ONE relay hop: a composed single-line message
/// (provenance `from` + `instruction`) is sent to pane `to` through the gate.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct HandoffArgs {
    /// The pane the work is handed off FROM (provenance; recorded in the message).
    pub from: String,
    /// The pane that RECEIVES the relayed instruction.
    pub to: String,
    /// The operator's single-line instruction for `to`.
    pub instruction: String,
}

/// Args for `team_delegate` (autonomous workers MVP, depth-1). The caller's own pane id
/// is NO LONGER an arg — the tool layer auto-fills it from `$AGENT_TEAMS_PANE_ID`
/// (supervisor-injected at spawn, the established `memory.rs`/`task.rs` pattern), so the
/// minimal call is `team_delegate{goal}` and an agent can no longer typo its own id into a
/// silent `UNKNOWN_WORKSPACE`. `max_workers` (default 3) is clamped app-side to the fairness
/// budget (`cap-1`); `depth` (default 1) is rejected app-side if >1 (no recursion in the MVP).
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DelegateArgs {
    /// The goal the invisible workers should autonomously pursue.
    pub goal: String,
    /// Number of invisible PTY workers to spawn (optional, default 3). Clamped
    /// app-side to `min(this, max_concurrent-1, 3)` so a human slot is always reserved.
    #[serde(default)]
    pub max_workers: Option<u32>,
    /// Delegation depth (optional, default 1). The MVP rejects depth>1 (no recursion).
    #[serde(default)]
    pub depth: Option<u32>,
}

/// Args for `team_synthesize` — TWO modes (06-05 dir fan-in + Phase 21 pane_ids).
/// Exactly ONE of `dir` / `pane_ids` must be given; both or neither is a clear
/// `BAD_REQUEST` at the args boundary ([`synthesize_mode`]). SERDE-ADDITIVE: the old
/// wire form `{dir,goal?}` still deserializes unchanged (`dir` merely became
/// `Option`, and an absent `pane_ids` defaults to `None`).
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SynthesizeArgs {
    /// DIR MODE (app-side): absolute path to the Bridge run directory — the folder
    /// holding `manifest.json` (the authoritative dispatched-id set) and each
    /// dispatched pane's `<id>.md` report. Fan-in reads EXACTLY the manifest's ids;
    /// `final.md` is written into this dir. Requires the app running.
    #[serde(default)]
    pub dir: Option<String>,
    /// PANE_IDS MODE (Phase 21, sidecar-local): FULL pane ids (e.g. "ws50144x0-p4",
    /// max [`SYNTH_PANE_IDS_CAP`]) to consolidate WITHOUT an orchestrate-minted run
    /// dir — each pane's produced output is resolved from disk exactly like
    /// `team_read_output` (report → transcript → honest `source:"none"`) and merged
    /// into ONE markdown document. Never dials the app; works with the app closed.
    #[serde(default)]
    pub pane_ids: Option<Vec<String>>,
    /// The shared goal the panes worked toward (frames the consolidation, anti-injection
    /// guarded app-side). Optional — a neutral default is used when omitted/empty.
    #[serde(default)]
    pub goal: Option<String>,
}

/// Hard cap on `pane_ids` per `team_synthesize` call (Phase 21) — bounds the disk
/// reads and the reply size (each pane's content is itself tail-capped by the
/// `team_read_output` resolver's default).
pub const SYNTH_PANE_IDS_CAP: usize = 16;

/// Which mode a `team_synthesize` call selected — exactly ONE of `dir` / `pane_ids`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SynthesizeMode {
    /// App-side fan-in over an orchestrate-minted run dir (06-05, dials the socket).
    Dir(String),
    /// Sidecar-local consolidation of the named panes' on-disk outputs (Phase 21).
    Panes(Vec<String>),
}

/// Validate the `team_synthesize` mode selection at the args boundary. Pure →
/// unit-testable without I/O. Every rejection is a clear `BAD_REQUEST: …` string
/// (the tool layer surfaces it verbatim — never a silent default):
/// - BOTH or NEITHER of `dir` / `pane_ids` ⇒ Err naming exactly what to pass;
/// - `pane_ids`: empty ⇒ Err; more than [`SYNTH_PANE_IDS_CAP`] ⇒ Err; every id must
///   pass [`validate_spawn_id`] (rejects `..` / `/` / `\` / whitespace; len-capped) —
///   traversal is structurally impossible before any path is built.
pub fn synthesize_mode(args: &SynthesizeArgs) -> Result<SynthesizeMode, String> {
    match (&args.dir, &args.pane_ids) {
        (Some(_), Some(_)) => Err(
            "BAD_REQUEST: pass exactly ONE of `dir` (app-side fan-in over an orchestrate \
             run dir) or `pane_ids` (sidecar-local consolidation of those panes' outputs) \
             — not both"
                .into(),
        ),
        (None, None) => Err(
            "BAD_REQUEST: pass exactly ONE of `dir` (app-side fan-in over an orchestrate \
             run dir) or `pane_ids` (sidecar-local consolidation of those panes' outputs) \
             — got neither"
                .into(),
        ),
        (Some(d), None) => {
            if d.trim().is_empty() {
                return Err("BAD_REQUEST: `dir` is empty — pass the absolute run directory".into());
            }
            Ok(SynthesizeMode::Dir(d.clone()))
        }
        (None, Some(ids)) => {
            if ids.is_empty() {
                return Err("BAD_REQUEST: `pane_ids` is empty — name at least one pane id".into());
            }
            if ids.len() > SYNTH_PANE_IDS_CAP {
                return Err(format!(
                    "BAD_REQUEST: {} pane_ids exceeds the cap of {SYNTH_PANE_IDS_CAP} per call",
                    ids.len()
                ));
            }
            if let Some(bad) = ids.iter().find(|id| !validate_spawn_id(id)) {
                return Err(format!(
                    "BAD_REQUEST: invalid pane id {bad:?} — pass FULL ids like \
                     \"ws50144x0-p4\" (only [A-Za-z0-9_-]; never a filesystem path)"
                ));
            }
            Ok(SynthesizeMode::Panes(ids.clone()))
        }
    }
}

/// Acknowledgement a Context Router tool returns. Object-rooted (MCP `outputSchema`
/// discipline) and carries the OPTIONAL typed [`SocketData`] payload — the
/// `team_orchestrate{dispatch:false}` preview mapping or the `team_broadcast`
/// `{sent,skipped}` result. `data` is `null` for ops that only ack (handoff, a
/// dispatched orchestrate still returns its `{sent,skipped}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct RouterAck {
    /// `true` iff the app applied the op.
    pub ok: bool,
    /// Human-readable detail echoed from the app (never secret-bearing).
    pub detail: String,
    /// The typed payload (preview mapping / broadcast result), or `null`.
    pub data: Option<SocketData>,
}

/// Map the app's [`socket::SocketResponse`] (or a transport failure) into a
/// [`RouterAck`] / [`MutationError`], PRESERVING the structured `data` payload. Pure
/// → unit-testable. Same code-mapping as [`map_reply`]; the only addition is that an
/// `ok` reply carries its `data` through to the host (the preview mapping / fan-out
/// result), and a non-`OK` reply is still a structured [`MutationError::Rejected`] —
/// never a silent vanish.
pub fn map_router_reply(
    reply: Result<socket::SocketResponse, PhaseBError>,
) -> Result<RouterAck, MutationError> {
    match reply {
        Err(_) => Err(MutationError::AppNotRunning),
        Ok(resp) if resp.ok => Ok(RouterAck {
            ok: true,
            detail: resp.detail,
            data: resp.data,
        }),
        Ok(resp) => Err(MutationError::Rejected {
            code: resp.code,
            detail: resp.detail,
        }),
    }
}

/// `team_orchestrate` (06-03). Dials [`SocketRequest::Orchestrate`]`{goal,dispatch,
/// target_workspace}` — the app runs the EXISTING synthesizer; `dispatch:false` returns
/// the mapping (preview, D23), `dispatch:true` fans it out. `target_workspace` (B4) scopes
/// the fan-out to one workspace's panes; the app refuses (`BAD_REQUEST`) when it is omitted
/// AND >1 workspace is live. The sidecar adds NO synthesis. This op needs the LONG read
/// window (the app runs headless claude up to 120s), so it dials via the per-op SSOT timeout.
pub fn team_orchestrate(
    socket_path: &Path,
    args: OrchestrateArgs,
) -> Result<RouterAck, MutationError> {
    let req = SocketRequest::Orchestrate {
        goal: args.goal,
        dispatch: args.dispatch,
        target_workspace: args.target_workspace,
    };
    map_router_reply(socket::dial_op(socket_path, &req))
}

/// `team_broadcast` (06-03). Dials [`SocketRequest::Broadcast`]`{text}`; the app
/// sends it to every live pane through the gate and returns `{sent,skipped}`. Model
/// A: the operator's text verbatim — no queue read, no "y"/"yes" branch.
pub fn team_broadcast(socket_path: &Path, args: BroadcastArgs) -> Result<RouterAck, MutationError> {
    let req = SocketRequest::Broadcast { text: args.text };
    map_router_reply(socket::dial_op(socket_path, &req))
}

/// `team_handoff` (06-03). Dials [`SocketRequest::Handoff`]`{from,to,instruction}`;
/// the app composes ONE single-line message and relays it to `to` through the gate.
/// ONE hop — no multi-turn loop.
pub fn team_handoff(socket_path: &Path, args: HandoffArgs) -> Result<RouterAck, MutationError> {
    let req = SocketRequest::Handoff {
        from: args.from,
        to: args.to,
        instruction: args.instruction,
    };
    map_router_reply(socket::dial_op(socket_path, &req))
}

/// `team_synthesize` DIR MODE (06-05 fan-in). Dials [`SocketRequest::Synthesize`]`{dir,goal}`;
/// the app reads the run dir's manifest + per-pane reports, runs the EXISTING in-app
/// synthesizer (no second copy) into `final.md`, and returns the path + the per-pane
/// `verify_dispatched` verdicts in [`SocketData::Synthesis`]. WRAPS headless claude → the
/// LONG read window (`dial_op` resolves it via the per-op SSOT timeout). `goal` defaults
/// to empty (the app substitutes a neutral consolidation objective). Takes the RESOLVED
/// dir (post-[`synthesize_mode`]); the Phase 21 `pane_ids` mode never dials — see
/// [`synthesize_panes_local`].
pub fn team_synthesize(
    socket_path: &Path,
    dir: String,
    goal: Option<String>,
) -> Result<RouterAck, MutationError> {
    let req = SocketRequest::Synthesize {
        dir,
        goal: goal.unwrap_or_default(),
    };
    map_router_reply(socket::dial_op(socket_path, &req))
}

// ──────────────── Phase 21: team_synthesize pane_ids mode (sidecar-local) ────────────
//
// Fan-in for a HAND-SPAWNED team: no orchestrate-minted run dir (no manifest.json, no
// per-pane <id>.md contract), so the app-side 06-05 path is a guaranteed BAD_REQUEST.
// This mode consolidates ANY set of panes by resolving each pane's produced output off
// disk via the EXISTING `team_read_output` resolver (`crate::read_output::resolve` —
// one locator, never a second copy) and merging the results into one markdown document.
// DELIBERATELY SIDECAR-LOCAL (asymmetric with dir mode, which dials the app and runs
// an LLM pass): pure assembly needs only disk reads, so it works with the app closed
// and adds no LLM cost. The output is CLAIMED — assembled from pane outputs, never
// verified.

/// Merge resolved pane outputs into ONE consolidated markdown document. Pure —
/// unit-testable without disk. Returns `(document, per-pane wire rows)`.
///
/// HONEST BY CONSTRUCTION: every requested pane appears BOTH in the summary table and
/// as its own section; a `source:"none"` pane shows its resolver note instead of
/// content — never silently dropped. Each found pane's section header carries the id,
/// the resolved source, and whether the content is a truncated tail.
pub fn merge_pane_outputs(
    goal: &str,
    results: &[PaneOutputResult],
) -> (String, Vec<PaneSourceWire>) {
    let panes: Vec<PaneSourceWire> = results
        .iter()
        .map(|r| PaneSourceWire {
            id: r.id.clone(),
            source: r.source.clone(),
            bytes: r.content.as_ref().map(|c| c.len() as u64).unwrap_or(0),
            truncated: r.truncated,
        })
        .collect();

    let found = panes.iter().filter(|p| p.source != "none").count();
    let goal_line = if goal.trim().is_empty() {
        "(none supplied)"
    } else {
        goal
    };

    let mut doc = String::new();
    doc.push_str("# Consolidated pane outputs (CLAIMED)\n\n");
    doc.push_str(&format!("Goal: {goal_line}\n\n"));
    doc.push_str(
        "Assembled sidecar-locally by `team_synthesize{pane_ids}` from each pane's \
         on-disk report/transcript (the `team_read_output` resolution). NO verification \
         was run — treat every claim below as [CLAIMED].\n\n",
    );

    // Summary table: which panes yielded output vs source:"none".
    doc.push_str("## Summary\n\n");
    doc.push_str(&format!(
        "{found} of {} pane(s) yielded output; {} with `source:\"none\"`.\n\n",
        panes.len(),
        panes.len() - found
    ));
    doc.push_str("| pane | source | bytes | truncated |\n|---|---|---|---|\n");
    for p in &panes {
        doc.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            p.id,
            p.source,
            p.bytes,
            if p.truncated { "yes" } else { "no" }
        ));
    }
    doc.push('\n');

    // One section per pane: header (id + source + truncated) then the content verbatim,
    // or the honest no-output note.
    for r in results {
        if r.source == "none" {
            doc.push_str(&format!("## {} — source: none\n\n", r.id));
            doc.push_str("_No output on disk for this pane._");
            if let Some(note) = r.note.as_deref().filter(|n| !n.trim().is_empty()) {
                doc.push_str(&format!(" {note}"));
            }
            doc.push_str("\n\n");
        } else {
            doc.push_str(&format!(
                "## {} — source: {}{}\n\n",
                r.id,
                r.source,
                if r.truncated {
                    " (TRUNCATED: newest tail only)"
                } else {
                    ""
                }
            ));
            doc.push_str(r.content.as_deref().unwrap_or(""));
            if !doc.ends_with('\n') {
                doc.push('\n');
            }
            doc.push('\n');
        }
    }
    (doc, panes)
}

/// `team_synthesize{pane_ids}` (Phase 21) — SIDECAR-LOCAL fan-in. Resolves each pane's
/// produced output from disk via the EXISTING `team_read_output` resolver (ids MUST be
/// pre-validated via [`synthesize_mode`]; `resolve` re-validates each defensively),
/// merges them with [`merge_pane_outputs`], writes the document to a SERVER-CHOSEN
/// synth dir beside the orchestrate run dirs
/// (`<state_root_parent>/agent-teams-synth/<run_id>/final.md` — never a caller-supplied
/// path, mirroring dir mode's final.md write), and returns a [`RouterAck`] carrying
/// [`SocketData::PaneSynthesis`] `{report_path, run_id, content, panes}` — the document
/// ITSELF rides the reply (unlike dir mode's path-only [`SocketData::Synthesis`])
/// because this mode must work with the app closed. Err(String) is a message the tool
/// layer surfaces verbatim (e.g. a failed write) — never a partial silent success.
pub fn synthesize_panes_local(
    state_dir: &Path,
    ids: &[String],
    goal: &str,
) -> Result<RouterAck, String> {
    let results: Vec<PaneOutputResult> = ids
        .iter()
        .map(|id| crate::read_output::resolve(state_dir, id, None))
        .collect();
    let (doc, panes) = merge_pane_outputs(goal, &results);

    // Server-chosen synth dir, a sibling of the `agent-teams-orchestrate` run-dir root
    // (both live beside the wiped state root). Nanos + pid keep concurrent calls apart.
    let parent = state_dir
        .parent()
        .ok_or_else(|| "no parent for state dir — cannot mint the synth dir".to_string())?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let run_id = format!("synth-{nanos}-{}", std::process::id());
    let synth_dir = parent.join("agent-teams-synth").join(&run_id);
    std::fs::create_dir_all(&synth_dir).map_err(|e| format!("create synth dir: {e}"))?;
    let write_path = synth_dir.join("final.md");
    std::fs::write(&write_path, &doc).map_err(|e| format!("write final.md: {e}"))?;

    // Post-run knowledge harvest (gate `memory_harvest`, default OFF ⇒ 0 ⇒ the ack below
    // stays byte-identical): final.md is written, so extract the explicitly-marked
    // `LESSON:` lines from the resolved pane outputs. The caller's `goal` arg drives the
    // lineage links (empty ⇒ sibling mesh only). Compiled only when the store crate is
    // linked (`memory-notes` — the shipped coordinator sidecar carries BOTH features);
    // a phase-b-only build has no store to write and harvests nothing. Never fails the op.
    #[cfg(feature = "memory-notes")]
    let harvested: usize = {
        let reports: Vec<(String, String)> = results
            .iter()
            .filter_map(|r| r.content.as_ref().map(|c| (r.id.clone(), c.clone())))
            .collect();
        crate::memory::harvest_reports(state_dir, &run_id, &reports, goal)
    };
    #[cfg(not(feature = "memory-notes"))]
    let harvested: usize = 0;

    let found = panes.iter().filter(|p| p.source != "none").count();
    let mut detail = format!(
        "consolidated {found}/{} pane output(s) → {} (sidecar-local, CLAIMED — \
         assembled from on-disk reports/transcripts, not verified)",
        panes.len(),
        write_path.to_string_lossy(),
    );
    // Surface the harvest honestly on the EXISTING ack detail — only when N>0.
    if harvested > 0 {
        detail.push_str(&format!(" — harvested {harvested} lesson(s)"));
    }
    Ok(RouterAck {
        ok: true,
        detail,
        data: Some(SocketData::PaneSynthesis {
            report_path: write_path.to_string_lossy().into_owned(),
            run_id,
            content: doc,
            panes,
        }),
    })
}

/// `team_delegate` (autonomous workers MVP). Dials [`SocketRequest::Delegate`]
/// `{parent_id,goal,max_workers,depth}`; the app's `socket_delegate` controller spawns
/// invisible PTY workers, fans their results in, and writes ONE line back to
/// `parent_id` — the socket reply is a FAST ack carrying `{run_id,workers}` (the real
/// work runs detached app-side), so this dials via the per-op timeout like the other
/// router ops. App-side double-gated: `allow_mutations` AND `autonomy_ceiling>=1`;
/// depth>1 ⇒ DEPTH_EXCEEDED. Defaults: `max_workers`=3, `depth`=1. `parent_id` is now
/// resolved by the tool layer (`$AGENT_TEAMS_PANE_ID`) and passed in, not carried on args.
pub fn team_delegate(
    socket_path: &Path,
    parent_id: String,
    args: DelegateArgs,
) -> Result<RouterAck, MutationError> {
    let req = SocketRequest::Delegate {
        parent_id,
        goal: args.goal,
        max_workers: args
            .max_workers
            .unwrap_or(agent_teams_core::DELEGATE_MAX_WORKERS),
        depth: args.depth.unwrap_or(1),
    };
    map_router_reply(socket::dial_op(socket_path, &req))
}

/// The Phase-B mutation tool names — the surface the future `#[tool_router]` will
/// register once IPC + auth land. Listed so the shape is reviewable while unwired.
pub const MUTATION_TOOLS: &[&str] = &["team_send_input", "team_focus_workspace"];

/// The autonomous-workers tool name (registered alongside the mutation/router tools
/// under `phase-b-mutations`, gated by `allow_mutations` + `autonomy_ceiling`).
pub const DELEGATE_TOOLS: &[&str] = &["team_delegate"];

/// The 06-03 Context Router tool names (registered alongside the mutation tools
/// under `phase-b-mutations`). Listed so the gated surface is reviewable.
pub const CONTEXT_ROUTER_TOOLS: &[&str] = &[
    "team_orchestrate",
    "team_broadcast",
    "team_prompt_all",
    "team_handoff",
    "team_synthesize",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dial_to_absent_socket_maps_to_app_not_running() {
        // No app ⇒ connect refused ⇒ the structured APP_NOT_RUNNING outcome, never
        // a panic and never a silent success.
        let nope = std::env::temp_dir().join("agent-teams-mut-DEFINITELY-ABSENT.sock");
        let _ = std::fs::remove_file(&nope);
        let r = team_send_input(
            &nope,
            SendInputArgs {
                id: "w".into(),
                text: "approve".into(),
            },
        );
        assert_eq!(r, Err(MutationError::AppNotRunning));
        let r = team_focus_workspace(&nope, FocusWorkspaceArgs { id: "w".into() });
        assert_eq!(r, Err(MutationError::AppNotRunning));
    }

    #[test]
    fn map_reply_distinguishes_ok_rejected_and_app_down() {
        use super::socket::SocketResponse;
        // ok:true ⇒ ack
        assert_eq!(
            map_reply(Ok(SocketResponse::ok("sent"))),
            Ok(MutationAck {
                ok: true,
                detail: "sent".into()
            })
        );
        // ok:false (D30 dead-pane gate) ⇒ Rejected, NOT a silent vanish (AC-2);
        // the app's detail rides along.
        assert_eq!(
            map_reply(Ok(SocketResponse::err(response_code::DEAD_PANE, "dead"))),
            Err(MutationError::Rejected {
                code: response_code::DEAD_PANE.to_string(),
                detail: "dead".to_string(),
            })
        );
        // transport failure ⇒ AppNotRunning (AC-3)
        assert_eq!(
            map_reply(Err(PhaseBError::Incomplete("x"))),
            Err(MutationError::AppNotRunning)
        );
    }

    #[test]
    fn error_messages_are_structured_and_secret_free() {
        let m = mutation_error_message(&MutationError::AppNotRunning);
        assert!(m.starts_with(APP_NOT_RUNNING));
        let m = mutation_error_message(&MutationError::Rejected {
            code: response_code::MUTATIONS_DISABLED.to_string(),
            detail: String::new(),
        });
        assert!(m.contains("MUTATIONS_DISABLED"));
    }

    #[test]
    fn mutation_error_message_covers_every_arm() {
        use MutationError::*;
        // Each SPECIFIC arm yields a distinctive substring the generic
        // "rejected by the app: {code}" fallback never contains — so an arm
        // reorder/drop or a constant drift that fell through to the generic arm
        // is caught (the existing test only checks AppNotRunning + MUTATIONS_DISABLED).
        fn rej(code: &str, detail: &str) -> MutationError {
            MutationError::Rejected {
                code: code.to_string(),
                detail: detail.to_string(),
            }
        }
        assert!(mutation_error_message(&AppNotRunning).contains("not running"));
        // The app's detail is AUTHORITATIVE — relayed verbatim, whatever the code.
        // (Regression: a trusted-repo FORBIDDEN used to surface as a bogus
        // "same-user check" — the brain relayed the invented cause to the operator.)
        let m = mutation_error_message(&rej(
            response_code::FORBIDDEN,
            "external spawn: repo is not in the trusted-repos allowlist (trust it in Agent Teams first)",
        ));
        assert!(
            m.contains("trusted-repos allowlist") && m.contains("FORBIDDEN"),
            "{m}"
        );
        assert!(
            !m.contains("euid"),
            "must NOT invent a same-user cause: {m}"
        );
        // Empty-detail fallbacks keep the canned, code-specific texts.
        assert!(
            mutation_error_message(&rej(response_code::DEAD_PANE, "")).contains("no longer alive")
        );
        assert!(
            mutation_error_message(&rej(response_code::UNKNOWN_WORKSPACE, ""))
                .contains("no such workspace")
        );
        assert!(
            mutation_error_message(&rej(response_code::MUTATIONS_DISABLED, ""))
                .contains("mutations are off")
        );
        // FORBIDDEN with no detail: honest ambiguity — names the candidate gates, asserts none.
        let f = mutation_error_message(&rej(response_code::FORBIDDEN, ""));
        assert!(
            f.contains("no detail") && f.contains("trusted-repos"),
            "{f}"
        );
        // WORKSPACE-ISOLATION (Phase 1): CROSS_WORKSPACE with empty detail yields the
        // canned "do not retry" message (the common case carries a non-empty detail
        // from AuthErr's Display — relayed verbatim by the detail-is-nonempty arm).
        let cw = mutation_error_message(&rej(response_code::CROSS_WORKSPACE, ""));
        assert!(
            cw.contains("CROSS_WORKSPACE") && cw.contains("do not retry"),
            "{cw}"
        );
        // CROSS_WORKSPACE with detail: the app's authoritative detail is relayed verbatim
        // (the coordinator agent reads the "caller_ws=…, target_ws=…" text).
        let cwd = mutation_error_message(&rej(
            response_code::CROSS_WORKSPACE,
            "CROSS_WORKSPACE: cross-workspace op 'write_to_pane' denied: caller_ws='ws1', target_ws='ws2' (at least one has allow_sharing=false)",
        ));
        assert!(
            cwd.contains("CROSS_WORKSPACE") && cwd.contains("allow_sharing=false"),
            "{cwd}"
        );
        // an UNRECOGNIZED code falls through to the generic arm (echoing the code).
        let g = mutation_error_message(&rej("WAT_UNKNOWN", ""));
        assert!(g.contains("rejected by the app") && g.contains("WAT_UNKNOWN"));
        // Incomplete carries its &'static str verbatim.
        assert!(mutation_error_message(&Incomplete("scaffolded")).contains("scaffolded"));
    }

    #[test]
    fn tool_surface_and_codes_are_stable() {
        assert_eq!(MUTATION_TOOLS, &["team_send_input", "team_focus_workspace"]);
        assert_eq!(APP_NOT_RUNNING, "APP_NOT_RUNNING");
    }

    // ───────────────── 06-03 Context Router tool plumbing ───────────────────────

    #[test]
    fn context_router_tool_surface_is_stable() {
        assert_eq!(
            CONTEXT_ROUTER_TOOLS,
            &[
                "team_orchestrate",
                "team_broadcast",
                "team_prompt_all",
                "team_handoff",
                "team_synthesize"
            ]
        );
    }

    #[test]
    fn map_router_reply_preserves_data_payload_and_distinguishes_outcomes() {
        use super::socket::SocketResponse;
        use agent_teams_core::{DispatchEntry, SocketData};

        // ok + Mapping payload (the dispatch:false preview) flows through to the host.
        let resp = SocketResponse::ok("preview").with_data(SocketData::Mapping {
            tasks: vec![DispatchEntry {
                id: "a".into(),
                task: "do A".into(),
            }],
        });
        let ack = map_router_reply(Ok(resp)).expect("ok reply");
        assert!(ack.ok);
        match ack.data.expect("mapping payload preserved") {
            SocketData::Mapping { tasks } => assert_eq!(tasks[0].id, "a"),
            other => panic!("expected Mapping, got {other:?}"),
        }
        // ok + Broadcast payload preserved.
        let resp = SocketResponse::ok("broadcast").with_data(SocketData::Broadcast {
            sent: vec!["a".into()],
            skipped: vec!["d".into()],
        });
        let ack = map_router_reply(Ok(resp)).expect("ok reply");
        assert!(matches!(ack.data, Some(SocketData::Broadcast { .. })));
        // non-OK reply ⇒ Rejected (NOT a silent vanish), carrying the code.
        assert_eq!(
            map_router_reply(Ok(SocketResponse::err(response_code::DEAD_PANE, "dead"))),
            Err(MutationError::Rejected {
                code: response_code::DEAD_PANE.to_string(),
                detail: "dead".to_string(), // the app's detail rides along — never dropped
            })
        );
        // transport failure ⇒ AppNotRunning.
        assert_eq!(
            map_router_reply(Err(PhaseBError::Incomplete("x"))),
            Err(MutationError::AppNotRunning)
        );
        // a payload-less ok reply ⇒ data is None (handoff acks this way).
        let ack = map_router_reply(Ok(SocketResponse::ok("handoff delivered"))).unwrap();
        assert!(ack.data.is_none());
    }

    #[test]
    fn router_dials_to_absent_socket_map_to_app_not_running() {
        // No app ⇒ connect refused ⇒ structured APP_NOT_RUNNING for all three tools,
        // never a panic and never a silent success.
        let nope = std::env::temp_dir().join("agent-teams-router-DEFINITELY-ABSENT.sock");
        let _ = std::fs::remove_file(&nope);
        assert_eq!(
            team_orchestrate(
                &nope,
                OrchestrateArgs {
                    goal: "g".into(),
                    dispatch: false,
                    target_workspace: None
                }
            ),
            Err(MutationError::AppNotRunning)
        );
        assert_eq!(
            team_broadcast(&nope, BroadcastArgs { text: "hi".into() }),
            Err(MutationError::AppNotRunning)
        );
        assert_eq!(
            team_handoff(
                &nope,
                HandoffArgs {
                    from: "a".into(),
                    to: "b".into(),
                    instruction: "go".into()
                }
            ),
            Err(MutationError::AppNotRunning)
        );
    }

    // ───────────────── Phase 21: team_synthesize pane_ids mode ──────────────────

    fn unique_root(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "at-mcp-synthpanes-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ))
    }

    #[test]
    fn synthesize_mode_requires_exactly_one_of_dir_and_pane_ids() {
        // BOTH ⇒ a clear BAD_REQUEST naming the choice.
        let both = SynthesizeArgs {
            dir: Some("/tmp/run".into()),
            pane_ids: Some(vec!["ws9-p0".into()]),
            goal: None,
        };
        let e = synthesize_mode(&both).unwrap_err();
        assert!(e.starts_with("BAD_REQUEST"), "{e}");
        assert!(e.contains("not both"), "{e}");

        // NEITHER ⇒ same clarity.
        let neither = SynthesizeArgs {
            dir: None,
            pane_ids: None,
            goal: None,
        };
        let e = synthesize_mode(&neither).unwrap_err();
        assert!(e.starts_with("BAD_REQUEST"), "{e}");
        assert!(e.contains("got neither"), "{e}");

        // Exactly one ⇒ the matching mode.
        let dir = SynthesizeArgs {
            dir: Some("/tmp/run".into()),
            pane_ids: None,
            goal: None,
        };
        assert_eq!(
            synthesize_mode(&dir).unwrap(),
            SynthesizeMode::Dir("/tmp/run".into())
        );
        let panes = SynthesizeArgs {
            dir: None,
            pane_ids: Some(vec!["ws9-p0".into(), "ws9-p1".into()]),
            goal: None,
        };
        assert_eq!(
            synthesize_mode(&panes).unwrap(),
            SynthesizeMode::Panes(vec!["ws9-p0".into(), "ws9-p1".into()])
        );

        // Empty selections are rejected, not silently treated as "the other mode".
        let empty_dir = SynthesizeArgs {
            dir: Some("  ".into()),
            pane_ids: None,
            goal: None,
        };
        assert!(synthesize_mode(&empty_dir)
            .unwrap_err()
            .contains("`dir` is empty"));
        let empty_ids = SynthesizeArgs {
            dir: None,
            pane_ids: Some(vec![]),
            goal: None,
        };
        assert!(synthesize_mode(&empty_ids)
            .unwrap_err()
            .contains("`pane_ids` is empty"));
    }

    #[test]
    fn synthesize_mode_validates_ids_and_caps_the_list() {
        // A traversal-shaped id is refused at the boundary (validate_spawn_id).
        for bad in ["../escape", "/etc/passwd", "ws..-p0", "a b"] {
            let args = SynthesizeArgs {
                dir: None,
                pane_ids: Some(vec!["ws9-p0".into(), bad.into()]),
                goal: None,
            };
            let e = synthesize_mode(&args).unwrap_err();
            assert!(e.contains("invalid pane id"), "{bad:?} → {e}");
            assert!(e.contains(bad), "the offending id is named: {e}");
        }
        // 17 ids ⇒ over the cap of 16; exactly 16 is allowed.
        let ids17: Vec<String> = (0..17).map(|i| format!("ws9-p{i}")).collect();
        let args = SynthesizeArgs {
            dir: None,
            pane_ids: Some(ids17),
            goal: None,
        };
        let e = synthesize_mode(&args).unwrap_err();
        assert!(e.contains("cap of 16"), "{e}");
        let ids16: Vec<String> = (0..16).map(|i| format!("ws9-p{i}")).collect();
        let args = SynthesizeArgs {
            dir: None,
            pane_ids: Some(ids16.clone()),
            goal: None,
        };
        assert_eq!(
            synthesize_mode(&args).unwrap(),
            SynthesizeMode::Panes(ids16)
        );
    }

    #[test]
    fn synthesize_args_are_serde_additive() {
        // The OLD wire form {dir[,goal]} still deserializes unchanged (dir mode).
        let old: SynthesizeArgs = serde_json::from_str(r#"{"dir":"/tmp/run-1"}"#).unwrap();
        assert_eq!(old.dir.as_deref(), Some("/tmp/run-1"));
        assert!(old.pane_ids.is_none());
        assert_eq!(
            synthesize_mode(&old).unwrap(),
            SynthesizeMode::Dir("/tmp/run-1".into())
        );
        // The NEW form {pane_ids[,goal]} deserializes into panes mode.
        let new: SynthesizeArgs =
            serde_json::from_str(r#"{"pane_ids":["ws9-p0"],"goal":"ship"}"#).unwrap();
        assert!(new.dir.is_none());
        assert_eq!(
            synthesize_mode(&new).unwrap(),
            SynthesizeMode::Panes(vec!["ws9-p0".into()])
        );
    }

    #[test]
    fn merge_pane_outputs_keeps_every_pane_visible_including_none() {
        let results = vec![
            PaneOutputResult {
                id: "ws9-p0".into(),
                harness: Some("claude".into()),
                source: "orchestrate_report".into(),
                path: Some("/x/run-1/ws9-p0.md".into()),
                content: Some("p0 did the thing".into()),
                truncated: false,
                note: None,
            },
            PaneOutputResult {
                id: "ws9-p1".into(),
                harness: Some("codex".into()),
                source: "none".into(),
                path: None,
                content: None,
                truncated: false,
                note: Some("no on-disk transcript for this harness".into()),
            },
            PaneOutputResult {
                id: "ws9-p2".into(),
                harness: Some("claude".into()),
                source: "claude_transcript".into(),
                path: Some("/x/t.jsonl".into()),
                content: Some("tail of p2".into()),
                truncated: true,
                note: None,
            },
        ];
        let (doc, panes) = merge_pane_outputs("ship the feature", &results);

        // Honesty banner + goal framing.
        assert!(doc.contains("CLAIMED"), "the doc is marked CLAIMED:\n{doc}");
        assert!(doc.contains("Goal: ship the feature"));
        // Summary table: one row per pane, none-pane included, counts honest.
        assert!(doc.contains("2 of 3 pane(s) yielded output; 1 with `source:\"none\"`"));
        assert!(doc.contains("| ws9-p0 | orchestrate_report |"));
        assert!(doc.contains("| ws9-p1 | none | 0 | no |"));
        assert!(doc.contains("| ws9-p2 | claude_transcript |"));
        // Per-pane sections: header carries id + source + truncated flag; content verbatim;
        // the none-pane appears WITH its note — never dropped.
        assert!(doc.contains("## ws9-p0 — source: orchestrate_report\n"));
        assert!(doc.contains("p0 did the thing"));
        assert!(doc.contains("## ws9-p1 — source: none"));
        assert!(doc.contains("No output on disk for this pane"));
        assert!(doc.contains("no on-disk transcript for this harness"));
        assert!(doc.contains("## ws9-p2 — source: claude_transcript (TRUNCATED: newest tail only)"));
        // Wire rows mirror the table.
        assert_eq!(panes.len(), 3);
        assert_eq!(panes[0].bytes, "p0 did the thing".len() as u64);
        assert_eq!(panes[1].source, "none");
        assert_eq!(panes[1].bytes, 0);
        assert!(panes[2].truncated);
    }

    #[test]
    fn synthesize_panes_local_consolidates_found_and_none_from_disk() {
        // End-to-end over temp dirs: one pane HAS an orchestrate report on disk, the
        // other has nothing — the merge carries the found content AND the honest none.
        let root = unique_root("e2e");
        let state = root.join("state");
        std::fs::create_dir_all(&state).unwrap();
        let hit = "wsSYNTHT21x0-p0";
        let miss = "wsSYNTHT21x0-p1";
        let run = root.join("agent-teams-orchestrate").join("run-1");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(
            run.join(format!("{hit}.md")),
            "# Report\np0 shipped the fix",
        )
        .unwrap();

        let ack = synthesize_panes_local(&state, &[hit.into(), miss.into()], "fix the bug")
            .expect("local synthesis succeeds");
        assert!(ack.ok);
        assert!(
            ack.detail.contains("1/2"),
            "detail counts found panes: {}",
            ack.detail
        );
        assert!(ack.detail.contains("CLAIMED"));

        let Some(SocketData::PaneSynthesis {
            report_path,
            run_id,
            content,
            panes,
        }) = ack.data
        else {
            panic!("expected PaneSynthesis data");
        };
        // The doc rides the reply AND is written under the server-chosen synth dir
        // (sibling of the orchestrate run-dir root, run id server-minted).
        assert!(content.contains("p0 shipped the fix"));
        assert!(content.contains(&format!("## {miss} — source: none")));
        assert!(run_id.starts_with("synth-"));
        let on_disk = std::fs::read_to_string(&report_path).expect("final.md written");
        assert_eq!(on_disk, content);
        assert!(
            report_path.contains("agent-teams-synth"),
            "server-chosen synth dir, never caller-supplied: {report_path}"
        );
        assert!(report_path.starts_with(&*root.to_string_lossy()));
        // Per-pane wire rows: found + none, both present.
        assert_eq!(panes.len(), 2);
        assert_eq!(panes[0].id, hit);
        assert_eq!(panes[0].source, "orchestrate_report");
        assert_eq!(panes[1].id, miss);
        assert_eq!(panes[1].source, "none");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Gate OFF (the default — no mcp-config.json at all): the sidecar-local fan-in
    /// writes ZERO notes, creates no store dir, and its ack detail is byte-free of any
    /// harvest mention — synthesis output unchanged.
    #[test]
    #[cfg(feature = "memory-notes")]
    fn synthesize_panes_local_gate_off_harvests_nothing() {
        let root = unique_root("harvest-off");
        let state = root.join("state");
        std::fs::create_dir_all(&state).unwrap();
        let hit = "wsHARVOFFx0-p0";
        let run = root.join("agent-teams-orchestrate").join("run-1");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(
            run.join(format!("{hit}.md")),
            "# Report\nLESSON: this marked lesson must NOT be stored while the gate is off\n",
        )
        .unwrap();

        let ack = synthesize_panes_local(&state, &[hit.into()], "goal").unwrap();
        assert!(ack.ok);
        assert!(
            !ack.detail.contains("harvested"),
            "gate off ⇒ ack detail byte-identical (no harvest mention): {}",
            ack.detail
        );
        // The store was never touched: the memory sibling of the state dir is absent.
        assert!(
            !root.join("state-memory").exists(),
            "gate off ⇒ zero writes, no store dir"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Gate ON: the marked lesson lands as a note (category/tags/provenance/origin),
    /// and the ack detail honestly reports the count.
    #[test]
    #[cfg(feature = "memory-notes")]
    fn synthesize_panes_local_gate_on_harvests_marked_lessons() {
        let root = unique_root("harvest-on");
        let state = root.join("state");
        std::fs::create_dir_all(&state).unwrap();
        // Arm the gate (mcp-config.json is a SIBLING of the state dir).
        std::fs::write(
            agent_teams_core::mcp_config_path(&state).unwrap(),
            r#"{"memory_harvest":true}"#,
        )
        .unwrap();
        let hit = "wsHARVONx0-p0";
        let run = root.join("agent-teams-orchestrate").join("run-1");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(
            run.join(format!("{hit}.md")),
            "# Report\n\
             - LESSON: gate-on harvest stores this marked line as a note\n\
             - LESSON: a second marked lesson meshes with its run sibling\n\
             prose line\n",
        )
        .unwrap();

        let ack = synthesize_panes_local(&state, &[hit.into()], "goal").unwrap();
        assert!(ack.ok);
        assert!(
            ack.detail.contains("harvested 2 lesson(s)"),
            "gate on + markers ⇒ honest ack: {}",
            ack.detail
        );
        // The notes landed in the SAME dir the memory tools resolve (env/cwd-keyed).
        let dir = crate::memory::notes_dir(&state).unwrap();
        let notes = agent_teams_memory::list_notes(&dir);
        assert_eq!(notes.len(), 2);
        for n in &notes {
            assert_eq!(
                n.category.as_deref(),
                Some(agent_teams_memory::HARVEST_CATEGORY)
            );
            assert_eq!(n.tags, vec![agent_teams_memory::HARVEST_TAG.to_string()]);
            assert_eq!(n.origin.as_deref(), Some(hit));
            assert!(
                n.body.contains(&format!("/{hit}, "))
                    && n.body.contains("harvested from run synth-"),
                "provenance line carries run + pane: {:?}",
                n.body
            );
            // LINKED, not orphaned: the same-run sibling mesh (2 notes → each links
            // the other; the empty pre-run store yields no lineage hits here).
            let sibling = notes.iter().find(|o| o.id != n.id).unwrap();
            assert_eq!(
                n.links,
                vec![sibling.id.clone()],
                "same-run sibling mesh, no self-link"
            );
        }
        assert!(notes
            .iter()
            .any(|n| n.title == "gate-on harvest stores this marked line as a note"));
        // Idempotent: re-synthesizing the same reports writes 0 new notes (title dedup).
        let ack2 = synthesize_panes_local(&state, &[hit.into()], "goal").unwrap();
        assert!(!ack2.detail.contains("harvested"), "dedup ⇒ 0 ⇒ no mention");
        assert_eq!(agent_teams_memory::list_notes(&dir).len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn orchestrate_dispatch_arg_defaults_false_preview_first() {
        // The preview-first invariant at the args boundary: an MCP client that omits
        // `dispatch` must get the SAFE preview (false), never a blind fan-out.
        let args: OrchestrateArgs = serde_json::from_str(r#"{"goal":"ship it"}"#).unwrap();
        assert!(
            !args.dispatch,
            "dispatch must default to false (preview-first)"
        );
        assert!(
            args.target_workspace.is_none(),
            "target_workspace defaults to None (unscoped)"
        );
        let args: OrchestrateArgs =
            serde_json::from_str(r#"{"goal":"ship it","dispatch":true}"#).unwrap();
        assert!(args.dispatch);
        // B4: target_workspace round-trips when supplied (scopes the fan-out).
        let args: OrchestrateArgs =
            serde_json::from_str(r#"{"goal":"ship it","target_workspace":"ws76101x0"}"#).unwrap();
        assert_eq!(args.target_workspace.as_deref(), Some("ws76101x0"));
    }
}
