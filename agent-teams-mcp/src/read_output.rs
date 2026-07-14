//! `team_read_output` — read a pane's produced OUTPUT (its report / transcript),
//! the missing read primitive an orchestrating brain needs to "go read what p4
//! produced". Always-on (registered in the base read router, like `team_get_queue`):
//! pure, ungated local file I/O over artifacts the user already produced on disk.
//!
//! ## Why this exists
//! The read tools (`team_get_queue` / `get_workspace`) expose STATUS rows only —
//! `{id, harness, state, reason, …}` — never the text a pane produced. An external
//! brain that wanted a pane's report had no tool, so it GUESSED a filesystem path and
//! 404'd (it hallucinated `agent-team/s-orchestrate` for the real single segment
//! `agent-teams-orchestrate`). This tool closes that gap: the caller passes a PANE ID
//! and the SERVER resolves the source — the brain never constructs a path.
//!
//! ## Safe-read contract (the security posture for this NEW exfil surface)
//! - **id-in, server-resolves-path.** Validate with [`validate_spawn_id`] (rejects
//!   `..` / `/` / `\` / whitespace; len-capped) → traversal is structurally
//!   impossible. A raw filesystem path from the caller is NEVER accepted.
//! - **Confined to AT-controlled artifacts.** Only the orchestrate/bridge `<id>.md`
//!   reports and the pane's OWN harness transcript — never arbitrary repo files.
//! - **Tail-capped.** `max_bytes` clamped server-side; the newest tail is returned
//!   with a `truncated` flag.
//! - **Audited.** Every read appends `{ts,op,id,source,bytes}` (NOT the content) to
//!   `agent-teams-external-reads.jsonl` (sibling of the state root), best-effort.
//!
//! ## Source precedence (per the harness-output investigation, 2026-06-30)
//! 1. **orchestrate/bridge report** `<run_dir>/<id>.md` — harness-agnostic, the
//!    cleanest "what this pane produced"; the primary answer to "give me p4's report".
//! 2. **claude transcript** `~/.claude/projects/*/<session>.jsonl` — for claude panes
//!    when no report exists. Located FIRST by the registry-recorded `session_id` (the
//!    transcript filename — cwd/encoding-independent, so a pane launched at cwd `/`
//!    whose project dir slugs to a bare `-` is still found; C1), then falling back to
//!    the encoded-worktree-dir suffix match when no session id was recorded.
//! 3. **live scrollback** (gap-7, `phase-b-mutations` builds only) — when every disk
//!    source misses but the live registry says the pane EXISTS, dial the running app's
//!    socket (`SocketRequest::ReadOutput`) for a tail of the pane's IN-MEMORY PTY
//!    scrollback. This is what makes state-blind harnesses (commandcode/codex/opencode/
//!    cline — no on-disk transcript at all) readable. UNVERIFIED live data (may be
//!    mid-stream); app-down / gate-off / any error falls through honestly. Base
//!    (read-only) builds compile the disk-only behavior — no socket dial exists.
//! 4. **honest "none"** — nothing on disk AND no live read possible; say so rather
//!    than fabricate (noting when a live read was attempted).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use agent_teams_core::{read_registry, validate_session_id, validate_spawn_id};

/// Default + hard cap on returned content bytes (the newest tail is kept). SSOT in
/// `agent_teams_core` (shared with the app-side `ReadOutput` handler so the disk and
/// live-scrollback tails obey ONE contract).
const DEFAULT_MAX_BYTES: usize = agent_teams_core::READ_OUTPUT_DEFAULT_MAX_BYTES;
const HARD_MAX_BYTES: usize = agent_teams_core::READ_OUTPUT_HARD_MAX_BYTES;

/// Arguments for `team_read_output`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReadOutputArgs {
    /// The FULL pane id, e.g. `ws50144x0-p4` (matches `QueueRow.id` / the live
    /// registry). NOT a short `p4`, and NOT a filesystem path — the server resolves
    /// the source from this id.
    pub id: String,
    /// Max bytes of content to return (default 65536, clamped to 262144). When the
    /// artifact is larger, the NEWEST tail is returned and `truncated` is true.
    pub max_bytes: Option<u32>,
}

/// Output of `team_read_output`. Object-rooted (MCP `outputSchema` requires a root
/// object).
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct PaneOutputResult {
    /// The pane id that was requested (echoed back).
    pub id: String,
    /// The pane's harness from the live registry (`claude`/`cursor`/…), if known.
    pub harness: Option<String>,
    /// Where the content came from: `orchestrate_report` | `claude_transcript` |
    /// `cursor_transcript` | `live_scrollback` (a live in-memory tail from the running
    /// app — unverified, may be mid-stream) | `none`.
    pub source: String,
    /// Absolute path of the artifact read, if any.
    pub path: Option<String>,
    /// The content (a tail of at most `max_bytes`), if any was found.
    pub content: Option<String>,
    /// True when the artifact was larger than the cap and only its tail is returned.
    pub truncated: bool,
    /// Human-readable explanation when `source == "none"` (or the id is invalid) —
    /// tells the brain WHY there's nothing and what to do instead.
    pub note: Option<String>,
}

/// Resolve a pane id to its produced output, applying the safe-read contract.
/// Read-only and best-effort: any IO failure degrades to a `source:"none"` result
/// with a note rather than an error, so the brain always gets a usable answer.
pub fn resolve(state_dir: &Path, id: &str, max_bytes: Option<u32>) -> PaneOutputResult {
    let cap = (max_bytes.map(|m| m as usize).unwrap_or(DEFAULT_MAX_BYTES)).min(HARD_MAX_BYTES);

    if !validate_spawn_id(id) {
        return PaneOutputResult {
            id: id.to_string(),
            harness: None,
            source: "none".into(),
            path: None,
            content: None,
            truncated: false,
            note: Some(
                "invalid pane id — pass the FULL id like \"ws50144x0-p4\" (only \
                 [A-Za-z0-9_-]; never a filesystem path)"
                    .into(),
            ),
        };
    }

    let reg_row = registry_lookup(state_dir, id);
    // Liveness drives the gap-7 live-scrollback attempt below; only consumed on
    // phase-b builds (the base build compiles disk-only behavior).
    #[allow(unused_variables)]
    let pane_is_live = reg_row.is_some();
    let (harness, repo, session_id) = reg_row.unwrap_or((None, None, None));

    // 1) orchestrate/bridge report — harness-agnostic, the primary "what p4 produced".
    if let Some((path, body)) = newest_report_md(state_dir, repo.as_deref(), id) {
        let (content, truncated) = tail_to(&body, cap);
        audit_read(state_dir, id, content.len(), "orchestrate_report");
        return PaneOutputResult {
            id: id.to_string(),
            harness,
            source: "orchestrate_report".into(),
            path: Some(path),
            content: Some(content),
            truncated,
            note: None,
        };
    }

    // 2) harness transcript — the pane's own conversation. Precedence:
    //    2a) SESSION-ID locator (C1): when the registry recorded a claude session id for
    //        this pane, glob ALL `~/.claude/projects/*/<session_id>.jsonl` — cwd/encoding-
    //        independent. This is the STABLE locator: a pane launched with cwd `/` slugs its
    //        project dir to a bare `-`, which the id-suffix match below MISSES, wrongly
    //        returning source:"none" for a transcript that exists. Traversal-proof: the
    //        session id is REGISTRY-SOURCED (never caller-supplied) and re-validated via
    //        `validate_session_id` before it becomes a filename component.
    //    2b) fall back to the id-suffix locators — the pane id is embedded VERBATIM in the
    //        encoded project dir for BOTH claude (~/.claude/projects/*<id>) and cursor
    //        (~/.cursor/projects/*<id>/agent-transcripts). Tried regardless of the registry's
    //        harness label (robust to a missing/mislabeled harness); each returns None when
    //        absent (so this degrades gracefully when no session id was recorded).
    let transcript = session_id
        .as_deref()
        .filter(|s| validate_session_id(s))
        .and_then(newest_claude_transcript_by_session)
        .map(|t| ("claude_transcript", t))
        .or_else(|| newest_claude_transcript(id).map(|t| ("claude_transcript", t)))
        .or_else(|| newest_cursor_transcript(id).map(|t| ("cursor_transcript", t)));
    if let Some((source, (path, body))) = transcript {
        let text = extract_transcript_text(&body);
        let (content, truncated) = tail_to(&text, cap);
        audit_read(state_dir, id, content.len(), source);
        return PaneOutputResult {
            id: id.to_string(),
            harness,
            source: source.into(),
            path: Some(path),
            content: Some(content),
            truncated,
            note: None,
        };
    }

    // 3) LIVE SCROLLBACK (gap-7, phase-b builds only): every disk source missed, but the
    //    live registry says the pane EXISTS — ask the RUNNING app for a tail of the
    //    pane's IN-MEMORY PTY scrollback over the socket (`SocketRequest::ReadOutput`,
    //    admitted app-side by the coordinator/external-orchestrator gate). This is what
    //    makes state-blind harnesses (commandcode/codex/opencode/cline) readable at all.
    //    Any failure (app down / gate off / no buffer) falls through to the honest
    //    "none" with a note that the live read was attempted. The base (read-only)
    //    build compiles NONE of this — disk-only behavior, byte-for-byte.
    #[allow(unused_mut)]
    let mut live_note = "";
    #[cfg(feature = "phase-b-mutations")]
    if pane_is_live {
        match live_scrollback_via_socket(state_dir, id, cap) {
            Some((content, truncated)) => {
                audit_read(state_dir, id, content.len(), "live_scrollback");
                return PaneOutputResult {
                    id: id.to_string(),
                    harness,
                    source: "live_scrollback".into(),
                    path: None,
                    content: Some(content),
                    truncated,
                    note: Some(
                        "LIVE tail of the pane's in-memory scrollback, read from the \
                         running app — UNVERIFIED and possibly mid-stream (not a \
                         persisted report). For a durable artifact, ask the pane to \
                         write a report via team_orchestrate."
                            .into(),
                    ),
                };
            }
            None => {
                live_note = " A live scrollback read over the app socket was ALSO \
                             attempted and did not succeed (app not running, the \
                             external-read gate is off, or the pane buffer was \
                             unavailable).";
            }
        }
    }

    // 4) honest "none" — nothing on disk for this pane (NOT a permission/gate issue).
    let note = match harness.as_deref() {
        Some(h) => format!(
            "No report or transcript found on disk for {id} (harness={h}) — it may not have \
             produced output yet, or this harness keeps no on-disk transcript. Read it live in \
             the Agent Teams app, or ask the pane to write a report via team_orchestrate.\
             {live_note}"
        ),
        None => format!(
            "Pane {id} is not in the live registry and has no report/transcript on disk — \
             check the id (list panes with team_get_queue).{live_note}"
        ),
    };
    audit_read(state_dir, id, 0, "none");
    PaneOutputResult {
        id: id.to_string(),
        harness,
        source: "none".into(),
        path: None,
        content: None,
        truncated: false,
        note: Some(note),
    }
}

/// gap-7 (phase-b builds only): dial the running app for a LIVE tail of pane `id`'s
/// in-memory scrollback. Reuses the SAME sidecar→app transport `team_orchestrate`
/// rides (`phase_b::dial_selected`: UDS preferred, per-op timeout — `ReadOutput` is a
/// fast op; the verify-before-send HTTP fallback stays gated inside). `None` on ANY
/// failure — app down, gate refused (the app enforces the coordinator/external-
/// orchestrator admission), malformed reply — so the caller falls through to the
/// honest `source:"none"`. The pane id was already `validate_spawn_id`-checked by
/// [`resolve`]; `cap` was already clamped, and the app RE-clamps server-side.
#[cfg(feature = "phase-b-mutations")]
fn live_scrollback_via_socket(state_dir: &Path, id: &str, cap: usize) -> Option<(String, bool)> {
    use agent_teams_core::{socket_path, SocketData, SocketRequest};
    let sock = socket_path(state_dir)?;
    let req = SocketRequest::ReadOutput {
        id: id.to_string(),
        max_bytes: Some(cap as u64),
    };
    let resp = crate::phase_b::dial_selected(&sock, state_dir, &req, true).ok()?;
    if !resp.ok {
        return None;
    }
    match resp.data {
        Some(SocketData::Output { content, truncated }) => Some((content, truncated)),
        _ => None,
    }
}

/// Look up `(harness, repo, session_id)` for a pane id from the live registry.
/// `None` when the registry is absent/malformed or the id isn't live — the OUTER
/// Option IS the pane-liveness signal the gap-7 live-scrollback attempt keys on.
/// `session_id` is the pane's stable claude conversation id the app recorded at spawn
/// (C1) — it is the transcript FILENAME, so [`resolve`] can locate the transcript by
/// session id regardless of the launch-cwd encoding of the project dir. It is
/// REGISTRY-SOURCED (never caller-supplied), preserving the traversal-proof contract.
#[allow(clippy::type_complexity)]
fn registry_lookup(
    state_dir: &Path,
    id: &str,
) -> Option<(Option<String>, Option<String>, Option<String>)> {
    let reg = read_registry(state_dir)?;
    let ws = reg.workspaces.iter().find(|w| w.id == id)?;
    Some((ws.harness.clone(), ws.repo.clone(), ws.session_id.clone()))
}

/// Find the newest `<id>.md` report across the orchestrate/bridge run dirs. The
/// orchestrate dir is the hardcoded single segment `agent-teams-orchestrate` directly
/// under the state root's parent (NOT dev/prod-suffixed); bridge runs live under
/// `agent-teams-bridge/` and, when a repo is known, `<repo>/bridge/`.
fn newest_report_md(state_dir: &Path, repo: Option<&str>, id: &str) -> Option<(String, String)> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(parent) = state_dir.parent() {
        roots.push(parent.join("agent-teams-orchestrate"));
        roots.push(parent.join("agent-teams-bridge"));
    }
    if let Some(r) = repo {
        roots.push(Path::new(r).join("bridge"));
    }

    let fname = format!("{id}.md");
    let mut best: Option<(PathBuf, SystemTime)> = None;
    for root in roots {
        let run_dirs = match fs::read_dir(&root) {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        for run in run_dirs.flatten() {
            let candidate = run.path().join(&fname);
            if let Ok(md) = fs::metadata(&candidate) {
                if md.is_file() && md.len() > 0 {
                    let mt = md.modified().unwrap_or(UNIX_EPOCH);
                    if best.as_ref().is_none_or(|(_, bt)| mt > *bt) {
                        best = Some((candidate, mt));
                    }
                }
            }
        }
    }

    let (path, _) = best?;
    let body = fs::read_to_string(&path).ok()?;
    Some((path.to_string_lossy().into_owned(), body))
}

/// Locate the newest Claude transcript JSONL for a pane: the encoded worktree dir
/// under `~/.claude/projects/` ends with the pane id (the worktree cwd is
/// `<repo>/.agent-teams-worktrees/<id>`, and the encoder replaces `/`+`.` with `-`).
/// Matching on the suffix avoids `p4` colliding with `p40`.
fn newest_claude_transcript(id: &str) -> Option<(String, String)> {
    let home = std::env::var_os("HOME")?;
    let projects = Path::new(&home).join(".claude").join("projects");
    let entries = fs::read_dir(&projects).ok()?;

    let mut best: Option<(PathBuf, SystemTime)> = None;
    for proj in entries.flatten() {
        let pname = proj.file_name();
        if !pname.to_string_lossy().ends_with(id) {
            continue;
        }
        let jsonls = match fs::read_dir(proj.path()) {
            Ok(j) => j,
            Err(_) => continue,
        };
        for f in jsonls.flatten() {
            let fp = f.path();
            if fp.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            if let Ok(md) = f.metadata() {
                let mt = md.modified().unwrap_or(UNIX_EPOCH);
                if best.as_ref().is_none_or(|(_, bt)| mt > *bt) {
                    best = Some((fp, mt));
                }
            }
        }
    }

    let (path, _) = best?;
    let body = fs::read_to_string(&path).ok()?;
    Some((path.to_string_lossy().into_owned(), body))
}

/// Locate the newest Claude transcript for a KNOWN session id by globbing ALL project
/// dirs under `~/.claude/projects/` for `<session_id>.jsonl` — cwd/encoding-independent
/// (mirrors the app-side `claude_session_exists_in` precedent). The project dir NAME
/// depends on the launch cwd (the CLI slugs it), so a pane launched at cwd `/` lands its
/// transcript under a bare `-` dir that the id-suffix match can never find; the session
/// FILENAME is stable, so this finds it. Returns the newest match by mtime with its body.
///
/// SAFETY: `session_id` MUST be pre-validated (it is registry-sourced, never caller-
/// supplied, and [`resolve`] gates it through `validate_session_id` before calling this),
/// because it is used VERBATIM as a filename component.
fn newest_claude_transcript_by_session(session_id: &str) -> Option<(String, String)> {
    let home = std::env::var_os("HOME")?;
    let projects = Path::new(&home).join(".claude").join("projects");
    newest_claude_transcript_by_session_in(&projects, session_id)
}

/// Testable core of [`newest_claude_transcript_by_session`]: glob `<projects>/*/<session_id>.jsonl`
/// (encoding-independent) and return the newest by mtime with its body. Split from the
/// `HOME`-reading wrapper so a temp `projects` root can be exercised without touching env.
fn newest_claude_transcript_by_session_in(
    projects: &Path,
    session_id: &str,
) -> Option<(String, String)> {
    let file = format!("{session_id}.jsonl");
    let entries = fs::read_dir(projects).ok()?;

    let mut best: Option<(PathBuf, SystemTime)> = None;
    for proj in entries.flatten() {
        let candidate = proj.path().join(&file);
        if let Ok(md) = fs::metadata(&candidate) {
            if md.is_file() && md.len() > 0 {
                let mt = md.modified().unwrap_or(UNIX_EPOCH);
                if best.as_ref().is_none_or(|(_, bt)| mt > *bt) {
                    best = Some((candidate, mt));
                }
            }
        }
    }

    let (path, _) = best?;
    let body = fs::read_to_string(&path).ok()?;
    Some((path.to_string_lossy().into_owned(), body))
}

/// Locate the newest Cursor agent transcript for a pane. Under `~/.cursor/projects/`, the
/// project dir is the worktree cwd slugified and ENDS with the pane id; the transcript lives
/// at `<proj>/agent-transcripts/<session>/<session>.jsonl`. Suffix match on the dir name
/// (avoids `p4` vs `p40`). This is the CLEAN agent-transcripts JSONL — NOT the
/// `~/.cursor/chats/<md5(cwd)>/store.db` protobuf store (md5-keyed, drifts, often absent).
fn newest_cursor_transcript(id: &str) -> Option<(String, String)> {
    let home = std::env::var_os("HOME")?;
    let projects = Path::new(&home).join(".cursor").join("projects");
    let entries = fs::read_dir(&projects).ok()?;

    let mut best: Option<(PathBuf, SystemTime)> = None;
    for proj in entries.flatten() {
        if !proj.file_name().to_string_lossy().ends_with(id) {
            continue;
        }
        let sessions = match fs::read_dir(proj.path().join("agent-transcripts")) {
            Ok(s) => s,
            Err(_) => continue,
        };
        for session in sessions.flatten() {
            let files = match fs::read_dir(session.path()) {
                Ok(f) => f,
                Err(_) => continue,
            };
            for f in files.flatten() {
                let fp = f.path();
                if fp.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                if let Ok(md) = f.metadata() {
                    let mt = md.modified().unwrap_or(UNIX_EPOCH);
                    if best.as_ref().is_none_or(|(_, bt)| mt > *bt) {
                        best = Some((fp, mt));
                    }
                }
            }
        }
    }

    let (path, _) = best?;
    let body = fs::read_to_string(&path).ok()?;
    Some((path.to_string_lossy().into_owned(), body))
}

/// Render a harness transcript JSONL (claude OR cursor) into readable `[role] text`
/// lines, keeping only user/assistant text blocks (drops tool-call/result noise). Both
/// stores share the `{role-or-type, message.content[]}` shape — claude carries the role in
/// `type`, cursor in `role`. Defensive: any line that doesn't parse is skipped.
fn extract_transcript_text(jsonl: &str) -> String {
    let mut out = String::new();
    for line in jsonl.lines() {
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let role = v
            .get("role")
            .or_else(|| v.get("type"))
            .and_then(|t| t.as_str())
            .unwrap_or("");
        if role != "user" && role != "assistant" {
            continue;
        }
        let text = match v.pointer("/message/content") {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(serde_json::Value::Array(arr)) => arr
                .iter()
                .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()).map(str::to_string))
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        };
        if !text.trim().is_empty() {
            out.push('[');
            out.push_str(role);
            out.push_str("] ");
            out.push_str(text.trim());
            out.push('\n');
        }
    }
    out
}

/// Keep at most `cap` bytes from the END (most recent) of `s`, aligned to a char
/// boundary. Returns `(tail, truncated)`.
fn tail_to(s: &str, cap: usize) -> (String, bool) {
    if s.len() <= cap {
        return (s.to_string(), false);
    }
    let mut start = s.len() - cap;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    (s[start..].to_string(), true)
}

/// Append a best-effort read-audit line (id + size + source, NEVER the content) to
/// `agent-teams-external-reads.jsonl` beside the state root. A failed write never
/// blocks the read.
fn audit_read(state_dir: &Path, id: &str, bytes: usize, source: &str) {
    let _ = (|| -> Option<()> {
        let parent = state_dir.parent()?;
        let path = parent.join("agent-teams-external-reads.jsonl");
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_millis() as u64;
        let line = serde_json::json!({
            "ts": ts,
            "op": "read_output",
            "id": id,
            "source": source,
            "bytes": bytes,
        })
        .to_string();
        use std::io::Write;
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok()?;
        f.write_all(line.as_bytes()).ok()?;
        f.write_all(b"\n").ok()?;
        Some(())
    })();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_root(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "at-mcp-readout-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ))
    }

    #[test]
    fn invalid_id_is_rejected_without_io() {
        let state = unique_root("badid").join("state");
        for bad in ["../escape", "/etc/passwd", "ws..-p0", "a b", ""] {
            let r = resolve(&state, bad, None);
            assert_eq!(r.source, "none", "{bad:?} must not resolve");
            assert!(r.content.is_none());
            assert!(
                r.note.as_deref().unwrap_or("").contains("invalid pane id"),
                "{bad:?} → note should flag the id: {:?}",
                r.note
            );
        }
    }

    #[test]
    fn tail_to_caps_on_char_boundary() {
        let (whole, trunc) = tail_to("hello", 10);
        assert_eq!(whole, "hello");
        assert!(!trunc, "under cap → not truncated");

        let (tail, trunc) = tail_to("abcdefghij", 4);
        assert_eq!(tail, "ghij", "keeps the newest 4 bytes");
        assert!(trunc);

        // Multibyte: 'é' is 2 bytes; a cap that would split it slides forward to a boundary.
        let s = "aébc"; // bytes: a(1) é(2) b(1) c(1) = 5
        let (tail, trunc) = tail_to(s, 3);
        assert!(trunc);
        assert!(s.ends_with(&tail), "tail is a valid suffix: {tail:?}");
        assert!(tail.is_char_boundary(0));
    }

    #[test]
    fn extract_transcript_text_handles_both_claude_and_cursor_shapes() {
        // Claude: role in `type`. Cursor: role in `role`. Both: message.content[] text blocks.
        let jsonl = [
            // claude-shape
            r#"{"type":"user","message":{"content":"hi there"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hello"},{"type":"tool_use","name":"Read"}]}}"#,
            r#"{"type":"summary","message":{"content":[{"type":"text","text":"IGNORED"}]}}"#,
            // cursor-shape (top-level role)
            r#"{"role":"assistant","message":{"content":[{"type":"text","text":"I am Auto in pane p5"}]}}"#,
            r#"{"role":"tool","message":{"content":[{"type":"text","text":"TOOLNOISE"}]}}"#,
            "not json at all",
        ]
        .join("\n");
        let out = extract_transcript_text(&jsonl);
        assert!(out.contains("[user] hi there"));
        assert!(out.contains("[assistant] hello"));
        assert!(
            out.contains("[assistant] I am Auto in pane p5"),
            "cursor role shape parsed"
        );
        assert!(!out.contains("IGNORED"), "non user/assistant roles dropped");
        assert!(!out.contains("tool_use"), "tool blocks dropped");
        assert!(!out.contains("TOOLNOISE"), "tool role dropped");
    }

    #[test]
    fn resolve_finds_newest_orchestrate_report() {
        let root = unique_root("report");
        let state = root.join("state");
        fs::create_dir_all(&state).unwrap();
        let id = "ws9-p4";

        // older report
        let old = root.join("agent-teams-orchestrate").join("run-1");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join(format!("{id}.md")), "OLD report").unwrap();
        // newer report (written second → newer mtime)
        let new = root.join("agent-teams-orchestrate").join("run-2");
        fs::create_dir_all(&new).unwrap();
        fs::write(
            new.join(format!("{id}.md")),
            "# Coordinator Status\nNEW report body",
        )
        .unwrap();

        let r = resolve(&state, id, None);
        assert_eq!(r.source, "orchestrate_report");
        assert!(r.content.as_deref().unwrap().contains("NEW report body"));
        assert!(
            r.path.as_deref().unwrap().contains("run-2"),
            "newest run wins"
        );
        assert!(!r.truncated);

        // the read was audited (id + source, not content)
        let audit = fs::read_to_string(root.join("agent-teams-external-reads.jsonl")).unwrap();
        assert!(audit.contains("\"op\":\"read_output\""));
        assert!(audit.contains(id));
        assert!(
            !audit.contains("NEW report body"),
            "audit must NOT carry content"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn session_id_locator_finds_transcript_when_dir_does_not_end_with_pane_id() {
        // C1 regression: a pane launched with cwd `/` makes claude slug its project dir to a
        // bare `-`, so `"-".ends_with("ws76101x0-p0")` is FALSE — the id-suffix match misses a
        // transcript that DOES exist and returns a false source:"none". The session-id locator
        // globs ALL project dirs by the stable session FILENAME, so it finds it regardless of
        // the dir name.
        let root = unique_root("sess");
        let projects = root.join("projects");
        // The cwd=`/` case: the encoded project dir is a bare "-", NOT ending with the pane id.
        let dash_dir = projects.join("-");
        fs::create_dir_all(&dash_dir).unwrap();
        let session = "3f2504e0-4f89-41d3-9a0c-0305e82c3301";
        let pane_id = "ws76101x0-p0";
        fs::write(
            dash_dir.join(format!("{session}.jsonl")),
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"from cwd=/ pane"}]}}"#,
        )
        .unwrap();

        // Premise: the id-suffix locator's match (dir must END WITH the pane id) misses "-".
        assert!(
            !"-".ends_with(pane_id),
            "bare dash dir never ends with the pane id"
        );
        // And a validated uuid is the precondition resolve() gates on before calling this.
        assert!(validate_session_id(session));

        // The session-id locator finds it by filename, encoding-independent.
        let found = newest_claude_transcript_by_session_in(&projects, session)
            .expect("session-id locator finds the transcript under a bare `-` project dir");
        assert!(
            found.0.ends_with(&format!("{session}.jsonl")),
            "returns the transcript path: {}",
            found.0
        );
        assert!(
            found.1.contains("from cwd=/ pane"),
            "returns the transcript body"
        );

        // An unknown session id (no such file) → None, never a panic.
        assert!(newest_claude_transcript_by_session_in(&projects, "no-such-session").is_none());

        let _ = fs::remove_dir_all(&root);
    }

    /// gap-7: every disk source misses but the pane is LIVE in the registry → resolve()
    /// dials the app socket (the SAME transport team_orchestrate rides) and serves the
    /// app's in-memory tail as `source:"live_scrollback"`. Mirrors the live-listener
    /// pattern of `socket.rs::dial_round_trips_live_replies_and_classifies_transport_errors`.
    #[cfg(feature = "phase-b-mutations")]
    #[test]
    fn resolve_serves_live_scrollback_when_disk_misses_and_app_answers() {
        use agent_teams_core::{SocketData, SocketRequest, SocketResponse};
        use std::io::{BufRead, Write};

        // A SHORT root ("/tmp", not env::temp_dir()'s long /var/folders/… path): the
        // socket path must fit macOS's 104-byte sockaddr_un SUN_LEN limit.
        let root = PathBuf::from("/tmp").join(format!(
            "at-ro-live-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        let state = root.join("state");
        fs::create_dir_all(&state).unwrap();
        let id = "ws9-p3";

        // Live registry: a commandcode pane (state-blind — never writes a disk transcript).
        let reg = serde_json::json!({
            "schema": 1,
            "workspaces": [ { "id": id, "harness": "commandcode", "repo": "/tmp/none" } ]
        });
        fs::write(root.join("agent-teams-live.json"), reg.to_string()).unwrap();

        // A mini "app" on the REAL socket path. `dial_selected` PROBES with a bare
        // connect first (transport selector), so loop: a connection with no request
        // line (EOF) is the probe; the one carrying a line gets the reply.
        let sock = agent_teams_core::socket_path(&state).unwrap();
        let _ = fs::remove_file(&sock);
        let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        let server = std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut stream) = conn else { break };
                let mut r = std::io::BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if r.read_line(&mut line).unwrap_or(0) == 0 {
                    continue; // the transport-selector probe: connect-and-close
                }
                let req: SocketRequest = serde_json::from_str(line.trim_end()).unwrap();
                assert!(
                    matches!(&req, SocketRequest::ReadOutput { id: rid, max_bytes: Some(_) } if rid == "ws9-p3"),
                    "app must receive ReadOutput for the pane: {req:?}"
                );
                let mut out = serde_json::to_string(
                    &SocketResponse::ok("live scrollback tail").with_data(SocketData::Output {
                        content: "LIVE TAIL $ build ok".into(),
                        truncated: true,
                    }),
                )
                .unwrap();
                out.push('\n');
                let _ = stream.write_all(out.as_bytes());
                let _ = stream.flush();
                break;
            }
        });

        let r = resolve(&state, id, None);
        assert_eq!(r.source, "live_scrollback");
        assert_eq!(r.harness.as_deref(), Some("commandcode"));
        assert_eq!(r.content.as_deref(), Some("LIVE TAIL $ build ok"));
        assert!(r.truncated, "the app's truncated flag passes through");
        assert!(r.path.is_none(), "a live tail has no artifact path");
        let note = r.note.as_deref().unwrap_or("");
        assert!(
            note.contains("UNVERIFIED"),
            "note flags the live tail: {note}"
        );
        assert!(
            note.contains("mid-stream"),
            "note warns it may be mid-stream: {note}"
        );

        // The read was audited (id + source, NEVER content).
        let audit = fs::read_to_string(root.join("agent-teams-external-reads.jsonl")).unwrap();
        assert!(audit.contains("\"source\":\"live_scrollback\""));
        assert!(audit.contains(id));
        assert!(!audit.contains("LIVE TAIL"), "audit must NOT carry content");

        server.join().unwrap();
        let _ = fs::remove_dir_all(&root);
    }

    /// gap-7: a LIVE pane with the app DOWN falls through to the honest `none`, noting
    /// the attempted live read; a NON-live pane must NOT claim an attempt was made.
    #[cfg(feature = "phase-b-mutations")]
    #[test]
    fn resolve_notes_attempted_live_read_when_app_down() {
        let root = unique_root("livedown");
        let state = root.join("state");
        fs::create_dir_all(&state).unwrap();
        let id = "ws9-p7";
        let reg = serde_json::json!({
            "schema": 1,
            "workspaces": [ { "id": id, "harness": "opencode", "repo": "/tmp/none" } ]
        });
        fs::write(root.join("agent-teams-live.json"), reg.to_string()).unwrap();

        // No socket bound → the dial fails fast → honest none + the attempted-read note.
        let r = resolve(&state, id, None);
        assert_eq!(r.source, "none");
        assert!(r.content.is_none());
        let note = r.note.as_deref().unwrap_or("");
        assert!(
            note.contains("live scrollback read"),
            "notes the attempt: {note}"
        );

        // A pane NOT in the live registry: no live read is attempted (or claimed).
        let r2 = resolve(&state, "ws9-p8", None);
        assert_eq!(r2.source, "none");
        let note2 = r2.note.as_deref().unwrap_or("");
        assert!(
            !note2.contains("live scrollback read"),
            "must not claim an attempt for a non-live pane: {note2}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn resolve_honest_none_for_state_blind_harness() {
        let root = unique_root("blind");
        let state = root.join("state");
        fs::create_dir_all(&state).unwrap();
        let id = "ws9-p2";

        // a live registry that says this pane is a cursor pane, but no report on disk
        let reg = serde_json::json!({
            "schema": 1,
            "workspaces": [ { "id": id, "harness": "cursor", "repo": "/tmp/none" } ]
        });
        fs::write(root.join("agent-teams-live.json"), reg.to_string()).unwrap();

        let r = resolve(&state, id, None);
        assert_eq!(r.source, "none");
        assert_eq!(r.harness.as_deref(), Some("cursor"));
        assert!(r.content.is_none());
        let note = r.note.unwrap();
        assert!(note.contains("cursor"), "names the harness: {note}");
        assert!(
            note.contains("team_orchestrate"),
            "tells the brain what to do: {note}"
        );

        let _ = fs::remove_dir_all(&root);
    }
}
