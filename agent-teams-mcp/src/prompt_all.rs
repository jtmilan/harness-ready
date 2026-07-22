//! `team_prompt_all` — broadcast one prompt to every live pane and collect responses.
//!
//! Compiled only under `--features phase-b-mutations` (sends input via the app socket).
//! Replaces the coordinator's external Python/script loop of:
//! 1. dial `SendInput` per pane
//! 2. poll / wait
//! 3. `team_read_output` each pane
//! 4. aggregate
//!
//! with a single MCP tool call. Send path reuses the same `SocketRequest::SendInput`
//! wire the mutation tools use; collect path reuses [`crate::read_output::resolve`]
//! (disk-first, live-scrollback fallback).
//!
//! ## Runtime gates
//! - Feature: `phase-b-mutations` (registered only on the coordinator sidecar).
//! - Config: `send_input_enabled=true` (each pane is prompted via `SendInput`, so the
//!   same narrow gate as `team_send_input` applies; the app re-enforces).
//! - App: coordinator peer-pid / external-orchestrator admission on each SendInput
//!   and on any live-scrollback ReadOutput.

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use agent_teams_core::{read_registry, socket_path, ws_prefix, LiveWorkspace, SocketRequest};

use crate::phase_b::mutations::{map_reply, MutationError};
use crate::phase_b::{self};
use crate::read_output::{self, PaneOutputResult};

/// Default wait for all panes to respond (seconds).
pub const DEFAULT_TIMEOUT_SECS: u32 = 60;
/// Hard cap on the wait (seconds).
pub const MAX_TIMEOUT_SECS: u32 = 300;
/// Poll interval while waiting for pane responses.
const POLL_INTERVAL: Duration = Duration::from_millis(750);
/// Consecutive identical non-baseline snapshots required before we treat a response
/// as settled (avoids returning a mid-stream live-scrollback fragment).
const STABLE_POLLS: u32 = 2;
/// Minimum new characters after the prompt (or beyond baseline) before we consider
/// a pane "has started responding".
const MIN_NEW_CHARS: usize = 8;

/// Default role exclusions — the coordinator should not prompt itself.
pub const DEFAULT_EXCLUDE_ROLES: &[&str] = &["coordinator"];

/// Arguments for `team_prompt_all`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PromptAllArgs {
    /// The prompt text to send to every live pane (one line, no interior newlines).
    pub prompt: String,
    /// Seconds to wait for responses (default 60, max 300).
    #[serde(default)]
    pub timeout_secs: Option<u32>,
    /// Scope to one workspace id (e.g. `"ws72581x0"`) or create-time tag. When
    /// absent, uses the caller's workspace (`$AGENT_TEAMS_PANE_ID` prefix), then the
    /// registry's `active` workspace, then every live pane.
    #[serde(default)]
    pub target_workspace: Option<String>,
    /// Exclude panes with these roles (default: exclude `"coordinator"`).
    #[serde(default)]
    pub exclude_roles: Option<Vec<String>>,
}

/// Aggregated result of `team_prompt_all`. Object-rooted (MCP `outputSchema`).
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct PromptAllResult {
    /// Successful responses keyed by pane id.
    pub responses: Vec<PaneResponse>,
    /// Panes that failed (dead, timeout, send rejected, etc).
    pub errors: Vec<PaneError>,
    /// Total elapsed wall time in seconds.
    pub elapsed_secs: f64,
}

/// One pane's collected response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct PaneResponse {
    pub id: String,
    pub harness: String,
    /// Extracted response text (delta after the prompt / new assistant block).
    pub response: String,
    /// Where the content came from — same source labels as `team_read_output`
    /// (`claude_transcript` | `cursor_transcript` | `grok_transcript` |
    /// `live_scrollback` | `orchestrate_report` | …).
    pub source: String,
}

/// One pane that did not yield a usable response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct PaneError {
    pub id: String,
    /// Machine-ish reason: `timeout` | `send_failed` | `no_output` | `dead` | …
    pub error: String,
    /// Optional detail (app rejection message, etc).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// A live pane selected for the fan-out (id + harness for the result rows).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetPane {
    pub id: String,
    pub harness: String,
    pub role: Option<String>,
}

/// Select live panes for `team_prompt_all`. Pure → unit-testable.
///
/// - `target_workspace`: when `Some`, keep panes whose [`ws_prefix`] or `tag`
///   matches (case-sensitive, same as orchestrate scoping).
/// - `exclude_roles`: drop panes whose role matches any entry (case-insensitive).
///   An empty exclude list excludes nothing.
pub fn select_target_panes(
    workspaces: &[LiveWorkspace],
    target_workspace: Option<&str>,
    exclude_roles: &[String],
) -> Vec<TargetPane> {
    let exclude: HashSet<String> = exclude_roles
        .iter()
        .map(|r| r.trim().to_ascii_lowercase())
        .filter(|r| !r.is_empty())
        .collect();

    workspaces
        .iter()
        .filter(|w| {
            if let Some(tw) = target_workspace.map(str::trim).filter(|s| !s.is_empty()) {
                let ws = ws_prefix(&w.id);
                let tag_ok = w.tag.as_deref() == Some(tw);
                if ws != tw && !tag_ok {
                    return false;
                }
            }
            let role = w.role.as_deref().unwrap_or("").trim().to_ascii_lowercase();
            if !role.is_empty() && exclude.contains(&role) {
                return false;
            }
            true
        })
        .map(|w| TargetPane {
            id: w.id.clone(),
            harness: w.harness.clone().unwrap_or_else(|| "unknown".into()),
            role: w.role.clone(),
        })
        .collect()
}

/// Resolve the workspace scope when the caller omitted `target_workspace`.
/// Order: caller's `$AGENT_TEAMS_PANE_ID` prefix → registry `active` → `None`
/// (all live panes, still subject to role exclusion).
pub fn resolve_target_workspace(
    explicit: Option<&str>,
    caller_pane_id: Option<&str>,
    registry_active: Option<&str>,
) -> Option<String> {
    if let Some(tw) = explicit.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(tw.to_string());
    }
    if let Some(pane) = caller_pane_id.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(ws_prefix(pane).to_string());
    }
    registry_active
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Clamp timeout to `[1, MAX_TIMEOUT_SECS]`, defaulting to [`DEFAULT_TIMEOUT_SECS`].
pub fn clamp_timeout(timeout_secs: Option<u32>) -> u32 {
    timeout_secs
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
        .clamp(1, MAX_TIMEOUT_SECS)
}

/// Extract a usable response from a pane's post-prompt output, relative to the
/// pre-prompt baseline. Pure → unit-testable.
///
/// Ready when:
/// 1. Current content differs from the baseline snapshot, AND
/// 2. Either the prompt appears and text after it is substantial, OR the content
///    grew past the baseline by [`MIN_NEW_CHARS`], OR a new `[assistant]` block
///    appeared that was not in the baseline.
///
/// Returns `None` when the pane has not yet produced a response we can claim.
pub fn extract_response(
    baseline: Option<&str>,
    current: &str,
    prompt: &str,
    source: &str,
) -> Option<String> {
    if source == "none" || current.trim().is_empty() {
        return None;
    }
    if baseline.map(|b| b == current).unwrap_or(false) {
        return None;
    }

    // Prefer text after the most recent occurrence of our prompt (echoed into
    // transcript / scrollback).
    if !prompt.is_empty() {
        if let Some(idx) = current.rfind(prompt) {
            let after = current[idx + prompt.len()..].trim();
            if let Some(resp) = newest_assistant_block(after) {
                if resp.chars().count() >= MIN_NEW_CHARS {
                    return Some(resp);
                }
            }
            // Live scrollback / plain text: any substantial tail after the prompt.
            if after.chars().count() >= MIN_NEW_CHARS {
                // Strip a leading prompt-echo newline junk.
                return Some(after.to_string());
            }
            // Prompt seen but no response body yet.
            return None;
        }
    }

    // Prompt not found in content yet — fall back to delta-from-baseline / new assistant.
    if let Some(base) = baseline {
        if let Some(resp) = newest_assistant_block(current) {
            let base_asst = newest_assistant_block(base);
            if base_asst.as_deref() != Some(resp.as_str()) && resp.chars().count() >= MIN_NEW_CHARS
            {
                return Some(resp);
            }
        }
        if current.starts_with(base) {
            let delta = current[base.len()..].trim();
            if delta.chars().count() >= MIN_NEW_CHARS {
                return Some(delta.to_string());
            }
            return None;
        }
        // Content rewrote (common for live_scrollback tails) — take the newest
        // assistant block, else the whole current tail if it clearly differs.
        if let Some(resp) = newest_assistant_block(current) {
            if resp.chars().count() >= MIN_NEW_CHARS {
                return Some(resp);
            }
        }
        if current != base && current.chars().count() >= MIN_NEW_CHARS {
            return Some(current.to_string());
        }
        return None;
    }

    // No baseline (pane had no prior output).
    if let Some(resp) = newest_assistant_block(current) {
        if resp.chars().count() >= MIN_NEW_CHARS {
            return Some(resp);
        }
    }
    if current.chars().count() >= MIN_NEW_CHARS {
        return Some(current.to_string());
    }
    None
}

/// Last `[assistant] …` block in a rendered transcript (see `read_output::extract_transcript_text`).
fn newest_assistant_block(text: &str) -> Option<String> {
    let marker = "[assistant]";
    let idx = text.rfind(marker)?;
    let body = text[idx + marker.len()..].trim();
    // Stop at the next role marker if present (shouldn't for the last block, but be safe).
    let body = body
        .split("\n[user]")
        .next()
        .unwrap_or(body)
        .split("\n[assistant]")
        .next()
        .unwrap_or(body)
        .trim();
    if body.is_empty() {
        None
    } else {
        Some(body.to_string())
    }
}

/// Per-pane tracking state during the poll loop.
struct PaneTrack {
    id: String,
    harness: String,
    baseline: Option<String>,
    /// Last extracted candidate response (for stability checks).
    last_candidate: Option<String>,
    stable_count: u32,
    done: bool,
    response: Option<PaneResponse>,
    error: Option<PaneError>,
}

/// Run the full broadcast-and-collect. Blocking (call from `spawn_blocking`).
///
/// Caller is responsible for the `send_input_enabled` UX pre-gate; this fn still
/// surfaces per-pane app rejections into `errors`.
pub fn run(state_dir: &Path, args: PromptAllArgs) -> PromptAllResult {
    let started = Instant::now();
    let timeout = Duration::from_secs(u64::from(clamp_timeout(args.timeout_secs)));
    let prompt = args.prompt;
    let exclude = args
        .exclude_roles
        .unwrap_or_else(|| DEFAULT_EXCLUDE_ROLES.iter().map(|s| (*s).to_string()).collect());

    // ── 1. Live registry → target panes ─────────────────────────────────────
    let Some(reg) = read_registry(state_dir) else {
        return PromptAllResult {
            responses: vec![],
            errors: vec![PaneError {
                id: "*".into(),
                error: "APP_NOT_RUNNING".into(),
                detail: Some(
                    "live registry absent — Agent Teams app is not running (or has no panes)"
                        .into(),
                ),
            }],
            elapsed_secs: elapsed_secs(started),
        };
    };

    let caller_pane = std::env::var("AGENT_TEAMS_PANE_ID").ok();
    let target_ws = resolve_target_workspace(
        args.target_workspace.as_deref(),
        caller_pane.as_deref(),
        reg.active.as_deref(),
    );
    let targets = select_target_panes(&reg.workspaces, target_ws.as_deref(), &exclude);

    if targets.is_empty() {
        return PromptAllResult {
            responses: vec![],
            errors: vec![PaneError {
                id: "*".into(),
                error: "NO_TARGETS".into(),
                detail: Some(format!(
                    "no live panes matched (target_workspace={:?}, exclude_roles={exclude:?})",
                    target_ws
                )),
            }],
            elapsed_secs: elapsed_secs(started),
        };
    }

    // ── 2. Baseline snapshot + SendInput per pane ───────────────────────────
    let Some(sock) = socket_path(state_dir) else {
        return PromptAllResult {
            responses: vec![],
            errors: vec![PaneError {
                id: "*".into(),
                error: "INTERNAL".into(),
                detail: Some("no parent for state dir — cannot resolve the mutation socket".into()),
            }],
            elapsed_secs: elapsed_secs(started),
        };
    };

    // Capture prompt timestamp (unix millis) for audit/debug; response readiness
    // is driven by content delta, not wall-clock mtime (transcript mtimes can lag).
    let _prompt_ts_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let mut tracks: Vec<PaneTrack> = Vec::with_capacity(targets.len());
    for t in &targets {
        let baseline = read_output::resolve(state_dir, &t.id, None)
            .content
            .filter(|c| !c.is_empty());

        let req = SocketRequest::SendInput {
            id: t.id.clone(),
            text: prompt.clone(),
        };
        // Fast dial (SendInput is a short op); UDS preferred, HTTP fallback.
        let send = map_reply(phase_b::dial_selected(&sock, state_dir, &req, false));

        let mut track = PaneTrack {
            id: t.id.clone(),
            harness: t.harness.clone(),
            baseline,
            last_candidate: None,
            stable_count: 0,
            done: false,
            response: None,
            error: None,
        };
        match send {
            Ok(_) => {}
            Err(MutationError::AppNotRunning) => {
                track.done = true;
                track.error = Some(PaneError {
                    id: t.id.clone(),
                    error: "APP_NOT_RUNNING".into(),
                    detail: Some("app socket unreachable during SendInput".into()),
                });
            }
            Err(MutationError::Rejected { code, detail }) => {
                track.done = true;
                let err = if code == "DEAD_PANE" {
                    "dead"
                } else {
                    "send_failed"
                };
                track.error = Some(PaneError {
                    id: t.id.clone(),
                    error: err.into(),
                    detail: Some(if detail.trim().is_empty() {
                        code
                    } else {
                        format!("{code}: {detail}")
                    }),
                });
            }
            Err(MutationError::Incomplete(s)) => {
                track.done = true;
                track.error = Some(PaneError {
                    id: t.id.clone(),
                    error: "send_failed".into(),
                    detail: Some(s.into()),
                });
            }
        }
        tracks.push(track);
    }

    // If every send failed immediately, skip the poll loop.
    if tracks.iter().all(|t| t.done) {
        return finish(started, tracks);
    }

    // ── 3. Poll until all settled or timeout ────────────────────────────────
    let deadline = started + timeout;
    while Instant::now() < deadline {
        if tracks.iter().all(|t| t.done) {
            break;
        }
        std::thread::sleep(POLL_INTERVAL);

        for track in tracks.iter_mut().filter(|t| !t.done) {
            let out: PaneOutputResult = read_output::resolve(state_dir, &track.id, None);
            let content = out.content.as_deref().unwrap_or("");
            match extract_response(track.baseline.as_deref(), content, &prompt, &out.source) {
                Some(resp) => {
                    if track.last_candidate.as_deref() == Some(resp.as_str()) {
                        track.stable_count = track.stable_count.saturating_add(1);
                    } else {
                        track.last_candidate = Some(resp);
                        track.stable_count = 1;
                    }
                    if track.stable_count >= STABLE_POLLS {
                        let response = track.last_candidate.take().unwrap_or_default();
                        track.response = Some(PaneResponse {
                            id: track.id.clone(),
                            harness: out
                                .harness
                                .clone()
                                .unwrap_or_else(|| track.harness.clone()),
                            response,
                            source: out.source.clone(),
                        });
                        track.done = true;
                    }
                }
                None => {
                    // Still waiting — reset stability so a later change restarts the settle.
                    track.stable_count = 0;
                    track.last_candidate = None;
                }
            }
        }
    }

    // ── 4. Timeouts for unfinished panes ────────────────────────────────────
    for track in tracks.iter_mut().filter(|t| !t.done) {
        // If we have an unstable candidate at deadline, still return it (better
        // than a pure timeout with nothing) — marked via source as-is.
        if let Some(resp) = track.last_candidate.take() {
            track.response = Some(PaneResponse {
                id: track.id.clone(),
                harness: track.harness.clone(),
                response: resp,
                source: "partial".into(),
            });
        } else {
            track.error = Some(PaneError {
                id: track.id.clone(),
                error: "timeout".into(),
                detail: Some(format!(
                    "no response within {}s (last source may still be none/mid-stream)",
                    timeout.as_secs()
                )),
            });
        }
        track.done = true;
    }

    finish(started, tracks)
}

fn finish(started: Instant, tracks: Vec<PaneTrack>) -> PromptAllResult {
    let mut responses = Vec::new();
    let mut errors = Vec::new();
    for t in tracks {
        if let Some(r) = t.response {
            responses.push(r);
        } else if let Some(e) = t.error {
            errors.push(e);
        } else {
            errors.push(PaneError {
                id: t.id,
                error: "no_output".into(),
                detail: None,
            });
        }
    }
    // Stable ordering by pane id for deterministic clients.
    responses.sort_by(|a, b| a.id.cmp(&b.id));
    errors.sort_by(|a, b| a.id.cmp(&b.id));
    PromptAllResult {
        responses,
        errors,
        elapsed_secs: elapsed_secs(started),
    }
}

fn elapsed_secs(started: Instant) -> f64 {
    started.elapsed().as_secs_f64()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_teams_core::LiveWorkspace;

    fn pane(id: &str, harness: &str, role: Option<&str>, tag: Option<&str>) -> LiveWorkspace {
        LiveWorkspace {
            id: id.into(),
            pid: None,
            harness: Some(harness.into()),
            repo: None,
            role: role.map(str::to_string),
            tag: tag.map(str::to_string),
            session_id: None,
            spawned_at: None,
        }
    }

    #[test]
    fn select_target_panes_excludes_coordinator_by_default_roles() {
        let ws = vec![
            pane("ws1-p0", "claude", Some("coordinator"), None),
            pane("ws1-p1", "claude", Some("builder"), None),
            pane("ws1-p2", "cursor", Some("reviewer"), None),
            pane("ws1-p3", "codex", None, None), // no role → kept
        ];
        let exclude: Vec<String> = DEFAULT_EXCLUDE_ROLES.iter().map(|s| (*s).into()).collect();
        let got = select_target_panes(&ws, Some("ws1"), &exclude);
        let ids: Vec<_> = got.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec!["ws1-p1", "ws1-p2", "ws1-p3"]);
    }

    #[test]
    fn select_target_panes_scopes_by_workspace_prefix_or_tag() {
        let ws = vec![
            pane("wsA-p0", "claude", Some("builder"), Some("team-a")),
            pane("wsB-p0", "claude", Some("builder"), Some("team-b")),
            pane("wsB-p1", "cursor", Some("scout"), Some("team-b")),
        ];
        let none: Vec<String> = vec![];
        let by_ws = select_target_panes(&ws, Some("wsB"), &none);
        assert_eq!(
            by_ws.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            vec!["wsB-p0", "wsB-p1"]
        );
        let by_tag = select_target_panes(&ws, Some("team-a"), &none);
        assert_eq!(
            by_tag.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            vec!["wsA-p0"]
        );
    }

    #[test]
    fn select_target_panes_role_exclude_is_case_insensitive() {
        let ws = vec![
            pane("ws1-p0", "claude", Some("Coordinator"), None),
            pane("ws1-p1", "claude", Some("BUILDER"), None),
        ];
        let exclude = vec!["COORDINATOR".into()];
        let got = select_target_panes(&ws, None, &exclude);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "ws1-p1");
    }

    #[test]
    fn resolve_target_workspace_prefers_explicit_then_caller_then_active() {
        assert_eq!(
            resolve_target_workspace(Some("wsX"), Some("wsY-p0"), Some("wsZ")),
            Some("wsX".into())
        );
        assert_eq!(
            resolve_target_workspace(None, Some("wsY-p3"), Some("wsZ")),
            Some("wsY".into())
        );
        assert_eq!(
            resolve_target_workspace(None, None, Some("wsZ")),
            Some("wsZ".into())
        );
        assert_eq!(resolve_target_workspace(None, None, None), None);
        assert_eq!(resolve_target_workspace(Some("  "), None, None), None);
    }

    #[test]
    fn clamp_timeout_bounds() {
        assert_eq!(clamp_timeout(None), DEFAULT_TIMEOUT_SECS);
        assert_eq!(clamp_timeout(Some(0)), 1);
        assert_eq!(clamp_timeout(Some(9999)), MAX_TIMEOUT_SECS);
        assert_eq!(clamp_timeout(Some(30)), 30);
    }

    #[test]
    fn extract_response_waits_until_assistant_block_after_prompt() {
        let prompt = "What is 2+2?";
        let baseline = "[user] hi\n[assistant] hello\n";
        // Prompt echoed but no assistant yet.
        let mid = format!("{baseline}[user] {prompt}\n");
        assert!(extract_response(Some(baseline), &mid, prompt, "claude_transcript").is_none());

        // Short answers fall below MIN_NEW_CHARS; use a full sentence.
        let done = format!("{mid}[assistant] The answer is four (4).\n");
        let r = extract_response(Some(baseline), &done, prompt, "claude_transcript")
            .expect("assistant block after prompt");
        assert!(r.contains("four"), "{r}");
    }

    #[test]
    fn extract_response_uses_delta_for_live_scrollback() {
        let prompt = "STATUS";
        let baseline = "old scroll\n$ ";
        let mid = format!("{baseline}{prompt}\n");
        assert!(extract_response(Some(baseline), &mid, prompt, "live_scrollback").is_none());

        let done = format!("{mid}ok all systems green\n");
        let r = extract_response(Some(baseline), &done, prompt, "live_scrollback").expect("delta");
        assert!(r.contains("ok all systems green"), "{r}");
    }

    #[test]
    fn extract_response_none_when_unchanged_or_source_none() {
        let c = "same content here long enough";
        assert!(extract_response(Some(c), c, "p", "claude_transcript").is_none());
        assert!(extract_response(None, "hello world!!", "p", "none").is_none());
    }

    #[test]
    fn extract_response_no_baseline_takes_assistant_or_full() {
        let text = "[user] go\n[assistant] I am Auto in pane p5 reporting ready.\n";
        let r = extract_response(None, text, "go", "claude_transcript").unwrap();
        assert!(r.contains("Auto in pane p5"), "{r}");
    }
}
