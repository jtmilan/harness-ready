//! `team_audit_log` — read back the append-only audit ledgers of EXTERNAL
//! orchestrator actions, the missing "what did I actually dispatch?" primitive.
//! Always-on (registered in the base read router, like `team_read_output`): pure,
//! ungated local file I/O over ledgers this codebase already writes on disk.
//!
//! ## Why this exists
//! The external brain dispatches through the sidecar (send/broadcast/orchestrate/
//! spawn) and reads through `team_read_output` — and BOTH paths append an audit
//! line on disk. But neither ledger was exposed over MCP, so when asked "what did
//! you send to the panes?" the brain could only narrate from its own conversation
//! memory — a fabrication risk. This tool closes the loop: the ledgers on disk are
//! the ground truth of what was actually dispatched/read, newest first.
//!
//! ## The two ledgers (both SIBLINGS of the state root, mirroring the live
//! registry / socket-file placement; see the writers)
//! - `agent-teams-external-mutations.jsonl` — written APP-side by
//!   `audit_external_mutation` (app/src-tauri/src/lib.rs) for every externally
//!   driven mutation: send_input / broadcast / orchestrate_preview /
//!   orchestrate_dispatch / focus / create_workspace / add_pane. Rows:
//!   `{ts, source, peer_pid, op, target, text, details}` (`text` is a ≤200-char
//!   snippet, never full content).
//! - `agent-teams-external-reads.jsonl` — written SIDECAR-side by
//!   `read_output::audit_read` for every `team_read_output` call. Rows:
//!   `{ts, op, id, source, bytes}` (sizes only, NEVER the content).
//!
//! ## Contract
//! - **Read-only, path-fixed.** The caller passes NO path — the server resolves
//!   both ledgers from the state root, exactly like the writers do.
//! - **Newest first.** Rows are merged across ledgers and sorted by `ts`
//!   descending (append order breaks ties), `limit` default 20, hard cap 200.
//! - **Best-effort, never all-or-nothing.** A malformed line is skipped and
//!   counted in `skipped`; a missing ledger yields an empty list plus a `note`,
//!   never an error — the brain always gets a usable answer.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Default + hard cap on returned rows (the newest are kept).
const DEFAULT_LIMIT: usize = 20;
const HARD_LIMIT: usize = 200;

/// Filename of the app-side external-mutation ledger (sibling of the state root;
/// see `audit_external_mutation` in app/src-tauri/src/lib.rs).
const MUTATIONS_LEDGER: &str = "agent-teams-external-mutations.jsonl";
/// Filename of the sidecar-side external-read ledger (sibling of the state root;
/// see `read_output::audit_read`).
const READS_LEDGER: &str = "agent-teams-external-reads.jsonl";

/// Arguments for `team_audit_log`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AuditLogArgs {
    /// Max rows to return (default 20, clamped to 200). The NEWEST rows are kept.
    pub limit: Option<u32>,
    /// Which ledger to read: `"mutations"` (externally driven sends/broadcasts/
    /// orchestrations/spawns), `"reads"` (team_read_output calls), or omit for both.
    pub kind: Option<String>,
}

/// One audit row, tagged with the ledger it came from.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct AuditEntry {
    /// Which ledger this row came from: `"mutations"` | `"reads"`.
    pub ledger: String,
    /// The row as written (parsed JSON): mutations carry
    /// `{ts, source, peer_pid, op, target, text, details}`; reads carry
    /// `{ts, op, id, source, bytes}`.
    pub row: serde_json::Value,
}

/// Output of `team_audit_log`. Object-rooted (MCP `outputSchema` requires a root
/// object).
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct AuditLogResult {
    /// Audit rows, NEWEST first (merged across ledgers, sorted by `ts` descending).
    pub entries: Vec<AuditEntry>,
    /// Count of malformed ledger lines that were skipped (never fails the call).
    pub skipped: u32,
    /// Human-readable explanation when a ledger is missing / the kind is unknown —
    /// tells the brain WHY the list is short, not an error.
    pub note: Option<String>,
}

/// Resolve the audit trail: read the requested ledger(s) beside the state root,
/// merge, and return the newest `limit` rows. Read-only and best-effort: missing
/// files and malformed lines degrade to `note`/`skipped`, never an error.
pub fn resolve(state_dir: &Path, limit: Option<u32>, kind: Option<&str>) -> AuditLogResult {
    let cap = (limit.map(|l| l as usize).unwrap_or(DEFAULT_LIMIT)).min(HARD_LIMIT);

    // Which ledgers to read. An unknown kind reads NOTHING (with a note) rather
    // than guessing — the honest degradation this surface promises.
    let (want_mutations, want_reads) = match kind {
        None | Some("both") => (true, true),
        Some("mutations") => (true, false),
        Some("reads") => (false, true),
        Some(other) => {
            return AuditLogResult {
                entries: Vec::new(),
                skipped: 0,
                note: Some(format!(
                    "unknown kind {other:?} — pass \"mutations\", \"reads\", or omit for both"
                )),
            };
        }
    };

    let Some(parent) = state_dir.parent() else {
        return AuditLogResult {
            entries: Vec::new(),
            skipped: 0,
            note: Some(
                "state root has no parent directory — cannot resolve the audit ledgers \
                 (they live as siblings of the state root)"
                    .into(),
            ),
        };
    };

    // (ts, append-seq, entry) — seq preserves append order as the tie-break so
    // "newest first" holds even for same-millisecond rows.
    let mut rows: Vec<(u64, usize, AuditEntry)> = Vec::new();
    let mut skipped: u32 = 0;
    let mut notes: Vec<String> = Vec::new();
    let mut seq: usize = 0;

    let mut read_ledger = |file: &str, tag: &str| {
        let path = parent.join(file);
        let body = match fs::read_to_string(&path) {
            Ok(b) => b,
            Err(_) => {
                notes.push(format!(
                    "no {tag} ledger at {} (nothing audited yet)",
                    path.display()
                ));
                return;
            }
        };
        for line in body.lines() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<serde_json::Value>(line) {
                Ok(row) => {
                    let ts = row.get("ts").and_then(|v| v.as_u64()).unwrap_or(0);
                    rows.push((
                        ts,
                        seq,
                        AuditEntry {
                            ledger: tag.to_string(),
                            row,
                        },
                    ));
                    seq += 1;
                }
                Err(_) => skipped += 1,
            }
        }
    };

    if want_mutations {
        read_ledger(MUTATIONS_LEDGER, "mutations");
    }
    if want_reads {
        read_ledger(READS_LEDGER, "reads");
    }

    // Newest first: ts descending, append order descending as the tie-break.
    rows.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    rows.truncate(cap);

    AuditLogResult {
        entries: rows.into_iter().map(|(_, _, e)| e).collect(),
        skipped,
        note: if notes.is_empty() {
            None
        } else {
            Some(notes.join("; "))
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn unique_root(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "at-mcp-auditlog-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ))
    }

    /// Make `<root>/state` plus the two sibling ledgers with the given lines.
    fn scaffold(root: &Path, mutations: &[&str], reads: &[&str]) -> PathBuf {
        let state = root.join("state");
        fs::create_dir_all(&state).unwrap();
        if !mutations.is_empty() {
            fs::write(root.join(MUTATIONS_LEDGER), mutations.join("\n") + "\n").unwrap();
        }
        if !reads.is_empty() {
            fs::write(root.join(READS_LEDGER), reads.join("\n") + "\n").unwrap();
        }
        state
    }

    fn ts_of(e: &AuditEntry) -> u64 {
        e.row.get("ts").and_then(|v| v.as_u64()).unwrap_or(0)
    }

    #[test]
    fn newest_first_across_both_ledgers() {
        let root = unique_root("newest");
        let state = scaffold(
            &root,
            &[
                r#"{"ts":100,"op":"broadcast","text":"a"}"#,
                r#"{"ts":300,"op":"orchestrate_dispatch","target":"ws9"}"#,
            ],
            &[r#"{"ts":200,"op":"read_output","id":"ws9-p4","bytes":42}"#],
        );

        let r = resolve(&state, None, None);
        assert_eq!(r.skipped, 0);
        assert!(
            r.note.is_none(),
            "both ledgers present → no note: {:?}",
            r.note
        );
        let ts: Vec<u64> = r.entries.iter().map(ts_of).collect();
        assert_eq!(ts, vec![300, 200, 100], "merged newest-first");
        assert_eq!(r.entries[0].ledger, "mutations");
        assert_eq!(r.entries[1].ledger, "reads");
        // rows pass through as parsed JSON
        assert_eq!(
            r.entries[1].row.get("id").and_then(|v| v.as_str()),
            Some("ws9-p4")
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn limit_defaults_to_20_and_hard_caps_at_200() {
        let root = unique_root("limit");
        let lines: Vec<String> = (0..250)
            .map(|i| format!(r#"{{"ts":{i},"op":"broadcast"}}"#))
            .collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let state = scaffold(&root, &refs, &[]);

        // default 20, newest first (highest ts)
        let r = resolve(&state, None, None);
        assert_eq!(r.entries.len(), 20);
        assert_eq!(ts_of(&r.entries[0]), 249);
        assert_eq!(ts_of(&r.entries[19]), 230);

        // an oversized limit is clamped to the hard cap
        let r = resolve(&state, Some(5000), None);
        assert_eq!(r.entries.len(), 200, "hard cap 200");
        assert_eq!(ts_of(&r.entries[0]), 249);
        assert_eq!(ts_of(&r.entries[199]), 50);

        // a small explicit limit is honored
        let r = resolve(&state, Some(3), None);
        assert_eq!(r.entries.len(), 3);
        assert_eq!(ts_of(&r.entries[2]), 247);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn kind_filters_to_one_ledger() {
        let root = unique_root("kind");
        let state = scaffold(
            &root,
            &[r#"{"ts":1,"op":"send_input","target":"ws9-p0"}"#],
            &[r#"{"ts":2,"op":"read_output","id":"ws9-p0","bytes":9}"#],
        );

        let m = resolve(&state, None, Some("mutations"));
        assert_eq!(m.entries.len(), 1);
        assert_eq!(m.entries[0].ledger, "mutations");
        assert!(m.note.is_none(), "requested ledger exists → no note");

        let r = resolve(&state, None, Some("reads"));
        assert_eq!(r.entries.len(), 1);
        assert_eq!(r.entries[0].ledger, "reads");

        let both = resolve(&state, None, Some("both"));
        assert_eq!(both.entries.len(), 2);

        // an unknown kind reads nothing and says why, never errors
        let bad = resolve(&state, None, Some("writes"));
        assert!(bad.entries.is_empty());
        assert!(
            bad.note.as_deref().unwrap_or("").contains("unknown kind"),
            "note names the problem: {:?}",
            bad.note
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn malformed_lines_are_skipped_and_counted_not_fatal() {
        let root = unique_root("malformed");
        let state = scaffold(
            &root,
            &[
                r#"{"ts":10,"op":"focus","target":"ws9"}"#,
                "not json at all",
                r#"{"ts":30,"op":"broadcast""#, // truncated line (torn write)
                r#"{"ts":20,"op":"broadcast","text":"ok"}"#,
            ],
            &[],
        );

        let r = resolve(&state, None, Some("mutations"));
        assert_eq!(r.skipped, 2, "two malformed lines counted");
        let ts: Vec<u64> = r.entries.iter().map(ts_of).collect();
        assert_eq!(ts, vec![20, 10], "good rows survive, newest first");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_ledgers_yield_empty_list_plus_note_never_error() {
        let root = unique_root("missing");
        let state = root.join("state");
        fs::create_dir_all(&state).unwrap();

        // both absent → empty + a note per missing ledger
        let r = resolve(&state, None, None);
        assert!(r.entries.is_empty());
        assert_eq!(r.skipped, 0);
        let note = r.note.expect("missing ledgers must be explained");
        assert!(
            note.contains("mutations"),
            "names the mutations ledger: {note}"
        );
        assert!(note.contains("reads"), "names the reads ledger: {note}");

        // one present, one absent → its rows + a note for the absent one only
        fs::write(
            root.join(READS_LEDGER),
            "{\"ts\":5,\"op\":\"read_output\"}\n",
        )
        .unwrap();
        let r = resolve(&state, None, None);
        assert_eq!(r.entries.len(), 1);
        assert_eq!(r.entries[0].ledger, "reads");
        let note = r.note.expect("the absent mutations ledger is still noted");
        assert!(note.contains("mutations"));
        assert!(
            !note.contains("no reads ledger"),
            "present ledger not noted: {note}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn same_ts_ties_break_by_append_order_newest_first() {
        let root = unique_root("ties");
        let state = scaffold(
            &root,
            &[
                r#"{"ts":7,"op":"broadcast","text":"first"}"#,
                r#"{"ts":7,"op":"broadcast","text":"second"}"#,
            ],
            &[],
        );

        let r = resolve(&state, None, Some("mutations"));
        let texts: Vec<&str> = r
            .entries
            .iter()
            .filter_map(|e| e.row.get("text").and_then(|v| v.as_str()))
            .collect();
        assert_eq!(texts, vec!["second", "first"], "later append wins the tie");

        let _ = fs::remove_dir_all(&root);
    }
}
