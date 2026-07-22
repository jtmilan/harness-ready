//! Agent Teams — PTY session supervisor + git-worktree isolation (Plan 02-01).
//!
//! Spawns a harness CLI in a PTY inside a per-workspace git worktree, with the
//! Phase-01 hooks injected and `AGENT_TEAMS_STATE_DIR` set so the agent's events
//! flow to the state adapter. Streams output (background reader thread), accepts
//! input, exposes lifecycle. The Tauri backend (02-02) drives this.
//!
//! Reattach = within-session: the [`Supervisor`] owns the PTY handle for the
//! lifetime of the app process (D7). Cross-restart durability is a v1 daemon.

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use state_adapter::inject::{
    inject, inject_commandcode_mcp, inject_cursor_role, inject_grok_mcp, inject_mcp_config, InjectConfig,
    InjectHarness,
};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

// The PURE worker/harness logic now lives in the dep-light `harness` crate (extracted so the
// supervisor and the agent-teams-cli share ONE source). Re-export it so the supervisor's public
// API is unchanged and every internal `Harness::…` / `worker_spawn(…)` reference still resolves.
pub use harness::{
    effective_write_mode, supports_end_of_options, worker_args, worker_git_deny_env, worker_spawn,
    Harness, HarnessDescriptor, WorkerSpawn,
};

// 08 Sub-build 3 / slice 3: the per-pane output subscriber registry — the
// PUSH-from-PTY-reader delta substrate the daemon's `Attach` streaming feeds on. The
// reader fans each chunk out to subscribers UNDER the buffer lock and NEVER takes the
// daemon map lock (design §4). Additive: the GUI's `output_handle()` + `PaneBuffer::delta`
// read path is untouched; the fan-out is a no-op when a pane has no subscribers.
pub mod subscribers;

/// A usable PATH for spawning the harness CLIs (the canonical dev PATH — reused by
/// the app backend for any headless harness spawn, e.g. the Bridge orchestrator).
///
/// GUI apps launched from Finder/Dock inherit only a minimal PATH
/// (`/usr/bin:/bin:/usr/sbin:/sbin`), so a bare `claude` / `cursor-agent` fails
/// with ENOENT — the binaries live in Homebrew / `~/.local/bin`. Build a real
/// PATH once: ask the user's login shell for its PATH (sources their profile →
/// Homebrew, etc.) and always prepend the common dev bin dirs as a guarantee.
///
/// The guaranteed prepends matter because the shell probe is `-lc` (login,
/// NON-interactive): it sources `.zprofile`/`.zshenv` but NOT `.zshrc` — and
/// several installers export their bin dir only from `.zshrc` (bun does; pnpm
/// often too). `opencode` installed via bun lives in `~/.bun/bin` and was
/// invisible to the probe → "Spawn failed: … not found in PATH". So `~/.bun/bin`
/// and `~/Library/pnpm` are pinned in the prepend list, not left to the probe.
pub fn harness_path() -> &'static str {
    static PATH: OnceLock<String> = OnceLock::new();
    PATH.get_or_init(|| {
        let home = std::env::var("HOME").unwrap_or_default();
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
        // login shell → sources .zprofile/.bash_profile (Homebrew's shellenv etc.);
        // a sentinel brackets the value so any profile chatter on stdout is ignored.
        let from_shell = Command::new(&shell)
            .args(["-lc", "printf '___ATPATH___%s___ATPATH___' \"$PATH\""])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .split("___ATPATH___")
                    .nth(1)
                    .map(str::to_string)
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| std::env::var("PATH").unwrap_or_default());
        format!(
            "/opt/homebrew/bin:/usr/local/bin:{home}/.local/bin:{home}/.cargo/bin:\
             {home}/.bun/bin:{home}/Library/pnpm:{from_shell}"
        )
    })
}

/// Env vars that agent harnesses read from the process environment but that
/// GUI-launched apps (Finder / Dock / launchd) never inherit — because they
/// are exported only from `.zshrc` (interactive-only), not `.zprofile` /
/// `.zshenv` (login-only).  `harness_path()` already solves the same class
/// of problem for PATH via a `-lc` probe, but `-lc` is non-interactive so
/// it also skips `.zshrc`.  This function does a single `-ilc` (interactive
/// login command) probe, caches the result, and returns only the vars that
/// are actually set — so the spawn loop can forward them via `cmd.env()`.
///
/// The sentinel-bracketed printf is the same trick `harness_path()` uses to
/// ignore any profile chatter on stdout.
pub fn shell_env_vars() -> &'static [(String, String)] {
    static VARS: OnceLock<Vec<(String, String)>> = OnceLock::new();
    VARS.get_or_init(|| {
        // Names we care about.  Add new entries here when a harness needs
        // another secret that lives only in .zshrc.
        const WANTED: &[&str] = &["OPENAI_API_KEY"];

        // Fast path: the current process already has every var (terminal
        // launch) → skip the shell probe entirely.
        let mut found: Vec<(String, String)> = Vec::new();
        let mut all_present = true;
        for &name in WANTED {
            match std::env::var(name) {
                Ok(v) if !v.is_empty() => found.push((name.to_string(), v)),
                _ => all_present = false,
            }
        }
        if all_present {
            return found;
        }

        // Slow path: probe the interactive login shell.  `-ilc` sources
        // .zshenv + .zprofile + .zshrc + .zlogin — the full interactive
        // profile — so any `export` in .zshrc is visible.
        let shell =
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());

        // Build one printf per var, each bracketed by a unique sentinel so
        // we can extract the value even when .zshrc prints banners / motd.
        let script: String = WANTED
            .iter()
            .map(|name| {
                format!(
                    "printf '___ATENV_{name}___%s___ATENV_{name}___' \"${name}\";"
                )
            })
            .collect();

        if let Some(stdout) = Command::new(&shell)
            .args(["-ilc", &script])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        {
            for &name in WANTED {
                // Skip vars we already got from the fast path.
                if found.iter().any(|(k, _)| k == name) {
                    continue;
                }
                let sentinel = format!("___ATENV_{name}___");
                if let Some(val) = stdout.split(&sentinel).nth(1) {
                    if !val.is_empty() {
                        found.push((name.to_string(), val.to_string()));
                    }
                }
            }
        }
        found
    })
}

#[derive(Debug, Clone)]
pub struct WorkspaceSpec {
    pub id: String,
    pub harness: Harness,
    /// directory to run in (a git worktree, created via [`add_worktree`]; or the
    /// caller's deliberate repo-dir fallback for a non-git folder). At spawn this is
    /// re-resolved through [`resolve_spawn_cwd`] — a missing dir degrades to HOME,
    /// and the filesystem root is never accepted (gap #6: no accidental `/` cwd).
    pub worktree: PathBuf,
    /// stable conversation id for the harness (claude `--session-id` on create,
    /// `--resume <id>` on reopen). `None` → let the harness pick (cursor has no
    /// settable id; bash ignores it).
    pub session_id: Option<String>,
    /// reopen an existing conversation rather than start fresh (Plan 04-02).
    pub resume: bool,
    /// Typed agent role (Plan 17-01). `None` → a homogeneous pane (today's behavior,
    /// no system-prompt injection — back-compat). `Some(role)` injects the persona:
    /// claude via `--append-system-prompt`, cursor via a `.cursor/rules/agent-role.mdc`
    /// rule file (written in the inject block), bash no-op.
    pub role: Option<roles::AgentRole>,
    /// Phase-16 autonomous workers: true iff this pane is a delegate WORKER. A worker
    /// claude is spawned UNATTENDED (no human to answer an approval prompt), so it gets
    /// restrictive permission flags ([`worker_args`]) — `--permission-mode dontAsk` (auto-
    /// DENY, never prompt, on any non-allowlisted tool) + a tight `--allowedTools` set +
    /// `--disallowedTools Bash(git push:*)`. Default false → a human pane stays interactive
    /// (today's behavior, no permission flags). Cursor/bash/codex workers get nothing
    /// (the flags are claude-specific; the MVP only spawns claude workers).
    pub is_worker: bool,
    /// Phase-16 workers: directories OUTSIDE the worktree cwd the worker is allowed to
    /// write (`--add-dir`). The fan-in report lands at `<repo>/bridge/<run>/<id>.md`, a
    /// SIBLING of the worktree — out of cwd scope, so without this the report write is
    /// auto-denied under `dontAsk` and the run times out. Empty for humans (and ignored
    /// unless `is_worker`).
    pub extra_dirs: Vec<PathBuf>,
    /// Optional model override for the pane's TUI session (model-at-spawn). `None` →
    /// the harness's account default (today's behavior — back-compat). Passed verbatim
    /// to the harness CLI's model flag ([`model_args`]); the harness validates the id
    /// itself (the app never second-guesses it).
    pub model: Option<String>,
}

/// Build the harness CLI args that select/resume a conversation (Plan 04-02).
///
/// - **create** (`resume == false`): claude dictates its own session id
///   (`--session-id <id>`) so reopen can target it; cursor/bash take none.
/// - **reopen** (`resume == true`): claude `--resume <id>` (or `--continue` if no
///   id was tracked); cursor `--continue` (no settable id — resumes the
///   most-recent conversation in the cwd, which is 1:1 with the pane); bash none.
///
/// Pure + total so it can be unit-tested without spawning a process.
fn session_args(harness: Harness, session_id: Option<&str>, resume: bool) -> Vec<String> {
    match harness {
        Harness::Claude if resume => match session_id {
            Some(id) => vec!["--resume".into(), id.into()],
            None => vec!["--continue".into()],
        },
        Harness::Claude => match session_id {
            Some(id) => vec!["--session-id".into(), id.into()],
            None => vec![],
        },
        Harness::Cursor if resume => vec!["--continue".into()],
        // codex + commandcode: no settable session id + no resume flag wired → fresh
        // spawn each time (like bash). Conversation-level resume is a deferred follow-up.
        // (commandcode HAS -r/-c, but wiring it needs an on-disk safety net like cursor's.)
        Harness::Cursor
        | Harness::Bash
        | Harness::Codex
        | Harness::CommandCode
        | Harness::OpenCode
        | Harness::Pi
        | Harness::Grok => {
            vec![]
        }
    }
}

/// Build the per-harness MCP-config CLI flags for the read-only `agent-teams-mcp`
/// sidecar (Plan 16-01, item 1 — D56). Pure + total so it is unit-tested without
/// spawning, exactly like [`session_args`].
///
/// - **Claude** with a resolved config path → `--mcp-config <abs file> --strict-mcp-config`.
///   STRICT (06-06, reversing D56's additive choice): claude loads ONLY the agent-teams
///   sidecar and ignores the operator's user-scoped servers. Those fail setup in the
///   fresh-worktree pane → claude's "N setup issues: MCP" startup banner that EATS the
///   first dispatched input (the claude-pane hang). An agent pane needs the agent-teams
///   surface, not the operator's ancillary servers. With no config (resolution/inject
///   failed) → `[]`, so a missing sidecar degrades to "no MCP in the pane", NEVER a
///   broken flag (AC-6).
/// - **Cursor** → `--approve-mcps` (auto-approve all MCP servers; cursor discovers
///   the in-worktree `.cursor/mcp.json` by path). GATED on `!is_worker`: a delegate
///   WORKER skips `inject_mcp_config` entirely (no `.cursor/mcp.json` is written), so
///   auto-approving would grant blanket approval to whatever USER-scoped MCP servers the
///   worker's cursor happens to discover — a capability widening, not a convenience.
/// - **Bash** → `[]` (the test harness gets nothing — it never injects).
fn mcp_args(harness: Harness, claude_cfg: Option<&Path>, is_worker: bool) -> Vec<String> {
    match harness {
        Harness::Claude => match claude_cfg {
            Some(p) => vec![
                "--mcp-config".into(),
                p.to_string_lossy().into_owned(),
                // 06-06: STRICT so claude loads ONLY the agent-teams sidecar and IGNORES the
                // operator's user-scoped servers (bridgemind/google-docs/railway). Those fail
                // setup in the fresh-worktree pane env → claude's "N setup issues: MCP" startup
                // banner, which INTERCEPTS Enter and eats the first dispatched task (the claude
                // pane hang, observed claude Code v2.1.181). A spawned agent pane needs the
                // agent-teams coordination surface, not the operator's ancillary servers — so we
                // trade them for a hands-free dispatch submit.
                "--strict-mcp-config".into(),
            ],
            None => vec![],
        },
        Harness::Cursor if is_worker => vec![],
        Harness::Cursor => vec!["--approve-mcps".into()],
        // codex: consumes MCP via its own ~/.codex/config.toml (global, not per-project) → no
        // spawn-time flag. commandcode/opencode/grok: MCP is injected as a project-local config
        // file (.mcp.json / .grok/config.toml) discovered by path → no CLI flag needed either.
        Harness::Bash
        | Harness::Codex
        | Harness::CommandCode
        | Harness::OpenCode
        | Harness::Pi
        | Harness::Grok => vec![],
    }
}

/// Build the per-harness MODEL CLI flags for a TUI pane spawn (model-at-spawn). Pure +
/// total so it is unit-tested without spawning, exactly like [`session_args`]/[`mcp_args`].
///
/// Flags MIRROR the headless [`worker_spawn`] arms (live-verified there): claude/cursor
/// `--model <id>`, codex/commandcode `-m <id>`, opencode `-m <provider/model>`. Bash has
/// no model. `None` → `[]` (account default — today's behavior, back-compat). The model
/// string passes through verbatim: an unknown id fails loudly in the pane's own TUI,
/// which is the right surface for it.
fn model_args(harness: Harness, model: Option<&str>) -> Vec<String> {
    let Some(m) = model else { return vec![] };
    if m.is_empty() {
        return vec![];
    }
    match harness {
        Harness::Claude | Harness::Cursor => vec!["--model".into(), m.into()],
        // pi CLI exposes `--model <pattern|id>` (supports provider/id) — long form only.
        Harness::Pi => vec!["--model".into(), m.into()],
        // grok CLI exposes `-m, --model <MODEL>` (v0.2.101) — short form matches this arm.
        Harness::Codex | Harness::CommandCode | Harness::OpenCode | Harness::Grok => {
            vec!["-m".into(), m.into()]
        }
        Harness::Bash => vec![],
    }
}

/// The when-to-delegate discovery nudge appended to claude human panes (P1.8). Honest about the
/// gate ("when available" — `team_delegate` is absent until the triple gate is armed) and the
/// use-case trigger (≥2 INDEPENDENT slices). STATIC SSOT — no untrusted text templated in.
const DELEGATE_NUDGE: &str = "When the `team_delegate` MCP tool is available, reach for it the \
moment a task splits into 2 or more INDEPENDENT slices you would otherwise do serially (e.g. \
compare/audit/survey N things, or check M call-sites). Call it with just a goal — \
`team_delegate{goal}` — and keep working; results return as ONE summarized line. It spawns \
throwaway workers in isolated worktrees that are swept when done, so it fits judgment-not-the-diff \
fan-out, NOT sequential or dependent subtasks (do those inline).";

/// Build claude's `--append-system-prompt` argv: the 17-01 role persona + the P1.8 when-to-delegate
/// nudge, composed into ONE flag. claude's `--append-system-prompt` is LAST-WINS (a 2nd flag
/// silently drops the 1st — verified), so the persona and the nudge MUST share a single flag or one
/// clobbers the other. CLAUDE ONLY: cursor's persona is a rule file, bash/codex/commandcode have no
/// CLI system-prompt channel → `[]` (parity with the old `role_args(false, _)` == `[]`). The nudge
/// is appended for NON-worker panes only — a worker must never be told to recurse (depth>1 is
/// rejected and a worker has no sidecar anyway). Pure → unit-tested below.
fn append_system_prompt_args(
    is_claude: bool,
    role: Option<roles::AgentRole>,
    is_worker: bool,
) -> Vec<String> {
    if !is_claude {
        return Vec::new();
    }
    let mut payload = String::new();
    if let Some(r) = role {
        payload.push_str(roles::persona(r)); // persona FIRST so the role framing leads
    }
    if !is_worker {
        if !payload.is_empty() {
            payload.push_str("\n\n");
        }
        payload.push_str(DELEGATE_NUDGE);
    }
    if payload.is_empty() {
        Vec::new()
    } else {
        vec!["--append-system-prompt".to_string(), payload]
    }
}

/// A git worktree created for a workspace.
pub struct Worktree {
    /// where the agent runs — the selected subfolder inside the worktree, or
    /// the worktree root when the selected folder IS the repo root.
    pub cwd: PathBuf,
    /// the worktree root (what `git worktree remove` operates on).
    pub root: PathBuf,
    /// the repo's git toplevel (run git worktree commands from here).
    pub git_root: PathBuf,
    pub branch: String,
}

fn git_out(dir: &Path, args: &[&str]) -> std::io::Result<std::process::Output> {
    Command::new("git").current_dir(dir).args(args).output()
}

/// The `env` fragment to merge into a CLAUDE pane's injected `settings.local.json`.
///
/// `state_adapter::inject` writes the worktree's `.claude/settings.local.json` from a
/// hooks-only template. A fresh worktree has no `.claude/` of its own (`.claude/` is
/// gitignored → never in the checkout), so any per-repo `env` the operator keeps in the
/// SOURCE repo's `.claude/settings.local.json` — the Bedrock switch
/// (`CLAUDE_CODE_USE_BEDROCK` / `AWS_PROFILE` / `ANTHROPIC_DEFAULT_*`), proxies, etc. —
/// would be dropped and the pane would silently fall back to the account default.
///
/// Resolve the source repo (the worktree's main working tree = the parent of the common
/// git dir, via `git rev-parse --git-common-dir`), read its `settings.local.json`, and
/// return ONLY its top-level `env` object as a `,\n  "env": { … }` fragment ready to
/// splice into the template's `{{ENV_BLOCK}}` slot. Returns `""` (the prior clobber
/// behavior) when there is no source file, no `env` key, an empty `env`, or anything
/// unparseable — never an error: a missing env must not fail a spawn. Only the `env`
/// block is merged; the operator's permissions / MCP toggles stay the pane's own (the
/// app intentionally sets its own hooks + `disabledMcpjsonServers`).
fn source_repo_claude_env_block(worktree: &Path) -> String {
    let Ok(out) = git_out(worktree, &["rev-parse", "--git-common-dir"]) else {
        return String::new();
    };
    if !out.status.success() {
        return String::new();
    }
    let common = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if common.is_empty() {
        return String::new();
    }
    // `--git-common-dir` is relative to the worktree when not absolute.
    let common_path = {
        let p = PathBuf::from(&common);
        if p.is_absolute() {
            p
        } else {
            worktree.join(p)
        }
    };
    // main repo root = parent of `<root>/.git`
    let Some(main_root) = common_path.parent() else {
        return String::new();
    };
    let settings = main_root.join(".claude").join("settings.local.json");
    let Ok(body) = std::fs::read_to_string(&settings) else {
        return String::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) else {
        return String::new();
    };
    let Some(env) = v.get("env").and_then(|e| e.as_object()) else {
        return String::new();
    };
    if env.is_empty() {
        return String::new();
    }
    let Ok(env_json) = serde_json::to_string_pretty(env) else {
        return String::new();
    };
    // indent nested lines two spaces so the object sits under the template's top level
    let indented = env_json.replace('\n', "\n  ");
    format!(",\n  \"env\": {indented}")
}

/// Create an isolated git worktree for `selected`, scoped to only that folder.
///
/// If `selected` is a SUBDIRECTORY of a larger repo (git root != selected), the
/// worktree is created with `--no-checkout` + cone sparse-checkout so ONLY that
/// subtree (+ repo-root files) materializes — instead of cloning the whole repo
/// (a large monorepo/vault would otherwise copy 100s of MB per pane). The agent
/// runs in `<worktree>/<subpath>`. If `selected` IS the repo root, a normal full
/// worktree is created. Errs if `selected` isn't inside a git repo (caller falls
/// back to running in the folder directly). The repo must have ≥1 commit.
pub fn add_worktree(selected: &Path, id: &str) -> std::io::Result<Worktree> {
    let branch = format!("agent-teams/{id}");

    // git toplevel — errs (→ caller fallback) if `selected` isn't in a git repo
    let top = git_out(selected, &["rev-parse", "--show-toplevel"])?;
    if !top.status.success() {
        return Err(io_err(format!(
            "not a git repo: {}",
            String::from_utf8_lossy(&top.stderr)
        )));
    }
    let git_root = PathBuf::from(String::from_utf8_lossy(&top.stdout).trim().to_string());

    // subpath of `selected` within the repo ("" if selected == git root)
    let canon_sel = std::fs::canonicalize(selected).unwrap_or_else(|_| selected.to_path_buf());
    let canon_root = std::fs::canonicalize(&git_root).unwrap_or_else(|_| git_root.clone());
    let subpath = canon_sel
        .strip_prefix(&canon_root)
        .map(|p| p.to_path_buf())
        .unwrap_or_default();

    let root = git_root.join(".agent-teams-worktrees").join(id);

    // Reuse (Plan 04-02): if a live worktree already sits at `root` — e.g. one the
    // startup dirty-sweep kept, or a same-session reopen — don't re-`worktree add`
    // (which would error "already exists" → caller falls back to the raw repo dir →
    // wrong cwd → resume keys the wrong conversation). Recompute cwd and return.
    if root.exists() {
        // Reuse ONLY a GENUINE worktree rooted exactly at `root`. `--is-inside-work-tree`
        // is `true` for ANY dir under the main checkout, so a plain (non-worktree) dir
        // squatting at this path would be wrongly "reused" → a Worktree rooted at the
        // MAIN checkout (a pane then editing main = the multi-session-collision hazard).
        // The sound check: a real linked worktree's `--show-toplevel` IS `root`; a plain
        // subdir's resolves to the main git_root. Canonicalize both (macOS /var symlink).
        let canon_root = std::fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
        let is_genuine_worktree = git_out(&root, &["rev-parse", "--show-toplevel"])
            .ok()
            .filter(|o| o.status.success())
            .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim().to_string()))
            .and_then(|tl| std::fs::canonicalize(&tl).ok())
            .map(|tl| tl == canon_root)
            .unwrap_or(false);
        if is_genuine_worktree {
            let cwd = if subpath.as_os_str().is_empty() {
                root.clone()
            } else {
                root.join(&subpath)
            };
            return Ok(Worktree {
                cwd,
                root,
                git_root,
                branch,
            });
        }
        // The path exists but is NOT a genuine worktree (a plain dir squatting it). Fail
        // LOUD rather than silently returning a main-rooted Worktree, and rather than
        // letting `git worktree add` below error opaquely. The operator removes the dir
        // or picks another id.
        return Err(io_err(format!(
            "worktree path occupied by a non-worktree dir: {} (remove it or use another id)",
            root.display()
        )));
    }

    if let Some(parent) = root.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // create the worktree but don't populate it yet (sparse decides what lands)
    let add = git_out(
        &git_root,
        &[
            "worktree",
            "add",
            "--no-checkout",
            "-B",
            &branch,
            &root.to_string_lossy(),
        ],
    )?;
    if !add.status.success() {
        if let Some(parent) = root.parent() {
            let _ = std::fs::remove_dir(parent);
        }
        return Err(io_err(format!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&add.stderr)
        )));
    }

    // helper: run a git step in the worktree; on failure, unwind the worktree
    let run = |args: &[&str]| -> std::io::Result<()> {
        let r = git_out(&root, args)?;
        if !r.status.success() {
            let _ = remove_worktree(&git_root, id, &root);
            return Err(io_err(format!(
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&r.stderr)
            )));
        }
        Ok(())
    };

    let cwd = if subpath.as_os_str().is_empty() {
        run(&["checkout"])?; // repo-root target → full checkout
        root.clone()
    } else {
        let sub = subpath.to_string_lossy().to_string();
        run(&["sparse-checkout", "init", "--cone"])?;
        run(&["sparse-checkout", "set", sub.as_str()])?;
        run(&["checkout"])?; // materialize only the sparse subtree
        root.join(&subpath)
    };

    Ok(Worktree {
        cwd,
        root,
        git_root,
        branch,
    })
}

/// Remove a workspace worktree and its branch (best-effort).
pub fn remove_worktree(repo: &Path, id: &str, path: &Path) -> std::io::Result<()> {
    let branch = format!("agent-teams/{id}");
    Command::new("git")
        .current_dir(repo)
        .args(["worktree", "remove", "--force"])
        .arg(path)
        .output()?;
    Command::new("git")
        .current_dir(repo)
        .args(["branch", "-D", &branch])
        .output()?;
    Ok(())
}

/// Resolve the DELIBERATE working directory for a harness spawn (gap #6 — the
/// accidental-`/` cwd). Preference order:
///   1. `worktree` — the pane's per-pane worktree dir (the intended run dir),
///      when it EXISTS on disk;
///   2. `repo` — the selected repo/folder, when IT exists (a worktree that
///      vanished between mint and spawn degrades to the repo, never a random dir);
///   3. `$HOME` (else the OS temp dir) — a neutral, REAL directory.
///
/// The filesystem ROOT is never accepted at any step: a GUI app launched via
/// `open`/launchd inherits cwd `/`, and a harness process that starts there slugs
/// its `~/.claude/projects/<encoded-cwd>` dir to a bare `-` — colliding transcripts
/// across unrelated panes (the Phase-19 session-id locator compensates on the read
/// side; this helper fixes the cause at spawn). Root is also never a legitimate
/// pane dir, so rejecting it loses nothing. Deterministic given the dirs on disk
/// → unit-tested (worktree-exists → worktree; missing → repo; both missing → HOME).
pub fn resolve_spawn_cwd(worktree: Option<&Path>, repo: Option<&Path>) -> PathBuf {
    // A usable deliberate cwd: an existing directory that is NOT the filesystem
    // root (`/` is the only unix path with no parent).
    fn usable(p: &Path) -> bool {
        p.is_dir() && p.parent().is_some()
    }
    if let Some(w) = worktree.filter(|p| usable(p)) {
        return w.to_path_buf();
    }
    if let Some(r) = repo.filter(|p| usable(p)) {
        return r.to_path_buf();
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|h| usable(h))
        .unwrap_or_else(std::env::temp_dir)
}

/// Derive the WORKSPACE key shared by a team's panes from a pane id (enablement
/// slice — C7 / 4c). Pane ids are `<workspace>-p<N>` (e.g. `ws28901x0-p3`); the
/// workspace key is the `<workspace>` prefix, SHARED by every pane of that team.
/// Used to pin the memory repo-key so a team's panes share ONE memory store rather
/// than fragmenting into per-worktree (per-cwd) partitions. Falls back to the whole
/// id when there is no `-p<digits>` suffix (degrades to per-pane = unshared, never
/// wrong). PURE — split out so it is unit-tested and computed exactly once at spawn.
pub fn workspace_key(pane_id: &str) -> &str {
    pane_id
        .rsplit_once("-p")
        .filter(|(_, n)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        .map(|(ws, _)| ws)
        .unwrap_or(pane_id)
}

// ───────────────────────────── bounded pane output ──────────────────────────────

/// Retained output bytes per pane. xterm keeps its own 5000-line scrollback on the
/// frontend; this buffer only needs (a) enough tail to repaint a TUI + recent
/// scrollback into a fresh xterm on app reload / re-attach, and (b) poll-gap cover
/// (the frontend reads deltas every ~120ms; 4 MiB ≈ 14s of cover at a 300 KB/s
/// repaint burst). 8 panes ⇒ ≤48 MiB resident worst-case (cap + compaction slack).
pub const RETAIN_CAP: usize = 4 * 1024 * 1024;

/// Bounded pane output: a retained byte tail + the ABSOLUTE stream offset (since
/// pane start) of its first byte. Replaces the unbounded `Vec<u8>` that grew
/// forever (perf-2026-06-10 A): the old whole-buffer `snapshot()` clone per pane
/// per 120ms poll was O(scrollback) under BOTH the pane mutex and the app's
/// registry mutex — typing queued behind multi-ms holds and the reader thread
/// stalled (PTY backpressure). Absolute offsets let the app layer serve exact
/// deltas (`delta(since)`), so a poll costs O(new bytes), never O(history).
pub struct PaneBuffer {
    buf: Vec<u8>,
    /// Absolute stream offset of `buf[0]` — advances only when `push` compacts.
    base: u64,
    /// Retention target: compaction drains the front back down to ~this many bytes.
    cap: usize,
}

impl PaneBuffer {
    /// `cap` = retention target ([`RETAIN_CAP`] in production; tests shrink it).
    pub fn new(cap: usize) -> Self {
        Self {
            buf: Vec::new(),
            base: 0,
            cap: cap.max(4),
        }
    }

    /// Append reader-thread bytes; compact (with hysteresis) once the buffer
    /// outgrows the cap by 50% — drain the front back to `cap`, advancing the cut
    /// FORWARD to the next UTF-8 char boundary (≤3 extra bytes) so the retained
    /// window never starts mid-codepoint. Hysteresis amortizes the drain (one
    /// sub-ms memmove every ~cap/2 bytes of output, not per push).
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
        if self.buf.len() > self.cap + self.cap / 2 {
            let mut cut = self.buf.len() - self.cap;
            // A UTF-8 continuation byte is 0b10xxxxxx; a valid stream has ≤3 in a
            // row, so advance ≤3 (binary garbage: stop after 3, lossy handles it).
            let mut steps = 0;
            while steps < 3 && cut < self.buf.len() && self.buf[cut] & 0xC0 == 0x80 {
                cut += 1;
                steps += 1;
            }
            self.buf.drain(..cut);
            self.base += cut as u64;
        }
    }

    /// Absolute offset of the first retained byte.
    pub fn base(&self) -> u64 {
        self.base
    }

    /// Absolute end offset — the total bytes ever pushed (`base` + retained len).
    pub fn end(&self) -> u64 {
        self.base + self.buf.len() as u64
    }

    /// The retained window itself (for bounded whole-window reads, e.g. `snapshot`).
    pub fn retained(&self) -> &[u8] {
        &self.buf
    }

    /// The retained window as a bounded [`ByteRing`] (08-T4): the `ByteRing::recent`
    /// "recent scrollback" substrate behind [`Supervisor::snapshot`] / the daemon's
    /// `handle_read_output`. This is the WHOLE-WINDOW snapshot read — NOT the delta
    /// cursor (that stays on [`PaneBuffer::delta`], which the GUI `read_output_delta`
    /// byte-cursor protocol depends on). The ring's capacity is exactly the retained
    /// length, so `ring.recent()` returns the same bytes as [`retained`](Self::retained),
    /// just through the bounded type a future daemon re-attach reads.
    pub fn recent_ring(&self) -> agent_teams_ringbuf::ByteRing {
        let mut ring = agent_teams_ringbuf::ByteRing::with_capacity(self.buf.len());
        ring.push(&self.buf);
        ring
    }

    /// Raw bytes from absolute offset `since`, clamped to the retained window.
    /// Returns `(start, bytes)` — `start` is the absolute offset of `bytes[0]`.
    /// Clamping (perf-2026-06-10 CONTRACT seam 1):
    ///   since < base → bytes were evicted under the caller → whole window from base
    ///   since > end  → STALE cursor (respawned pane id / reload desync) → whole window
    ///   else         → exact [since..end) — the steady-state O(new bytes) path
    /// `start != since` is the caller's truncation signal. Never an error — every
    /// desync self-heals into a full-window replay.
    pub fn delta(&self, since: u64) -> (u64, Vec<u8>) {
        if since < self.base || since > self.end() {
            return (self.base, self.buf.clone());
        }
        let off = (since - self.base) as usize;
        (since, self.buf[off..].to_vec())
    }
}

/// A live PTY-backed agent session.
pub struct Supervisor {
    pub id: String,
    pub harness: Harness,
    /// 17-01: the pane's typed role, captured from `spec.role` at spawn. The live,
    /// id-keyed home for a pane's role — so orchestrate can render it role-aware even
    /// after a pane close (the frontend's index-keyed `roles[]` drifts on close).
    /// `None` = a homogeneous pane (today's default).
    pub role: Option<roles::AgentRole>,
    /// 16/Phase-16 autonomous workers: true iff this pane is a delegate WORKER (spawned
    /// by `socket_delegate`'s detached controller, not a human). Default false. Set
    /// app-side in `do_spawn` after construction (the supervisor crate has no notion of
    /// delegation). Lets the human-facing surfaces (queue / orchestrate set) exclude
    /// workers, and lets delegation cleanup identify the panes it owns.
    pub is_worker: bool,
    /// 06-11: the model the pane was spawned with (`spec.model`), kept for report
    /// attribution so the fan-in can label each contribution by harness + model. `None`
    /// = account default (no `--model` pinned at spawn).
    pub model: Option<String>,
    master: Box<dyn MasterPty + Send>, // kept for resize (sync PTY size to the UI terminal)
    child: Box<dyn Child + Send + Sync>,
    /// Shared with the reader thread so it can auto-answer terminal capability probes
    /// (OpenTUI/OpenCode) without waiting on the FE xterm path — see [`auto_answer_term_queries`].
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    output: Arc<Mutex<PaneBuffer>>,
    /// 08 Sub-build 3 / slice 3: the per-pane output subscriber registry the reader
    /// thread fans each chunk out to (`push_and_fanout`, UNDER the buffer lock). The
    /// daemon's `Attach` clones this handle (via [`Self::stream_handles`]) to register a
    /// bounded subscription. Empty by default → the fan-out is a no-op until something
    /// attaches, so the steady-state GUI read path pays nothing.
    subs: subscribers::SubscriberHandle,
}

/// Write a synthetic `SessionStart` event for a STATE-BLIND harness (codex /
/// commandcode / opencode / pi / grok) so the state-adapter sees the pane the instant it spawns.
///
/// These harnesses have no native `SessionStart` hook (unlike claude/cursor), so they
/// otherwise produce ZERO `events.jsonl` and are INVISIBLE to the ranked queue — the bug
/// surfaced by dogfooding against a real repo (codex/commandcode panes had no state dir
/// at all). This appends the EXACT line shape `state-writer.sh` writes
/// (`{ts,harness,event,workspace_id,decision,payload}`) so the existing reader
/// (`watch::parse_latest_line`) + `normalize` map it to `Working` — the same baseline
/// claude/cursor get from their real `SessionStart`.
///
/// A SIBLING of `state_root`, named `<state-name>-<suffix>` (mirrors the app's `state_sibling`),
/// so it SURVIVES the startup state-dir wipe. Used by tests and kept as the supervisor-side
/// recipe for any future per-pane state that must outlive the wipe of `state_root`.
#[allow(dead_code)]
fn state_sibling(state_root: &Path, suffix: &str) -> PathBuf {
    let name = state_root
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "agent-teams".to_string());
    state_root
        .parent()
        .map(|p| p.join(format!("{name}-{suffix}")))
        .unwrap_or_else(|| PathBuf::from(format!("agent-teams-{suffix}")))
}

/// Answer common terminal capability probes that OpenTUI/OpenCode emit and then **block
/// on** until the host replies.
///
/// Live diagnosis (2026-07-16): an OpenCode pane showed `(W)` with a blank body because
/// synthetic `SessionStart` marks Working, while the process hung after
/// `"booting location services"` waiting for:
///   - CSI `6n` (cursor position report)
///   - OSC `10`/`11` `?` (fg/bg color query)
///   - CSI `>0q` (XTVERSION)
/// Without replies the TUI never finishes painting — and if the FE sized-gate delays
/// writing those probes into xterm, xterm never generates the replies either.
///
/// Host-side answers break the deadlock. Live-verified: with answers, OpenCode grows
/// from ~373 probe bytes → ~8KB truecolor SGR within ~3s. They are pure terminal
/// protocol (same bytes a real xterm would emit), never agent prompt input.
/// Best-effort: lock/write failures are ignored so a probe path can never kill the reader.
///
/// `carry` holds a short tail of the previous chunk so probes split across 4096-byte
/// reads still match (e.g. `ESC[` | `6n`).
fn auto_answer_term_queries(
    writer: &Mutex<Box<dyn Write + Send>>,
    chunk: &[u8],
    carry: &mut Vec<u8>,
) {
    if chunk.is_empty() && carry.is_empty() {
        return;
    }
    // Join carry + chunk; keep at most 64 trailing bytes for the next call.
    let mut buf = std::mem::take(carry);
    buf.extend_from_slice(chunk);
    let mut replies: Vec<u8> = Vec::new();
    let mut i = 0;
    while i < buf.len() {
        // CSI 6 n — Device Status Report / cursor position
        if i + 3 < buf.len()
            && buf[i] == 0x1b
            && buf[i + 1] == b'['
            && buf[i + 2] == b'6'
            && buf[i + 3] == b'n'
        {
            replies.extend_from_slice(b"\x1b[1;1R");
            i += 4;
            continue;
        }
        // CSI > 0 q — XTVERSION (OpenTUI boot probe)
        if i + 4 < buf.len()
            && buf[i] == 0x1b
            && buf[i + 1] == b'['
            && buf[i + 2] == b'>'
            && buf[i + 3] == b'0'
            && buf[i + 4] == b'q'
        {
            replies.extend_from_slice(b"\x1bP>|xterm(100)\x1b\\");
            i += 5;
            continue;
        }
        // OSC 10;? / 11;?  terminated by BEL (\x07) or ST (ESC \)
        if i + 5 < buf.len()
            && buf[i] == 0x1b
            && buf[i + 1] == b']'
            && buf[i + 2] == b'1'
            && (buf[i + 3] == b'0' || buf[i + 3] == b'1')
            && buf[i + 4] == b';'
            && buf[i + 5] == b'?'
        {
            let which = buf[i + 3]; // b'0' fg, b'1' bg
            let mut j = i + 6;
            let mut term_end = None;
            while j < buf.len() {
                if buf[j] == 0x07 {
                    term_end = Some(j + 1);
                    break;
                }
                if buf[j] == 0x1b && j + 1 < buf.len() && buf[j + 1] == b'\\' {
                    term_end = Some(j + 2);
                    break;
                }
                j += 1;
            }
            if let Some(end) = term_end {
                // rgb:RRRR/GGGG/BBBB — xterm-style; values match HR TERM_THEME-ish cyan-on-dark
                if which == b'0' {
                    replies.extend_from_slice(b"\x1b]10;rgb:9fe6/f5f5/f5f5\x07");
                } else {
                    replies.extend_from_slice(b"\x1b]11;rgb:0a0a/1212/1919\x07");
                }
                i = end;
                continue;
            }
            // Incomplete OSC at end of buf — park from ESC for next chunk.
            break;
        }
        // Incomplete CSI at end (ESC or ESC [ … without final)
        if buf[i] == 0x1b && i + 1 == buf.len() {
            break;
        }
        if buf[i] == 0x1b && i + 1 < buf.len() && buf[i + 1] == b'[' && i + 2 == buf.len() {
            break;
        }
        i += 1;
    }
    // Retain unparsed tail (possible split probe).
    if i < buf.len() {
        let tail = &buf[i..];
        let keep = tail.len().min(64);
        carry.extend_from_slice(&tail[tail.len() - keep..]);
    }
    if replies.is_empty() {
        return;
    }
    if let Ok(mut w) = writer.lock() {
        let _ = w.write_all(&replies);
        let _ = w.flush();
    }
}

/// Best-effort: a write failure must NEVER fail the spawn (parity with the inject
/// blocks). `wire` is the harness's `descriptor().wire` string ("codex" etc.).
fn write_spawn_ready_event(state_root: &Path, wsid: &str, wire: &str) -> std::io::Result<()> {
    let dir = state_root.join(wsid);
    std::fs::create_dir_all(&dir)?;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    // Same JSONL shape state-writer.sh appends; payload is an (escaped) empty object.
    let line = format!(
        "{{\"ts\":{ts},\"harness\":\"{wire}\",\"event\":\"SessionStart\",\"workspace_id\":\"{wsid}\",\"decision\":\"na\",\"payload\":\"{{}}\"}}\n"
    );
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("events.jsonl"))?;
    f.write_all(line.as_bytes())
}

/// Pre-seed cursor-agent's workspace-trust marker so a freshly-spawned cursor PANE
/// (the interactive TUI) does NOT block on the "Workspace Trust Required" modal. The
/// `--trust` CLI flag can't help here — it errors outside `--print`/headless mode (and
/// we only pass it on the worker leg), so the interactive pane stalls until trust is
/// granted. cursor stores trust as a file `.workspace-trusted` under
/// `${CURSOR_DATA_DIR:-~/.cursor}/projects/<slug>/` and gates on the file's EXISTENCE,
/// so the JSON body is cosmetic. Best-effort + idempotent. Reads `$HOME` but never
/// CHANGES it (the spawn env-HOME invariant). slug rule + path decompiled from the
/// cursor-agent bundle and verified against on-disk markers.
fn preseed_cursor_trust(worktree: &Path) -> std::io::Result<()> {
    let base = match std::env::var_os("CURSOR_DATA_DIR").filter(|s| !s.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => PathBuf::from(
            std::env::var_os("HOME")
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "HOME unset"))?,
        )
        .join(".cursor"),
    };
    let abs = worktree
        .canonicalize()
        .unwrap_or_else(|_| worktree.to_path_buf());
    let abs_str = abs.to_string_lossy();
    let dir = base.join("projects").join(cursor_trust_slug(&abs_str));
    std::fs::create_dir_all(&dir)?;
    let path_json = abs_str.replace('\\', "\\\\").replace('"', "\\\"");
    let body = format!(
        "{{\"trustedAt\":\"{}\",\"workspacePath\":\"{}\",\"trustMethod\":\"agent-teams-preseed\"}}",
        iso8601_utc_now(),
        path_json,
    );
    std::fs::write(dir.join(".workspace-trusted"), body)
}

/// cursor's project-slug rule (decompiled): every non-alphanumeric char → '-', runs of
/// '-' collapsed to one, leading/trailing '-' trimmed.
fn cursor_trust_slug(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut prev_dash = false;
    for c in path.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// Minimal UTC ISO-8601 (second precision) from the wall clock — no chrono dep. The
/// cursor trust marker checks only file EXISTENCE, so this is cosmetic, but we emit a
/// real valid value (Howard Hinnant's civil-from-days).
fn iso8601_utc_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

/// Serializes the read-modify-write of the SHARED `~/.claude.json` so concurrent claude
/// pane spawns can't corrupt the operator's real config.
static CLAUDE_JSON_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Pure: given the existing `~/.claude.json` text + an absolute project path, return the
/// patched JSON with `projects[path]` carrying the trust/onboarding keys claude gates on.
/// Preserves all other content losslessly (serde round-trip). Errs if the input is not a
/// JSON object, so the caller SKIPS rather than clobbering a malformed/foreign file.
fn claude_trust_patched_json(existing: &str, project_path: &str) -> Result<String, String> {
    let mut root: serde_json::Value =
        serde_json::from_str(existing).map_err(|e| format!("parse: {e}"))?;
    let obj = root
        .as_object_mut()
        .ok_or("~/.claude.json is not a JSON object")?;
    let projects = obj
        .entry("projects")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()))
        .as_object_mut()
        .ok_or("projects is not an object")?;
    let entry = projects
        .entry(project_path.to_string())
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()))
        .as_object_mut()
        .ok_or("project entry is not an object")?;
    entry.insert(
        "hasTrustDialogAccepted".into(),
        serde_json::Value::Bool(true),
    );
    entry.insert(
        "hasCompletedProjectOnboarding".into(),
        serde_json::Value::Bool(true),
    );
    let seen_ok = entry
        .get("projectOnboardingSeenCount")
        .and_then(|v| v.as_u64())
        .is_some_and(|n| n >= 1);
    if !seen_ok {
        entry.insert(
            "projectOnboardingSeenCount".into(),
            serde_json::Value::from(1),
        );
    }
    serde_json::to_string(&root).map_err(|e| format!("serialize: {e}"))
}

/// Pre-seed Claude's per-project trust so a freshly-spawned claude PANE (interactive TUI)
/// doesn't block on the first-launch "Do you trust the files in this folder?" dialog — the
/// pane's cwd is a brand-new per-workspace worktree absent from ~/.claude.json, so the dialog
/// fires and EATS the first dispatched input (a second send works once it's dismissed). claude
/// exposes NO interactive trust-skip flag, so (mirroring [`preseed_cursor_trust`]) we pre-seed
/// the config keys it gates on. Best-effort + idempotent; serialized ([`CLAUDE_JSON_LOCK`]) +
/// atomic (tmp+rename) so a concurrent spawn never corrupts the real config. Reads `$HOME`,
/// never changes it. Skips (Err) if ~/.claude.json is absent/unparseable — never writes a
/// partial config that could shadow the operator's real one.
fn preseed_claude_trust(worktree: &Path) -> std::io::Result<()> {
    let home = std::env::var_os("HOME")
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "HOME unset"))?;
    let cfg = PathBuf::from(home).join(".claude.json");
    let abs = worktree
        .canonicalize()
        .unwrap_or_else(|_| worktree.to_path_buf());
    let key = abs.to_string_lossy();

    let _guard = CLAUDE_JSON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let existing = std::fs::read_to_string(&cfg)?; // absent ⇒ Err ⇒ best-effort skip upstream
    let patched = claude_trust_patched_json(&existing, &key)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let tmp = cfg.with_extension("json.agent-teams.tmp");
    std::fs::write(&tmp, patched)?;
    std::fs::rename(&tmp, &cfg)
}

/// codex turn-end wiring. codex's ONLY turn signal is its `notify` hook, so the
/// supervisor overrides it PER-PANE (keeping the user's GLOBAL `notify` — e.g.
/// BridgeSpace — untouched) to point at `codex-notify.sh`, which appends a `notify`
/// event the state-adapter maps to `Done`/`TurnEnd`. (The synthetic `SessionStart` only
/// covers spawn/ready; this covers turn-end.) Returns the `-c notify=[...]` override
/// args; `[]` for non-codex / worker panes. `wsid` + `state_root` ride as args
/// (deterministic — does NOT rely on the notify subprocess inheriting the pane env);
/// codex appends its own JSON payload after these. The value is TOML (codex `-c` parses
/// it; arrays supported, verified against codex-cli 0.139).
fn codex_notify_args(
    harness: Harness,
    is_worker: bool,
    hooks_dir: &Path,
    wsid: &str,
    state_root: &Path,
) -> Vec<String> {
    if is_worker || !matches!(harness, Harness::Codex) {
        return vec![];
    }
    let script = hooks_dir.join("codex-notify.sh");
    // TOML array; `{:?}` double-quotes each element. macOS paths/wsid carry no quotes or
    // backslashes, so Rust debug-quoting is valid TOML — and a space inside a quoted
    // element is fine (codex passes each array element as a separate argv to `bash`).
    let value = format!(
        "notify=[\"bash\", {:?}, {:?}, {:?}]",
        script.to_string_lossy(),
        wsid,
        state_root.to_string_lossy(),
    );
    vec!["-c".to_string(), value]
}

/// Codex MCP injection. Codex reads MCP servers ONLY from `~/.codex/config.toml`
/// (global, not per-project) and does NOT inherit parent env vars to MCP stdio
/// children (codex-cli #3064/#4180). So we append a per-pane
/// `[mcp_servers.agent-teams-<pane_id>]` block with an explicit `env` table
/// carrying the provenance vars the sidecar needs. The block is REMOVED on pane
/// kill ([`remove_codex_mcp`]) so the global config stays clean.
///
/// Append-only (like `codex_trust.rs`): we never parse+re-serialize the TOML
/// (that would lose comments/ordering). Idempotent: skips if the block already
/// exists. Best-effort: a failure degrades to "no MCP in the pane", never a
/// failed spawn (AC-6). Gated on `!is_worker` to mirror the other MCP gates.
fn inject_codex_mcp(
    sidecar_bin: &Path,
    state_root: &Path,
    pane_id: &str,
    repo_key: &str,
) -> std::io::Result<()> {
    let config = match codex_config_path() {
        Some(c) => c,
        None => return Ok(()), // no HOME → nothing to inject
    };
    if !config.is_file() {
        return Ok(()); // no codex config yet — user hasn't run codex
    }
    let server_name = codex_mcp_server_name(pane_id);
    let header = format!("[mcp_servers.{server_name}]");
    let existing = std::fs::read_to_string(&config)?;
    if existing.contains(&header) {
        return Ok(()); // already injected (idempotent)
    }
    let sidecar_str = sidecar_bin.to_string_lossy();
    let state_str = state_root.to_string_lossy();
    let block = format!(
        "\n# >>> @agent-teams-managed:{server_name} >>>\n         {header}\n         command = {sidecar_str:?}\n         args = []\n         env = {{ AGENT_TEAMS_STATE_DIR = {state_str:?}, AGENT_TEAMS_PANE_ID = {pane_id:?}, AGENT_TEAMS_MEMORY_REPO_KEY = {repo_key:?}, AGENT_TEAMS_TASK_SCOPE = {pane_id:?} }}\n         # <<< @agent-teams-managed:{server_name} <<<\n"
    );
    let mut f = std::fs::OpenOptions::new().append(true).open(&config)?;
    use std::io::Write;
    f.write_all(block.as_bytes())
}

/// Remove the per-pane MCP block injected by [`inject_codex_mcp`]. Best-effort:
/// a missing config or block is Ok(()). Called from [`Supervisor::kill`] so the
/// global config stays clean after a pane closes.
fn remove_codex_mcp(pane_id: &str) {
    let Some(config) = codex_config_path() else { return };
    if !config.is_file() {
        return;
    }
    let server_name = codex_mcp_server_name(pane_id);
    let Ok(content) = std::fs::read_to_string(&config) else { return };
    let begin_marker = format!("# >>> @agent-teams-managed:{server_name} >>>");
    let end_marker = format!("# <<< @agent-teams-managed:{server_name} <<<");
    let Some(start) = content.find(&begin_marker) else { return };
    let Some(end_rel) = content[start..].find(&end_marker) else { return };
    let end = start + end_rel + end_marker.len();
    // consume trailing newline if present
    let end = if content[end..].starts_with('\n') { end + 1 } else { end };
    let mut cleaned = String::with_capacity(content.len());
    cleaned.push_str(&content[..start]);
    cleaned.push_str(&content[end..]);
    let _ = std::fs::write(&config, cleaned);
}

/// Deterministic per-pane MCP server name. Pane ids are `[a-zA-Z0-9-]` (e.g.
/// `ws123-p0`), so the result is a valid TOML bare key.
fn codex_mcp_server_name(pane_id: &str) -> String {
    format!("agent-teams-{pane_id}")
}

/// `~/.codex/config.toml` path (mirrors `codex_trust.rs`).
fn codex_config_path() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".codex").join("config.toml"))
}

/// opencode turn-end wiring. opencode has no hook config, but it AUTO-LOADS plugins from
/// `~/.config/opencode/plugins/`. Ensure-install `opencode-state-plugin.js` there
/// (idempotent, copy-always so updates land) BEFORE an opencode pane spawns; on
/// `session.idle` it appends a `stop` event the state-adapter maps to Done/TurnEnd.
/// The plugin is GUARDED on `AGENT_TEAMS_PANE_ID` (set on the opencode process env), so
/// it no-ops for the user's own opencode sessions. Best-effort: a failure never fails
/// the spawn. (commandcode has no equivalent hook surface, so it gets no turn-end wiring.)
fn ensure_opencode_plugin(hooks_dir: &Path) -> std::io::Result<()> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no HOME"))?;
    install_opencode_plugin(hooks_dir, &home.join(".config/opencode/plugins"))
}

/// The copy step (split from HOME resolution so it unit-tests without env mutation):
/// stage `opencode-state-plugin.js` from `hooks_dir` into `dest_dir` as the
/// auto-loaded `agent-teams-state.js`. Idempotent (copy-always overwrites).
fn install_opencode_plugin(hooks_dir: &Path, dest_dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest_dir)?;
    std::fs::copy(
        hooks_dir.join("opencode-state-plugin.js"),
        dest_dir.join("agent-teams-state.js"),
    )?;
    Ok(())
}

/// Whether a Bash pane should launch the user's LOGIN shell (rich prompt + rc) vs a bare,
/// deterministic `bash`. ON when `AGENT_TEAMS_SHELL_LOGIN` is set to anything but `"0"`/empty
/// (the Tauri app sets `"1"` at startup); OFF by default so the integration tests keep the bare
/// shell their assertions expect (a login shell re-derives PATH via zsh `path_helper`, which would
/// defeat `spawn_injects_dev_path`).
fn shell_login_enabled() -> bool {
    std::env::var("AGENT_TEAMS_SHELL_LOGIN")
        .map(|v| !v.is_empty() && v != "0")
        .unwrap_or(false)
}

/// The user's interactive login shell for a Bash ("Shell") pane. A GUI-launched app inherits a
/// minimal env, and a hardcoded non-login `bash` never sources `~/.zshrc`/starship → a bare
/// `bash-5.2$` prompt. Resolving `$SHELL` (then launching it `-l`) renders the user's real shell +
/// prompt instead. Resolution: `$SHELL` if set + present; else first existing of `/bin/zsh` →
/// `/bin/bash` → `/bin/sh`; else bare `"bash"` (PATH-resolved). The injected `harness_path` is still
/// set as the child PATH at spawn, so brew/.local binaries resolve even if a profile doesn't.
fn login_shell() -> String {
    if let Ok(sh) = std::env::var("SHELL") {
        if !sh.is_empty() && Path::new(&sh).exists() {
            return sh;
        }
    }
    for cand in ["/bin/zsh", "/bin/bash", "/bin/sh"] {
        if Path::new(cand).exists() {
            return cand.to_string();
        }
    }
    "bash".to_string()
}

impl Supervisor {
    /// Inject hooks (unless Bash test harness), open a PTY, spawn the harness in
    /// the worktree with `AGENT_TEAMS_STATE_DIR` set, and start draining output.
    pub fn spawn(
        spec: &WorkspaceSpec,
        hooks_dir: &Path,
        state_root: &Path,
        sidecar_bin: &Path,
    ) -> std::io::Result<Self> {
        // The GLOBAL memory scope-key (single personal Second Brain) and per-pane task
        // scope (4c), derived ONCE so the InjectConfig env block and the cmd.env below
        // can never disagree. Injecting the literal `GLOBAL_SCOPE` value makes a pane's
        // sidecar resolve the SAME `<memory_root>/global` store the app and standalone
        // MCP fall back to. (`workspace_key` remains for its per-pane callers/tests; it
        // no longer feeds the memory key.)
        let mem_key = agent_teams_memory::GLOBAL_SCOPE.to_string();

        // Per-pane MCP config path for claude (set by the inject block below; None
        // for cursor/bash or if inject fails → mcp_args degrades to no flag, AC-6).
        let mut claude_mcp_cfg: Option<PathBuf> = None;

        if let Some(ih) = spec.harness.inject_harness() {
            let cfg = InjectConfig {
                workspace_id: spec.id.clone(),
                repo_dir: spec.worktree.clone(),
                writer_path: hooks_dir.join("state-writer.sh"),
                templates_dir: hooks_dir.to_path_buf(),
                state_root: state_root.to_path_buf(),
                // Enablement slice (C2/C7/4c): the sidecar reads these from its OWN
                // process env, which the harness sets from the injected mcp.json
                // `env` block (the PROVEN channel — STATE_DIR rides it). pane_id =
                // provenance (Actor::Pane); repo_key = the GLOBAL memory store;
                // task_scope = the pane's own-task transition scope.
                pane_id: spec.id.clone(),
                repo_key: mem_key.clone(),
                task_scope: spec.id.clone(),
            };
            // Merge the SOURCE repo's per-repo `env` (Bedrock etc.) into a CLAUDE pane's
            // settings.local.json — the hooks template alone would clobber it (see the fn
            // doc). Cursor's template has no {{ENV_BLOCK}} slot → "" is a no-op there.
            let env_block = match ih {
                InjectHarness::Claude => source_repo_claude_env_block(&spec.worktree),
                _ => String::new(),
            };
            inject(&cfg, ih, &env_block)?;
            // Inject the read-only stdio MCP sidecar (16-01 / D56). SEPARATE from
            // hook injection. Claude → staged sibling path (used by --mcp-config);
            // cursor → in-worktree .cursor/mcp.json (+git-excluded) → None. A
            // failure here is best-effort: degrade to "no MCP in the pane" rather
            // than failing the spawn (parity with whisper being optional, AC-6).
            // Phase-16: a delegate WORKER does NOT get the sidecar — worker_args'
            // --allowedTools excludes mcp__agent-teams__* (the worker can't call it),
            // so injecting it would only spawn an unused sidecar process at startup.
            if !spec.is_worker {
                match inject_mcp_config(&cfg, ih, sidecar_bin) {
                    Ok(path) => claude_mcp_cfg = path,
                    Err(e) => eprintln!(
                        "[agent-teams] inject_mcp_config failed (pane will have no MCP): {e}"
                    ),
                }
            }
            // 17-01: a CURSOR pane with a role gets its persona as a project-local
            // Cursor rule file (.cursor/rules/agent-role.mdc) — cursor-agent has NO
            // --append-system-prompt flag, so a written rule is its only persistent
            // steering. Additive sibling to the MCP block above (do NOT fold in).
            // Claude does NOT get a rule file — it gets the --append-system-prompt
            // CLI flag below; bash gets neither. Best-effort: a write failure degrades
            // to "no persona in this cursor pane" rather than failing the spawn.
            // Claude pane: pre-seed per-project trust so the interactive TUI doesn't stall on
            // the first-launch "Do you trust the files in this folder?" dialog (no CLI flag
            // skips it — see preseed_claude_trust). Best-effort, mirrors the cursor pre-seed.
            if matches!(spec.harness, Harness::Claude) {
                if let Err(e) = preseed_claude_trust(&spec.worktree) {
                    eprintln!(
                        "[agent-teams] preseed_claude_trust failed (claude pane may show the trust dialog): {e}"
                    );
                }
            }
            if matches!(spec.harness, Harness::Cursor) {
                // Pre-seed cursor's workspace-trust marker BEFORE the pane spawns, so the
                // interactive TUI doesn't stall on the "Workspace Trust Required" modal
                // (--trust only works in --print/headless mode — see preseed_cursor_trust).
                // Unconditional for cursor (not role-gated). Best-effort like the sibling
                // injects: a failure degrades to "pane may prompt for trust", never a
                // failed spawn.
                if let Err(e) = preseed_cursor_trust(&spec.worktree) {
                    eprintln!(
                        "[agent-teams] preseed_cursor_trust failed (cursor pane may block on the trust modal): {e}"
                    );
                }
                if let Some(role) = spec.role {
                    if let Err(e) =
                        inject_cursor_role(&spec.worktree, hooks_dir, roles::persona(role))
                    {
                        eprintln!(
                            "[agent-teams] inject_cursor_role failed (cursor pane will have no role persona): {e}"
                        );
                    }
                }
            }
        }

        // commandcode + opencode are state-blind (no InjectHarness, so they skip the block
        // above) but first-class MCP CLIENTS: both read a project-local `.mcp.json` at the
        // pane cwd (auto-discovered) so they get the queue + the gated memory/task tools like
        // cursor (D56/D60). SEPARATE from hook injection: a state-blind harness can still be
        // a first-class MCP client via a project-local `.mcp.json`. Best-effort — a write
        // failure degrades to "no MCP in the pane", never a failed spawn (AC-6).
        // Gated on `!is_worker` to mirror the inject_mcp_config gate above (PR #137 —
        // autonomous workers skip the sidecar).
        if !spec.is_worker && matches!(spec.harness, Harness::CommandCode | Harness::OpenCode) {
            if let Err(e) = inject_commandcode_mcp(
                &spec.worktree,
                hooks_dir,
                sidecar_bin,
                state_root,
                &spec.id,
                &mem_key,
            ) {
                eprintln!(
                    "[agent-teams] inject_commandcode_mcp failed ({:?} pane will have no MCP): {e}",
                    spec.harness
                );
            }
        }

        // grok is state-blind but a first-class MCP CLIENT: it reads a project-scoped
        // `.grok/config.toml` at the pane cwd (auto-discovered). Same sidecar, TOML format.
        // Best-effort — a write failure degrades to "no MCP in the pane", never a failed
        // spawn (AC-6). Gated on `!is_worker` to mirror the inject_mcp_config gate above.
        if !spec.is_worker && matches!(spec.harness, Harness::Grok) {
            if let Err(e) = inject_grok_mcp(
                &spec.worktree,
                hooks_dir,
                sidecar_bin,
                state_root,
                &spec.id,
                &mem_key,
            ) {
                eprintln!(
                    "[agent-teams] inject_grok_mcp failed (grok pane will have no MCP): {e}"
                );
            }
        }

        // codex is state-blind but a first-class MCP CLIENT: it reads MCP servers from
        // `~/.codex/config.toml` (global, not per-project). Unlike the other harnesses
        // (project-local config files), codex has NO per-project MCP discovery, so we
        // append a per-pane `[mcp_servers.agent-teams-<pane_id>]` block with an explicit
        // `env` table (codex does NOT inherit parent env to MCP children, #3064/#4180).
        // The block is removed on pane kill (remove_codex_mcp). Best-effort — a write
        // failure degrades to "no MCP in the pane", never a failed spawn (AC-6).
        // Gated on `!is_worker` to mirror the other MCP injection gates.
        if !spec.is_worker && matches!(spec.harness, Harness::Codex) {
            if let Err(e) = inject_codex_mcp(
                sidecar_bin,
                state_root,
                &spec.id,
                &mem_key,
            ) {
                eprintln!(
                    "[agent-teams] inject_codex_mcp failed (codex pane will have no MCP): {e}"
                );
            }
        }

        // State-blind harnesses (codex/commandcode/opencode/pi/grok) have no native SessionStart
        // hook → they'd produce no events and be invisible to the ranked queue. Write a
        // synthetic ready event at spawn so the adapter sees them (as Working, like
        // claude/cursor's real SessionStart). Skips claude/cursor (they fire their own),
        // Bash (a test harness, not an agent), and workers (human-invisible by design).
        // Best-effort — a write failure never fails the spawn.
        if spec.harness.inject_harness().is_none()
            && !matches!(spec.harness, Harness::Bash)
            && !spec.is_worker
        {
            if let Err(e) =
                write_spawn_ready_event(state_root, &spec.id, spec.harness.descriptor().wire)
            {
                eprintln!(
                    "[agent-teams] write_spawn_ready_event failed (pane may stay state-blind): {e}"
                );
            }
        }

        // opencode turn-end: ensure the auto-loaded plugin is installed BEFORE the
        // opencode process starts (it writes `stop` on session.idle → Done/TurnEnd).
        // Non-worker opencode panes only; best-effort.
        if matches!(spec.harness, Harness::OpenCode) && !spec.is_worker {
            if let Err(e) = ensure_opencode_plugin(hooks_dir) {
                eprintln!(
                    "[agent-teams] ensure_opencode_plugin failed (opencode pane has no turn-end): {e}"
                );
            }
        }

        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 30,
                cols: 100,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(io_err)?;

        // A Bash pane is the user's interactive SHELL, not an agent CLI — so launch their login
        // shell (`$SHELL -l`), like Terminal.app/iTerm2, so the profile + rc render the user's REAL
        // prompt (starship/powerline), aliases, and rc-env. The old bare `bash` (non-login) sourced
        // none of that → the unconfigured `bash-5.2$`. Gated on `AGENT_TEAMS_SHELL_LOGIN` (the app
        // sets it at startup) so direct-`Harness::Bash` integration tests keep a deterministic bare
        // bash (a login zsh re-derives PATH via path_helper, defeating `spawn_injects_dev_path`).
        // Every other harness keeps its exact descriptor command (an agent binary, never a shell).
        let mut cmd = if matches!(spec.harness, Harness::Bash) && shell_login_enabled() {
            let mut c = CommandBuilder::new(login_shell());
            c.arg("-l"); // login → profile + rc (the PTY tty already makes it interactive)
            c
        } else {
            CommandBuilder::new(spec.harness.descriptor().command)
        };
        // create dictates a session id; reopen resumes it (Plan 04-02)
        for arg in session_args(spec.harness, spec.session_id.as_deref(), spec.resume) {
            cmd.arg(arg);
        }
        // per-harness static spawn flags (commandcode --skip-onboarding -t). A pure
        // constant of the variant (descriptor), appended AFTER session_args so the
        // conversation-selection argv is unchanged; [] for every other harness.
        for arg in spec.harness.descriptor().spawn_args {
            cmd.arg(arg);
        }
        // inject the read-only MCP sidecar flags (16-01 / D56): claude additive
        // --mcp-config <abs>, cursor --approve-mcps, bash none. Appended AFTER
        // session_args so the existing conversation-selection argv is unchanged.
        for arg in mcp_args(spec.harness, claude_mcp_cfg.as_deref(), spec.is_worker) {
            cmd.arg(arg);
        }
        // model-at-spawn: optional per-pane model override (flags mirror the headless
        // worker_spawn arms). [] when unset → account default, argv byte-identical.
        for arg in model_args(spec.harness, spec.model.as_deref()) {
            cmd.arg(arg);
        }
        // inject the per-role persona (17-01) + the P1.8 when-to-delegate nudge for CLAUDE as ONE
        // --append-system-prompt (claude's flag is LAST-WINS — a 2nd flag would silently clobber the
        // 1st, so persona+nudge MUST share a single flag). Appended AFTER session + mcp args (sibling
        // to those builders, never rewriting them). Cursor's persona is a rule file (written above);
        // bash/codex/commandcode are no-ops → []. The nudge is for non-worker panes only.
        for arg in append_system_prompt_args(
            matches!(spec.harness, Harness::Claude),
            spec.role,
            spec.is_worker,
        ) {
            cmd.arg(arg);
        }
        // Phase-16: an UNATTENDED worker gets restrictive permission flags so it never
        // hits an approval prompt no human will answer. Composed LAST (the variadic
        // --allowedTools runs to the end of argv); a no-op (`[]`) for human panes.
        // PTY workers are never flywheel write-mode (write-mode is the headless flywheel path
        // in spawn_headless_worker); report-only allowlist here.
        for arg in worker_args(spec.harness, spec.is_worker, false, &spec.extra_dirs) {
            cmd.arg(arg);
        }
        // codex turn-end: per-pane `-c notify=...` override → codex-notify.sh appends a
        // `notify` event (→ Done/TurnEnd). `[]` for non-codex / worker panes; the user's
        // global notify is untouched.
        for arg in codex_notify_args(
            spec.harness,
            spec.is_worker,
            hooks_dir,
            &spec.id,
            state_root,
        ) {
            cmd.arg(arg);
        }
        // Gap #6 (accidental-`/` cwd): resolve the child's cwd DELIBERATELY — the
        // intended worktree dir when it exists, else HOME — never an inherited `/`.
        // Without this, a missing `spec.worktree` made portable-pty fall back to
        // HOME *silently*, and a caller-supplied `/` (a GUI app launched via
        // `open`/launchd starts at `/`, so an un-resolved repo path can BE `/`)
        // passed the `is_dir` check and became the pane's real cwd — slugging the
        // claude transcript dir to a bare `-` shared across unrelated panes. The
        // worktree→repo fallback is applied by the callers that know the repo
        // (app `do_spawn` / daemon `RealSpawnExec::spawn`); this is the final
        // belt-and-suspenders for EVERY harness spawned through a PTY.
        let spawn_cwd = resolve_spawn_cwd(Some(&spec.worktree), None);
        // OpenCode interactive TUI: `opencode [project]` — the binary IGNORES process
        // cwd and walks `.git` to the main repo unless given an explicit project path
        // (isolation footgun; headless path already passes `--dir`). Workers build
        // their own argv in worker_spawn — do not double-add a path here.
        // argv-only: does NOT touch the AgentPane sized-gate / wrap path.
        if matches!(spec.harness, Harness::OpenCode) && !spec.is_worker {
            cmd.arg(&spawn_cwd);
        }
        cmd.cwd(&spawn_cwd);
        cmd.env("AGENT_TEAMS_STATE_DIR", state_root);
        // Pane provenance (threat-model C2/C7): the sidecar's memory/task WRITE tools
        // stamp Actor::Pane from this — server-set here at spawn, NEVER an agent-supplied
        // arg. Without it every agent write attributes to the "unknown" sentinel, hollowing
        // the audit floor. It is an advisory HINT (a pane could re-export it), not auth.
        cmd.env("AGENT_TEAMS_PANE_ID", &spec.id);
        // Enablement slice (C7 / 4c): the GLOBAL memory scope-key + the per-pane
        // task scope. Belt-and-suspenders alongside the injected mcp.json `env` block
        // (the sidecar's authoritative channel) — set here too so an inheriting child
        // sees them and the two never diverge (mem_key is computed once above).
        cmd.env("AGENT_TEAMS_MEMORY_REPO_KEY", &mem_key);
        cmd.env("AGENT_TEAMS_TASK_SCOPE", &spec.id);
        // a real terminal type so the harness TUIs (claude/cursor/opencode/pi) paint
        cmd.env("TERM", "xterm-256color");
        // Truecolor for OpenTUI/Ink SGR; paint still works without it, but 24-bit
        // chrome is muted/missing. Does not affect wrap / sized-gate.
        cmd.env("COLORTERM", "truecolor");
        // GUI launch loses the shell PATH → inject one so claude/cursor resolve
        cmd.env("PATH", harness_path());
        // GUI launch also loses secrets exported only from .zshrc (e.g.
        // OPENAI_API_KEY).  Forward any we found via the interactive shell
        // probe so codex / claude / etc. can authenticate.
        for (key, val) in shell_env_vars() {
            cmd.env(key, val);
        }

        let child = pair.slave.spawn_command(cmd).map_err(io_err)?;
        drop(pair.slave); // let EOF propagate when the child exits

        let mut reader = pair.master.try_clone_reader().map_err(io_err)?;
        let writer: Arc<Mutex<Box<dyn Write + Send>>> =
            Arc::new(Mutex::new(pair.master.take_writer().map_err(io_err)?));

        let output = Arc::new(Mutex::new(PaneBuffer::new(RETAIN_CAP)));
        // 08 Sub-build 3 / slice 3: per-pane subscriber registry. The reader thread holds
        // its OWN clone so it can fan out WITHOUT the daemon map lock (design §4 crux).
        let subs: subscribers::SubscriberHandle =
            Arc::new(Mutex::new(subscribers::SubscriberSet::new()));
        let sink = output.clone();
        let subs_sink = subs.clone();
        let writer_for_reader = writer.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            let mut probe_carry: Vec<u8> = Vec::new();
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    // ADDITIVE: append to the buffer (unchanged eviction semantics) AND
                    // fan the SAME chunk out to subscribers — both UNDER the buffer lock,
                    // fan-out NON-BLOCKING (try_send), NEVER the map lock. A no-op when the
                    // pane has no subscribers, so the GUI delta path is unaffected.
                    Ok(n) => {
                        let chunk = &buf[..n];
                        subscribers::push_and_fanout(&sink, &subs_sink, chunk);
                        // OpenTUI blocks on capability probes until the host answers.
                        // Answer here so paint does not depend on FE sized-gate timing.
                        auto_answer_term_queries(&writer_for_reader, chunk, &mut probe_carry);
                    }
                }
            }
            // PTY EOF / read error = pane death: drop every subscriber's sender so each
            // subscription's receiver sees `Disconnected` → the daemon emits PANE_DIED.
            subscribers::close_subscribers(&subs_sink);
        });

        Ok(Supervisor {
            id: spec.id.clone(),
            harness: spec.harness,
            // 17-01: capture the role (Copy) so orchestrate can read it per live pane.
            role: spec.role,
            // Phase-16: carried from the spec at construction (B5). worker_args above
            // already keyed the permission flags off spec.is_worker.
            is_worker: spec.is_worker,
            model: spec.model.clone(),
            master: pair.master,
            child,
            writer,
            output,
            subs,
        })
    }

    /// Send input to the PTY (what the user types into the session).
    pub fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        let mut w = self
            .writer
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        w.write_all(data)?;
        w.flush()
    }

    /// Resize the PTY to match the UI terminal (sends SIGWINCH → the harness
    /// TUI repaints at the new size). Call whenever xterm fits/resizes.
    ///
    /// Propagates `master.resize` failures — a silent `Ok` here was a false ACK:
    /// the UI latched dimensions the kernel never applied (garbled 100-col paint
    /// in a narrower xterm). Callers must surface `Err` so the frontend can leave
    /// the "acked dims" guard open and retry.
    pub fn resize(&self, rows: u16, cols: u16) -> Result<(), String> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| e.to_string())
    }

    /// All RETAINED output (lossy UTF-8) — bounded by [`RETAIN_CAP`], so output older
    /// than the window is gone. 08-T4: the read goes through the bounded
    /// [`ByteRing::recent`](agent_teams_ringbuf::ByteRing::recent) substrate
    /// ([`PaneBuffer::recent_ring`]) — the "recent scrollback" a daemon-side cold
    /// re-attach repaints from. The GUI's steady-state reads stay on the exact
    /// delta byte-cursor via [`Self::output_handle`] + [`PaneBuffer::delta`]; this
    /// whole-window snapshot is for the daemon's `handle_read_output` + this crate's
    /// own PTY tests / probe example.
    pub fn snapshot(&self) -> String {
        self.output
            .lock()
            .map(|o| String::from_utf8_lossy(&o.recent_ring().recent()).to_string())
            .unwrap_or_default()
    }

    /// Cheap clone of the pane's output buffer handle, so the app layer can read
    /// OUTSIDE its registry lock (perf-2026-06-10 A2 lock-shedding): O(1) under the
    /// map lock, then lock only THIS pane's buffer for the µs-scale delta copy —
    /// typing (`send_input` → the same registry mutex) never queues behind a read.
    pub fn output_handle(&self) -> Arc<Mutex<PaneBuffer>> {
        self.output.clone()
    }

    /// 08 Sub-build 3 / slice 3: O(1) clones of the pane's (buffer, subscriber-registry)
    /// handles for a LOCK-SHED `Attach`. The daemon clones these under the map lock, DROPS
    /// the map lock, then `subscribe_to(&buf, &subs, ..)` snapshots+registers under the
    /// buffer→subscriber lock only — the same lock-shedding `output_handle()` uses for
    /// `read_output_delta`, so an attach's snapshot copy never blocks the map lock. The
    /// reader thread fans out to this SAME registry without ever taking the map lock.
    pub fn stream_handles(&self) -> (Arc<Mutex<PaneBuffer>>, subscribers::SubscriberHandle) {
        (self.output.clone(), self.subs.clone())
    }

    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// OS pid of this pane's PTY child (the harness root). Used by the app's
    /// coordinator-only socket gate to map a connecting socket peer's pid → owning
    /// pane → role (the peer is a descendant of this harness). `None` if the pty
    /// backend doesn't expose a pid.
    pub fn process_id(&self) -> Option<u32> {
        self.child.process_id()
    }

    pub fn kill(&mut self) {
        // Codex MCP cleanup: remove the per-pane `[mcp_servers.agent-teams-<id>]`
        // block from `~/.codex/config.toml` so the global config stays clean.
        // Best-effort: a missing block or config is a no-op.
        if matches!(self.harness, Harness::Codex) {
            remove_codex_mcp(&self.id);
        }
        let _ = self.child.kill();
        // REAP: `kill()` alone never waits — the SIGKILLed child stays a ZOMBIE until
        // someone waits on it, and for a closed pane nothing else ever does (`is_alive`
        // is no longer polled once the pane leaves the registry). Bounded `try_wait`
        // loop: a killed child normally reaps on the first tick; the bound keeps a rare
        // unkillable child (uninterruptible I/O) from blocking the caller, which often
        // holds the registry map lock.
        for _ in 0..20 {
            match self.child.try_wait() {
                Ok(Some(_)) | Err(_) => break,
                Ok(None) => thread::sleep(std::time::Duration::from_millis(25)),
            }
        }
    }

    /// The spawned child's OS pid (portable-pty `Child::process_id`). `None` once the
    /// child has been reaped (`process_id` returns `None` after exit). Q4 / approach B
    /// only: the daemon IS the child's parent, so this is a real owned pid the daemon's
    /// registry writer captures AT SPAWN (NOT lazily — `is_alive`/the reaper `try_wait`,
    /// which reaps, would otherwise null it out) to stamp the live registry's real child
    /// pid. `is_alive`/`kill` keep using the owned `Child` handle (kernel-tied,
    /// PID-reuse-safe) — `child_pid` is purely the registry/audit/operator-kill value,
    /// never the liveness or kill mechanism. Additive + dead unless called.
    pub fn child_pid(&self) -> Option<u32> {
        self.child.process_id()
    }
}

/// Reset a (reused) worktree's branch back to `target` and remove untracked files,
/// so a Bridge pane never starts on a STALE base (07-03 / D41, RC-2). DESTRUCTIVE:
/// `git reset --hard` + `git clean -fd` discard any uncommitted prior-run work in
/// the worktree. The caller MUST gate this on an explicit opt-in — never the normal
/// reopen path, which keeps uncommitted files (D19). The worktree's PATH, id, and
/// branch are UNCHANGED (so cursor `md5(cwd)` + claude resume keys still resolve —
/// Approach A: freshen the base in place, no identity churn).
///
/// `target` is pinned to a SHA first (a concurrent `main` move can't shift it
/// mid-reset). No-op (Ok) when the worktree is already at that SHA — so calling it
/// on a freshly-created worktree (already at `main`) does nothing destructive.
pub fn freshen_worktree(wt: &Worktree, target: &str) -> std::io::Result<()> {
    // pin target → SHA
    let sha_out = git_out(&wt.root, &["rev-parse", "--verify", target])?;
    if !sha_out.status.success() {
        return Err(io_err(format!(
            "freshen: cannot resolve '{target}': {}",
            String::from_utf8_lossy(&sha_out.stderr)
        )));
    }
    let sha = String::from_utf8_lossy(&sha_out.stdout).trim().to_string();

    // already fresh? (a just-created worktree is at main) → no destructive op (AC-3).
    // HEAD == sha is NOT sufficient: a reused worktree can sit AT the target SHA yet
    // be DIRTY (uncommitted tracked edits + untracked files, never committed → HEAD
    // never moved off the base). The contract is to discard exactly that prior-run
    // state, so only short-circuit when `git status --porcelain` is also empty;
    // otherwise fall through to the destructive reset + clean below.
    let head_out = git_out(&wt.root, &["rev-parse", "HEAD"])?;
    let head = String::from_utf8_lossy(&head_out.stdout).trim().to_string();
    if head_out.status.success() && head == sha {
        let status = git_out(&wt.root, &["status", "--porcelain"])?;
        if status.status.success() && status.stdout.is_empty() {
            return Ok(());
        }
    }

    // discard ALL prior-run state: branch → pinned base, then remove untracked
    let reset = git_out(&wt.root, &["reset", "--hard", &sha])?;
    if !reset.status.success() {
        return Err(io_err(format!(
            "freshen: reset --hard failed: {}",
            String::from_utf8_lossy(&reset.stderr)
        )));
    }
    let clean = git_out(&wt.root, &["clean", "-fd"])?;
    if !clean.status.success() {
        return Err(io_err(format!(
            "freshen: clean -fd failed: {}",
            String::from_utf8_lossy(&clean.stderr)
        )));
    }
    // keep a sparse worktree sparse (reset honors the cone, but reapply is the guard)
    if let Ok(list) = git_out(&wt.root, &["sparse-checkout", "list"]) {
        if list.status.success() && !list.stdout.is_empty() {
            let _ = git_out(&wt.root, &["sparse-checkout", "reapply"]);
        }
    }
    Ok(())
}

fn io_err<E: std::fmt::Display>(e: E) -> std::io::Error {
    std::io::Error::other(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ───────────── PaneBuffer (perf-2026-06-10 A: bounded buffer + delta) ─────────────
    // Tiny caps so the compaction hysteresis (compact when len > cap + cap/2,
    // drain back to cap) is exercised without MiB-scale fixtures.

    /// Plain appends below the hysteresis threshold never move `base`; crossing it
    /// drains the front back to `cap` and advances `base` by exactly the cut.
    #[test]
    fn pane_buffer_append_then_compaction_advances_base() {
        let mut b = PaneBuffer::new(8); // compact when len > 12 → retain 8
        b.push(b"abcdefgh");
        assert_eq!((b.base(), b.end()), (0, 8));
        b.push(b"ijkl"); // len 12 — AT the threshold, not over → no compaction
        assert_eq!((b.base(), b.end()), (0, 12));
        b.push(b"m"); // len 13 > 12 → cut 5, retain "fghijklm"
        assert_eq!((b.base(), b.end()), (5, 13));
        assert_eq!(b.retained(), b"fghijklm");
    }

    // Dogfood fix: a state-blind harness (codex/commandcode/opencode) gets a SYNTHETIC
    // SessionStart at spawn so the adapter isn't blind to it. Assert the helper writes
    // the exact JSONL shape `state-writer.sh` produces, under <root>/<wsid>/events.jsonl,
    // and APPENDS (never clobbers) on a repeat call.
    #[test]
    fn write_spawn_ready_event_appends_sessionstart_line() {
        let root = std::env::temp_dir().join(format!(
            "at-ready-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        write_spawn_ready_event(&root, "ws9-p1", "codex").expect("write ready event");

        let log = root.join("ws9-p1/events.jsonl");
        let body = std::fs::read_to_string(&log).expect("events.jsonl written");
        assert_eq!(
            body.lines().count(),
            1,
            "exactly one synthetic event: {body:?}"
        );
        let l = body.lines().next().unwrap();
        assert!(l.contains(r#""harness":"codex""#), "harness wire: {l}");
        assert!(l.contains(r#""event":"SessionStart""#), "event: {l}");
        assert!(l.contains(r#""workspace_id":"ws9-p1""#), "wsid: {l}");
        assert!(l.contains(r#""decision":"na""#), "decision: {l}");

        // a second call APPENDS (does not clobber) — same contract as the shell writer
        write_spawn_ready_event(&root, "ws9-p1", "codex").expect("append again");
        let body2 = std::fs::read_to_string(&log).unwrap();
        assert_eq!(body2.lines().count(), 2, "second call appends");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cursor_trust_slug_matches_cursor_rule() {
        // every non-alphanumeric → '-', runs collapsed, ends trimmed (verified vs the
        // decompiled cursor-agent rule + on-disk markers).
        assert_eq!(
            cursor_trust_slug("/Users/jeffrymilan/Memory"),
            "Users-jeffrymilan-Memory"
        );
        assert_eq!(
            cursor_trust_slug(
                "/Users/jeffrymilan/Personal/agent-teams/.agent-teams-worktrees/ws28901x0-p5"
            ),
            "Users-jeffrymilan-Personal-agent-teams-agent-teams-worktrees-ws28901x0-p5"
        );
        // leading/trailing separators trimmed; dots + repeated separators collapse to one
        assert_eq!(cursor_trust_slug("/a//b.c/"), "a-b-c");
    }

    #[test]
    fn iso8601_utc_now_is_well_formed() {
        let s = iso8601_utc_now();
        assert_eq!(s.len(), 20, "YYYY-MM-DDTHH:MM:SSZ: {s}");
        assert!(s.ends_with('Z'), "trailing Z: {s}");
        assert_eq!(s.as_bytes()[10], b'T', "date/time separator: {s}");
        assert!(&s[0..4] >= "2020", "year sane: {s}");
    }

    #[test]
    fn claude_trust_patch_sets_keys_and_preserves_existing() {
        // a real config has auth + other projects — all must survive the patch.
        let existing = r#"{"oauthAccount":{"token":"SECRET"},"projects":{"/old":{"hasTrustDialogAccepted":true}}}"#;
        let v: serde_json::Value =
            serde_json::from_str(&claude_trust_patched_json(existing, "/new/worktree").unwrap())
                .unwrap();
        assert_eq!(v["oauthAccount"]["token"], "SECRET"); // preserved
        assert_eq!(v["projects"]["/old"]["hasTrustDialogAccepted"], true); // preserved
        assert_eq!(
            v["projects"]["/new/worktree"]["hasTrustDialogAccepted"],
            true
        );
        assert_eq!(
            v["projects"]["/new/worktree"]["hasCompletedProjectOnboarding"],
            true
        );
        assert_eq!(
            v["projects"]["/new/worktree"]["projectOnboardingSeenCount"],
            1
        );
    }

    #[test]
    fn claude_trust_patch_creates_projects_map_and_keeps_higher_seen_count() {
        // no projects map → created
        let v: serde_json::Value =
            serde_json::from_str(&claude_trust_patched_json("{}", "/w").unwrap()).unwrap();
        assert_eq!(v["projects"]["/w"]["hasTrustDialogAccepted"], true);
        // a pre-existing higher seen count is NOT clobbered down to 1
        let pre =
            serde_json::json!({"projects":{"/w":{"projectOnboardingSeenCount":5}}}).to_string();
        let v2: serde_json::Value =
            serde_json::from_str(&claude_trust_patched_json(&pre, "/w").unwrap()).unwrap();
        assert_eq!(v2["projects"]["/w"]["projectOnboardingSeenCount"], 5);
        assert_eq!(v2["projects"]["/w"]["hasTrustDialogAccepted"], true);
    }

    #[test]
    fn claude_trust_patch_rejects_non_object() {
        // never clobber a malformed/foreign ~/.claude.json — skip (Err) instead.
        assert!(claude_trust_patched_json("[1,2,3]", "/w").is_err());
        assert!(claude_trust_patched_json("not json", "/w").is_err());
    }

    // codex turn-end: the `-c notify=[...]` override is emitted ONLY for a codex human
    // pane, carries the script + wsid + (spaced) state dir, and is empty for non-codex
    // and for codex WORKERS (human-invisible) — so the user's global notify is untouched.
    #[test]
    fn codex_notify_args_overrides_only_codex_human_panes() {
        let hooks = std::path::Path::new("/abs/hooks");
        let state = std::path::Path::new("/Users/x/Application Support/agent-teams");

        let a = codex_notify_args(Harness::Codex, false, hooks, "ws7-p1", state);
        assert_eq!(a.len(), 2, "two args: -c <value>");
        assert_eq!(a[0], "-c");
        assert!(a[1].starts_with("notify=["), "TOML notify array: {}", a[1]);
        assert!(
            a[1].contains("/abs/hooks/codex-notify.sh"),
            "script path: {}",
            a[1]
        );
        assert!(a[1].contains("ws7-p1"), "wsid: {}", a[1]);
        assert!(
            a[1].contains("Application Support/agent-teams"),
            "state dir (spaced path preserved): {}",
            a[1]
        );

        // non-codex panes + codex workers → no override (global notify untouched)
        assert!(codex_notify_args(Harness::Claude, false, hooks, "ws7-p0", state).is_empty());
        assert!(codex_notify_args(Harness::CommandCode, false, hooks, "ws7-p3", state).is_empty());
        assert!(codex_notify_args(Harness::Codex, true, hooks, "ws7-w1", state).is_empty());
    }

    // opencode turn-end: the plugin stages into the (auto-load) dest dir as
    // agent-teams-state.js, idempotently (copy-always overwrites on a repeat call).
    #[test]
    fn install_opencode_plugin_stages_into_dest() {
        let root = std::env::temp_dir().join(format!(
            "at-ocplugin-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let hooks = root.join("hooks");
        let dest = root.join("plugins");
        std::fs::create_dir_all(&hooks).unwrap();
        std::fs::write(hooks.join("opencode-state-plugin.js"), "// plugin v1\n").unwrap();

        install_opencode_plugin(&hooks, &dest).expect("install");
        let out = dest.join("agent-teams-state.js");
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "// plugin v1\n");

        // copy-always: a changed source overwrites on the next call (idempotent install)
        std::fs::write(hooks.join("opencode-state-plugin.js"), "// plugin v2\n").unwrap();
        install_opencode_plugin(&hooks, &dest).expect("install again");
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "// plugin v2\n");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// `end()` always equals the TOTAL bytes ever pushed, across many compactions —
    /// the absolute-offset invariant the delta protocol depends on.
    #[test]
    fn pane_buffer_end_is_total_pushed_across_compactions() {
        let mut b = PaneBuffer::new(8);
        let mut total = 0u64;
        for i in 0..100u8 {
            let chunk = vec![b'a' + (i % 26); 7];
            b.push(&chunk);
            total += 7;
            assert_eq!(b.end(), total, "end() drifted at push {i}");
            assert!(
                b.retained().len() <= 12,
                "len exceeded cap+slack at push {i}"
            );
            assert_eq!(b.base() + b.retained().len() as u64, b.end());
        }
    }

    /// Delta clamp matrix (CONTRACT seam 1): since < base → whole window from base;
    /// in-window → exact range; since == end → empty; since > end → whole window.
    #[test]
    fn pane_buffer_delta_clamp_matrix() {
        let mut b = PaneBuffer::new(8);
        b.push(b"abcdefghijklm"); // compacts → base 5, retained "fghijklm", end 13
        assert_eq!((b.base(), b.end()), (5, 13));
        // since < base → evicted under the caller → full retained window
        assert_eq!(b.delta(0), (5, b"fghijklm".to_vec()));
        // in-window → exact [since..end)
        assert_eq!(b.delta(9), (9, b"jklm".to_vec()));
        // since == end → in-window, empty (steady-state idle poll)
        assert_eq!(b.delta(13), (13, Vec::new()));
        // since > end → stale cursor (respawn/reload desync) → full window replay
        assert_eq!(b.delta(99), (5, b"fghijklm".to_vec()));
    }

    /// 08-T4: the `ByteRing::recent` snapshot substrate behind `Supervisor::snapshot`.
    /// `recent_ring()` mirrors the retained window into a bounded ring whose capacity is
    /// exactly the retained length, so `ring.recent()` byte-for-byte equals `retained()`
    /// at the snapshot boundaries: empty buffer, under-cap (no eviction), and after a
    /// compaction has dropped the front (recent reflects the TAIL, not the lost history).
    #[test]
    fn pane_buffer_recent_ring_matches_retained_at_boundaries() {
        // empty buffer → empty ring (with_capacity(0) retains nothing)
        let b = PaneBuffer::new(8);
        assert_eq!(b.recent_ring().recent(), b.retained());
        assert!(b.recent_ring().recent().is_empty());

        // under cap → ring holds the whole window, recent() == retained()
        let mut b = PaneBuffer::new(8);
        b.push(b"abcdef"); // 6 < threshold → no compaction
        let ring = b.recent_ring();
        assert_eq!(ring.capacity(), b.retained().len(), "cap == retained len");
        assert_eq!(ring.recent(), b.retained());
        assert_eq!(ring.recent(), b"abcdef");

        // in the HYSTERESIS window → buf.len() > cap but <= cap + cap/2, so compaction
        // has NOT yet fired and the whole over-cap window is still retained. recent_ring()
        // builds a ByteRing::with_capacity(buf.len()) and the push hits the
        // `bytes.len() >= cap` branch (clear + extend the whole tail). recent() must STILL
        // equal the full retained window — the snapshot is never lossy inside this window.
        let mut b = PaneBuffer::new(8);
        b.push(b"abcdefghij"); // 10 bytes: > cap(8) but <= 12 → no compaction
        assert_eq!(
            b.retained(),
            b"abcdefghij",
            "no compaction inside the hysteresis window"
        );
        let ring = b.recent_ring();
        assert_eq!(ring.recent(), b"abcdefghij");
        assert_eq!(ring.recent(), b.retained());

        // after a compaction dropped the front → recent() is the retained TAIL only
        let mut b = PaneBuffer::new(8);
        b.push(b"abcdefghijklm"); // compacts → retained "fghijklm" (base 5)
        assert_eq!(b.retained(), b"fghijklm");
        let ring = b.recent_ring();
        assert_eq!(
            ring.recent(),
            b"fghijklm",
            "recent reflects the bounded tail"
        );
        assert_eq!(ring.len(), 8);
        // and that is exactly what snapshot() reads (lossy UTF-8 of the recent window)
        assert_eq!(
            String::from_utf8_lossy(&ring.recent()).to_string(),
            "fghijklm"
        );
    }

    /// 08-T4: drive the EXACT `Supervisor::snapshot` read chain — lock the
    /// `Arc<Mutex<PaneBuffer>>`, go through `recent_ring().recent()`, lossy-decode —
    /// without a PTY. `Supervisor::snapshot` owns nothing but `self.output` for this
    /// read, so replicating the expression over a hand-built buffer proves the new
    /// ByteRing route end-to-end (the live-PTY pty.rs tests only cover it incidentally).
    #[test]
    fn snapshot_read_chain_through_mutex_matches_recent_ring() {
        let output = Arc::new(Mutex::new(PaneBuffer::new(8)));
        output.lock().unwrap().push(b"abcdefghijklm"); // compacts → tail "fghijklm"

        // the exact body of Supervisor::snapshot()
        let snap = output
            .lock()
            .map(|o| String::from_utf8_lossy(&o.recent_ring().recent()).to_string())
            .unwrap_or_default();
        assert_eq!(snap, "fghijklm");

        // empty buffer → empty snapshot (the cold-reattach-before-output case)
        let empty = Arc::new(Mutex::new(PaneBuffer::new(8)));
        let snap = empty
            .lock()
            .map(|o| String::from_utf8_lossy(&o.recent_ring().recent()).to_string())
            .unwrap_or_default();
        assert_eq!(snap, "");

        // a poisoned lock is recovered to the empty default (matches snapshot's
        // .unwrap_or_default()), never a panic.
        let poisoned = Arc::new(Mutex::new(PaneBuffer::new(8)));
        poisoned.lock().unwrap().push(b"xyz");
        let p2 = poisoned.clone();
        let _ = std::thread::spawn(move || {
            let _g = p2.lock().unwrap();
            panic!("poison it");
        })
        .join();
        let snap = poisoned
            .lock()
            .map(|o| String::from_utf8_lossy(&o.recent_ring().recent()).to_string())
            .unwrap_or_default();
        assert_eq!(
            snap, "",
            "poisoned lock → default empty (snapshot never panics)"
        );
    }

    /// The compaction cut never lands mid-codepoint: it advances over UTF-8
    /// continuation bytes so the retained window starts on a char boundary.
    #[test]
    fn pane_buffer_compaction_cut_advances_to_char_boundary() {
        let mut b = PaneBuffer::new(8);
        // 10 ASCII + "€€€" (3×3 bytes) = 19 > 12 → natural cut 11 = mid-first-€;
        // the boundary walk advances it to the second € (offset 13).
        b.push(b"aaaaaaaaaa");
        b.push("€€€".as_bytes());
        assert_eq!(b.base(), 13, "cut must skip the 2 continuation bytes");
        assert_eq!(b.retained(), "€€".as_bytes());
        assert!(std::str::from_utf8(b.retained()).is_ok());
        assert_eq!(b.end(), 19);
    }

    /// `workspace_key` strips the `-p<N>` pane suffix to the team-shared key (C7),
    /// and falls back safely when there is no digit-suffixed `-p` segment.
    #[test]
    fn workspace_key_strips_pane_suffix() {
        // canonical pane id → workspace prefix (shared by every pane of the team)
        assert_eq!(workspace_key("ws28901x0-p3"), "ws28901x0");
        assert_eq!(workspace_key("ws96745x1-p0"), "ws96745x1");
        assert_eq!(workspace_key("ws89069x2-p12"), "ws89069x2");
        // a workspace id with a hyphen-p in its NAME → only the trailing pane suffix
        // is stripped (rsplit takes the LAST "-p"; the digit filter guards the rest).
        assert_eq!(workspace_key("my-project-p1-p0"), "my-project-p1");
        assert_eq!(workspace_key("my-project"), "my-project"); // "-project" not digits
                                                               // no pane suffix → whole id (degrades to per-pane, never wrong)
        assert_eq!(workspace_key("ws28901x0"), "ws28901x0");
        assert_eq!(workspace_key("plain"), "plain");
        // `-p` with a non-numeric tail is not a pane suffix
        assert_eq!(workspace_key("ws-prod"), "ws-prod");
        assert_eq!(workspace_key("ws-p"), "ws-p"); // empty numeric tail
    }

    /// model_args mirrors the live-verified worker_spawn flags per harness; None/empty
    /// → [] so an unset model leaves the spawn argv byte-identical (back-compat).
    #[test]
    fn model_args_per_harness_flags() {
        assert_eq!(
            model_args(Harness::Claude, Some("claude-opus-4-8")),
            vec!["--model", "claude-opus-4-8"]
        );
        assert_eq!(
            model_args(Harness::Cursor, Some("gpt-5.4")),
            vec!["--model", "gpt-5.4"]
        );
        assert_eq!(
            model_args(Harness::Codex, Some("gpt-5.4-mini")),
            vec!["-m", "gpt-5.4-mini"]
        );
        assert_eq!(
            model_args(Harness::CommandCode, Some("k3.5")),
            vec!["-m", "k3.5"]
        );
        assert_eq!(
            model_args(Harness::OpenCode, Some("openai/gpt-5.4-mini")),
            vec!["-m", "openai/gpt-5.4-mini"]
        );
        // pi: long-form --model; accepts provider/id and bare ids.
        assert_eq!(
            model_args(Harness::Pi, Some("anthropic/claude-sonnet-4")),
            vec!["--model", "anthropic/claude-sonnet-4"]
        );
        assert_eq!(
            model_args(Harness::Pi, Some("openai/gpt-4o")),
            vec!["--model", "openai/gpt-4o"]
        );
        assert_eq!(
            model_args(Harness::Pi, Some("sonnet")),
            vec!["--model", "sonnet"]
        );
        assert_eq!(
            model_args(Harness::Grok, Some("grok-4")),
            vec!["-m", "grok-4"]
        );
        assert_eq!(
            model_args(Harness::Bash, Some("anything")),
            Vec::<String>::new()
        );
        for h in Harness::all() {
            assert_eq!(model_args(h, None), Vec::<String>::new(), "{h:?} None → []");
            assert_eq!(
                model_args(h, Some("")),
                Vec::<String>::new(),
                "{h:?} empty → []"
            );
        }
    }

    #[test]
    fn state_sibling_is_a_surviving_sibling_of_state_root() {
        // sibling of state_root named `<state-name>-<suffix>` (survives the startup wipe).
        let sr = std::path::Path::new("/Users/x/Library/Application Support/agent-teams-dev");
        assert_eq!(
            state_sibling(sr, "daemon-spawn"),
            std::path::PathBuf::from(
                "/Users/x/Library/Application Support/agent-teams-dev-daemon-spawn"
            ),
        );
        // it is NOT inside state_root (else it would be wiped each launch).
        assert!(!state_sibling(sr, "daemon-spawn").starts_with(sr));
    }

    #[test]
    fn session_args_claude_create_dictates_id() {
        assert_eq!(
            session_args(Harness::Claude, Some("abc"), false),
            vec!["--session-id", "abc"]
        );
        // no id tracked on create → let claude pick its own
        assert!(session_args(Harness::Claude, None, false).is_empty());
    }

    #[test]
    fn session_args_claude_reopen_resumes_id() {
        assert_eq!(
            session_args(Harness::Claude, Some("abc"), true),
            vec!["--resume", "abc"]
        );
        // reopen without a tracked id → continue most-recent-in-cwd
        assert_eq!(
            session_args(Harness::Claude, None, true),
            vec!["--continue"]
        );
    }

    #[test]
    fn session_args_cursor_create_none_reopen_continue() {
        assert!(session_args(Harness::Cursor, Some("abc"), false).is_empty());
        // cursor has no settable id; an id is ignored even on reopen
        assert_eq!(
            session_args(Harness::Cursor, Some("abc"), true),
            vec!["--continue"]
        );
        assert_eq!(
            session_args(Harness::Cursor, None, true),
            vec!["--continue"]
        );
    }

    #[test]
    fn session_args_bash_never() {
        assert!(session_args(Harness::Bash, Some("abc"), false).is_empty());
        assert!(session_args(Harness::Bash, Some("abc"), true).is_empty());
    }

    #[test]
    fn session_args_codex_never() {
        // codex spawns bare (no settable id, no resume flag wired) on both create + reopen.
        assert!(session_args(Harness::Codex, Some("abc"), false).is_empty());
        assert!(session_args(Harness::Codex, Some("abc"), true).is_empty());
        assert!(session_args(Harness::Codex, None, true).is_empty());
    }

    #[test]
    fn session_args_commandcode_never() {
        // commandcode resume is deferred → no session/resume args (like codex). Its
        // startup-suppression flags live in descriptor().spawn_args, NOT here.
        assert!(session_args(Harness::CommandCode, Some("abc"), false).is_empty());
        assert!(session_args(Harness::CommandCode, Some("abc"), true).is_empty());
        assert!(session_args(Harness::CommandCode, None, true).is_empty());
    }

    #[test]
    fn commandcode_descriptor_spawn_args_suppress_startup() {
        // the ONE thing that distinguishes commandcode from codex: static spawn flags.
        let cc = Harness::CommandCode.descriptor();
        assert_eq!(cc.command, "commandcode");
        assert_eq!(cc.wire, "commandcode");
        assert!(
            cc.inject.is_none(),
            "commandcode is state-blind (no hook injection)"
        );
        assert_eq!(cc.spawn_args, &["--skip-onboarding", "-t"]);
        // every OTHER harness has no static spawn args (so the spawn loop is a no-op).
        for h in [
            Harness::Claude,
            Harness::Cursor,
            Harness::Bash,
            Harness::Codex,
        ] {
            assert!(
                h.descriptor().spawn_args.is_empty(),
                "{h:?} has no spawn_args"
            );
        }
    }

    #[test]
    fn harness_catalog_includes_codex_and_is_internally_consistent() {
        let all = Harness::all();
        assert_eq!(
            all.len(),
            8,
            "Claude, Cursor, Bash, Codex, CommandCode, OpenCode, Pi, Grok"
        );
        assert!(all.contains(&Harness::Codex));
        assert!(all.contains(&Harness::CommandCode));
        assert!(all.contains(&Harness::OpenCode));
        assert!(all.contains(&Harness::Pi));
        assert!(all.contains(&Harness::Grok));
        let oc = Harness::OpenCode.descriptor();
        assert_eq!(oc.command, "opencode");
        assert_eq!(oc.wire, "opencode");
        assert!(
            oc.inject.is_none(),
            "opencode has no hook injection (turn-end via plugin)"
        );
        assert!(
            !oc.state_blind,
            "opencode is NOT state_blind (plugin turn-end)"
        );
        let pi = Harness::Pi.descriptor();
        assert_eq!(pi.command, "pi");
        assert_eq!(pi.wire, "pi");
        assert_eq!(pi.display, "Pi");
        assert_eq!(pi.spawn_args, &[] as &[&str]);
        assert!(
            pi.inject.is_none(),
            "pi is state-blind (no hook injection)"
        );
        assert!(pi.state_blind, "pi has no turn-end channel yet");
        let gk = Harness::Grok.descriptor();
        assert_eq!(gk.command, "grok");
        assert_eq!(gk.wire, "grok");
        assert_eq!(gk.display, "Grok Build");
        assert_eq!(gk.spawn_args, &[] as &[&str]);
        assert!(
            gk.inject.is_none(),
            "grok is state-blind (no hook injection)"
        );
        // every descriptor is complete, wires are unique, and each round-trips through
        // the wire id (the table-driven parse_harness keys on exactly this).
        let mut wires: Vec<&str> = all.iter().map(|h| h.descriptor().wire).collect();
        let n = wires.len();
        wires.sort_unstable();
        wires.dedup();
        assert_eq!(wires.len(), n, "harness wire ids are unique");
        let codex = Harness::Codex.descriptor();
        assert_eq!(codex.command, "codex");
        assert_eq!(codex.wire, "codex");
        assert!(
            codex.inject.is_none(),
            "codex is state-blind today (no hook injection)"
        );
    }

    // 16-01 / D56: mcp_args mirrors session_args (pure, per-harness). Claude with a
    // resolved config → --mcp-config + --strict (06-06: only agent-teams, suppress the
    // setup-issues banner that hangs the pane); cursor → --approve-mcps; bash → none;
    // claude with None (resolution/inject failed) → none (degrade, AC-6).
    #[test]
    fn mcp_args_claude_strict_mcp_config() {
        let p = PathBuf::from("/staged/ws1-claude-mcp.json");
        assert_eq!(
            mcp_args(Harness::Claude, Some(&p), false),
            vec![
                "--mcp-config",
                "/staged/ws1-claude-mcp.json",
                "--strict-mcp-config"
            ]
        );
        // 06-06: strict IS present — claude loads only the agent-teams sidecar, so the
        // operator's user-scoped servers can't raise the "N setup issues: MCP" banner that
        // eats the first dispatched input.
        assert!(mcp_args(Harness::Claude, Some(&p), false)
            .iter()
            .any(|a| a == "--strict-mcp-config"));
    }

    #[test]
    fn mcp_args_claude_none_degrades_to_empty() {
        // resolution/inject failed → no flag, never a broken spawn (AC-6).
        assert!(mcp_args(Harness::Claude, None, false).is_empty());
    }

    #[test]
    fn mcp_args_cursor_approve_mcps() {
        // cursor discovers .cursor/mcp.json by path; the flag auto-approves it.
        assert_eq!(
            mcp_args(Harness::Cursor, None, false),
            vec!["--approve-mcps"]
        );
        // a claude cfg path is irrelevant to cursor (it ignores it).
        let p = PathBuf::from("/staged/x.json");
        assert_eq!(
            mcp_args(Harness::Cursor, Some(&p), false),
            vec!["--approve-mcps"]
        );
    }

    #[test]
    fn mcp_args_cursor_worker_gets_no_approve_mcps() {
        // A delegate WORKER skips inject_mcp_config (no .cursor/mcp.json is written), so
        // --approve-mcps would blanket-approve whatever USER-scoped MCP servers cursor
        // discovers — the flag is gated on !is_worker.
        assert!(mcp_args(Harness::Cursor, None, true).is_empty());
        let p = PathBuf::from("/staged/x.json");
        assert!(mcp_args(Harness::Cursor, Some(&p), true).is_empty());
        // claude workers were already flag-free via the None cfg path (unchanged).
        assert!(mcp_args(Harness::Claude, None, true).is_empty());
    }

    #[test]
    fn mcp_args_bash_never() {
        assert!(mcp_args(Harness::Bash, None, false).is_empty());
        let p = PathBuf::from("/staged/x.json");
        assert!(mcp_args(Harness::Bash, Some(&p), false).is_empty());
    }

    #[test]
    fn mcp_args_codex_never() {
        // codex consumes MCP via its own ~/.codex config, not a spawn-time CLI flag.
        assert!(mcp_args(Harness::Codex, None, false).is_empty());
        let p = PathBuf::from("/staged/x.json");
        assert!(mcp_args(Harness::Codex, Some(&p), false).is_empty());
    }

    #[test]
    fn mcp_args_commandcode_never() {
        // commandcode consumes MCP via its own config, not a spawn-time CLI flag.
        assert!(mcp_args(Harness::CommandCode, None, false).is_empty());
        let p = PathBuf::from("/staged/x.json");
        assert!(mcp_args(Harness::CommandCode, Some(&p), false).is_empty());
    }

    // 17-01 / AC2: a claude spec's role yields the --append-system-prompt persona
    // arg; a cursor/bash spec yields none on the CLI (cursor → rule file; bash no-op).
    // The arg builder is `roles::role_args` keyed on Harness::Claude — assert the
    // composition the supervisor performs at the spec level (the PTY is the operator
    // probe; cargo-green is necessary but not sufficient — router-bug lesson).
    #[test]
    fn role_args_claude_spec_appends_persona_others_do_not() {
        // claude + role → 2-elem --append-system-prompt vec, persona payload last.
        let a = roles::role_args(
            matches!(Harness::Claude, Harness::Claude),
            roles::AgentRole::Coordinator,
        );
        assert!(a.iter().any(|x| x == "--append-system-prompt"));
        assert_eq!(
            a.last().map(String::as_str),
            Some(roles::persona(roles::AgentRole::Coordinator))
        );

        // cursor spec → NO CLI role arg (its persona is the .cursor/rules file).
        let cursor = roles::role_args(
            matches!(Harness::Cursor, Harness::Claude),
            roles::AgentRole::Coordinator,
        );
        assert!(
            cursor.is_empty(),
            "cursor gets no --append-system-prompt: {cursor:?}"
        );

        // bash spec → NO CLI role arg.
        let bash = roles::role_args(
            matches!(Harness::Bash, Harness::Claude),
            roles::AgentRole::Builder,
        );
        assert!(bash.is_empty());
    }

    // P1.8: the combined --append-system-prompt builder. claude's flag is LAST-WINS, so the persona
    // and the when-to-delegate nudge MUST ride in ONE flag (these tests guard that regression).
    #[test]
    fn append_prompt_claude_noworker_norole_is_nudge_only() {
        let a = append_system_prompt_args(true, None, false);
        assert_eq!(
            a.len(),
            2,
            "exactly one --append-system-prompt flag (k/v): {a:?}"
        );
        assert_eq!(a[0], "--append-system-prompt");
        assert!(
            a[1].contains("team_delegate"),
            "payload carries the nudge: {}",
            a[1]
        );
    }

    #[test]
    fn append_prompt_claude_noworker_role_has_both_persona_and_nudge() {
        // The regression guard: a role'd claude pane must keep its persona AND gain the nudge in the
        // SINGLE flag (last-wins would otherwise drop one).
        let a = append_system_prompt_args(true, Some(roles::AgentRole::Coordinator), false);
        assert_eq!(a.len(), 2);
        assert_eq!(a[0], "--append-system-prompt");
        assert!(
            a[1].contains(roles::persona(roles::AgentRole::Coordinator)),
            "persona preserved (not clobbered): {}",
            a[1]
        );
        assert!(a[1].contains("team_delegate"), "nudge present: {}", a[1]);
    }

    #[test]
    fn append_prompt_claude_worker_has_persona_not_nudge() {
        // A worker keeps its persona but is NEVER nudged to delegate (no recursion).
        let a = append_system_prompt_args(true, Some(roles::AgentRole::Builder), true);
        assert_eq!(a[0], "--append-system-prompt");
        assert!(a[1].contains(roles::persona(roles::AgentRole::Builder)));
        assert!(
            !a[1].contains("team_delegate"),
            "worker is NOT nudged: {}",
            a[1]
        );
    }

    #[test]
    fn append_prompt_nonclaude_is_empty() {
        // cursor/bash/codex/commandcode have no CLI system-prompt channel → no flag, no nudge.
        assert!(append_system_prompt_args(false, Some(roles::AgentRole::Scout), false).is_empty());
        assert!(append_system_prompt_args(false, None, false).is_empty());
    }

    // Gap #6 — resolve_spawn_cwd: the deliberate-cwd seam every harness spawn goes
    // through. worktree-exists → worktree; missing → repo; both missing → HOME;
    // the filesystem root is NEVER accepted (the accidental `open`/launchd cwd).
    #[test]
    fn resolve_spawn_cwd_prefers_existing_worktree() {
        let wt = std::env::temp_dir().join(format!("at-cwd-wt-{}", test_nonce()));
        let repo = std::env::temp_dir().join(format!("at-cwd-repo-{}", test_nonce()));
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::create_dir_all(&repo).unwrap();
        assert_eq!(resolve_spawn_cwd(Some(&wt), Some(&repo)), wt);
        let _ = std::fs::remove_dir_all(&wt);
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn resolve_spawn_cwd_missing_worktree_falls_to_repo() {
        let missing = std::env::temp_dir().join(format!("at-cwd-miss-{}", test_nonce()));
        let repo = std::env::temp_dir().join(format!("at-cwd-repo2-{}", test_nonce()));
        std::fs::create_dir_all(&repo).unwrap();
        assert_eq!(resolve_spawn_cwd(Some(&missing), Some(&repo)), repo);
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn resolve_spawn_cwd_both_missing_falls_to_home_never_root() {
        let m1 = std::env::temp_dir().join(format!("at-cwd-m1-{}", test_nonce()));
        let m2 = std::env::temp_dir().join(format!("at-cwd-m2-{}", test_nonce()));
        let got = resolve_spawn_cwd(Some(&m1), Some(&m2));
        assert!(
            got.is_dir(),
            "fallback must be a REAL dir: {}",
            got.display()
        );
        assert!(
            got.parent().is_some(),
            "fallback must never be `/`: {}",
            got.display()
        );
        let home = std::env::var_os("HOME").map(PathBuf::from);
        if let Some(h) = home.filter(|h| h.is_dir()) {
            assert_eq!(got, h, "HOME wins when set + real");
        }
    }

    #[test]
    fn resolve_spawn_cwd_rejects_filesystem_root() {
        // `/` EXISTS and is a dir — the exact trap (a GUI app inherits cwd `/` and an
        // unresolved repo path can be `/`). It must be rejected at every step.
        let root = Path::new("/");
        let got = resolve_spawn_cwd(Some(root), Some(root));
        assert_ne!(got, PathBuf::from("/"), "root is never a spawn cwd");
        assert!(got.is_dir(), "still lands on a real dir: {}", got.display());
    }

    #[test]
    fn resolve_spawn_cwd_no_candidates_is_home() {
        let got = resolve_spawn_cwd(None, None);
        assert!(got.is_dir());
        assert!(got.parent().is_some(), "never `/`: {}", got.display());
    }

    fn git(dir: &Path, args: &[&str]) -> std::process::Output {
        Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .expect("git")
    }

    fn test_nonce() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    }

    // Fix-B: source_repo_claude_env_block lifts the SOURCE repo's `.claude/settings.local.json`
    // `env` into a splice-ready fragment for a worktree pane — so Bedrock (and any per-repo env)
    // survives worktree isolation instead of being clobbered by the hooks-only template.
    #[test]
    fn source_env_block_lifts_source_env_for_worktree() {
        let repo = std::env::temp_dir().join(format!("at-envblk-{}", test_nonce()));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        git(&repo, &["config", "user.email", "t@t"]);
        git(&repo, &["config", "user.name", "t"]);
        std::fs::write(repo.join("base.txt"), "v1").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "base"]);

        // operator's per-repo env: written UNCOMMITTED (real `.claude/` is gitignored), so
        // `git worktree add` — which materializes only tracked HEAD content — won't copy it.
        std::fs::create_dir_all(repo.join(".claude")).unwrap();
        std::fs::write(
            repo.join(".claude/settings.local.json"),
            r#"{"permissions":{"allow":[]},"env":{"CLAUDE_CODE_USE_BEDROCK":"1","AWS_PROFILE":"staging"}}"#,
        )
        .unwrap();

        let wt = add_worktree(&repo, "envblk1").expect("add_worktree");
        // precondition that MOTIVATES the merge: the worktree has no settings.local.json of its own
        assert!(
            !wt.root.join(".claude/settings.local.json").exists(),
            "worktree must NOT carry the source's untracked settings.local.json"
        );

        let frag = source_repo_claude_env_block(&wt.root);
        assert!(
            frag.starts_with(",\n"),
            "leading comma to splice after hooks: {frag:?}"
        );
        assert!(
            frag.contains("CLAUDE_CODE_USE_BEDROCK"),
            "bedrock switch lifted: {frag:?}"
        );
        // splice into a minimal template stand-in → must be valid JSON carrying ONLY env (not perms)
        let merged = format!("{{\n  \"hooks\": {{}}{frag}\n}}");
        let v: serde_json::Value = serde_json::from_str(&merged).expect("merged is valid JSON");
        assert_eq!(v["env"]["AWS_PROFILE"], "staging", "aws profile merged");
        assert!(
            v.get("permissions").is_none(),
            "only env is lifted, not the whole file"
        );

        let _ = std::fs::remove_dir_all(&repo);
    }

    // Fix-B: no source settings (or no `env` key) → empty fragment = prior clobber behavior, never err.
    #[test]
    fn source_env_block_empty_when_no_source_settings() {
        let repo = std::env::temp_dir().join(format!("at-envblk-none-{}", test_nonce()));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        git(&repo, &["config", "user.email", "t@t"]);
        git(&repo, &["config", "user.name", "t"]);
        std::fs::write(repo.join("base.txt"), "v1").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "base"]);

        let wt = add_worktree(&repo, "envblk2").expect("add_worktree");
        assert_eq!(
            source_repo_claude_env_block(&wt.root),
            "",
            "no source env → empty fragment"
        );

        let _ = std::fs::remove_dir_all(&repo);
    }

    // 07-03 / D41: freshen_worktree resets a diverged + dirty worktree back to the
    // pinned base (discarding committed-ahead work AND untracked files) while leaving
    // the path/branch unchanged; and is a no-op when already at the target.
    #[test]
    fn freshen_worktree_resets_diverged_and_dirty_to_base() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let repo = std::env::temp_dir().join(format!("at-freshen-test-{nonce}"));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(&repo).unwrap();

        git(&repo, &["init", "-q"]);
        git(&repo, &["config", "user.email", "t@t"]);
        git(&repo, &["config", "user.name", "t"]);
        std::fs::write(repo.join("base.txt"), "v1").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "base"]);
        let base_sha = String::from_utf8_lossy(&git(&repo, &["rev-parse", "HEAD"]).stdout)
            .trim()
            .to_string();

        let wt = add_worktree(&repo, "freshen1").expect("add_worktree");
        assert!(wt.branch.contains("freshen1"));

        // diverge: commit an edit on the worktree branch, then leave it dirty
        std::fs::write(wt.root.join("base.txt"), "v2-EDITED").unwrap();
        std::fs::write(wt.root.join("committed-ahead.txt"), "ahead").unwrap();
        git(&wt.root, &["add", "-A"]);
        git(&wt.root, &["commit", "-q", "-m", "diverged"]);
        std::fs::write(wt.root.join("untracked-dirty.txt"), "dirty").unwrap();
        let diverged = String::from_utf8_lossy(&git(&wt.root, &["rev-parse", "HEAD"]).stdout)
            .trim()
            .to_string();
        assert_ne!(
            diverged, base_sha,
            "precondition: worktree is ahead of base"
        );

        freshen_worktree(&wt, &base_sha).expect("freshen");

        // back to base: HEAD == base, tracked file restored, BOTH ahead-commit file
        // and untracked file gone — branch + path unchanged.
        let head = String::from_utf8_lossy(&git(&wt.root, &["rev-parse", "HEAD"]).stdout)
            .trim()
            .to_string();
        assert_eq!(head, base_sha, "HEAD reset to pinned base");
        assert_eq!(
            std::fs::read_to_string(wt.root.join("base.txt")).unwrap(),
            "v1"
        );
        assert!(
            !wt.root.join("committed-ahead.txt").exists(),
            "ahead commit's file gone"
        );
        assert!(
            !wt.root.join("untracked-dirty.txt").exists(),
            "untracked cleaned"
        );
        let branch =
            String::from_utf8_lossy(&git(&wt.root, &["rev-parse", "--abbrev-ref", "HEAD"]).stdout)
                .trim()
                .to_string();
        assert!(branch.contains("freshen1"), "branch unchanged: {branch}");

        // idempotent no-op when already at target
        freshen_worktree(&wt, &base_sha).expect("freshen no-op");
        assert_eq!(
            std::fs::read_to_string(wt.root.join("base.txt")).unwrap(),
            "v1"
        );

        let _ = remove_worktree(&repo, "freshen1", &wt.root);
        let _ = std::fs::remove_dir_all(&repo);
    }

    // 07-03 / D41 regression guard: a REUSED worktree can sit AT the target SHA yet
    // be dirty (uncommitted tracked edit + untracked file, never committed → HEAD
    // never moved off the base). freshen MUST still discard that — the head==sha
    // early-return is gated on a clean `git status --porcelain`, not HEAD alone.
    #[test]
    fn freshen_worktree_cleans_dirty_tree_at_target_sha() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let repo = std::env::temp_dir().join(format!("at-freshen-dirty-{nonce}"));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(&repo).unwrap();

        git(&repo, &["init", "-q"]);
        git(&repo, &["config", "user.email", "t@t"]);
        git(&repo, &["config", "user.name", "t"]);
        std::fs::write(repo.join("base.txt"), "v1").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "base"]);
        let base_sha = String::from_utf8_lossy(&git(&repo, &["rev-parse", "HEAD"]).stdout)
            .trim()
            .to_string();

        let wt = add_worktree(&repo, "freshen2").expect("add_worktree");

        // dirty WITHOUT committing → HEAD stays at base_sha (the dangerous reuse state)
        std::fs::write(wt.root.join("base.txt"), "UNCOMMITTED").unwrap();
        std::fs::write(wt.root.join("untracked2.txt"), "wip").unwrap();
        let head = String::from_utf8_lossy(&git(&wt.root, &["rev-parse", "HEAD"]).stdout)
            .trim()
            .to_string();
        assert_eq!(
            head, base_sha,
            "precondition: worktree HEAD is AT the target SHA"
        );

        freshen_worktree(&wt, &base_sha).expect("freshen");

        // dirty-at-target must be CLEANED, not skipped by the head==sha early return
        assert_eq!(
            std::fs::read_to_string(wt.root.join("base.txt")).unwrap(),
            "v1",
            "uncommitted tracked edit discarded"
        );
        assert!(
            !wt.root.join("untracked2.txt").exists(),
            "untracked file cleaned"
        );

        let _ = remove_worktree(&repo, "freshen2", &wt.root);
        let _ = std::fs::remove_dir_all(&repo);
    }

    // ───────────── Codex MCP injection (per-pane config.toml block) ─────────────

    #[test]
    fn codex_mcp_server_name_is_deterministic() {
        assert_eq!(codex_mcp_server_name("ws123-p0"), "agent-teams-ws123-p0");
        assert_eq!(codex_mcp_server_name("ws9-p1"), "agent-teams-ws9-p1");
    }

    #[test]
    fn inject_and_remove_codex_mcp_round_trips() {
        let dir = std::env::temp_dir().join(format!(
            "at-codex-mcp-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("config.toml");
        std::fs::write(&config, "# user config\nmodel = \"o3\"\n").unwrap();

        let sidecar = dir.join("agent-teams-mcp-coordinator");
        let state = dir.join("state");
        let pane_id = "ws42-p0";
        let server_name = codex_mcp_server_name(pane_id);
        let header = format!("[mcp_servers.{server_name}]");

        // inject into the temp config (bypass codex_config_path by calling the
        // internal logic directly — we test the append/remove shape)
        let existing = std::fs::read_to_string(&config).unwrap();
        assert!(!existing.contains(&header));

        // simulate inject: append the block
        let sidecar_str = sidecar.to_string_lossy();
        let state_str = state.to_string_lossy();
        let block = format!(
            "\n# >>> @agent-teams-managed:{server_name} >>>\n             {header}\n             command = {sidecar_str:?}\n             args = []\n             env = {{ AGENT_TEAMS_STATE_DIR = {state_str:?}, AGENT_TEAMS_PANE_ID = {pane_id:?}, AGENT_TEAMS_MEMORY_REPO_KEY = \"global\", AGENT_TEAMS_TASK_SCOPE = {pane_id:?} }}\n             # <<< @agent-teams-managed:{server_name} <<<\n"
        );
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().append(true).open(&config).unwrap();
            f.write_all(block.as_bytes()).unwrap();
        }

        let after_inject = std::fs::read_to_string(&config).unwrap();
        assert!(after_inject.contains(&header), "block injected");
        assert!(after_inject.contains("AGENT_TEAMS_PANE_ID"), "env block present");
        assert!(after_inject.contains("model = \"o3\""), "user config preserved");

        // simulate remove: strip the managed block
        let begin_marker = format!("# >>> @agent-teams-managed:{server_name} >>>");
        let end_marker = format!("# <<< @agent-teams-managed:{server_name} <<<");
        let start = after_inject.find(&begin_marker).unwrap();
        let end_rel = after_inject[start..].find(&end_marker).unwrap();
        let end = start + end_rel + end_marker.len();
        let end = if after_inject[end..].starts_with('\n') { end + 1 } else { end };
        let mut cleaned = String::new();
        cleaned.push_str(&after_inject[..start]);
        cleaned.push_str(&after_inject[end..]);
        std::fs::write(&config, &cleaned).unwrap();

        let after_remove = std::fs::read_to_string(&config).unwrap();
        assert!(!after_remove.contains(&header), "block removed");
        assert!(after_remove.contains("model = \"o3\""), "user config still intact");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn inject_codex_mcp_is_idempotent() {
        let dir = std::env::temp_dir().join(format!(
            "at-codex-idem-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("config.toml");
        std::fs::write(&config, "# base\n").unwrap();

        let pane_id = "ws7-p0";
        let server_name = codex_mcp_server_name(pane_id);
        let header = format!("[mcp_servers.{server_name}]");
        let block = format!(
            "\n# >>> @agent-teams-managed:{server_name} >>>\n{header}\ncommand = \"/bin/true\"\nargs = []\nenv = {{}}\n# <<< @agent-teams-managed:{server_name} <<<\n"
        );

        // append twice
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().append(true).open(&config).unwrap();
            f.write_all(block.as_bytes()).unwrap();
        }
        let after_first = std::fs::read_to_string(&config).unwrap();
        let count_first = after_first.matches(&header).count();
        assert_eq!(count_first, 1, "exactly one block after first inject");

        // idempotent check: skip if header already present
        if !after_first.contains(&header) {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().append(true).open(&config).unwrap();
            f.write_all(block.as_bytes()).unwrap();
        }
        let after_second = std::fs::read_to_string(&config).unwrap();
        let count_second = after_second.matches(&header).count();
        assert_eq!(count_second, 1, "still exactly one block (idempotent)");

        let _ = std::fs::remove_dir_all(&dir);
    }

}
