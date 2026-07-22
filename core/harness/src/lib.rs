//! Agent Teams — PURE worker/harness logic extracted from the supervisor
//! (Plan: feat/core-harness-extract). Dep-light: depends only on `state-adapter`
//! (for `InjectHarness` in the harness descriptor) + std. NO portable-pty / roles /
//! ringbuf here — those stay in the supervisor. The supervisor re-exports every
//! symbol below so its public API and internal references are unchanged, and the
//! agent-teams-cli can share this ONE harness source.

use state_adapter::inject::InjectHarness;
use std::path::PathBuf;

/// §9.2 — STRUCTURAL, harness-agnostic worker push-denial. The env to layer onto EVERY headless
/// worker `Command` so the worker (and any `git` it shells out) can `git commit` locally but CANNOT
/// `git push` — regardless of how coarse the harness's own auto-approve flag is (cursor `--force`,
/// opencode `--dangerously-skip-permissions`, commandcode `--yolo` can all RUN `git push`; with this
/// env it just can't AUTHENTICATE). The CLI-flag allowlists could never give "commit-yes / push-no";
/// this does, at the process boundary. Returned as `(sets, removes)` to apply to the Command.
///
/// PUSH denied — both transports:
///   - `GIT_TERMINAL_PROMPT=0`               no interactive credential prompt (would hang headless)
///   - `GIT_ASKPASS=/usr/bin/false`          askpass returns failure, never a credential
///   - `GIT_CONFIG_NOSYSTEM=1`               ignore /etc/gitconfig
///   - `GIT_CONFIG_GLOBAL=/dev/null`         ignore ~/.gitconfig → NO `credential.helper` (kills the
///                                           macOS osxkeychain token path for HTTPS remotes)
///   - remove `SSH_AUTH_SOCK`                no ssh-agent identities
///   - `GIT_SSH_COMMAND=/usr/bin/false`      git's ssh transport ALWAYS fails — airtight regardless of
///                                           the user's `~/.ssh/config` (a host-alias `IdentityFile`
///                                           survives `-i /dev/null -o IdentitiesOnly=yes`, so the
///                                           naive ssh-flag approach LET A PUSH THROUGH in live-verify;
///                                           replacing the transport with `false` cannot be bypassed)
/// COMMIT preserved — `GIT_CONFIG_GLOBAL=/dev/null` also drops the user's `user.name/email`, which
/// makes `git commit` fail "author unknown"; re-inject a synthetic identity so write-mode still works:
///   - `GIT_AUTHOR_NAME/EMAIL` + `GIT_COMMITTER_NAME/EMAIL` = `flywheel-worker <flywheel@localhost>`
///
/// HOME is deliberately NOT relocated: the harness binaries read their OWN auth from `$HOME`
/// (`~/.claude`, `~/.cursor`, …) — moving HOME would break the harness login. Only git's
/// config/identity/transport are neutralized. The controller's `flywheel_push_and_pr` runs in the
/// APP's env (full creds) and stays the ONE legitimate push path (explicit refspec, never main/force).
/// The throwaway-repo live-verify is the proof obligation: push FAILS, commit SUCCEEDS, every harness.
// The doc table above uses deliberate column alignment for readability.
#[allow(clippy::doc_overindented_list_items, clippy::doc_lazy_continuation)]
pub fn worker_git_deny_env() -> (Vec<(&'static str, String)>, Vec<&'static str>) {
    (
        vec![
            ("GIT_TERMINAL_PROMPT", "0".to_string()),
            ("GIT_ASKPASS", "/usr/bin/false".to_string()),
            ("GIT_CONFIG_NOSYSTEM", "1".to_string()),
            ("GIT_CONFIG_GLOBAL", "/dev/null".to_string()),
            // Replace git's ssh transport with a command that always fails — airtight (a host-alias
            // IdentityFile in ~/.ssh/config survives the ssh-flag denials; live-verify proved a push
            // got through with `-i /dev/null`). The worker is local-only (commits its own branch);
            // it never needs ssh. The controller's push runs in the APP env (no GIT_SSH_COMMAND).
            ("GIT_SSH_COMMAND", "/usr/bin/false".to_string()),
            ("GIT_AUTHOR_NAME", "flywheel-worker".to_string()),
            ("GIT_AUTHOR_EMAIL", "flywheel@localhost".to_string()),
            ("GIT_COMMITTER_NAME", "flywheel-worker".to_string()),
            ("GIT_COMMITTER_EMAIL", "flywheel@localhost".to_string()),
        ],
        vec!["SSH_AUTH_SOCK"],
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Harness {
    Claude,
    Cursor,
    /// test harness — a plain shell; no hook injection
    Bash,
    /// OpenAI Codex CLI. Spawnable (bare `codex` launches its interactive TUI), but
    /// like bash it has NO hook mechanism today → `inject: None` → it does not report
    /// state into the who-needs-you queue (state-blind). Deeper integration (Codex
    /// `notify`/MCP-driven state, role persona) is a deferred follow-up.
    Codex,
    /// Command Code CLI (`commandcode` binary — an account-based "learns your taste"
    /// coding agent). Spawnable interactive TUI, but like codex/bash it has NO hook
    /// mechanism today → `inject: None` → STATE-BLIND (no who-needs-you queue report).
    /// Unlike codex it needs static startup-suppression flags (`--skip-onboarding -t`,
    /// see `spawn_args`) so an automated spawn doesn't block on taste-onboarding / the
    /// trust prompt. Conversation resume (`-r`/`-c`), MCP, role persona = deferred.
    CommandCode,
    /// OpenCode CLI (`opencode` binary — sst/opencode, provider-agnostic). Spawnable
    /// interactive TUI; like codex/commandcode it has NO hook mechanism today →
    /// `inject: None` → STATE-BLIND (no who-needs-you queue report). Headless worker mode
    /// is `opencode run [message]` with `-m provider/model` + `--dangerously-skip-permissions`
    /// (see `worker_spawn`). Resume/MCP/role persona = deferred.
    OpenCode,
    /// Pi CLI (`pi` binary — pi.dev / @earendil-works/pi-coding-agent). Spawnable
    /// interactive TUI via bare `pi`; like codex/commandcode/opencode it has NO hook
    /// mechanism today → `inject: None` → STATE-BLIND (no who-needs-you queue report).
    /// Headless worker mode is `pi -p [--model id] <positional-prompt>` (prompt is the
    /// trailing positional, see `worker_spawn`). ALL headless pi behavior is UNVERIFIED.
    /// Resume/MCP/role persona = deferred. Replaces the former Cline harness.
    Pi,
    /// Grok CLI (`grok` binary — xAI's "Grok Build TUI"). Spawnable interactive
    /// TUI via bare `grok`; like codex/opencode/pi it has NO hook mechanism
    /// today → `inject: None` → STATE-BLIND (no who-needs-you queue report).
    /// Headless worker mode is `grok agent --cwd <cwd> [-m model] <positional-prompt>`
    /// (see `worker_spawn`). ALL headless grok behavior is UNVERIFIED.
    /// Resume/MCP/role persona = deferred.
    Grok,
}

/// The data-driven, per-variant facts about a harness (Phase 2 / preset-wizard
/// facet 1). Everything here is a PURE CONSTANT of the variant — no runtime state.
/// Behavior that depends on runtime state (conversation resume/id, MCP config path,
/// role persona) is NOT here: it stays in `session_args` / `mcp_args` and the role
/// block in `Supervisor::spawn`, which are parameterized by `&WorkspaceSpec`.
///
/// Adding a harness = one new `Harness` variant + one new row in
/// `Harness::descriptor`. The `match` in `descriptor` is CLOSED (no `_ =>`), so Rust
/// exhaustiveness FORCES a complete row for every new variant — the migration safety
/// net. `all()` then surfaces the variant to the catalog/UI for free.
#[derive(Debug, Clone, Copy)]
pub struct HarnessDescriptor {
    /// the binary to spawn (`CommandBuilder::new(this)`).
    pub command: &'static str,
    /// hook-injection harness, or `None` for a non-injecting shell (bash).
    pub inject: Option<InjectHarness>,
    /// stable wire id — `parse_harness`, the admission queue, the synthesis prompt,
    /// and preset `harnesses[]` all key on this (`"claude"`).
    pub wire: &'static str,
    /// human display label for the wizard dropdown / harness grid (`"Claude"`).
    pub display: &'static str,
    /// static CLI args appended to EVERY spawn of this variant — a pure constant
    /// (NOT runtime state, so it lives here, not in `session_args`). Empty for most;
    /// state-blind harnesses may need startup-suppression flags (commandcode:
    /// `--skip-onboarding -t`). `&'static [&'static str]` keeps the `Copy` derive.
    pub spawn_args: &'static [&'static str],
    /// TRUE when the harness has NO turn-end signal at all: after the synthetic
    /// spawn-time `SessionStart` its pane shows **Working forever** (never Done /
    /// Needs-You) because nothing ever reports a turn boundary. Today that is
    /// commandcode + pi + grok. FALSE for the harnesses with a real turn-end channel —
    /// claude/cursor (lifecycle hooks), codex (`notify` override), opencode (the
    /// auto-loaded turn-end plugin) — and for bash (a plain shell that never enters
    /// the agent state machine at all). Pure data bit; surfaces (queue/UI) consume it
    /// separately so a perpetual "Working" can be labeled honestly.
    pub state_blind: bool,
}

impl Harness {
    /// The per-variant table row (Phase 2). CLOSED match — adding a variant without a
    /// row is a COMPILE error, which is the whole point (forces a complete row).
    pub fn descriptor(self) -> HarnessDescriptor {
        match self {
            Harness::Claude => HarnessDescriptor {
                command: "claude",
                inject: Some(InjectHarness::Claude),
                wire: "claude",
                display: "Claude",
                spawn_args: &[],
                // has a turn-end channel (or, for bash, never enters the agent state machine)
                state_blind: false,
            },
            Harness::Cursor => HarnessDescriptor {
                command: "cursor-agent",
                inject: Some(InjectHarness::Cursor),
                wire: "cursor",
                display: "Cursor",
                spawn_args: &[],
                // has a turn-end channel (or, for bash, never enters the agent state machine)
                state_blind: false,
            },
            Harness::Bash => HarnessDescriptor {
                command: "bash",
                inject: None,
                wire: "bash",
                display: "Bash",
                spawn_args: &[],
                // has a turn-end channel (or, for bash, never enters the agent state machine)
                state_blind: false,
            },
            Harness::Codex => HarnessDescriptor {
                command: "codex",
                // No hook injection today (like bash) → state-blind. See the variant doc.
                inject: None,
                wire: "codex",
                display: "Codex",
                spawn_args: &[],
                // has a turn-end channel (or, for bash, never enters the agent state machine)
                state_blind: false,
            },
            Harness::CommandCode => HarnessDescriptor {
                command: "commandcode",
                // State-blind like codex (no hooks). `--skip-onboarding` skips the taste
                // onboarding and `-t`/--trust auto-trusts the project, so an automated
                // spawn never blocks on a startup prompt (these suppress STARTUP only —
                // mid-session tool prompts are still answered live in the TUI, Model-A).
                inject: None,
                wire: "commandcode",
                display: "CommandCode",
                spawn_args: &["--skip-onboarding", "-t"],
                // NO turn-end signal → perpetual Working after the synthetic SessionStart
                state_blind: true,
            },
            Harness::OpenCode => HarnessDescriptor {
                command: "opencode",
                // State-blind like codex/commandcode (no hooks). Bare `opencode` launches
                // the interactive TUI; no startup-suppression flags are needed for the TUI
                // spawn (the headless WORKER path uses `opencode run …`, built in
                // `worker_spawn`, not here).
                inject: None,
                wire: "opencode",
                display: "OpenCode",
                spawn_args: &[],
                // has a turn-end channel (or, for bash, never enters the agent state machine)
                state_blind: false,
            },
            Harness::Pi => HarnessDescriptor {
                command: "pi",
                // State-blind like codex/opencode (no hooks). Bare `pi` opens the interactive
                // TUI; no startup-suppression flags are needed (the headless WORKER path uses
                // `pi -p …`, built in `worker_spawn`, not here).
                inject: None,
                wire: "pi",
                display: "Pi",
                spawn_args: &[],
                // NO turn-end signal → perpetual Working after the synthetic SessionStart
                state_blind: true,
            },
            Harness::Grok => HarnessDescriptor {
                command: "grok",
                // State-blind like codex/opencode/pi (no hooks). Bare `grok` opens the
                // interactive TUI; no startup-suppression flags needed for the TUI spawn
                // (the headless WORKER path uses `grok agent --cwd <cwd> ...`, built in
                // `worker_spawn`, not here).
                inject: None,
                wire: "grok",
                display: "Grok Build",
                spawn_args: &[],
                // NO turn-end signal -> perpetual Working after the synthetic SessionStart
                state_blind: true,
            },
        }
    }

    /// All variants — for enumerating in the harness catalog / wizard UI and for a
    /// table-driven `parse_harness` (no per-variant arm to forget).
    pub fn all() -> [Harness; 8] {
        [
            Harness::Claude,
            Harness::Cursor,
            Harness::Bash,
            Harness::Codex,
            Harness::CommandCode,
            Harness::OpenCode,
            Harness::Pi,
            Harness::Grok,
        ]
    }

    pub fn inject_harness(self) -> Option<InjectHarness> {
        self.descriptor().inject
    }

    /// Map a stable WIRE string back to its variant (the inverse of
    /// `descriptor().wire`). Table-driven off [`Harness::all`], so a new harness is
    /// parseable the moment it has a descriptor row — no per-variant arm to forget
    /// (the same shape as the app's `parse_harness`). Lifted into this shared crate so
    /// the Q4 daemon can validate a wire `SpawnSpec.harness` without reaching into the
    /// app; the app's `parse_harness` is left untouched. `None` for an unknown wire.
    pub fn from_wire(s: &str) -> Option<Harness> {
        Harness::all()
            .into_iter()
            .find(|h| h.descriptor().wire == s)
    }
}

/// Whether this harness's CLI is VERIFIED to honor the POSIX `--` end-of-options
/// marker — i.e. it treats everything AFTER `--` as positional, so a worker prompt
/// that begins with `-`/`--` can be passed safely as a trailing positional instead
/// of being mis-parsed as a flag.
///
/// CONSERVATIVELY `false` for EVERY harness today. No CLI (claude/cursor/codex/
/// opencode/commandcode/pi/bash) has been live-verified to accept `--`, so the
/// safe posture is to REJECT a `-`-leading prompt rather than risk a silent mis-parse
/// that would drop the task. Flip a variant to `true` ONLY after live-verifying that
/// its CLI honors `--` (a live-verify-owed follow-up). The match is CLOSED (no
/// `_ =>`) so a new harness variant must take an explicit position here.
pub fn supports_end_of_options(h: Harness) -> bool {
    match h {
        Harness::Claude
        | Harness::Cursor
        | Harness::Bash
        | Harness::Codex
        | Harness::CommandCode
        | Harness::OpenCode
        | Harness::Pi
        | Harness::Grok => false,
    }
}

/// Build the per-worker PERMISSION CLI flags for an UNATTENDED delegate worker (Phase-16).
/// Pure + total so it is unit-tested without spawning, exactly like [`session_args`]/[`mcp_args`].
///
/// A human pane stays interactive (returns `[]` for `is_worker == false`, and for any
/// non-claude harness — the flags are claude-specific). A claude WORKER has no human to
/// answer an approval prompt, so it MUST never hit one:
/// - `--permission-mode dontAsk` — any tool NOT in the allow set is auto-DENIED (no prompt,
///   so the worker fails fast and records UNVERIFIED instead of hanging to the 900s deadline).
///   `default`/`acceptEdits`/`plan` would PROMPT and stall (verified, claude 2.1.168).
/// - `--allowedTools <set>` — the EXACT tools the fan-in report needs: file Read/Grep/Glob +
///   Write/Edit + read-only git + cargo. `Write` is LOAD-BEARING (no Write ⇒ no report ⇒
///   guaranteed timeout). The set is intentionally TIGHT — anything a goal needs beyond it
///   fails fast (recorded UNVERIFIED), never stalls; broaden later if the live-verify shows gaps.
/// - `--disallowedTools Bash(git push:*)` — deny wins over allow (fail-fast); no network push.
/// - `--add-dir <extra>` — extend the writable scope to the report dir (a SIBLING of the
///   worktree cwd, else the out-of-cwd report write is denied under dontAsk).
///
/// MUST be composed LAST in the spawn argv (the variadic `--allowedTools` runs to the end).
pub fn worker_args(
    harness: Harness,
    is_worker: bool,
    write_mode: bool,
    extra_dirs: &[PathBuf],
) -> Vec<String> {
    if !is_worker || !matches!(harness, Harness::Claude) {
        return vec![];
    }
    let mut a: Vec<String> = vec!["--permission-mode".into(), "dontAsk".into()];
    for d in extra_dirs {
        a.push("--add-dir".into());
        a.push(d.to_string_lossy().into_owned());
    }
    a.push("--disallowedTools".into());
    a.push("Bash(git push:*)".into());
    // --allowedTools LAST: it is variadic and consumes the rest of the argv. Nothing is
    // appended after worker_args in Supervisor::spawn, so this is safe (a terminating flag
    // would be needed only if a positional/flag followed).
    a.push("--allowedTools".into());
    let mut tools: Vec<&str> = vec![
        "Read",
        "Grep",
        "Glob",
        "Write",
        "Edit",
        "Bash(git rev-parse:*)",
        "Bash(git merge-base:*)",
        "Bash(git diff:*)",
        "Bash(git status:*)",
        "Bash(git log:*)",
        "Bash(git show:*)",
        "Bash(cargo test:*)",
        "Bash(cargo build:*)",
        "Bash(cargo check:*)",
    ];
    if write_mode {
        // Flywheel write-mode (Phase 2, gated by flywheel_apply): the worker FIXES code and
        // COMMITS it on its isolated branch so the controller can fold the branches. `git add`/
        // `git commit` are LOCAL ONLY — `git push` stays DENIED above, so a worker can never
        // publish; the integration fold (and, in Phase 3, the PR) is the controller's single,
        // auditable action, never the worker's. Report-only workers (write_mode=false) keep the
        // read-only-git allowlist.
        tools.push("Bash(git add:*)");
        tools.push("Bash(git commit:*)");
    }
    for t in tools {
        a.push(t.into());
    }
    a
}

/// The headless invocation for ONE flywheel/delegate worker on a given harness + optional model.
/// PURE (no spawn, no I/O) so the per-harness argv is unit-testable. The PROMPT is NOT included:
/// claude needs it on STDIN (its variadic `--allowedTools` would swallow a trailing positional —
/// see `worker_args`); for every other harness we ASSUME it takes the prompt as the trailing
/// positional arg (`prompt_via_stdin = false`).
///
/// ⚠️ UNVERIFIED per harness: the probe established each CLI's headless + model + commit FLAGS, not
/// HOW it ingests the task. A CLI that instead reads the prompt from stdin (or mangles a multi-line
/// positional) would get an empty/garbled task → no fix → no commit. That is the expected failure
/// mode of the experimental non-claude paths, and it is caught (not hidden) by the controller's
/// non-committer guard: a worker that produced no commit on `agent-teams/<id>` is surfaced
/// per-worker, never folded as a hollow pass. Only the claude path is live-verifiable today.
///
/// `write_mode` adds the per-harness AUTONOMOUS-COMMIT posture (the flywheel needs the worker to
/// `git commit` on its `agent-teams/<id>` branch so the controller can fold it).
///
/// ⚠️ SECURITY (delegate-live review owed): only **claude** gets a TIGHT allowlist (commit yes,
/// `git push` DENIED — see `worker_args`). The non-claude autonomous modes are COARSER and CLI-
/// flag-only: cursor `--force`+`--trust`, codex `-a never -s workspace-write`, commandcode
/// `--permission-mode auto-accept`, opencode `--dangerously-skip-permissions`. These grant a
/// broader command surface than claude's allowlist (potentially including push), so a non-claude
/// worker is a BIGGER trust grant than the claude path the owed `delegate-live` security review
/// assumed. All of this stays behind the triple gate + `delegate-live` (compiled out by default).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerSpawn {
    /// binary to exec.
    pub program: String,
    /// argv (flags only — NOT the prompt).
    pub args: Vec<String>,
    /// claude=true (prompt fed on stdin); every other harness=false (prompt appended as the
    /// trailing positional arg by the caller).
    pub prompt_via_stdin: bool,
}

/// Whether a worker of `harness` may run in WRITE mode (autonomous commit). The seam where a
/// harness whose isolation/commit posture is unproven gets DOWNGRADED to report-only regardless of
/// the operator's `flywheel_apply` (the controller says-back when it downgrades).
///
/// History: opencode was gated here 2026-06-09 after it ESCAPED the worktree sandbox (it ignored
/// the spawn cwd, followed the worktree's `.git` file to the parent repo, and committed onto the
/// parent `main`). The `--dir <worktree>` fix was then LIVE-VERIFIED the same day: a gated
/// report-only run showed the worker's edit confined to its isolated worktree (`M src/lib.rs` in
/// the worktree per the run's ground-truth diff) with the parent `main` untouched at the seed —
/// so the gate was lifted. All harnesses currently honor the requested mode; keep this seam for
/// the next unproven harness.
pub fn effective_write_mode(_harness: Harness, requested: bool) -> bool {
    requested
}

pub fn worker_spawn(
    harness: Harness,
    model: Option<&str>,
    write_mode: bool,
    extra_dirs: &[PathBuf],
    cwd: &std::path::Path,
) -> WorkerSpawn {
    let m = model.map(str::to_string);
    match harness {
        Harness::Claude => {
            // headless one-shot, structured streaming (stream-json requires --verbose).
            let mut args: Vec<String> = vec![
                "-p".into(),
                "--output-format".into(),
                "stream-json".into(),
                "--verbose".into(),
                "--no-session-persistence".into(),
            ];
            if let Some(m) = &m {
                args.push("--model".into());
                args.push(m.clone());
            }
            // worker_args ends with the variadic --allowedTools, so it MUST be last.
            args.extend(worker_args(Harness::Claude, true, write_mode, extra_dirs));
            WorkerSpawn {
                program: "claude".into(),
                args,
                prompt_via_stdin: true,
            }
        }
        Harness::Cursor => {
            let mut args: Vec<String> =
                vec!["-p".into(), "--output-format".into(), "stream-json".into()];
            // --trust: skip the workspace trust prompt in headless mode. ALWAYS needed for `-p`:
            // without it cursor-agent blocks on the trust modal and there is no human to answer
            // (the pane is headless → hang → the controller's deadline kills the worker). The
            // supervisor's `preseed_cursor_trust` writes the trust marker to disk BEFORE the spawn,
            // but --trust is the belt-and-suspenders: if the preseed fails (disk full, $HOME
            // unset, permission error), the flag alone keeps the headless worker moving.
            args.push("--trust".into());
            if write_mode {
                // --force (run everything) is the FLOOR for headless auto-approve of `git commit` —
                // but deep-verify proved it OVER-GRANTS and there is NO cursor flag/config that
                // gives commit-yes / push-no: `--force` overrides a `.cursor/cli.json`
                // `deny:[Shell(git:push)]`, and even an allow-list lets cursor compound-wrap
                // (`cd .. && git push`) past the matcher → a bare `git push` to the network runs.
                // ⚠️ Unlike the claude path (which DENIES push), a cursor worker CAN push. Push
                // must be prevented ENVIRONMENTALLY (no push-capable remote / no creds in the
                // worker env / OS network sandbox) — owed before any non-throwaway cursor use;
                // the owed delegate-live security review must cover it.
                args.push("--force".into());
            }
            if let Some(m) = &m {
                args.push("--model".into());
                args.push(m.clone());
            }
            WorkerSpawn {
                program: "cursor-agent".into(),
                args,
                prompt_via_stdin: false,
            }
        }
        Harness::Codex => {
            // `codex exec` = non-interactive run-to-completion. `-s workspace-write` scopes edits +
            // commit to the cwd worktree (NOT external push) — VERIFIED by deep-verify (a real
            // `codex exec -s workspace-write` committed on the worktree's `agent-teams/<id>` branch;
            // workspace-write resolves the linked gitdir even when `.git` is a file). NOTE: `codex
            // exec` has NO `-a/--ask-for-approval` flag (that lives on the interactive top-level
            // `codex` only) — passing `-a never` makes EVERY spawn fail `unexpected argument '-a'`
            // (exit 2) before doing anything. So write-mode is just `exec [-m M] -s workspace-write`.
            let mut args: Vec<String> = vec!["exec".into()];
            if let Some(m) = &m {
                args.push("-m".into());
                args.push(m.clone());
            }
            // The fan-in report (and, in write-mode, the committed code) lives in the run dir —
            // OUTSIDE codex's workspace-write root (the cwd worktree). UNLIKE claude (which gets the
            // run dir via `--add-dir`) and the coarse allow-all harnesses (cursor/commandcode/
            // opencode have no path sandbox), codex's sandbox is genuinely PATH-scoped: it recons
            // fine but the `<id>.md` write is REJECTED ("operation not permitted") and the run never
            // settles (verified live 2026-06-08). Grant each run dir with codex's own `--add-dir`
            // (writable alongside the primary workspace). `--add-dir` implies workspace-write, so set
            // `-s workspace-write` whenever we add a dir — even in report-only — else a read-only
            // default config still blocks the report write.
            if write_mode || !extra_dirs.is_empty() {
                args.push("-s".into());
                args.push("workspace-write".into());
            }
            for d in extra_dirs {
                args.push("--add-dir".into());
                args.push(d.to_string_lossy().into_owned());
            }
            WorkerSpawn {
                program: "codex".into(),
                args,
                prompt_via_stdin: false,
            }
        }
        Harness::CommandCode => {
            // `-p`/--print runs non-interactive AND TAKES THE TASK AS ITS QUERY — so it MUST be the
            // LAST arg, since the caller appends the task as the trailing positional. With `-p` first
            // + a stray trailing task, commandcode drops to its interactive "Ready. What's the task?"
            // banner and never runs (verified live 2026-06-08). --skip-onboarding -t = no startup
            // prompts; --max-turns caps a runaway loop.
            //
            // WRITE posture: commandcode REFUSES file/shell edits headlessly under --permission-mode
            // auto-accept — it explicitly demands `--yolo`/--dangerously-skip-permissions (verified:
            // "I am unable to modify files... requires the --yolo flag"). So write-mode (and any run
            // that must write the fan-in report — extra_dirs present) needs --yolo. ⚠️ SECURITY:
            // --yolo is a COARSE bypass (can push) — the §9.2 structural push-denial is owed before
            // any non-throwaway commandcode write-mode use.
            //
            // The fan-in <id>.md lives OUTSIDE the cwd worktree → grant each run dir via --add-dir.
            let mut args: Vec<String> = vec![
                "--skip-onboarding".into(),
                "-t".into(),
                "--max-turns".into(),
                "30".into(),
            ];
            if write_mode || !extra_dirs.is_empty() {
                args.push("--yolo".into());
            }
            for d in extra_dirs {
                args.push("--add-dir".into());
                args.push(d.to_string_lossy().into_owned());
            }
            if let Some(m) = &m {
                args.push("-m".into());
                args.push(m.clone());
            }
            args.push("-p".into()); // LAST: the caller's appended task becomes -p's query
            WorkerSpawn {
                program: "commandcode".into(),
                args,
                prompt_via_stdin: false,
            }
        }
        Harness::OpenCode => {
            // `opencode run` = headless run-to-completion; --format json = machine output;
            // --dangerously-skip-permissions = auto-approve (opencode has no finer CLI flag; its
            // normal permission model is config-based). Model is `provider/model`.
            //
            // ⚠️ ISOLATION (live-verified 2026-06-09): opencode IGNORES the spawn cwd — it resolves
            // the git project root by following the worktree's `.git` FILE back to the main repo and
            // edits/commits THERE, escaping the isolated worktree (it committed straight onto the
            // parent repo's `main`, polluting it + collapsing the PR-gate diff → no PR). Unlike
            // claude/codex/commandcode which honor cwd. FIX: pass `--dir <worktree>` (opencode's own
            // "directory to run in" flag) so its file ops + git stay in the worktree. Until this is
            // live-verified to actually CONFINE writes, the controller GATES opencode to report-only
            // (see `effective_write_mode`) — so write_mode is false here in practice, but --dir is
            // emitted in BOTH modes (report-only also must read the worktree, not the main checkout).
            let mut args: Vec<String> = vec![
                "run".into(),
                "--format".into(),
                "json".into(),
                "--dir".into(),
                cwd.to_string_lossy().into_owned(),
            ];
            // --dangerously-skip-permissions is needed for write_mode AND for any run that must write
            // the fan-in report (extra_dirs present) — WITHOUT it headless opencode auto-REJECTS its
            // own tool calls ("The user rejected permission to use this specific tool call.") and the
            // report is never written → the run times out (live-verified 2026-06-09). Same shape as
            // commandcode's --yolo / codex's workspace-write: report-only STILL must write the report.
            // The COMMIT step stays gated off in report-only (the task carries no commit instruction)
            // and --dir keeps git ops in the worktree.
            if write_mode || !extra_dirs.is_empty() {
                args.push("--dangerously-skip-permissions".into());
            }
            if let Some(m) = &m {
                args.push("-m".into());
                args.push(m.clone());
            }
            WorkerSpawn {
                program: "opencode".into(),
                args,
                prompt_via_stdin: false,
            }
        }
        Harness::Pi => {
            // `pi -p [prompt]` = non-interactive print mode (process prompt and exit). Prompt is the
            // trailing POSITIONAL (prompt_via_stdin=false). `--model` sets provider/id or bare id.
            // Pi honors process cwd (no OpenCode-style --dir needed). No documented path sandbox /
            // `--add-dir`, so extra_dirs need no extra flag. ⚠️ SECURITY: tool use is COARSE (can
            // `git push`); the §9.2 worker_git_deny_env is what actually denies push (same trust
            // class as cursor/commandcode/opencode). State-blind (inject:None) → no who-needs-you
            // report. UNVERIFIED headless.
            let mut args: Vec<String> = vec!["-p".into()];
            if let Some(m) = &m {
                args.push("--model".into());
                args.push(m.clone());
            }
            WorkerSpawn {
                program: "pi".into(),
                args,
                prompt_via_stdin: false,
            }
        }
        // UNVERIFIED: headless shape (`grok agent --cwd <cwd>` + trailing positional prompt).
        // `grok agent` is the headless subcommand; `grok --cwd` redirects the interactive TUI's cwd.
        // For headless worker mode, use `grok agent --cwd <cwd> [-m model] <positional-prompt>`.
        // grok has a path sandbox (`--sandbox`), so extra_dirs are NOT wired through here.
        Harness::Grok => {
            let mut args: Vec<String> =
                vec!["agent".into(), "--cwd".into(), cwd.to_string_lossy().into_owned()];
            if let Some(m) = &m {
                args.push("-m".into());
                args.push(m.clone());
            }
            WorkerSpawn {
                program: "grok".into(),
                args,
                prompt_via_stdin: false,
            }
        }
        Harness::Bash => {
            // bash is NOT an agentic worker (no model, no edit loop). The flywheel UI must not
            // offer it; if one slips through, this no-op exits cleanly (the controller's
            // non-committer guard then surfaces it as "produced no commit").
            WorkerSpawn {
                program: "true".into(),
                args: vec![],
                prompt_via_stdin: false,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn all_lists_each_variant_exactly_once() {
        // `all()` is a hand-written array; `descriptor()`/`worker_spawn()` are exhaustive
        // matches (a new variant is a compile error THERE), but nothing forces a new variant
        // into `all()` — and the table-driven parse/catalog/round-trip tests iterate `all()`,
        // so a forgotten entry silently under-tests it. The `tag` match below is exhaustive:
        // a new variant is a compile error here, and the count assert proves `all()` carries
        // each variant exactly once (nothing dropped or duplicated).
        fn tag(h: Harness) -> usize {
            match h {
                Harness::Claude => 0,
                Harness::Cursor => 1,
                Harness::Bash => 2,
                Harness::Codex => 3,
                Harness::CommandCode => 4,
                Harness::OpenCode => 5,
                Harness::Pi => 6,
                Harness::Grok => 7,
            }
        }
        let all = Harness::all();
        assert_eq!(
            all.len(),
            8,
            "all() size must equal the Harness variant count"
        );
        let mut seen = [0u32; 8];
        for h in all {
            seen[tag(h)] += 1;
        }
        assert!(
            seen.iter().all(|&c| c == 1),
            "all() must contain each Harness variant exactly once: {seen:?}"
        );
    }

    #[test]
    fn from_wire_round_trips_every_variant_and_rejects_unknown() {
        // Inverse of descriptor().wire for every variant — the Q4 daemon's harness
        // validator. A new variant is covered automatically (driven off all()).
        for h in Harness::all() {
            assert_eq!(
                Harness::from_wire(h.descriptor().wire),
                Some(h),
                "{h:?} must round-trip through its wire string"
            );
        }
        assert_eq!(Harness::from_wire("nope"), None, "unknown wire → None");
        assert_eq!(Harness::from_wire(""), None, "empty wire → None");
        assert_eq!(
            Harness::from_wire("Claude"),
            None,
            "wire is case-sensitive (lowercase)"
        );
    }

    #[test]
    fn state_blind_bit_marks_exactly_the_no_turn_end_harnesses() {
        // state_blind = NO turn-end signal at all → the pane shows Working forever after
        // the synthetic SessionStart. Today: commandcode + pi + grok. claude/cursor have
        // lifecycle hooks, codex has the notify override, opencode has the auto-loaded
        // plugin, and bash never enters the agent state machine. Driven off all() so a
        // new variant must take an explicit position (the descriptor match is closed).
        for h in Harness::all() {
            let d = h.descriptor();
            let want = matches!(h, Harness::CommandCode | Harness::Pi | Harness::Grok);
            assert_eq!(
                d.state_blind, want,
                "{h:?} state_blind must be {want} (wire {:?})",
                d.wire
            );
        }
    }

    #[test]
    fn end_of_options_unsupported_for_every_harness() {
        // No CLI is live-verified to honor `--` yet → the conservative default is FALSE
        // for every variant, so a `-`-leading worker prompt is REJECTED (never silently
        // mis-parsed). Driven off all() so a new variant must take an explicit position.
        for h in Harness::all() {
            assert!(
                !supports_end_of_options(h),
                "{h:?} must default to NO `--` support (live-verify-owed)"
            );
        }
    }

    #[test]
    fn worker_args_only_for_claude_workers_and_carries_the_no_stall_contract() {
        // Human panes (is_worker=false) stay interactive — NO permission flags, any harness.
        assert!(worker_args(Harness::Claude, false, false, &[]).is_empty());
        assert!(worker_args(Harness::Cursor, false, false, &[]).is_empty());
        // A non-claude worker gets nothing (the flags are claude-specific; MVP spawns claude).
        assert!(worker_args(Harness::Bash, true, false, &[]).is_empty());
        assert!(worker_args(Harness::Codex, true, false, &[]).is_empty());

        let dir = PathBuf::from("/repo/bridge/delegate-1");
        let a = worker_args(Harness::Claude, true, false, std::slice::from_ref(&dir));
        let joined = a.join(" ");
        // dontAsk is load-bearing: it AUTO-DENIES (no prompt) non-allowlisted tools, so an
        // unattended worker fails fast instead of hanging — default/acceptEdits would prompt.
        assert!(
            a.windows(2)
                .any(|w| w[0] == "--permission-mode" && w[1] == "dontAsk"),
            "worker must run --permission-mode dontAsk, got: {joined}"
        );
        // Write is the catastrophic-if-missing tool (no Write ⇒ no report ⇒ guaranteed timeout).
        assert!(
            a.iter().any(|s| s == "Write"),
            "allow set MUST include Write"
        );
        assert!(a.iter().any(|s| s == "Read") && a.iter().any(|s| s == "Edit"));
        // read-only git for the report's BASE/CHANGED sections; cargo for VERIFIED.
        assert!(
            a.iter().any(|s| s == "Bash(git diff:*)")
                && a.iter().any(|s| s == "Bash(cargo test:*)")
        );
        // git push is explicitly denied (deny > allow, fail-fast).
        assert!(a
            .windows(2)
            .any(|w| w[0] == "--disallowedTools" && w[1] == "Bash(git push:*)"));
        // the report dir (sibling of the worktree) is granted as a writable scope.
        assert!(
            a.windows(2)
                .any(|w| w[0] == "--add-dir" && w[1] == "/repo/bridge/delegate-1"),
            "worker must --add-dir the report dir, got: {joined}"
        );
        // --allowedTools is LAST (variadic to end-of-argv) — nothing may follow it.
        let allow_pos = a.iter().position(|s| s == "--allowedTools").unwrap();
        assert!(
            a[allow_pos + 1..].iter().all(|s| !s.starts_with("--")),
            "no flag may follow the variadic --allowedTools"
        );
        // report-only (write_mode=false) must NOT grant add/commit — a worker that can't change
        // the repo is the report-only default; write is the deliberate Phase-2 escalation.
        assert!(
            !a.iter()
                .any(|s| s == "Bash(git add:*)" || s == "Bash(git commit:*)"),
            "report-only worker must NOT be able to git add/commit"
        );
    }

    // Flywheel write-mode (Phase 2): the ONLY delta vs report-only is +git add +git commit, so the
    // worker can commit its fix on its branch for the controller to fold. git push STAYS denied
    // (local-only escalation); --allowedTools stays variadic-last. Pure → unit-pinned.
    #[test]
    fn worker_args_write_mode_adds_local_commit_but_never_push() {
        let dir = PathBuf::from("/repo/bridge/flywheel-1");
        let w = worker_args(Harness::Claude, true, true, std::slice::from_ref(&dir));
        // the write-mode escalation: add + commit are now allowed.
        assert!(
            w.iter().any(|s| s == "Bash(git add:*)"),
            "write-mode must allow git add"
        );
        assert!(
            w.iter().any(|s| s == "Bash(git commit:*)"),
            "write-mode must allow git commit"
        );
        // push is STILL denied — a worker can never publish; the PR is the controller's action.
        assert!(
            w.windows(2)
                .any(|p| p[0] == "--disallowedTools" && p[1] == "Bash(git push:*)"),
            "write-mode must STILL deny git push"
        );
        let allow_pos = w.iter().position(|s| s == "--allowedTools").unwrap();
        assert!(
            !w[allow_pos..].iter().any(|s| s.contains("git push")),
            "git push must never be in the allow set"
        );
        // --allowedTools stays LAST (no flag after the added tools).
        assert!(
            w[allow_pos + 1..].iter().all(|s| !s.starts_with("--")),
            "no flag may follow --allowedTools even in write-mode"
        );
        // a non-worker is still empty regardless of write_mode.
        assert!(worker_args(Harness::Claude, false, true, &[]).is_empty());
    }

    #[test]
    fn worker_spawn_claude_no_model_is_byte_identical_to_the_pre_reroute_argv() {
        // REGRESSION GUARD: claude was rerouted from a hardcoded `Command::new("claude")` + inline
        // args to worker_spawn. With NO model picked, the spawned argv MUST be byte-identical to the
        // path that shipped + worked before — else claude flywheel silently breaks. Reconstruct the
        // old hardcoded argv and assert equality for both report-only and write modes.
        let dir = PathBuf::from("/repo/bridge/fw-1");
        for write_mode in [false, true] {
            let mut expected: Vec<String> = vec![
                "-p".into(),
                "--output-format".into(),
                "stream-json".into(),
                "--verbose".into(),
                "--no-session-persistence".into(),
            ];
            expected.extend(worker_args(
                Harness::Claude,
                true,
                write_mode,
                std::slice::from_ref(&dir),
            ));
            let s = worker_spawn(
                Harness::Claude,
                None,
                write_mode,
                std::slice::from_ref(&dir),
                std::path::Path::new("/wt"),
            );
            assert_eq!(s.program, "claude");
            assert!(s.prompt_via_stdin, "claude still feeds the prompt on stdin");
            assert_eq!(
                s.args, expected,
                "claude no-model argv must match the pre-reroute spawn (write_mode={write_mode})"
            );
        }
    }

    #[test]
    fn worker_spawn_claude_threads_model_and_keeps_stdin_prompt_and_allowlist_last() {
        let dir = PathBuf::from("/repo/bridge/fw-1");
        let s = worker_spawn(
            Harness::Claude,
            Some("claude-haiku-4-5"),
            true,
            std::slice::from_ref(&dir),
            std::path::Path::new("/wt"),
        );
        assert_eq!(s.program, "claude");
        assert!(s.prompt_via_stdin, "claude feeds the prompt on stdin");
        // --model threaded.
        assert!(
            s.args
                .windows(2)
                .any(|p| p[0] == "--model" && p[1] == "claude-haiku-4-5"),
            "claude must carry the chosen --model"
        );
        // write-mode commit posture present, push still denied, --allowedTools still last.
        assert!(s.args.iter().any(|a| a == "Bash(git commit:*)"));
        assert!(s
            .args
            .windows(2)
            .any(|p| p[0] == "--disallowedTools" && p[1] == "Bash(git push:*)"));
        let allow = s.args.iter().position(|a| a == "--allowedTools").unwrap();
        assert!(
            s.args[allow + 1..].iter().all(|a| !a.starts_with("--")),
            "nothing (incl --model) may follow the variadic --allowedTools"
        );
    }

    #[test]
    fn worker_spawn_non_claude_uses_positional_prompt_model_and_commit_posture() {
        // Each non-claude harness: right binary, prompt NOT via stdin, --model threaded,
        // and write_mode adds the harness's autonomous-commit mode.
        let cases: &[(Harness, &str, &str, &[&str])] = &[
            (
                Harness::Cursor,
                "cursor-agent",
                "composer-2.5-fast",
                &["--force", "--trust"],
            ),
            // codex exec has NO -a flag (deep-verify: passing it = exit 2). write-mode = exec -s workspace-write.
            (
                Harness::Codex,
                "codex",
                "gpt-5.5",
                &["exec", "workspace-write"],
            ),
            (
                Harness::CommandCode,
                "commandcode",
                "gpt-5",
                &["--yolo", "--skip-onboarding"],
            ),
            // opencode also carries --dir (it ignores cwd → escapes the worktree without it).
            (
                Harness::OpenCode,
                "opencode",
                "github-copilot/claude-haiku-4.5",
                &["run", "--dangerously-skip-permissions", "--dir"],
            ),
            (
                Harness::Pi,
                "pi",
                "anthropic/claude-sonnet-4",
                &["-p"],
            ),
            (
                Harness::Grok,
                "grok",
                "xai/grok-3.5",
                &["agent", "--cwd"],
            ),
        ];
        for (h, prog, model, must_have) in cases {
            let s = worker_spawn(*h, Some(model), true, &[], std::path::Path::new("/wt"));
            assert_eq!(&s.program, prog, "{prog}: program");
            assert!(
                !s.prompt_via_stdin,
                "{prog}: non-claude takes the prompt as a positional arg"
            );
            assert!(
                s.args.iter().any(|a| a == model) || s.args.windows(2).any(|p| p[1] == *model),
                "{prog}: must carry the chosen model"
            );
            for tok in *must_have {
                assert!(
                    s.args.iter().any(|a| a == tok),
                    "{prog}: missing autonomous-commit token {tok}"
                );
            }
        }
        // opencode --dir must carry the worktree cwd (it ignores the spawn cwd and would otherwise
        // resolve the git root to the parent repo + escape the worktree — live-verified 2026-06-09).
        let oc = worker_spawn(
            Harness::OpenCode,
            Some("x/y"),
            true,
            &[],
            std::path::Path::new("/wt"),
        );
        assert!(
            oc.args.windows(2).any(|p| p[0] == "--dir" && p[1] == "/wt"),
            "opencode must pass --dir <worktree> so it stays inside the sandbox"
        );
        // codex must NEVER carry `-a` (codex exec has no such flag → exit 2), in either mode.
        for wm in [true, false] {
            let c = worker_spawn(
                Harness::Codex,
                Some("gpt-5.5"),
                wm,
                &[],
                std::path::Path::new("/wt"),
            );
            assert!(
                !c.args.iter().any(|a| a == "-a"),
                "codex exec has no -a flag (write_mode={wm})"
            );
        }
        // report-only (write_mode=false) with NO run dir drops the autonomous-commit escalation
        // (codex -s workspace-write).
        let ro = worker_spawn(
            Harness::Codex,
            Some("gpt-5.5"),
            false,
            &[],
            std::path::Path::new("/wt"),
        );
        assert!(
            !ro.args.iter().any(|a| a == "workspace-write"),
            "report-only codex must not pass -s workspace-write"
        );
        // BUT codex MUST grant each run dir as a writable root (--add-dir) AND enable workspace-write
        // when a run dir is present — its sandbox is path-scoped (unlike the allow-all harnesses), so
        // without this it cannot write the fan-in <id>.md outside the worktree and the run never
        // settles (live-verified 2026-06-08). True in BOTH report-only and write mode.
        let rd = std::path::PathBuf::from("/tmp/run-xyz");
        for wm in [true, false] {
            let c = worker_spawn(
                Harness::Codex,
                Some("gpt-5.5"),
                wm,
                std::slice::from_ref(&rd),
                std::path::Path::new("/wt"),
            );
            assert!(
                c.args.windows(2).any(|p| p[0] == "--add-dir" && p[1] == "/tmp/run-xyz"),
                "codex must --add-dir the run dir so it can write the fan-in report (write_mode={wm})"
            );
            assert!(
                c.args.iter().any(|a| a == "workspace-write"),
                "codex with a run dir must enable workspace-write so --add-dir is writable (write_mode={wm})"
            );
        }
        // commandcode: `-p` MUST be the FINAL arg (the caller's appended task becomes its query;
        // else it drops to the interactive "Ready. What's the task?" banner — live-verified). And
        // write-mode needs --yolo (auto-accept is REFUSED headlessly — live-verified).
        let cc = worker_spawn(
            Harness::CommandCode,
            Some("gpt-5.5"),
            true,
            &[],
            std::path::Path::new("/wt"),
        );
        assert_eq!(
            cc.args.last().map(String::as_str),
            Some("-p"),
            "commandcode -p must be the LAST arg"
        );
        assert!(
            cc.args.iter().any(|a| a == "--yolo"),
            "commandcode write-mode needs --yolo"
        );
        assert!(
            !cc.args.iter().any(|a| a == "auto-accept"),
            "auto-accept is refused headlessly — must NOT be used"
        );
        // commandcode --add-dir's each run dir so it can write the fan-in report outside cwd; and a
        // run-dir-present run gets --yolo even in report-only (it must write the report).
        let ccd = worker_spawn(
            Harness::CommandCode,
            Some("gpt-5.5"),
            false,
            std::slice::from_ref(&rd),
            std::path::Path::new("/wt"),
        );
        assert!(
            ccd.args
                .windows(2)
                .any(|p| p[0] == "--add-dir" && p[1] == "/tmp/run-xyz"),
            "commandcode must --add-dir the run dir"
        );
        assert!(
            ccd.args.iter().any(|a| a == "--yolo"),
            "commandcode with a run dir needs --yolo to write the report"
        );
        assert_eq!(
            ccd.args.last().map(String::as_str),
            Some("-p"),
            "commandcode -p stays LAST with extra_dirs"
        );
    }

    // P3: report-only symmetry for the coarse auto-approve harnesses. WITHOUT write_mode (and no
    // run dir), cursor/opencode must NOT carry the §9.2 push-capable auto-approve flags — a
    // report-only worker reads + writes its fan-in report and never auto-commits. Mirrors the
    // codex report-only assertion above. (cursor/opencode have no path sandbox, so unlike codex a
    // run dir alone doesn't force the escalation — only write_mode does.)
    #[test]
    fn worker_spawn_report_only_drops_autocommit_for_cursor_and_opencode() {
        let wt = std::path::Path::new("/wt");
        let cur = worker_spawn(Harness::Cursor, Some("auto"), false, &[], wt);
        assert!(
            !cur.args.iter().any(|a| a == "--force"),
            "report-only cursor must not --force (tool permission stays interactive)"
        );
        // --trust IS present in report-only: the headless `-p` mode needs it regardless of
        // write_mode to skip the workspace trust modal (no human to answer → hang).
        assert!(
            cur.args.iter().any(|a| a == "--trust"),
            "report-only cursor MUST --trust (headless mode needs trust to avoid hanging)"
        );
        let oc = worker_spawn(Harness::OpenCode, Some("github-copilot/x"), false, &[], wt);
        assert!(
            !oc.args
                .iter()
                .any(|a| a == "--dangerously-skip-permissions"),
            "report-only opencode with NO run dir doesn't skip permissions"
        );
        // report-only opencode STILL carries --dir (it must read the worktree, not the main checkout).
        assert!(
            oc.args.windows(2).any(|p| p[0] == "--dir" && p[1] == "/wt"),
            "report-only opencode keeps --dir"
        );
        // BUT report-only WITH a run dir (the real controller path) MUST skip permissions, else headless
        // opencode auto-rejects its own tool calls and never writes the fan-in report → timeout
        // (live-verified). Mirrors commandcode's --yolo-even-in-report-only.
        let rd = std::path::PathBuf::from("/tmp/run-oc");
        let ocr = worker_spawn(
            Harness::OpenCode,
            Some("x/y"),
            false,
            std::slice::from_ref(&rd),
            wt,
        );
        assert!(
            ocr.args
                .iter()
                .any(|a| a == "--dangerously-skip-permissions"),
            "report-only opencode WITH a run dir must skip permissions to write the report"
        );
        // write_mode flips the TOOL auto-approve posture ON (--force, the push-capable §9.2 floor).
        // --trust is already present in BOTH modes (headless needs it regardless).
        let curw = worker_spawn(Harness::Cursor, Some("auto"), true, &[], wt);
        assert!(
            curw.args.iter().any(|a| a == "--force") && curw.args.iter().any(|a| a == "--trust"),
            "write-mode cursor carries --force --trust"
        );
        let ocw = worker_spawn(Harness::OpenCode, Some("github-copilot/x"), true, &[], wt);
        assert!(
            ocw.args
                .iter()
                .any(|a| a == "--dangerously-skip-permissions"),
            "write-mode opencode carries --dangerously-skip-permissions"
        );
    }

    // Cursor headless (`-p`) ALWAYS needs --trust regardless of write_mode: without it the
    // cursor-agent blocks on the workspace trust modal and there's no human to answer (headless
    // → hang → the controller's deadline kills the worker). The supervisor's preseed_cursor_trust
    // writes the marker to disk, but --trust is the belt-and-suspenders for preseed failures.
    // --force (the push-capable tool auto-approve) stays write-mode-gated.
    #[test]
    fn worker_spawn_cursor_headless_always_carries_trust_only_force_is_write_mode_gated() {
        let wt = std::path::Path::new("/wt");
        // report-only: --trust present, --force absent.
        let ro = worker_spawn(Harness::Cursor, Some("composer-2.5-fast"), false, &[], wt);
        assert!(
            ro.args.iter().any(|a| a == "--trust"),
            "report-only cursor headless MUST carry --trust (avoids trust-modal hang)"
        );
        assert!(
            !ro.args.iter().any(|a| a == "--force"),
            "report-only cursor must NOT carry --force (tool permission stays interactive)"
        );
        // write-mode: both --trust and --force present.
        let wm = worker_spawn(Harness::Cursor, Some("composer-2.5-fast"), true, &[], wt);
        assert!(
            wm.args.iter().any(|a| a == "--trust"),
            "write-mode cursor headless MUST carry --trust"
        );
        assert!(
            wm.args.iter().any(|a| a == "--force"),
            "write-mode cursor headless MUST carry --force (auto-approve commit)"
        );
        // the headless floor is always present regardless of write_mode.
        for s in [&ro, &wm] {
            assert!(s.args.iter().any(|a| a == "-p"), "cursor headless always -p");
            assert!(
                s.args.windows(2).any(|p| p[0] == "--output-format" && p[1] == "stream-json"),
                "cursor headless always --output-format stream-json"
            );
        }
    }

    // §9.2: the worker push-denial env denies BOTH transports (no credential.helper for HTTPS, no
    // ssh identity/agent for SSH, no prompts) while preserving commit (synthetic identity injected).
    #[test]
    fn worker_git_deny_env_blocks_push_keeps_commit() {
        let (sets, removes) = worker_git_deny_env();
        let get = |k: &str| sets.iter().find(|(n, _)| *n == k).map(|(_, v)| v.as_str());
        // push-denial: no prompt, no askpass, no global/system config (→ no credential.helper).
        assert_eq!(get("GIT_TERMINAL_PROMPT"), Some("0"));
        assert_eq!(get("GIT_ASKPASS"), Some("/usr/bin/false"));
        assert_eq!(get("GIT_CONFIG_NOSYSTEM"), Some("1"));
        assert_eq!(get("GIT_CONFIG_GLOBAL"), Some("/dev/null"));
        // ssh transport replaced with an always-failing command — airtight (the ssh-flag approach
        // let a push through in live-verify because a ~/.ssh/config host-alias IdentityFile survived).
        assert_eq!(
            get("GIT_SSH_COMMAND"),
            Some("/usr/bin/false"),
            "git ssh transport must always fail"
        );
        // ssh-agent socket removed entirely.
        assert!(
            removes.contains(&"SSH_AUTH_SOCK"),
            "ssh-agent identities removed"
        );
        // commit preserved: synthetic identity present (GIT_CONFIG_GLOBAL=/dev/null dropped user.name).
        assert_eq!(get("GIT_AUTHOR_NAME"), Some("flywheel-worker"));
        assert_eq!(get("GIT_AUTHOR_EMAIL"), Some("flywheel@localhost"));
        assert_eq!(get("GIT_COMMITTER_NAME"), Some("flywheel-worker"));
        assert_eq!(get("GIT_COMMITTER_EMAIL"), Some("flywheel@localhost"));
    }

    // The write-mode downgrade seam: with no harness currently unproven, EVERY harness honors the
    // requested mode (opencode's 2026-06-09 gate was lifted after `--dir` confinement was
    // live-verified — worker edit landed in its isolated worktree, parent main untouched).
    #[test]
    fn effective_write_mode_honors_request_for_all_harnesses() {
        for h in Harness::all() {
            assert!(
                effective_write_mode(h, true),
                "{h:?} honors requested write-mode"
            );
            assert!(!effective_write_mode(h, false), "{h:?} honors report-only");
        }
    }
}
