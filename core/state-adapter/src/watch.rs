//! events.jsonl reader + workspace discovery (Plan 01-01, Task 4).
//!
//! Reads the latest event line per workspace, normalises it, and returns the
//! ranked "who needs me" list. std-only: a minimal field extractor parses our
//! own fixed JSONL shape (written by `state-writer.sh`), so no serde/notify
//! dependency — the crate stays offline-buildable. The CLI ([`bin/adapter`])
//! polls these functions; Phase 02 can swap polling for the `notify` crate.
//!
//! **Read-path cost (perf 2026-06-10):** [`latest`] is on a ~1 s poll cadence
//! from THREE callers (app `list_queue`, the backend HUD timer, the MCP
//! sidecar) and events.jsonl grows without bound (a live pane measured 9.1 MB).
//! So the read path is a process-global **(mtime, len)-gated tail cache**:
//! steady state costs one `stat` per file and zero read bytes; a changed file
//! costs a bounded ≤[`TAIL_BYTES`] seek+read instead of a whole-file
//! `read_to_string`. The cache is transparent — `latest`'s signature (and
//! through it `current_states` → core/mcp `compute_queue`) is unchanged, so
//! every caller gets the win with zero edits.

use crate::{normalize, rank, AgentState, Decision, Harness, RawEvent};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::SystemTime;

pub struct Workspace {
    pub id: String,
    pub events_path: PathBuf,
}

/// Find workspaces under a state dir: each `<state_dir>/<id>/events.jsonl`.
pub fn discover(state_dir: &Path) -> Vec<Workspace> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(state_dir) {
        for entry in rd.flatten() {
            let dir = entry.path();
            let events = dir.join("events.jsonl");
            if events.exists() {
                if let Some(id) = dir.file_name().and_then(|s| s.to_str()) {
                    out.push(Workspace {
                        id: id.to_string(),
                        events_path: events,
                    });
                }
            }
        }
    }
    out
}

/// Reference implementation: whole-file read. Kept ONLY as the oracle for the
/// differential tests against [`last_nonempty_line_tail`] — production reads go
/// through the bounded tail path (a live events.jsonl measured 9.1 MB; reading
/// it whole 3×/s was the perf bug this module's cache exists to fix).
#[cfg_attr(not(test), allow(dead_code))]
fn last_nonempty_line(path: &Path) -> Option<String> {
    let body = fs::read_to_string(path).ok()?;
    body.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.to_string())
}

/// Upper bound on bytes read per changed file. Comfortably holds many lines:
/// post-truncation writer lines are ≤ ~0.7 KB; even the legacy full-payload
/// lines measured ~22 KB. A single line longer than this window is unparseable
/// for that tick → `None` (same drop-one-tick semantics as a torn line; the
/// row recovers on the next appended event).
const TAIL_BYTES: u64 = 64 * 1024;

/// Bounded tail read: the last non-empty COMPLETE line without reading the
/// whole file. Seeks to `len - TAIL_BYTES` and, when that lands mid-file, the
/// first (possibly partial) line fragment after the seek point is dropped —
/// scanning starts at the first `'\n'` — so a mid-line (or mid-UTF-8-codepoint)
/// landing can never surface a torn fragment as a "line".
fn last_nonempty_line_tail(path: &Path) -> Option<String> {
    let mut f = fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    if start > 0 {
        f.seek(SeekFrom::Start(start)).ok()?;
    }
    let mut buf = Vec::with_capacity((len - start) as usize);
    f.read_to_end(&mut buf).ok()?;
    let mut window: &[u8] = &buf;
    if start > 0 {
        // Drop everything up to AND INCLUDING the first newline: the seek can
        // land anywhere, so only what follows a '\n' is a complete line. No
        // newline in the whole window ⇒ one giant partial line ⇒ None.
        match window.iter().position(|&b| b == b'\n') {
            Some(i) => window = &window[i + 1..],
            None => return None,
        }
    }
    // Lossy is safe: after the fragment drop the window starts on a line
    // boundary, and our writer emits ASCII-framed JSONL (any multibyte content
    // lives inside the payload string, which the field extractor never needs).
    let s = String::from_utf8_lossy(window);
    s.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.to_string())
}

/// Extract a string field `"key":"value"` from one of our JSONL lines.
fn str_field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{key}\":\"");
    let start = line.find(&pat)? + pat.len();
    let rest = &line[start..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

/// Extract a numeric field `"key":123`.
fn num_field(line: &str, key: &str) -> Option<u64> {
    let pat = format!("\"{key}\":");
    let start = line.find(&pat)? + pat.len();
    let rest = &line[start..];
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// Parse one event line → (harness, normalized state). The post-read body of
/// the old `latest()`, extracted so the cached tail path and any future reader
/// share ONE parser. Pure (no I/O, no clock — `normalize` derives everything
/// from the line's own `ts`), which is what makes the parse result cacheable.
fn parse_latest_line(line: &str) -> Option<(Harness, AgentState)> {
    let harness = match str_field(line, "harness")? {
        "claude" => Harness::Claude,
        "cursor" => Harness::Cursor,
        // state-blind harnesses: recognized so the supervisor's synthetic SessionStart
        // (write_spawn_ready_event) is parsed instead of dropped as an unknown harness.
        "codex" => Harness::Codex,
        "commandcode" => Harness::CommandCode,
        "opencode" => Harness::OpenCode,
        "pi" => Harness::Pi,
        "grok" => Harness::Grok,
        _ => return None,
    };
    let event = str_field(line, "event")?.to_string();
    let decision = match str_field(line, "decision") {
        Some("allow") => Some(Decision::Allow),
        Some("defer") => Some(Decision::Defer),
        _ => None,
    };
    let at = num_field(line, "ts").unwrap_or(0);
    Some((
        harness,
        normalize(&RawEvent {
            harness,
            event,
            decision,
            at,
        }),
    ))
}

// ───────────────────────── (mtime, len)-gated tail cache ─────────────────────────
//
// Process-global on purpose: `latest()` keeps its exact signature, so the app's
// `list_queue`, the 1 s HUD timer, and the MCP sidecar (its own process → its own
// cache) all hit it with ZERO caller edits (the frozen seam-3 contract,
// `.paul/analysis/perf-2026-06-10/CONTRACT.md`). Both the parse RESULT and a
// parse FAILURE (`None`) are cached, so a malformed/torn latest line costs one
// read total, not one read per tick.

/// One cached file: the `(mtime, len)` the parse was taken at, plus the result.
struct TailEntry {
    mtime: SystemTime,
    len: u64,
    /// Generation of the last touch (hit or refresh) — the LRU eviction key.
    last_used: u64,
    /// Parsed result for this `(mtime, len)`. `(Harness, AgentState)` is `Copy`,
    /// so a hit is a plain copy out of the map — no allocation.
    parsed: Option<(Harness, AgentState)>,
}

struct TailCache {
    /// Monotonic call counter; stamps `last_used` on every touch.
    gen: u64,
    entries: HashMap<PathBuf, TailEntry>,
}

/// Cap on cached files. Panes are few (≤ ~10 live); the cap only guards
/// unbounded growth from many closed panes over a long-lived process. At the
/// cap the least-recently-used entry is evicted (linear scan over ≤ CAP
/// entries — trivial at this size).
const TAIL_CACHE_CAP: usize = 64;

fn tail_cache() -> MutexGuard<'static, TailCache> {
    static CACHE: OnceLock<Mutex<TailCache>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            Mutex::new(TailCache {
                gen: 0,
                entries: HashMap::new(),
            })
        })
        .lock()
        // A poisoned lock just means some thread panicked while holding it; the
        // map is still a structurally valid cache (worst case: one stale entry
        // → one extra bounded read). Recover instead of propagating the panic.
        .unwrap_or_else(PoisonError::into_inner)
}

/// Cache lookup. Outer `None` = miss (absent or `(mtime, len)` changed — this
/// is deliberately an EQUALITY check, not an ordering one, so clock skew /
/// backwards mtime and a shrunk len (truncate/rotate) all invalidate). Inner
/// value = the cached parse result.
#[allow(clippy::type_complexity)]
fn cache_get(path: &Path, mtime: SystemTime, len: u64) -> Option<Option<(Harness, AgentState)>> {
    let mut c = tail_cache();
    c.gen += 1;
    let gen = c.gen;
    let e = c.entries.get_mut(path)?;
    if e.mtime == mtime && e.len == len {
        e.last_used = gen;
        Some(e.parsed)
    } else {
        None
    }
}

fn cache_put(path: &Path, mtime: SystemTime, len: u64, parsed: Option<(Harness, AgentState)>) {
    let mut c = tail_cache();
    c.gen += 1;
    let gen = c.gen;
    if c.entries.len() >= TAIL_CACHE_CAP && !c.entries.contains_key(path) {
        if let Some(lru) = c
            .entries
            .iter()
            .min_by_key(|(_, e)| e.last_used)
            .map(|(p, _)| p.clone())
        {
            c.entries.remove(&lru);
        }
    }
    c.entries.insert(
        path.to_path_buf(),
        TailEntry {
            mtime,
            len,
            last_used: gen,
            parsed,
        },
    );
}

/// Evict one path — called when its file goes missing/unreadable, so a later
/// recreate can never false-hit on a coincidentally equal `(mtime, len)`.
fn cache_remove(path: &Path) {
    tail_cache().entries.remove(path);
}

/// Parse the latest event for a workspace → (harness, normalized state).
///
/// Steady state (file unchanged since the last call) this is ONE `stat` and
/// zero read bytes; on change it is a bounded ≤[`TAIL_BYTES`] tail read. The
/// stat→read window is benignly racy: if the writer appends between our stat
/// and read we cache content NEWER than the recorded `(mtime, len)`, and the
/// very next append changes the pair again → re-read. Never stale beyond one
/// event — the same tolerance as the pre-cache torn-line tick.
pub fn latest(ws: &Workspace) -> Option<(Harness, AgentState)> {
    let Ok(meta) = fs::metadata(&ws.events_path) else {
        // Missing/unreadable file: pass through as None (the stat IS the cheap
        // negative probe) and drop any stale entry for the path.
        cache_remove(&ws.events_path);
        return None;
    };
    let len = meta.len();
    let Ok(mtime) = meta.modified() else {
        // No mtime on this platform/filesystem: skip the cache (still bounded —
        // the tail read never exceeds TAIL_BYTES). Unreachable on macOS/APFS.
        return last_nonempty_line_tail(&ws.events_path)
            .as_deref()
            .and_then(parse_latest_line);
    };
    if let Some(hit) = cache_get(&ws.events_path, mtime, len) {
        return hit;
    }
    let parsed = last_nonempty_line_tail(&ws.events_path)
        .as_deref()
        .and_then(parse_latest_line);
    cache_put(&ws.events_path, mtime, len, parsed);
    parsed
}

/// Current ranked states across all workspaces: `(id, harness, state)`, ordered
/// by the "who needs me" comparator (AC-4).
pub fn current_states(workspaces: &[Workspace]) -> Vec<(String, Harness, AgentState)> {
    let items: Vec<((String, Harness), AgentState)> = workspaces
        .iter()
        .filter_map(|w| latest(w).map(|(h, s)| ((w.id.clone(), h), s)))
        .collect();
    rank(&items)
        .into_iter()
        .map(|((id, h), s)| (id, h, s))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Harness;

    /// Write `body` as a workspace's events.jsonl in a unique temp dir; return the
    /// Workspace. Zero-dep (temp_dir convention, like tests/inject.rs).
    fn ws_with(tag: &str, body: &str) -> Workspace {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("at-watch-{tag}-{nonce}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let events = dir.join("events.jsonl");
        fs::write(&events, body).unwrap();
        Workspace {
            id: tag.to_string(),
            events_path: events,
        }
    }

    #[test]
    fn str_field_extracts_present_and_none_for_absent() {
        let line = r#"{"harness":"claude","event":"PermissionRequest","decision":"na","ts":1000}"#;
        assert_eq!(str_field(line, "harness"), Some("claude"));
        assert_eq!(str_field(line, "event"), Some("PermissionRequest"));
        // the writer DEFAULTS decision="na"; it is neither allow nor defer → maps to None.
        assert_eq!(str_field(line, "decision"), Some("na"));
        assert_eq!(str_field(line, "missing"), None);
        // torn: opening quote present but no closing quote → None.
        assert_eq!(str_field(r#"{"harness":"cla"#, "harness"), None);
    }

    #[test]
    fn num_field_parses_digits_and_none_otherwise() {
        let line = r#"{"event":"stop","ts":1234,"x":"y"}"#;
        assert_eq!(num_field(line, "ts"), Some(1234));
        assert_eq!(num_field(line, "absent"), None);
        // a non-numeric value yields a zero-length digit slice → parse fails → None.
        assert_eq!(num_field(r#"{"ts":"NaN"}"#, "ts"), None);
    }

    #[test]
    fn last_nonempty_line_returns_last_skipping_blanks() {
        let ws = ws_with("lastline", "first\n\nsecond\n\n  \n");
        assert_eq!(
            last_nonempty_line(&ws.events_path),
            Some("second".to_string())
        );
        let empty = ws_with("emptyfile", "\n  \n\n");
        assert_eq!(last_nonempty_line(&empty.events_path), None);
    }

    #[test]
    fn latest_parses_valid_harness_and_drops_malformed() {
        assert!(matches!(
            latest(&ws_with(
                "c",
                r#"{"harness":"claude","event":"PermissionRequest","ts":1}"#
            )),
            Some((Harness::Claude, _))
        ));
        assert!(matches!(
            latest(&ws_with(
                "u",
                r#"{"harness":"cursor","event":"stop","ts":2}"#
            )),
            Some((Harness::Cursor, _))
        ));
        // state-blind harnesses now PARSE (the dogfood fix) — their synthetic SessionStart
        // is recognized instead of dropped as an unknown harness.
        assert!(matches!(
            latest(&ws_with(
                "cx",
                r#"{"harness":"codex","event":"SessionStart","ts":3}"#
            )),
            Some((Harness::Codex, _))
        ));
        assert!(matches!(
            latest(&ws_with(
                "cc",
                r#"{"harness":"commandcode","event":"SessionStart","ts":3}"#
            )),
            Some((Harness::CommandCode, _))
        ));
        assert!(matches!(
            latest(&ws_with(
                "oc",
                r#"{"harness":"opencode","event":"SessionStart","ts":3}"#
            )),
            Some((Harness::OpenCode, _))
        ));
        // genuinely-unknown harness, missing harness, missing event, torn line → all None.
        assert!(latest(&ws_with(
            "x",
            r#"{"harness":"gemini","event":"stop","ts":3}"#
        ))
        .is_none());
        assert!(latest(&ws_with("nh", r#"{"event":"stop","ts":4}"#)).is_none());
        assert!(latest(&ws_with("ne", r#"{"harness":"claude","ts":5}"#)).is_none());
        assert!(latest(&ws_with("torn", r#"{"harness":"cla"#)).is_none());
    }

    #[test]
    fn latest_uses_the_last_event_line() {
        // older cursor line, newest claude line → the newest (claude) is read.
        let ws = ws_with(
            "multi",
            "{\"harness\":\"cursor\",\"event\":\"stop\",\"ts\":1}\n\
             {\"harness\":\"claude\",\"event\":\"PermissionRequest\",\"ts\":2}\n",
        );
        assert!(matches!(latest(&ws), Some((Harness::Claude, _))));
    }

    #[test]
    fn current_states_drops_workspace_with_malformed_latest() {
        // one valid, one whose newest line has no harness → silently dropped (the
        // documented divergence: discover lists it, current_states omits it).
        let good = ws_with(
            "good",
            r#"{"harness":"claude","event":"PermissionRequest","ts":1}"#,
        );
        let bad = ws_with("bad", r#"{ torn / no harness field"#);
        let rows = current_states(&[good, bad]);
        assert_eq!(rows.len(), 1, "malformed-latest workspace is dropped");
        assert_eq!(rows[0].0, "good");
    }

    // ───────────────── bounded tail read + (mtime,len) cache ─────────────────
    //
    // Tests get unique temp paths per `ws_with` call, so they can't collide in
    // the process-global cache even though `cargo test` runs them in one
    // process. Correctness never DEPENDS on a hit (a miss just re-reads), so
    // cross-test eviction at the cap would be benign too.

    use std::time::Duration;

    /// Differential: the bounded tail read must agree with the whole-file
    /// oracle for every file that fits the window (file < TAIL_BYTES).
    #[test]
    fn tail_read_equals_full_read_for_small_files() {
        let fixtures: &[(&str, &str)] = &[
            ("dempty", ""),
            ("dblanks", "\n  \n\n"),
            (
                "doneline",
                "{\"harness\":\"claude\",\"event\":\"stop\",\"ts\":1}\n",
            ),
            (
                "dmulti",
                "{\"harness\":\"cursor\",\"event\":\"stop\",\"ts\":1}\n\
                 {\"harness\":\"claude\",\"event\":\"PermissionRequest\",\"ts\":2}\n",
            ),
            // torn last line (writer mid-append, no trailing newline): both
            // paths surface the partial line; parse_latest_line drops it later.
            (
                "dtorn",
                "{\"harness\":\"claude\",\"event\":\"stop\",\"ts\":1}\n{\"harness\":\"cla",
            ),
        ];
        for (tag, body) in fixtures {
            let ws = ws_with(tag, body);
            assert_eq!(
                last_nonempty_line_tail(&ws.events_path),
                last_nonempty_line(&ws.events_path),
                "tail/full divergence for fixture {tag}"
            );
        }
    }

    /// File > TAIL_BYTES: the seek lands mid-way through a giant first line;
    /// the partial fragment after the seek point must be skipped and the LAST
    /// complete line returned — same answer as the whole-file oracle.
    #[test]
    fn tail_read_skips_partial_fragment_when_seek_lands_mid_line() {
        let giant = "x".repeat(100 * 1024); // one 100 KB line, then a real event
        let body = format!(
            "{giant}\n{{\"harness\":\"claude\",\"event\":\"PermissionRequest\",\"ts\":9}}\n"
        );
        let ws = ws_with("bigmid", &body);
        assert!(fs::metadata(&ws.events_path).unwrap().len() > TAIL_BYTES);
        assert_eq!(
            last_nonempty_line_tail(&ws.events_path),
            last_nonempty_line(&ws.events_path),
            "tail must find the last complete line past the giant first line"
        );
        // …and the whole read path (cache + parse) sees the claude event.
        assert!(matches!(latest(&ws), Some((Harness::Claude, _))));
    }

    /// A single line longer than the whole window is unparseable for the tick →
    /// None (documented drop-one-tick divergence from the full-read oracle; the
    /// row recovers on the next appended event).
    #[test]
    fn tail_read_gives_up_on_single_line_longer_than_window() {
        let giant = format!("{}\n", "y".repeat(100 * 1024));
        let ws = ws_with("bigone", &giant);
        assert_eq!(last_nonempty_line_tail(&ws.events_path), None);
        assert!(latest(&ws).is_none());
        // appending a normal event recovers it on the next call.
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(&ws.events_path)
            .unwrap();
        std::io::Write::write_all(
            &mut f,
            b"{\"harness\":\"cursor\",\"event\":\"stop\",\"ts\":10}\n",
        )
        .unwrap();
        assert!(matches!(latest(&ws), Some((Harness::Cursor, _))));
    }

    /// The cache gate: unchanged (mtime, len) serves the CACHED parse with no
    /// re-read (proved by swapping same-length content under a restored mtime),
    /// and the gate is an EQUALITY check — an mtime moved FORWARD or BACKWARD
    /// (clock skew) with equal len both invalidate.
    #[test]
    fn cache_hit_on_unchanged_mtime_len_and_equality_gate_invalidation() {
        // "claude" / "cursor" are the same byte length → len is identical below.
        let ws = ws_with(
            "cachehit",
            "{\"harness\":\"claude\",\"event\":\"stop\",\"ts\":7}\n",
        );
        assert!(matches!(latest(&ws), Some((Harness::Claude, _))));
        let mtime0 = fs::metadata(&ws.events_path).unwrap().modified().unwrap();

        // Swap content (same length), restore the original mtime: (mtime, len)
        // unchanged → MUST serve the cached claude parse, i.e. no re-read.
        fs::write(
            &ws.events_path,
            "{\"harness\":\"cursor\",\"event\":\"stop\",\"ts\":7}\n",
        )
        .unwrap();
        let f = fs::OpenOptions::new()
            .write(true)
            .open(&ws.events_path)
            .unwrap();
        f.set_modified(mtime0).unwrap();
        drop(f);
        assert!(
            matches!(latest(&ws), Some((Harness::Claude, _))),
            "unchanged (mtime,len) must hit the cache, not re-read"
        );

        // mtime FORWARD, len equal → invalidate → re-read sees cursor.
        let f = fs::OpenOptions::new()
            .write(true)
            .open(&ws.events_path)
            .unwrap();
        f.set_modified(mtime0 + Duration::from_secs(2)).unwrap();
        drop(f);
        assert!(
            matches!(latest(&ws), Some((Harness::Cursor, _))),
            "a forward mtime change with equal len must invalidate"
        );

        // mtime BACKWARD (clock skew), len equal, content swapped back: an
        // ordering gate would wrongly serve the cached cursor; equality re-reads.
        fs::write(
            &ws.events_path,
            "{\"harness\":\"claude\",\"event\":\"stop\",\"ts\":7}\n",
        )
        .unwrap();
        let f = fs::OpenOptions::new()
            .write(true)
            .open(&ws.events_path)
            .unwrap();
        f.set_modified(mtime0).unwrap();
        drop(f);
        assert!(
            matches!(latest(&ws), Some((Harness::Claude, _))),
            "a backward mtime change must also invalidate (equality, not ordering)"
        );
    }

    /// The behavioral contract the pollers rely on: an append (len grows) is
    /// seen by the very next call.
    #[test]
    fn second_call_after_append_returns_the_new_state() {
        let ws = ws_with(
            "appendinv",
            "{\"harness\":\"claude\",\"event\":\"stop\",\"ts\":1}\n",
        );
        assert!(matches!(latest(&ws), Some((Harness::Claude, _))));
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(&ws.events_path)
            .unwrap();
        std::io::Write::write_all(
            &mut f,
            b"{\"harness\":\"cursor\",\"event\":\"PermissionRequest\",\"ts\":2}\n",
        )
        .unwrap();
        drop(f);
        assert!(
            matches!(latest(&ws), Some((Harness::Cursor, _))),
            "append must invalidate the cache on the next call"
        );
    }

    /// Truncated/rotated file: a SHRUNK len invalidates even though the path is
    /// the same file.
    #[test]
    fn shrunk_file_invalidates() {
        let ws = ws_with(
            "shrink",
            "{\"harness\":\"claude\",\"event\":\"stop\",\"ts\":1}\n\
             {\"harness\":\"cursor\",\"event\":\"PermissionRequest\",\"ts\":2}\n",
        );
        assert!(matches!(latest(&ws), Some((Harness::Cursor, _))));
        // rotate: rewrite with a single (shorter) line.
        fs::write(
            &ws.events_path,
            "{\"harness\":\"claude\",\"event\":\"stop\",\"ts\":3}\n",
        )
        .unwrap();
        assert!(
            matches!(latest(&ws), Some((Harness::Claude, _))),
            "a shrunk file must be re-read"
        );
    }

    /// Missing file passes through as None (and evicts), and a recreated file
    /// is re-parsed — pane close + reopen.
    #[test]
    fn missing_then_recreated_file_reparses() {
        let ws = ws_with(
            "recreate",
            "{\"harness\":\"claude\",\"event\":\"stop\",\"ts\":1}\n",
        );
        assert!(matches!(latest(&ws), Some((Harness::Claude, _))));
        fs::remove_file(&ws.events_path).unwrap();
        assert!(
            latest(&ws).is_none(),
            "missing file is None, not a stale hit"
        );
        fs::write(
            &ws.events_path,
            "{\"harness\":\"cursor\",\"event\":\"stop\",\"ts\":2}\n",
        )
        .unwrap();
        assert!(matches!(latest(&ws), Some((Harness::Cursor, _))));
    }

    // Cross-crate wire-parse EXHAUSTIVENESS: `parse_latest_line` must recognize the wire
    // name of EVERY core/harness `Harness::all()` descriptor (bash excepted — the
    // non-agent test shell deliberately never emits events). Without this, adding a new
    // harness compiles everywhere but its panes are silently INVISIBLE to the ranked
    // queue (the exact class of drift the two-enums lesson warned about).
    #[test]
    fn parse_latest_line_recognizes_every_core_harness_wire() {
        for h in ::harness::Harness::all() {
            let wire = h.descriptor().wire;
            let line = format!(
                "{{\"ts\":1,\"harness\":\"{wire}\",\"event\":\"SessionStart\",\"workspace_id\":\"ws\",\"decision\":\"na\",\"payload\":\"{{}}\"}}"
            );
            let parsed = parse_latest_line(&line);
            if wire == "bash" {
                assert!(
                    parsed.is_none(),
                    "bash is the deliberate non-agent exception"
                );
            } else {
                assert!(
                    parsed.is_some(),
                    "parse_latest_line must recognize harness wire {wire:?} — add it to the match"
                );
            }
        }
    }
}
