//! `agent-teams-mcp` — read-only MCP sidecar over the Agent Teams state adapter
//! (PRD §14 Phase A; `.paul/analysis/context-router-mcp.md`).
//!
//! **Pitch:** expose *your* team state to *your* MCP clients (Cursor / Claude
//! Code / Codex) — not BridgeMind interop. **Phase A is read-only and stdio
//! only.** Three tools + read-only resources project the ranked "who needs me"
//! queue, the workspace list, and a single workspace's state.
//!
//! **Liveness (Phase A→B seam).** The queue tools/resources filter by the live
//! set when the app is running: [`identified_queue`] reads the sibling live registry
//! (`agent_teams_core::registry_path`) and passes it to
//! [`compute_queue_identified`] — which both filters to the live set AND joins each
//! row's spawn identity (`role`/`tag`) from the registry (gap #4). Registry absent ⇒
//! the app-down superset (FR-7), rows carrying only the id-derived `workspace`.
//! `list_workspaces` stays pure discovery (everything on disk), so it can disagree
//! with the live queue by design — it is the app-down introspection tool.
//!
//! **Phase B mutation posture (capability-by-role).** The mutation tools in
//! [`phase_b`] compile under `--features phase-b-mutations` and ARE wired: the
//! separate `mutation_tool_router` is MERGED into the main router in
//! `TeamServer::new` when the feature is on, and the tools dial the app's Unix
//! socket for real. The DEFAULT `agent-teams-mcp` build stays read-only (feature
//! off — this binary is what every ordinary pane gets); the SHIPPED
//! `agent-teams-mcp-coordinator` sidecar is built WITH the feature and is handed
//! ONLY to Coordinator-role panes (the app/daemon select it by role) — so
//! broadcast/orchestrate capability is role-scoped by ABSENCE, not merely refused.
//! Every mutating call is still double-gated at the app boundary
//! (`allow_mutations`, or `send_input_enabled` for team_send_input, plus the
//! coordinator peer-pid gate).
//!
//! State dir resolution mirrors the `adapter` binary: `$AGENT_TEAMS_STATE_DIR`,
//! else `~/Library/Application Support/harness-ready/agent-teams`.

use std::path::{Path, PathBuf};

use rmcp::handler::server::router::prompt::PromptRouter;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{
    AnnotateAble, GetPromptRequestParams, GetPromptResult, ListPromptsResult, ListResourcesResult,
    PaginatedRequestParams, PromptMessage, PromptMessageRole, RawResource,
    ReadResourceRequestParams, ReadResourceResult, ResourceContents, ServerCapabilities,
    ServerInfo,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::transport::stdio;
use rmcp::{
    prompt, prompt_handler, prompt_router, tool, tool_handler, tool_router, ErrorData,
    ServerHandler, ServiceExt,
};
use serde::{Deserialize, Serialize};

use agent_teams_core::{compute_queue_identified, list_workspaces, read_registry, QueueRow};

/// PHASE B — the LIVE mutation tool surface (Unix-socket dial + wire protocol).
/// Compiled only under `--features phase-b-mutations` and WIRED: the separate
/// `mutation_tool_router` is merged into the live router in `new()`. The shipped
/// `agent-teams-mcp-coordinator` binary carries this feature (capability-by-role:
/// only Coordinator panes get it); the default read-only build does not.
#[cfg(feature = "phase-b-mutations")]
mod phase_b;

/// `team_read_output` — read a pane's produced report/transcript off disk. ALWAYS-ON
/// (registered in the base read router), ungated local file I/O. The caller passes a
/// pane id; the server resolves the source (orchestrate `.md` → claude transcript →
/// honest "none"). See the module header for the safe-read contract.
mod read_output;

/// `team_audit_log` — read back the external-mutation + external-read audit ledgers
/// (siblings of the state root), newest first. ALWAYS-ON (base read router), ungated
/// read-only local file I/O — the same trust class as `team_read_output`. Closes the
/// "brain narrates its own dispatches from memory" gap (#9): the on-disk ledgers are
/// the ground truth of what was actually dispatched/read.
mod audit_log;

/// PHASE 10 — Mem-1 BridgeMemory note tools (ungated, inert-first). Compiled only
/// under `--features memory-notes`; a separate `memory_tool_router` merged in `new()`.
#[cfg(feature = "memory-notes")]
mod memory;
/// Phase 14 / item 4b — the `task_*` lifecycle tools, compiled + registered only
/// under `--features task-tools`; a separate `task_tool_router` merged in `new()`.
#[cfg(feature = "task-tools")]
mod task;

/// Read-only MCP server. Holds the resolved state dir + the macro-generated
/// tool router. `Clone` is cheap (a `PathBuf` + an `Arc`-backed router).
#[derive(Clone)]
struct TeamServer {
    state_dir: PathBuf,
    // Read by the `#[tool_handler]`-generated dispatch; the dead-code lint can't
    // see through the macro + derived `Clone`, hence the allow.
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
    // Read by the `#[prompt_handler]`-generated dispatch (same macro/Clone lint blind
    // spot as `tool_router`).
    #[allow(dead_code)]
    prompt_router: PromptRouter<Self>,
}

/// URI of the read-only queue resource.
const QUEUE_URI: &str = "team://queue";
/// URI of the read-only workspace-list resource.
const WORKSPACES_URI: &str = "team://workspaces";

/// Arguments for [`TeamServer::get_workspace`].
#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct GetWorkspaceArgs {
    /// The workspace id (the directory name under the state dir).
    id: String,
}

// Tool outputs must be object-rooted (MCP requires `outputSchema` root type
// `object`), so each tool returns a named wrapper rather than a bare array /
// `serde_json::Value` (whose schema has no root `type`). The wrappers reuse the
// single shared [`QueueRow`] — no duplicated row shape.

/// Output of `team_get_queue`.
#[derive(Serialize, schemars::JsonSchema)]
struct QueueResult {
    /// Ranked queue rows, "who needs me" first.
    queue: Vec<QueueRow>,
}

/// Output of `list_workspaces`.
#[derive(Serialize, schemars::JsonSchema)]
struct WorkspacesResult {
    /// Discovered workspace ids, sorted.
    workspaces: Vec<String>,
}

/// Output of `get_workspace`.
#[derive(Serialize, schemars::JsonSchema)]
struct WorkspaceResult {
    /// The row, or `null` if the workspace has no current state / is unknown.
    workspace: Option<QueueRow>,
}

/// schemars (1.x) tags `u64`/`u32` fields with `"format":"uint64"`/`"uint32"`,
/// which are NOT standard JSON-Schema formats. Strict MCP clients (e.g. opencode)
/// log `unknown format … ignored in schema` for every such field on connect. This
/// strips those numeric format hints from a schema object in place. Purely
/// cosmetic — the field stays an integer; only the unrecognized `format` tag goes.
fn strip_numeric_formats(map: &mut serde_json::Map<String, serde_json::Value>) {
    const NUMERIC_FORMATS: &[&str] = &[
        "uint", "uint8", "uint16", "uint32", "uint64", "uint128", "int", "int8", "int16", "int32",
        "int64", "int128", "float", "double",
    ];
    if let Some(serde_json::Value::String(fmt)) = map.get("format") {
        if NUMERIC_FORMATS.contains(&fmt.as_str()) {
            map.remove("format");
        }
    }
    for value in map.values_mut() {
        strip_numeric_formats_in_value(value);
    }
}

fn strip_numeric_formats_in_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => strip_numeric_formats(map),
        serde_json::Value::Array(items) => {
            for item in items.iter_mut() {
                strip_numeric_formats_in_value(item);
            }
        }
        _ => {}
    }
}

/// Apply [`strip_numeric_formats`] to every registered tool's input + output schema
/// (walks `$defs` too). Called once after the routers are merged, so every served
/// tool — read, memory, task, mutation — is sanitized regardless of feature flags.
fn sanitize_tool_schemas(router: &mut ToolRouter<TeamServer>) {
    for route in router.map.values_mut() {
        strip_numeric_formats(std::sync::Arc::make_mut(&mut route.attr.input_schema));
        if let Some(output) = route.attr.output_schema.as_mut() {
            strip_numeric_formats(std::sync::Arc::make_mut(output));
        }
    }
}

#[tool_router]
impl TeamServer {
    fn new(state_dir: PathBuf) -> Self {
        // Phase-A read tools always. Under `--features phase-b-mutations` MERGE the
        // mutation router (its own `#[tool_router(router = …)]` impl) — keeping the
        // two routers separate is what lets the macro cfg-gate the mutation tools
        // (the macro ignores per-method `#[cfg]`, so a gated tool can't live in the
        // same router as the always-on reads). Default builds register reads only.
        #[allow(unused_mut)]
        let mut router = Self::tool_router();
        #[cfg(feature = "phase-b-mutations")]
        router.merge(Self::mutation_tool_router());
        #[cfg(feature = "memory-notes")]
        router.merge(Self::memory_tool_router());
        #[cfg(feature = "task-tools")]
        router.merge(Self::task_tool_router());
        // Strip non-standard numeric `format` hints (schemars u64/u32 → "uint64"/
        // "uint32") so strict clients (opencode) don't log `unknown format` per field.
        sanitize_tool_schemas(&mut router);
        Self {
            state_dir,
            tool_router: router,
            prompt_router: Self::prompt_router(),
        }
    }

    #[tool(
        name = "team_get_queue",
        description = "The ranked 'who needs me' queue across all workspaces: \
            agents needing a human (approval/question) first, then turn_end > \
            rate_limit, tie-broken by longest wait. Each row: \
            {id, harness, state, reason, needs_human, since} plus OPTIONAL identity \
            fields (omitted when unknown): 'role' (the persona assigned at spawn — \
            e.g. \"coordinator\"/\"builder\"/\"scout\", so you can resolve WHICH pane \
            is the coordinator), 'tag' (the create-time tag from create_workspace, so \
            you can find the panes YOU spawned), and 'workspace' (the wsNNNNNxK prefix \
            of the id — no need to re-derive it). Read-only; reflects on-disk events \
            and works whether or not the app is running (role/tag come from the app's \
            live registry, so they are present only while the app records it)."
    )]
    async fn team_get_queue(&self) -> Result<Json<QueueResult>, ErrorData> {
        Ok(Json(QueueResult {
            queue: identified_queue(&self.state_dir),
        }))
    }

    #[tool(
        name = "list_workspaces",
        description = "List discovered workspace ids (sorted). A workspace is any \
            directory under the state dir that has an events.jsonl. Read-only."
    )]
    async fn list_workspaces(&self) -> Result<Json<WorkspacesResult>, ErrorData> {
        Ok(Json(WorkspacesResult {
            workspaces: list_workspaces(&self.state_dir),
        }))
    }

    #[tool(
        name = "get_workspace",
        description = "Current ranked-queue row for a single workspace id, or null \
            if it has no current state / is unknown. Same row shape as \
            team_get_queue, including the OPTIONAL identity fields 'role' (spawn \
            persona, e.g. \"coordinator\"), 'tag' (create-time tag), and 'workspace' \
            (the wsNNNNNxK prefix of the id) — omitted when unknown. Read-only."
    )]
    async fn get_workspace(
        &self,
        Parameters(GetWorkspaceArgs { id }): Parameters<GetWorkspaceArgs>,
    ) -> Result<Json<WorkspaceResult>, ErrorData> {
        let workspace = identified_queue(&self.state_dir)
            .into_iter()
            .find(|row| row.id == id);
        Ok(Json(WorkspaceResult { workspace }))
    }

    #[tool(
        name = "team_read_output",
        description = "Read the OUTPUT a pane produced — its report or transcript, the \
            answer to 'go read what p4 wrote'. Pass the FULL pane id (e.g. \
            \"ws50144x0-p4\" as it appears in team_get_queue) — NOT a short 'p4', and \
            NOT a filesystem path; the server resolves the source from the id and reads \
            only Agent-Teams-controlled artifacts. Source precedence: (1) the \
            orchestrate/bridge <id>.md report (harness-agnostic — what a dispatched pane \
            wrote); (2) for claude panes, the pane's own transcript; (3) for a LIVE pane \
            with nothing on disk (commandcode/codex/opencode/cline keep no transcript), \
            a live tail of its in-memory scrollback read from the running app \
            (source='live_scrollback' — UNVERIFIED, may be mid-stream; needs the app \
            running and the external-read gate armed); (4) otherwise an honest 'none' \
            with a note. Returns {id, harness, source, path, content, truncated, note}; \
            content is the newest tail capped at max_bytes (default 65536). Read-only \
            (never mutates a pane); every read is audited (id+size, not content)."
    )]
    async fn team_read_output(
        &self,
        Parameters(args): Parameters<read_output::ReadOutputArgs>,
    ) -> Result<Json<read_output::PaneOutputResult>, ErrorData> {
        // spawn_blocking: resolve() is sync file I/O, and on phase-b builds its gap-7
        // live-scrollback fallback DIALS the app socket (blocking std I/O, up to the
        // fast-op window) — never stall the async runtime on it.
        let state_dir = self.state_dir.clone();
        let out = tokio::task::spawn_blocking(move || {
            read_output::resolve(&state_dir, &args.id, args.max_bytes)
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("read task join: {e}"), None))?;
        Ok(Json(out))
    }

    #[tool(
        name = "team_audit_log",
        description = "The audit trail of EXTERNAL orchestrator actions — the honest \
            way to answer 'what did I actually dispatch?'. Merges the two append-only \
            ledgers this system already writes on disk: the app-side external-mutation \
            log (send_input / broadcast / orchestrate preview+dispatch / focus / \
            create_workspace / add_pane — rows {ts, source, peer_pid, op, target, text, \
            details}, where text is a <=200-char snippet) and the sidecar's \
            team_read_output log (rows {ts, op, id, source, bytes} — sizes only, never \
            content). Returns {entries, skipped, note} with entries NEWEST first, each \
            tagged with its ledger; limit defaults to 20 (hard cap 200); kind narrows \
            to 'mutations' or 'reads' (omit for both). Read-only, ungated; malformed \
            lines are skipped and counted, a missing ledger is a note — never an error. \
            Prefer this over recalling your own actions from conversation memory."
    )]
    async fn team_audit_log(
        &self,
        Parameters(args): Parameters<audit_log::AuditLogArgs>,
    ) -> Result<Json<audit_log::AuditLogResult>, ErrorData> {
        Ok(Json(audit_log::resolve(
            &self.state_dir,
            args.limit,
            args.kind.as_deref(),
        )))
    }
}

// ───────────── Developer-guide prompt (Phase 13 / D52) ──────────────────────────
//
// A static, no-argument MCP prompt that teaches a client how to drive the `team_*`
// surface. Closes the `prompts/list` gap (the sidecar advertised tools + resources
// but had no prompts capability). Read-only metadata: no new tool, no write path, no
// second ranker. VERIFY via a live stdio `initialize`→`prompts/list`→`prompts/get`
// probe — a macro-registration no-op cannot be caught by unit tests
// ([[mcp-tool-handler-router-bug]], cb999a3).
#[prompt_router]
impl TeamServer {
    #[prompt(
        name = "agent_teams_developer_guide",
        description = "How to drive the Agent Teams MCP surface: the read tools, the \
            ranked-queue semantics, the app-up vs app-down read path, the resources, \
            and the allow_mutations / Model-A gate for the Phase-B mutation tools."
    )]
    async fn agent_teams_developer_guide(&self) -> GetPromptResult {
        GetPromptResult::new(vec![PromptMessage::new_text(
            PromptMessageRole::User,
            DEVELOPER_GUIDE,
        )])
    }
}

/// The static text returned by the `agent_teams_developer_guide` prompt. Terse +
/// structural (names + semantics) so it lives next to the surface it documents — a
/// tool change is in the same file as its doc.
const DEVELOPER_GUIDE: &str = "\
Agent Teams MCP — developer guide.\n\n\
THE WEDGE: one ranked 'who needs me' queue across your Claude + Cursor agent panes — the same \
order the app's global hotkey (Cmd+Shift+J) jumps to. Terminals cannot model agent state; this \
surface exposes it.\n\n\
READ TOOLS (all read-only; work whether or not the app is running):\n\
- team_get_queue -> the ranked queue. Rows: {id, harness, state, reason, needs_human, since} \
plus OPTIONAL identity fields (omitted when unknown): role (spawn persona, e.g. 'coordinator' — \
how you resolve which pane is the coordinator), tag (the create-time tag you passed to \
create_workspace — how you find YOUR panes), workspace (the wsNNNNNxK prefix of the id). \
Order: needs_human (approval/question) first, then turn_end > rate_limit, tie-broken by longest \
wait (smallest 'since').\n\
- list_workspaces -> all discovered workspace ids (any dir with an events.jsonl), sorted. Pure \
on-disk discovery; it can disagree with the live queue by design (the app-down introspection tool).\n\
- get_workspace {id} -> that workspace's current queue row (same shape, incl. role/tag/workspace), \
or null.\n\
- team_read_output {id, max_bytes?} -> the OUTPUT a pane produced (its report/transcript), \
NOT just status. Pass the FULL pane id; the server resolves the source (orchestrate <id>.md \
report first, then a claude pane's transcript, then — for a LIVE pane with nothing on disk — \
a live in-memory scrollback tail from the running app, source='live_scrollback', unverified/\
may be mid-stream) and reads only AT-controlled artifacts — you NEVER pass a path. {source, \
path, content, truncated, note}; source='none' (with a note) when nothing is readable.\n\
- team_audit_log {limit?, kind?} -> the audit trail of EXTERNAL orchestrator actions, NEWEST \
first — what was ACTUALLY dispatched (app-side mutation ledger: send/broadcast/orchestrate/\
focus/spawn) and read (sidecar read_output ledger), merged from the on-disk ledgers. Each \
entry is tagged with its ledger ('mutations'|'reads'); kind narrows to one; limit defaults \
20, caps at 200. Rows are audit metadata (ts/op/target/bytes; mutation text is a <=200-char \
snippet), never full content. USE THIS instead of recalling your own dispatches from memory.\n\n\
RESOURCES (mirror the tools): team://queue, team://workspaces, team://workspace/{id}.\n\n\
APP-UP vs APP-DOWN: when the Agent Teams app is running, team_get_queue / get_workspace reflect the \
LIVE set; when it is down they fall back to the discovered on-disk superset (which may include stale \
workspaces). list_workspaces is ALWAYS pure discovery.\n\n\
FAN-IN: team_synthesize consolidates many panes' outputs into ONE markdown doc. Two modes — \
{dir}: an orchestrate run dir's <id>.md reports -> final.md + per-pane verdicts \
(ok/empty/incomplete/missing; LLM pass, requires app running + allow_mutations); \
{pane_ids:[...]}: verbatim merge (no LLM) of ANY <=16 panes' outputs resolved exactly like \
team_read_output (app-closed works, disk-only then; no allow_mutations needed). Either way the \
result is CLAIMED (assembled from pane reports), not verified — relay it as such.\n\n\
MUTATIONS ARE GATED + MODEL A: the mutating / Context-Router tools (team_focus_workspace, \
team_orchestrate, team_broadcast, team_handoff, team_synthesize{dir}) require BOTH the app running \
AND allow_mutations=true in mcp-config.json (a sibling of the state dir); they are absent by \
default. team_synthesize{pane_ids} is the one read-shaped exception (see FAN-IN). \
team_send_input has its OWN NARROW gate: send_input_enabled=true (decoupled from allow_mutations; \
armed via the app's Settings toggle). \
COORDINATOR-ONLY: on TOP of those config gates, EVERY mutating op called from a PANE also requires \
that the CALLING pane resolve to a Coordinator role — the app derives the caller from the connected \
peer's pid via LOCAL_PEERPID ancestry (not agent-forgeable) and refuses any non-Coordinator pane \
with FORBIDDEN. So a pane mutation needs the config gate AND a Coordinator caller; a plain worker \
pane cannot broadcast, delegate, focus, or send-input even with allow_mutations on. (An operator- \
armed EXTERNAL orchestrator — #262: allow_external_* + pid-pinned binary — is the one additive \
non-pane path.) \
Model A: this server NEVER auto-approves an agent's pending prompt — it only delivers the operator's \
text, verbatim, one line.\n\n\
NAMING: native names only. There are NO BridgeMCP-style compat aliases (e.g. no list_agents) — a \
workspace is an ephemeral PTY session, not a persisted agent/persona, so such an alias would be a \
semantic lie.";

// ───────────── Phase-B mutation tools (06-02) — feature-gated, gate-checked ──────
//
// A SEPARATE `#[tool_router(router = mutation_tool_router)]` impl (its own router
// name) so the read tools above stay always-on while these compile ONLY under
// `--features phase-b-mutations`. `TeamServer::new` MERGEs this router into the main
// one when the feature is on. (The macro ignores per-method `#[cfg]`, so a gated
// tool cannot share a router with the always-on reads — hence two routers.)
//
// Each tool: (1) reads `mcp-config.json` (sibling of state_root, SSOT
// `read_mcp_config`); `allow_mutations=false` (the SAFE default, incl.
// absent/malformed) ⇒ refuse WITHOUT dialing (UX gate; the app RE-checks — the app
// is the load-bearing enforcement against a rogue dialer). (2) resolves the socket
// path (SSOT `socket_path`); (3) dials on a `spawn_blocking` thread (std
// `UnixStream`, no async-runtime stall); (4) maps the app's structured reply.
// Model A: send_input passes the operator's text VERBATIM — no queue read, no branch
// on "y"/"yes"; the app boundary normalizes + gates.
#[cfg(feature = "phase-b-mutations")]
#[tool_router(router = mutation_tool_router)]
impl TeamServer {
    #[tool(
        name = "team_send_input",
        description = "Route the human's reply to a LIVE workspace's PTY (one line; \
            the app appends the single trailing newline). Requires the app running \
            AND send_input_enabled=true in mcp-config.json (its OWN narrow gate, \
            DECOUPLED from allow_mutations; armed via the app's Settings toggle). \
            The calling pane must ALSO be a Coordinator (the app resolves the caller \
            role via peer-pid ancestry) — a non-Coordinator pane is refused FORBIDDEN. \
            Returns a structured error on a dead pane (DEAD_PANE), unknown id \
            (UNKNOWN_WORKSPACE), gate-off (SEND_INPUT_DISABLED), or app-down \
            (APP_NOT_RUNNING). NEVER auto-approves an agent's pending prompt — it \
            only delivers the operator's text."
    )]
    async fn team_send_input(
        &self,
        Parameters(args): Parameters<phase_b::mutations::SendInputArgs>,
    ) -> Result<Json<phase_b::mutations::MutationAck>, ErrorData> {
        self.run_mutation(phase_b::socket::SocketRequest::SendInput {
            id: args.id,
            text: args.text,
        })
        .await
    }

    #[tool(
        name = "team_focus_workspace",
        description = "Raise the Agent Teams app and focus a workspace by id. \
            Requires the app running AND allow_mutations=true in mcp-config.json, \
            and the calling pane must be a Coordinator (peer-pid ancestry gate) — a \
            non-Coordinator pane is refused FORBIDDEN. \
            Returns UNKNOWN_WORKSPACE for an unknown id, APP_NOT_RUNNING if the app \
            is down."
    )]
    async fn team_focus_workspace(
        &self,
        Parameters(args): Parameters<phase_b::mutations::FocusWorkspaceArgs>,
    ) -> Result<Json<phase_b::mutations::MutationAck>, ErrorData> {
        self.run_mutation(phase_b::socket::SocketRequest::Focus { id: args.id })
            .await
    }

    // ──────────── 06-03 Context Router tools (gated by allow_mutations) ──────────
    // Drive the in-app team (orchestrate / broadcast / handoff) over the SAME
    // euid-gated, capability-gated socket. PREVIEW-FIRST: team_orchestrate's
    // `dispatch` defaults FALSE — an external host gets the {id,task} mapping to
    // inspect and must make a SEPARATE dispatch:true call to fan out (D23, never a
    // blind fan-out). All three are absent from the router unless `phase-b-mutations`
    // is built AND allow_mutations=true at call time.

    #[tool(
        name = "team_orchestrate",
        description = "Context Router: synthesize the goal into one task per LIVE pane \
            IN ONE WORKSPACE via the in-app orchestrator, then PREVIEW or DISPATCH. \
            target_workspace SCOPES the fan-out to that workspace (a workspace id like \
            'ws76101x0', or the create-time tag) — pass it whenever more than one \
            workspace is live; OMITTING it while >1 workspace is live is refused with \
            BAD_REQUEST (the tool will NOT blast every workspace), and with exactly one \
            live workspace it may be omitted. dispatch=false (DEFAULT) returns the \
            {id,task} mapping and dispatches NOTHING (inspect first); dispatch=true sends \
            each task to its pane AND stamps a fan-in run dir — it writes a manifest and \
            appends a report-path instruction to each task (each pane writes \
            <run_dir>/<id>.md ending '## BOUNDARIES'), returning {run_dir, sent, skipped}. \
            Pass that run_dir to team_synthesize to fan the reports into one final.md — \
            the full orchestrate->synthesize loop, no GUI. Requires the app running AND \
            allow_mutations=true, and a PANE caller must be a Coordinator (peer-pid \
            ancestry gate) — a non-Coordinator pane is refused FORBIDDEN. Uses the same \
            single in-app synthesizer as the Bridge (no second copy); a synthesis failure \
            dispatches nothing. NEVER auto-approves a pane's pending prompt."
    )]
    async fn team_orchestrate(
        &self,
        Parameters(args): Parameters<phase_b::mutations::OrchestrateArgs>,
    ) -> Result<Json<phase_b::mutations::RouterAck>, ErrorData> {
        self.run_router_op(phase_b::socket::SocketRequest::Orchestrate {
            goal: args.goal,
            dispatch: args.dispatch,
            target_workspace: args.target_workspace,
        })
        .await
    }

    #[tool(
        name = "team_broadcast",
        description = "Context Router: send one line of text to EVERY live pane \
            through the same gate as team_send_input (is_alive, one line + newline). \
            Returns {sent:[ids], skipped:[dead ids]} — dead panes are reported, never \
            silently dropped. Requires the app running AND allow_mutations=true, and \
            the calling pane must be a Coordinator (peer-pid ancestry gate) — a \
            non-Coordinator pane is refused FORBIDDEN. \
            Model A: delivers the operator's text verbatim — NEVER auto-answers a \
            pending approval."
    )]
    async fn team_broadcast(
        &self,
        Parameters(args): Parameters<phase_b::mutations::BroadcastArgs>,
    ) -> Result<Json<phase_b::mutations::RouterAck>, ErrorData> {
        self.run_router_op(phase_b::socket::SocketRequest::Broadcast { text: args.text })
            .await
    }

    #[tool(
        name = "team_handoff",
        description = "Context Router: relay ONE single-line handoff message \
            (provenance 'from' + your instruction) to pane 'to' through the gate — \
            one hop, no multi-turn loop. Requires the app running AND \
            allow_mutations=true, and the calling pane must be a Coordinator (peer-pid \
            ancestry gate) — a non-Coordinator pane is refused FORBIDDEN. Returns \
            DEAD_PANE / UNKNOWN_WORKSPACE / \
            APP_NOT_RUNNING on failure."
    )]
    async fn team_handoff(
        &self,
        Parameters(args): Parameters<phase_b::mutations::HandoffArgs>,
    ) -> Result<Json<phase_b::mutations::RouterAck>, ErrorData> {
        self.run_router_op(phase_b::socket::SocketRequest::Handoff {
            from: args.from,
            to: args.to,
            instruction: args.instruction,
        })
        .await
    }

    #[tool(
        name = "team_synthesize",
        description = "Context Router fan-in — TWO MODES; pass exactly ONE of `dir` / \
            `pane_ids` (both or neither is BAD_REQUEST). Either way the output is \
            CLAIMED: assembled from pane reports/transcripts, not verified — no git \
            ground-truth or authoritative test is run. \
            DIR MODE (app-side): `dir` = the absolute Bridge run directory holding \
            manifest.json + each dispatched pane's <id>.md report (the dir a dispatched \
            team_orchestrate returned). The SAME in-app synthesizer as the Bridge (no \
            second copy, an LLM pass) consolidates them into <dir>/final.md and returns \
            its path + per-pane verdicts (ok/empty/incomplete/missing). Requires the app \
            running AND allow_mutations=true, and a PANE caller must be a Coordinator \
            (peer-pid ancestry gate) — a non-Coordinator pane is refused FORBIDDEN; \
            BAD_REQUEST if the dir has no manifest or \
            no reports yet, APP_NOT_RUNNING if the app is down. \
            PANE_IDS MODE (sidecar-driven): `pane_ids` = FULL pane ids (e.g. \
            \"ws50144x0-p4\", max 16) from ANY team — hand-spawned included, no \
            orchestrate dance. Each pane's output is resolved exactly like \
            team_read_output (report -> transcript -> live scrollback tail from the \
            running app for a live pane with nothing on disk -> honest source:\"none\", \
            never silently dropped) and MERGED VERBATIM (no LLM pass) into one markdown \
            doc (summary table + per-pane sections), returned in the result \
            (data.content) AND written to a server-chosen synth dir beside the \
            orchestrate run dirs — never a caller-supplied path. ASYMMETRY: this mode \
            needs no allow_mutations and still works with the app closed (disk sources \
            only then; the live-scrollback step simply reports none) — same read \
            surface as team_read_output, including its per-pane read audit. `goal` is \
            optional framing for both modes."
    )]
    async fn team_synthesize(
        &self,
        Parameters(args): Parameters<phase_b::mutations::SynthesizeArgs>,
    ) -> Result<Json<phase_b::mutations::RouterAck>, ErrorData> {
        use phase_b::mutations::{synthesize_mode, SynthesizeMode};
        // Exactly-one-of `dir`/`pane_ids` + pane-id validation + cap, at the boundary.
        let mode = synthesize_mode(&args).map_err(|m| ErrorData::invalid_params(m, None))?;
        let goal = args.goal.unwrap_or_default();
        match mode {
            // DIR MODE — the unchanged 06-05 app-side path (gate → dial → map).
            SynthesizeMode::Dir(dir) => {
                self.run_router_op(phase_b::socket::SocketRequest::Synthesize { dir, goal })
                    .await
            }
            // PANE_IDS MODE (Phase 21) — sidecar-local assembly via the team_read_output
            // resolver: no socket, no allow_mutations gate (the same data is already
            // readable ungated via team_read_output; the only write lands in the
            // server-chosen synth dir). Off-thread: up to 16 artifact reads + one write.
            SynthesizeMode::Panes(ids) => {
                let state_dir = self.state_dir.clone();
                let ack = tokio::task::spawn_blocking(move || {
                    phase_b::mutations::synthesize_panes_local(&state_dir, &ids, &goal)
                })
                .await
                .map_err(|e| ErrorData::internal_error(format!("synth task join: {e}"), None))?
                .map_err(|m| ErrorData::invalid_request(m, None))?;
                Ok(Json(ack))
            }
        }
    }

    // ──────────── Autonomous workers (MVP, depth-1, gated) ──────────────────────
    // The one new verb. Spawns invisible PTY workers to autonomously pursue a goal;
    // results fan in to the caller's pane as ONE line. Double-gated app-side
    // (allow_mutations + autonomy_ceiling>=1) on top of the allow_mutations UX gate
    // here; depth>1 is rejected (no recursion in the MVP). The socket reply is a FAST
    // ack ({run_id,workers}) — the spawn→orchestrate→fan-in→write-back runs detached
    // app-side, so the serial accept loop is never wedged.
    #[tool(
        name = "team_delegate",
        description = "Autonomous workers (MVP): spawn invisible PTY workers to \
            autonomously pursue `goal`; each gets a tailored subtask, runs in its own \
            git worktree, and the results fan in to YOUR pane terminal as one summary \
            line. Call it as `team_delegate{goal}` — your own pane id is filled in \
            automatically from the session env, you do NOT pass it. `max_workers` \
            (default 3) is clamped to the fairness budget (cap-1, a human slot is \
            always reserved). `depth` (default 1) >1 is rejected — no recursion in the \
            MVP. Requires the app running AND allow_mutations=true AND \
            autonomy_ceiling>=1 in mcp-config.json, and the calling pane must be a \
            Coordinator (peer-pid ancestry gate) — a non-Coordinator pane is refused \
            FORBIDDEN. Returns {run_id,workers} on accept; \
            AUTONOMY_DISABLED / DEPTH_EXCEEDED / UNKNOWN_WORKSPACE / MUTATIONS_DISABLED \
            / APP_NOT_RUNNING on refusal. The work result arrives in your pane, not \
            this reply."
    )]
    async fn team_delegate(
        &self,
        Parameters(args): Parameters<phase_b::mutations::DelegateArgs>,
    ) -> Result<Json<phase_b::mutations::RouterAck>, ErrorData> {
        // Auto-fill the caller's pane id from `$AGENT_TEAMS_PANE_ID` (supervisor-injected at
        // spawn — the established memory.rs/task.rs pattern). UNLIKE those tolerant readers,
        // delegation has nowhere to fan results in without a parent pane, so an absent/empty
        // var is a HARD, structured rejection rather than a silent fallback.
        let parent_id = std::env::var("AGENT_TEAMS_PANE_ID")
            .ok()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                ErrorData::invalid_params(
                    "team_delegate must run inside an Agent Teams pane \
                     ($AGENT_TEAMS_PANE_ID is unset) — it has no parent to fan results into"
                        .to_string(),
                    None,
                )
            })?;
        self.run_router_op(phase_b::socket::SocketRequest::Delegate {
            parent_id,
            goal: args.goal,
            max_workers: args
                .max_workers
                .unwrap_or(agent_teams_core::DELEGATE_MAX_WORKERS),
            depth: args.depth.unwrap_or(1),
        })
        .await
    }

    #[tool(
        name = "team_create_workspace",
        description = "Open ONE NEW VISIBLE workspace in the running Agent Teams grid and spawn its \
            panes in a single call. For a MIXED team, pass `panes` — a per-pane spec list like \
            [{\"harness\":\"claude\",\"role\":\"builder\",\"count\":2},{\"harness\":\"codex\",\"role\":\"reviewer\"}] \
            — and ALL panes land in the SAME workspace. For a simple homogeneous team, omit `panes` and \
            pass scalar `harness`/`count`/`role`. `role` is a persona (e.g. coordinator/reviewer/builder/ \
            tester) stamped into the pane's prompt. This is CREATE, not control (see team_send_input), and \
            NOT autonomous (that is team_delegate, which you never have). NEVER call this more than once for \
            one logical team — each call opens a SEPARATE workspace. Requires the app running AND \
            allow_external_spawn=true. The app ENFORCES: harness allowlist (agent harnesses only — NO \
            bash/shell), each role must be a known agent role, repo in the trusted-repos allowlist, TOTAL \
            panes capped (default 8; operator-configurable via external_spawn_max_panes in mcp-config.json, max 16), and (unless opted out) a human confirm. ASYNC: returns SPAWN_REQUESTED; the new \
            workspace id is minted by the UI and is NOT in this reply."
    )]
    async fn team_create_workspace(
        &self,
        Parameters(args): Parameters<phase_b::mutations::CreateWorkspaceArgs>,
    ) -> Result<Json<phase_b::mutations::RouterAck>, ErrorData> {
        let panes = args
            .panes
            .into_iter()
            .map(|p| phase_b::socket::PaneSpec {
                harness: p.harness,
                role: p.role,
                model: p.model,
                count: p.count.unwrap_or(1),
            })
            .collect();
        self.run_spawn_op(phase_b::socket::SocketRequest::CreateWorkspace {
            repo: args.repo,
            harness: args.harness.unwrap_or_else(|| "claude".into()),
            count: args.count.unwrap_or(1),
            role: args.role,
            model: args.model,
            panes,
            tag: args.tag,
        })
        .await
    }

    #[tool(
        name = "team_add_pane",
        description = "Add ONE pane to an EXISTING workspace in the running grid. Name the destination \
            with `target_workspace` (a workspace id OR the `tag` you used when creating it); if omitted it \
            adds to whatever workspace is currently ACTIVE (less reliable — prefer naming it). Use this to \
            give a workspace panes with DIFFERENT roles after creating it. CREATE, not control; not \
            autonomous. Requires the app running AND allow_external_spawn=true; the app enforces the harness \
            + role allowlists (no bash/shell) and a human confirm. ASYNC: returns SPAWN_REQUESTED."
    )]
    async fn team_add_pane(
        &self,
        Parameters(args): Parameters<phase_b::mutations::AddPaneArgs>,
    ) -> Result<Json<phase_b::mutations::RouterAck>, ErrorData> {
        self.run_spawn_op(phase_b::socket::SocketRequest::AddPane {
            harness: args.harness,
            role: args.role,
            model: args.model,
            target_workspace: args.target_workspace,
        })
        .await
    }
}

/// Phase-B mutation plumbing on `TeamServer` (plain `impl`, no `#[tool]` methods).
/// Gate → resolve path → dial off-thread → map reply. Compiled only under
/// `--features phase-b-mutations`.
#[cfg(feature = "phase-b-mutations")]
impl TeamServer {
    async fn run_mutation(
        &self,
        req: phase_b::socket::SocketRequest,
    ) -> Result<Json<phase_b::mutations::MutationAck>, ErrorData> {
        use agent_teams_core::{read_mcp_config, socket_path};
        use phase_b::mutations::{map_reply, MutationError};

        // (1) Capability gate (UX pre-check; the app re-enforces authoritatively). SAFE default:
        //     absent/malformed config ⇒ off. `SendInput` (agent→agent prompting) uses its OWN narrow
        //     axis `send_input_enabled`, DECOUPLED from the broad `allow_mutations` — kept in lock-step
        //     with the app/daemon handlers so a sidecar pre-gate can't refuse an armed send-input.
        let cfg = read_mcp_config(&self.state_dir);
        let is_send_input = matches!(req, phase_b::socket::SocketRequest::SendInput { .. });
        if is_send_input && !cfg.send_input_enabled {
            return Err(ErrorData::invalid_request(
                "SEND_INPUT_DISABLED: agent→agent send-input is off (arm it in Settings → \
                 set send_input_enabled=true). The app also enforces this gate."
                    .to_string(),
                None,
            ));
        }
        if !is_send_input && !cfg.allow_mutations {
            return Err(ErrorData::invalid_request(
                "MUTATIONS_DISABLED: mutations are off (set allow_mutations=true in mcp-config.json, \
                 sibling of the state dir). The app also enforces this gate."
                    .to_string(),
                None,
            ));
        }
        // (2) Resolve the socket path (sibling of state_root, SSOT).
        let Some(sock) = socket_path(&self.state_dir) else {
            return Err(ErrorData::internal_error(
                "no parent for state dir — cannot resolve the mutation socket path".to_string(),
                None,
            ));
        };
        // (3) Dial on a blocking thread (don't stall the async runtime). Transport
        //     selector: UDS PREFERRED (stronger euid gate, no Bearer on wire — the existing
        //     `dial` path BYTE-FOR-BYTE unchanged), HTTP additive fallback (verify-before-
        //     send) when the socket is absent AND http_enabled. The per-op timeout (06-03
        //     GAP 2) is selected inside `dial`/`dial_http_op` (fast for these ops).
        let state_dir = self.state_dir.clone();
        let reply = tokio::task::spawn_blocking(move || {
            phase_b::dial_selected(&sock, &state_dir, &req, false)
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("dial task join: {e}"), None))?;
        // (4) Map the app's structured reply.
        match map_reply(reply) {
            Ok(ack) => Ok(Json(ack)),
            Err(e) => {
                let msg = phase_b::mutations::mutation_error_message(&e);
                // App-down is a transport condition; the rest are app-side rejections.
                match e {
                    MutationError::AppNotRunning => Err(ErrorData::internal_error(msg, None)),
                    _ => Err(ErrorData::invalid_request(msg, None)),
                }
            }
        }
    }

    /// Context Router (06-03) plumbing: the SAME gate → resolve → dial → map flow as
    /// [`run_mutation`], but returns a [`phase_b::mutations::RouterAck`] (carries the
    /// structured `data` payload — preview mapping / fan-out result) and dials via the
    /// per-op timeout (`dial_op`), so `Orchestrate` gets the long (>120s) read window
    /// while the rest stay fast (06-03 GAP 2). The capability gate is re-checked HERE
    /// (UX) and AGAIN app-side (the load-bearing enforcement against a rogue dialer).
    async fn run_router_op(
        &self,
        req: phase_b::socket::SocketRequest,
    ) -> Result<Json<phase_b::mutations::RouterAck>, ErrorData> {
        use agent_teams_core::{read_mcp_config, socket_path};
        use phase_b::mutations::{map_router_reply, MutationError};

        // (1) Capability gate. SAFE default: absent/malformed config ⇒ mutations off.
        if !read_mcp_config(&self.state_dir).allow_mutations {
            return Err(ErrorData::invalid_request(
                "MUTATIONS_DISABLED: mutations are off (set allow_mutations=true in mcp-config.json, \
                 sibling of the state dir). The app also enforces this gate."
                    .to_string(),
                None,
            ));
        }
        // (2) Resolve the socket path (sibling of state_root, SSOT).
        let Some(sock) = socket_path(&self.state_dir) else {
            return Err(ErrorData::internal_error(
                "no parent for state dir — cannot resolve the mutation socket path".to_string(),
                None,
            ));
        };
        // (3) Dial on a blocking thread with the PER-OP timeout. Same transport selector as
        //     run_mutation: UDS preferred (`dial_op`, unchanged), HTTP additive fallback.
        let state_dir = self.state_dir.clone();
        let reply = tokio::task::spawn_blocking(move || {
            phase_b::dial_selected(&sock, &state_dir, &req, true)
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("dial task join: {e}"), None))?;
        // (4) Map the app's structured reply (preserving the `data` payload).
        match map_router_reply(reply) {
            Ok(ack) => Ok(Json(ack)),
            Err(e) => {
                let msg = phase_b::mutations::mutation_error_message(&e);
                match e {
                    MutationError::AppNotRunning => Err(ErrorData::internal_error(msg, None)),
                    _ => Err(ErrorData::invalid_request(msg, None)),
                }
            }
        }
    }

    // #262 ext: external visible-grid SPAWN. Its own UX pre-gate axis (`allow_external_spawn`),
    // distinct from `allow_mutations` — the app re-enforces authoritatively (pid-pin + harness/
    // repo/count validation), so this pre-check only avoids dialing when the spawn axis is off.
    async fn run_spawn_op(
        &self,
        req: phase_b::socket::SocketRequest,
    ) -> Result<Json<phase_b::mutations::RouterAck>, ErrorData> {
        use agent_teams_core::{read_mcp_config, socket_path};
        use phase_b::mutations::{map_router_reply, MutationError};

        if !read_mcp_config(&self.state_dir).allow_external_spawn {
            return Err(ErrorData::invalid_request(
                "EXTERNAL_SPAWN_DISABLED: external spawn is off (set allow_external_spawn=true in \
                 mcp-config.json, sibling of the state dir). The app also enforces this gate."
                    .to_string(),
                None,
            ));
        }
        let Some(sock) = socket_path(&self.state_dir) else {
            return Err(ErrorData::internal_error(
                "no parent for state dir — cannot resolve the mutation socket path".to_string(),
                None,
            ));
        };
        let state_dir = self.state_dir.clone();
        let reply = tokio::task::spawn_blocking(move || {
            phase_b::dial_selected(&sock, &state_dir, &req, true)
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("dial task join: {e}"), None))?;
        match map_router_reply(reply) {
            Ok(ack) => Ok(Json(ack)),
            Err(e) => {
                let msg = phase_b::mutations::mutation_error_message(&e);
                match e {
                    MutationError::AppNotRunning => Err(ErrorData::internal_error(msg, None)),
                    _ => Err(ErrorData::invalid_request(msg, None)),
                }
            }
        }
    }
}

#[tool_handler(router = self.tool_router)]
#[prompt_handler(router = self.prompt_router)]
impl ServerHandler for TeamServer {
    fn get_info(&self) -> ServerInfo {
        // `ServerInfo` is `#[non_exhaustive]` — start from default, then set the
        // fields we care about (advertise both tools and resources).
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_resources()
            .enable_prompts()
            .build();
        // Base (Phase-A) instructions; the mutation note is appended only when the
        // Phase-B tools are actually registered (feature on), so the advertised
        // surface never contradicts the registered tool set.
        #[allow(unused_mut)]
        let mut instructions = String::from(
            "Read-only Agent Teams state. Tools: team_get_queue, list_workspaces, \
             get_workspace. Resources: team://queue (ranked queue), team://workspaces \
             (id list), team://workspace/{id} (one row). Prompt: \
             agent_teams_developer_guide (how to drive this surface).",
        );
        #[cfg(feature = "phase-b-mutations")]
        instructions.push_str(
            " Mutation tools (Phase B, gated by allow_mutations in mcp-config.json, \
             require the app running): team_send_input (route the human's reply to a \
             live PTY — NEVER auto-approves; gated by its OWN send_input_enabled flag, \
             decoupled from allow_mutations), team_focus_workspace (focus a workspace \
             by id). Context Router tools (06-03, allow_mutations gate): team_orchestrate \
             (synthesize the goal into per-pane tasks; dispatch=false PREVIEWS the \
             mapping, dispatch=true fans it out), team_broadcast (one line to every \
             live pane → {sent,skipped}), team_handoff (relay one line from→to, one \
             hop), team_synthesize (fan-in, two modes — dir: consolidate an orchestrate \
             run dir's pane reports into final.md + per-pane verdicts \
             ok/empty/incomplete/missing; pane_ids: sidecar-driven verbatim merge of ANY \
             panes' outputs resolved like team_read_output (max 16, works app-closed, \
             ungated) — both CLAIMED, not verified).",
        );
        #[cfg(not(feature = "phase-b-mutations"))]
        instructions.push_str(" Phase A: no mutations.");
        #[cfg(feature = "memory-notes")]
        instructions.push_str(
            " Memory tools (Phase 10, UNGATED local file I/O for pain #3 / context loss — \
             no app required, no allow_mutations gate): create_memory, search_memories, \
             find_backlinks, suggest_connections, get_memory, list_memories, update_memory, \
             delete_memory; get_memory_graph (Phase 11 read-only {nodes,edges} projection).",
        );
        #[cfg(feature = "task-tools")]
        instructions.push_str(
            " Task tools (Phase 14, UNGATED local file I/O — no app required, no \
             allow_mutations gate): task_list, task_get (READ the board, each row's \
             lifecycle folded from the append-only transition log; unions the operator \
             kanban with agent-created tasks); task_create {title} (server-minted id, \
             append-only genesis — never the mutable store); task_transition {id,to} \
             (advance the legal lifecycle graph; scope-gated to a pane's own tasks).",
        );
        info.instructions = Some(instructions);
        info
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        let resources = vec![
            RawResource::new(QUEUE_URI, "queue")
                .with_description("The ranked 'who needs me' queue (JSON array of rows).")
                .with_mime_type("application/json")
                .no_annotation(),
            RawResource::new(WORKSPACES_URI, "workspaces")
                .with_description("Discovered workspace ids (sorted JSON array).")
                .with_mime_type("application/json")
                .no_annotation(),
        ];
        Ok(ListResourcesResult {
            resources,
            next_cursor: None,
            meta: None,
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        let uri = request.uri.as_str();
        let json = if uri == QUEUE_URI {
            serde_json::to_string_pretty(&identified_queue(&self.state_dir))
        } else if uri == WORKSPACES_URI {
            serde_json::to_string_pretty(&list_workspaces(&self.state_dir))
        } else if let Some(id) = uri.strip_prefix("team://workspace/") {
            let row = identified_queue(&self.state_dir)
                .into_iter()
                .find(|r| r.id == id);
            serde_json::to_string_pretty(&row)
        } else {
            return Err(ErrorData::resource_not_found(
                format!("unknown resource uri: {uri}"),
                None,
            ));
        }
        .map_err(|e| ErrorData::internal_error(format!("serialize resource: {e}"), None))?;

        // Set the mime type explicitly so it agrees with `list_resources`
        // (`ResourceContents::text` would default to a bare "text").
        Ok(ReadResourceResult::new(vec![
            ResourceContents::TextResourceContents {
                uri: request.uri,
                mime_type: Some("application/json".to_string()),
                text: json,
                meta: None,
            },
        ]))
    }
}

/// THE queue-row read path: the ranked queue with liveness filtering AND spawn
/// identity (`role`/`tag`/`workspace`) joined on — served by `team_get_queue`,
/// `get_workspace`, and the queue/workspace resources.
///
/// Reads the sibling live registry that the app maintains
/// (`agent_teams_core::registry_path` — the single source of truth for that path;
/// the app writer imports the same fn) and hands it to
/// [`compute_queue_identified`]:
///
/// - **Registry present & parses** → the app is running: rows are filtered to its
///   live set (an empty set ⇒ "app up, nothing live" ⇒ empty queue) and each row
///   gains the `role` + `tag` the app recorded at spawn (gap #4 — an external
///   orchestrator can resolve WHICH pane is the coordinator / find its tagged
///   panes without guessing). The registry's `app_pid` is carried for a future
///   process-liveness check but is **not** verified in Phase A.
/// - **Registry absent/invalid** → app-down ⇒ the discovered superset (FR-7),
///   which may include stale workspaces; rows carry only the id-derived
///   `workspace` (role/tag omitted — the serde-additive contract).
///
/// Known Phase-A approximation: a *crashed* app leaves a stale registry, so this
/// can momentarily trust a dead set; event-gating means only ids that still have
/// `events.jsonl` rows ever surface. Precise app-liveness lands in Phase B (over
/// the socket). **Sidecar-side only** — the app writes the registry; we read it.
fn identified_queue(state_dir: &Path) -> Vec<QueueRow> {
    let registry = read_registry(state_dir);
    compute_queue_identified(state_dir, registry.as_ref())
}

/// `$AGENT_TEAMS_STATE_DIR`, else `~/Library/Application Support/harness-ready/agent-teams`
/// (mirrors `core/state-adapter/src/bin/adapter.rs`).
fn default_state_dir() -> PathBuf {
    if let Ok(d) = std::env::var("AGENT_TEAMS_STATE_DIR") {
        return PathBuf::from(d);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join("Library/Application Support/harness-ready/agent-teams")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let server = TeamServer::new(default_state_dir());
    // Serve MCP over stdio (stdin/stdout). Logs MUST go to stderr — stdout is
    // the protocol channel.
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

// The SHIPPED mutation path is run_mutation / run_router_op (the registered #[tool]
// methods route through them inline). The existing tests in phase_b/mutations.rs cover
// the free team_* wrappers (NOT on this path) + map_reply. These drive the registered
// path's capability gate directly: the SAFE-default refusal (config absent ⇒ mutations
// off) must reject BEFORE dialing, and an enabled gate must let the dial through.
#[cfg(all(test, feature = "phase-b-mutations"))]
mod gate_tests {
    use super::*;
    use phase_b::socket::SocketRequest;

    /// A unique temp state dir; the mutation socket + mcp-config.json are its siblings.
    fn scratch(tag: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!("at-mcp-gate-{tag}-{nonce}"));
        let _ = std::fs::remove_dir_all(&root);
        let state = root.join("state");
        std::fs::create_dir_all(&state).unwrap();
        state
    }

    #[tokio::test]
    async fn run_mutation_refuses_before_dialing_when_disabled() {
        // no mcp-config.json → allow_mutations defaults FALSE → refuse at the gate,
        // never dialing (so the message is MUTATIONS_DISABLED, NOT APP_NOT_RUNNING).
        let srv = TeamServer::new(scratch("off"));
        let Err(err) = srv
            .run_mutation(SocketRequest::Focus { id: "w".into() })
            .await
        else {
            panic!("disabled mutations must be refused");
        };
        assert!(
            err.message.contains("MUTATIONS_DISABLED"),
            "got: {}",
            err.message
        );
        assert!(
            !err.message.contains("APP_NOT_RUNNING"),
            "must not have dialed: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn run_mutation_passes_gate_when_enabled_then_app_not_running() {
        // allow_mutations=true + no live app → past the gate, dials, app-down. Proves the
        // gate did NOT short-circuit (no MUTATIONS_DISABLED) and surfaces APP_NOT_RUNNING.
        let state = scratch("on");
        let cfg = agent_teams_core::mcp_config_path(&state).expect("config path");
        std::fs::write(&cfg, r#"{"allow_mutations":true}"#).unwrap();
        let srv = TeamServer::new(state);
        let Err(err) = srv
            .run_mutation(SocketRequest::Focus { id: "w".into() })
            .await
        else {
            panic!("no live app → app-not-running");
        };
        assert!(
            !err.message.contains("MUTATIONS_DISABLED"),
            "gate must let it through: {}",
            err.message
        );
        assert!(
            err.message.contains("APP_NOT_RUNNING"),
            "got: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn run_router_op_refuses_before_dialing_when_disabled() {
        // the Context Router path (orchestrate/broadcast/handoff) shares the same gate.
        let srv = TeamServer::new(scratch("router-off"));
        let Err(err) = srv
            .run_router_op(SocketRequest::Focus { id: "w".into() })
            .await
        else {
            panic!("disabled mutations must be refused on the router path too");
        };
        assert!(
            err.message.contains("MUTATIONS_DISABLED"),
            "got: {}",
            err.message
        );
    }
}

#[cfg(test)]
mod format_strip_tests {
    use super::strip_numeric_formats_in_value;
    use serde_json::json;

    #[test]
    fn strips_numeric_formats_at_every_depth_but_keeps_string_formats() {
        let mut v = json!({
            "type": "object",
            "properties": { "since": { "type": "integer", "format": "uint64" } },
            "$defs": {
                "Row": {
                    "type": "object",
                    "properties": {
                        "score": { "type": "integer", "format": "uint32" },
                        "when": { "type": "string", "format": "date-time" },
                        "items": {
                            "type": "array",
                            "items": { "type": "integer", "format": "int64" }
                        }
                    }
                }
            }
        });
        strip_numeric_formats_in_value(&mut v);
        // numeric formats gone at top level, inside $defs, and in array items
        assert!(v["properties"]["since"].get("format").is_none());
        assert!(v["$defs"]["Row"]["properties"]["score"]
            .get("format")
            .is_none());
        assert!(v["$defs"]["Row"]["properties"]["items"]["items"]
            .get("format")
            .is_none());
        // standard string formats are preserved
        assert_eq!(
            v["$defs"]["Row"]["properties"]["when"]["format"],
            "date-time"
        );
    }
}
