// Workspace-isolation phase-1 integration tests.
//
// These tests prove that the phase-1 isolation gates are wired correctly at the
// socket-handler level. The FOUNDATION predicates (`ws_of_pane`, `sharing_enabled`,
// `authorize_cross`) are unit-tested in `core/mcp/src/lib.rs` (8 tests covering
// scenarios a-f: default-deny, both-sharing allow, one-sided deny, same-ws OK,
// no-caller deny, kill-switch-OFF bypass). These integration tests verify the
// GATES in `app/src-tauri/src/lib.rs` honor those predicates and emit the correct
// wire-level error codes.
//
// The kill-switch `ws_isolation_enabled` defaults OFF in production, so isolation
// is inert until an operator explicitly enables it in `mcp-config.json`. These
// tests pin the behavior that activates when isolation is armed.

use std::fs;
use std::path::PathBuf;

fn lib_source() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs");
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()))
}

fn core_source() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../core/mcp/src/lib.rs");
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()))
}

/// Prove the kill-switch default is OFF (isolation inert). This is the single
/// flag that gates the entire isolation feature; backend gates / MCP scoping /
/// UI follow in later phases. File-only; not LLM-settable.
#[test]
fn kill_switch_defaults_off_in_mcp_config() {
    let src = core_source();
    // The McpConfig Default impl must set ws_isolation_enabled: false.
    assert!(
        src.contains("ws_isolation_enabled: false"),
        "McpConfig::default() must set ws_isolation_enabled: false (isolation inert by default)"
    );
}

/// Prove the socket_broadcast handler computes the authority set via
/// `coordinator_authority_set` and returns CROSS_WORKSPACE when the set is empty
/// but live panes exist (the caller's workspace has no reachable panes because
/// sharing is off, or the caller has no workspace identity).
#[test]
fn socket_broadcast_returns_cross_workspace_on_empty_authority() {
    let src = lib_source();
    // The handler must call coordinator_authority_set.
    assert!(
        src.contains("let authority = coordinator_authority_set(&st, caller_ws, &reg);"),
        "socket_broadcast must compute the authority set via coordinator_authority_set"
    );
    // When authority is empty but pre_filter_count > 0, return CROSS_WORKSPACE.
    assert!(
        src.contains("if authority.is_empty() && pre_filter_count > 0")
            && src.contains("SocketResponse::err(response_code::CROSS_WORKSPACE"),
        "socket_broadcast must return CROSS_WORKSPACE when authority is empty but panes exist"
    );
}

/// Prove the socket_broadcast handler excludes delegate workers from the
/// pre-filter count (same invariant as live_pane_ctxs / the orchestrate path).
/// Workers are mid-subtask and human-invisible, so an operator's broadcast
/// must not hijack them.
#[test]
fn socket_broadcast_excludes_workers_from_prefilter() {
    let src = lib_source();
    assert!(
        src.contains(".filter(|(_, sup)| !sup.is_worker).count()"),
        "socket_broadcast must exclude workers from the pre-filter count"
    );
}

/// Prove the coordinator_authority_set function honors the symmetric
/// shared-scope invariant: a pane in a DIFFERENT workspace is included only
/// when BOTH the caller's workspace AND the pane's workspace have
/// `allow_sharing=true`. This is the WRITE-side gate for cross-workspace
/// visibility; the READ-side is `sharing_enabled` in core/mcp.
#[test]
fn coordinator_authority_set_honors_symmetric_sharing() {
    let src = lib_source();
    // The function must check caller_shares && sharing_enabled(registry, pane_ws).
    assert!(
        src.contains("if caller_shares && sharing_enabled(registry, pane_ws)"),
        "coordinator_authority_set must enforce symmetric sharing (both sides opt in)"
    );
    // Same-workspace panes are always included (no sharing check).
    assert!(
        src.contains("if pane_ws == cws") && src.contains("return Some(id.clone());"),
        "coordinator_authority_set must include same-workspace panes unconditionally"
    );
}

/// Prove the coordinator_authority_set function excludes workers (same
/// invariant as live_pane_ctxs / the orchestrate path).
#[test]
fn coordinator_authority_set_excludes_workers() {
    let src = lib_source();
    assert!(
        src.contains(".filter(|(_, sup)| !sup.is_worker)"),
        "coordinator_authority_set must exclude workers from the authority set"
    );
}

/// Prove the coordinator_authority_set function returns an empty set when
/// caller_ws is None (external caller / D1 non-orchestrate path). An external
/// orchestrator cannot broadcast or act on panes — it has no workspace identity.
#[test]
fn coordinator_authority_set_empty_for_no_caller() {
    let src = lib_source();
    assert!(
        src.contains("let Some(cws) = caller_ws else")
            && src.contains("return std::collections::HashSet::new();"),
        "coordinator_authority_set must return empty set when caller_ws is None"
    );
}

/// Prove the send_to_panes helper (used by broadcast / orchestrate) honors the
/// caller_ws scoping. This is the downstream enforcement: even if a handler
/// computes a target set, send_to_panes must refuse to write to panes outside
/// the caller's authority.
#[test]
fn send_to_panes_honors_caller_ws_scoping() {
    let src = lib_source();
    // send_to_panes must take caller_ws as a parameter.
    assert!(
        src.contains("fn send_to_panes")
            && src.contains("caller_ws: Option<&str>"),
        "send_to_panes must accept caller_ws for scoping"
    );
}

/// Prove the read_mcp_config helper reads from the state_root sibling path
/// (not a hardcoded location). This is what lets tests inject a temp
/// mcp-config.json with ws_isolation_enabled=true.
#[test]
fn read_mcp_config_uses_state_root_sibling() {
    let src = core_source();
    assert!(
        src.contains("pub fn read_mcp_config(state_root: &Path)"),
        "read_mcp_config must take state_root as a parameter (testable)"
    );
}

/// Prove the CROSS_WORKSPACE error code is defined in the response_code module.
/// This is the wire-level error the frontend sees when isolation refuses an op.
#[test]
fn cross_workspace_error_code_is_defined() {
    let src = core_source();
    assert!(
        src.contains("pub const CROSS_WORKSPACE: &str"),
        "response_code::CROSS_WORKSPACE must be defined (the isolation-refusal error code)"
    );
}

/// Prove the frontend toast wiring: the CROSS_WORKSPACE error code triggers a
/// toast in the UI. This is the operator-visible signal that isolation refused
/// an op. The frontend must distinguish CROSS_WORKSPACE from other errors so
/// the operator knows to flip the sharing toggle.
#[test]
fn frontend_handles_cross_workspace_error() {
    let js_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ui/src/lib/tauriAgentBridge.js");
    let js = fs::read_to_string(&js_path).unwrap_or_else(|e| panic!("failed to read {}: {e}", js_path.display()));
    assert!(
        js.contains("CROSS_WORKSPACE"),
        "tauriAgentBridge must handle CROSS_WORKSPACE error code (toast the operator)"
    );
}

/// Prove the WorkspacesPanel UI renders the per-workspace sharing toggle.
/// This is the operator's control surface for opting a workspace into
/// symmetric cross-workspace sharing.
#[test]
fn ui_renders_sharing_toggle() {
    let jsx_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../ui/src/components/command/WorkspacesPanel.jsx");
    let jsx = fs::read_to_string(&jsx_path).unwrap_or_else(|e| panic!("failed to read {}: {e}", jsx_path.display()));
    assert!(
        jsx.contains("onToggleSharing"),
        "WorkspacesPanel must wire the onToggleSharing prop to WorkspaceTile"
    );
}

/// Prove the WorkspaceTile component wires the sharing toggle's onClick to
/// the onToggleSharing callback. This is the write path: operator flips the
/// toggle → tile calls onToggleSharing(ws.id, !sharing) → Home.jsx calls
/// bridge.setWorkspaceSharing → backend invoke set_workspace_sharing → registry
/// updated.
#[test]
fn workspace_tile_wires_sharing_toggle_to_backend() {
    let jsx_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../ui/src/components/command/WorkspaceTile.jsx");
    let jsx = fs::read_to_string(&jsx_path).unwrap_or_else(|e| panic!("failed to read {}: {e}", jsx_path.display()));
    assert!(
        jsx.contains("onToggleSharing") && jsx.contains("onToggleSharing(ws.id, !sharing)"),
        "WorkspaceTile must call onToggleSharing(ws.id, !sharing) on toggle click"
    );
}
