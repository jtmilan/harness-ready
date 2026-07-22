//! Worker invocation — the per-harness headless `Command` for a flywheel/delegate worker.
//!
//! P4 multi-harness: claude + cursor/codex/opencode. Ported faithfully from agent-teams
//! `core/supervisor/src/lib.rs` — `worker_spawn` (~L478), `worker_args` (~L382), and
//! `worker_git_deny_env` (~L82). The cursor/codex/opencode arms mirror the supervisor argv
//! verbatim (cursor L504-525, codex L526-557, opencode L589-625); commandcode + the `true`
//! bash fallback are deliberately out of scope.
//!
//! Security posture (the §9.2 push-denial contract, preserved verbatim): a claude worker may
//! `git commit` on its isolated `agent-teams/<id>` branch but can NEVER `git push` —
//! `--disallowedTools Bash(git push:*)` (the allowlist) AND `GIT_SSH_COMMAND=/usr/bin/false` +
//! no credentials (the env). The integration fold + PR are the controller's single auditable
//! action (`flywheel::flywheel_push_and_pr`), never the worker's.

use std::path::Path;
use std::process::Command;

/// The push-denial env set for a worker process (the SET list from supervisor's
/// `worker_git_deny_env`). A worker can commit locally (synthetic author identity supplied,
/// since `GIT_CONFIG_GLOBAL=/dev/null` drops the user's `user.name/email`) but cannot reach a
/// network remote (askpass + ssh transport both forced to `/usr/bin/false`).
///
/// NOTE: supervisor also returns a REMOVE list (`["SSH_AUTH_SOCK"]`) — applied separately by
/// [`apply_git_deny_env`] via `Command::env_remove`.
pub fn worker_git_deny_env() -> Vec<(&'static str, String)> {
    vec![
        ("GIT_TERMINAL_PROMPT", "0".to_string()),
        ("GIT_ASKPASS", "/usr/bin/false".to_string()),
        ("GIT_CONFIG_NOSYSTEM", "1".to_string()),
        ("GIT_CONFIG_GLOBAL", "/dev/null".to_string()),
        // git's ssh transport ALWAYS fails — airtight regardless of ~/.ssh/config IdentityFile.
        ("GIT_SSH_COMMAND", "/usr/bin/false".to_string()),
        ("GIT_AUTHOR_NAME", "flywheel-worker".to_string()),
        ("GIT_AUTHOR_EMAIL", "flywheel@localhost".to_string()),
        ("GIT_COMMITTER_NAME", "flywheel-worker".to_string()),
        ("GIT_COMMITTER_EMAIL", "flywheel@localhost".to_string()),
    ]
}

/// Apply the worker git-deny env (set + remove) onto a `Command`.
///
/// REMOVE list: `SSH_AUTH_SOCK` (no ssh-agent identity) plus `GITHUB_TOKEN`/`GH_TOKEN`. The latter
/// two are the load-bearing scrub for the coarse-approve non-claude harnesses (cursor/codex/
/// opencode): they have NO tool allowlist and can run arbitrary shell, so an inherited token would
/// be a push/exfil channel (`gh`, `curl`, or `git push https://x-access-token:$TOKEN@…`) that routes
/// AROUND `GIT_SSH_COMMAND`/`GIT_ASKPASS`. The controller already scrubs `GITHUB_TOKEN` on its own
/// git/gh calls (`flywheel.rs`, `ade/main.rs`) — this makes the worker env boundary equally total.
pub fn apply_git_deny_env(cmd: &mut Command) {
    for (k, v) in worker_git_deny_env() {
        cmd.env(k, v);
    }
    cmd.env_remove("SSH_AUTH_SOCK");
    cmd.env_remove("GITHUB_TOKEN");
    cmd.env_remove("GH_TOKEN");
}

/// claude's worker `--allowedTools`/`--disallowedTools` allowlist (mirror of supervisor's
/// `worker_args(Harness::Claude, true, write_mode, &[])`). `git push` is DENIED; `git add`/`git
/// commit` are added ONLY in write mode. The variadic `--allowedTools` MUST be last (it consumes
/// the rest of argv) — the caller appends nothing after this.
fn claude_worker_args(write_mode: bool) -> Vec<String> {
    let mut a: Vec<String> = vec!["--permission-mode".into(), "dontAsk".into()];
    a.push("--disallowedTools".into());
    a.push("Bash(git push:*)".into());
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
        // write-mode: worker FIXES code and COMMITS on its branch (LOCAL ONLY — push stays denied).
        tools.push("Bash(git add:*)");
        tools.push("Bash(git commit:*)");
    }
    for t in tools {
        a.push(t.into());
    }
    a
}

/// Build the headless claude worker `Command` (flags only — the PROMPT is delivered on STDIN by
/// `RunContext::spawn_worker`, since claude's variadic `--allowedTools` would swallow a trailing
/// positional). Mirror of `worker_spawn(Harness::Claude, model, write_mode, &[], cwd)`. The model
/// is resolved through the repo's Bedrock alias map (`crate::model::resolve_headless_model`) so a
/// Bedrock repo doesn't 400 on a 1P alias. `cwd`/PATH/process-group/stdin are set by `spawn_worker`.
pub fn claude_worker_command(repo: &Path, model: Option<&str>, write_mode: bool) -> Command {
    let mut cmd = Command::new("claude");
    cmd.args([
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        "--no-session-persistence",
    ]);
    if let Some(m) = model {
        let resolved = crate::model::resolve_headless_model(repo, m);
        cmd.args(["--model", &resolved]);
    }
    // worker_args ends with the variadic --allowedTools → MUST be appended last.
    for a in claude_worker_args(write_mode) {
        cmd.arg(a);
    }
    apply_git_deny_env(&mut cmd);
    cmd
}

/// A built worker invocation + how its PROMPT is delivered. claude needs the prompt on STDIN
/// (its variadic `--allowedTools` would swallow a trailing positional — see `claude_worker_args`);
/// every other harness takes the prompt as the trailing positional arg, appended by
/// `RunContext::spawn_worker`. Mirror of supervisor's `WorkerSpawn.prompt_via_stdin`.
pub struct WorkerSpec {
    pub cmd: std::process::Command,
    /// claude=true (prompt fed on stdin); cursor/codex/opencode=false (prompt appended positional).
    pub prompt_via_stdin: bool,
    /// TRUE only when the harness CLI is LIVE-VERIFIED to honor the POSIX `--` end-of-options
    /// marker (`harness::supports_end_of_options`). When FALSE (the default for EVERY harness
    /// today), `RunContext::spawn_worker` REJECTS a prompt that begins with `-`/`--` for a
    /// positional-delivery harness (`!prompt_via_stdin`) rather than risk a silent flag mis-parse
    /// that would drop the task. When TRUE it inserts `--` before the trailing positional prompt.
    /// claude (`prompt_via_stdin`) is unaffected either way.
    pub end_of_options_supported: bool,
}

/// Map a CLI harness string to the shared `harness::Harness`. `None` for claude/bash/unknown — those
/// take the CLI's own `claude_worker_command` path (which adds the repo's Bedrock model resolution).
fn non_claude_harness(name: &str) -> Option<harness::Harness> {
    match name {
        "cursor" => Some(harness::Harness::Cursor),
        "codex" => Some(harness::Harness::Codex),
        "opencode" => Some(harness::Harness::OpenCode),
        "commandcode" => Some(harness::Harness::CommandCode),
        "pi" => Some(harness::Harness::Pi),
        // Grok was silently missing here → fell through to the claude fallback, so a Grok
        // worker dispatched by the flywheel actually ran as a claude worker (wrong binary,
        // wrong argv, wrong prompt delivery). The harness variant exists in worker_spawn
        // (grok agent --cwd …) — this mapping just wasn't wired.
        "grok" => Some(harness::Harness::Grok),
        _ => None,
    }
}

/// Dispatch a worker invocation by harness name.
///
/// claude (and any unknown harness → claude fallback) → the CLI's `claude_worker_command`, which
/// resolves the model through the repo's Bedrock alias map (`crate::model::resolve_headless_model`)
/// and feeds the prompt on STDIN. cursor/codex/opencode/commandcode → the **single-source** worker
/// argv from the shared `harness` crate (`harness::worker_spawn` — the supervisor's canonical builder;
/// no more hand-ported drift), `extra_dirs = &[]` (the controller writes the fan-in artifacts, not the
/// worker), the prompt appended as a trailing positional, model passed VERBATIM.
///
/// EVERY path applies the CLI's `apply_git_deny_env` for the deny boundary — `harness::worker_spawn`
/// returns program+args ONLY (no env), so the CLI owns the env, which is what preserves the CLI's
/// extra `GITHUB_TOKEN`/`GH_TOKEN` scrub (the security-review fix) on top of the ssh/askpass deny.
pub fn worker_command(
    harness_name: &str,
    repo: &Path,
    model: Option<&str>,
    write_mode: bool,
    worktree: &Path,
) -> WorkerSpec {
    let Some(h) = non_claude_harness(harness_name) else {
        return WorkerSpec {
            cmd: claude_worker_command(repo, model, write_mode),
            prompt_via_stdin: true,
            // claude delivers on stdin → `--` gating never applies; false is the honest default.
            end_of_options_supported: harness::supports_end_of_options(harness::Harness::Claude),
        };
    };
    let spawn = harness::worker_spawn(h, model, write_mode, &[], worktree);
    let mut cmd = Command::new(&spawn.program);
    cmd.args(&spawn.args);
    apply_git_deny_env(&mut cmd); // CLI deny env (ssh + GITHUB_TOKEN/GH_TOKEN scrub) — the sole barrier.
    WorkerSpec {
        cmd,
        prompt_via_stdin: spawn.prompt_via_stdin,
        // SSOT: the shared harness crate owns whether `--` is safe for this harness (false today).
        end_of_options_supported: harness::supports_end_of_options(h),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_deny_env_blocks_push_keeps_commit() {
        let env = worker_git_deny_env();
        // push blocked: ssh transport + askpass both forced to false.
        assert!(env
            .iter()
            .any(|(k, v)| *k == "GIT_SSH_COMMAND" && v == "/usr/bin/false"));
        assert!(env
            .iter()
            .any(|(k, v)| *k == "GIT_ASKPASS" && v == "/usr/bin/false"));
        // commit preserved: synthetic identity present (GIT_CONFIG_GLOBAL=/dev/null drops user.name).
        assert!(env.iter().any(|(k, _)| *k == "GIT_AUTHOR_NAME"));
        assert!(env
            .iter()
            .any(|(k, v)| *k == "GIT_CONFIG_GLOBAL" && v == "/dev/null"));
    }

    #[test]
    fn write_mode_adds_commit_but_never_push() {
        let ro = claude_worker_args(false);
        let rw = claude_worker_args(true);
        // push always denied
        assert!(ro.iter().any(|a| a == "Bash(git push:*)"));
        assert!(rw.iter().any(|a| a == "Bash(git push:*)"));
        // commit only in write mode
        assert!(!ro.iter().any(|a| a == "Bash(git commit:*)"));
        assert!(rw.iter().any(|a| a == "Bash(git commit:*)"));
        // --allowedTools is present and precedes the tool list (variadic, last flag)
        assert!(rw.iter().any(|a| a == "--allowedTools"));
    }

    // ===================== P4 multi-harness hermetic argv/env tests =====================
    // All pure: introspect the built Command via get_program/get_args/get_envs — no live binary.

    /// Collect a Command's args as owned Strings (lossy) for ordering/pair assertions.
    fn argv(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }
    /// Find the value that immediately follows `flag` in the argv (the paired positional).
    fn after<'a>(args: &'a [String], flag: &str) -> Option<&'a String> {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
    }
    // NOTE: per-harness argv shape (cursor/codex/opencode/commandcode) is asserted in the
    // shared `harness` crate now (its own 8 unit tests) — the single source of truth. The CLI
    // keeps only the integration assertions below (deny-env on every non-claude Command, the
    // prompt-delivery contract, the claude byte-identical regression).

    #[test]
    fn non_claude_workers_carry_git_deny_env_on_every_command() {
        // The load-bearing security assertion: each non-claude builder must apply the FULL deny env
        // SET (by EQUALITY against worker_git_deny_env) + remove SSH_AUTH_SOCK / GITHUB_TOKEN /
        // GH_TOKEN — the SOLE structural push/exfil barrier for harnesses with no tool allowlist.
        let wt = Path::new("/tmp/ade-wt/w0");
        let repo = Path::new("/tmp/ade-repo");
        // Build each non-claude worker via the REAL dispatch path (worker_command → harness::worker_spawn
        // + the CLI's apply_git_deny_env). The deny env is the CLI's responsibility (harness returns
        // program+args only), so this guards that the adapter still applies it on every harness.
        let builders: Vec<(&str, Command)> =
            // grok was silently missing from non_claude_harness (BUG-1) → was never built here,
            // so its deny env was untested. Added alongside pi.
            ["cursor", "codex", "opencode", "commandcode", "pi", "grok"]
                .into_iter()
                .map(|h| (h, worker_command(h, repo, Some("m"), true, wt).cmd))
                .collect();
        let expected: Vec<(String, Option<String>)> = worker_git_deny_env()
            .into_iter()
            .map(|(k, v)| (k.to_string(), Some(v)))
            .chain(
                ["SSH_AUTH_SOCK", "GITHUB_TOKEN", "GH_TOKEN"]
                    .into_iter()
                    .map(|k| (k.to_string(), None)),
            )
            .collect();
        for (name, cmd) in &builders {
            let envs: std::collections::HashMap<String, Option<String>> = cmd
                .get_envs()
                .map(|(k, v)| {
                    (
                        k.to_string_lossy().into_owned(),
                        v.map(|x| x.to_string_lossy().into_owned()),
                    )
                })
                .collect();
            for (k, want) in &expected {
                assert_eq!(
                    envs.get(k),
                    Some(want),
                    "{name} builder must carry deny env {k}={want:?} (forgot apply_git_deny_env?)"
                );
            }
        }
    }

    #[test]
    fn prompt_delivery_is_stdin_for_claude_positional_for_others() {
        let repo = Path::new("/tmp/ade-repo");
        let wt = Path::new("/tmp/ade-wt/w0");
        assert!(worker_command("claude", repo, None, false, wt).prompt_via_stdin);
        assert!(!worker_command("cursor", repo, None, false, wt).prompt_via_stdin);
        assert!(!worker_command("codex", repo, None, false, wt).prompt_via_stdin);
        assert!(!worker_command("opencode", repo, None, false, wt).prompt_via_stdin);
        assert!(!worker_command("commandcode", repo, None, false, wt).prompt_via_stdin);
        // unknown harness → claude fallback → stdin (preserves today's behavior)
        assert!(worker_command("unknown-harness", repo, None, false, wt).prompt_via_stdin);
    }

    #[test]
    fn claude_worker_command_unchanged_regression_in_new_dispatch() {
        // worker_command("claude", ...) must be argv-byte-identical to claude_worker_command(...).
        let repo = Path::new("/tmp/ade-repo");
        let wt = Path::new("/tmp/ade-wt/w0");
        for write in [false, true] {
            let direct = claude_worker_command(repo, None, write);
            let spec = worker_command("claude", repo, None, write, wt);
            assert_eq!(
                spec.cmd.get_program(),
                direct.get_program(),
                "claude program unchanged"
            );
            assert_eq!(
                argv(&spec.cmd),
                argv(&direct),
                "claude argv unchanged (write={write})"
            );
            // pin the load-bearing claude tokens
            let a = argv(&direct);
            assert_eq!(
                &a[0..5],
                &[
                    "-p",
                    "--output-format",
                    "stream-json",
                    "--verbose",
                    "--no-session-persistence"
                ]
            );
            assert_eq!(
                after(&a, "--disallowedTools").map(|s| s.as_str()),
                Some("Bash(git push:*)")
            );
            // --allowedTools is the last FLAG (variadic, tools follow it to the end)
            let allowed_idx = a
                .iter()
                .position(|x| x == "--allowedTools")
                .expect("--allowedTools present");
            assert!(allowed_idx < a.len() - 1, "tools follow --allowedTools");
            if write {
                assert!(a.iter().any(|x| x == "Bash(git commit:*)"));
                assert!(a.iter().any(|x| x == "Bash(git add:*)"));
            } else {
                assert!(!a.iter().any(|x| x == "Bash(git commit:*)"));
            }
        }
    }
}
