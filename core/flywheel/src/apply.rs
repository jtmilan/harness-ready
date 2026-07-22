//! `run_apply_single` — the P1 MVP goal→PR spine for ONE claude worker (N=1).
//!
//! Connects the extracted stages into a single flow on the proven `RunContext` keystone:
//!   orchestrate 1 pane → spawn the claude worker in an isolated worktree → settle →
//!   N=1 trivial fold (the worker's worktree IS the integration tree) → `synthesize_core`
//!   (gates the diff; the LLM call stays injected) → `flywheel_push_and_pr` (gated; `--no-pr`
//!   emits a patch instead). Emits a verdict + exit code for the CLI to surface.
//!
//! The three external effects — the orchestration LLM call, the worker `Command`, and the
//! synthesis LLM call — are injected via [`ApplyHooks`] so the whole spine is testable against a
//! mock worker + stub synth with NO live `claude` and NO network (see the tests at the bottom).
//!
//! Deliberately N=1 claude-only: multi-harness fan-out + the full `fold_support` merge-tree are
//! P2; the 5 `synthesize` gate stubs stay unreachable here (all three gate flags are OFF).

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::gitutil::git_capture;
use crate::orchestrate::{self, ClaudeUsage, PaneCtx};
use crate::runctx::{RunContext, StderrEmitter};
use crate::synthesize::{self, BridgeVerdict, CoreOutcome, PaneTruth, RepoTarget, SynthOpts};

/// Inputs for one `ade run` (single-harness). Built from the CLI `RunArgs`.
#[derive(Clone, Debug)]
pub struct ApplyOpts {
    pub run_id: String,
    pub goal: String,
    pub repo: PathBuf,
    pub base: String,
    /// the harnesses to fan out to (one worker per orchestrated pane). N=1 → single-worker path.
    pub harnesses: Vec<String>,
    /// competing-arms mode (`--compete`): each pane runs the FULL goal; the objective winner is
    /// picked + shipped (vs the default fold-all where panes do complementary subtasks).
    pub compete: bool,
    /// refuse unpinned harnesses upfront (apples-to-apples). bridgespace `--fair`.
    pub fair: bool,
    /// per-harness model pins, bridgespace grammar `--models "claude=...,codex=..."`.
    pub models: Vec<(String, String)>,
    /// claude model pin (1P alias; resolved to Bedrock against the repo), or None for the default.
    pub model: Option<String>,
    pub effort: String,
    pub timeout_secs: u64,
    pub write: bool,
    /// arm the cross-domain critique gate (`--critique`). OFF by default → advisory, no verdict change.
    pub critique: bool,
    /// arm the calibrated smart-PR-review gate (`--review`). OFF by default.
    pub review: bool,
    /// arm the CRAP-delta gate (`--crap`). OFF by default; coverage-gated → inert without an artifact.
    pub crap: bool,
    /// open a PR (the default). `false` == `--no-pr` → emit a patch instead, never touch the network.
    pub pr: bool,
    /// controller claude-token guardrail; worker-harness tokens are NOT metered. `Some(N)` → a run
    /// whose metered controller usage (`input + output`) exceeds N must NOT auto-open a PR.
    pub budget_tokens: Option<u64>,
    /// out/artifact dir (verdict.json, final.md, consolidated.patch live here).
    pub out_dir: PathBuf,
}

/// The result of a run — what the CLI turns into `verdict.json` + an exit code.
#[derive(Clone, Debug)]
pub struct ApplyOutcome {
    /// "pass" | "hold" | "reject" | "impl-exhausted" | "secret-blocked" | "budget-exceeded" |
    /// "pr-failed" (clean synth verdict but the push/PR ship step failed) | "error".
    pub verdict: String,
    /// 0 shipped/patch · 1 orchestrate/precondition · 2 impl exhausted · 3 synth gate · 4 PR step ·
    /// 5 token budget exceeded (PR skipped).
    pub exit_code: u8,
    pub pr_url: Option<String>,
    pub patch_path: Option<PathBuf>,
    pub final_md: Option<PathBuf>,
    pub usage: ClaudeUsage,
    /// human-readable one-liner (held reason / error / "PR opened").
    pub detail: String,
    /// per-pane/arm report rows (the fan-out table + verdict.json `per_arm`).
    pub per_arm: Vec<ArmSummary>,
    /// the winning harness in compete-mode (None for fold-all / errors).
    pub winner_harness: Option<String>,
}

impl ApplyOutcome {
    fn err(verdict: &str, code: u8, detail: impl Into<String>) -> Self {
        ApplyOutcome {
            verdict: verdict.to_string(),
            exit_code: code,
            pr_url: None,
            patch_path: None,
            final_md: None,
            usage: ClaudeUsage::default(),
            detail: detail.into(),
            per_arm: Vec::new(),
            winner_harness: None,
        }
    }
}

/// One pane's work unit (P2-03): the worker id, its harness, and the prompt it runs.
#[derive(Clone, Debug)]
pub struct PaneTask {
    pub wid: String,
    pub harness: String,
    pub prompt: String,
}

/// One synthesis pass's product: (document, usage) or an error string.
pub type SynthOutput = Result<(String, ClaudeUsage), String>;

/// The injected effects. Production builds these from `ApplyOpts`; tests supply mocks.
pub struct ApplyHooks<'a> {
    /// Orchestrate the goal → the per-pane work units (one worker each).
    pub plan: &'a dyn Fn() -> Result<Vec<PaneTask>, String>,
    /// Build a pane's worker spec (per-harness `Command` + prompt-delivery mode), given the pane and
    /// its worktree cwd. Production dispatches per harness; tests supply `sh` mocks (`prompt_via_stdin
    /// = true` so no extra positional is appended to the mock argv).
    pub worker_cmd: &'a dyn Fn(&PaneTask, &std::path::Path) -> crate::worker::WorkerSpec,
    /// One synthesis pass: prompt → (document, usage). The LLM call stays the caller's.
    pub synth_one_pass: &'a dyn Fn(&str) -> SynthOutput,
}

/// The N>1 fan-in outcome (P2-02): the assembled integration worktree + which panes folded in.
#[derive(Debug, Clone)]
pub(crate) struct FoldResult {
    pub worktree: PathBuf,
    pub committed: Vec<String>,
    pub conflicts: usize,
    pub skipped: usize,
}

/// Fold N settled worker branches (`agent-teams/<wid>`) into ONE integration worktree via the frozen
/// `fold_support::assemble_integration_blocking` (git merge-tree). Returns None when no worker
/// committed OR the fold produced no tree (git <2.38 / all-conflict) — the caller then degrades.
/// This is the N>1 fan-in primitive the multi-worker apply path drops into; it is proven against
/// REAL local git with no claude/network.
pub(crate) fn fold_workers(git_root: &Path, integ_id: &str, wids: &[String]) -> Option<FoldResult> {
    use crate::synthesize::fold_support;
    let committed = fold_support::panes_with_commits(git_root, wids);
    if committed.is_empty() {
        return None;
    }
    let res = fold_support::assemble_integration_blocking(git_root, integ_id, &committed);
    let worktree = res.worktree.as_deref().map(PathBuf::from)?;
    Some(FoldResult {
        worktree,
        committed,
        conflicts: res.conflicts.len(),
        skipped: res.skipped.len(),
    })
}

/// One competing arm's machine-collected result (bridgespace winner-pick, P2-04).
#[allow(dead_code)] // consumed by compete-mode orchestration (P2-05)
#[derive(Debug, Clone)]
pub(crate) struct ArmResult {
    pub harness: String,
    pub wid: String,
    pub committed: bool,
    pub verdict: BridgeVerdict,
    pub diff_lines: usize,
}

fn verdict_rank(v: BridgeVerdict) -> u8 {
    match v {
        BridgeVerdict::Pass => 0,
        BridgeVerdict::Hold => 1,
        BridgeVerdict::Reject => 2,
    }
}

/// The OBJECTIVE winner among competing arms (PURE, deterministic): the committed arm with the best
/// verdict (Pass > Hold > Reject), tie-broken by the smallest diff (least churn), then stable order.
/// None when no arm committed. (bridgespace "objective winner pick".)
#[allow(dead_code)] // wired into compete-mode in P2-05
pub(crate) fn pick_winner(arms: &[ArmResult]) -> Option<usize> {
    arms.iter()
        .enumerate()
        .filter(|(_, a)| a.committed)
        .min_by(|(_, x), (_, y)| {
            verdict_rank(x.verdict)
                .cmp(&verdict_rank(y.verdict))
                .then(x.diff_lines.cmp(&y.diff_lines))
        })
        .map(|(i, _)| i)
}

/// CI-disarm safety gate for `--apply` (auto-merge, DANGER). PURE — the binary passes the resolved
/// env booleans. Returns `Ok(armed)` (whether auto-merge may proceed) or `Err(reason)` (refused):
/// - `--apply` not requested → `Ok(false)`.
/// - requested under CI (`CI`/`GITHUB_ACTIONS`) → `Err` (NEVER auto-merge unattended in CI).
/// - requested locally without the explicit `ADE_ARM_APPLY=1` opt-in → `Err` (off by default).
/// - requested locally WITH the opt-in → `Ok(true)`.
pub fn apply_gate(apply_requested: bool, in_ci: bool, armed_optin: bool) -> Result<bool, String> {
    if !apply_requested {
        return Ok(false);
    }
    if in_ci {
        return Err("--apply (auto-merge) is REFUSED under CI (CI/GITHUB_ACTIONS set) — never auto-merge unattended".to_string());
    }
    if !armed_optin {
        return Err("--apply (auto-merge) is OFF by default — set ADE_ARM_APPLY=1 to arm it (local, non-CI only)".to_string());
    }
    Ok(true)
}

/// Pure token-budget guardrail. `false` when `budget` is `None` (no ceiling); otherwise true iff the
/// metered controller usage (`input + output`) STRICTLY exceeds the budget (exactly-at is allowed).
/// Worker-harness tokens are NOT metered — only the controller's claude calls (orchestrate + synth).
pub fn over_budget(usage: &crate::orchestrate::ClaudeUsage, budget: Option<u64>) -> bool {
    match budget {
        None => false,
        Some(b) => usage.input.saturating_add(usage.output) > b,
    }
}

/// The resolved target of an `ade merge` (P3-02): which PR/branch to merge for a chosen arm.
#[derive(Debug, Clone, PartialEq)]
pub struct MergePlan {
    pub harness: String,
    pub pr_url: String,
    pub branch: String,
}

/// Anchored validation that `u` is a real GitHub PR URL — `https://github.com/<owner>/<repo>/pull/<n>`
/// with no whitespace/control chars. Because a valid value MUST start with `https://`, it can never be
/// an argv flag (`-…`) → this is the defense against flag-smuggling when `pr_url` is read from a
/// (possibly tampered) `verdict.json` and passed to `gh`. No regex crate (no new deps) → manual parse.
fn is_github_pr_url(u: &str) -> bool {
    if u.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let Some(rest) = u.strip_prefix("https://github.com/") else {
        return false;
    };
    let parts: Vec<&str> = rest.split('/').collect();
    parts.len() == 4
        && !parts[0].is_empty()
        && !parts[1].is_empty()
        && parts[2] == "pull"
        && !parts[3].is_empty()
        && parts[3].bytes().all(|b| b.is_ascii_digit())
}

/// Resolve `ade merge <harness>` against a run's `verdict.json` (PURE — no I/O). Returns the PR to
/// merge iff: the run opened a PR (`pr_url` is a valid github PR URL), the harness's `per_arm` entry
/// exists AND committed, AND — when a `winner_harness` is recorded (compete) — it equals `harness`
/// (only the shipped arm has the PR). Fold/single runs have no `winner_harness` → any committed arm.
pub fn resolve_merge(verdict: &serde_json::Value, harness: &str) -> Result<MergePlan, String> {
    let pr_url = verdict
        .get("pr_url")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or("this run has no PR (run without --no-pr first, or the PR step failed)")?;
    // Defense-in-depth: pr_url comes from a file → validate its SHAPE before it ever reaches `gh`,
    // so a tampered value can't smuggle an argv flag (a valid URL starts with https://, never `-`).
    if !is_github_pr_url(pr_url) {
        return Err(format!(
            "refusing to merge: pr_url is not a valid GitHub PR URL ({pr_url:?})"
        ));
    }
    let per_arm = verdict
        .get("per_arm")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let arm = per_arm
        .iter()
        .find(|a| a.get("harness").and_then(|h| h.as_str()) == Some(harness))
        .ok_or_else(|| format!("no arm for harness '{harness}' in this run"))?;
    if !arm
        .get("committed")
        .and_then(|c| c.as_bool())
        .unwrap_or(false)
    {
        return Err(format!("arm '{harness}' did not commit — nothing to merge"));
    }
    if let Some(w) = verdict.get("winner_harness").and_then(|w| w.as_str()) {
        if w != harness {
            return Err(format!(
                "'{harness}' is not the winning arm ('{w}') — only the shipped arm's PR exists"
            ));
        }
    }
    let branch = verdict
        .get("run_id")
        .and_then(|r| r.as_str())
        .map(|r| format!("ade/{r}"))
        .unwrap_or_default();
    Ok(MergePlan {
        harness: harness.to_string(),
        pr_url: pr_url.to_string(),
        branch,
    })
}

/// Count the leading run of chars matching `pred` in `s` (for token-shape checks).
fn leading_run(s: &str, pred: impl Fn(char) -> bool) -> usize {
    s.chars().take_while(|c| pred(*c)).count()
}

/// Scan a unified diff for high-signal secrets on the ADDED lines (so context/removed lines are
/// ignored). Returns a deduped description per hit. PURE — no I/O, no regex crate. High-signal only
/// (private-key headers + GitHub/OpenAI/AWS token shapes) to avoid false positives. The pre-PR gate.
pub fn scan_secrets(diff: &str) -> Vec<String> {
    let mut hits: Vec<String> = Vec::new();
    let note = |h: &str, hits: &mut Vec<String>| {
        if !hits.iter().any(|x| x == h) {
            hits.push(h.to_string());
        }
    };
    for line in diff
        .lines()
        .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
    {
        let l = &line[1..]; // strip the diff '+'
        if l.contains("-----BEGIN") && l.contains("PRIVATE KEY") {
            note(
                "private key header (-----BEGIN … PRIVATE KEY-----)",
                &mut hits,
            );
        }
        for pref in ["ghp_", "gho_", "ghu_", "ghs_", "ghr_"] {
            if let Some(idx) = l.find(pref) {
                let tail = &l[idx + pref.len()..];
                if leading_run(tail, |c| c.is_ascii_alphanumeric()) >= 20 {
                    note("GitHub token (gh*_…)", &mut hits);
                }
            }
        }
        if let Some(idx) = l.find("sk-") {
            let tail = &l[idx + 3..];
            if leading_run(tail, |c| c.is_ascii_alphanumeric()) >= 20 {
                note("OpenAI key (sk-…)", &mut hits);
            }
        }
        if let Some(idx) = l.find("AKIA") {
            let tail = &l[idx + 4..];
            if leading_run(tail, |c| c.is_ascii_uppercase() || c.is_ascii_digit()) >= 16 {
                note("AWS access key (AKIA…)", &mut hits);
            }
        }
    }
    hits
}

/// One row of the fan-out report (P2-07): what each pane/arm did.
#[derive(Debug, Clone)]
pub struct ArmSummary {
    pub wid: String,
    pub harness: String,
    pub committed: bool,
    /// "folded" | "skipped" (fold-all) · "pass"/"hold"/"reject" (compete) · the winner is flagged.
    pub verdict: String,
    pub diff_lines: usize,
    pub winner: bool,
}

/// Render the per-arm fan-out report as an aligned table (PURE, deterministic, newline-terminated).
/// The winner row is marked `★`. Used for the stderr "live table" + verdict.json `per_arm`.
pub fn render_arm_table(arms: &[ArmSummary]) -> String {
    let wid_w = arms
        .iter()
        .map(|a| a.wid.len())
        .chain([3])
        .max()
        .unwrap_or(3);
    let har_w = arms
        .iter()
        .map(|a| a.harness.len())
        .chain([7])
        .max()
        .unwrap_or(7);
    let ver_w = arms
        .iter()
        .map(|a| a.verdict.len())
        .chain([7])
        .max()
        .unwrap_or(7);
    let mut s = format!(
        "  {:<wid_w$}  {:<har_w$}  {:<9}  {:<ver_w$}  {:>5}\n",
        "arm", "harness", "committed", "verdict", "±loc"
    );
    for a in arms {
        s.push_str(&format!(
            "{} {:<wid_w$}  {:<har_w$}  {:<9}  {:<ver_w$}  {:>5}\n",
            if a.winner { "★" } else { " " },
            a.wid,
            a.harness,
            if a.committed { "yes" } else { "no" },
            a.verdict,
            a.diff_lines,
        ));
    }
    s
}

/// The harnesses NOT pinned to a model — the ones `--fair` refuses (apples-to-apples). PURE.
fn fair_refusals(harnesses: &[String], models: &[(String, String)]) -> Vec<String> {
    harnesses
        .iter()
        .filter(|h| !models.iter().any(|(mh, _)| mh == *h))
        .cloned()
        .collect()
}

/// Char-boundary-safe truncation to at most `max` CHARS, appending `…` when cut. A byte slice
/// (`&s[..70]`) panics when byte 70 lands inside a multibyte char — this never does.
pub(crate) fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        None => s.to_string(),
        Some((idx, _)) => format!("{}…", &s[..idx]),
    }
}

/// The REAL base-freshness stamp for `PaneTruth::base_vs_main` (the CLI spine previously fabricated
/// "base == current main (fresh)" without ever checking). Fetches `origin main` (tolerating an
/// unreachable origin), then counts how far `base_sha` is behind `origin/main`:
/// - fetch failed → "origin unreachable; freshness unverified" (no Hold — honest, not fabricated),
/// - 0 behind    → "base == current main (fresh)",
/// - N behind    → "STALE BASE — N commit(s) behind origin/main" (the `contains("STALE BASE")`
///   stamp `pane_truth_contradicted` needs so the STALE BASE Hold can actually fire).
pub(crate) fn base_vs_main_stamp(git_root: &Path, base_sha: &str) -> String {
    let fetched = std::process::Command::new("git")
        .arg("-C")
        .arg(git_root)
        .args(["fetch", "--quiet", "origin", "main"])
        .env("PATH", crate::gitutil::harness_path())
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !fetched {
        return "origin unreachable; freshness unverified".to_string();
    }
    match git_capture(
        git_root,
        &["rev-list", "--count", &format!("{base_sha}..origin/main")],
    )
    .and_then(|s| s.trim().parse::<u64>().ok())
    {
        Some(0) => "base == current main (fresh)".to_string(),
        Some(n) => format!("STALE BASE — {n} commit(s) behind origin/main"),
        None => "origin fetched but behind-count failed; freshness unverified".to_string(),
    }
}

#[allow(clippy::too_many_arguments)]
fn count_diff_lines(diff: &str) -> usize {
    diff.lines()
        .filter(|l| {
            (l.starts_with('+') || l.starts_with('-'))
                && !l.starts_with("+++")
                && !l.starts_with("---")
        })
        .count()
}

/// Build the per-arm tasks for COMPETE mode: one `PaneTask` per harness, EACH running the FULL goal
/// (competing arms — no decomposition). PURE → unit-testable. NOTE: the production orchestrate→tasks
/// mapping is for FOLD-ALL only; with `decompose=false` orchestrate emits ONE task, which would spawn
/// a single arm — so compete MUST build its N full-goal tasks here, not from orchestrate.
pub(crate) fn compete_pane_tasks(harnesses: &[String], goal: &str) -> Vec<PaneTask> {
    harnesses
        .iter()
        .enumerate()
        .map(|(i, h)| PaneTask {
            wid: format!("w{i}"),
            harness: h.clone(),
            prompt: goal.to_string(),
        })
        .collect()
}

/// COMPETE-MODE pick (P2-05): synthesize EACH committed arm independently (its own worktree as the
/// integration target → its own authoritative-test verdict), then [`pick_winner`] selects the
/// objective best, returning the winner's (wid, harness, worktree, CoreOutcome). The losing arms'
/// trees are discarded (RunContext Drop cleans them). The LLM synth call stays injected.
#[allow(clippy::too_many_arguments)]
fn compete_pick(
    ctx: &RunContext,
    tasks: &[PaneTask],
    committed: &[String],
    base_sha: &str,
    base_vs_main: &str,
    git_root: &Path,
    opts: &ApplyOpts,
    synth_one_pass: &dyn Fn(&str) -> SynthOutput,
) -> Option<(String, String, PathBuf, CoreOutcome, Vec<ArmResult>)> {
    let mut arms: Vec<ArmResult> = Vec::new();
    let mut outcomes: Vec<(String, CoreOutcome)> = Vec::new();
    for wid in committed {
        let wt = match ctx.worktree_of(wid) {
            Some(p) => p.to_path_buf(),
            None => continue,
        };
        let harness = tasks
            .iter()
            .find(|t| &t.wid == wid)
            .map(|t| t.harness.clone())
            .unwrap_or_default();
        let range = format!("{base_sha}..HEAD");
        let diff_lines =
            count_diff_lines(&git_capture(&wt, &["diff", base_sha, "HEAD"]).unwrap_or_default());
        let truth = vec![PaneTruth {
            pane: wid.clone(),
            worktree_found: true,
            head: git_capture(&wt, &["rev-parse", "HEAD"]).unwrap_or_default(),
            base_vs_main: base_vs_main.to_string(),
            diff_stat: git_capture(&wt, &["diff", "--stat", &range]).unwrap_or_default(),
            status: git_capture(&wt, &["status", "--porcelain"]).unwrap_or_default(),
        }];
        let docs = vec![(
            wid.clone(),
            format!("Competing arm {harness} ({wid}) — full-goal attempt."),
        )];
        let arm_dir = opts.out_dir.join(format!("arm-{wid}"));
        let _ = std::fs::create_dir_all(&arm_dir);
        let repo_target = RepoTarget {
            git_root: git_root.to_path_buf(),
            integ_target: Some(wt.clone()),
            refreshed_fold: None,
        };
        let synth_opts = SynthOpts {
            goal: &opts.goal,
            contributors: &docs,
            critique_on: opts.critique,
            report_only_allowed: true,
            retry_inadequate: true,
            review_on: opts.review,
            crap_on: opts.crap,
        };
        match synthesize::synthesize_core(
            &docs,
            &truth,
            repo_target,
            synth_opts,
            synth_one_pass,
            &arm_dir,
        ) {
            Ok(o) => {
                arms.push(ArmResult {
                    harness: harness.clone(),
                    wid: wid.clone(),
                    committed: true,
                    verdict: o.verdict,
                    diff_lines,
                });
                outcomes.push((wid.clone(), o));
            }
            // an arm whose synth errored is ineligible (committed=false) but still recorded.
            Err(_) => arms.push(ArmResult {
                harness,
                wid: wid.clone(),
                committed: false,
                verdict: BridgeVerdict::Reject,
                diff_lines,
            }),
        }
    }
    let idx = pick_winner(&arms)?;
    let wwid = arms[idx].wid.clone();
    let wharness = arms[idx].harness.clone();
    let wwt = ctx.worktree_of(&wwid)?.to_path_buf();
    let outcome = outcomes
        .into_iter()
        .find(|(w, _)| *w == wwid)
        .map(|(_, o)| o)?;
    Some((wwid, wharness, wwt, outcome, arms))
}

fn verdict_label(v: BridgeVerdict) -> &'static str {
    match v {
        BridgeVerdict::Pass => "pass",
        BridgeVerdict::Hold => "hold",
        BridgeVerdict::Reject => "reject",
    }
}

/// The production worker-spec dispatch, extracted PURE (no spawn/IO) so the per-harness routing is
/// unit-testable. Delegates to `crate::worker::worker_command`: claude/unknown → claude (stdin),
/// cursor/codex/opencode → their faithful argv (positional prompt). opencode's `--dir <wt>` is baked
/// against `wt` (the worktree cwd). Non-claude models pass VERBATIM.
fn production_worker_spec(
    pt: &PaneTask,
    repo: &Path,
    model: Option<&str>,
    write: bool,
    wt: &std::path::Path,
) -> crate::worker::WorkerSpec {
    crate::worker::worker_command(&pt.harness, repo, model, write, wt)
}

/// Production entry: build the real hooks (orchestrate_sync / claude worker / run_claude_capture)
/// and run the N-worker spine.
pub fn run_apply(opts: ApplyOpts) -> ApplyOutcome {
    let deadline = Duration::from_secs(opts.timeout_secs);
    let goal = opts.goal.clone();
    let repo = opts.repo.clone();
    let effort = opts.effort.clone();
    let model = opts.model.clone();
    let write = opts.write;
    let harnesses = opts.harnesses.clone();
    let compete = opts.compete;

    let plan = || -> Result<Vec<PaneTask>, String> {
        // COMPETE → N competing arms, each running the FULL goal (orchestrate's decompose=false emits
        // ONE task → would spawn a single arm, so build the N full-goal tasks directly).
        if compete {
            let tasks = compete_pane_tasks(&harnesses, &goal);
            if tasks.is_empty() {
                return Err("compete: no harnesses".into());
            }
            return Ok(tasks);
        }
        // FOLD-ALL → orchestrate splits the goal into per-pane subtasks.
        let panes: Vec<PaneCtx> = harnesses
            .iter()
            .enumerate()
            .map(|(i, h)| PaneCtx {
                id: format!("p{i}"),
                harness: h.clone(),
                focus: None,
                role: None,
            })
            .collect();
        let decompose = panes.len() > 1;
        let orch = orchestrate::orchestrate_sync(&panes, &goal, Some(&repo), decompose)?;
        let tasks: Vec<PaneTask> = orch
            .tasks
            .into_iter()
            .enumerate()
            .map(|(i, t)| {
                let harness = panes
                    .iter()
                    .find(|p| p.id == t.id)
                    .map(|p| p.harness.clone())
                    .unwrap_or_else(|| "claude".into());
                PaneTask {
                    wid: format!("w{i}"),
                    harness,
                    prompt: t.task,
                }
            })
            .collect();
        if tasks.is_empty() {
            return Err("orchestrate produced no tasks".into());
        }
        Ok(tasks)
    };
    // P4 multi-harness: dispatch the real per-harness worker `Command` by `pt.harness`
    // (claude/cursor/codex/opencode; unknown → claude fallback). opencode bakes `--dir <wt>` here.
    let worker_cmd = |pt: &PaneTask, wt: &std::path::Path| {
        production_worker_spec(pt, &repo, model.as_deref(), write, wt)
    };
    let synth = |prompt: &str| -> Result<(String, ClaudeUsage), String> {
        let raw =
            orchestrate::run_claude_capture(prompt, deadline, None, Some(&repo), Some(&effort))?;
        let v: serde_json::Value =
            serde_json::from_str(&raw).map_err(|e| format!("synth parse: {e}"))?;
        let doc = v
            .get("result")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        Ok((doc, ClaudeUsage::from_value(&v)))
    };

    run_apply_with(
        &opts,
        &ApplyHooks {
            plan: &plan,
            worker_cmd: &worker_cmd,
            synth_one_pass: &synth,
        },
    )
}

/// Sweep every leftover `.agent-teams-worktrees/*.manifest` (a prior run that crashed before its
/// `RunContext::Drop` could clean). Checks BOTH the selected repo dir and the git toplevel (the
/// manifest is written under the selected dir; worktrees land under the toplevel). Best-effort.
fn sweep_stale_manifests(repo: &Path, git_root: &Path) {
    let mut dirs = vec![repo.join(".agent-teams-worktrees")];
    let root_dir = git_root.join(".agent-teams-worktrees");
    if root_dir != dirs[0] {
        dirs.push(root_dir);
    }
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("manifest") {
                let n = crate::runctx::sweep_manifest(repo, &p);
                if n > 0 {
                    eprintln!(
                        "ade: swept {n} orphan worktree(s) from stale manifest {}",
                        p.display()
                    );
                }
            }
        }
    }
}

/// The testable N-worker spine. `hooks` injects the three external effects so this runs hermetically.
/// N=1 keeps the proven trivial-fold path; N>1 folds the committed branches into one integration tree.
pub fn run_apply_with(opts: &ApplyOpts, hooks: &ApplyHooks) -> ApplyOutcome {
    let _ = std::fs::create_dir_all(&opts.out_dir);

    // ⓪ FAIR MODE: refuse unpinned harnesses upfront (apples-to-apples) BEFORE spawning anything.
    if opts.fair {
        let refusals = fair_refusals(&opts.harnesses, &opts.models);
        if !refusals.is_empty() {
            return ApplyOutcome::err(
                "error",
                1,
                format!("fair: refused — unpinned harness(es): {refusals:?} (pin with --models)"),
            );
        }
    }

    // ① ORCHESTRATE → the per-pane work units.
    let tasks = match (hooks.plan)() {
        Ok(t) if !t.is_empty() => t,
        Ok(_) => return ApplyOutcome::err("error", 1, "orchestrate produced no tasks"),
        Err(e) => return ApplyOutcome::err("error", 1, format!("orchestrate failed: {e}")),
    };

    // The base every worker branch forks from (repo HEAD at spawn). Fold compares base..HEAD.
    let base_sha = match git_capture(&opts.repo, &["rev-parse", "HEAD"]) {
        Some(s) if !s.is_empty() => s,
        _ => {
            return ApplyOutcome::err("error", 1, "repo has no HEAD (not a git repo / no commits)")
        }
    };
    let git_root = git_capture(&opts.repo, &["rev-parse", "--show-toplevel"])
        .map(PathBuf::from)
        .unwrap_or_else(|| opts.repo.clone());

    // ①.5 CRASH-RECOVERY: sweep worktree manifests left by prior runs that died before
    // `RunContext::Drop` ran (the sweep half of the Drop design — previously never called).
    // Best-effort: log + continue, never abort the run. NOTE: a manifest only outlives its run on
    // a crash, but a CONCURRENT run on the same repo would also have a live manifest — accepted
    // trade-off (ade runs are per-repo serial today).
    sweep_stale_manifests(&opts.repo, &git_root);

    // ② SPAWN N workers. RunContext owns the worktrees; keep it in scope until after fold/synth/PR.
    let mut ctx = RunContext::new(
        opts.run_id.clone(),
        opts.repo.clone(),
        Box::new(StderrEmitter),
    );
    for pt in &tasks {
        let log_path = opts.out_dir.join(format!("{}.log", pt.wid));
        if let Err(e) = ctx.spawn_worker(
            &pt.wid,
            |wt| (hooks.worker_cmd)(pt, wt),
            Some(&pt.prompt),
            Some(log_path),
        ) {
            return ApplyOutcome::err("error", 1, format!("worker {} spawn failed: {e}", pt.wid));
        }
    }

    // ③ SETTLE all.
    let _settled = ctx.settle(Duration::from_secs(opts.timeout_secs));

    // ③.5 AUTO-COMMIT safety net (harness-agnostic). A worker may EDIT its isolated worktree but
    // never `git commit`: claude self-commits from a bare task, but cursor/codex/opencode routinely
    // leave the change uncommitted (live-verified — a cursor worker edited + `cargo test`-passed but
    // never committed → empty `base..HEAD` → false impl-exhausted). Stage + commit any leftover
    // changes on the worker's `agent-teams/<wid>` branch with a synthetic identity so the fold sees a
    // diff. No-op when the worker already committed or made no change. Push stays denied — this is a
    // LOCAL commit in the isolated worktree; the controller's git has the same no-remote posture.
    for pt in &tasks {
        if let Some(wt) = ctx.worktree_of(&pt.wid) {
            let dirty = git_capture(wt, &["status", "--porcelain"])
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            if dirty {
                let _ = git_capture(wt, &["add", "-A"]);
                let _ = git_capture(
                    wt,
                    &[
                        "-c",
                        "user.name=flywheel-worker",
                        "-c",
                        "user.email=flywheel@localhost",
                        "commit",
                        "-q",
                        "-m",
                        "ade worker: autocommit (controller safety net)",
                    ],
                );
            }
        }
    }

    // ④ Which panes actually committed (non-empty diff vs base)?
    let committed: Vec<String> = tasks
        .iter()
        .map(|pt| pt.wid.clone())
        .filter(|wid| {
            ctx.worktree_of(wid)
                .map(|wt| {
                    let count =
                        git_capture(wt, &["rev-list", "--count", &format!("{base_sha}..HEAD")])
                            .unwrap_or_default();
                    let diff = git_capture(wt, &["diff", &base_sha, "HEAD"]).unwrap_or_default();
                    count != "0" && !count.is_empty() && !diff.trim().is_empty()
                })
                .unwrap_or(false)
        })
        .collect();
    if committed.is_empty() {
        return ApplyOutcome::err(
            "impl-exhausted",
            2,
            "no worker produced a commit (empty diff vs base)",
        );
    }

    // ④.5 REAL base-freshness stamp (computed ONCE per run, after we know there is work to ship):
    // fetch origin main + count how far the spawn base is behind it, instead of fabricating
    // "base == current main (fresh)" — so the synthesizer's STALE BASE Hold can actually fire.
    let base_vs_main = base_vs_main_stamp(&git_root, &base_sha);

    // Choose the integration target + synthesis result. Two modes:
    //  • COMPETE (--compete, N>1): synth each committed arm on its OWN worktree, pick the objective
    //    winner, ship the winner (losing arms discarded by Drop). integ_target = winner's worktree.
    //  • FOLD-ALL (default): N=1 → the worker worktree IS the integration tree; N>1 → fold the
    //    committed branches into a fresh integration worktree (cleaned before return; degrade to the
    //    first committer if the fold can't assemble). Synthesize once over all committed panes.
    let mut integ_cleanup: Option<(String, PathBuf)> = None;
    let mut winner_note = String::new();
    let per_arm: Vec<ArmSummary>;
    let mut winner_harness: Option<String> = None;
    let (integ_target, synth): (PathBuf, Result<CoreOutcome, String>) = if opts.compete
        && committed.len() > 1
    {
        match compete_pick(
            &ctx,
            &tasks,
            &committed,
            &base_sha,
            &base_vs_main,
            &git_root,
            opts,
            hooks.synth_one_pass,
        ) {
            Some((wid, harness, wt, outcome, arms)) => {
                eprintln!(
                    "ade: compete — winner {harness} ({wid}) of {} arm(s)",
                    committed.len()
                );
                winner_note = format!(" [compete winner: {harness} ({wid})]");
                winner_harness = Some(harness.clone());
                per_arm = arms
                    .iter()
                    .map(|a| ArmSummary {
                        wid: a.wid.clone(),
                        harness: a.harness.clone(),
                        committed: a.committed,
                        verdict: if a.committed {
                            verdict_label(a.verdict).to_string()
                        } else {
                            "skipped".to_string()
                        },
                        diff_lines: a.diff_lines,
                        winner: a.wid == wid,
                    })
                    .collect();
                (wt, Ok(outcome))
            }
            None => {
                return ApplyOutcome::err(
                    "impl-exhausted",
                    2,
                    "compete: no arm produced a shippable result",
                )
            }
        }
    } else {
        let integ_target: PathBuf = if committed.len() == 1 {
            ctx.worktree_of(&committed[0])
                .expect("committed worktree")
                .to_path_buf()
        } else {
            let integ_id = format!("bridge-integ-{}", opts.run_id);
            match fold_workers(&git_root, &integ_id, &committed) {
                Some(f) => {
                    eprintln!(
                        "ade: folded {} pane(s) → integration tree ({} conflict(s), {} skipped)",
                        f.committed.len(),
                        f.conflicts,
                        f.skipped
                    );
                    integ_cleanup = Some((integ_id, f.worktree.clone()));
                    f.worktree
                }
                None => ctx
                    .worktree_of(&committed[0])
                    .expect("committed worktree")
                    .to_path_buf(),
            }
        };
        // SYNTHESIZE once against the integration tree. truth/docs are per committed pane.
        let truth: Vec<PaneTruth> = committed
            .iter()
            .map(|wid| {
                let wt = ctx.worktree_of(wid).expect("committed worktree");
                PaneTruth {
                    pane: wid.clone(),
                    worktree_found: true,
                    head: git_capture(wt, &["rev-parse", "HEAD"]).unwrap_or_default(),
                    base_vs_main: base_vs_main.clone(),
                    diff_stat: git_capture(wt, &["diff", "--stat", &format!("{base_sha}..HEAD")])
                        .unwrap_or_default(),
                    status: git_capture(wt, &["status", "--porcelain"]).unwrap_or_default(),
                }
            })
            .collect();
        let docs: Vec<(String, String)> = truth
            .iter()
            .map(|t| {
                (
                    t.pane.clone(),
                    format!(
                        "Worker {} committed on its branch.\n\n{}",
                        t.pane, t.diff_stat
                    ),
                )
            })
            .collect();
        let repo_target = RepoTarget {
            git_root: git_root.clone(),
            integ_target: Some(integ_target.clone()),
            refreshed_fold: None,
        };
        let synth_opts = SynthOpts {
            goal: &opts.goal,
            contributors: &docs,
            critique_on: opts.critique,
            report_only_allowed: true,
            retry_inadequate: true,
            review_on: opts.review,
            crap_on: opts.crap,
        };
        let synth = synthesize::synthesize_core(
            &docs,
            &truth,
            repo_target,
            synth_opts,
            hooks.synth_one_pass,
            &opts.out_dir,
        );
        // fold report: every pane — committed → "folded", else "skipped".
        per_arm = tasks
            .iter()
            .map(|pt| {
                let is_committed = committed.contains(&pt.wid);
                let diff_lines = ctx
                    .worktree_of(&pt.wid)
                    .map(|wt| {
                        count_diff_lines(
                            &git_capture(wt, &["diff", &base_sha, "HEAD"]).unwrap_or_default(),
                        )
                    })
                    .unwrap_or(0);
                ArmSummary {
                    wid: pt.wid.clone(),
                    harness: pt.harness.clone(),
                    committed: is_committed,
                    verdict: if is_committed {
                        "folded".to_string()
                    } else {
                        "skipped".to_string()
                    },
                    diff_lines,
                    winner: false,
                }
            })
            .collect();
        (integ_target, synth)
    };

    // Persist the patch artifact (the diff vs base of whatever we are shipping).
    let patch_path = opts.out_dir.join("consolidated.patch");
    let folded_diff = git_capture(&integ_target, &["diff", &base_sha, "HEAD"]).unwrap_or_default();
    let _ = std::fs::write(&patch_path, &folded_diff);

    // Build the outcome, THEN clean up the integration worktree (single exit) so every path cleans.
    let out = match synth {
        Err(e) => ApplyOutcome::err("error", 3, format!("synthesize failed: {e}")),
        Ok(outcome) => {
            let final_md = Some(outcome.write_path.clone());
            let usage = outcome.usage;
            match outcome.verdict {
                BridgeVerdict::Hold | BridgeVerdict::Reject => {
                    let v = if matches!(outcome.verdict, BridgeVerdict::Hold) {
                        "hold"
                    } else {
                        "reject"
                    };
                    ApplyOutcome {
                        verdict: v.to_string(),
                        exit_code: 3,
                        pr_url: None,
                        patch_path: Some(patch_path.clone()),
                        final_md,
                        usage,
                        detail: outcome.held_reason,
                        per_arm: Vec::new(),
                        winner_harness: None,
                    }
                }
                BridgeVerdict::Pass if !opts.pr => {
                    // --no-pr already emits the patch (the output). A budget overrun just gets a note;
                    // there is no PR to skip, so the exit stays 0.
                    let budget_note = if over_budget(&usage, opts.budget_tokens) {
                        format!(
                            " [token budget exceeded: used {} > budget {:?}]",
                            usage.input + usage.output,
                            opts.budget_tokens
                        )
                    } else {
                        String::new()
                    };
                    ApplyOutcome {
                        verdict: "pass".to_string(),
                        exit_code: 0,
                        pr_url: None,
                        patch_path: Some(patch_path.clone()),
                        final_md,
                        usage,
                        detail: format!(
                            "clean verdict — patch emitted (--no-pr){winner_note}{budget_note}"
                        ),
                        per_arm: Vec::new(),
                        winner_harness: None,
                    }
                }
                BridgeVerdict::Pass => {
                    // BUDGET-BEFORE-PR: a run that blew its controller token budget must NOT auto-open
                    // a PR (don't ship budget-overrun work unreviewed). Runs BEFORE any network → the
                    // patch is still emitted, but flywheel is never called.
                    if over_budget(&usage, opts.budget_tokens) {
                        ApplyOutcome {
                            verdict: "budget-exceeded".into(),
                            exit_code: 5,
                            pr_url: None,
                            patch_path: Some(patch_path.clone()),
                            final_md,
                            usage,
                            detail: format!(
                                "token budget exceeded (used {} > budget {:?}) — PR skipped, patch emitted",
                                usage.input + usage.output,
                                opts.budget_tokens
                            ),
                            per_arm: Vec::new(),
                            winner_harness: None,
                        }
                    } else {
                        // SECRET-SCAN-BEFORE-PR: refuse to push a diff carrying a secret. Runs BEFORE any
                        // network → a hit blocks here, flywheel is never called.
                        let secret_hits = scan_secrets(&folded_diff);
                        if !secret_hits.is_empty() {
                            ApplyOutcome {
                                verdict: "secret-blocked".to_string(),
                                exit_code: 3,
                                pr_url: None,
                                patch_path: Some(patch_path.clone()),
                                final_md,
                                usage,
                                detail: format!(
                                    "secret-scan blocked the PR (push refused): {secret_hits:?}"
                                ),
                                per_arm: Vec::new(),
                                winner_harness: None,
                            }
                        } else {
                            let branch = format!("ade/{}", opts.run_id);
                            let title = {
                                let g = opts.goal.trim();
                                let first = g.lines().next().unwrap_or(g);
                                // char-boundary-safe: a byte slice here panicked on multibyte goals.
                                truncate_chars(first, 70)
                            };
                            match crate::flywheel::flywheel_push_and_pr(
                                &opts.repo,
                                &integ_target,
                                &branch,
                                &outcome.write_path,
                                &title,
                            ) {
                                Ok(url) => ApplyOutcome {
                                    verdict: "pass".to_string(),
                                    exit_code: 0,
                                    pr_url: Some(url.clone()),
                                    patch_path: Some(patch_path.clone()),
                                    final_md,
                                    usage,
                                    detail: format!("PR opened: {url}{winner_note}"),
                                    per_arm: Vec::new(),
                                    winner_harness: None,
                                },
                                // NOT a clean pass: the synth verdict was Pass but the SHIP step failed.
                                // When the push succeeded and only `gh pr create` failed, the remote branch
                                // is left orphaned — record that explicitly instead of reporting "pass".
                                Err(e) => ApplyOutcome {
                                    verdict: "pr-failed".to_string(),
                                    exit_code: 4,
                                    pr_url: None,
                                    patch_path: Some(patch_path.clone()),
                                    final_md,
                                    usage,
                                    detail: if e.contains("gh pr create failed") {
                                        format!("pushed but PR creation failed: {e} (remote branch '{branch}' left orphaned)")
                                    } else {
                                        format!("PR step failed: {e}")
                                    },
                                    per_arm: Vec::new(),
                                    winner_harness: None,
                                },
                            }
                        }
                    }
                }
            }
        }
    };

    // Clean up the N>1 integration worktree (N=1 uses a ctx-owned worktree → Drop cleans it).
    if let Some((integ_id, path)) = integ_cleanup {
        let _ = crate::synthesize::fold_support::remove_worktree(&git_root, &integ_id, &path);
    }
    // Attach the per-arm fan-out report (table + verdict.json per_arm).
    let mut out = out;
    out.per_arm = per_arm;
    out.winner_harness = winner_harness;
    out
    // ctx drops here → worker worktrees + branches + manifest cleaned (RAII).
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let st = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .status()
            .unwrap();
        assert!(st.success(), "git {args:?} failed");
    }

    fn arm(
        harness: &str,
        wid: &str,
        committed: bool,
        verdict: BridgeVerdict,
        diff_lines: usize,
    ) -> ArmResult {
        ArmResult {
            harness: harness.into(),
            wid: wid.into(),
            committed,
            verdict,
            diff_lines,
        }
    }

    #[test]
    fn compete_pane_tasks_one_full_goal_arm_per_harness() {
        // regression: live smoke caught that compete spawned 1 arm (orchestrate decompose=false → 1
        // task). compete MUST build one full-goal arm per harness.
        let t = compete_pane_tasks(
            &["claude".into(), "claude".into(), "cursor".into()],
            "ship X",
        );
        assert_eq!(t.len(), 3, "one arm per harness");
        assert_eq!(t[0].wid, "w0");
        assert_eq!(t[2].wid, "w2");
        assert_eq!(t[2].harness, "cursor");
        assert!(
            t.iter().all(|p| p.prompt == "ship X"),
            "each arm runs the FULL goal verbatim"
        );
    }

    #[test]
    fn truncate_chars_is_char_boundary_safe() {
        // ASCII: unchanged under the limit, cut + ellipsis over it.
        assert_eq!(truncate_chars("short", 70), "short");
        let long = "a".repeat(80);
        assert_eq!(truncate_chars(&long, 70), format!("{}…", "a".repeat(70)));
        // Multibyte: byte 70 lands INSIDE a char for "é" (2 bytes each) — the old `&s[..70]`
        // byte-slice panicked here; this must cut at 70 CHARS cleanly.
        let multi = "é".repeat(80);
        assert_eq!(truncate_chars(&multi, 70), format!("{}…", "é".repeat(70)));
        // Exactly at the limit → unchanged (no ellipsis).
        assert_eq!(truncate_chars(&"é".repeat(70), 70), "é".repeat(70));
        assert_eq!(truncate_chars("", 70), "");
    }

    #[test]
    fn base_vs_main_stamp_reports_unverified_without_origin() {
        // A repo with NO origin remote: the fetch fails → honest "unverified" stamp that neither
        // fabricates freshness NOR trips the STALE BASE / base UNKNOWN Hold.
        let repo = seed_repo("stamp");
        let head = git_capture(&repo, &["rev-parse", "HEAD"]).unwrap();
        let stamp = base_vs_main_stamp(&repo, &head);
        assert_eq!(stamp, "origin unreachable; freshness unverified");
        assert!(!stamp.contains("STALE BASE") && !stamp.starts_with("base UNKNOWN"));
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn base_vs_main_stamp_counts_commits_behind_local_origin() {
        // origin = a local clone source; advance origin/main by one commit after cloning → the
        // stamp must say STALE BASE — 1 commit(s) behind origin/main (no network needed).
        let origin = seed_repo("stamp-origin");
        let clone_dir = std::env::temp_dir().join(format!(
            "ade-apply-spine-{}-stamp-clone",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&clone_dir);
        let st = std::process::Command::new("git")
            .args([
                "clone",
                "-q",
                &origin.to_string_lossy(),
                &clone_dir.to_string_lossy(),
            ])
            .status()
            .unwrap();
        assert!(st.success(), "clone failed");
        let base = git_capture(&clone_dir, &["rev-parse", "HEAD"]).unwrap();
        // fresh right after clone
        assert_eq!(
            base_vs_main_stamp(&clone_dir, &base),
            "base == current main (fresh)"
        );
        // advance origin main
        std::fs::write(origin.join("NEW.txt"), "x\n").unwrap();
        git(&origin, &["add", "-A"]);
        git(&origin, &["commit", "-qm", "advance"]);
        let stamp = base_vs_main_stamp(&clone_dir, &base);
        assert_eq!(
            stamp, "STALE BASE — 1 commit(s) behind origin/main",
            "got: {stamp}"
        );
        let _ = std::fs::remove_dir_all(&origin);
        let _ = std::fs::remove_dir_all(&clone_dir);
    }

    #[test]
    fn pick_winner_is_objective_and_deterministic() {
        // Pass beats Hold regardless of diff size.
        let arms = vec![
            arm("a", "w0", true, BridgeVerdict::Hold, 1),
            arm("b", "w1", true, BridgeVerdict::Pass, 999),
        ];
        assert_eq!(pick_winner(&arms), Some(1), "Pass wins over Hold");
        // Same verdict → smaller diff wins.
        let arms = vec![
            arm("a", "w0", true, BridgeVerdict::Pass, 50),
            arm("b", "w1", true, BridgeVerdict::Pass, 10),
        ];
        assert_eq!(pick_winner(&arms), Some(1), "smaller diff wins the tie");
        // Uncommitted arms are ineligible.
        let arms = vec![
            arm("a", "w0", false, BridgeVerdict::Pass, 1),
            arm("b", "w1", true, BridgeVerdict::Reject, 5),
        ];
        assert_eq!(
            pick_winner(&arms),
            Some(1),
            "only committed arms are eligible"
        );
        // Nothing committed → None.
        assert_eq!(
            pick_winner(&[arm("a", "w0", false, BridgeVerdict::Pass, 1)]),
            None
        );
        // Stable: first of equal minima.
        let arms = vec![
            arm("a", "w0", true, BridgeVerdict::Pass, 10),
            arm("b", "w1", true, BridgeVerdict::Pass, 10),
        ];
        assert_eq!(pick_winner(&arms), Some(0), "ties resolve to the first arm");
    }

    #[test]
    fn resolve_merge_validates_the_chosen_arm() {
        use serde_json::json;
        let v = json!({
            "run_id": "ade-123",
            "pr_url": "https://github.com/o/r/pull/7",
            "winner_harness": "claude",
            "per_arm": [
                {"harness": "claude", "committed": true, "winner": true},
                {"harness": "cursor", "committed": true, "winner": false},
            ],
        });
        // winner arm → Ok with the PR + branch
        let plan = resolve_merge(&v, "claude").unwrap();
        assert_eq!(plan.pr_url, "https://github.com/o/r/pull/7");
        assert_eq!(plan.branch, "ade/ade-123");
        // a non-winning arm → refused (only the shipped arm has the PR)
        assert!(resolve_merge(&v, "cursor").is_err(), "non-winner refused");
        // unknown harness → refused
        assert!(resolve_merge(&v, "codex").is_err());
        // no pr_url → refused
        let np = json!({"run_id": "x", "per_arm": [{"harness": "claude", "committed": true}]});
        assert!(resolve_merge(&np, "claude").is_err(), "no PR → refused");
        // uncommitted arm (fold/single, no winner_harness) → refused
        let nc = json!({"pr_url": "https://github.com/o/r/pull/1", "run_id": "x", "per_arm": [{"harness": "claude", "committed": false}]});
        assert!(
            resolve_merge(&nc, "claude").is_err(),
            "uncommitted → refused"
        );
        // fold/single (no winner_harness) with a committed arm → Ok
        let fold = json!({"pr_url": "https://github.com/o/r/pull/2", "run_id": "x", "per_arm": [{"harness": "claude", "committed": true}]});
        assert!(
            resolve_merge(&fold, "claude").is_ok(),
            "fold committed arm resolves"
        );
        // SECURITY: a flag-shaped / non-github pr_url is refused before it can reach `gh` (no argv smuggling)
        for bad in [
            "--squash",
            "-X",
            "https://evil.com/o/r/pull/1",
            "https://github.com/o/r/pull/1 --delete",
            "https://github.com/o/r/issues/1",
        ] {
            let v = json!({"pr_url": bad, "run_id": "x", "per_arm": [{"harness": "claude", "committed": true}]});
            assert!(
                resolve_merge(&v, "claude").is_err(),
                "rejects unsafe pr_url: {bad:?}"
            );
        }
    }

    #[test]
    fn scan_secrets_flags_high_signal_added_lines_only() {
        let diff = "\
diff --git a/k b/k
+-----BEGIN OPENSSH PRIVATE KEY-----
+token = ghp_0123456789abcdefABCDEF0123456789wxyz
+aws = AKIAIOSFODNN7EXAMPLE
+oai = sk-0123456789abcdefghijABCDEF
 context line ghp_should_be_ignored_not_added
-removed = AKIAIOSFODNN7EXAMPLE
+harmless = hello world
";
        let hits = scan_secrets(diff);
        assert!(
            hits.iter().any(|h| h.contains("private key")),
            "key header: {hits:?}"
        );
        assert!(
            hits.iter().any(|h| h.contains("GitHub")),
            "gh token: {hits:?}"
        );
        assert!(hits.iter().any(|h| h.contains("AWS")), "aws: {hits:?}");
        assert!(
            hits.iter().any(|h| h.contains("OpenAI")),
            "openai: {hits:?}"
        );
        // a clean diff → no hits; context/removed lines never counted
        assert!(
            scan_secrets("diff\n+let x = 1;\n-AKIAIOSFODNN7EXAMPLE\n context ghp_aaaa").is_empty()
        );
    }

    #[test]
    fn secret_in_diff_blocks_pr_before_any_network() {
        let repo = seed_repo("secret");
        let opts = ApplyOpts {
            run_id: "tsec".to_string(),
            goal: "leak a key".to_string(),
            repo: repo.clone(),
            base: "main".to_string(),
            harnesses: vec!["claude".to_string()],
            compete: false,
            fair: false,
            models: vec![],
            model: None,
            effort: "high".to_string(),
            timeout_secs: 60,
            write: true,
            critique: false,
            review: false,
            crap: false,
            pr: true, // PR path — but the secret blocks BEFORE flywheel touches the network
            budget_tokens: None,
            out_dir: repo.join(".ade-out"),
        };
        // worker commits a file carrying a GitHub token
        let worker_cmd = |_pt: &PaneTask, _wt: &std::path::Path| {
            let mut c = std::process::Command::new("sh");
            c.arg("-c").arg(
                "printf 'tok = ghp_0123456789abcdefABCDEF0123456789wxyz\\n' > LEAK.txt && \
                 git add LEAK.txt && git commit -qm 'leak'",
            );
            crate::worker::WorkerSpec {
                cmd: c,
                prompt_via_stdin: true,
                end_of_options_supported: false,
            }
        };
        let plan = || {
            Ok(vec![PaneTask {
                wid: "w0".into(),
                harness: "claude".into(),
                prompt: "x".into(),
            }])
        };
        let synth = |_p: &str| Ok(("# Synthesis [VERIFIED]".to_string(), ClaudeUsage::default()));
        let outcome = run_apply_with(
            &opts,
            &ApplyHooks {
                plan: &plan,
                worker_cmd: &worker_cmd,
                synth_one_pass: &synth,
            },
        );
        assert_eq!(
            outcome.exit_code, 3,
            "secret → blocked; detail={}",
            outcome.detail
        );
        assert_eq!(outcome.verdict, "secret-blocked");
        assert!(outcome.pr_url.is_none(), "no PR opened");
        assert!(
            outcome.detail.contains("GitHub"),
            "names the hit: {}",
            outcome.detail
        );
        assert!(
            !repo.join(".agent-teams-worktrees/w0").exists(),
            "worktree cleaned"
        );
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn apply_gate_disarms_under_ci_and_off_by_default() {
        // not requested → Ok(false)
        assert_eq!(apply_gate(false, false, false), Ok(false));
        assert_eq!(apply_gate(false, true, true), Ok(false));
        // requested under CI → refused (even with the opt-in)
        assert!(apply_gate(true, true, true).is_err(), "CI refuses --apply");
        assert!(apply_gate(true, true, false).is_err());
        // requested locally without opt-in → refused
        assert!(
            apply_gate(true, false, false).is_err(),
            "off by default locally"
        );
        // requested locally WITH opt-in → armed
        assert_eq!(apply_gate(true, false, true), Ok(true));
    }

    #[test]
    fn render_arm_table_aligns_and_marks_winner() {
        let arms = vec![
            ArmSummary {
                wid: "w0".into(),
                harness: "claude".into(),
                committed: true,
                verdict: "pass".into(),
                diff_lines: 3,
                winner: false,
            },
            ArmSummary {
                wid: "w1".into(),
                harness: "cursor".into(),
                committed: true,
                verdict: "pass".into(),
                diff_lines: 1,
                winner: true,
            },
        ];
        let t = render_arm_table(&arms);
        assert!(
            t.contains("harness") && t.contains("verdict") && t.contains("±loc"),
            "has a header"
        );
        assert!(
            t.contains("★ w1") || t.contains("★ w1 ".trim_end()),
            "winner row marked: {t}"
        );
        assert!(t.lines().count() == 3, "header + 2 rows");
        assert!(t.ends_with('\n'), "newline-terminated");
    }

    #[test]
    fn fair_refusals_lists_unpinned_harnesses() {
        let harnesses = vec![
            "claude".to_string(),
            "cursor".to_string(),
            "codex".to_string(),
        ];
        let models = vec![("claude".to_string(), "sonnet-4-6".to_string())];
        assert_eq!(
            fair_refusals(&harnesses, &models),
            vec!["cursor".to_string(), "codex".to_string()]
        );
        // all pinned → empty
        let models_all = vec![
            ("claude".to_string(), "m".to_string()),
            ("cursor".to_string(), "m".to_string()),
            ("codex".to_string(), "m".to_string()),
        ];
        assert!(fair_refusals(&harnesses, &models_all).is_empty());
    }

    #[test]
    fn fair_mode_refuses_unpinned_before_spawn() {
        let repo = seed_repo("fair");
        let opts = ApplyOpts {
            run_id: "tf".to_string(),
            goal: "g".to_string(),
            repo: repo.clone(),
            base: "main".to_string(),
            harnesses: vec!["claude".to_string(), "cursor".to_string()],
            compete: false,
            fair: true,
            models: vec![("claude".to_string(), "sonnet-4-6".to_string())], // cursor unpinned
            model: None,
            effort: "high".to_string(),
            timeout_secs: 60,
            write: true,
            critique: false,
            review: false,
            crap: false,
            pr: false,
            budget_tokens: None,
            out_dir: repo.join(".ade-out"),
        };
        // plan/worker must NEVER run — fair refuses first.
        let plan =
            || -> Result<Vec<PaneTask>, String> { panic!("plan must not run when fair refuses") };
        let worker_cmd = |_pt: &PaneTask, _wt: &std::path::Path| -> crate::worker::WorkerSpec {
            panic!("worker must not spawn")
        };
        let synth = |_p: &str| Ok((String::new(), ClaudeUsage::default()));
        let outcome = run_apply_with(
            &opts,
            &ApplyHooks {
                plan: &plan,
                worker_cmd: &worker_cmd,
                synth_one_pass: &synth,
            },
        );
        assert_eq!(outcome.exit_code, 1, "fair refusal → exit 1");
        assert!(
            outcome.detail.contains("cursor"),
            "names the unpinned harness"
        );
        let _ = std::fs::remove_dir_all(&repo);
    }

    // P4: the PRODUCTION dispatch routes to the real per-harness program — NOT always claude.
    // Pins the unknown-harness policy (claude fallback) explicitly. Pure: introspects the built
    // Command program; no spawn, no live binary.
    #[test]
    fn production_worker_cmd_dispatches_per_harness_not_always_claude() {
        let repo = std::path::Path::new("/tmp/ade-apply-dispatch");
        let wt = std::path::Path::new("/tmp/ade-apply-dispatch/.wt/w0");
        let cases = [
            ("claude", "claude"),
            ("cursor", "cursor-agent"),
            ("codex", "codex"),
            ("opencode", "opencode"),
            ("commandcode", "commandcode"),
            ("pi", "pi"),
            // unknown harness → claude fallback (preserves today's behavior, never a silent push path)
            ("totally-unknown", "claude"),
        ];
        for (harness, want_program) in cases {
            let pt = PaneTask {
                wid: "w0".into(),
                harness: harness.into(),
                prompt: "x".into(),
            };
            let spec = production_worker_spec(&pt, repo, Some("m"), true, wt);
            assert_eq!(
                spec.cmd.get_program().to_string_lossy(),
                want_program,
                "harness {harness} must dispatch to program {want_program}"
            );
            // claude + unknown → stdin; the three real non-claude harnesses → positional
            let want_stdin = matches!(harness, "claude" | "totally-unknown");
            assert_eq!(
                spec.prompt_via_stdin, want_stdin,
                "prompt mode for {harness}"
            );
        }
    }

    #[test]
    fn compete_picks_smaller_diff_winner_and_ships_it() {
        let repo = seed_repo("compete2");
        let opts = ApplyOpts {
            run_id: "tc".to_string(),
            goal: "competing arms".to_string(),
            repo: repo.clone(),
            base: "main".to_string(),
            harnesses: vec!["claude".to_string(), "claude".to_string()],
            compete: true,
            fair: false,
            models: vec![],
            model: None,
            effort: "high".to_string(),
            timeout_secs: 60,
            write: true,
            critique: false,
            review: false,
            crap: false,
            pr: false,
            budget_tokens: None,
            out_dir: repo.join(".ade-out"),
        };
        // w0 makes a BIG change (3 lines), w1 a SMALL one (1 line) to the SAME file → both Pass on the
        // always-green gate → pick_winner tie-breaks to the smaller diff (w1).
        let worker_cmd = |pt: &PaneTask, _wt: &std::path::Path| {
            let mut c = std::process::Command::new("sh");
            let body = if pt.wid == "w0" {
                "printf 'a\\nb\\nc\\n' > OUT.txt"
            } else {
                "printf 'x\\n' > OUT.txt"
            };
            c.arg("-c").arg(format!(
                "{body} && git add OUT.txt && git commit -qm '{}: out'",
                pt.wid
            ));
            crate::worker::WorkerSpec {
                cmd: c,
                prompt_via_stdin: true,
                end_of_options_supported: false,
            }
        };
        let plan = || {
            Ok(vec![
                PaneTask {
                    wid: "w0".into(),
                    harness: "claude".into(),
                    prompt: "full goal".into(),
                },
                PaneTask {
                    wid: "w1".into(),
                    harness: "claude".into(),
                    prompt: "full goal".into(),
                },
            ])
        };
        let synth = |_p: &str| {
            Ok((
                "# Synthesis\n\n[VERIFIED]".to_string(),
                ClaudeUsage::default(),
            ))
        };

        let outcome = run_apply_with(
            &opts,
            &ApplyHooks {
                plan: &plan,
                worker_cmd: &worker_cmd,
                synth_one_pass: &synth,
            },
        );
        assert_eq!(
            outcome.exit_code, 0,
            "compete winner ships clean; detail={}",
            outcome.detail
        );
        assert_eq!(outcome.verdict, "pass");
        assert!(
            outcome.detail.contains("compete winner"),
            "names a winner: {}",
            outcome.detail
        );
        assert!(
            outcome.detail.contains("w1"),
            "smaller-diff arm w1 wins; detail={}",
            outcome.detail
        );
        // per-arm report populated + winner flagged
        assert_eq!(outcome.per_arm.len(), 2, "both arms reported");
        assert_eq!(outcome.winner_harness.as_deref(), Some("claude"));
        assert!(
            outcome.per_arm.iter().any(|a| a.wid == "w1" && a.winner),
            "w1 flagged winner"
        );
        // shipped patch is the WINNER's (1 line), not the loser's 3-line change
        let patch = std::fs::read_to_string(outcome.patch_path.expect("patch")).unwrap();
        assert!(patch.contains("+x"), "winner's content shipped");
        assert!(
            !repo.join(".agent-teams-worktrees/w0").exists(),
            "w0 cleaned"
        );
        assert!(
            !repo.join(".agent-teams-worktrees/w1").exists(),
            "w1 cleaned"
        );
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn fold_workers_merges_two_worker_branches() {
        use crate::synthesize::fold_support;
        let repo = seed_repo("fold2");
        // two isolated worker worktrees, each commits a DISTINCT file on its agent-teams/<wid> branch
        for (wid, file) in [("w0", "A.txt"), ("w1", "B.txt")] {
            let wt = fold_support::add_worktree(&repo, wid).expect("worktree");
            std::fs::write(wt.root.join(file), format!("from {wid}\n")).unwrap();
            git(&wt.root, &["add", "-A"]);
            git(&wt.root, &["commit", "-qm", &format!("{wid}: add {file}")]);
        }
        let res = fold_workers(&repo, "integ-fold2", &["w0".to_string(), "w1".to_string()])
            .expect("fold produced an integration tree");
        assert_eq!(res.committed.len(), 2, "both panes committed");
        assert_eq!(res.conflicts, 0, "disjoint files → no conflict");
        assert!(res.worktree.join("A.txt").exists(), "w0's file folded in");
        assert!(res.worktree.join("B.txt").exists(), "w1's file folded in");
        // cleanup: integ worktree + the two worker worktrees/branches
        let _ = fold_support::remove_worktree(&repo, "integ-fold2", &res.worktree);
        for wid in ["w0", "w1"] {
            let _ = fold_support::remove_worktree(
                &repo,
                wid,
                &repo.join(".agent-teams-worktrees").join(wid),
            );
        }
        let _ = std::fs::remove_dir_all(&repo);
    }

    /// Seed a throwaway git repo with a passing deterministic test gate (no cargo, no network).
    /// `tag` keeps parallel tests on disjoint paths.
    fn seed_repo(tag: &str) -> PathBuf {
        let nonce = std::process::id();
        let dir = std::env::temp_dir().join(format!("ade-apply-spine-{nonce}-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("README.md"), "seed\n").unwrap();
        // A bridge-tests.json "commands" gate that always passes → deterministic Pass verdict.
        std::fs::write(dir.join("bridge-tests.json"), r#"{"commands":[["true"]]}"#).unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-qm", "seed"]);
        dir
    }

    #[test]
    fn spine_drives_goal_to_patch_hermetically() {
        let repo = seed_repo("patch");
        let out_dir = repo.join(".ade-out");
        let opts = ApplyOpts {
            run_id: "t0".to_string(),
            goal: "add a greeting file".to_string(),
            repo: repo.clone(),
            base: "main".to_string(),
            harnesses: vec!["claude".to_string()],
            model: None,
            compete: false,
            fair: false,
            models: vec![],
            effort: "high".to_string(),
            timeout_secs: 60,
            write: true,
            critique: false,
            review: false,
            crap: false,
            pr: false, // --no-pr → patch path, no network
            budget_tokens: None,
            out_dir: out_dir.clone(),
        };

        // Mock worker: a sh script that writes a file + commits in its cwd (the isolated worktree).
        let worker_cmd = |_pt: &PaneTask, _wt: &std::path::Path| {
            let mut c = std::process::Command::new("sh");
            c.arg("-c").arg(
                "printf 'hello from worker\\n' > GREETING.txt && \
                 git add GREETING.txt && \
                 git commit -qm 'worker: add GREETING.txt'",
            );
            crate::worker::WorkerSpec {
                cmd: c,
                prompt_via_stdin: true,
                end_of_options_supported: false,
            }
        };
        let plan = || {
            Ok(vec![PaneTask {
                wid: "w0".into(),
                harness: "claude".into(),
                prompt: "add a greeting file".into(),
            }])
        };
        let synth = |_prompt: &str| {
            Ok((
                "# Synthesis\n\nWorker added GREETING.txt. [VERIFIED]".to_string(),
                ClaudeUsage::default(),
            ))
        };

        let outcome = run_apply_with(
            &opts,
            &ApplyHooks {
                plan: &plan,
                worker_cmd: &worker_cmd,
                synth_one_pass: &synth,
            },
        );

        assert_eq!(
            outcome.exit_code, 0,
            "expected clean patch exit; detail={}",
            outcome.detail
        );
        assert_eq!(outcome.verdict, "pass", "detail={}", outcome.detail);
        assert!(outcome.pr_url.is_none(), "no PR in --no-pr mode");
        // artifacts written
        let patch = outcome.patch_path.expect("patch path");
        assert!(patch.is_file(), "consolidated.patch written");
        let patch_body = std::fs::read_to_string(&patch).unwrap();
        assert!(
            patch_body.contains("GREETING.txt"),
            "patch shows the worker change"
        );
        assert!(outcome.final_md.unwrap().is_file(), "final.md written");
        // Drop ran → no orphan worktree left behind.
        assert!(
            !repo.join(".agent-teams-worktrees/w0").exists(),
            "worktree cleaned by Drop"
        );

        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn spine_reports_impl_exhausted_when_worker_makes_no_commit() {
        let repo = seed_repo("noop");
        let out_dir = repo.join(".ade-out");
        let opts = ApplyOpts {
            run_id: "t1".to_string(),
            goal: "do nothing".to_string(),
            repo: repo.clone(),
            base: "main".to_string(),
            harnesses: vec!["claude".to_string()],
            model: None,
            compete: false,
            fair: false,
            models: vec![],
            effort: "high".to_string(),
            timeout_secs: 60,
            write: true,
            critique: false,
            review: false,
            crap: false,
            pr: false,
            budget_tokens: None,
            out_dir,
        };
        // Worker that exits without committing.
        let worker_cmd = |_pt: &PaneTask, _wt: &std::path::Path| {
            let mut c = std::process::Command::new("sh");
            c.arg("-c").arg("true");
            crate::worker::WorkerSpec {
                cmd: c,
                prompt_via_stdin: true,
                end_of_options_supported: false,
            }
        };
        let plan = || {
            Ok(vec![PaneTask {
                wid: "w0".into(),
                harness: "claude".into(),
                prompt: "do nothing".into(),
            }])
        };
        let synth = |_p: &str| Ok((String::new(), ClaudeUsage::default()));
        let outcome = run_apply_with(
            &opts,
            &ApplyHooks {
                plan: &plan,
                worker_cmd: &worker_cmd,
                synth_one_pass: &synth,
            },
        );
        assert_eq!(
            outcome.exit_code, 2,
            "no commit → impl exhausted; detail={}",
            outcome.detail
        );
        assert_eq!(outcome.verdict, "impl-exhausted");
        assert!(
            !repo.join(".agent-teams-worktrees/w0").exists(),
            "worktree cleaned by Drop"
        );
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn autocommit_net_commits_a_worker_that_edited_but_did_not_commit() {
        // Live-verified gap: a non-claude worker (cursor) edited its worktree + passed cargo test but
        // never `git commit`ed → false impl-exhausted. The controller auto-commit net must stage +
        // commit the leftover change so the fold sees a diff and the run reaches a verdict.
        let repo = seed_repo("autocommit");
        let out_dir = repo.join(".ade-out");
        let opts = ApplyOpts {
            run_id: "tac".to_string(),
            goal: "leave an uncommitted edit".to_string(),
            repo: repo.clone(),
            base: "main".to_string(),
            harnesses: vec!["cursor".to_string()],
            model: None,
            compete: false,
            fair: false,
            models: vec![],
            effort: "high".to_string(),
            timeout_secs: 60,
            write: true,
            critique: false,
            review: false,
            crap: false,
            pr: false,
            budget_tokens: None,
            out_dir,
        };
        // Mock worker EDITS the worktree but deliberately does NOT git add/commit (the cursor case).
        let worker_cmd = |_pt: &PaneTask, _wt: &std::path::Path| {
            let mut c = std::process::Command::new("sh");
            c.arg("-c")
                .arg("printf 'edited but not committed\\n' > LEFTOVER.txt");
            crate::worker::WorkerSpec {
                cmd: c,
                prompt_via_stdin: false,
                end_of_options_supported: false,
            }
        };
        let plan = || {
            Ok(vec![PaneTask {
                wid: "w0".into(),
                harness: "cursor".into(),
                prompt: "edit".into(),
            }])
        };
        let synth = |_p: &str| {
            Ok((
                "# Synthesis\n\nWorker added LEFTOVER.txt. [VERIFIED]".to_string(),
                ClaudeUsage::default(),
            ))
        };
        let outcome = run_apply_with(
            &opts,
            &ApplyHooks {
                plan: &plan,
                worker_cmd: &worker_cmd,
                synth_one_pass: &synth,
            },
        );
        assert_eq!(
            outcome.exit_code, 0,
            "auto-commit net should make the edit foldable; detail={}",
            outcome.detail
        );
        assert_eq!(outcome.verdict, "pass", "detail={}", outcome.detail);
        let patch = outcome.patch_path.expect("patch path");
        let body = std::fs::read_to_string(&patch).unwrap();
        assert!(
            body.contains("LEFTOVER.txt"),
            "patch shows the auto-committed worker change"
        );
        assert!(
            !repo.join(".agent-teams-worktrees/w0").exists(),
            "worktree cleaned by Drop"
        );
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn spine_folds_two_workers_end_to_end() {
        let repo = seed_repo("e2e2");
        let out_dir = repo.join(".ade-out");
        let opts = ApplyOpts {
            run_id: "t2".to_string(),
            goal: "two-pane fan-out".to_string(),
            repo: repo.clone(),
            base: "main".to_string(),
            harnesses: vec!["claude".to_string(), "claude".to_string()],
            model: None,
            compete: false,
            fair: false,
            models: vec![],
            effort: "high".to_string(),
            timeout_secs: 60,
            write: true,
            critique: false,
            review: false,
            crap: false,
            pr: false, // --no-pr → patch, no network
            budget_tokens: None,
            out_dir,
        };
        // Each pane writes a DISTINCT file keyed off its wid → disjoint → fold has no conflict.
        let worker_cmd = |pt: &PaneTask, _wt: &std::path::Path| {
            let mut c = std::process::Command::new("sh");
            c.arg("-c").arg(format!(
                "printf 'from {0}\\n' > {0}.txt && git add {0}.txt && git commit -qm '{0}: add file'",
                pt.wid
            ));
            crate::worker::WorkerSpec {
                cmd: c,
                prompt_via_stdin: true,
                end_of_options_supported: false,
            }
        };
        let plan = || {
            Ok(vec![
                PaneTask {
                    wid: "w0".into(),
                    harness: "claude".into(),
                    prompt: "part A".into(),
                },
                PaneTask {
                    wid: "w1".into(),
                    harness: "claude".into(),
                    prompt: "part B".into(),
                },
            ])
        };
        let synth = |_p: &str| {
            Ok((
                "# Synthesis\n\nFolded w0 + w1. [VERIFIED]".to_string(),
                ClaudeUsage::default(),
            ))
        };

        let outcome = run_apply_with(
            &opts,
            &ApplyHooks {
                plan: &plan,
                worker_cmd: &worker_cmd,
                synth_one_pass: &synth,
            },
        );

        assert_eq!(
            outcome.exit_code, 0,
            "two-worker fold → clean patch; detail={}",
            outcome.detail
        );
        assert_eq!(outcome.verdict, "pass", "detail={}", outcome.detail);
        let patch = std::fs::read_to_string(outcome.patch_path.expect("patch")).unwrap();
        assert!(patch.contains("w0.txt"), "folded patch shows w0's file");
        assert!(patch.contains("w1.txt"), "folded patch shows w1's file");
        // worker worktrees cleaned by Drop; integ worktree cleaned explicitly
        assert!(
            !repo.join(".agent-teams-worktrees/w0").exists(),
            "w0 worktree cleaned"
        );
        assert!(
            !repo.join(".agent-teams-worktrees/w1").exists(),
            "w1 worktree cleaned"
        );
        assert!(
            !repo.join(".agent-teams-worktrees/bridge-integ-t2").exists(),
            "integ worktree cleaned"
        );
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn over_budget_table() {
        let usage = |i: u64, o: u64| ClaudeUsage {
            input: i,
            output: o,
            ..Default::default()
        };
        // No budget → never over.
        assert!(!over_budget(&usage(1_000, 1_000), None));
        // Under budget → false.
        assert!(!over_budget(&usage(40, 40), Some(100)));
        // Exactly at budget → allowed (strict >).
        assert!(!over_budget(&usage(60, 40), Some(100)));
        // Over budget → true.
        assert!(over_budget(&usage(60, 41), Some(100)));
    }

    #[test]
    fn spine_skips_pr_when_over_budget() {
        // Mirrors spine_drives_goal_to_patch but with pr:true + a budget of 1 and a synth that meters
        // >1 token → the PR is skipped (no network), exit 5, patch still emitted.
        let repo = seed_repo("budget");
        let out_dir = repo.join(".ade-out");
        let opts = ApplyOpts {
            run_id: "tb".to_string(),
            goal: "ship over budget".to_string(),
            repo: repo.clone(),
            base: "main".to_string(),
            harnesses: vec!["claude".to_string()],
            model: None,
            compete: false,
            fair: false,
            models: vec![],
            effort: "high".to_string(),
            timeout_secs: 60,
            write: true,
            critique: false,
            review: false,
            crap: false,
            pr: true, // PR path — but the budget overrun blocks BEFORE flywheel touches the network
            budget_tokens: Some(1),
            out_dir,
        };
        let worker_cmd = |_pt: &PaneTask, _wt: &std::path::Path| {
            let mut c = std::process::Command::new("sh");
            c.arg("-c").arg(
                "printf 'hello\\n' > GREETING.txt && git add GREETING.txt && git commit -qm 'worker: add'",
            );
            crate::worker::WorkerSpec {
                cmd: c,
                prompt_via_stdin: true,
                end_of_options_supported: false,
            }
        };
        let plan = || {
            Ok(vec![PaneTask {
                wid: "w0".into(),
                harness: "claude".into(),
                prompt: "x".into(),
            }])
        };
        // Synth meters 10 input tokens (> budget of 1) → over budget. The doc must be ADEQUATE (≥200
        // chars + a heading) so synthesize_core keeps the metered usage instead of the zero-usage
        // fallback that fires when the synthesizer fails to produce a real document.
        let synth = |_p: &str| {
            let doc = format!(
                "# Synthesis\n\n{}\n\nWorker added GREETING.txt. [VERIFIED]",
                "z".repeat(250)
            );
            Ok((
                doc,
                ClaudeUsage {
                    input: 10,
                    output: 0,
                    ..Default::default()
                },
            ))
        };
        let outcome = run_apply_with(
            &opts,
            &ApplyHooks {
                plan: &plan,
                worker_cmd: &worker_cmd,
                synth_one_pass: &synth,
            },
        );
        assert_eq!(
            outcome.exit_code, 5,
            "over budget → exit 5; detail={}",
            outcome.detail
        );
        assert_eq!(outcome.verdict, "budget-exceeded");
        assert!(outcome.pr_url.is_none(), "no PR opened when over budget");
        assert!(
            outcome
                .patch_path
                .as_ref()
                .map(|p| p.is_file())
                .unwrap_or(false),
            "patch still emitted"
        );
        assert!(
            outcome.detail.contains("budget exceeded"),
            "detail explains: {}",
            outcome.detail
        );
        assert!(
            !repo.join(".agent-teams-worktrees/w0").exists(),
            "worktree cleaned"
        );
        let _ = std::fs::remove_dir_all(&repo);
    }
}
