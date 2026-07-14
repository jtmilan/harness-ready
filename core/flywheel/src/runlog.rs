//! Run history — a repo-scoped append-only `runs.jsonl` audit log + the `ade runs` list/render.
//!
//! Each `ade run` appends ONE JSON line (a [`RunRecord`]) to `<repo>/.ade/runs.jsonl`; `ade runs`
//! reads + renders it. Pure std (`std::fs` only — the boundary invariant is tauri, not fs). The
//! core forbids clock access (resume determinism), so the `ts` is stamped by the BINARY and passed
//! in (same rule as the binary's `run_id_now`).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One persisted run (the `runs.jsonl` line shape).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RunRecord {
    pub run_id: String,
    /// epoch seconds, stamped by the binary (core has no clock access).
    pub ts: u64,
    pub goal: String,
    pub harnesses: Vec<String>,
    /// the PR base branch the run targeted (audit context).
    #[serde(default)]
    pub base: String,
    pub verdict: String,
    pub exit_code: u8,
    pub pr_url: Option<String>,
    pub patch: Option<String>,
    pub winner_harness: Option<String>,
    pub cost_usd: f64,
}

/// The repo-scoped run log path: `<repo>/.ade/runs.jsonl` (stable regardless of a run's `--out`).
pub fn runs_path(repo: &Path) -> PathBuf {
    repo.join(".ade").join("runs.jsonl")
}

/// Append ONE record as a JSON line. Creates `<repo>/.ade/` if needed. Append-only — never rewrites
/// prior lines (the audit log). Best-effort caller: a log-write failure must not change a run's exit.
pub fn append_run(repo: &Path, rec: &RunRecord) -> std::io::Result<()> {
    let path = runs_path(repo);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Build the FULL line (json + newline) and write it in ONE `write_all`: with O_APPEND a single
    // write is atomic, so concurrent `ade run`s can't interleave a record's json with another's
    // newline. (read_runs also skips any malformed line as a belt-and-braces fallback.)
    let line = format!(
        "{}\n",
        serde_json::to_string(rec).map_err(std::io::Error::other)?
    );
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    use std::io::Write;
    f.write_all(line.as_bytes())
}

/// Read all records, NEWEST-FIRST. A missing file → `vec![]` (not an error). Each line is parsed
/// independently and a malformed line is SKIPPED — a partial/corrupt tail never breaks `ade runs`.
pub fn read_runs(repo: &Path) -> Vec<RunRecord> {
    let path = runs_path(repo);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let mut rows: Vec<RunRecord> = body
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<RunRecord>(l).ok())
        .collect();
    rows.reverse(); // append order → newest-first
    rows
}

/// Render the run list as a fixed-width table (newest-first as given). `pr_url` if present, else the
/// patch path, else `—`. Empty input → a friendly "no runs yet" line.
pub fn render_runs_table(rows: &[RunRecord]) -> String {
    if rows.is_empty() {
        return "ade runs: no runs yet (run `ade run --goal …` first)\n".to_string();
    }
    let run_w = rows
        .iter()
        .map(|r| r.run_id.len())
        .max()
        .unwrap_or(6)
        .max(6);
    let ver_w = rows
        .iter()
        .map(|r| r.verdict.len())
        .max()
        .unwrap_or(7)
        .max(7);
    let har_w = rows
        .iter()
        .map(|r| r.harnesses.join(",").len())
        .max()
        .unwrap_or(8)
        .max(8);
    let mut s = format!(
        "  {:<run_w$}  {:<ver_w$}  {:>4}  {:<har_w$}  {:>7}  {}\n",
        "run", "verdict", "exit", "harness", "cost", "pr / patch",
    );
    for r in rows {
        let target = r.pr_url.as_deref().or(r.patch.as_deref()).unwrap_or("—");
        s.push_str(&format!(
            "  {:<run_w$}  {:<ver_w$}  {:>4}  {:<har_w$}  {:>7}  {}\n",
            r.run_id,
            r.verdict,
            r.exit_code,
            r.harnesses.join(","),
            format!("${:.2}", r.cost_usd),
            target,
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(run_id: &str, ts: u64, verdict: &str, pr: Option<&str>) -> RunRecord {
        RunRecord {
            run_id: run_id.to_string(),
            ts,
            goal: "g".to_string(),
            harnesses: vec!["claude".to_string()],
            base: "main".to_string(),
            verdict: verdict.to_string(),
            exit_code: 0,
            pr_url: pr.map(str::to_string),
            patch: None,
            winner_harness: None,
            cost_usd: 0.17,
        }
    }

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("ade-runlog-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn append_then_read_roundtrips_newest_first() {
        let repo = tmp("roundtrip");
        append_run(&repo, &rec("ade-1", 1, "pass", Some("https://x/pull/1"))).unwrap();
        append_run(&repo, &rec("ade-2", 2, "hold", None)).unwrap();
        let rows = read_runs(&repo);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].run_id, "ade-2", "newest first");
        assert_eq!(rows[1].run_id, "ade-1");
        assert_eq!(rows[1].pr_url.as_deref(), Some("https://x/pull/1"));
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn read_runs_missing_file_is_empty() {
        let repo = tmp("missing");
        assert!(read_runs(&repo).is_empty());
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn read_runs_skips_malformed_lines() {
        let repo = tmp("malformed");
        append_run(&repo, &rec("ade-good", 1, "pass", None)).unwrap();
        // corrupt tail line
        let path = runs_path(&repo);
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(f, "{{ this is not valid json").unwrap();
        let rows = read_runs(&repo);
        assert_eq!(
            rows.len(),
            1,
            "malformed line skipped, good record survives"
        );
        assert_eq!(rows[0].run_id, "ade-good");
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn render_runs_table_shows_verdict_and_pr_or_patch() {
        let mut a = rec("ade-1", 1, "pass", Some("https://github.com/o/r/pull/1"));
        a.harnesses = vec!["claude".into(), "cursor".into()];
        let mut b = rec("ade-2", 2, "hold", None);
        b.patch = Some("/out/consolidated.patch".into());
        let table = render_runs_table(&[b, a]);
        assert!(table.contains("verdict"), "has a header");
        assert!(table.contains("ade-1") && table.contains("ade-2"));
        assert!(
            table.contains("https://github.com/o/r/pull/1"),
            "pr_url shown"
        );
        assert!(
            table.contains("/out/consolidated.patch"),
            "patch shown when no pr"
        );
        assert!(table.contains("claude,cursor"), "harness list shown");
    }

    #[test]
    fn render_empty_is_friendly() {
        assert!(render_runs_table(&[]).contains("no runs yet"));
    }
}
